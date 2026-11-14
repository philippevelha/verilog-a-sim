#!/usr/bin/env python3
"""Translate a verilog-a-sim netlist (.net) into a Cadence Spectre netlist (.scs).

    convert(path, model_dirs) -> (text, modules_used, problems)

`problems` is a list of reasons the deck cannot be translated faithfully; when it is non-empty
the deck is reported as refused and no netlist is written for it (a netlist that silently means
something else is worse than none). Covered:

  R C L (with IC=), V I (DC, AC mag [phase], SIN PULSE PWL), E G F H, K, D M Q and X instances
  of Verilog-A modules or of .subckt definitions, .subckt/.ends (with parameters), .model,
  .include (inlined), + continuations, .tran [UIC], .ac dec|oct|lin, .dc, .noise, .op, .ic,
  .nodeset, .temp. .options, .global and .control blocks are dropped (simulator settings).

Values are converted to plain numbers: SPICE's `meg` is mega and `m` milli, while Spectre's
own scale factor `M` is mega, so suffixes are never passed through. Braced parameter
expressions `{...}` are passed to Spectre unbraced.
"""
import os, re

SUFFIX = [("meg", 1e6), ("mil", 25.4e-6), ("t", 1e12), ("g", 1e9), ("k", 1e3), ("m", 1e-3),
          ("u", 1e-6), ("n", 1e-9), ("p", 1e-12), ("f", 1e-15), ("a", 1e-18)]
NUM = re.compile(r"^([-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?)([a-zA-Z]*)$")


def value(tok):
    """A SPICE number with an optional scale suffix and unit letters -> float, else None."""
    m = NUM.match(tok.strip())
    if not m:
        return None
    x, suf = float(m.group(1)), m.group(2).lower()
    for s, f in SUFFIX:
        if suf.startswith(s):
            return x * f
    return x                                # unit letters only ("V", "Hz", "s") or none


def num(tok):
    v = value(tok)
    return f"{v:.12g}" if v is not None else tok


def param(tok):
    """name=value with the value made Spectre-safe."""
    k, _, v = tok.partition("=")
    v = v.strip()
    if v.startswith("{") and v.endswith("}"):
        v = v[1:-1]
    return f"{k}={num(v)}"


def node(n):
    return "0" if n.lower() in ("0", "gnd") else n


def read_lines(path, seen=None):
    """Lines of `path` with .include inlined (relative to the including file) and + joined."""
    seen = seen or set()
    path = os.path.normpath(path)
    if path in seen:
        raise ValueError(f"recursive .include of {path}")
    seen = seen | {path}
    out = []
    for raw in open(path, encoding="utf-8", errors="replace").read().splitlines():
        line = raw.rstrip()
        s = line.strip()
        if not s or s.startswith("*"):
            continue
        if s.lower().startswith(".include"):
            f = s.split(None, 1)[1].strip().strip("'\"")
            out += read_lines(os.path.join(os.path.dirname(path), f), seen)
            continue
        if s.startswith("+") and out:
            out[-1] += " " + s[1:].strip()
        else:
            out.append(s)
    return out


PORTS = {}                                   # module name -> number of ports
INSTANCE_PARAMS = {}                         # module -> set of instance parameters, when the model declares them
DECLARED = {}                                # module -> {lower-case name: declared spelling} of its parameters


def with_includes(f, seen=()):
    """Source of `f` with its `include files inlined (relative to `f`; a missing one is dropped).

    The CMC models declare their parameters through macros defined and called in included files
    (PSP103's Common103_macrodefs.include, BSIM-CMG's bsimcmg_macros.include), so a scan of the
    top file alone misses both the parameter names and their `type="instance"` tags."""
    f = os.path.normpath(f)
    if f in seen:
        return ""
    try:
        src = open(f, encoding="utf-8", errors="replace").read()
    except OSError:
        return ""
    def inc(m):
        return with_includes(os.path.join(os.path.dirname(f), m.group(1)), seen + (f,))
    return re.sub(r'^[ \t]*`include\s+"([^"]+)"', inc, src, flags=re.M)


def declared_params(src):
    """Parameter names a (comment-stripped, include-expanded) source declares, directly or
    through a parameter-declaring macro (`IPRco(TYPE, ...)`), and through aliasparam."""
    names = set(re.findall(r"\bparameter\s+(?:real|integer|string)?\s*([A-Za-z_]\w*)\s*=", src))
    names |= set(re.findall(r"\baliasparam\s+([A-Za-z_]\w*)", src))
    for mac in re.findall(r"`define\s+(\w+)\s*\([^)]*\)[^\n]*\bparameter\b", src):
        names |= set(re.findall(r"`" + mac + r"\s*\(\s*([A-Za-z_]\w*)", src))
    return names


