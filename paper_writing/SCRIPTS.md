# Scripts in `paper_writing/`: what each one does, and how to regenerate the MWP figures

Written 2026-11-18 at verilog-a-sim 2.57.0+5. All paths are relative to `paper_writing/`.
Every Python script here calls the simulator binary `va-cli`, taken from the `VACLI` environment
variable or, by default, `<repo>/target/release/va-cli[.exe]`.

---

## 1. Map of the folder

| Folder | What it is | Feeds |
|---|---|---|
| `main.tex`, `sections/`, `refs.bib`, `latexmkrc` | the TCAD paper (`latexmk` builds it here) | — |
| `figures/` | the figures `main.tex` actually includes (**copies**, see §4) | TCAD paper |
| `mwp/` | microwave-photonics Verilog-A library, its tests t01–t13, its figures | conference abstract (`mwp/abstract_conference.md`) |
| `codesign/` | photonic library + IHP SG13G2 transistors in one deck (3 cases, driver, noise). **Keep local until the abstract is approved** | conference abstract |
| `mrm_tcmt/` | microring modulator, TCMT model; ngspice comparison, speed, Q×η sweep | TCAD §09b |
| `mrm_eqc/` | microring equivalent-circuit model ([B] paper): PAM4/PAM8 eyes, FFE | TCAD §09b |
| `mrm_injection/` | p-i-n (carrier-injection) microring, Wang 2016 | TCAD §09b |
| `*.zip`, `mrm_results.png`, `overshoot.png`, `q_eta_*.png`, `sweep_capacita_driver.xlsx` at the top level | stray copies/archives (byte-identical to files in the subfolders, checked by MD5) | nothing reads them |

---

## 2. `mwp/` — microwave-photonics library

| Script | What it does | Writes |
|---|---|---|
| `tests/mwp.py` | **Harness.** `run(deck, analysis, report=…, tag=…, flags=…, env=…)` writes `tests/work/<tag>.net`, runs `va-cli`, parses its output. Also `o()` (optical bus → two nets), `tones()` (least-squares tone fit), `freq_est`, `resample`, `dbm`, `pwl`, `Report` (pass/fail JSON) | `tests/work/<tag>.net/.out/.err` |
| `tests/fit_dispersion.py` | designs the all-pass ring chain that imitates fibre dispersion (`design`, `chain`) | — (library for t09 and the figure) |
| `tests/t01_imdd_link.py` | IMDD link: transfer curve, AC response, two-tone IMD3, noise, NF, SFDR | `t01_imdd_link.json` |
| `tests/t02_ring_filter.py` | ring spectrum, single-passband MWP filter (AC + transient) | `t02_*.json` |
| `tests/t03_rf_generation.py` | heterodyne 1–100 GHz, ×2/×4 multiplication, laser linewidth discriminator | `t03_*.json` |
| `tests/t04_oeo.py` | opto-electronic oscillator: frequency, amplitude, mode selection | `t04_*.json` |
| `tests/t05_waveform_generation.py` | 16-QAM at 20 GHz, 4 GHz LFM chirp | `t05_*.json` |
| `tests/t06_optical_sampling.py` | photonic sampling, arcsine linearisation, jitter, demux | `t06_*.json` |
| `tests/t07_beamforming.py` | true-time delay vs phase-shifter beamforming (squint) | `t07_*.json` |
| `tests/t08_mixer.py` | series and dual-parallel MZM down-converters | `t08_*.json` |
| `tests/t09_dispersion.py` | fibre fading vs all-pass chain (4/8/12 sections) | `t09_*.json` |
| `tests/t10_optical_amplifier.py` | gain saturation, signal-spontaneous beat noise | `t10_*.json` |
| `tests/t11_transient_noise.py` | `mwp_noise` module vs `va-cli --tran-noise` | `t11_*.json` |
| `tests/t12_spectre_golden.py` | verilog-a-sim vs Spectre outputs (skips itself without `golden/spectre/mwp/`) | `t12_*.json` |
| `tests/t13_electrical_loads.py` | MZM electrode admittance, PD junction pole | `t13_*.json` |
| `tests/run_all.py` | runs t01–t13 (or `run_all.py t03 t07`), then rebuilds `RESULTS.md` from the JSONs | `../RESULTS.md` |
| **`tests/make_figures.py`** | **the 12 MWP figures** (§3) | `../figures/fig_*.pdf/.png` |
| `spectre/run.sh` | runs `spectre/NN_*.scs` on the Spectre server, packs results | `mwp_spectre_results.tar.gz` |
| `spectre/psf.py` | reader for Spectre ASCII PSF files | — |
| `spectre/compare_spectre.py` | Spectre results vs the tests' closed forms | printed table |
| `spectre_golden/make_netlists.py` | one Spectre netlist per test circuit (golden reference for t12) | `netlists/*.scs`, `MANIFEST.md`, package |
| `spectre_golden/run.sh` | runs those netlists on the server | `mwp_golden_results.tar.gz` |

