#!/usr/bin/env python3
"""Spectre golden references for the repository's circuits that have none.

    python scripts/spectre_golden/make_package.py

For every deck under circuits/ without a golden/<stem>.golden, translate it to Spectre
(net2spectre.py) with the model library its own `Run:`/`va-cli` line names, and write:

  netlists/<name>.scs           one per translatable deck (name = path under circuits/, '/' -> '__')
  MANIFEST.md                   deck -> netlist, analyses, models, size class
  REFUSED.md                    decks left out, each with its reason
  zoo_golden_package.tar.gz     netlists + run.sh + the model files they include

Left out on purpose, before translation: circuits/benchmark/iscas85 (up to 268 000 unknowns) and
circuits/snn/mnist (hours per deck). A deck whose model comes from the untracked external/
corpus is refused after translation (the package can only carry tracked files). Everything else is attempted; a deck the translator cannot render faithfully
is refused with the reason rather than approximated.
"""
import os, re, sys, tarfile, shutil, glob
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from net2spectre import convert, value, module_index  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
OUT = os.path.join(HERE, "netlists")
SKIP = [("benchmark/iscas85/", "ISCAS'85: up to 268 000 unknowns, hours per run"),
        ("snn/mnist/", "hours per deck"),
        ("multiphysics/gray_scott", "two-dimensional net arrays in gray_scott.va, which Spectre rejects (VACOMP-2259); needs a model change")]
HEAD = ('simulator lang=spectre\nglobal 0\n'
        "opts options reltol=1e-6 vabstol=1e-9 iabstol=1e-15 temp=27 tnom=27 gmin=1e-18 save=selected\n")


def rel(p):
    return os.path.relpath(p, REPO).replace("\\", "/")


def run_line(path):
    """--model arguments and --report names from the deck's documented command line."""
    head = []
    for l in open(path, encoding="utf-8", errors="replace").read().splitlines()[:80]:
        if l.strip().startswith("*"):
            head.append(l.strip().lstrip("*").strip().rstrip("\\"))
    text = " ".join(head)
    if "va-cli" not in text:
        return [], []
    models = re.findall(r"--model\s+(\S+)", text)
    rep = re.findall(r"--report\s+(\S+)", text)
    return [m.replace("\\", "/") for m in models], (rep[0].split(",") if rep else [])