def spelled(mod, tok):
    """`name=value` with the name in the spelling module `mod` declares it.

    A SPICE netlist's parameter names are case-insensitive (verilog-a-sim matches them so); a
    Spectre netlist's are not, and Spectre drops a misspelt one with only a warning (SFE-101 --
    round 2 simulated every SG13G2 PSP103 card with `type`, `tr`, ... ignored). A name that the
    module does not declare, or declares in two spellings, is left as written."""
    k, eq, v = tok.partition("=")
    d = DECLARED.get(mod)
    if not eq or not d or k in d.values():
        return tok
    return f"{d.get(k.lower(), k)}={v}"


def module_index(roots):
    """Verilog-A module name -> list of files defining it, over the given directories/files."""
    idx = {}
    files = []
    for r in roots:
        if os.path.isfile(r):
            files.append(r)
        else:
            for d, _, fs in os.walk(r):
                files += [os.path.join(d, f) for f in fs if f.endswith((".va", ".vams"))]
    for f in files:
        src = with_includes(f)
        if not src:
            continue
        src = re.sub(r"//.*", "", src)
        # A macro defined as nothing takes its arguments with it: PSP102 writes every attribute
        # as `P(type="instance" ...) with `define P(txt) empty, so the compiler never sees those
        # tags and every parameter is an instance parameter. Counting them anyway moved TYPE to
        # a model card, where Spectre then ignored it (round 3, SFE-3238).
        for mac in re.findall(r"^[ \t]*`define[ \t]+(\w+)\([^)]*\)[ \t]*$", src, re.M):
            src = re.sub(r"`" + mac + r"\s*\((?:[^()]|\([^()]*\))*\)", "", src)
        names = declared_params(src)
        lower = {}
        for n in names:
            lower.setdefault(n.lower(), set()).add(n)
        spell = {k: next(iter(v)) for k, v in lower.items() if len(v) == 1}
        # per-parameter instance tagging, `(* type = "instance" *)`, directly or through macros
        # (the CMC models' IPR*/IPI* macros): every untagged parameter is then a model parameter
        tagged = None
        if re.search(r'type\s*=\s*"instance"', src):
            macros = re.findall(r'`define\s+(\w+)\s*\([^)]*\)[^\n]*type\s*=\s*"instance"', src)
            tagged = set()
            for mac in macros:
                tagged |= set(re.findall(r"`" + mac + r"\s*\(\s*(\w+)", src))
            tagged |= set(re.findall(r'type\s*=\s*"instance"[^*]*\*\)\s*parameter\s+(?:real|integer)\s+(\w+)', src))
        for m in re.finditer(r"(?<![A-Za-z_$])(?:module|macromodule)\s+([A-Za-z_][\w$]*)\s*(\(([^)]*)\))?", src):
            idx.setdefault(m.group(1), []).append(os.path.normpath(f))
            PORTS[m.group(1)] = len([x for x in (m.group(3) or "").split(",") if x.strip()])
            DECLARED[m.group(1)] = spell            # per file: a file with two modules shares one map
            before = src[max(0, m.start() - 4000):m.start()]
            att = re.findall(r"instance_parameter_list\s*=\s*\{([^}]*)\}", before)
            if att:
                INSTANCE_PARAMS[m.group(1)] = {x.strip().strip('"') for x in att[-1].split(",") if x.strip()}
            elif tagged is not None:
                INSTANCE_PARAMS[m.group(1)] = tagged
    return idx


def split_source(toks):
    """V/I value tokens -> (dc, ac (mag, phase) or None, waveform (kind, [numbers]) or None)."""
    s = " ".join(toks)
    wave = None
    m = re.search(r"\b(SIN|PULSE|PWL|EXP)\s*\(([^)]*)\)", s, re.I)
    if m:
        wave = (m.group(1).upper(), [num(x) for x in m.group(2).replace(",", " ").split()])
        s = s[:m.start()] + " " + s[m.end():]
    t = s.split()
    dc, ac = None, None
    i = 0
    while i < len(t):
        w = t[i].upper()
        if w == "DC" and i + 1 < len(t):
            dc = num(t[i + 1]); i += 2
        elif w == "AC" and i + 1 < len(t):
            mag = num(t[i + 1]); ph = "0"
            if i + 2 < len(t) and value(t[i + 2]) is not None:
                ph = num(t[i + 2]); i += 1
            ac = (mag, ph); i += 2
        elif value(t[i]) is not None and dc is None:
            dc = num(t[i]); i += 1
        else:
            raise ValueError(f"source token `{t[i]}`")
    return dc, ac, wave


