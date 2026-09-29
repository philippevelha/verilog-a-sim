"""Summarize the MNIST circuit runs: a table and a figure from circuits/snn/mnist/results.jsonl.

Usage (from the repository root):
  python circuits/snn/mnist/summarize.py

Prints a markdown table (per image: va-cli wall time, timepoints, spike slots differing from the
quantized model per layer, output spike counts, class from circuit / model / label) with totals,
and writes docs/examples/snn_mnist/summary.png.
"""
import json
import os

HERE = os.path.dirname(os.path.abspath(__file__))


def main():
    rs = sorted((json.loads(l) for l in open(os.path.join(HERE, 'results.jsonl')) if l.strip()),
                key=lambda r: r['digit'])
    print('| label | va-cli wall | timepoints | slots differing h2 / h3 / out | '
          'circuit class | model class |')
    print('|---|---|---|---|---|---|')
    for r in rs:
        dh2 = r['h2']['total'] - r['h2']['agree']
        dh3 = r['h3']['total'] - r['h3']['agree']
        do = r['o']['total'] - r['o']['agree']
        cls = str(r['circuit_class']) + (f" (tie {'/'.join(map(str, r['ties']))})"
                                         if len(r['ties']) > 1 else '')
        print(f"| {r['digit']} | {r['wall_s']:.1f} s | {r['points']} | {dh2} / {dh3} / {do} | "
              f"{cls} | {r['model_class']} |")
    walls = [r['wall_s'] for r in rs]
    slots = sum(r[k]['total'] for r in rs for k in ('h2', 'h3', 'o'))
    agree = sum(r[k]['agree'] for r in rs for k in ('h2', 'h3', 'o'))
    same = sum(r['circuit_class'] == r['model_class'] for r in rs)
    right = sum(r['circuit_class'] == r['digit'] for r in rs)
    print(f'\nimages: {len(rs)}; va-cli wall total {sum(walls):.1f} s ({sum(walls) / 3600:.2f} h), '
          f'mean {sum(walls) / len(rs):.1f} s, min {min(walls):.1f} s, max {max(walls):.1f} s')
    print(f'timepoints: mean {sum(r["points"] for r in rs) / len(rs):.0f}')
    print(f'spike slots agreeing with the model: {agree} / {slots} ({100 * agree / slots:.3f} %)')
    print(f'circuit class = model class: {same}/{len(rs)}; = label: {right}/{len(rs)}')

    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt
    d = [r['digit'] for r in rs]
    fig, ax = plt.subplots(2, 1, figsize=(8, 6))
    ax[0].bar(d, [r['wall_s'] / 60 for r in rs])
    for r in rs:
        ax[0].text(r['digit'], r['wall_s'] / 60 + 0.8, f"{r['wall_s'] / 60:.1f}", ha='center',
                   fontsize=7)
    ax[0].set_ylabel('va-cli wall time (min)')
    ax[0].set_xticks(d)
    ax[0].set_ylim(0, 75)
    ax[0].set_title('MNIST samples through the SNN circuit (hidden 2, hidden 3, output), '
                    'i7-1185G7, 8 threads', fontsize=9)
    width = 0.25
    for off, key, lab in ((-width, 'h2', 'hidden 2'), (0, 'h3', 'hidden 3'), (width, 'o', 'output')):
        ax[1].bar([x + off for x in d], [r[key]['total'] - r[key]['agree'] for r in rs], width,
                  label=lab)
    ax[1].set_ylabel('spike slots differing\nfrom the model')
    ax[1].set_xlabel('image (label)')
    ax[1].set_xticks(d)
    ax[1].legend(fontsize=7)
    fig.tight_layout()
    out = os.path.join(HERE, '..', '..', '..', 'docs', 'examples', 'snn_mnist', 'summary.png')
    fig.savefig(out, dpi=140)
    print('figure:', os.path.relpath(out, os.getcwd()))


if __name__ == '__main__':
    main()
