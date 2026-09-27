# =============================================================================================
# verilog-a-sim — platform benchmark, LINUX (x86-64)
# =============================================================================================
#
# What it does: runs a fixed set of decks with the compiled `va-cli` from a release archive,
# times each one, and writes `benchmark.txt` holding
#   - the machine's specifications (CPU, cores, memory, OS, power source), the baseline for
#     comparing platforms;
#   - the wall time and exit code of every deck;
#   - a SHA-256 of each deck's output. Since 2.0.0 the output is meant to be bit-identical on
#     Linux, macOS and Windows, so equal hashes across the three benchmark.txt files confirm it.
#
# How to run it:
#   1. Download `va-cli-v2.0.0-x86_64-unknown-linux-gnu.tar.gz` from
#      https://github.com/philippevelha/verilog-a-sim/releases and unpack it:
#          tar xzf va-cli-v2.0.0-x86_64-unknown-linux-gnu.tar.gz
#          cd va-cli-v2.0.0-x86_64-unknown-linux-gnu
#      (the directory holds `va-cli`, `circuits/`, `models/` and `benchmark/`).
#   2. For a clean baseline: plug in the charger (a laptop on battery throttles), close other
#      heavy programs, and leave the machine alone while it runs.
#   3. Copy everything from the `bash -s` line down to the final `BENCH` line and paste it into
#      the terminal, inside that directory. It takes about 4 minutes. To also run the two long
#      decks (c432's 1 ns transient, ~15 min; c7552's .op, ~6 min and ~4 GB of RAM), change
#      LONG=0 to LONG=1 on the first line below before pasting.
#   4. Send back `benchmark.txt` (it is written in the same directory). Per-deck outputs are
#      kept in `bench-out/` in case a hash differs and the outputs need comparing.
#
# Timing notes: each figure is one run's wall time, and includes compiling the Verilog-A model
# (tens of ms) and writing the output. Decks run with the default thread count (one per logical
# CPU), plus c432's .op once with `--threads 1`, to show how much the parallel evaluation buys.
# =============================================================================================

LONG=0 bash -s <<'BENCH'
set -u
# The pasted script arrives on this shell's stdin (`bash -s`), so nothing may redirect or read
# stdin here — `exec </dev/null` would silently end the script at that line. Only `va-cli`,
# the one program that could read it, gets `< /dev/null`.
VA=./va-cli
OUT=benchmark.txt
[ -x "$VA" ] && [ -d benchmark ] && [ -d circuits ] || {
  echo "Run this inside the unpacked va-cli archive directory (va-cli, circuits/, benchmark/)."; exit 1; }
mkdir -p bench-out
PSP=benchmark/models/psp103/psp103.va

{
  echo "verilog-a-sim platform benchmark — Linux"
  echo "date (UTC):   $(date -u '+%Y-%m-%d %H:%M:%S')"
  echo "archive dir:  $(basename "$PWD")"
  echo "va-cli sha256: $(sha256sum "$VA" | cut -d' ' -f1)"
  echo
  echo "== machine"
  echo "os:           $(. /etc/os-release 2>/dev/null && echo "$PRETTY_NAME")"
  echo "kernel:       $(uname -srm)"
  echo "cpu:          $(lscpu 2>/dev/null | sed -n 's/^Model name: *//p' | head -1)"
  echo "sockets:      $(lscpu 2>/dev/null | sed -n 's/^Socket(s): *//p')"
  echo "cores/socket: $(lscpu 2>/dev/null | sed -n 's/^Core(s) per socket: *//p')"
  echo "threads/core: $(lscpu 2>/dev/null | sed -n 's/^Thread(s) per core: *//p')"
  echo "logical cpus: $(nproc)"
  echo "max MHz:      $(lscpu 2>/dev/null | sed -n 's/^CPU max MHz: *//p')"
  echo "simd:         $(grep -m1 -o -w -E 'avx512f|avx2|fma' /proc/cpuinfo | sort -u | tr '\n' ' ')"
  echo "memory:       $(awk '/MemTotal/ {printf "%.1f GiB", $2/1048576}' /proc/meminfo)"
  echo "governor:     $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>/dev/null || echo n/a)"
  ac=$(cat /sys/class/power_supply/A*/online 2>/dev/null | head -1)
  case "$ac" in 1) echo "power:        mains";; 0) echo "power:        BATTERY (timings not a baseline)";; *) echo "power:        n/a (no power-supply info; desktop or VM)";; esac
  echo "load avg:     $(cut -d' ' -f1-3 /proc/loadavg) (before the run)"
  echo
  echo "== decks (wall time, one run each)"
  printf '%-48s %8s %5s  %s\n' "deck" "seconds" "exit" "sha256(stdout)"
} > "$OUT"

run() {  # run <label> <va-cli args...>
  label=$1; shift
  f="bench-out/$(echo "$label" | tr '/ ' '__').out"
  t0=$(date +%s.%N)
  "$VA" "$@" < /dev/null > "$f" 2> "$f.err"; code=$?
  t1=$(date +%s.%N)
  secs=$(awk -v a="$t0" -v b="$t1" 'BEGIN { printf "%.2f", b - a }')
  printf '%-48s %8s %5s  %s\n' "$label" "$secs" "$code" "$(sha256sum "$f" | cut -d' ' -f1)" >> "$OUT"
  echo "  $label: $secs s (exit $code)"
}

echo "Running the benchmark; results go to $OUT"
run "rectifier (.tran, diode)"          sim circuits/rectifier.net --model models/diode.va --tran
run "ring_osc (.tran, reference BJTs)"  sim circuits/ring_osc.net --tran
run "nand/psp103 (.op)"                 sim circuits/benchmark/nand/psp103.net --model "$PSP" --report out
run "psp103_inverter_tran (.tran)"      sim circuits/benchmark/psp103_inverter_tran.net --model "$PSP" --tran --report in,out
run "iscas85/c17 (.op)"                 sim circuits/benchmark/iscas85/c17.net --model "$PSP"
run "iscas85/c432 (.op)"                sim circuits/benchmark/iscas85/c432.net --model "$PSP"
run "iscas85/c432 (.op, --threads 1)"   sim circuits/benchmark/iscas85/c432.net --model "$PSP" --threads 1
run "iscas85/c499 (.op)"                sim circuits/benchmark/iscas85/c499.net --model "$PSP"
run "iscas85/c880 (.op)"                sim circuits/benchmark/iscas85/c880.net --model "$PSP"
run "iscas85/c17_tran (.tran 12 ns)"    sim circuits/benchmark/iscas85/c17_tran.net --model "$PSP" --tran
if [ "$LONG" = 1 ]; then
  run "iscas85/c432_tran (.tran 1 ns)"  sim circuits/benchmark/iscas85/c432_tran.net --model "$PSP" --tran
  run "iscas85/c7552 (.op)"             sim circuits/benchmark/iscas85/c7552.net --model "$PSP"
fi
echo "== end $(date -u '+%H:%M:%S') UTC" >> "$OUT"
echo
cat "$OUT"
echo
echo "Done. Send back: $PWD/$OUT"
BENCH
