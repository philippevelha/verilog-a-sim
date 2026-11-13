#!/usr/bin/env python3
"""Reader for Spectre's ASCII PSF result files (spectre -format psfascii).

    d = read("work/01_link_ac/ac1.ac")   ->  {"freq": array, "Vs:p": complex array, ...}

Written from the format's general layout (HEADER / TYPE / SWEEP / TRACE / VALUE sections) and
checked only against a hand-made sample (the self-test below), not against a file Spectre
wrote: if a real file does not parse, `traces(path)` prints what the VALUE section contains.
A swept file gives one array per trace; an unswept one (a DC point) one-element arrays.
A parenthesised pair is a complex number; a longer parenthesised group is kept as a tuple.
"""
import re, sys
import numpy as np

_NUM = r"[-+]?(?:\d+\.?\d*(?:[eE][-+]?\d+)?|\.\d+(?:[eE][-+]?\d+)?|inf|nan)"
_ITEM = re.compile(r'"([^"]*)"\s+(\([^()]*\)|' + _NUM + r')', re.I)


def _sections(text):
    out, name = {}, None
    for line in text.splitlines():
        s = line.strip()
        if s in ("HEADER", "TYPE", "SWEEP", "TRACE", "VALUE", "END"):
            name = s; out[name] = []
        elif name:
            out[name].append(line)
    return {k: "\n".join(v) for k, v in out.items()}


def read(path):
    sec = _sections(open(path, encoding="utf-8", errors="replace").read())
    if "VALUE" not in sec:
        raise ValueError(f"{path}: no VALUE section (not an ASCII PSF file?)")
    sweep = None
    if sec.get("SWEEP", "").strip():
        m = re.search(r'"([^"]*)"', sec["SWEEP"])
        sweep = m.group(1) if m else None
    data = {}
    if sweep is None:
        # an unswept result (a DC operating point) lists `"name" "type" value`
        for name, typ, val in re.findall(r'"([^"]*)"\s+"([^"]*)"\s+(' + _NUM + r')', sec["VALUE"], re.I):
            data.setdefault(name, []).append(float(val))
        if data:
            out = {k: np.array(v) for k, v in data.items()}
            out["_sweep"] = None
            return out
    for name, val in _ITEM.findall(sec["VALUE"]):
        if val.startswith("("):
            nums = [float(x) for x in val[1:-1].split()]
            v = complex(nums[0], nums[1]) if len(nums) == 2 else tuple(nums)
        else:
            v = float(val)
        data.setdefault(name, []).append(v)
    out = {}
    for k, v in data.items():
        try:
            out[k] = np.array(v)
        except Exception:
            out[k] = v
    out["_sweep"] = sweep
    return out


def traces(path):
    d = read(path)
    for k, v in d.items():
        if k != "_sweep":
            print(f"{k}: {len(v)} value(s)" + ("  <- sweep" if k == d["_sweep"] else ""))


_SAMPLE = '''HEADER
"PSFversion" "1.00"
"simulator" "spectre"
TYPE
"sweep" FLOAT DOUBLE PROP(
"key" "sweep"
)
"I" COMPLEX DOUBLE PROP(
"units" "A"
)
SWEEP
"freq" "sweep" PROP(
"units" "Hz"
)
TRACE
"Vs:p" "I"
VALUE
"freq" 1.000000e+08
"Vs:p" (-3.141593e-03 1.5e-09)
"freq" 1.258925e+08
"Vs:p" (-3.141590e-03 2.0e-09)
END
'''

if __name__ == "__main__":
    if len(sys.argv) > 1:
        traces(sys.argv[1])
    else:
        import tempfile, os
        with tempfile.NamedTemporaryFile("w", suffix=".ac", delete=False) as f:
            f.write(_SAMPLE)
        d = read(f.name); os.unlink(f.name)
        assert d["_sweep"] == "freq" and len(d["freq"]) == 2 and d["Vs:p"][1] == complex(-3.141590e-03, 2.0e-09), d
        print("self-test on a hand-made sample: ok (not a file written by Spectre)")
