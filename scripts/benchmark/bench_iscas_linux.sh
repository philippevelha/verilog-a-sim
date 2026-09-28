# =============================================================================================
# verilog-a-sim — the full ISCAS'85 suite, LINUX (x86-64)
# =============================================================================================
#
# What it does: runs every ISCAS'85 circuit at transistor level (PSP103) with the compiled
# `va-cli` from a release archive — c17, then c432 ... c7552 as operating points, and c17's
# 12 ns transient — and writes `iscas_suite.txt` holding, per deck, the wall time, exit code,
# SHA-256 of the output and a logic check: `check_iscas.py` compares every reported gate output
# with the `.bench` netlist's logic for the vector the deck was generated for (read back from
# the deck's DC sources). c17 and c432_tran have no check (c17 has no `.bench`), only a hash.
#
# How to run it:
#   1. Unpack `va-cli-v2.0.0-x86_64-unknown-linux-gnu.tar.gz` and cd into it (as for
#      bench_linux.sh). The logic check needs `python3` (no packages); without it the check
#      column reads n/a and the rest still runs.
#   2. Copy everything from the `bash -s` line down to the final `BENCH` line and paste it into
#      the terminal, inside that directory. Measured on an i7-1185G7 (8 threads) at ~19 min for
#      the operating points; c7552 (268 676 unknowns) needs ~4 GB of RAM. LONG=1 adds c432's
#      vector sequence (`c432_seq`, logic-checked, ~47 min there) and its 1 ns transient
#      (~15 min there).
#   3. Send back `iscas_suite.txt`; per-deck outputs, stderr and check reports are in
#      `iscas-out/`.
# =============================================================================================

LONG=0 bash -s <<'BENCH'
set -u
VA=./va-cli
OUT=iscas_suite.txt
D=circuits/benchmark/iscas85
PSP=benchmark/models/psp103/psp103.va
[ -x "$VA" ] && [ -d "$D" ] && [ -f "$PSP" ] || {
  echo "Run this inside the unpacked va-cli archive directory (va-cli, circuits/, benchmark/)."; exit 1; }
PY=$(command -v python3 || true)
mkdir -p iscas-out

# The input vector an .op deck was generated for, read back from its DC sources in .bench INPUT order.
vector_of() {  # vector_of <circuit>
  "$PY" - "$D/$1.bench" "$D/$1.net" <<'PYEOF'
import re, sys
sys.path.insert(0, sys.argv[1].rsplit('/', 1)[0])
from gen_iscas import read_bench, net_name
ins, _, _ = read_bench(sys.argv[1])
dc = {m.group(1): float(m.group(2)) for m in
      re.finditer(r'^V\S+\s+(\S+)_0\s+gnd\s+DC\s+(\S+)', open(sys.argv[2]).read(), re.M)}
print(''.join('1' if dc[net_name(s)] > 0.9 else '0' for s in ins))
PYEOF
}

{
  echo "verilog-a-sim ISCAS'85 full suite — Linux"
  echo "date (UTC):   $(date -u '+%Y-%m-%d %H:%M:%S')"
  echo "archive dir:  $(basename "$PWD")"
  echo "va-cli sha256: $(sha256sum "$VA" | cut -d' ' -f1)"
  echo "cpu:          $(lscpu 2>/dev/null | sed -n 's/^Model name: *//p' | head -1)"
  echo "logical cpus: $(nproc)"
  echo "memory:       $(awk '/MemTotal/ {printf "%.1f GiB", $2/1048576}' /proc/meminfo)"
  echo "governor:     $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>/dev/null || echo n/a)"
  echo "logic check:  ${PY:-SKIPPED (no python3)}"
  echo
  printf '%-26s %9s %5s  %-64s  %s\n' "deck" "seconds" "exit" "sha256(stdout)" "logic"
} > "$OUT"

run() {  # run <label> <deck> <check: op|tran|none> <va-cli extra args...>
  label=$1; deck=$2; check=$3; shift 3
  f="iscas-out/$label.out"
  t0=$(date +%s.%N)
  "$VA" sim "$D/$deck.net" --model "$PSP" "$@" < /dev/null > "$f" 2> "$f.err"; code=$?
  t1=$(date +%s.%N)
  secs=$(awk -v a="$t0" -v b="$t1" 'BEGIN { printf "%.2f", b - a }')
  logic="n/a"
  if [ -n "$PY" ] && [ "$code" = 0 ]; then
    bench="$D/${deck%%_*}.bench"
    case "$check" in
      op)   "$PY" "$D/check_iscas.py" "$bench" "$(vector_of "$deck")" "$f" > "$f.check" 2>&1 ;;
      tran) "$PY" "$D/check_iscas.py" tran "$bench" "$f" > "$f.check" 2>&1 ;;
      *)    false ;;
    esac && logic="PASS: $(tail -1 "$f.check")" || { [ "$check" = none ] || logic="FAIL (see $f.check)"; }
  fi
  printf '%-26s %9s %5s  %-64s  %s\n' "$label" "$secs" "$code" "$(sha256sum "$f" | cut -d' ' -f1)" "$logic" >> "$OUT"
  echo "  $label: $secs s (exit $code) $logic"
}

echo "Running the ISCAS'85 suite; results go to $OUT"
run c17        c17   none
for c in c432 c499 c880 c1355 c1908 c2670 c3540 c5315 c6288 c7552; do
  run "$c" "$c" op
done
run c17_tran   c17_tran none --tran
if [ "$LONG" = 1 ]; then
  REP=$("$PY" "$D/check_iscas.py" report "$D/c432.bench")
  run c432_seq   c432_seq tran --tran --report "$REP"
  run c432_tran  c432_tran none --tran
fi
echo "== end $(date -u '+%H:%M:%S') UTC" >> "$OUT"
echo
cat "$OUT"
echo
echo "Done. Send back: $PWD/$OUT"
BENCH
