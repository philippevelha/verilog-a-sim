# Circuits without a golden reference that are not in the Spectre package

| Deck | Reason |
|---|---|
| `circuits/benchmark/hicum_output_family.net` | not translated: model from the untracked external/ corpus: external/code/hicum2/vacode/hicumL2V3p0p0.va |
| `circuits/benchmark/iscas85/c1355.net` | left out: ISCAS'85: up to 268 000 unknowns, hours per run |
| `circuits/benchmark/iscas85/c17.net` | left out: ISCAS'85: up to 268 000 unknowns, hours per run |
| `circuits/benchmark/iscas85/c17_tran.net` | left out: ISCAS'85: up to 268 000 unknowns, hours per run |
| `circuits/benchmark/iscas85/c1908.net` | left out: ISCAS'85: up to 268 000 unknowns, hours per run |
| `circuits/benchmark/iscas85/c2670.net` | left out: ISCAS'85: up to 268 000 unknowns, hours per run |
| `circuits/benchmark/iscas85/c3540.net` | left out: ISCAS'85: up to 268 000 unknowns, hours per run |
| `circuits/benchmark/iscas85/c432.net` | left out: ISCAS'85: up to 268 000 unknowns, hours per run |
| `circuits/benchmark/iscas85/c432_seq.net` | left out: ISCAS'85: up to 268 000 unknowns, hours per run |
| `circuits/benchmark/iscas85/c432_tran.net` | left out: ISCAS'85: up to 268 000 unknowns, hours per run |
| `circuits/benchmark/iscas85/c499.net` | left out: ISCAS'85: up to 268 000 unknowns, hours per run |
| `circuits/benchmark/iscas85/c5315.net` | left out: ISCAS'85: up to 268 000 unknowns, hours per run |
| `circuits/benchmark/iscas85/c6288.net` | left out: ISCAS'85: up to 268 000 unknowns, hours per run |
| `circuits/benchmark/iscas85/c7552.net` | left out: ISCAS'85: up to 268 000 unknowns, hours per run |
| `circuits/benchmark/iscas85/c7552_tran.net` | left out: ISCAS'85: up to 268 000 unknowns, hours per run |
| `circuits/benchmark/iscas85/c880.net` | left out: ISCAS'85: up to 268 000 unknowns, hours per run |
| `circuits/benchmark/nand/bsim4.net` | not translated: model from the untracked external/ corpus: external/code/bsim4/vacode/bsim4.va |
| `circuits/benchmark/nand/ekv26.net` | not translated: model from the untracked external/ corpus: external/code/ekv/vacode/ekv26.va |
| `circuits/fiber_mzi_noise.net` | not translated: `X4  pb   pbo phb phbo wl           waveguide  L=40 lambda0=1319e-9 w0=`: 5 terminals on `waveguide`, which has 6 ports (an unconnected optional port is $port_connected-dependent; Spectre would not see it as unconnected) |
| `circuits/microring_thermal.net` | not translated: `X1 lin  wl            cw_laser    power=1e-3 lambda=1550.5e-9`: 2 terminals on `cw_laser`, which has 3 ports (an unconnected optional port is $port_connected-dependent; Spectre would not see it as unconnected); `X2 hp   gnd  th       heater      R=2e3 Rth=1e3 Cth=1.2e-8`: 3 terminals on `heater`, which has 2 ports (an unconnected optional port is $port_connected-dependent; Spectre would not see it as unconnected) |
| `circuits/microring_thermal_fast.net` | not translated: `X1 lin  wl            cw_laser    power=1e-3 lambda=1550.5e-9`: 2 terminals on `cw_laser`, which has 3 ports (an unconnected optional port is $port_connected-dependent; Spectre would not see it as unconnected); `X2 hp   gnd  th       heater      R=2e3 Rth=1e3 Cth=1.2e-8`: 3 terminals on `heater`, which has 2 ports (an unconnected optional port is $port_connected-dependent; Spectre would not see it as unconnected) |
| `circuits/motorway_ramp.net` | not translated: `X1   c1  c2   v1  v1        section`: 4 terminals on `section`, which has 5 ports (an unconnected optional port is $port_connected-dependent; Spectre would not see it as unconnected); `X2   c2  c3   v1  v2        section`: 4 terminals on `section`, which has 5 ports (an unconnected optional port is $port_connected-dependent; Spectre would not see it as unconnected); `X4   c4  cb   v3  v4        section`: 4 terminals on `section`, which has 5 ports (an unconnected optional port is $port_connected-dependent; Spectre would not see it as unconnected) |
| `circuits/motorway_ramp_alinea.net` | not translated: `X1   c1  c2   v1  v1        section`: 4 terminals on `section`, which has 5 ports (an unconnected optional port is $port_connected-dependent; Spectre would not see it as unconnected); `X2   c2  c3   v1  v2        section`: 4 terminals on `section`, which has 5 ports (an unconnected optional port is $port_connected-dependent; Spectre would not see it as unconnected); `X4   c4  cb   v3  v4        section`: 4 terminals on `section`, which has 5 ports (an unconnected optional port is $port_connected-dependent; Spectre would not see it as unconnected) |
| `circuits/motorway_ramp_mpc.net` | not translated: `X1   c1  c2   v1  v1        section`: 4 terminals on `section`, which has 5 ports (an unconnected optional port is $port_connected-dependent; Spectre would not see it as unconnected); `X2   c2  c3   v1  v2        section`: 4 terminals on `section`, which has 5 ports (an unconnected optional port is $port_connected-dependent; Spectre would not see it as unconnected); `X4   c4  cb   v3  v4        section`: 4 terminals on `section`, which has 5 ports (an unconnected optional port is $port_connected-dependent; Spectre would not see it as unconnected) |
| `circuits/multiphysics/gray_scott_spots.net` | left out: two-dimensional net arrays in gray_scott.va, which Spectre rejects (VACOMP-2259); needs a model change |
| `circuits/multiphysics/gray_scott_wellmixed.net` | left out: two-dimensional net arrays in gray_scott.va, which Spectre rejects (VACOMP-2259); needs a model change |
| `circuits/ring_gyro_noise.net` | not translated: `X1   lin  wl        cw_laser   power=1e-3 lambda=1550.00053e-9 rin=3.1`: 2 terminals on `cw_laser`, which has 3 ports (an unconnected optional port is $port_connected-dependent; Spectre would not see it as unconnected) |
| `circuits/snn/mnist/decks/digit0.net` | left out: hours per deck |
| `circuits/snn/mnist/decks/digit1.net` | left out: hours per deck |
| `circuits/snn/mnist/decks/digit2.net` | left out: hours per deck |
| `circuits/snn/mnist/decks/digit3.net` | left out: hours per deck |
| `circuits/snn/mnist/decks/digit4.net` | left out: hours per deck |
| `circuits/snn/mnist/decks/digit5.net` | left out: hours per deck |
| `circuits/snn/mnist/decks/digit6.net` | left out: hours per deck |
| `circuits/snn/mnist/decks/digit7.net` | left out: hours per deck |
| `circuits/snn/mnist/decks/digit8.net` | left out: hours per deck |
| `circuits/snn/mnist/decks/digit9.net` | left out: hours per deck |
