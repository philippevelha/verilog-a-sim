#!/usr/bin/env bash
# Run the ten MNIST samples through the circuit, resumably (from the repository root):
#   bash circuits/snn/mnist/run_all.sh [path/to/va-cli]
#
# A digit already in circuits/snn/mnist/results.jsonl is skipped, so after an interruption (the
# machine sleeping, the process killed for memory) the same command continues where it stopped.
# A digit whose deck generation or simulation fails is retried, up to 3 attempts. Every attempt is
# logged with its start and end time in circuits/snn/mnist/run_log.txt; the wall time of each
# va-cli run is in results.jsonl (check_mnist.py measures it).
set -u
VA=${1:-target/release/va-cli.exe}
DIR=circuits/snn/mnist
LOG=$DIR/run_log.txt
mkdir -p docs/examples/snn_mnist
done_digits() {
  [ -f "$DIR/results.jsonl" ] || return 0
  python -c "import json;print(' '.join(str(json.loads(l)['digit']) for l in open('$DIR/results.jsonl') if l.strip()))"
}
for d in 0 1 2 3 4 5 6 7 8 9; do
  case " $(done_digits) " in *" $d "*) echo "$(date '+%F %T') digit $d: already done, skipped" >> "$LOG"; continue;; esac
  for attempt in 1 2 3; do
    echo "$(date '+%F %T') digit $d: attempt $attempt started" >> "$LOG"
    if python "$DIR/gen_mnist_deck.py" "$d" >> "$LOG" 2>&1 &&
       python "$DIR/check_mnist.py" "$d" "$VA" --plot "docs/examples/snn_mnist/digit$d.png" >> "$LOG" 2>&1; then
      echo "$(date '+%F %T') digit $d: attempt $attempt finished" >> "$LOG"
      break
    fi
    echo "$(date '+%F %T') digit $d: attempt $attempt FAILED (exit $?)" >> "$LOG"
  done
done
echo "$(date '+%F %T') run_all: done; digits recorded: $(done_digits)" >> "$LOG"
