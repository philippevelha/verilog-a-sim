#!/usr/bin/env bash
# =============================================================================================
# verilog-a-sim — MNIST through the memristor-crossbar SNN, LINUX (x86-64)
# =============================================================================================
#
# What it does: puts the ten MNIST samples of circuits/snn/mnist/ (one per digit, held out from
# training) through the three circuit layers of the Xuan et al. (IJCNN 2026) SNN — hidden 2,
# hidden 3 and the output layer as memristor crossbars and analog neurons, 10 973 devices and
# 2 626 unknowns per sample — and writes `bench_snn.txt`:
#   - the machine (CPU, cores, memory, OS, governor, power) and the va-cli binary's SHA-256;
#   - per sample: **the wall time of the va-cli run alone** (deck generation and the check are
#     timed separately and not included), the timepoint count, the exit code, the SHA-256 of the
#     output, and the check against the quantized model: spike agreement per layer, output
#     spike counts, predicted class, label.
# The output hashes should match another machine's: since 2.0.0 the output is bit-identical
# across platforms and thread counts.
#
# What it needs:
#   - a checkout of the repository at (or after) the commit that added circuits/snn/mnist/:
#         git clone https://github.com/philippevelha/verilog-a-sim.git && cd verilog-a-sim
#     (the decks, models/snn/ and the trained weights model.json are not in the v2.0.0 archive);
#   - a va-cli executable: the one from the v2.0.0 release archive works (the SNN decks use only
#     constructs 2.0.0 has), or `cargo build --release` in the checkout (target/release/va-cli);
#   - python3 (standard library only) for generating the decks and checking the output.
#   No MNIST data is needed: the ten samples' hidden-1 spike trains are in model.json.
#
# How to run it (from the repository root):
#     bash scripts/benchmark/bench_snn_linux.sh /path/to/va-cli              # all ten
#     bash scripts/benchmark/bench_snn_linux.sh /path/to/va-cli 0 3 7        # a subset
#     THREADS=1 bash scripts/benchmark/bench_snn_linux.sh /path/to/va-cli 0  # pin the threads
#     KEEP=1 ...   keeps each sample's raw output in snn-out/ (~0.2 GB of text each)
# Cost, measured on an i7-1185G7 (4 cores / 8 threads, Windows 11, mains, 2.1.0): digit 0 took
# 3 762 s (63 min) of va-cli wall time, 63 214 timepoints, >1.2 GB of memory (the waveform is held
# in memory); so ~10 h for all ten there. Start with one digit (`... /path/to/va-cli 0`).
# For a clean baseline: mains power, nothing else running. Send back bench_snn.txt.
# =============================================================================================
set -u
VA=${1:?usage: bench_snn_linux.sh /path/to/va-cli [digit ...]}
shift
DIGITS=${*:-0 1 2 3 4 5 6 7 8 9}
OUT=bench_snn.txt
[ -x "$VA" ] || { echo "not an executable: $VA"; exit 1; }
[ -f circuits/snn/mnist/model.json ] && [ -d models/snn ] || {
  echo "Run this from the root of a verilog-a-sim checkout that has circuits/snn/mnist/."; exit 1; }
command -v python3 >/dev/null || { echo "python3 is required"; exit 1; }
mkdir -p snn-out
THREAD_ARGS=()
[ -n "${THREADS:-}" ] && THREAD_ARGS=(--threads "$THREADS")
REPORT=$(python3 circuits/snn/mnist/check_mnist.py report)

{
  echo "verilog-a-sim — MNIST SNN benchmark (Xuan et al., IJCNN 2026) — Linux"
  echo "date (UTC):    $(date -u '+%Y-%m-%d %H:%M:%S')"
  echo "checkout:      $(git rev-parse --short HEAD 2>/dev/null || echo n/a) $(git log -1 --format=%s 2>/dev/null | cut -c1-60)"
  echo "va-cli:        $VA"
  echo "va-cli sha256: $(sha256sum "$VA" | cut -d' ' -f1)"
  echo "threads:       ${THREADS:-default (one per logical CPU)}"
  echo
  echo "== machine"
  echo "os:           $(. /etc/os-release 2>/dev/null && echo "$PRETTY_NAME")"
  echo "kernel:       $(uname -srm)"
  echo "cpu:          $(lscpu 2>/dev/null | sed -n 's/^Model name: *//p' | head -1)"
  echo "cores/socket: $(lscpu 2>/dev/null | sed -n 's/^Core(s) per socket: *//p')  threads/core: $(lscpu 2>/dev/null | sed -n 's/^Thread(s) per core: *//p')"
  echo "logical cpus: $(nproc)"
  echo "memory:       $(awk '/MemTotal/ {printf "%.1f GiB", $2/1048576}' /proc/meminfo)"
  echo "governor:     $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>/dev/null || echo n/a)"
  ac=$(cat /sys/class/power_supply/A*/online 2>/dev/null | head -1)
  case "$ac" in 1) echo "power:        mains";; 0) echo "power:        BATTERY (timings not a baseline)";; *) echo "power:        n/a (desktop or VM)";; esac
  echo "load avg:     $(cut -d' ' -f1-3 /proc/loadavg) (before the run)"
  echo
  echo "== samples (wall time = the va-cli run alone)"
} > "$OUT"

total=0
for d in $DIGITS; do
  deck=circuits/snn/mnist/decks/digit$d.net
  python3 circuits/snn/mnist/gen_mnist_deck.py "$d" > /dev/null || { echo "digit $d: deck generation failed" | tee -a "$OUT"; continue; }
  f=snn-out/digit$d.out
  echo "digit $d: simulating ($(date -u '+%H:%M:%S') UTC) ..."
  t0=$(date +%s.%N)
  "$VA" sim "$deck" --model models/snn --tran --report "$REPORT" "${THREAD_ARGS[@]}" < /dev/null > "$f" 2> "$f.err"; code=$?
  t1=$(date +%s.%N)
  secs=$(awk -v a="$t0" -v b="$t1" 'BEGIN { printf "%.1f", b - a }')
  total=$(awk -v a="$total" -v b="$secs" 'BEGIN { printf "%.1f", a + b }')
  points=$(grep -c '^ *t=' "$f")
  sha=$(sha256sum "$f" | cut -d' ' -f1)
  {
    echo "--- digit $d: va-cli wall ${secs} s, exit $code, $points timepoints"
    echo "    sha256(stdout) $sha"
    grep -h 'circuit:' "$f.err" | sed 's/^\[va-cli\] /    /'
  } >> "$OUT"
  if [ "$code" = 0 ]; then
    python3 circuits/snn/mnist/check_mnist.py "$d" --from "$f" --wall "$secs" | sed 's/^/    /' >> "$OUT"
  else
    tail -5 "$f.err" | sed 's/^/    /' >> "$OUT"
  fi
  echo "  digit $d: ${secs} s (exit $code)"
  [ "${KEEP:-0}" = 1 ] || rm -f "$f"
done
{
  echo
  echo "== total va-cli wall time: ${total} s"
  echo "== end $(date -u '+%H:%M:%S') UTC"
} >> "$OUT"
echo
cat "$OUT"
echo
echo "Done. Send back: $PWD/$OUT"
