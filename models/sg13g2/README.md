# IHP SG13G2 MOS devices (open PDK)

The thick-oxide 3.3 V MOSFETs (`sg13_hv_nmos`, `sg13_hv_pmos`) of IHP's **SG13G2** open 130 nm
BiCMOS PDK, for the transistor-level SNN decks in `circuits/snn/transistor/`. The SNN of Xuan et
al. (IJCNN 2026) is designed in IHP **SG13S** with the MEMRES RRAM module, which is not public;
SG13G2 is its public sibling on the same 130 nm platform, used here until the SG13S models
arrive. **It is not the paper's process**, and a result with these devices is not the paper's
circuit.

## What is here

- `psp103/` — the PDK's own PSP 103.8.2 Verilog-A (module `PSP103VA`), copied unchanged from
  `IHP-Open-PDK/ihp-sg13g2/libs.tech/verilog-a/psp103` at commit `5e6d592e`. It differs from
  `benchmark/models/psp103/` by IHP's safe-operating-area checks (`SWSOA`, `VGS_MAX`, …; off by
  default) and an integer `NF`. `discipline.h` is added (the PDK expects the simulator to supply
  it); it is the copy in `benchmark/models/psp103/`. Builds self-contained through
  `va-cli check --codegen`: 4 ports, 13 nodes, 887 parameters.
- `sg13g2_cards.py` — turns the PDK's ngspice libraries (`sg13g2_moshv_parm.lib`,
  `cornerMOShv.lib`, the `sg13g2_moshv_mod.lib` wrapper) into flat `.model` cards va-cli reads,
  one per device geometry, plus the instance line. It needs the PDK checked out at
  `external/ihp-open-pdk` (gitignored):

      git clone --depth 1 --filter=blob:none --sparse https://github.com/IHP-GmbH/IHP-Open-PDK external/ihp-open-pdk
      git -C external/ihp-open-pdk sparse-checkout set ihp-sg13g2/libs.tech/ngspice/models ihp-sg13g2/libs.tech/verilog-a

## Limitations

- **Typical corner only** (`.LIB mos_tt`); `corner=` selects another section but is untested.
  No mismatch or statistical models.
- Pre-layout (`pre_layout = 1`), non-RF (`rfmode = 0`: the quasi-static `_psp` card, not the
  NQS `_rf` one), no layout parasitics.
- **Not validated against a reference simulator.** QSPICE cannot run PSP103 Verilog-A, and the
  PDK's reference flow (ngspice + OpenVAF) is not installed here, by decision (2026-10-01): these
  devices are a stand-in until the SG13S models arrive. A smoke test only: W/L = 1/0.45 µm NMOS,
  V_DS = 3.3 V, I_D = 1.07 µA at V_GS = 0.6 V and 533 µA at 3.3 V.
- PDK validity range: L ≥ 0.45 µm (NMOS), 0.40 µm (PMOS), W 0.30–10 µm per finger;
  `circuits/snn/transistor/tech.py` refuses geometries outside it.

## License

The PDK, and the cards generated from it, are Apache-2.0 (IHP GmbH). PSP 103 carries its own
copyright notice in the source (NXP Semiconductors, CEA-Leti, TU Delft, Arizona State
University) and is redistributed as the PDK redistributes it, as `benchmark/models/psp103/` is.
As under `benchmark/`, the license of this repository does **not** apply to `psp103/`.