## 3. `codesign/`

| Script | What it does | Writes |
|---|---|---|
| `mwp.py` | **identical copy** of `mwp/tests/mwp.py` (if you change one, change both) | — |
| `csd.py` | harness = `mwp.run` + SG13G2 PSP103 cards per geometry (`Cards`, `mos`) | — |
| `link.py`, `case1.py`, `sweep1.py` | case 1: RF photonic link with transistor driver and TIA; gain/NF/OIP3/SFDR; sweep V_π × P | `sweep1.json` |
| `ring.py`, `sweep2.py` | case 2: microring thermal lock loop; sweep C_int × P | `sweep2.json`, `sweep2_waves.json` |
| `case3.py` | case 3: photonic down-converter into the case-1 receiver | `case3.json` |
| `driver.py` | microring driver (`microring_driver_sg13g2.cir`, HV devices) into `mrm_tcmt` | — |
| `eye.py` | 2 Gb/s PRBS7 eyes of the driver; `--rerun` to resimulate, else redraws the cache | `figures/fig_driver_eye*.pdf/png`, `work/eye_*.json` |
| `noise.py` | `.noise` budgets + transient-noise eye of driver + ring + receiver | `figures/fig_driver_noise.*` |
| `make_figures.py` | reads `sweep1.json`, `sweep2_waves.json`; overlays Spectre crosses if results are on this machine | `figures/fig_c1_sfdr`, `fig_c1_nf`, `fig_c2_step` |
| `spectre/make_package.py` | translates the decks for Spectre | `spectre/codesign_package.tar.gz` |
| `spectre/compare.py` | accuracy vs Spectre on those decks; eye-vs-filter-cutoff figures | `compare.json`, `figures/fig_eye_cutoffs*`, `fig_eye_spectre*`, `fig_eye_vasim*` |

## 4. The TCAD paper's own figures (`figures/`)

`main.tex` includes six PNGs. They are **manual copies** of files written elsewhere (same MD5);
re-running a script does *not* update `figures/`, you copy the result over:

| `figures/…` (included in) | Produced by | Original file |
|---|---|---|
| `mrm_pin_static.png` (09b) | `mrm_injection/tests/plot_validation.py` | `mrm_injection/tests/validation_static.png` |
| `mrm_pin_eyes.png` (09b) | `mrm_injection/tests/plot_eyes.py va` (data from `eyes.py --sim va`) | `mrm_injection/tests/eyes_8G_va.png` |
| `mrm_eqc_pam8_eye.png` (09b) | `mrm_eqc/tests/t8_eye_power.py --eye` | `mrm_eqc/tests/t8_eye.png` |
| `mrm_eqc_pam8_power.png` (09b) | `mrm_eqc/tests/t8_eye_power.py --power` | `mrm_eqc/tests/t8_power.png` |
| `mrm_eqc_pam4_eyes.png` (09b) | `mrm_eqc/tests/t9_pam4_ffe.py` | `mrm_eqc/tests/t9_pam4_eyes.png` |
| `mrm_tcmt_results.png` (not included at present) | `mrm_tcmt/plot_results.py` | `mrm_tcmt/mrm_results.png` |
| `pipeline.tikz` (03) | hand-written TikZ | — |

