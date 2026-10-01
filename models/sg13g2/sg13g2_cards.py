"""IHP SG13G2 thick-oxide (3.3 V) MOS devices as flat `.model` cards for va-cli.

The IHP Open PDK (https://github.com/IHP-GmbH/IHP-Open-PDK, Apache-2.0) ships its MOS models as
ngspice libraries: a `.subckt sg13_hv_nmos d g s b` wrapper with geometry parameters and
`.if` branches, a `.model ... psp103va` card whose values are expressions in corner parameters
(`sg13g2_hv_nmos_mueo`, ...) and in the instance geometry (`w`, `l`, `ng`), and a `.LIB mos_tt`
section in cornerMOShv.lib that sets the corner parameters. va-cli reads none of `.lib`,
`.param` expressions, `.if` or subcircuit parameters, so this script evaluates them, once per
device geometry, into a plain card for the PDK's own PSP103 (models/sg13g2/psp103/, module
`PSP103VA`), and gives the instance line that uses it.

What is evaluated, and as what:
  - corner: `.LIB mos_tt` of cornerMOShv.lib (typical);
  - pre_layout = 1, rfmode = 0 (the `sg13g2_hv_*_psp` card, not the `_rf`/NQS one);
  - the wrapper's default (`as <= 1e-50`) branch: AS/AD/PS/PD from w, ng, z1, z2, wmin;
  - trise = 0, delvto = 0, factuo = 1 (as the wrapper passes them).
Not modelled: mismatch and statistical corners (the `_mismatch`/`_stat` libraries), the other
corners (ff/ss/fs/sf -- `corner=` selects the section, untested), layout parasitics.

    python models/sg13g2/sg13g2_cards.py [pdk_root] > cards.inc   # self-test: prints two cards
"""
import math
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_PDK = os.path.join(HERE, "..", "..", "external", "ihp-open-pdk")
MODELS = os.path.join("ihp-sg13g2", "libs.tech", "ngspice", "models")
VA_DIR = os.path.join(HERE, "psp103")

# The wrapper's geometry defaults (sg13g2_moshv_mod.lib, `.subckt sg13_hv_nmos`/`pmos`).
Z1, Z2, WMIN = 0.34e-6, 0.38e-6, 0.15e-6


def _psp_params():
    """Every parameter name PSP103VA declares (lower case), so a card carries none it lacks."""
    names = set()
    for f in os.listdir(VA_DIR):
        if f.endswith(".include"):
            with open(os.path.join(VA_DIR, f)) as fh:
                for m in re.finditer(r"`[IM]P[RI]\w\w\(\s*(\w+)", fh.read()):
                    names.add(m.group(1).lower())
    return names


def _corner(pdk, corner):
    vals = {}
    path = os.path.join(pdk, MODELS, "cornerMOShv.lib")
    inside = False
    with open(path) as fh:
        for line in fh:
            s = line.strip()
            if s.upper().startswith(".LIB "):
                inside = s.split()[1].lower() == corner
            elif s.upper().startswith(".ENDL"):
                inside = False
            elif inside and s.lower().startswith(".param"):
                k, v = s[len(".param"):].split("=")
                vals[k.strip().lower()] = float(v)
    if not vals:
        raise SystemExit(f"no `.LIB {corner}` section in {path}")
    return vals


def _raw_card(pdk, name):
    """The `key = value` pairs of `.model <name>`, values as written (quoted or not)."""
    path = os.path.join(pdk, MODELS, "sg13g2_moshv_parm.lib")
    with open(path) as fh:
        text = fh.read()
    m = re.search(r"^\.model\s+" + name + r"\s+\S+(.*?)(?=^\s*$|^\.model)", text, re.M | re.S)
    if not m:
        raise SystemExit(f"no `.model {name}` in {path}")
    body = re.sub(r"^\+", " ", m.group(1), flags=re.M)
    return re.findall(r"(\w+)\s*=\s*('[^']*'|[^\s']+)", body)


def _eval(expr, env):
    expr = expr.strip("'").strip().lower()  # SPICE names are case-insensitive
    # SPICE `c ? a : b` (only inside the RF card's parentheses) -> Python
    expr = re.sub(r"\(([^()?]+)\?([^():]+):([^()]+)\)", r"((\2) if (\1) else (\3))", expr)
    return float(eval(expr, {"__builtins__": {}, "max": max, "min": min}, env))


def card(kind, w, l, ng=1, corner="mos_tt", pdk=DEFAULT_PDK):
    """(model name, card text, instance-parameter string) for one sg13_hv_<kind> geometry.

    `kind` is "nmos" or "pmos"; `w`, `l` in metres (total width, as the PDK's `w`)."""
    if kind not in ("nmos", "pmos"):
        raise ValueError(kind)
    env = dict(_corner(pdk, corner))
    env.update(w=w, l=l, ng=ng, pre_layout=1, rfmode=0, swsoa=0.0)
    known = _psp_params()
    params = []
    for k, v in _raw_card(pdk, f"sg13g2_hv_{kind}_psp"):
        k = k.lower()
        if k == "level" or k not in known:
            continue
        params.append((k, _eval(v, env)))
    wf = max(w / ng, WMIN)
    if math.floor(math.floor(ng / 2 + 0.501) * 2 + 0.001) != ng:  # odd finger count
        as_ = ad = wf * (Z1 + ((ng - 1) / 2) * Z2)
        ps = pd = 2 * (wf * ((ng - 1) / 2 + 1) + Z1 + (ng - 1) / 2 * Z2)
    else:
        as_ = wf * (2 * Z1 + max(0, (ng - 2) / 2) * Z2)
        ad = wf * Z2 / 2 * ng
        ps = 2 * (wf * (2 + max(ng - 2, 0) / 2) + 2 * Z1 + max(ng - 2, 0) / 2 * Z2)
        pd = (wf + Z2) * ng
    name = f"hv{kind[0]}_w{w * 1e9:.0f}n_l{l * 1e9:.0f}n_ng{ng}"
    lines = [f".model {name} PSP103VA"]
    lines += [f"+ {k}={v:.10g}" for k, v in params]
    inst = (f"w={w:.6g} l={l:.6g} nf={ng} mult=1 as={as_:.6g} ad={ad:.6g} ps={ps:.6g} "
            f"pd={pd:.6g} ngcon=2 delvto=0 factuo=1")
    return name, "\n".join(lines), inst


if __name__ == "__main__":
    pdk = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_PDK
    for kind in ("nmos", "pmos"):
        name, text, inst = card(kind, 1e-6, 0.45e-6, pdk=pdk)
        print(f"* {name}: X<name> d g s b {name} {inst}")
        print(text)
