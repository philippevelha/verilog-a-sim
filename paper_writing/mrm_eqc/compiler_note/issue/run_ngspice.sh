#!/bin/sh
# Compile and run both minimal variants with OpenVAF-reloaded + ngspice.
# OPENVAF and NGSPICE default to the names on PATH.
set -e
cd "$(dirname "$0")"
cp ../../vasim/disciplines.vams .
OPENVAF=${OPENVAF:-openvaf-r}
NGSPICE=${NGSPICE:-ngspice}
"$OPENVAF" --version || true
"$NGSPICE" --version | head -2 || true
for v in loop noloop; do
    "$OPENVAF" rl_$v.va -o rl_$v.osdi
    echo "== rl_$v (expected |V(nind)| = 0.0995, 0.3015, 0.7071, 0.9535, 0.9950)"
    "$NGSPICE" -b rl_$v.cir 2>&1 | grep -E '^[0-9]+\s' || "$NGSPICE" -b rl_$v.cir
done
