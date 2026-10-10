#!/usr/bin/env bash
# Run the MNIST hidden-2 crossbar (16 neurons, 20 cycles) for several digits and access devices,
# several runs at once, one va-cli process each. From the repository root:
#
#   bash scripts/benchmark/xbar_digits.sh <runs-dir> <jobs> <threads> <kind> <digit>...
#
# <kind> is gen_xbar_deck.py's --synapses (tx, fit, fitc, fitbc, ideal), or txbp: tx with --bypass.
# <jobs> runs go at once with <threads> threads each. Each writes <runs-dir>/<digit>/<kind>.out and
# .err (ending in `exit <status>`); a run whose .err already ends in `exit 0` is skipped, so the
# same command resumes. Compare with circuits/snn/transistor/cmp_digits.py <runs-dir>.
#
# Memory, not cores, bounds <jobs>: the transient keeps every unknown at every timepoint, so a
# 20-cycle PSP103 run (16 830 unknowns) ends near 3.5 GB. Four at once overran a 15 GB container.
set -u
[ $# -ge 5 ] || { sed -n 2,13p "$0"; exit 2; }
DIR=$(cd "$1" && pwd) JOBS=$2 THREADS=$3 KIND=$4
shift 4
cargo build -q --release -p va-cli || exit 1
run() {
  local d=$1 k=$2 dir=$DIR/$1 m="--model models/snn" x="" s e
  local r
  r=$(seq -s, -f "XN_h2_%g.vm" 0 15),$(seq -s, -f "h2_%g" 0 15)
  [ "$k" = txbp ] && x="--bypass"
  [ "${k%bp}" = tx ] && m="$m --model models/sg13g2/psp103/psp103.va"
  mkdir -p "$dir"
  [ -f "$dir/$k.err" ] && tail -1 "$dir/$k.err" | grep -q "^exit 0" && return 0
  python3 scripts/benchmark/gen_xbar_deck.py "$dir/$k" --neurons 16 --cycles 20 --digit "$d" \
    --synapses "${k%bp}" > "$dir/$k.gen" 2>&1 || { echo "digit $d $k: deck generation failed"; return 1; }
  s=$(date +%s)
  VA_JIT=on VA_COLLAPSE=on target/release/va-cli sim "$dir/$k/d/xbar.net" $m --tran $x \
    --threads "$THREADS" --report "$r" > "$dir/$k.out" 2> "$dir/$k.err"
  e=$?
  echo "exit $e" >> "$dir/$k.err"
  echo "$(date +%T) digit $d $k: exit $e, $(( $(date +%s) - s )) s"
}
export -f run
export DIR THREADS
printf '%s\n' "$@" | xargs -P "$JOBS" -I{} bash -c "run {} $KIND"
