import subprocess, re, sys
sys.path.insert(0,'.')
import bench_input_stage as z_bench
VA=['../../../target/release/va-cli','sim','scan_input_stage.net','--model','../../../models/sg13g2/psp103/psp103.va','--report','Vout,sl']
def run(text):
    open('scan_input_stage.net','w').write(text)
    out=subprocess.run(VA,capture_output=True,text=True).stdout
    return [[float(x) for x in re.findall(r'=([-\d.e+]+)',l)] for l in out.splitlines() if '=' in l and ('Vbs=' in l or 'Vout=' in l)]
def trim(vbpc,vbnc):
    rows=run(z_bench.deck('.dc Vbs 0.5 1.5 0.02', vt=1.6515, vbpc=vbpc, vbnc=vbnc))
    for a,b in zip(rows,rows[1:]):
        if a[1]>0>=b[1]: return a[0]+(b[0]-a[0])*a[1]/(a[1]-b[1])
def flat(vbpc,vbnc,vbs):
    res={}
    for iin in (0.0,10e-6,-10e-6):
        vt=1.6515+iin*50e3
        rows=run(z_bench.deck('.dc Vout 0.45 2.4 0.15', vt=vt, vbs=vbs, vbpc=vbpc, vbnc=vbnc))
        res[iin]=[r[1] for r in rows]
    z=res[0.0]; err0=max(abs(v) for v in z)
    gp=[(a-b)/10e-6 for a,b in zip(res[10e-6],z)]; gn=[(b-a)/10e-6 for a,b in zip(res[-10e-6],z)]
    return err0, min(gp+gn), max(gp+gn)
for vbpc in (1.5,1.7,1.9,2.1,2.3):
    for vbnc in (0.9,1.0,1.1,1.2):
        vbs=trim(vbpc,vbnc)
        if vbs is None: print(f'vbpc={vbpc} vbnc={vbnc}: no trim'); continue
        e,gmin,gmax=flat(vbpc,vbnc,vbs)
        print(f'vbpc={vbpc} vbnc={vbnc} vbs={vbs:.4f}: |I_out(I_in=0)| max {e*1e6:.3f} uA over V_m 0.45-2.4; gain {gmin:.4f}..{gmax:.4f} (target 1.4368)', flush=True)
