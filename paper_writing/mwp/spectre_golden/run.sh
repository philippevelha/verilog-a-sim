#!/bin/bash
# Run every golden-reference netlist of the mwp library in Cadence Spectre.
#
#   bash run.sh                      # all netlists (see MANIFEST.md)
#   bash run.sh t01_ac t04_oeo_502   # a subset, by name (with or without .scs)
#
# Needs: spectre on PATH, the library in ./lib (as packaged). No Python.
# Produces golden/<netlist>/ (result files + the netlist + spectre.log), summary.txt,
# environment.txt, and mwp_golden_results.tar.gz to send back. The layout of golden/ is the
# layout of golden/spectre/mwp/ in the repository: it is copied there as it is.
set -u
here="$(cd "$(dirname "$0")" && pwd)"
lib=""
for d in "$here/lib" "$here/../lib"; do
  if [ -f "$d/mwp_laser.va" ]; then lib="$d"; break; fi
done
[ -z "$lib" ] && { echo "ERROR: model library not found in $here/lib" >&2; exit 1; }
command -v spectre > /dev/null 2>&1 || { echo "ERROR: spectre is not on the PATH" >&2; exit 1; }

work="$here/work"; out="$here/golden"
rm -rf "$work" "$out"; mkdir -p "$work" "$out"
cp "$lib"/mwp_*.va "$lib"/mwp.vams "$here/models.scs" "$work"/
cp "$here"/t[0-9][0-9]_*.scs "$work"/
cd "$work" || exit 1
{ echo "date: $(date -u)"; echo "host: $(uname -srm)"; echo "spectre: $(command -v spectre)"
  echo "library: $lib ($(ls "$lib"/mwp_*.va | wc -l) models)"; spectre -W 2>&1 | tail -1; } > "$here/environment.txt" 2>&1

list=("$@")
if [ ${#list[@]} -eq 0 ]; then for f in t[0-9][0-9]_*.scs; do list+=("${f%.scs}"); done; fi
: > "$here/summary.txt"
for n in "${list[@]}"; do
  n="$(basename "$n" .scs)"
  if [ ! -f "$n.scs" ]; then echo "$n: no such netlist" | tee -a "$here/summary.txt"; continue; fi
  start=$(date +%s.%N)
  spectre "$n.scs" -format psfascii -raw "$n" +log "$n.log" > "$n.stdout" 2>&1
  status=$?
  wall=$(echo "$(date +%s.%N) $start" | awk '{printf "%.1f", $1 - $2}')
  errs=$(grep -c "ERROR (" "$n.log" 2>/dev/null)
  peak=$(grep "Peak resident memory used" "$n.log" 2>/dev/null | tail -1 | sed 's/^ *//')
  mkdir -p "$out/$n"
  [ -d "$n" ] && cp "$n"/* "$out/$n/" 2>/dev/null
  cp "$n.scs" "$out/$n/"; cp "$n.log" "$out/$n/spectre.log" 2>/dev/null
  echo "$n: exit $status, errors ${errs:-?}, ${wall} s wall, ${peak:-no memory line}" | tee -a "$here/summary.txt"
done
cp "$here/environment.txt" "$here/summary.txt" "$out"/
cd "$here" || exit 1
tar czf mwp_golden_results.tar.gz golden
echo; echo "Send back: $here/mwp_golden_results.tar.gz"
