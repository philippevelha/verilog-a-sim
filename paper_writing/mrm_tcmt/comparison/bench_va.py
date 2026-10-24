import subprocess,statistics as st,os,re,numpy as np,hashlib
VA='/home/claude/verilog-a-sim/target/release/va-cli'
BASE=[VA,'sim','tran.net','--model','.','--tran','--report','kk,tre,tim']
def parse(txt):
    return np.array(re.findall(r"t=([-\d.e]+)s\s+V\(kk\)=([-\d.e]+) V\s+V\(tre\)=([-\d.e]+) V\s+V\(tim\)=([-\d.e]+) V",txt),float)
ref=np.loadtxt('ng_tran_tight.dat',skiprows=1); tg=np.arange(0.1e-9,10.15e-9,0.05e-12)
Pref=np.interp(tg,ref[:,0],ref[:,2]**2+ref[:,3]**2); Pm=Pref.mean()
base_hash=None
def run(label,extra=[],env={},n=3):
    W=[];U=[];M=[]
    for _ in range(n):
        e=dict(os.environ); e.update(env)
        p=subprocess.run(['/usr/bin/time','-f','%e %U %S %M','-o','/tmp/claude-0/t.txt']+BASE+extra,cwd='vasim',env=e,capture_output=True,text=True)
        w,u,s,m=open('/tmp/claude-0/t.txt').read().split()[-4:]; W.append(float(w));U.append(float(u));M.append(int(m))
    out=p.stdout; a=parse(out)
    if len(a)==0: print(label,"FAILED",(p.stdout+p.stderr)[-300:]); return
    h=hashlib.sha1("\n".join(l for l in out.splitlines() if l.startswith('  t=')).encode()).hexdigest()[:10]
    global base_hash
    if base_hash is None: base_hash=h
    P=np.interp(tg,a[:,0],a[:,2]**2+a[:,3]**2)
    err=np.sqrt(np.mean((P-Pref)**2))/Pm
    print(f"| {label:34s} | {len(a):6d} | {st.median(W):6.2f} | {st.mean(U):6.2f} | {max(M)/1024:5.1f} | {err:.2e} | {'same' if h==base_hash else 'differs'} |",flush=True)
print("| variant | points | wall s | user s | RSS MB | RMS dP/Pm vs ref | output vs default |")
run("default")
run("--threads 1",['--threads','1'])
run("VA_JIT=on",env={'VA_JIT':'on'})
run("--bypass",['--bypass'])
run("VA_JIT=on --bypass",['--bypass'],env={'VA_JIT':'on'})
run("VA_COLLAPSE=on",env={'VA_COLLAPSE':'on'})
run("--solver sparse",['--solver','sparse'])
run("VA_SCHED=off",env={'VA_SCHED':'off'})
run("--integration gear",['--integration','gear'])
run("--integration be",['--integration','be'])
