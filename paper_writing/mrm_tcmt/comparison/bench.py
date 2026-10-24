import subprocess,statistics as st,os,sys
VA='/home/claude/verilog-a-sim/target/release/va-cli'; NG='/opt/ngspice-new/bin/ngspice'; OV='/home/claude/arpadbuermen/openvaf/target/release/openvaf-r'
def bench(label,cmd,cwd='.',env=None,n=5):
    W=[];U=[];S=[];M=[]
    for _ in range(n):
        e=dict(os.environ); e.update(env or {})
        subprocess.run(['/usr/bin/time','-f','%e %U %S %M','-o','/tmp/claude-0/t.txt']+cmd,cwd=cwd,env=e,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        w,u,s,m=open('/tmp/claude-0/t.txt').read().split()[-4:]; W.append(float(w));U.append(float(u));S.append(float(s));M.append(int(m))
    print(f"{label:36s} wall med {st.median(W):6.3f} s [{min(W):.3f}-{max(W):.3f}]  user {st.mean(U):6.3f}  sys {st.mean(S):6.3f}  peakRSS {max(M)/1024:6.1f} MB",flush=True)
bench("openvaf compile mrm_tcmt.va",[OV,'../mrm_tcmt.va','-o','/tmp/claude-0/x.osdi'])
bench("ngspice DC (OMP default, 2 thr)",[NG,'-b','ng_dc_def.cir'])
bench("ngspice DC (OMP_NUM_THREADS=1)",[NG,'-b','ng_dc_def.cir'],env={'OMP_NUM_THREADS':'1'})
bench("va-sim DC (incl. VA compile)",[VA,'sim','dc.net','--model','.','--report','tre,tim'],cwd='vasim')
bench("ngspice tran (OMP default, 2 thr)",[NG,'-b','ng_tran_def.cir'])
bench("ngspice tran (OMP_NUM_THREADS=1)",[NG,'-b','ng_tran_def.cir'],env={'OMP_NUM_THREADS':'1'})
bench("va-sim tran (incl. VA compile)",[VA,'sim','tran.net','--model','.','--tran','--report','kk,tre,tim'],cwd='vasim')
bench("ngspice tran TIGHT (1 thr)",[NG,'-b','ng_tran_tight.cir'],env={'OMP_NUM_THREADS':'1'},n=2)
bench("va-sim tran TIGHT",[VA,'sim','tran.net','--model','.','--tran','--reltol','1e-7','--abstol','1e-10','--report','kk,tre,tim'],cwd='vasim',n=2)
