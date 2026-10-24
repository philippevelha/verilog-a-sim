# ngspice runs of the two carrier-filter forms, 2026-10-24

ngspice-47 (master a80f6e3) + OpenVAF-reloaded, Linux. Decks in `..`: `pulse_ng.cir`
(derivative-free W form, `mrm_pin.osdi`) and `pulse_direct_ng.cir` (direct form,
`mrm_pin_direct.osdi`), both with `option reltol=1e-5`; the `_deftol` files are the same decks
at ngspice's default reltol. Columns: time, ia, v(n1#xfc) (free-carrier current X, mA),
v(n1#n12), v(n1#n20), v(n1#l3), v(th_re), v(th_im).

| File | Points | min X after fall (% of peak) | max \|ΔP\| vs W form |
|---|---|---|---|
| pulse_ng.dat (byte-identical to `../pulse_ng.dat`) | 6308 | +0.006 | — |
| pulse_direct_ng.dat | 7570 | −0.043 | 2.59 µW |
| pulse_ng_deftol.dat | 6049 | +0.006 | — |
| pulse_direct_ng_deftol.dat | 6051 | −0.214 | 11.35 µW |

These replace the earlier README claim that the direct form lost "up to 4×" of the free
charge in ngspice. See `../../README.md`, Numerics.
