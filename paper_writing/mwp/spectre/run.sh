#!/bin/bash
# Run the mwp library's reference circuits in Cadence Spectre and pack the results.
#
#   bash run.sh                             # everything
#   bash run.sh 01_link_ac 02_filter_ac     # a subset, by netlist name
#
# Needs: spectre on PATH, and the model library in ./lib (the packaged form) or ../lib (the
# repository layout). Nothing else: no Python, no /usr/bin/time.
# Produces: work/<netlist>/ (ASCII PSF results), work/<netlist>.log, work/summary.txt, and
# mwp_spectre_results.tar.gz to send back.
# Runs every NN_*.scs: DC, AC, noise, transient (stage 1) and harmonic balance with periodic
# noise (stage 2). It also saves this Spectre's help text for the analyses used.
set -u
here="$(cd "$(dirname "$0")" && pwd)"

lib=""
for d in "$here/lib" "$here/../lib"; do
  if [ -f "$d/mwp_laser.va" ]; then lib="$d"; break; fi
done
if [ -z "$lib" ]; then
  echo "ERROR: the model library was not found in $here/lib or $here/../lib." >&2
  echo "Unpack the whole package (mwp_spectre_package.tar.gz), not only the netlists." >&2
  exit 1
fi
if ! command -v spectre > /dev/null 2>&1; then
  echo "ERROR: spectre is not on the PATH." >&2
  exit 1
fi

work="$here/work"
rm -rf "$work"
mkdir -p "$work"
# the models and our header; NOT our disciplines.vams: Cadence's own is used
cp "$lib"/mwp_*.va "$lib"/mwp.vams "$work"/
cp "$here"/*.scs "$work"/
cd "$work" || exit 1

{
  echo "date: $(date -u)"; echo "host: $(uname -srm)"
  echo "spectre: $(command -v spectre)"
  echo "library: $lib ($(ls "$lib"/mwp_*.va | wc -l) models)"
  spectre -W 2>&1 | tail -1
} > environment.txt 2>&1

for a in hb hbnoise hbac pss pnoise tran noise tline delay; do
  spectre -h "$a" > "help_$a.txt" 2>&1
done

list=("$@")
if [ ${#list[@]} -eq 0 ]; then
  for f in [0-9][0-9]_*.scs; do list+=("${f%.scs}"); done
fi

: > summary.txt
for n in "${list[@]}"; do
  n="$(basename "$n" .scs)"          # accept 01_link_dc, 01_link_dc.scs or a path
  if [ ! -f "$n.scs" ]; then echo "$n: no such netlist" | tee -a summary.txt; continue; fi
  start=$(date +%s.%N)
  spectre "$n.scs" -format psfascii -raw "$n" +log "$n.log" > "$n.stdout" 2>&1
  status=$?
  wall=$(echo "$(date +%s.%N) $start" | awk '{printf "%.1f", $1 - $2}')
  errs=$(grep -c "ERROR (" "$n.log" 2>/dev/null)
  peak=$(grep "Peak resident memory used" "$n.log" 2>/dev/null | tail -1 | sed 's/^ *//')
  echo "$n: exit $status, errors ${errs:-?}, ${wall} s wall, ${peak:-no memory line}" | tee -a summary.txt
done

cd "$here" || exit 1
tar czf mwp_spectre_results.tar.gz --exclude='*.ahdlSimDB' --exclude='*ahdlcmi*' -C "$work" .
echo
echo "Send back: $here/mwp_spectre_results.tar.gz"