**No MWP figure is included in `main.tex` at present.** `mwp/figures/` serves the conference abstract.

Other `mrm_*` scripts, briefly: `mrm_tcmt/reference/make_reference.py` (+ `check_rerun.py`,
`va_rerun_windows.py`) — ngspice vs verilog-a-sim reference runs; `mrm_tcmt/speed/sweep*.py` +
`plot_q_eta.py` — eye vs symbol rate / Q×η sweeps → `q_eta_*.png`; `mrm_tcmt/overshoot/` —
overshoot analysis → `overshoot.png`; `mrm_tcmt/comparison/bench*.py` — timing (Linux paths
hard-coded); `mrm_eqc/tests/harness.py` — one circuit → ngspice and verilog-a-sim decks;
`mrm_eqc/tests/t1…t10` — the checks against [B]; `mrm_eqc/compiler_note/` — OpenVAF bug bisection;
`mrm_injection/calib/calibrate_vact.py`, `tests/eq4_*.py`, `make_eq4.py`, `fc_form_check.py`,
`tolcheck/compare.py` — calibration and model-form checks. Most `mrm_*` scripts expect ngspice +
OpenVAF (Linux) and use `/usr/bin/time`.

---

## 5. Regenerating the MWP figures by hand

### 5.1 Once

```powershell
# from the repo root: build the simulator the scripts call
cargo build --release -p va-cli          # -> target\release\va-cli.exe
pip install numpy scipy matplotlib psutil  # psutil optional (peak memory only)
```
(On Windows-gnu, put the 64-bit MSYS2 binutils first on PATH if the link fails on `dlltool`.)

### 5.2 Run

```powershell
cd paper_writing\mwp\tests
python make_figures.py                  # all 12, a few minutes
python make_figures.py oeo              # only figures whose function name contains "oeo"
python make_figures.py chirp mixer      # several
$env:VACLI = "C:\path\to\other\va-cli.exe"; python make_figures.py beam   # another binary
```

Selection is a **substring of the function name**: `noise` runs both `linewidth_noise` and
`transient_noise`; use `linewidth` or `transient` to pick one. The output **overwrites**
`mwp/figures/<name>.pdf` and `.png` (300 dpi) — copy the old ones aside first if you want to compare.

Each run also leaves its netlist in `tests/work/<tag>.net` (and `.out`, `.err`), so you can
look at or re-run one circuit alone:

```powershell
..\..\..\target\release\va-cli.exe sim work\fig_ip3.net --model ..\lib --tran --report Vs
```

### 5.3 What to edit, figure by figure

All in `mwp/tests/make_figures.py`. Each figure is one function; its **first line holds the
circuit parameters** (change those), the f-string `deck` is the netlist, and the last ~8 lines
are axes, limits and legend.

