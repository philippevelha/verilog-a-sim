# Golden references for every test — Spectre netlists

One Cadence Spectre netlist per circuit and analysis of every validation test (t01–t11; t12 is
the gate that compares against the results and has no circuit of its own). The full list, with
what each netlist computes, is `MANIFEST.md`.

**Status, 2026-11-12: generated, not yet run.** Twelve circuits already have Spectre references
in `golden/spectre/mwp/` from the earlier package (`../spectre/`); this folder covers all tests
with the tests' own parameters, so that every check of t01–t11 can be compared with Spectre.
The netlists reuse only Spectre syntax that ran on the server in those rounds (sources, `dc`
sweeps, `ac`, `noise`, `tran`, two-tone `hb`); new here are `strobeperiod`/`strobestart` on `tran`
(checked against this installation's `spectre -h tran`), `phase=` on an AC source, and `sinedc`/
`sinephase` on a sine (checked against `spectre -h vsource`).

## Run on the server

```
tar xzf mwp_golden_package.tar.gz
cd mwp_golden
bash run.sh
```

It writes `golden/<netlist>/` (Spectre's ASCII PSF results, the netlist, `spectre.log`),
`summary.txt` (exit status, errors, wall time, peak memory per netlist) and
`mwp_golden_results.tar.gz`. Send that file back. Its `golden/` directory goes into the
repository's `golden/spectre/mwp/` unchanged.

Expect roughly 20–30 minutes in all: most netlists take 5–15 s, the oscillator transients and the
4 µs noise transients a few minutes each.

## What differs from the tests, and why

- **Transients are thinned** with `strobeperiod` (stated in each netlist's first line), and the
  oscillator ones start saving at 100 ns, so the results stay small enough to commit.
- **Random quantities are statistics only.** The jitter of `t06_jitter` comes from Spectre's
  `$rdist_normal`, and the noise of `t11_*` from Spectre's transient noise or its RNG: the same
  seeds give different sequences in the two simulators. The comparison is on variances and
  spectra, as the tests' own checks are.
- **Spectre's `lin=N` is N steps** (N+1 points): the netlists ask for one step fewer than the
  tests' point counts.
- **Not covered:** the figure-only variants of `make_figures.py` (extra tuning voltages,
  linewidths and drive levels), and the OEO phase noise (open, `../README.md`).

## Regenerating the netlists

`python make_netlists.py` writes `netlists/`, `MANIFEST.md` and `mwp_golden_package.tar.gz` (the
netlists, `run.sh`, `models.scs` and the library). Rerun it after changing a test's parameters or
a model.
