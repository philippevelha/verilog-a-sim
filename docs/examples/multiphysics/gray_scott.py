"""Run a Gray-Scott deck and draw the snapshots the model wrote.

models/multiphysics/grayscott/gray_scott.va writes target/gray_scott.txt: for every snapshot a
line `# <instance> t=<time>`, then N rows of u and N rows of v. This script runs the deck through
va-cli, reads that file, draws v at every snapshot side by side, and prints, per snapshot, the
mean and maximum of v and the number of spots (connected regions with v above `threshold`, on
the torus the model's grid is: regions touching across an edge count once).

Usage (from the repository root, after `cargo build --release`):
    python docs/examples/multiphysics/gray_scott.py <deck-name> [path/to/va-cli]
with <deck-name> one of the circuits/multiphysics/gray_scott_*.net stems (e.g. gray_scott_spots);
writes docs/examples/multiphysics/<deck-name>.svg.
"""
import subprocess
import sys

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

DECK = sys.argv[1] if len(sys.argv) > 1 else "gray_scott_spots"
VA = sys.argv[2] if len(sys.argv) > 2 else "target/release/va-cli"
THRESHOLD = 0.3

out = subprocess.run(
    [VA, "sim", f"circuits/multiphysics/{DECK}.net", "--model", "models/multiphysics/grayscott",
     "--tran", "--report", "X1.v[0][0]"],
    capture_output=True, text=True, check=True,
)

snaps = []  # (t, u, v)
rows = []
t = None
with open("target/gray_scott.txt", encoding="utf-8") as fh:
    for line in fh:
        if line.startswith("#"):
            if t is not None:
                snaps.append((t, rows))
            t = float(line.split("t=")[1])
            rows = []
        elif line.strip():
            rows.append([float(x) for x in line.split()])
    if t is not None:
        snaps.append((t, rows))
n = len(snaps[0][1]) // 2
snaps = [(t, r[:n], r[n:]) for t, r in snaps]


def count_spots(v):
    """Connected regions (4-neighbour, periodic) with v > THRESHOLD."""
    seen = [[False] * n for _ in range(n)]
    count = 0
    for i in range(n):
        for j in range(n):
            if v[i][j] <= THRESHOLD or seen[i][j]:
                continue
            count += 1
            stack = [(i, j)]
            seen[i][j] = True
            while stack:
                a, b = stack.pop()
                for da, db in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                    p, q = (a + da) % n, (b + db) % n
                    if v[p][q] > THRESHOLD and not seen[p][q]:
                        seen[p][q] = True
                        stack.append((p, q))
    return count


fig, axes = plt.subplots(1, len(snaps), figsize=(2.2 * len(snaps), 2.6))
for ax, (t, _u, v) in zip(axes, snaps):
    ax.imshow(v, cmap="viridis", vmin=0.0, vmax=0.45, interpolation="nearest")
    ax.set_title(f"t = {t:.0f}", fontsize=9)
    ax.set_xticks([])
    ax.set_yticks([])
    flat = [x for r in v for x in r]
    print(f"t={t:8.0f}: mean v {sum(flat) / len(flat):.5f}, max v {max(flat):.5f}, "
          f"{count_spots(v)} region(s) with v > {THRESHOLD}")
fig.suptitle(f"Gray-Scott, {DECK}: v on a {n} x {n} torus", fontsize=10)
fig.tight_layout()
fig.savefig(f"docs/examples/multiphysics/{DECK}.svg")
print(f"wrote docs/examples/multiphysics/{DECK}.svg")
