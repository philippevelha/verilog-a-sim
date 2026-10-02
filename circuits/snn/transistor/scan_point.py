"""One bias point of the input stage: trim vbs, then I_out(I_in=0) and gain over V_m, for a device
ratio k_dev (the output mirror is W/k_dev; k_dev != K trims the gain).
    python scan_point.py vbpc vbnc k_dev [k_dev ...]"""
import re, subprocess, sys
sys.path.insert(0, '.')
import bench_input_stage as b
VA = ['../../../target/release/va-cli', 'sim', 'scan_input_stage.net', '--model',
      '../../../models/sg13g2/psp103/psp103.va', '--report', 'Vout,sl']
def run(text):
    open('scan_input_stage.net', 'w').write(text)
    out = subprocess.run(VA, capture_output=True, text=True).stdout
    return [[float(x) for x in re.findall(r'=([-\d.e+]+)', l)] for l in out.splitlines()
            if '=' in l and ('Vbs=' in l or 'Vout=' in l)]
def trim(vbpc, vbnc, k):
    rows = run(b.deck('.dc Vbs 0.5 1.5 0.02', vt=1.65, vbpc=vbpc, vbnc=vbnc, k=k))
    for a, c in zip(rows, rows[1:]):
        if a[1] > 0 >= c[1]:
            return a[0] + (c[0] - a[0]) * a[1] / (a[1] - c[1])
def flat(vbpc, vbnc, vbs, k):
    res = {}
    for iin in (0.0, 10e-6, -10e-6):
        rows = run(b.deck('.dc Vout 0.45 2.4 0.15', vt=1.65 + iin * 50e3, vbs=vbs, vbpc=vbpc,
                          vbnc=vbnc, k=k))
        res[iin] = [r[1] for r in rows]
    z = res[0.0]
    gp = [(a - c) / 10e-6 for a, c in zip(res[10e-6], z)]
    gn = [(c - a) / 10e-6 for a, c in zip(res[-10e-6], z)]
    return z, gp, gn
if __name__ == '__main__':
    vbpc, vbnc = float(sys.argv[1]), float(sys.argv[2])
    for k in map(float, sys.argv[3:]):
        vbs = trim(vbpc, vbnc, k)
        z, gp, gn = flat(vbpc, vbnc, vbs, k)
        print(f'k_dev={k} vbs={vbs:.4f} |I0|max={max(map(abs, z))*1e6:.3f} uA', flush=True)
        print(' I0 uA ' + ' '.join(f'{v*1e6:.3f}' for v in z))
        print(' g+    ' + ' '.join(f'{g:.4f}' for g in gp))
        print(' g-    ' + ' '.join(f'{g:.4f}' for g in gn), flush=True)
