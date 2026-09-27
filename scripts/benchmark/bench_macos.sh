# =============================================================================================
# verilog-a-sim — platform benchmark, macOS (Apple silicon)
# =============================================================================================
#
# What it does: runs a fixed set of decks with the compiled `va-cli` from a release archive,
# times each one, and writes `benchmark.txt` holding
#   - the machine's specifications (chip, performance/efficiency cores, memory, macOS version,
#     power source), the baseline for comparing platforms;
#   - the wall time and exit code of every deck;
#   - a SHA-256 of each deck's output. Since 2.0.0 the output is meant to be bit-identical on
#     Linux, macOS and Windows, so equal hashes across the three benchmark.txt files confirm it.
#
# How to run it:
#   1. Download `va-cli-v2.0.0-aarch64-apple-darwin.tar.gz` from
#      https://github.com/philippevelha/verilog-a-sim/releases and, in Terminal:
#          cd ~/Downloads
#          tar xzf va-cli-v2.0.0-aarch64-apple-darwin.tar.gz
#          cd va-cli-v2.0.0-aarch64-apple-darwin
#          xattr -dr com.apple.quarantine .
#      The last line is needed once: the binary is not notarised, and without it macOS refuses
#      to run a downloaded executable ("cannot be opened because the developer cannot be
#      verified").
#   2. For a clean baseline: plug in the charger and turn Low Power Mode off (both change the
#      clock), close other heavy programs, and leave the machine alone while it runs.
#   3. Copy everything from the `bash -s` line down to the final `BENCH` line and paste it into
#      Terminal, inside that directory. Copy only that part: zsh, the default shell, does not
#      accept the `#` comment lines above when they are pasted. It takes about 4 minutes. To also
#      run the two long decks (c432's 1 ns transient, ~15 min; c7552's .op, ~6 min and ~4 GB of
#      RAM), change LONG=0 to LONG=1 on that first line before pasting.
#   4. Send back `benchmark.txt` (it is written in the same directory). Per-deck outputs are
#      kept in `bench-out/` in case a hash differs and the outputs need comparing.
#
# Timing notes: each figure is one run's wall time (bash's `time`, 1 ms resolution), and
# includes compiling the Verilog-A model (tens of ms) and writing the output. Decks run with
# the default thread count (one per logical CPU, performance and efficiency cores alike), plus
# c432's .op once with `--threads 1`, to show how much the parallel evaluation buys.
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
TIMEFORMAT=%R

{
  echo "verilog-a-sim platform benchmark — macOS"
  echo "date (UTC):   $(date -u '+%Y-%m-%d %H:%M:%S')"
  echo "archive dir:  $(basename "$PWD")"
  echo "va-cli sha256: $(shasum -a 256 "$VA" | cut -d' ' -f1)"
  echo
  echo "== machine"
  echo "os:           $(sw_vers -productName) $(sw_vers -productVersion) ($(sw_vers -buildVersion))"
  echo "kernel:       $(uname -srm)"
  echo "model:        $(sysctl -n hw.model)"
  echo "chip:         $(sysctl -n machdep.cpu.brand_string)"
  echo "p-cores:      $(sysctl -n hw.perflevel0.physicalcpu 2>/dev/null || echo n/a)"
  echo "e-cores:      $(sysctl -n hw.perflevel1.physicalcpu 2>/dev/null || echo n/a)"
  echo "logical cpus: $(sysctl -n hw.logicalcpu)"
  echo "memory:       $(sysctl -n hw.memsize | awk '{printf "%.1f GiB", $1/1073741824}')"
  src=$(pmset -g batt 2>/dev/null | head -1)
  case "$src" in *"AC Power"*) echo "power:        mains";; *"Battery Power"*) echo "power:        BATTERY (timings not a baseline)";; *) echo "power:        n/a";; esac
  echo "low power:    $(pmset -g 2>/dev/null | awk '/lowpowermode/ {print ($2=="1") ? "ON (timings not a baseline)" : "off"}')"
  echo "load avg:     $(sysctl -n vm.loadavg | tr -d '{}') (before the run)"
  echo
  echo "== decks (wall time, one run each)"
  printf '%-48s %8s %5s  %s\n' "deck" "seconds" "exit" "sha256(stdout)"
} > "$OUT"

run() {  # run <label> <va-cli args...>
  label=$1; shift
  f="bench-out/$(echo "$label" | tr '/ ' '__').out"
  { time { "$VA" "$@" < /dev/null > "$f" 2> "$f.err"; echo $? > "$f.code"; }; } 2> "$f.time"
  secs=$(tail -1 "$f.time"); code=$(cat "$f.code")
  printf '%-48s %8s %5s  %s\n' "$label" "$secs" "$code" "$(shasum -a 256 "$f" | cut -d' ' -f1)" >> "$OUT"
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
