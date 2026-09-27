"""Run `cargo xtask deck-compare <a> <b>` and turn its report into GitHub annotations.

Road to 2.0 item 3: the comparison's details must be readable without the job log, which needs
repository admin rights to download. Annotations are public on a public repository, so each
differing deck becomes one annotation (its first differing line, windowed by deck-compare), a
deck that failed on both sides a warning, and the summary line a notice. The exit code is
deck-compare's.

Usage: python3 compare_annotate.py <title> <dir-a> <dir-b>
"""
import subprocess
import sys

title, a, b = sys.argv[1], sys.argv[2], sys.argv[3]
run = subprocess.run(['cargo', 'xtask', 'deck-compare', a, b], capture_output=True, text=True)
report = run.stderr
print(report)

blocks, cur = [], None
for line in report.splitlines():
    if not line.startswith('[xtask]'):
        continue
    text = line[len('[xtask]'):].strip()
    if text.startswith(('DIFF', 'ONLY IN', 'FAILED ON BOTH')):
        cur = [text]
        blocks.append(cur)
    elif text.startswith('deck-compare:'):
        print(f'::notice title={title}::{text}')
        cur = None
    elif cur is not None:
        cur.append(text)


def escape(s):
    return s.replace('%', '%25').replace('\r', '').replace('\n', '%0A')


# GitHub keeps at most 10 error and 10 warning annotations per step; the full report is in the
# job log above.
for kind, level in (('FAILED ON BOTH', 'warning'), ('', 'error')):
    chosen = [blk for blk in blocks if blk[0].startswith(kind)] if kind else \
        [blk for blk in blocks if not blk[0].startswith('FAILED ON BOTH')]
    for blk in chosen[:10]:
        print(f'::{level} title={title}::' + escape('\n'.join(x[:300] for x in blk)))
sys.exit(run.returncode)
