#!/bin/bash
# Run the Spectre translations of the circuits that have no golden reference.
#
#   bash run.sh                 # the standard decks (MANIFEST.md, class "standard")
#   bash run.sh --all           # also the heavy ones (heavy.txt: large or long transients)
#   bash run.sh rc_pulse ...    # a subset, by netlist name
#
# Needs: spectre on PATH; the package unpacked as is (netlists at the top, the model files under
# models/, benchmark/models/, tests/fixtures/ at the paths the netlists include). No Python.
# Produces golden/<netlist>/ (results, the netlist, spectre.log), summary.txt, environment.txt and
# zoo_golden_results.tar.gz to send back.
set -u
here="$(cd "$(dirname "$0")" && pwd)"
command -v spectre > /dev/null 2>&1 || { echo "ERROR: spectre is not on the PATH" >&2; exit 1; }
cd "$here" || exit 1
out="$here/golden"; rm -rf "$out"; mkdir -p "$out"
{ echo "date: $(date -u)"; echo "host: $(uname -srm)"; echo "spectre: $(command -v spectre)"
  spectre -W 2>&1 | tail -1; } > environment.txt 2>&1

all=0; list=()
for a in "$@"; do if [ "$a" = "--all" ]; then all=1; else list+=("$(basename "$a" .scs)"); fi; done
if [ ${#list[@]} -eq 0 ]; then
  for f in *.scs; do
    n="${f%.scs}"
    if [ $all -eq 0 ] && grep -qx "$n" heavy.txt; then continue; fi
    list+=("$n")
  done
fi
: > summary.txt
for n in "${list[@]}"; do
  if [ ! -f "$n.scs" ]; then echo "$n: no such netlist" | tee -a summary.txt; continue; fi
  start=$(date +%s.%N)
  mkdir -p raw logs
  spectre "$n.scs" -format psfascii -raw "raw/$n" +log "logs/$n.log" > /dev/null 2>&1
  status=$?
  wall=$(echo "$(date +%s.%N) $start" | awk '{printf "%.1f", $1 - $2}')
  errs=$(grep -c "ERROR (" "logs/$n.log" 2>/dev/null)
  peak=$(grep "Peak resident memory used" "logs/$n.log" 2>/dev/null | tail -1 | sed 's/^ *//')
  mkdir -p "$out/$n"
  [ -d "raw/$n" ] && cp "raw/$n"/* "$out/$n/" 2>/dev/null
  cp "$n.scs" "$out/$n/"; cp "logs/$n.log" "$out/$n/spectre.log" 2>/dev/null
  # a model that failed to compile leaves its C compiler log here: keep it
  if [ $status -ne 0 ] && [ -d "$n.ahdlSimDB" ]; then
    k=0; find "$n.ahdlSimDB" -name ahdlcmi.out | while read -r f; do k=$((k+1)); cp "$f" "$out/$n/ahdlcmi_$k.out"; done
  fi
  echo "$n: exit $status, errors ${errs:-?}, ${wall} s wall, ${peak:-no memory line}" | tee -a summary.txt
done
cp environment.txt summary.txt "$out"/
tar czf zoo_golden_results.tar.gz golden
echo; echo "Send back: $here/zoo_golden_results.tar.gz"
