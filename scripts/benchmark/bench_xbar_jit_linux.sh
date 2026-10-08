#!/usr/bin/env bash
# =============================================================================================
# verilog-a-sim — a small MNIST crossbar with SG13G2 transistors: interpreter vs JIT, with and
# without device bypass, LINUX (x86-64)
# =============================================================================================
#
# What it does: generates a small slice of the transistor-level MNIST network: hidden layer 2's
# first NEURONS neurons (behavioural blocks) and their 64-input 2T2R crossbar, every access
# switch an IHP SG13G2 HV NMOS through PSP103 (scripts/benchmark/gen_xbar_deck.py, from
# circuits/snn/transistor/gen_mnist_tx.py). With the defaults that is 1 024 PSP103 and 17 710
# unknowns, for CYCLES 250 ns cycles. It then runs the deck in each configuration:
#   plain      the interpreted static program (the default)
#   jit        VA_JIT=on: each model's program compiled to machine code (Cranelift, 2.18.0+)
#   bp         --bypass: device bypass (2.7.0+)
#   jit_bp     both
# at the default thread count, and the bypass ones on one thread too (`_t1`), and writes
# `bench_xbar_jit.txt`:
#   - the machine and the va-cli binary's SHA-256;
#   - per run: wall time of va-cli alone, peak memory, exit code, timepoints, the SHA-256 of the
#     output, and for bypass runs the replay statistics;
#   - the checks: `jit` must hash-equal `plain`, and `jit_bp` must hash-equal `bp` (the JIT is
#     bit-identical); every bypass run must hash-equal every other bypass run (thread count changes
#     no bit). `bp` against `plain` is NOT identical by design: the script reports the largest
#     difference of the 8 membrane voltages at the timepoints both runs share, and how many there
#     are.
#
# Reference (2.18.0+1, Xeon 2.8 GHz, 4 vCPU, the defaults, 2 234 timepoints): plain 108.6 s,
# jit 72.8 s, bp 30.3 s, jit_bp 29.8 s; on one thread bp 46.2 s, jit_bp 38.6 s. Bypass replayed
# 93.4 % of loads; every check OK; bypass moved V_m by at most 1.0e-14 V. Peak memory ~0.6 GB.
#
# Reference (2.18.0+4, Ryzen 9 9950X, 32 logical CPUs, NEURONS=32 CYCLES=5: 4 096 PSP103, 70 366
# unknowns, 5 141 timepoints): plain 619.0 s, jit 615.4 s, bp 257.7 s, jit_bp 255.8 s; on one
# thread bp 355.7 s, jit_bp 323.4 s. Bypass replayed 95.3 % of loads; every check OK; bypass moved
# V_m by at most 1.8e-5 V. Peak memory ~3.9 GB. At this size the JIT buys under 1 % (10 % on one
# thread): device evaluation is no longer what the run waits on.
#
# What it needs: a checkout at or after 2.18.0 and a va-cli built from it:
#     cargo build --release -p va-cli
#     bash scripts/benchmark/bench_xbar_jit_linux.sh target/release/va-cli
# and python3 (standard library only). No PDK: the one card the deck uses is committed.
# Environment:
#   NEURONS=8      hidden-2 neurons kept (the crossbar has 64 x NEURONS synapses, 2 PSP103 each)
#   CYCLES=3       250 ns cycles simulated (the first carries no spikes yet; 3 shows two)
#   DIGIT=0        which held-out image's spike trains drive the crossbar
#   RUNS="..."     default: "plain jit bp jit_bp bp_t1 jit_bp_t1"; add plain_t1 / jit_t1 if wanted
#   EXTRA_ENV="VAR=value ..." passed to every va-cli run
# Send back bench_xbar_jit.txt.
# =============================================================================================
set -u
VA=${1:?usage: bench_xbar_jit_linux.sh /path/to/va-cli}
OUT=bench_xbar_jit.txt
NEURONS=${NEURONS:-8}
CYCLES=${CYCLES:-3}
DIGIT=${DIGIT:-0}
RUNS=${RUNS:-plain jit bp jit_bp bp_t1 jit_bp_t1}
[ -x "$VA" ] || { echo "not an executable: $VA"; exit 1; }
[ -f scripts/benchmark/gen_xbar_deck.py ] || {
  echo "Run this from the root of a verilog-a-sim checkout at or after 2.18.0."; exit 1; }
PY=${PYTHON:-python3}
command -v "$PY" >/dev/null || { echo "python3 is required (or set PYTHON)"; exit 1; }
VA=$(cd "$(dirname "$VA")" && pwd)/$(basename "$VA")
W=xbar-out
rm -rf "$W"; mkdir -p "$W"
"$PY" scripts/benchmark/gen_xbar_deck.py "$W/deck" --neurons "$NEURONS" --cycles "$CYCLES" \
  --digit "$DIGIT" > "$W/gen.log" 2>&1 || { cat "$W/gen.log"; exit 1; }
DECK=$W/deck/d/xbar.net
REPORT=$(seq -s, -f "XN_h2_%g.vm" 0 $((NEURONS - 1)))

