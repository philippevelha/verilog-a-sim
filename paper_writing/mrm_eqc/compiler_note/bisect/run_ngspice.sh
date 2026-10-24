#!/bin/sh
# Compile and run every bisection variant (run gen.py first). Prints max |V(nind)| over the
# 13 AC points: 0 means the variant still triggers the bug.
cd "$(dirname "$0")"
OPENVAF=${OPENVAF:-openvaf-r}
NGSPICE=${NGSPICE:-ngspice}
cp ../../vasim/disciplines.vams ../../vasim/constants.vams . 2>/dev/null
for va in b*_*.va; do
    v=${va%.va}
    "$OPENVAF" "$va" -o "$v.osdi" > "$v.openvaf.log" 2>&1 || { echo "$v: OpenVAF failed, see $v.openvaf.log"; continue; }
    "$NGSPICE" -b "$v.cir" > "$v.ngspice.log" 2>&1
    awk -v v="$v" '$1 ~ /^[0-9]+$/ && NF >= 4 { a = $NF < 0 ? -$NF : $NF; if (a > m) m = a; n++ }
        END { printf "%-20s max|V(nind)| = %.6e over %d points%s\n", v, m, n, (m == 0 ? "   <- BUG" : "") }' "$v.ngspice.log"
done
