# mwp in Cadence Spectre — reference runs

**Status, 2026-11-12 (SPECTRE 24.1.0.078, Rocky Linux 9.7):** twelve of the fourteen netlists
ran; `compare_spectre.py` finds **21 of 21 comparisons within tolerance**. Two did not run and
need one more pass on the server: `01_link_dc` (stopped with SPECTRE-25, "stopped by the user
or the farm", before simulating) and `23_oeo_hb_noise` (its kick was a `pulse` source, which an
autonomous analysis refuses; now a PWL).

| Result | Spectre against the closed form |
|---|---|
| All 15 modules compile; DC point of a circuit holding every one | no error (one benign `absdelay` warning) |
| Link AC response; filter response | 6e-16; 8e-12 |
| Link noise; linewidth noise through `absdelay` | 1e-6; 5e-6 |
| Two-tone fundamental and IMD3 (transient) | 9e-10; 3e-7 |
| True-time-delay element delays | 1e-24 s |
| kT/C: Spectre's transient noise; the library's `mwp_noise` | −5.9 %; −0.4 % (about 3 % statistical) |
| Harmonic balance: multiplier 2f, 4f | 1e-16; 2e-14 |
| Two-tone harmonic balance: mixer IF, mean current | 7e-13; 2e-13 |
| Periodic noise at the mixer's IF port: thermal + `2q<i>` + `RIN<i²>` | 1.4e-4 (the RIN part alone: 6e-12) |
| OEO by transient: frequency; amplitude, against the loop equation | 5e-6; 6.5e-4 |

What it means: the models are portable to Spectre as written, the closed forms used by the
tests are confirmed by a second simulator, and there is now a closed form for the noise of a
pumped circuit (time-averaged cyclostationary shot and RIN) that Spectre reproduces: a gate
for step 4 of `docs/proposals/harmonic-balance.md`.

**The opto-electronic oscillator (decks 22–29, three further rounds on the server).**

| Quantity | Result |
|---|---|
| Frequency, amplitude: Spectre transient (`25`) | 8.4e-6, 5.5e-5 from the loop equation (with the 40 GHz detector pole) |
| Frequency, amplitude: Spectre autonomous harmonic balance (`23`) | 1.6e-6, 6.2e-4 |
| `24` (other settings) | **converged to the non-oscillating solution without an error**: fundamental 8.5e-17 V. `compare_spectre.py` flags it |
| `26`–`28` (delay as Spectre `tline`) | harmonic balance did not converge (iteration limit; two-tier start failed) |
| Phase noise from `23` | −92.4 dBc/Hz at 100 kHz, 1/f², Lorentzian corner 43.6 Hz; laser RIN 82 % of it |
| Yao–Maleki formula, same circuit and noise sources | −105.3 dBc/Hz at 100 kHz |

Spectre is 13 dB above the formula. For the load resistor alone (a plain additive source, so the
sources are not in question) Spectre equals the formula **with the tank's 1.41 ns group delay
only**, to 0.01 dB, and is 13.2 dB above it with the 5.02 ns delay line included.

Deck `29` tests whether `hbnoise` applies an `absdelay` at the noise frequency: thermal noise added
to a copy of itself delayed 1 ns, with a sine making the circuit periodic. It does: `noise` and
`hbnoise` with `mwp_delay` both give `4kTR·4cos²(πfτ)` to 2e-5, nulls included (`hbnoise` with
`tline` does not: it loses the delayed path). So the simple explanation, "the delay is ignored at
the sidebands", is wrong. What is left, **an inference not yet tested**: the oscillator's phase
noise in Spectre comes from its periodic adjoint (the PPV and its Lorentzian corner), and for a
circuit with a delay that calculation needs the delay's memory in its normalization. A
normalization that leaves it out would scale the phase noise by `(τ_tank+τ_delay)²/τ_tank² = 21`,
13.2 dB: what is observed. The test would be the same oscillator with the delay built from lumped
elements, an ordinary ODE: if this is the cause, its phase noise falls by 13 dB toward the formula.

Until that is settled, Spectre's phase-noise number for this oscillator is not a reference.

Two things Spectre said that bear on the library:
- `$abstime` is "invalid in HB analysis" (`mwp_laser`, a warning). With `df = 0` the term is
  zero and the results above are right; a rotating envelope (`df != 0`), and `mwp_mll`, cannot
  be used in Spectre's harmonic balance.
- A periodic source may not appear in an autonomous analysis: start an oscillator with a PWL
  kick or an initial condition.

Cost, for the record: OEO transient, 200 ns at 1 ps: 1.89 M steps, 296 s, 877 MB in Spectre
(verilog-a-sim, same deck: 0.28 M points, 34 s). The 4 µs noise transients: 65 s and 89 s.