{
  echo "verilog-a-sim — small MNIST crossbar (SG13G2 PSP103): interpreter vs JIT, bypass — Linux"
  echo "date (UTC):    $(date -u '+%Y-%m-%d %H:%M:%S')"
  echo "checkout:      $(git rev-parse --short HEAD 2>/dev/null || echo n/a) $(git log -1 --format=%s 2>/dev/null | cut -c1-60)"
  echo "va-cli:        $VA"
  echo "va-cli sha256: $(sha256sum "$VA" | cut -d' ' -f1)"
  echo "deck:          hidden-2 crossbar, digit $DIGIT, $NEURONS neurons, $CYCLES cycles ($((128 * NEURONS)) PSP103)"
  echo "extra env:     ${EXTRA_ENV:-none}"
  echo
  echo "== machine"
  echo "os:           $(. /etc/os-release 2>/dev/null && echo "$PRETTY_NAME")"
  echo "kernel:       $(uname -srm)"
  echo "cpu:          $(lscpu 2>/dev/null | sed -n 's/^Model name: *//p' | head -1)"
  echo "logical cpus: $(nproc)"
  echo "memory:       $(awk '/MemTotal/ {printf "%.1f GiB", $2/1048576}' /proc/meminfo)"
  echo "governor:     $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>/dev/null || echo n/a)"
  echo "load avg:     $(cut -d' ' -f1-3 /proc/loadavg) (before the runs)"
  echo
  echo "== runs (wall time = the va-cli run alone)"
} > "$OUT"

for r in $RUNS; do
  env_=(); args=()
  case $r in *jit*) env_+=(VA_JIT=on) ;; *) env_+=(VA_JIT=off) ;; esac
  case $r in *bp*) args+=(--bypass) ;; esac
  case $r in *_t1) args+=(--threads 1) ;; esac
  case $r in plain|jit|bp|jit_bp|plain_t1|jit_t1|bp_t1|jit_bp_t1) ;;
    *) echo "unknown run $r" | tee -a "$OUT"; continue ;; esac
  f=$W/$r.out
  echo "--- $r: ${env_[*]} ${args[*]}" | tee -a "$OUT"
  # Wall time and peak memory of va-cli alone, measured by python (no /usr/bin/time needed).
  stats=$(env ${EXTRA_ENV:-} "${env_[@]}" "$PY" -c '
import resource, subprocess, sys, time
out, err = open(sys.argv[1], "w"), open(sys.argv[2], "w")
t0 = time.time()
code = subprocess.run(sys.argv[3:], stdout=out, stderr=err).returncode
dt = time.time() - t0
print(f"{dt:.1f} {code} {resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss / 1024:.0f}")
' "$f" "$f.err" "$VA" sim "$DECK" --model models/snn --model models/sg13g2/psp103/psp103.va \
    --tran --report "$REPORT" "${args[@]}")
  read -r wall code rss <<< "$stats"
  pts=$(grep -c '^  t=' "$f")
  hash=$(grep -v 'BYPASS' "$f" | sha256sum | cut -c1-16)
  echo "$r $hash" >> "$W/hashes"
  {
    echo "    wall ${wall} s, exit $code, peak ${rss} MB, $pts timepoints, sha256(stdout) $hash"
    grep 'circuit:' "$f.err" | head -1 | sed 's/^\[va-cli\] /    /'
    grep 'BYPASS.*replayed' "$f.err" | head -1 | sed 's/^\*\*\*\*\* BYPASS \[ACTIVE\] \*\*\*\*\* /    bypass: /'
    [ "$code" = 0 ] || { echo "    stderr tail:"; tail -5 "$f.err" | sed 's/^/      /'; }
  } | tee -a "$OUT"
done

{
  echo
  echo "== checks"
  "$PY" - "$W" <<'EOF'
import os, re, sys
w = sys.argv[1]
h = dict(l.split() for l in open(os.path.join(w, "hashes")))
def same(a, b, what):
    if a in h and b in h:
        print(f"{'OK  ' if h[a] == h[b] else 'DIFF'} {a} vs {b}: {what}")
same("plain", "jit", "the JIT is bit-identical")
same("plain_t1", "jit_t1", "the JIT is bit-identical")
same("bp", "jit_bp", "the JIT is bit-identical under bypass")
same("bp", "bp_t1", "bypass gives the same bits on any thread count")
same("jit_bp", "jit_bp_t1", "same, with the JIT")
same("plain", "plain_t1", "thread count changes no bit")
def read(name):
    p = os.path.join(w, name + ".out")
    if not os.path.exists(p):
        return None
    rows = {}
    for line in open(p):
        if line.startswith("  t="):
            t = line.split()[0][2:-1]
            rows[t] = [float(x.split("=")[1]) for x in re.findall(r"=\S+", line)[1:]]
    return rows
a, b = read("plain"), read("bp")
if a and b:
    shared = [t for t in a if t in b]
    worst = max((abs(x - y) for t in shared for x, y in zip(a[t], b[t])), default=float("nan"))
    print(f"bypass vs plain (not identical by design): {len(shared)} shared timepoints of "
          f"{len(a)} / {len(b)}; largest |delta V_m| there {worst:.3e} V")
EOF
} | tee -a "$OUT"
echo
echo "Done: send back $OUT (outputs are in $W/)."