| Output file | Function (line) | Parameters on the first line | Swept / plotted | Test it mirrors |
|---|---|---|---|---|
| `fig_link_ip3` | `link_ip3` (64) | `P` laser mW, `F1,F2` tones, `T` window | `vas` (tone amplitudes, line 66) | t01 |
| `fig_filter_passband` | `filter_passband` (107) | `NU` THz, `P`, `QC` ring Q, `F0` detuning | tuning voltages 0/4/8 V (line 120), `.ac lin 461 1e9 24e9` | t02 |
| `fig_linewidth_noise` | `linewidth_noise` (148) | `PL`, `TD` delay, `RL` | linewidths 1 MHz/100 kHz/10 kHz (line 151), `.noise … dec 6 1e5 6e9` | t03 |
| `fig_oeo_startup` | `oeo_startup` (183) | `P, RL, RT, F0, QT, GM, TD` | `.tran 1e-12 120e-9`; `VA_ABSDELAY_POINTS=4096` (line 207); envelope bin 0.25 ns | t04 |
| `fig_qam_constellation` | `qam_constellation` (227) | `PS, PLO, FC, BAUD, NSYM, TR`; seed 7 | drive 0.1/0.3 × V_π (line 233) | t05 |
| `fig_chirp` | `chirp` (279) | `PA, PB, F0, T, B` | — | t05 |
| `fig_sampling_spectrum` | `sampling_spectrum` (311) | `PAVG, FREP, FWHM, NS, M, VA`; `--reltol 1e-6` | — | t06 |
| `fig_beam_squint` | `beam_squint` (349) | `P, NEL, NU0, DNU, DISP, LEN, TD0, FDES`; `AC` grid | element spacing `d_el` (line 385) | t07 |
| `fig_mixer_conversion` | `mixer_conversion` (407) | `P, FIF, VRF, VLO, F3` | `frfs` (line 410): one transient per point and per mixer | t08 |
| `fig_dispersion_fading` | `dispersion_fading` (453) | `P, NU0, DISP, LEN, BAND` | 8 and 4 sections (line 461) | t09 |
| `fig_amplifier_gain` | `amplifier_gain` (479) | `G0_DB, NF_DB` | `pin_dbm` (line 482), P_sat 10/1 mW (line 485) | t10 |
| `fig_transient_noise` | `transient_noise` (511) | `T0, R, C, FMAX, GAIN, TSTOP` | Welch `nperseg=16384`, 19 log bins | t11 |

Shared constants, top of the file:

- `RESP, VPI = 0.8, 4.0` (line 58) — photodiode responsivity and modulator V_π for **every** figure.
- Style (lines 29–39): colours `BLACK, BLUE, ORANGE, VIOLET` (a CVD-checked set: keep it or
  re-check contrast), `LW` line width, `MS` marker size, the `rcParams` block (font sizes,
  ticks, `figure.figsize = (7.4, 5.4)`), `MK` (open markers).
- `save()` (line 42): formats, dpi, transparency, padding. `OUT` (line 27): output folder.
- Per figure, `figsize` can be overridden (`qam_constellation` uses `(6.4, 6.0)`), and the
  legend's `loc`, `ncol`, `fontsize` are set in each function's last lines.

### 5.4 Things to keep in mind when you tune

1. **The figure parameters are copies of the tests', not shared with them.** Changing a value in
   `make_figures.py` changes the figure only; `RESULTS.md` and the numbers quoted in the abstract
   still come from `tNN_*.py`. If the text should match the new figure, change the same value in
   the test and run `python run_all.py tNN`.
2. **The closed form is recomputed from the same constants**, so the black curve follows any
   parameter you change on the first line. It does *not* follow a change made only inside the
   `deck` string (e.g. editing `rterm=0` or a model parameter there): keep both in step.
3. **Changing the models** (`mwp/lib/*.va`) changes every figure and every test: re-run
   `run_all.py` after it (about 6 min), and `t12` against Spectre on the machine that has the outputs.
4. Run time grows with points: `mixer_conversion` runs 12 transients, `link_ip3` 8,
   `amplifier_gain` 30 `.op`s; `oeo_startup` and `transient_noise` are the long single runs.
   Changing `TSTOP`, `.tran` steps or `NSYM` scales them directly.
5. A failing simulation raises `RuntimeError: va-cli failed on …\work\<tag>.net` with the tail
   of the output; open that `.net`/`.out` to see why. A `refused[...]` block also fails the run on purpose.
6. The MWP figures do not involve Spectre (only `codesign/` figures overlay Spectre crosses).
   No speed or memory figure against Spectre may be published (Cadence NDA).
7. The harness `mwp.py` exists twice (`mwp/tests/` and `codesign/`): parsing changes go in both.
