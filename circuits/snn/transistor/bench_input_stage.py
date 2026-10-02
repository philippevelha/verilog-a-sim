import sys; sys.path.insert(0,'.')
import tech, input_stage as ist
def deck(sweep, vt=1.65, vbs=0.9, vout=1.5, k=0.696, vbpc=2.1, vbnc=1.05):
    t=tech.get()
    L=["* input stage DC bench", ".include bench_tech.inc",
       "Vdd vdd gnd DC 3.3", "Vref vref gnd DC 1.65", "Vbn vbn gnd DC 0.8", "Vbp vbp gnd DC 1.8",
       f"Vbs vbs gnd DC {vbs}", f"Vbpc vbpc gnd DC {vbpc}", f"Vbnc vbnc gnd DC {vbnc}", "Vclk clk gnd DC 3.3", "Vclkb clkb gnd DC 0",
       f"Vt vt gnd DC {vt}", "Rin vt sl 50k", f"Vout out gnd DC {vout}", "Vdump dump gnd DC 1.65"]
    L+=ist.lines(t, k, dump="dump")
    L+=[sweep, ".end"]
    t.write_inc('bench_tech.inc')
    return "\n".join(L)+"\n"
if __name__=="__main__":
    kind=sys.argv[1]
    if kind=="trim": open('bench_input_stage.net','w').write(deck(".dc Vbs 0.6 1.4 0.05"))
    elif kind=="gain": open('bench_input_stage.net','w').write(deck(".dc Vt 1.15 2.15 0.05", vbs=float(sys.argv[2])))
    elif kind=="vout": open('bench_input_stage.net','w').write(deck(".dc Vout 0.3 2.7 0.3", vt=float(sys.argv[3]), vbs=float(sys.argv[2])))
