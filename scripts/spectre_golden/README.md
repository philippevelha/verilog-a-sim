# Spectre golden references for the circuits that have none

The repository has 173 decks under `circuits/`; 33 have a QSPICE golden reference in `golden/`.
This folder translates the other 140 to Cadence Spectre so they can get one, the same way the
microwave-photonics library did (`paper_writing/mwp/spectre_golden/`).

**Status, 2026-11-13 (afternoon): second server run returned 72 of 86 clean, of which 24
simulated the wrong circuit; 56 kept as golden. Package fixed for a third round** (below). 103 decks translated; 37 left out, each with its reason in `REFUSED.md`. `MANIFEST.md` lists every netlist with its deck,
analyses, models and size class.

## First run on the server (2026-11-12): 33 of 105 clean

| Cause | Decks | Whose | Fixed in the package |
|---|---|---|---|
| model helper files (`.include`, `.inc`) not packaged | 27 | packaging | yes: every file beside a used model is shipped |
| a model instantiates another module whose file was not included (`actuator_plant`, `lib_tee`) | 2 | translator | yes: child modules are followed and included |
| `$table_model` data file not shipped (`thermistor_rt`) | 1 | packaging | yes: copied into the run directory |
| natures reuse `Temp`, `Pos` or `Flux`, which Cadence's standard `disciplines.vams` already defines (VACOMP-1897, and a syntax error for `Flux`) | 31 | **models** | worked around: see below |
| two-dimensional net array `reactant [0:N-1][0:N-1] u, v;` (`gray_scott`) | 2 | **model** | moved to `REFUSED.md`: Spectre rejects it; a model change is needed |
| a model's C compilation failed (`bubble_single`, `grid_*`, SNN `neuron_logic`, `mos_switch`) | 5 | unknown | `run.sh` now keeps Spectre's `ahdlcmi.out` so the next round shows why |
| `$finish` in the model ends the run by design (`tasks/rc_watch`), or exit status without an error (`portprobe_ac`, `seu_collection`) | 3 | — | none needed; to be checked against their results |

**The nature collisions are a real portability defect of the multiphysics models.** Their
directories have no `disciplines.vams`; verilog-a-sim drops the unresolved include and uses its
built-in `electrical`, so the models never met the standard header. Spectre finds Cadence's,
whose `Temperature` (access `Temp`), `Position` (`Pos`) and `Flux` collide with the models' own
natures. The package now ships the repository's `models/disciplines.vams` beside a model
directory whose natures reuse one of the standard access names, so Spectre reads the same
declarations verilog-a-sim does. The portable fix is in the models (distinct access-function
names, or the standard natures); not done, it is a decision.

The six bubble-cloud sweeps saved every net for 9139 time units: 2.7–9.4 GB of waveform each
(23 GB in the archive). Their result is the `$strobe` Poincaré output, which is in the log; they
now save only the drive.

## Do the clean ones check out? (2026-11-13)

33 decks ran cleanly; their outputs are in `golden/spectre/zoo/` (and `zoo_large/` for the seven
largest, not committed). `compare_zoo.py` runs the same decks in verilog-a-sim and compares every
common signal; the full table is in `golden/spectre/zoo/README.md`. In short: **17 of 24 agree**;
4 show a known verilog-a-sim approximation (the port-current probe omits the capacitive current
in transient), 1 is Spectre's `$finish` semantics, 1 is a chaotic-onset event (breaker arc
chopping), 1 needs a better metric for a current spike. Two Spectre results simulated the wrong
circuit (`TYPE` ignored as an instance parameter on BSIM-BULK and BSIM-SOI) and were removed; the
translator is fixed for the next round.

## Second run (2026-11-13): 72 of 86 clean, 56 usable

The standard set only (`run.sh` without `--all`). Full table and the comparison in
`golden/spectre/zoo/README.md`; in short:

- **24 clean runs were the wrong circuit**, which only the warnings in `spectre.log` show:
  `TYPE` on an instance where the module declares it a model parameter (SFE-3239: nine NAND and
  inverter decks), and the SG13G2 PSP103 cards' lower-case names, which Spectre does not match to
  PSP103's upper-case ones and drops (SFE-101: every SNN transistor deck). Both came from
  `module_index` reading only a model's top file. Fixed: it now inlines `` `include``s, records
  each module's declared parameter names, and `spelled()` writes every instance and `.model`
  parameter in the module's own spelling. Regenerating changed 28 netlists and no others: the nine
  `TYPE` decks, NAND `psp102` (its `TYPE` now on a model card too, although Spectre had accepted
  it on the instance and agreed with us), ten standard and six heavy SNN transistor decks
  (`mnist_tx_digit0*`, `xbar_h2_*`), and the two `hh_*` decks of the next point.
  **Check for it before trusting a result:** `grep -l 'SFE-101\|SFE-3239\|SFE-30)' */spectre.log`
  must list nothing for a deck to go into `golden/spectre/zoo/`.
- `save X1.v[0]` stopped `hh_*` (SFE-874): brackets are now escaped, `X1.v\[0\]`. Unverified
  until the next round.
- `thermistor.tbl` used `*` comments, which the LRM does not allow (9.21.1: `#` only); fixed in
  the data file.
- Not fixed, decisions: `gray_scott` (2-D net array, refused; the server's directory still held
  its netlists from round 1, so check it is unpacked fresh), and the li-ion and speaker models,
  whose net-array indices are parameter expressions (`sh[N-1]`), which Spectre's compiler
  rejects although the LRM allows them.
- 41 of the 56 agree with verilog-a-sim. Of the rest, `bsimsoi`'s NAND (our output stuck at
  0.44 V) is the one new disagreement that looks like ours; `portprobe_ac` extends the known
  port-current approximation to AC.

## What is left out, and why

| Group | Decks | Reason |
|---|---|---|
| ISCAS'85 | 15 | up to 268 000 unknowns, hours per run |
| SNN MNIST | 10 | hours per deck |
| untracked models | 3 | `hicum_output_family`, NAND `bsim4`, `ekv26`: their models are in the untracked `external/` corpus, which the package cannot carry |
| optional ports | 7 | `fiber_mzi_noise`, `microring_thermal`(`_fast`), `ring_gyro_noise`, the three `motorway_ramp` decks: an instance leaves an optional port unconnected, which the model detects with `$port_connected`; Spectre needs every port connected, so the translation would be a different circuit |

## Run on the server

```
tar xzf zoo_golden_package.tar.gz
cd zoo_golden
bash run.sh            # the 86 standard decks
bash run.sh --all      # all 105, including the heavy ones (large circuits or long transients)
```

It writes `golden/<netlist>/` (Spectre's ASCII PSF results, the netlist, `spectre.log`),
`summary.txt` and `zoo_golden_results.tar.gz`. Send that file back. The heavy list is
`heavy.txt`: the SNN transistor decks and the long multiphysics transients.

## How the translation works, and its limits

`net2spectre.py` reads a deck as verilog-a-sim does (`+` continuations, `.include` inlined,
`.subckt`/`.ends`, `.model`) and writes the Spectre equivalent: R C L (with `IC=`), V and I
sources (DC, AC, `SIN`, `PULSE`, `PWL`), E G F H, K, and D M Q X instances of Verilog-A modules
or subcircuits; `.tran` (`UIC` -> `skipdc=yes`), `.ac`, `.dc`, `.noise`, `.op`, `.ic`, `.nodeset`,
`.temp`. Each netlist `ahdl_include`s the model files its deck uses, found from the deck's own
documented `va-cli` command line (`--model`), else in `models/`. Anything it cannot render
faithfully refuses the whole deck with the reason; nothing is approximated.

Known limits:

- Values are converted to plain numbers, since SPICE's `meg` and Spectre's `M` disagree.
- Saved signals: the deck's documented `--report` list when it has one (device currents as
  `<name>:p`), otherwise the top-level nets. Internal nodes of a Verilog-A instance are written
  as `X1.node`; whether Spectre accepts that spelling for every vector node is untested.
- **The translator has only been checked by reading its output.** None of these netlists has run
  in Spectre yet; expect a few to fail in the first round, and the logs to say why.
- Model files are those of the repository at packaging time; Cadence's own `disciplines.vams` is
  not used when a model directory carries its own (the repository's headers are included
  relative to each model).

## Regenerating

`python make_package.py` rewrites `netlists/`, `MANIFEST.md`, `REFUSED.md`, `heavy.txt` and
`zoo_golden_package.tar.gz`. Rerun it after adding a deck or a golden file.
