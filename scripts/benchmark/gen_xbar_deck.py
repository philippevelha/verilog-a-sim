"""Write a small MNIST crossbar deck for bench_xbar_jit_linux.sh: hidden layer 2 only, its first N
neurons as the behavioural blocks, every 2T2R synapse with its two SG13G2 PSP103 access
transistors (circuits/snn/transistor/gen_mnist_tx.py), for a given number of 250 ns cycles.

    python3 scripts/benchmark/gen_xbar_deck.py <out-dir> [--neurons 8] [--cycles 3] [--digit 0]

Writes <out-dir>/d/xbar.net, <out-dir>/d/tech.inc and <out-dir>/dff.inc (the deck includes
../dff.inc).

**No PDK needed.** gen_mnist_tx.py asks the technology layer for a card per geometry, and the
layer reads the IHP PDK (external/, not committed). This deck's only transistor is the
W/L = 10/0.45 um access NMOS, whose card is committed in circuits/snn/transistor/tech.inc, so the
cards are served from that file instead. The generator also asks for the transistor neuron's
geometries, which this deck does not instantiate. Those get a placeholder that is never written
into the deck. The script checks that every card the deck names is a committed one, and copies the
committed tech.inc next to it.
"""
import argparse
import os
import re
import shutil
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
TX = os.path.join(ROOT, "circuits", "snn", "transistor")
TECH_INC = os.path.join(TX, "tech.inc")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--neurons", type=int, default=8)
    ap.add_argument("--cycles", type=float, default=3)
    ap.add_argument("--digit", type=int, default=0)
    a = ap.parse_args()

    # The behavioural network deck the crossbar deck is generated from.
    mnist_deck = os.path.join(ROOT, "circuits", "snn", "mnist", "decks", f"digit{a.digit}.net")
    if not os.path.exists(mnist_deck):
        subprocess.run([sys.executable, os.path.join(ROOT, "circuits", "snn", "mnist",
                                                     "gen_mnist_deck.py"), str(a.digit)],
                       check=True, cwd=ROOT)

    text = open(TECH_INC).read()
    cards = {m.group(1): m.group(0).rstrip("\n")
             for m in re.finditer(r"^\.model (\S+) .*?(?=^\.model |\Z)", text, re.M | re.S)}

    sys.path.insert(0, os.path.join(ROOT, "models", "sg13g2"))
    sys.path.insert(0, TX)
    import sg13g2_cards  # noqa: E402

    real_card = sg13g2_cards.card

    def card(kind, w, l, ng=1, corner="mos_tt", **kw):
        name = f"hv{kind[0]}_w{w * 1e9:.0f}n_l{l * 1e9:.0f}n_ng{ng}"
        try:
            return real_card(kind, w, l, ng, corner=corner, **kw)  # the PDK, if present
        except (OSError, KeyError):
            pass
        # No PDK: the committed card's text; the instance parameters are geometry only, so the
        # PDK's own function computes them without reading the PDK.
        saved = (sg13g2_cards._psp_params, sg13g2_cards._corner, sg13g2_cards._raw_card)
        sg13g2_cards._psp_params = lambda *x, **y: set()
        sg13g2_cards._corner = lambda *x, **y: {}
        sg13g2_cards._raw_card = lambda *x, **y: []
        try:
            _, _, inst = real_card(kind, w, l, ng, corner=corner, **kw)
        finally:
            sg13g2_cards._psp_params, sg13g2_cards._corner, sg13g2_cards._raw_card = saved
        return name, cards.get(name, f"* {name}: placeholder, not used by this deck"), inst

    sg13g2_cards.card = card

    d = os.path.join(os.path.abspath(a.out), "d")
    os.makedirs(d, exist_ok=True)
    deck = os.path.join(d, "xbar.net")
    sys.argv = ["gen_mnist_tx.py", "--digit", str(a.digit), "--cycles", str(a.cycles),
                "--layers", "h2", "--per-layer", str(a.neurons), "--neurons", "behavioural",
                "--out", deck]
    cwd = os.getcwd()
    os.chdir(TX)
    try:
        import runpy
        runpy.run_path(os.path.join(TX, "gen_mnist_tx.py"), run_name="__main__")
    finally:
        os.chdir(cwd)

    used = set(re.findall(r"\bhv[np]_w\d+n_l\d+n_ng\d+\b", open(deck).read()))
    missing = sorted(used - set(cards))
    if missing:
        raise SystemExit(f"the deck names cards not in {TECH_INC}: {missing}")
    shutil.copy(TECH_INC, os.path.join(d, "tech.inc"))
    shutil.copy(os.path.join(ROOT, "circuits", "snn", "dff.inc"),
                os.path.join(os.path.abspath(a.out), "dff.inc"))
    print(f"{deck}: cards used {sorted(used)}")


if __name__ == "__main__":
    main()