**Not committed:** the result archive is ignored by git. Whether Spectre output may be
published or kept in the repository depends on the licence agreement and is not settled.

## What it is for

1. **Portability.** The library has only ever run in verilog-a-sim. `00_models.scs` instantiates
   all fifteen modules; whether Spectre compiles them is the first real test of the claim that
   the models are plain Verilog-A.
2. **A reference simulator** for the circuits of the validation tests, beside the closed forms.
3. **Stage 2, not written yet:** harmonic balance and periodic noise, as the reference the
   proposed analysis (`docs/proposals/harmonic-balance.md`) would be validated against.
   `run.sh` collects `spectre -h hb|hbnoise|pss|pnoise` and runs a one-tone `hb` probe, so stage 2
   can be written from this installation's own help text and licence, not from memory.

## Run

Copy **`mwp_spectre_package.tar.gz`** to the server (it holds the netlists, `run.sh` and the
model library; the netlists alone are not enough), then:

```
tar xzf mwp_spectre_package.tar.gz
cd mwp_spectre
bash run.sh
```

Needs only `spectre` on the PATH (written for SPECTRE 24.10.078). It runs each netlist with
ASCII PSF output and writes `mwp_spectre_results.tar.gz`. Send that file back.
`work/summary.txt` has one line per netlist: exit status, error count, wall time, peak memory.

To rebuild the package after changing a netlist or a model, from this directory:
`mkdir -p _pkg/mwp_spectre/lib && cp *.scs run.sh README.md _pkg/mwp_spectre/ && cp ../lib/mwp_*.va ../lib/mwp.vams _pkg/mwp_spectre/lib/ && tar czf mwp_spectre_package.tar.gz -C _pkg mwp_spectre && rm -rf _pkg`

**First attempt, 2026-11-12:** nothing was simulated. The script called `/usr/bin/time`, which
the server does not have, and only this folder had been copied, so the models were missing.
Both are fixed in the script. What that run did establish: the Spectre licence checks out, and
the help text gave the real parameter names for the stage-2 decks (see below).

## Netlists (stage 1)

| Netlist | Test | Analysis | Compared with |
|---|---|---|---|
| `00_models` | — | `dc` | compiles, converges |
| `01_link_dc` | t01 a | `dc` sweep of the bias | `R P cos²(πV/2Vπ)` |
| `01_link_ac` | t01 b | `ac` | two-pole response |
| `01_link_noise` | t01 d | `noise` | shot + RIN + thermal |
| `01_link_tran` | t01 c | `tran`, two tones | `R P J1 J0`, `R P J1 J2` |
| `02_filter_ac` | t02 b | `ac` | ring-filter closed form |
| `03_linewidth_noise` | t03 c | `noise` through `absdelay` | `(RP)² 4 sin²(πfτ) Δν/(πf²)` |
| `07_ttd_ac` | t07 a | `ac`, four elements | `−2πf τ_n` |
| `11_rc_trannoise` | t11 a | `tran` with Spectre's transient noise | `kT/C` |
| `11_rc_libnoise` | t11 a | `tran`, the library's `mwp_noise` | `kT/C` |

## Netlists (stage 2, drafts)

Parameter names corrected against this installation's `spectre -h hb` and `-h hbnoise`
(`funds`, `fundfreqs`, `maxharms`, `tstab`, `maxsideband`, `noiseout`; an oscillator is marked
by a node pair on the `hb` line, there is no `oscana`). Still never run. `compare_spectre.py` does not read them yet.

| Netlist | Test | Analysis | Will be compared with |
|---|---|---|---|
| `20_multiplier_hb` | t03 b | `hb`, one tone | `R P J2(m)`, `R P J4(m)` |
| `21_mixer_hb_noise` | t08 | `hb`, two tones, `hbnoise` | IF `R P J1 J1`; noise at the IF: no closed form yet |
| `22_oeo_tran` | t04 | `tran` | frequency and amplitude from the loop equation |
| `23_oeo_hb_noise` | t04 | autonomous `hb`, `hbnoise` | the same, and `L(f)` against Yao–Maleki |

The comparison is done on the development machine from the returned archive
(`compare_spectre.py`, with `psf.py` to read the ASCII PSF files); the server needs no Python.

## Known unknowns

- Whether every construct is accepted as written (`absdelay`, `@(timer)`, `$rdist_normal`,
  `$bound_step`, `flicker_noise` with exponent 2, a noise source on a voltage contribution).
- The exact spelling of some analysis parameters (`noisefmax`/`noiseseed` on `tran`, `hb`'s
  `fund`/`maxharms`). The help files in the archive settle them.
- Whether the RF analyses are licensed (the `probe_hb` line of `summary.txt`).
- Whether Spectre's outputs may be committed or published: check the licence agreement before
  any of these results goes into `golden/` or the paper.