def source_params(dc, ac, wave):
    p = []
    if wave:
        kind, a = wave
        if kind == "SIN":
            names = ["sinedc", "ampl", "freq", "delay", "damp", "sinephase"]
            p.append("type=sine")
            p += [f"{n}={v}" for n, v in zip(names, a)]
            if dc is None:
                dc = a[0]
        elif kind == "PULSE":
            names = ["val0", "val1", "delay", "rise", "fall", "width", "period"]
            p.append("type=pulse")
            p += [f"{n}={v}" for n, v in zip(names, a)]
            if dc is None:
                dc = a[0]
        elif kind == "PWL":
            p.append("type=pwl")
            pts = [f"{a[i]} {a[i + 1]}" for i in range(0, len(a) - 1, 2)]
            rows = [" ".join(pts[i:i + 6]) for i in range(0, len(pts), 6)]
            p.append("wave=[ \\\n    " + " \\\n    ".join(rows) + " ]")
            if dc is None and a:
                dc = a[1]
        else:
            raise ValueError(f"{kind} waveform")
    if dc is not None:
        p.insert(0, f"dc={dc}")
    if ac:
        p += [f"mag={ac[0]}", f"phase={ac[1]}"]
    return " ".join(p)


def convert(path, model_roots):
    idx = module_index(model_roots)
    lines = read_lines(path)
    out, probs, used = [], [], []
    subckts = set(l.split()[1] for l in lines if l.lower().startswith(".subckt") and len(l.split()) > 1)
    models = {}                                 # .model name -> master
    for l in lines:
        t = l.split()
        if t[0].lower() == ".model" and len(t) >= 3:
            models[t[1]] = re.sub(r"\(.*", "", t[2])
    analyses, uic, temp = [], False, None
    in_control = False
    stack = []

    def master(name):
        """A model name -> the Verilog-A module (or subckt) it instantiates, recording the file."""
        if name in subckts:
            return name
        mod = models.get(name, name)
        if mod in idx:
            f = idx[mod][0]
            if f not in used:
                used.append(f)
            return name
        probs.append(f"`{name}` is neither a .subckt nor a Verilog-A module in {model_roots}")
        return name

    for l in lines:
        t = l.split()
        k = t[0].lower()
        if in_control:
            if k == ".endc":
                in_control = False
            continue
        if k == ".control":
            in_control = True; continue
        if k == ".end":
            break
        try:
            if k in (".options", ".option", ".global", ".width", ".probe", ".save", ".print", ".plot"):
                continue
            if k == ".subckt":
                ports = [p for p in t[2:] if "=" not in p]
                pars = [param(p) for p in t[2:] if "=" in p]
                out.append(f"subckt {t[1]} ({' '.join(ports)})")
                if pars:
                    out.append("parameters " + " ".join(pars))
                stack.append(t[1]); continue
            if k == ".ends":
                out.append(f"ends {stack.pop() if stack else ''}"); continue
            if k == ".model":
                pars = " ".join(param(spelled(models[t[1]], p))
                                for p in re.sub(r"[()]", " ", " ".join(t[3:])).split() if "=" in p)
                master(t[1])
                out.append(f"model {t[1]} {models[t[1]]} {pars}".rstrip()); continue
            if k == ".tran":
                rest = [x for x in t[1:] if x.upper() != "UIC"]
                uic = uic or any(x.upper() == "UIC" for x in t[1:])
                step, stop = num(rest[0]), num(rest[1])
                analyses.append(("tran", f"tr tran stop={stop} step={step}")); continue
            if k == ".ac":
                kind, n, f1, f2 = t[1].lower(), int(value(t[2])), num(t[3]), num(t[4])
                grid = {"dec": f"dec={n}", "oct": f"oct={n}", "lin": f"lin={max(n - 1, 1)}"}[kind]
                analyses.append(("ac", f"ac1 ac start={f1} stop={f2} {grid}")); continue
            if k == ".dc":
                analyses.append(("dc", f"sw dc dev={t[1]} param=dc start={num(t[2])} stop={num(t[3])} step={num(t[4])}")); continue
            if k == ".noise":
                m = re.match(r"V\(\s*([^,)\s]+)\s*(?:,\s*([^)\s]+))?\s*\)", t[1], re.I)
                if not m:
                    raise ValueError(f".noise output `{t[1]}`")
                outp, ref = node(m.group(1)), node(m.group(2) or "0")
                kind, n, f1, f2 = t[3].lower(), int(value(t[4])), num(t[5]), num(t[6])
                grid = {"dec": f"dec={n}", "oct": f"oct={n}", "lin": f"lin={max(n - 1, 1)}"}[kind]
                analyses.append(("noise", f"nz ({outp} {ref}) noise start={f1} stop={f2} {grid} iprobe={t[2]}")); continue
            if k == ".op":
                analyses.append(("op", "op1 dc")); continue
            if k in (".ic", ".nodeset"):
                pairs = re.findall(r"V\(\s*([^)\s]+)\s*\)\s*=\s*(\S+)", " ".join(t[1:]), re.I)
                out.append(("ic " if k == ".ic" else "nodeset ") + " ".join(f"{node(a)}={num(b)}" for a, b in pairs)); continue
            if k == ".temp":
                temp = num(t[1]); continue
            if k.startswith("."):
                raise ValueError(f"card `{t[0]}`")
            e, name = k[0], t[0]
            if e == "r":
                out.append(f"{name} ({node(t[1])} {node(t[2])}) resistor r={num(t[3])}")
            elif e in "cl":
                ic = [x for x in t[4:] if x.upper().startswith("IC=")]
                extra = f" ic={num(ic[0][3:])}" if ic else ""
                dev = "capacitor c" if e == "c" else "inductor l"
                out.append(f"{name} ({node(t[1])} {node(t[2])}) {dev}={num(t[3])}{extra}")
            elif e in "vi":
                dc, ac, wave = split_source(t[3:])
                out.append(f"{name} ({node(t[1])} {node(t[2])}) {'vsource' if e == 'v' else 'isource'} {source_params(dc, ac, wave)}".rstrip())
            elif e in "eg":
                if len(t) != 6:
                    raise ValueError(f"{t[0]}: only the linear four-node form is translated")
                dev = "vcvs gain" if e == "e" else "vccs gm"
                out.append(f"{name} ({node(t[1])} {node(t[2])} {node(t[3])} {node(t[4])}) {dev}={num(t[5])}")
            elif e in "fh":
                dev = "cccs gain" if e == "f" else "ccvs rm"
                out.append(f"{name} ({node(t[1])} {node(t[2])}) {dev}={num(t[4])} probe={t[3]}")
            elif e == "k":
                out.append(f"{name} mutual_inductor coupling={num(t[3])} ind1={t[1]} ind2={t[2]}")
            elif e in "dmqxn":
                toks = t[1:]
                pars = [x for x in toks if "=" in x]
                rest = [x for x in toks if "=" not in x]
                nodes, mname = rest[:-1], rest[-1]
                master(mname)
                mod = models.get(mname, mname)
                pars = [spelled(mod, x) for x in pars]
                if mname not in subckts and PORTS.get(mod) and len(nodes) != PORTS[mod]:
                    raise ValueError(f"{len(nodes)} terminals on `{mod}`, which has {PORTS[mod]} ports "
                                     "(an unconnected optional port is $port_connected-dependent; Spectre "
                                     "would not see it as unconnected)")
                inst = ("X" + name) if e == "n" else name
                ip = INSTANCE_PARAMS.get(mod)
                if ip is not None and mname not in models:
                    # The module declares which of its parameters are instance parameters; Spectre
                    # ignores any other on an instance (SFE-30), so those go on a model card.
                    mp = [x for x in pars if x.split("=")[0] not in ip]
                    pars = [x for x in pars if x.split("=")[0] in ip]
                    if mp:
                        card = f"{inst}_model"
                        out.append(f"model {card} {mod} {' '.join(param(p) for p in mp)}")
                        mname = card
                out.append(f"{inst} ({' '.join(node(n) for n in nodes)}) {mname} {' '.join(param(p) for p in pars)}".rstrip())
            else:
                raise ValueError(f"element `{t[0]}`")
        except (ValueError, IndexError, KeyError) as err:
            probs.append(f"`{l[:70]}`: {err}")
    return out, used, analyses, uic, temp, probs