def main():
    golden = {os.path.splitext(f)[0] for f in os.listdir(os.path.join(REPO, "golden")) if f.endswith(".golden")}
    decks = sorted(glob.glob(os.path.join(REPO, "circuits", "**", "*.net"), recursive=True))
    os.makedirs(OUT, exist_ok=True)
    for f in glob.glob(os.path.join(OUT, "*.scs")):
        os.remove(f)
    manifest, refused, needed = [], [], set()
    for d in decks:
        r = rel(d); stem = os.path.splitext(os.path.basename(d))[0]
        if stem in golden:
            continue
        why = next((w for s, w in SKIP if s in r), None)
        if why:
            refused.append((r, f"left out: {why}")); continue
        models, report = run_line(d)
        roots = [os.path.join(REPO, m) for m in models] or [os.path.join(REPO, "models")]
        try:
            body, used, analyses, uic, temp, probs = convert(d, roots)
        except Exception as e:                          # noqa: BLE001
            refused.append((r, f"not translated: {e}")); continue
        # child modules a used model instantiates (Spectre needs their files included too)
        idx = module_index(roots)
        k = 0
        while k < len(used):
            src = re.sub(r"//.*", "", open(used[k], encoding="utf-8", errors="replace").read())
            for m in re.finditer(r"^\s*([A-Za-z_]\w*)\s+(?:#\s*\([^;]*?\)\s*)?[A-Za-z_]\w*\s*\(", src, re.M):
                if m.group(1) in idx and idx[m.group(1)][0] not in used:
                    used.append(idx[m.group(1)][0])
            k += 1
        untracked = [rel(f) for f in used if rel(f).startswith("external/")]
        if untracked:
            probs.append("model from the untracked external/ corpus: " + ", ".join(untracked))
        if probs:
            refused.append((r, "not translated: " + "; ".join(probs[:3]) + (" ..." if len(probs) > 3 else ""))); continue
        name = r[len("circuits/"):-4].replace("/", "__")
        incl = [f'ahdl_include "{rel(f)}"' for f in used]
        needed.update(used)
        if not analyses:
            analyses = [("op", "op1 dc")]
        if uic:
            analyses = [(k, a + " skipdc=yes" if k == "tran" else a) for k, a in analyses]
        elem = {l.split()[0] for l in body if l and l[0].isalpha() and " (" in l}
        saves = []
        for q in report:
            if q in elem and q[0].upper() in "VILEH":
                saves.append(f"{q}:p")
            else:
                # a vector node (X1.v[0]): Spectre's parser reads a bare `[` as syntax (SFE-874,
                # round 2, hh_cable), so the brackets of a name are escaped
                saves.append(q.replace("[", "\\[").replace("]", "\\]"))
        if "bubble_cloud_sweep" in name:
            saves = ["pb"]          # the result is the $strobe Poincare output in the log, not waveforms
        save = ("save " + " ".join(saves)) if saves else "opts2 options save=lvlpub nestlvl=1"
        n_inst = sum(1 for l in body if " (" in l)
        steps = 0
        for k, a in analyses:
            if k == "tran":
                st = value(re.search(r"step=(\S+)", a).group(1)); sp = value(re.search(r"stop=(\S+)", a).group(1))
                steps = max(steps, sp / st if st else 0)
        heavy = n_inst > 300 or steps > 2e5
        lines = [f"// {name} -- Spectre translation of {r} (scripts/spectre_golden/net2spectre.py)", HEAD]
        if temp is not None:
            lines.append(f"optT options temp={temp} tnom=27")
        lines += incl + [""] + body + ["", save] + [a for _, a in analyses] + [""]
        open(os.path.join(OUT, name + ".scs"), "w", newline="\n", encoding="utf-8").write("\n".join(lines))
        manifest.append((name, r, ", ".join(k for k, _ in analyses), ", ".join(rel(f) for f in used) or "built-ins only",
                         "heavy" if heavy else "standard", n_inst))
    with open(os.path.join(HERE, "MANIFEST.md"), "w", newline="\n", encoding="utf-8") as f:
        f.write("# Manifest: Spectre netlists for the circuits without a golden reference\n\n")
        f.write(f"{len(manifest)} translated ({sum(m[4] == 'standard' for m in manifest)} standard, "
                f"{sum(m[4] == 'heavy' for m in manifest)} heavy), {len(refused)} left out (REFUSED.md).\n\n")
        f.write("| Netlist | Deck | Analyses | Models | Class | Instances |\n|---|---|---|---|---|---|\n")
        for m in manifest:
            f.write(f"| `{m[0]}` | `{m[1]}` | {m[2]} | {m[3]} | {m[4]} | {m[5]} |\n")
    with open(os.path.join(HERE, "REFUSED.md"), "w", newline="\n", encoding="utf-8") as f:
        f.write("# Circuits without a golden reference that are not in the Spectre package\n\n| Deck | Reason |\n|---|---|\n")
        for r, w in refused:
            f.write(f"| `{r}` | {w.replace('|', '/')} |\n")
    open(os.path.join(HERE, "heavy.txt"), "w", newline="\n").write("\n".join(m[0] for m in manifest if m[4] == "heavy") + "\n")
    # model files plus every header in their directories (for `include)
    # every file beside a used model (include helpers such as .include/.inc, headers), and the
    # data files of $table_model, which Spectre opens relative to the run directory
    files, data = set(needed), set()
    for f in needed:
        dd = os.path.dirname(f)
        for g in os.listdir(dd):
            q = os.path.join(dd, g)
            if os.path.isfile(q) and os.path.getsize(q) < 20e6:
                files.add(q)
        src = open(f, encoding="utf-8", errors="replace").read()
        for m in re.finditer(r'\$table_model\s*\([^;]*?"([^"]+)"', src):
            q = os.path.join(dd, m.group(1))
            if os.path.isfile(q):
                data.add(q)
    tar = os.path.join(HERE, "zoo_golden_package.tar.gz")
    with tarfile.open(tar, "w:gz") as tf:
        for n in sorted(os.listdir(OUT)):
            tf.add(os.path.join(OUT, n), arcname=f"zoo_golden/{n}")
        for n in ("run.sh", "README.md", "MANIFEST.md", "REFUSED.md", "heavy.txt"):
            tf.add(os.path.join(HERE, n), arcname=f"zoo_golden/{n}")
        for f in sorted(files):
            tf.add(f, arcname="zoo_golden/" + rel(f))
        for f in sorted(data):
            tf.add(f, arcname="zoo_golden/" + os.path.basename(f))
        # A model directory with no disciplines.vams of its own: verilog-a-sim drops the unresolved
        # include and uses its built-in `electrical`, while Spectre would find Cadence's full
        # standard header, whose Temperature/Position/Flux natures collide with these models' own
        # (VACOMP-1897). Ship the repository's header beside them so both simulators read the same
        # declarations. (The portable fix is in the models: rename their access functions.)
        ours = os.path.join(REPO, "models", "disciplines.vams")
        for dd in sorted({os.path.dirname(f) for f in needed}):
            if not os.path.isfile(os.path.join(dd, "disciplines.vams")):
                # only where a local nature reuses an access function the standard header defines
                std = {"Temp", "Pwr", "Pos", "F", "Vel", "Acc", "Imp", "Theta", "Tau", "Omega", "Alpha",
                       "MMF", "Phi", "Flux", "Conc"}
                local = set()
                for g in os.listdir(dd):
                    if g.endswith(".vams"):
                        local |= set(re.findall(r"access\s*=\s*(\w+)", open(os.path.join(dd, g), encoding="utf-8", errors="replace").read()))
                if local & std:
                    tf.add(ours, arcname="zoo_golden/" + rel(dd) + "/disciplines.vams")
    print(f"{len(manifest)} netlists ({sum(m[4] == 'heavy' for m in manifest)} heavy), {len(refused)} left out -> {tar}")


if __name__ == "__main__":
    main()
