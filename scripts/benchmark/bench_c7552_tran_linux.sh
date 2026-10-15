#!/usr/bin/env bash
# =============================================================================================
# verilog-a-sim — ISCAS'85 c7552 transient, LINUX (x86-64): the OpenVAF comparison
# =============================================================================================
#
# What it does: runs circuits/benchmark/iscas85/c7552_tran.net (c7552 at transistor level,
# PSP103, 207 PULSE inputs, `.tran 10p 10n`) twice, with default settings and then with
# `--bypass`. It writes `c7552_tran.txt` with the wall time, peak memory, exit code, timepoint
# count and a logic check (`check_iscas.py tran`) for each run. The reference is ngspice with
# OpenVAF's PSP103: 20 min for ~1 020 timepoints on an i7-9750H (16 GB). That run used a
# different stimulus and netlist (15 882 devices against our 14 942), so compare orders of
# magnitude, not percentages.
#
# How to run it, from the repository root of a checkout:
#   bash scripts/benchmark/bench_c7552_tran_linux.sh
# Environment (all optional):
#   VA=path/to/va-cli   use this binary instead of building target/release/va-cli
#   THREADS=N           worker threads (default: nproc)
#   CAP=SECONDS         stop a run after this long (default 10800, 3 h; 0 = no cap)
#   ONLY=default|bypass run one configuration only
#
# Expect hours: the staggered input edges make the step control accept ~2 500 points or more,
# and an interrupted 4-vCPU Xeon run had not finished after 2 h. Memory: ~4-7 GB. Only the
# 3 719 checked nodes (inputs and gate outputs) are reported: the default report would keep
# all 268 676 unknowns, ~2.15 MB per timepoint, and run out of memory. Output is written only
# when a run finishes, so a run stopped by CAP leaves no waveform, only its time and memory.
# Send back `c7552_tran.txt`; full outputs are in `c7552-out/`.
# =============================================================================================
set -u
D=circuits/benchmark/iscas85
PSP=benchmark/models/psp103/psp103.va
[ -f "$D/c7552_tran.net" ] && [ -f "$PSP" ] || { echo "Run this from the repository root."; exit 1; }
command -v python3 > /dev/null || { echo "python3 is needed (no packages)."; exit 1; }
if [ -z "${VA:-}" ]; then
  cargo build -q --release -p va-cli || exit 1
  VA=target/release/va-cli
fi
THREADS=${THREADS:-$(nproc)} CAP=${CAP:-10800} OUT=c7552_tran.txt
mkdir -p c7552-out
REP=$(python3 "$D/check_iscas.py" report "$D/c7552.bench") || exit 1

{
  echo "verilog-a-sim ISCAS'85 c7552 transient — Linux"
  echo "date (UTC):    $(date -u '+%Y-%m-%d %H:%M:%S')"
  echo "commit:        $(git rev-parse --short HEAD 2>/dev/null || echo n/a)"
  echo "version:       $(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1), $VA"
  echo "cpu:           $(lscpu 2>/dev/null | sed -n 's/^Model name: *//p' | head -1)"
  echo "logical cpus:  $(nproc), --threads $THREADS"
  echo "memory:        $(awk '/MemTotal/ {printf "%.1f GiB", $2/1048576}' /proc/meminfo)"
  echo "governor:      $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>/dev/null || echo n/a)"
  echo "cap:           ${CAP} s per run"
  echo
  printf '%-8s %10s %9s %5s %8s  %s\n' "config" "seconds" "peak MB" "exit" "points" "logic"
} > "$OUT"

run() {  # run <config> <va-cli extra args...>
  local cfg=$1 f="c7552-out/$1.out" res code secs mb pts logic
  shift
  echo "$(date -u +%T) UTC: c7552_tran $cfg started"
  # The child's wall time and peak resident memory, without needing GNU time installed.
  res=$(python3 - "$f" "$CAP" "$VA" sim "$D/c7552_tran.net" --model "$PSP" --tran "$@" \
        --threads "$THREADS" --report "$REP" <<'PYEOF'
import resource, subprocess, sys, time
out, cap, cmd = sys.argv[1], float(sys.argv[2]), sys.argv[3:]
t = time.time()
with open(out, "w") as fh, open(out + ".err", "w") as err:
    try:
        code = subprocess.run(cmd, stdout=fh, stderr=err, timeout=cap or None).returncode
    except subprocess.TimeoutExpired:
        code = "cap"
print(code, f"{time.time() - t:.1f}",
      f"{resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss / 1024:.0f}")
PYEOF
  )
  read -r code secs mb <<< "$res"
  pts=$(grep -o -m1 '([0-9]* points' "$f" | tr -dc 0-9)
  logic=n/a
  if [ "$code" = 0 ]; then
    python3 "$D/check_iscas.py" tran "$D/c7552.bench" "$f" > "$f.check" 2>&1 \
      && logic="PASS: $(tail -1 "$f.check")" || logic="FAIL (see $f.check)"
  fi
  printf '%-8s %10s %9s %5s %8s  %s\n' "$cfg" "$secs" "$mb" "$code" "${pts:-n/a}" "$logic" >> "$OUT"
  echo "$(date -u +%T) UTC: c7552_tran $cfg: $secs s, $mb MB, exit $code, ${pts:-n/a} points, $logic"
}

[ "${ONLY:-default}" = default ] && run default
[ "${ONLY:-bypass}" = bypass ] && run bypass --bypass
echo "== end $(date -u '+%H:%M:%S') UTC" >> "$OUT"
echo
cat "$OUT"
echo
echo "Done. Send back: $PWD/$OUT"
