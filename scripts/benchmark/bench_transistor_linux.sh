#!/usr/bin/env bash
# =============================================================================================
# verilog-a-sim — the SNN neuron with ideal switches vs real access transistors, LINUX (x86-64)
# =============================================================================================
#
# What it does: runs the Fig. 5 single-neuron deck of the Xuan et al. (IJCNN 2026) SNN twice —
#   - circuits/snn/fig5_neuron.net             ideal access switches (vswitch.va, R_on = 10 ohm)
#   - circuits/snn/transistor/fig5_access.net  the 8 access switches as IHP SG13G2 HV NMOS
#                                              (PSP103, W/L = 10/0.45 um, typical corner)
# at the default thread count and on one thread, and writes `bench_transistor.txt`:
#   - the machine and the va-cli binary's SHA-256;
#   - per run: the wall time of va-cli alone, the exit code, unknowns, timepoints, the SHA-256 of
#     the output (should match another machine's: output is bit-identical across platforms and
#     thread counts), and check_snn.py's verdict (output spike steps, worst V_m error).
# Expected: both decks spike at steps 6, 10, 14; the transistor deck's V_m departs ~0.13 V from
# the paper's equations (R_on = 377 ohm, not an error — check_snn's FAIL is its 0.05 V tolerance,
# written for ideal switches). On an i7-1185G7 (Windows, 2.3.0): 4.0 s vs 64.3 s (8 threads),
# 115.5 s for the transistor deck on 1 thread.
#
# What it needs: a checkout at or after 2.3.0 (repeatable --model), and a va-cli built from it:
#     git clone https://github.com/philippevelha/verilog-a-sim.git && cd verilog-a-sim
#     cargo build --release -p va-cli
#     bash scripts/benchmark/bench_transistor_linux.sh target/release/va-cli
# python3 (standard library only) for the check. The SG13G2 cards are committed
# (circuits/snn/transistor/tech.inc); the PDK checkout is not needed.
# Environment: RUNS="beh acc acc1" picks runs (acc1 = transistor deck on one thread);
#              EXTRA_ENV="VAR=value ..." is passed to every va-cli run (e.g. a simulator switch).
# Send back bench_transistor.txt.
# =============================================================================================
set -u
VA=${1:?usage: bench_transistor_linux.sh /path/to/va-cli}
OUT=bench_transistor.txt
RUNS=${RUNS:-beh acc acc1}
[ -x "$VA" ] || { echo "not an executable: $VA"; exit 1; }
[ -f circuits/snn/transistor/fig5_access.net ] || {
  echo "Run this from the root of a verilog-a-sim checkout at or after 2.3.0."; exit 1; }
PY=${PYTHON:-python3}
command -v "$PY" >/dev/null || { echo "python3 is required (or set PYTHON)"; exit 1; }
mkdir -p transistor-out

{
  echo "verilog-a-sim — SNN Fig. 5 neuron: ideal switches vs SG13G2 access transistors — Linux"
  echo "date (UTC):    $(date -u '+%Y-%m-%d %H:%M:%S')"
  echo "checkout:      $(git rev-parse --short HEAD 2>/dev/null || echo n/a) $(git log -1 --format=%s 2>/dev/null | cut -c1-60)"
  echo "va-cli:        $VA"
  echo "va-cli sha256: $(sha256sum "$VA" | cut -d' ' -f1)"
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
  case $r in
    beh)  deck=circuits/snn/fig5_neuron.net; models=(--model models/snn); thr=() ;;
    acc)  deck=circuits/snn/transistor/fig5_access.net
          models=(--model models/snn --model models/sg13g2/psp103/psp103.va); thr=() ;;
    acc1) deck=circuits/snn/transistor/fig5_access.net
          models=(--model models/snn --model models/sg13g2/psp103/psp103.va); thr=(--threads 1) ;;
    *) echo "unknown run $r" | tee -a "$OUT"; continue ;;
  esac
  f=transistor-out/$r.out
  echo "$r: simulating $deck ${thr[*]:-} ..."
  t0=$(date +%s.%N)
  env ${EXTRA_ENV:-} "$VA" sim "$deck" "${models[@]}" "${thr[@]}" --tran \
      --report clk,wl1,vm,spk > "$f" 2> "$f.err"
  rc=$?
  t1=$(date +%s.%N)
  wall=$("$PY" -c "print(f'{$t1-$t0:.2f}')")
  pts=$(sed -n 's/^Transient analysis (\([0-9]*\) points.*/\1/p' "$f")
  unk=$(sed -n 's/.*circuit: .* \([0-9]*\) unknown(s).*/\1/p' "$f.err" | head -1)
  chk=$("$PY" circuits/snn/check_snn.py fig5 "$f" 2>&1 | grep -E "output spikes, equations|circuit vs equations|PASS|FAIL" | tr '\n' ';')
  {
    echo "$r  deck=$deck threads=${thr[*]:-default}"
    echo "    exit=$rc wall=${wall}s unknowns=${unk:-?} timepoints=${pts:-?} sha256=$(sha256sum "$f" | cut -c1-16)"
    echo "    check: $chk"
  } >> "$OUT"
  echo "$r: exit $rc, ${wall} s"
done
echo >> "$OUT"
echo "done; results in $OUT"
