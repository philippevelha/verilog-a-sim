//! **Statically scheduled evaluation** of a compiled model's per-load statements — prototype,
//! branch `static-tape`, 2026-10-01 (`docs/proposals/gradient-storage.md` §0.1).
//!
//! # Why
//!
//! A PSP103 `load()` with IHP's SG13G2 card executes ~6 100 tape instructions over ~1 225
//! assignments, and 57 % of them only *push* a variable (2 551) or a constant (939) onto the
//! value stack, copying a [`Dual`] each time. The cost is per node (~40 ns), not per partial:
//! changing how a gradient is stored did not help (sparse gradients, same document).
//!
//! # What
//!
//! Which local unknowns an expression can depend on is fixed by the model's **structure**, not by
//! the bias: a probe's two slots, the union of an operator's operands, and for a variable the
//! union over every assignment to it (a fixpoint). So this module computes that **support** once
//! per model and gives every variable, every intermediate result and every probe its own
//! **register**: one value and a fixed run of partials, one per slot of its support. A statement's
//! expression becomes a short list of instructions that read their operands in place and write
//! their result in place:
//!
//! - a variable read or a constant is no instruction at all — it *is* a register;
//! - an assignment's last operation writes straight into the variable's register;
//! - no stack, no allocation, no `Dual` is built until a contribution needs one to stamp.
//!
//! The arithmetic is each rule's own (`ad::Dual`'s), applied to the same operand partials, so a
//! partial this produces is the partial the tree walk produces, bit for bit. `xtask sched-check`
//! compares every stamp of the two paths. Where a slot is zero in both operands the result is
//! `0.0`, as `Grad::Zero` has always given; the dense form computes `f(0.0, 0.0)` there, which is
//! `±0.0` for finite operand values and `NaN` only where a rule multiplies a zero partial by an
//! infinite operand value.
//!
//! # Less to run (2.17.0)
//!
//! Four compile-time reductions, each leaving every stamp bit-identical (`xtask sched-check`,
//! and every deck's output under `xtask deck-diff`):
//!
//! - **Constant folding.** An operation whose operands are all fixed for the program's life — a
//!   constant, a parameter, a setup value no per-load statement reassigns, or an operation
//!   folded already — is computed once, by the very expression the instruction would evaluate
//!   ([`bin_value`], [`chain_value`], [`flat1_value`], [`flat2_value`]), and becomes a constant
//!   register. A setup value that is never reassigned is no longer reset on every load either.
//! - **Dead assignments.** An assignment whose variable reaches no contribution, condition,
//!   bound or task argument, directly or through other assignments ([`live_vars`],
//!   flow-insensitive), compiles to nothing — PSP103's operating-point outputs, for one. Kept
//!   if its code calls the tree walker, which can fail: a failure must still stop the walk.
//! - **Whole `if`s.** An `if` whose arms hold only assignments (and such `if`s) is laid out as
//!   condition, [`Instr::JumpIfZero`], arms ([`Layout::list`]), instead of returning to the
//!   statement walk to evaluate the condition and descend into an arm.
//! - **Stretches.** Consecutive statements of a list whose code follows on — assignments and
//!   whole `if`s — run as one [`exec`] ([`Run::stretch`]), not one call each.
//!
//! On PSP103 with the SG13G2 card, one `load()` goes from ~78 µs to ~47 µs (`xtask
//! bench-instance circuits/snn/transistor/bench_access_bias.net`, Xeon 2.8 GHz).
//!
//! # Scope (what makes a model fall back to the tree walk)
//!
//! A model runs here only if its per-load statements can be compiled whole; otherwise
//! [`Program::compile`] returns `None` and nothing changes. Refused: persistent variables,
//! `idt`/`laplace_*`/`zi_*` accumulators, a `ddt` inside an expression, a user function or other
//! non-maths call whose arguments read a variable, and any variable that may be read before it is
//! assigned in the same evaluation (a definite-assignment check — the tree walk reports such a
//! read as an error, and a register would silently hold the previous evaluation's value). Calls
//! with no variable in their arguments run through the tree walker (`ad::eval`) and are scattered
//! into a register (`Instr::Tree`).

mod jit;

use crate::ad::{self, Ctx, Dual};
use crate::lower::{Lowered, LoweredStmt};
use crate::CodegenError;
use std::cell::RefCell;
use std::collections::HashMap;
use va_ir::{AccessKind, BinOp, Builtin, Expr, ExprId, Module, UnOp, VarId};

const NONE: u32 = u32::MAX;
/// The index of the always-zero partial in [`Regs::parts`]: what an operand that lacks a
/// slot contributes there.
const ZERO: u32 = 0;

/// A support: the local slots an expression's partials can be non-zero in, as a bit set.
#[derive(Clone, PartialEq, Eq, Debug)]
struct Support(Vec<u64>);

impl Support {
    fn empty(n: usize) -> Self {
        Support(vec![0; n.div_ceil(64).max(1)])
    }
    fn full(n: usize) -> Self {
        let mut s = Support::empty(n);
        for k in 0..n {
            s.insert(k);
        }
        s
    }
    fn insert(&mut self, k: usize) {
        self.0[k / 64] |= 1 << (k % 64);
    }
    fn union(&mut self, o: &Support) -> bool {
        let mut changed = false;
        for (a, b) in self.0.iter_mut().zip(&o.0) {
            let n = *a | *b;
            changed |= n != *a;
            *a = n;
        }
        changed
    }
    fn slots(&self) -> Vec<u32> {
        let mut v = Vec::new();
        for (wi, w) in self.0.iter().enumerate() {
            let mut w = *w;
            while w != 0 {
                let b = w.trailing_zeros();
                v.push(wi as u32 * 64 + b);
                w &= w - 1;
            }
        }
        v
    }
}

/// A derivative rule over two operands (`ad::Dual`'s own rules).
#[derive(Clone, Copy, Debug)]
enum Bin {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Hypot,
    Atan2,
}

/// A chain-rule function of one operand (`ad::Dual::chain` and `scale`).
#[derive(Clone, Copy, Debug)]
enum Chain {
    Neg,
    Exp,
    Ln,
    Log10,
    Sqrt,
    Abs,
    Sin,
    Cos,
    Tan,
    Sinh,
    Cosh,
    Tanh,
    Asin,
    Acos,
    Atan,
    Asinh,
    Acosh,
    Atanh,
}

#[derive(Clone, Copy, Debug)]
enum Instr {
    /// `V(p) - V(n)` (or a flow slot's value): value only — a probe register's partials are
    /// constants set when the register file is built.
    Probe {
        dst: u32,
        p: u32,
        n: u32,
    },
    Bin {
        op: Bin,
        dst: u32,
        a: u32,
        b: u32,
        map: u32,
    },
    Chain {
        op: Chain,
        dst: u32,
        a: u32,
        map: u32,
    },
    /// A value-only binary operator (comparison, logic, bitwise, `%`): `ad::apply_binary` on
    /// constants, so the arithmetic is literally the same.
    Flat2 {
        op: BinOp,
        dst: u32,
        a: u32,
        b: u32,
    },
    /// A value-only unary operator or rounding call.
    Flat1 {
        op: Flat1,
        dst: u32,
        a: u32,
    },
    /// `min`/`max`: the selected operand, copied.
    Pick {
        max: bool,
        dst: u32,
        a: u32,
        b: u32,
        map_a: u32,
        map_b: u32,
    },
    /// Copy `src` into `dst`, zero-filling the slots `src` lacks.
    Copy {
        dst: u32,
        src: u32,
        map: u32,
    },
    /// `ddx(e, V(p, ·))`: the partial of `src` at one slot (`idx`, absolute, or `ZERO`).
    Ddx {
        dst: u32,
        idx: u32,
    },
    /// A variable-free subexpression evaluated by the tree walker and scattered into `dst`.
    Tree {
        dst: u32,
        expr: ExprId,
    },
    JumpIfZero {
        cond: u32,
        to: u32,
    },
    Jump {
        to: u32,
    },
}

#[derive(Clone, Copy, Debug)]
enum Flat1 {
    Unary(UnOp),
    Call(Builtin),
}

/// One compiled root expression: run `code[start..end]`, then read register `result`.
#[derive(Clone, Copy, Debug)]
struct Block {
    start: u32,
    end: u32,
    result: u32,
    /// Where the code of the statement this root heads ends: `end` for an assignment; past
    /// both arms for an `if` laid out whole ([`Self::whole`]).
    stmt_end: u32,
    /// This is the condition of an `if` laid out whole: running `start..stmt_end` runs the
    /// statement, arms included.
    whole: bool,
    /// For the block heading a statement (an assignment's, or a whole `if`'s condition): how
    /// many consecutive such statements of its list, starting with this one, have contiguous
    /// code (at least 1), and where the last one's code ends. [`Run::stretch`] executes them
    /// as one piece.
    run_len: u32,
    run_end: u32,
}

/// The statically scheduled form of a model's per-load statements. Built once per model
/// (`crate::SharedModel`), shared by every instance; each instance owns a [`Regs`].
#[derive(Debug)]
pub(crate) struct Program {
    n_slots: usize,
    /// Per register: start of its partials in [`Regs::parts`], and their count.
    off: Vec<u32>,
    len: Vec<u32>,
    /// Per register, its support's slots (concatenated, `off`-indexed like the partials).
    slots: Vec<u32>,
    /// Initial values: constants, parameters, setup-variable values; zero elsewhere.
    init_vals: Vec<f64>,
    /// Initial partials: probes' constant `±1`, zero elsewhere.
    init_parts: Vec<f64>,
    /// Block of each root expression (indexed by `ExprId`), `NONE` if not a compiled root.
    root_block: Vec<u32>,
    blocks: Vec<Block>,
    code: Vec<Instr>,
    /// Per instruction with a map: for each of `dst`'s partials, the absolute index of the
    /// matching partial in each operand, or `ZERO` (the zero cell) if it has none.
    maps: Vec<(u32, u32)>,
    /// Per-load resets: a variable that the setup assigns and the per-load statements may
    /// reassign starts every evaluation from its setup value.
    resets: Vec<(u32, f64)>,
    /// The program as machine code (`VA_JIT=on`, [`jit`]), run in place of the interpreter.
    jit: Option<jit::Jit>,
}

/// An instance's register file. Kept between evaluations (a constant, a parameter and a setup
/// value are written once), behind a `Mutex` in the instance.
#[derive(Debug)]
pub(crate) struct Regs {
    vals: Vec<f64>,
    parts: Vec<f64>,
}

/// What an evaluation's context carries while this path is active.
pub(crate) struct Run<'a> {
    pub(crate) prog: &'a Program,
    pub(crate) regs: RefCell<&'a mut Regs>,
    /// The terminal voltages the machine code's probes read ([`jit`]), gathered once per
    /// evaluation; empty when the program is interpreted.
    tv: Vec<f64>,
}

struct Compiler<'m> {
    module: &'m Module,
    params: &'m [f64],
    n: usize,
    var_support: Vec<Support>,
    // registers
    off: Vec<u32>,
    len: Vec<u32>,
    slots: Vec<u32>,
    init_vals: Vec<f64>,
    init_parts: Vec<f64>,
    var_reg: Vec<u32>,
    /// Per register: its value is fixed for the program's life (a constant, a parameter, a
    /// setup value no per-load statement reassigns, or an operation on only such registers,
    /// folded when compiled).
    konst: Vec<bool>,
    consts: HashMap<u64, u32>,
    code: Vec<Instr>,
    maps: Vec<(u32, u32)>,
    support_memo: HashMap<u32, Support>,
}

/// Why a model is not compiled here — never shown to a user, only to `sched-check`.
type Refuse = &'static str;

impl Program {
    /// Compile `lowered`'s per-load statements, or `None` (with the reason) if the model uses
    /// anything this path does not cover. `setup` is the setup prefix's variable bindings.
    pub(crate) fn compile(
        module: &Module,
        lowered: &Lowered,
        params: &[f64],
        setup: &[Option<Dual>],
        n_slots: usize,
    ) -> Result<Program, Refuse> {
        if !lowered.persistent_vars.is_empty() {
            return Err("persistent variables");
        }
        if !lowered.idt_accumulators.is_empty()
            || !lowered.laplace_terms.is_empty()
            || !lowered.zi_terms.is_empty()
        {
            return Err("idt/laplace/zi state");
        }
        if setup.iter().flatten().any(|d| d.depends_on_unknowns()) {
            return Err("a setup value depends on the unknowns");
        }
        let stmts = &lowered.stmts[lowered.static_prefix..];
        let nvars = module.vars.len();

        // Definite assignment: a read must follow an assignment in the same evaluation, or read
        // a setup value.
        let mut da: Vec<bool> = (0..nvars)
            .map(|i| setup.get(i).is_some_and(|b| b.is_some()))
            .collect();
        check_da(module, stmts, &mut da)?;

        let mut c = Compiler {
            module,
            params,
            n: n_slots,
            var_support: vec![Support::empty(n_slots); nvars],
            off: Vec::new(),
            len: Vec::new(),
            // Partial 0 is the zero cell (`ZERO`): no register's run includes it, so nothing
            // writes it, and a missing operand partial reads it.
            slots: vec![NONE],
            init_vals: Vec::new(),
            init_parts: vec![0.0],
            var_reg: vec![NONE; nvars],
            konst: Vec::new(),
            consts: HashMap::new(),
            code: Vec::new(),
            maps: Vec::new(),
            support_memo: HashMap::new(),
        };
        // Variable supports: a fixpoint over every assignment.
        loop {
            c.support_memo.clear();
            let mut changed = false;
            let mut assigns = Vec::new();
            collect_assigns(stmts, &mut assigns);
            for (lhs, rhs) in assigns {
                let s = c.support(rhs)?;
                changed |= c.var_support[lhs.0 as usize].union(&s);
            }
            if !changed {
                break;
            }
        }
        c.support_memo.clear();

        // Registers for every variable the per-load statements read or write.
        let mut used = vec![false; nvars];
        mark_vars(module, stmts, &mut used);
        let mut written = vec![false; nvars];
        mark_written(stmts, &mut written);
        let mut resets = Vec::new();
        for (i, u) in used.iter().enumerate() {
            if !*u {
                continue;
            }
            let s = c.var_support[i].clone();
            let r = c.new_reg(&s);
            if let Some(Some(d)) = setup.get(i) {
                c.init_vals[r as usize] = d.value;
                if written[i] {
                    resets.push((r, d.value));
                } else {
                    // Never reassigned per load: its register holds the setup value for good.
                    c.konst[r as usize] = true;
                }
            }
            c.var_reg[i] = r;
        }

        // Compile every root expression, statement by statement in the walk's order. An `if`
        // whose arms hold only assignments (and such `if`s) is laid out whole — condition,
        // jump, arms — so a stretch of statements runs as one piece of code.
        let mut roots = Vec::new();
        collect_roots(stmts, &mut roots);
        let mut assigns = Vec::new();
        collect_assigns(stmts, &mut assigns);
        let assign_rhs: HashMap<u32, VarId> = assigns.iter().map(|(l, r)| (r.0, *l)).collect();
        let live = live_vars(module, &roots, &assigns, &assign_rhs);
        let mut uses: HashMap<u32, u32> = HashMap::new();
        for r in &roots {
            *uses.entry(r.0).or_default() += 1;
        }
        let mut lay = Layout {
            c,
            assign_rhs: &assign_rhs,
            live: &live,
            uses: &uses,
            root_block: vec![NONE; module.exprs.len()],
            blocks: Vec::new(),
        };
        lay.list(stmts)?;
        let Layout {
            mut c,
            root_block,
            mut blocks,
            ..
        } = lay;
        // An assignment whose rhs is shared with another assignment (the same `ExprId`) would
        // write only the first variable: refuse rather than guess.
        let mut seen = HashMap::new();
        for (lhs, rhs) in &assigns {
            if let Some(prev) = seen.insert(rhs.0, *lhs) {
                if prev != *lhs {
                    return Err("one expression assigned to two variables");
                }
            }
        }
        mark_runs(stmts, &root_block, &mut blocks);
        // A missing operand partial reads the zero cell rather than testing for `NONE`.
        for m in &mut c.maps {
            if m.0 == NONE {
                m.0 = ZERO;
            }
            if m.1 == NONE {
                m.1 = ZERO;
            }
        }
        for i in &mut c.code {
            if let Instr::Ddx { idx, .. } = i {
                if *idx == NONE {
                    *idx = ZERO;
                }
            }
        }
        let mut prog = Program {
            n_slots,
            off: c.off,
            len: c.len,
            slots: c.slots,
            init_vals: c.init_vals,
            init_parts: c.init_parts,
            root_block,
            blocks,
            code: c.code,
            maps: c.maps,
            resets,
            jit: None,
        };
        if crate::jit_enabled() {
            prog.jit = jit::Jit::compile(&prog);
        }
        Ok(prog)
    }

    /// How many local slots a gradient spans.
    pub(crate) fn n_slots(&self) -> usize {
        self.n_slots
    }

    /// Whether this program runs as machine code ([`jit`]).
    pub(crate) fn has_jit(&self) -> bool {
        self.jit.is_some()
    }

    /// A fresh register file for one instance.
    pub(crate) fn regs(&self) -> Regs {
        Regs {
            vals: self.init_vals.clone(),
            parts: self.init_parts.clone(),
        }
    }

    /// Reset what must start each evaluation from its setup value.
    pub(crate) fn begin(&self, regs: &mut Regs) {
        for &(r, v) in &self.resets {
            regs.vals[r as usize] = v;
            let (o, l) = (self.off[r as usize] as usize, self.len[r as usize] as usize);
            regs.parts[o..o + l].fill(0.0);
        }
    }

    /// The program's instructions by kind, in order (`"probe"`, `"bin"`, `"tree"`, ...).
    #[cfg(test)]
    pub(crate) fn instr_kinds(&self) -> Vec<&'static str> {
        self.code
            .iter()
            .map(|i| match i {
                Instr::Probe { .. } => "probe",
                Instr::Bin { .. } => "bin",
                Instr::Chain { .. } => "chain",
                Instr::Flat2 { .. } => "flat2",
                Instr::Flat1 { .. } => "flat1",
                Instr::Pick { .. } => "pick",
                Instr::Copy { .. } => "copy",
                Instr::Ddx { .. } => "ddx",
                Instr::Tree { .. } => "tree",
                Instr::JumpIfZero { .. } => "jz",
                Instr::Jump { .. } => "jump",
            })
            .collect()
    }

    /// Whether `root` was compiled here.
    pub(crate) fn has_root(&self, root: ExprId) -> bool {
        self.root_block
            .get(root.0 as usize)
            .is_some_and(|b| *b != NONE)
    }
}

impl<'a> Run<'a> {
    /// The run of `prog` over `regs` for an evaluation at `x` (through `terminals`).
    pub(crate) fn new(
        prog: &'a Program,
        regs: &'a mut Regs,
        x: &[f64],
        terminals: &[usize],
    ) -> Self {
        let tv = if prog.jit.is_some() {
            (0..prog.n_slots)
                .map(|k| {
                    let g = terminals.get(k).copied().unwrap_or(usize::MAX);
                    x.get(g).copied().unwrap_or(0.0)
                })
                .collect()
        } else {
            Vec::new()
        };
        Run {
            prog,
            regs: RefCell::new(regs),
            tv,
        }
    }

    fn block(&self, root: ExprId) -> Result<Block, CodegenError> {
        let b = self
            .prog
            .root_block
            .get(root.0 as usize)
            .copied()
            .unwrap_or(NONE);
        if b == NONE {
            return Err(ad::internal_error("sched: root expression not compiled"));
        }
        Ok(self.prog.blocks[b as usize])
    }

    /// Run the statement headed by root `head` — an assignment (whose block writes its
    /// variable's register), or an `if` laid out whole ([`Self::is_whole`]) — together with the
    /// statements after it in its list whose code follows on: the same instructions in the
    /// same order as the statement walk would run them one by one, without returning to it in
    /// between. `remaining` is how many statements of the list, this one included, the caller
    /// has left; returns how many it ran.
    pub(crate) fn stretch(
        &self,
        ctx: &Ctx,
        head: ExprId,
        remaining: usize,
    ) -> Result<usize, CodegenError> {
        let b = self.block(head)?;
        let (end, len) = if (b.run_len as usize) <= remaining {
            (b.run_end, b.run_len as usize)
        } else {
            (b.stmt_end, 1)
        };
        exec(
            self.prog,
            &mut self.regs.borrow_mut(),
            &self.tv,
            ctx,
            b.start,
            end,
        )?;
        Ok(len)
    }

    /// Whether the `if` with condition `cond` is laid out whole, arms included, so that
    /// [`Self::stretch`] runs it.
    pub(crate) fn is_whole(&self, cond: ExprId) -> bool {
        self.prog
            .root_block
            .get(cond.0 as usize)
            .and_then(|b| self.prog.blocks.get(*b as usize))
            .is_some_and(|b| b.whole)
    }

    /// Run a root and write its gradient densely into `buf` (one entry per local slot, zero
    /// outside its support), as [`Self::dual`]'s `Dual` holds it; returns its value and whether it
    /// has a gradient at all (an empty support gives none, as `Dual::from_entries` does).
    pub(crate) fn dual_into(
        &self,
        ctx: &Ctx,
        root: ExprId,
        buf: &mut [f64],
    ) -> Result<(f64, bool), CodegenError> {
        let b = self.block(root)?;
        let mut regs = self.regs.borrow_mut();
        exec(self.prog, &mut regs, &self.tv, ctx, b.start, b.end)?;
        let r = b.result as usize;
        let (o, l) = (self.prog.off[r] as usize, self.prog.len[r] as usize);
        if l == 0 {
            return Ok((regs.vals[r], false));
        }
        buf.fill(0.0);
        for (&s, &v) in self.prog.slots[o..o + l].iter().zip(&regs.parts[o..o + l]) {
            if let Some(g) = buf.get_mut(s as usize) {
                *g = v;
            }
        }
        Ok((regs.vals[r], true))
    }

    /// Run a root and return its value only.
    pub(crate) fn value(&self, ctx: &Ctx, root: ExprId) -> Result<f64, CodegenError> {
        let b = self.block(root)?;
        let mut regs = self.regs.borrow_mut();
        exec(self.prog, &mut regs, &self.tv, ctx, b.start, b.end)?;
        Ok(regs.vals[b.result as usize])
    }

    /// Run a root and return it as a [`Dual`] — for a contribution term, which the existing
    /// stamping code consumes.
    pub(crate) fn dual(&self, ctx: &Ctx, root: ExprId) -> Result<Dual, CodegenError> {
        let b = self.block(root)?;
        let mut regs = self.regs.borrow_mut();
        exec(self.prog, &mut regs, &self.tv, ctx, b.start, b.end)?;
        let r = b.result as usize;
        let (o, l) = (self.prog.off[r] as usize, self.prog.len[r] as usize);
        Ok(Dual::from_entries(
            regs.vals[r],
            self.prog.n_slots,
            self.prog.slots[o..o + l]
                .iter()
                .zip(&regs.parts[o..o + l])
                .map(|(&s, &v)| (s as usize, v)),
        ))
    }
}

/// The root-by-root compilation of a statement list, with the bookkeeping [`Program`] keeps.
struct Layout<'c, 'm> {
    c: Compiler<'m>,
    assign_rhs: &'c HashMap<u32, VarId>,
    live: &'c [bool],
    /// How many times each root expression occurs among the per-load statements' roots.
    uses: &'c HashMap<u32, u32>,
    root_block: Vec<u32>,
    blocks: Vec<Block>,
}

impl Layout<'_, '_> {
    /// Compile one root expression (once; a root seen again keeps its first block).
    fn root(&mut self, root: ExprId) -> Result<u32, Refuse> {
        if self.root_block[root.0 as usize] != NONE {
            return Ok(self.root_block[root.0 as usize]);
        }
        let c = &mut self.c;
        let start = c.code.len() as u32;
        let maps_at = c.maps.len();
        // An assignment's root writes straight into its variable's register.
        let target = self
            .assign_rhs
            .get(&root.0)
            .map(|v| c.var_reg[v.0 as usize])
            .filter(|r| *r != NONE);
        let result = match target {
            Some(t) => {
                let r = c.expr(root, Some(t))?;
                if r != t {
                    let map = c.copy_map(t, r);
                    c.code.push(Instr::Copy {
                        dst: t,
                        src: r,
                        map,
                    });
                }
                t
            }
            None => c.expr(root, None)?,
        };
        // A dead assignment — its variable reaches no contribution, condition, bound or
        // task — keeps an empty block, unless its code calls out to the tree walker (which
        // can fail, and a failure must still stop the walk where it did).
        let dead = self
            .assign_rhs
            .get(&root.0)
            .is_some_and(|v| !self.live[v.0 as usize]);
        if dead
            && !c.code[start as usize..]
                .iter()
                .any(|i| matches!(i, Instr::Tree { .. }))
        {
            c.code.truncate(start as usize);
            c.maps.truncate(maps_at);
        }
        let end = c.code.len() as u32;
        let b = self.blocks.len() as u32;
        self.root_block[root.0 as usize] = b;
        self.blocks.push(Block {
            start,
            end,
            result,
            stmt_end: end,
            whole: false,
            run_len: 1,
            run_end: end,
        });
        Ok(b)
    }

    fn roots(&mut self, roots: impl IntoIterator<Item = ExprId>) -> Result<(), Refuse> {
        for r in roots {
            self.root(r)?;
        }
        Ok(())
    }

    /// Whether `if` statement `s` can be laid out whole: its arms nothing but assignments and
    /// such `if`s, and every root inside it (its condition, the arms' right-hand sides and
    /// conditions) a root of that one place alone and not compiled yet — so its code is laid
    /// out right there, and its block stands for nothing else.
    fn can_lay_whole(&self, s: &LoweredStmt) -> bool {
        let own =
            |e: &ExprId| self.root_block[e.0 as usize] == NONE && self.uses.get(&e.0) == Some(&1);
        match s {
            LoweredStmt::Assign { rhs, .. } => own(rhs),
            LoweredStmt::If { cond, then_, else_ } => {
                own(cond)
                    && then_.iter().chain(else_).all(|t| {
                        matches!(t, LoweredStmt::Assign { .. } | LoweredStmt::If { .. })
                            && self.can_lay_whole(t)
                    })
            }
            _ => false,
        }
    }

    /// Compile `stmts`' roots in [`collect_roots`]' order.
    fn list(&mut self, stmts: &[LoweredStmt]) -> Result<(), Refuse> {
        for s in stmts {
            match s {
                LoweredStmt::Assign { rhs, .. } => {
                    self.root(*rhs)?;
                }
                LoweredStmt::BoundStep(e) => {
                    self.root(*e)?;
                }
                LoweredStmt::Task { args, .. } => {
                    self.roots(args.iter().filter_map(va_ir::TaskArg::expr))?;
                }
                LoweredStmt::Contribute(c) => {
                    self.roots(c.resistive.iter().map(|t| t.expr))?;
                    for t in &c.charge {
                        self.root(t.expr)?;
                        self.roots(t.coeffs.iter().map(|(e, _)| *e))?;
                    }
                }
                LoweredStmt::If { cond, then_, else_ } if self.can_lay_whole(s) => {
                    let b = self.root(*cond)?;
                    let rc = self.blocks[b as usize].result;
                    let jz = self.c.code.len();
                    self.c.code.push(Instr::JumpIfZero { cond: rc, to: 0 });
                    self.list(then_)?;
                    let jump = (!else_.is_empty()).then(|| {
                        self.c.code.push(Instr::Jump { to: 0 });
                        self.c.code.len() - 1
                    });
                    let else_at = self.c.code.len() as u32;
                    self.list(else_)?;
                    let end = self.c.code.len() as u32;
                    self.c.code[jz] = Instr::JumpIfZero {
                        cond: rc,
                        to: else_at,
                    };
                    if let Some(j) = jump {
                        self.c.code[j] = Instr::Jump { to: end };
                    }
                    let blk = &mut self.blocks[b as usize];
                    blk.stmt_end = end;
                    blk.whole = true;
                    blk.run_end = end;
                }
                LoweredStmt::If { cond, then_, else_ } => {
                    self.root(*cond)?;
                    self.list(then_)?;
                    self.list(else_)?;
                }
                LoweredStmt::Case {
                    selector,
                    arms,
                    default,
                } => {
                    self.root(*selector)?;
                    for a in arms {
                        self.roots(a.labels.iter().copied())?;
                        self.list(&a.body)?;
                    }
                    self.list(default)?;
                }
                LoweredStmt::While { cond, body } => {
                    self.root(*cond)?;
                    self.list(body)?;
                }
                LoweredStmt::For {
                    init,
                    cond,
                    step,
                    body,
                } => {
                    self.list(init)?;
                    self.root(*cond)?;
                    self.list(step)?;
                    self.list(body)?;
                }
                LoweredStmt::Repeat { count, body } => {
                    self.root(*count)?;
                    self.list(body)?;
                }
            }
        }
        Ok(())
    }
}

impl Compiler<'_> {
    fn new_reg(&mut self, s: &Support) -> u32 {
        let r = self.off.len() as u32;
        let slots = s.slots();
        self.off.push(self.slots.len() as u32);
        self.len.push(slots.len() as u32);
        self.init_parts
            .extend(std::iter::repeat_n(0.0, slots.len()));
        self.slots.extend(slots);
        self.init_vals.push(0.0);
        self.konst.push(false);
        r
    }

    fn const_reg(&mut self, v: f64) -> u32 {
        if let Some(r) = self.consts.get(&v.to_bits()) {
            return *r;
        }
        let r = self.new_reg(&Support::empty(self.n));
        self.init_vals[r as usize] = v;
        self.konst[r as usize] = true;
        self.consts.insert(v.to_bits(), r);
        r
    }

    /// The value of register `r` if it is fixed for the program's life.
    fn konst_val(&self, r: u32) -> Option<f64> {
        self.konst[r as usize].then(|| self.init_vals[r as usize])
    }

    fn reg_slots(&self, r: u32) -> &[u32] {
        let (o, l) = (self.off[r as usize] as usize, self.len[r as usize] as usize);
        &self.slots[o..o + l]
    }

    /// For each partial of `dst`, the absolute index of the same slot in `a` (or `NONE`).
    fn index_of(&self, a: u32, slot: u32) -> u32 {
        let o = self.off[a as usize];
        self.reg_slots(a)
            .iter()
            .position(|&s| s == slot)
            .map_or(NONE, |i| o + i as u32)
    }

    fn map2(&mut self, dst: u32, a: u32, b: u32) -> u32 {
        let start = self.maps.len() as u32;
        let ds: Vec<u32> = self.reg_slots(dst).to_vec();
        for s in ds {
            let ia = self.index_of(a, s);
            let ib = if b == NONE { NONE } else { self.index_of(b, s) };
            self.maps.push((ia, ib));
        }
        start
    }

    fn copy_map(&mut self, dst: u32, src: u32) -> u32 {
        self.map2(dst, src, NONE)
    }

    fn support(&mut self, e: ExprId) -> Result<Support, Refuse> {
        if let Some(s) = self.support_memo.get(&e.0) {
            return Ok(s.clone());
        }
        let n = self.n;
        let s = match self.module.expr(e) {
            Expr::Const(_)
            | Expr::Param(_)
            | Expr::ParamGiven(_)
            | Expr::PortConnected(_)
            | Expr::EventFired(_)
            | Expr::Ddx(..) => Support::empty(n),
            Expr::Var(v) => self.var_support[v.0 as usize].clone(),
            Expr::Probe(access) if access.kind == AccessKind::Flow => Support::full(n),
            Expr::Probe(access) => {
                let mut s = Support::empty(n);
                for k in probe_slots(self.module, access, n).into_iter().flatten() {
                    s.insert(k);
                }
                s
            }
            Expr::Unary(UnOp::Neg, a) => self.support(*a)?,
            Expr::Unary(..) => Support::empty(n),
            Expr::Binary(op, a, b) => match op {
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Pow => {
                    let mut s = self.support(*a)?;
                    s.union(&self.support(*b)?);
                    s
                }
                _ => Support::empty(n),
            },
            Expr::Select(_, a, b) => {
                let mut s = self.support(*a)?;
                s.union(&self.support(*b)?);
                s
            }
            Expr::Call(builtin, args) => {
                if let Some(ch) = chain_of(*builtin) {
                    let _ = ch;
                    self.support(args[0])?
                } else if flat_call(*builtin) {
                    Support::empty(n)
                } else if bin_call(*builtin).is_some() || pick_call(*builtin).is_some() {
                    let mut s = self.support(args[0])?;
                    s.union(&self.support(args[1])?);
                    s
                } else {
                    tree_support(self.module, e, n)?
                }
            }
            Expr::CallUser(..) => tree_support(self.module, e, n)?,
        };
        self.support_memo.insert(e.0, s.clone());
        Ok(s)
    }

    /// Compile `e`; its value ends up in the returned register. `hint` is the variable register
    /// an assignment's root may write straight into.
    fn expr(&mut self, e: ExprId, hint: Option<u32>) -> Result<u32, Refuse> {
        let n = self.n;
        let module = self.module;
        let dst_for = |c: &mut Compiler, s: &Support| hint.unwrap_or_else(|| c.new_reg(s));
        match module.expr(e) {
            Expr::Const(v) => Ok(self.const_reg(*v)),
            Expr::Param(p) => {
                let v = *self
                    .params
                    .get(p.0 as usize)
                    .ok_or("parameter out of range")?;
                Ok(self.const_reg(v))
            }
            Expr::ParamGiven(p) => {
                Ok(self.const_reg(if module.param_is_given(*p) { 1.0 } else { 0.0 }))
            }
            Expr::PortConnected(i) => {
                Ok(self.const_reg(if module.port_is_connected(*i as usize) {
                    1.0
                } else {
                    0.0
                }))
            }
            Expr::Var(v) => {
                let r = self.var_reg[v.0 as usize];
                if r == NONE {
                    return Err("variable without a register");
                }
                Ok(r)
            }
            // A flow read's slot is the context's (`Ctx::branch_current_slots`): tree-walked.
            Expr::Probe(access) if access.kind == AccessKind::Flow => self.tree(e, hint),
            Expr::Probe(access) => {
                // Its own register (constant partials), never the hint.
                let s = self.support(e)?;
                let r = self.new_reg(&s);
                let [p, m] = probe_slots(module, access, n);
                let (raw_p, raw_n) = probe_raw(module, access);
                for (k, slot) in self.reg_slots(r).to_vec().into_iter().enumerate() {
                    // The dense form's arithmetic: `0.0`, `+= 1.0`, `-= 1.0`.
                    let mut g = 0.0;
                    if Some(slot as usize) == p {
                        g += 1.0;
                    }
                    if Some(slot as usize) == m {
                        g -= 1.0;
                    }
                    let o = self.off[r as usize] as usize;
                    self.init_parts[o + k] = g;
                }
                self.code.push(Instr::Probe {
                    dst: r,
                    p: raw_p,
                    n: raw_n,
                });
                Ok(r)
            }
            Expr::Unary(UnOp::Neg, a) => {
                let ra = self.expr(*a, None)?;
                if let Some(v) = self.konst_val(ra) {
                    return Ok(self.const_reg(chain_value(Chain::Neg, v).0));
                }
                let s = self.support(e)?;
                let dst = dst_for(self, &s);
                let map = self.map2(dst, ra, NONE);
                self.code.push(Instr::Chain {
                    op: Chain::Neg,
                    dst,
                    a: ra,
                    map,
                });
                Ok(dst)
            }
            Expr::Unary(op, a) => {
                let ra = self.expr(*a, None)?;
                if let Some(v) = self.konst_val(ra) {
                    return Ok(self.const_reg(flat1_value(Flat1::Unary(*op), v)));
                }
                let dst = dst_for(self, &Support::empty(n));
                self.code.push(Instr::Flat1 {
                    op: Flat1::Unary(*op),
                    dst,
                    a: ra,
                });
                Ok(dst)
            }
            Expr::Binary(op, a, b) => {
                let ra = self.expr(*a, None)?;
                let rb = self.expr(*b, None)?;
                let bin = match op {
                    BinOp::Add => Some(Bin::Add),
                    BinOp::Sub => Some(Bin::Sub),
                    BinOp::Mul => Some(Bin::Mul),
                    BinOp::Div => Some(Bin::Div),
                    BinOp::Pow => Some(Bin::Pow),
                    _ => None,
                };
                if let (Some(av), Some(bv)) = (self.konst_val(ra), self.konst_val(rb)) {
                    let v = match bin {
                        Some(bin) => bin_value(bin, av, bv),
                        None => flat2_value(*op, av, bv),
                    };
                    return Ok(self.const_reg(v));
                }
                match bin {
                    Some(bin) => {
                        let s = self.support(e)?;
                        let dst = dst_for(self, &s);
                        let map = self.map2(dst, ra, rb);
                        self.code.push(Instr::Bin {
                            op: bin,
                            dst,
                            a: ra,
                            b: rb,
                            map,
                        });
                        Ok(dst)
                    }
                    None => {
                        let dst = dst_for(self, &Support::empty(n));
                        self.code.push(Instr::Flat2 {
                            op: *op,
                            dst,
                            a: ra,
                            b: rb,
                        });
                        Ok(dst)
                    }
                }
            }
            Expr::Select(cond, a, b) => {
                let s = self.support(e)?;
                let dst = dst_for(self, &s);
                let rc = self.expr(*cond, None)?;
                let jz = self.code.len();
                self.code.push(Instr::JumpIfZero { cond: rc, to: 0 });
                let ra = self.expr(*a, None)?;
                let map = self.copy_map(dst, ra);
                self.code.push(Instr::Copy { dst, src: ra, map });
                let j = self.code.len();
                self.code.push(Instr::Jump { to: 0 });
                let else_at = self.code.len() as u32;
                let rb = self.expr(*b, None)?;
                let map = self.copy_map(dst, rb);
                self.code.push(Instr::Copy { dst, src: rb, map });
                let end = self.code.len() as u32;
                self.code[jz] = Instr::JumpIfZero {
                    cond: rc,
                    to: else_at,
                };
                self.code[j] = Instr::Jump { to: end };
                Ok(dst)
            }
            Expr::Ddx(inner, access) => {
                let ri = self.expr(*inner, None)?;
                let br = module.branches[access.branch.0 as usize];
                let p = br.p.0;
                let idx = self.index_of(ri, p);
                let dst = dst_for(self, &Support::empty(n));
                self.code.push(Instr::Ddx { dst, idx });
                Ok(dst)
            }
            Expr::Call(builtin, args) => {
                if let Some(op) = chain_of(*builtin) {
                    let ra = self.expr(*args.first().ok_or("call arity")?, None)?;
                    if let Some(v) = self.konst_val(ra) {
                        return Ok(self.const_reg(chain_value(op, v).0));
                    }
                    let s = self.support(e)?;
                    let dst = dst_for(self, &s);
                    let map = self.map2(dst, ra, NONE);
                    self.code.push(Instr::Chain {
                        op,
                        dst,
                        a: ra,
                        map,
                    });
                    Ok(dst)
                } else if flat_call(*builtin) {
                    let ra = self.expr(*args.first().ok_or("call arity")?, None)?;
                    if let Some(v) = self.konst_val(ra) {
                        return Ok(self.const_reg(flat1_value(Flat1::Call(*builtin), v)));
                    }
                    let dst = dst_for(self, &Support::empty(n));
                    self.code.push(Instr::Flat1 {
                        op: Flat1::Call(*builtin),
                        dst,
                        a: ra,
                    });
                    Ok(dst)
                } else if let Some(op) = bin_call(*builtin) {
                    if args.len() < 2 {
                        return Err("call arity");
                    }
                    let ra = self.expr(args[0], None)?;
                    let rb = self.expr(args[1], None)?;
                    if let (Some(av), Some(bv)) = (self.konst_val(ra), self.konst_val(rb)) {
                        return Ok(self.const_reg(bin_value(op, av, bv)));
                    }
                    let s = self.support(e)?;
                    let dst = dst_for(self, &s);
                    let map = self.map2(dst, ra, rb);
                    self.code.push(Instr::Bin {
                        op,
                        dst,
                        a: ra,
                        b: rb,
                        map,
                    });
                    Ok(dst)
                } else if let Some(max) = pick_call(*builtin) {
                    if args.len() < 2 {
                        return Err("call arity");
                    }
                    let ra = self.expr(args[0], None)?;
                    let rb = self.expr(args[1], None)?;
                    if let (Some(av), Some(bv)) = (self.konst_val(ra), self.konst_val(rb)) {
                        return Ok(self.const_reg(if pick_takes_a(max, av, bv) { av } else { bv }));
                    }
                    let s = self.support(e)?;
                    let dst = dst_for(self, &s);
                    let map_a = self.copy_map(dst, ra);
                    let map_b = self.copy_map(dst, rb);
                    self.code.push(Instr::Pick {
                        max,
                        dst,
                        a: ra,
                        b: rb,
                        map_a,
                        map_b,
                    });
                    Ok(dst)
                } else {
                    self.tree(e, hint)
                }
            }
            Expr::CallUser(..) => self.tree(e, hint),
            Expr::EventFired(_) => self.tree(e, hint),
        }
    }

    fn tree(&mut self, e: ExprId, hint: Option<u32>) -> Result<u32, Refuse> {
        let s = tree_support(self.module, e, self.n)?;
        let r = self.new_reg(&s);
        self.code.push(Instr::Tree { dst: r, expr: e });
        let _ = hint;
        Ok(r)
    }
}

/// The support a tree-walked subexpression gets: refused if it reads a variable or holds a
/// `ddt`; every slot if it can depend on the unknowns at all, none otherwise.
fn tree_support(module: &Module, e: ExprId, n: usize) -> Result<Support, Refuse> {
    let mut reads_var = false;
    let mut biased = false;
    let mut has_ddt = false;
    scan(module, e, &mut |x| match x {
        Expr::Var(_) => reads_var = true,
        Expr::Probe(_) | Expr::Ddx(..) => biased = true,
        Expr::Call(Builtin::Ddt, _) => has_ddt = true,
        Expr::Call(..) | Expr::CallUser(..) => biased = true,
        _ => {}
    });
    if reads_var {
        return Err("a non-maths call reads a variable");
    }
    if has_ddt {
        return Err("ddt inside an expression");
    }
    Ok(if biased {
        Support::full(n)
    } else {
        Support::empty(n)
    })
}

fn scan(module: &Module, e: ExprId, f: &mut impl FnMut(&Expr)) {
    let x = module.expr(e);
    f(x);
    match x {
        Expr::Unary(_, a) | Expr::Ddx(a, _) => scan(module, *a, f),
        Expr::Binary(_, a, b) => {
            scan(module, *a, f);
            scan(module, *b, f);
        }
        Expr::Select(c, a, b) => {
            scan(module, *c, f);
            scan(module, *a, f);
            scan(module, *b, f);
        }
        Expr::Call(_, args) | Expr::CallUser(_, args) => {
            for a in args {
                scan(module, *a, f);
            }
        }
        _ => {}
    }
}

/// A probe's `(plus, minus)` local slots, `None` where out of range (ground).
fn probe_slots(module: &Module, access: &va_ir::Access, n: usize) -> [Option<usize>; 2] {
    let (p, m) = probe_raw(module, access);
    let ok = |k: u32| (k != NONE && (k as usize) < n).then_some(k as usize);
    [ok(p), ok(m)]
}

/// A potential probe's raw `(plus, minus)` node slots. A flow probe's slot is the context's, so
/// flow probes never reach here (they are tree-walked); `NONE` for them all the same.
fn probe_raw(module: &Module, access: &va_ir::Access) -> (u32, u32) {
    match access.kind {
        AccessKind::Potential => {
            let br = module.branches[access.branch.0 as usize];
            (br.p.0, br.n.0)
        }
        AccessKind::Flow => (NONE, NONE),
    }
}

fn chain_of(b: Builtin) -> Option<Chain> {
    Some(match b {
        Builtin::Exp => Chain::Exp,
        Builtin::Ln => Chain::Ln,
        Builtin::Log => Chain::Log10,
        Builtin::Sqrt => Chain::Sqrt,
        Builtin::Abs => Chain::Abs,
        Builtin::Sin => Chain::Sin,
        Builtin::Cos => Chain::Cos,
        Builtin::Tan => Chain::Tan,
        Builtin::Sinh => Chain::Sinh,
        Builtin::Cosh => Chain::Cosh,
        Builtin::Tanh => Chain::Tanh,
        Builtin::Asin => Chain::Asin,
        Builtin::Acos => Chain::Acos,
        Builtin::Atan => Chain::Atan,
        Builtin::Asinh => Chain::Asinh,
        Builtin::Acosh => Chain::Acosh,
        Builtin::Atanh => Chain::Atanh,
        _ => return None,
    })
}

fn flat_call(b: Builtin) -> bool {
    matches!(
        b,
        Builtin::Floor | Builtin::Ceil | Builtin::Round | Builtin::Int
    )
}

fn bin_call(b: Builtin) -> Option<Bin> {
    Some(match b {
        Builtin::Pow => Bin::Pow,
        Builtin::Hypot => Bin::Hypot,
        Builtin::Atan2 => Bin::Atan2,
        _ => return None,
    })
}

fn pick_call(b: Builtin) -> Option<bool> {
    match b {
        Builtin::Min => Some(false),
        Builtin::Max => Some(true),
        _ => None,
    }
}

/// Mark, in every statement list, each maximal stretch of consecutive assignments whose blocks
/// follow one another in the code: the stretch's first block learns its length and end
/// ([`Block::run_len`], [`Block::run_end`]).
fn mark_runs(stmts: &[LoweredStmt], root_block: &[u32], blocks: &mut [Block]) {
    let head = |e: &ExprId| root_block.get(e.0 as usize).copied().filter(|b| *b != NONE);
    let block_of = |s: &LoweredStmt, blocks: &[Block]| match s {
        LoweredStmt::Assign { rhs, .. } => head(rhs),
        LoweredStmt::If { cond, .. } => head(cond).filter(|b| blocks[*b as usize].whole),
        _ => None,
    };
    let mut i = 0;
    while i < stmts.len() {
        let Some(first) = block_of(&stmts[i], blocks) else {
            i += 1;
            continue;
        };
        let mut last = first;
        let mut len = 1;
        while let Some(next) = stmts.get(i + len).and_then(|t| block_of(t, blocks)) {
            if blocks[next as usize].start != blocks[last as usize].stmt_end {
                break;
            }
            last = next;
            len += 1;
        }
        let end = blocks[last as usize].stmt_end;
        let b = &mut blocks[first as usize];
        b.run_len = len as u32;
        b.run_end = end;
        i += len;
    }
    visit_children(stmts, &mut |list| mark_runs(list, root_block, blocks));
}

/// Call `f` on each statement list nested directly in `stmts` (an arm, a loop body, …).
fn visit_children(stmts: &[LoweredStmt], f: &mut impl FnMut(&[LoweredStmt])) {
    for s in stmts {
        match s {
            LoweredStmt::Assign { .. }
            | LoweredStmt::Contribute(_)
            | LoweredStmt::BoundStep(_)
            | LoweredStmt::Task { .. } => {}
            LoweredStmt::If { then_, else_, .. } => {
                f(then_);
                f(else_);
            }
            LoweredStmt::Case { arms, default, .. } => {
                for a in arms {
                    f(&a.body);
                }
                f(default);
            }
            LoweredStmt::While { body, .. } | LoweredStmt::Repeat { body, .. } => f(body),
            LoweredStmt::For {
                init, step, body, ..
            } => {
                f(init);
                f(step);
                f(body);
            }
        }
    }
}

fn collect_assigns(stmts: &[LoweredStmt], out: &mut Vec<(VarId, ExprId)>) {
    for s in stmts {
        match s {
            LoweredStmt::Assign { lhs, rhs } => out.push((*lhs, *rhs)),
            LoweredStmt::Contribute(_) | LoweredStmt::BoundStep(_) | LoweredStmt::Task { .. } => {}
            LoweredStmt::If { then_, else_, .. } => {
                collect_assigns(then_, out);
                collect_assigns(else_, out);
            }
            LoweredStmt::Case { arms, default, .. } => {
                for a in arms {
                    collect_assigns(&a.body, out);
                }
                collect_assigns(default, out);
            }
            LoweredStmt::While { body, .. } | LoweredStmt::Repeat { body, .. } => {
                collect_assigns(body, out)
            }
            LoweredStmt::For {
                init, step, body, ..
            } => {
                collect_assigns(init, out);
                collect_assigns(step, out);
                collect_assigns(body, out);
            }
        }
    }
}

/// Every root expression the statements evaluate (the same set `crate::tape` compiles).
fn collect_roots(stmts: &[LoweredStmt], out: &mut Vec<ExprId>) {
    for s in stmts {
        match s {
            LoweredStmt::Assign { rhs, .. } => out.push(*rhs),
            LoweredStmt::BoundStep(e) => out.push(*e),
            LoweredStmt::Task { args, .. } => {
                out.extend(args.iter().filter_map(va_ir::TaskArg::expr));
            }
            LoweredStmt::Contribute(c) => {
                out.extend(c.resistive.iter().map(|t| t.expr));
                for t in &c.charge {
                    out.push(t.expr);
                    out.extend(t.coeffs.iter().map(|(e, _)| *e));
                }
            }
            LoweredStmt::If { cond, then_, else_ } => {
                out.push(*cond);
                collect_roots(then_, out);
                collect_roots(else_, out);
            }
            LoweredStmt::Case {
                selector,
                arms,
                default,
            } => {
                out.push(*selector);
                for a in arms {
                    out.extend(a.labels.iter().copied());
                    collect_roots(&a.body, out);
                }
                collect_roots(default, out);
            }
            LoweredStmt::While { cond, body } => {
                out.push(*cond);
                collect_roots(body, out);
            }
            LoweredStmt::For {
                init,
                cond,
                step,
                body,
            } => {
                collect_roots(init, out);
                out.push(*cond);
                collect_roots(step, out);
                collect_roots(body, out);
            }
            LoweredStmt::Repeat { count, body } => {
                out.push(*count);
                collect_roots(body, out);
            }
        }
    }
}

/// Which variables the per-load statements need: those read by a root that is not an
/// assignment's right-hand side (a contribution term, a condition, a bound, a task argument),
/// and, to a fixpoint, those read by an assignment to a needed variable. Flow-insensitive, so
/// conservative: a variable read anywhere it matters is needed everywhere it is assigned.
/// Visiting the sinks in a hash map's order is fine: the result is a set.
fn live_vars(
    module: &Module,
    roots: &[ExprId],
    assigns: &[(VarId, ExprId)],
    assign_rhs: &HashMap<u32, VarId>,
) -> Vec<bool> {
    let mut live = vec![false; module.vars.len()];
    let mark = |e: ExprId, live: &mut Vec<bool>| {
        let mut changed = false;
        scan(module, e, &mut |x| {
            if let Expr::Var(v) = x {
                if let Some(l) = live.get_mut(v.0 as usize) {
                    changed |= !*l;
                    *l = true;
                }
            }
        });
        changed
    };
    // A root is a sink where it occurs other than as an assignment's right-hand side. Counted
    // rather than looked up: an expression shared between an assignment and, say, a
    // contribution is both, and then the assignment (whose block computes it) must stay too.
    let mut count: HashMap<u32, i64> = HashMap::new();
    for r in roots {
        *count.entry(r.0).or_default() += 1;
    }
    for (_, rhs) in assigns {
        *count.entry(rhs.0).or_default() -= 1;
    }
    for (&r, &n) in &count {
        if n > 0 {
            mark(ExprId(r), &mut live);
            if let Some(l) = assign_rhs.get(&r).and_then(|v| live.get_mut(v.0 as usize)) {
                *l = true;
            }
        }
    }
    loop {
        let mut changed = false;
        for (lhs, rhs) in assigns {
            if live.get(lhs.0 as usize).copied().unwrap_or(true) {
                changed |= mark(*rhs, &mut live);
            }
        }
        if !changed {
            return live;
        }
    }
}

/// Every variable a per-load statement can write: an assignment's target or a task's.
fn mark_written(stmts: &[LoweredStmt], written: &mut [bool]) {
    let mut assigns = Vec::new();
    collect_assigns(stmts, &mut assigns);
    for (l, _) in assigns {
        written[l.0 as usize] = true;
    }
    visit_stmts(stmts, &mut |s| {
        if let LoweredStmt::Task {
            target: Some(v), ..
        } = s
        {
            if let Some(w) = written.get_mut(v.0 as usize) {
                *w = true;
            }
        }
    });
}

/// Call `f` on every statement in `stmts`, nested ones included.
fn visit_stmts(stmts: &[LoweredStmt], f: &mut impl FnMut(&LoweredStmt)) {
    for s in stmts {
        f(s);
    }
    visit_children(stmts, &mut |list| visit_stmts(list, &mut *f));
}

fn mark_vars(module: &Module, stmts: &[LoweredStmt], used: &mut [bool]) {
    let mut roots = Vec::new();
    collect_roots(stmts, &mut roots);
    for r in roots {
        scan(module, r, &mut |x| {
            if let Expr::Var(v) = x {
                used[v.0 as usize] = true;
            }
        });
    }
    let mut assigns = Vec::new();
    collect_assigns(stmts, &mut assigns);
    for (l, _) in assigns {
        used[l.0 as usize] = true;
    }
}

/// Definite assignment over the per-load statements: every variable read is preceded, on every
/// path, by an assignment in this evaluation or holds a setup value. Conservative where the tree
/// walk is lenient (an untaken `?:` arm, a loop body read before its own assignment).
fn check_da(module: &Module, stmts: &[LoweredStmt], da: &mut Vec<bool>) -> Result<(), Refuse> {
    let reads_ok = |e: ExprId, da: &[bool]| {
        let mut ok = true;
        scan(module, e, &mut |x| {
            if let Expr::Var(v) = x {
                ok &= da.get(v.0 as usize).copied().unwrap_or(false);
            }
        });
        if ok {
            Ok(())
        } else {
            Err("a variable may be read before it is assigned")
        }
    };
    for s in stmts {
        match s {
            LoweredStmt::Assign { lhs, rhs } => {
                reads_ok(*rhs, da)?;
                da[lhs.0 as usize] = true;
            }
            LoweredStmt::BoundStep(e) => reads_ok(*e, da)?,
            // `$fopen` assigns its variable outside the schedule's registers.
            LoweredStmt::Task {
                target: Some(_), ..
            } => return Err("a $fopen descriptor"),
            LoweredStmt::Task { args, .. } => {
                for e in args.iter().filter_map(va_ir::TaskArg::expr) {
                    reads_ok(e, da)?;
                }
            }
            LoweredStmt::Contribute(c) => {
                for t in &c.resistive {
                    reads_ok(t.expr, da)?;
                }
                for t in &c.charge {
                    reads_ok(t.expr, da)?;
                    for (k, _) in &t.coeffs {
                        reads_ok(*k, da)?;
                    }
                }
            }
            LoweredStmt::If { cond, then_, else_ } => {
                reads_ok(*cond, da)?;
                let mut a = da.clone();
                check_da(module, then_, &mut a)?;
                let mut b = da.clone();
                check_da(module, else_, &mut b)?;
                for (d, (x, y)) in da.iter_mut().zip(a.iter().zip(&b)) {
                    *d = *x && *y;
                }
            }
            LoweredStmt::Case {
                selector,
                arms,
                default,
            } => {
                reads_ok(*selector, da)?;
                let mut acc = da.clone();
                check_da(module, default, &mut acc)?;
                for arm in arms {
                    for l in &arm.labels {
                        reads_ok(*l, da)?;
                    }
                    let mut a = da.clone();
                    check_da(module, &arm.body, &mut a)?;
                    for (x, y) in acc.iter_mut().zip(&a) {
                        *x = *x && *y;
                    }
                }
                *da = acc;
            }
            LoweredStmt::While { cond, body } => {
                reads_ok(*cond, da)?;
                check_da(module, body, &mut da.clone())?;
            }
            LoweredStmt::For {
                init,
                cond,
                step,
                body,
            } => {
                check_da(module, init, da)?;
                reads_ok(*cond, da)?;
                let mut inner = da.clone();
                check_da(module, body, &mut inner)?;
                check_da(module, step, &mut inner)?;
            }
            LoweredStmt::Repeat { count, body } => {
                reads_ok(*count, da)?;
                check_da(module, body, &mut da.clone())?;
            }
        }
    }
    Ok(())
}

#[inline]
fn get(parts: &[f64], i: u32) -> f64 {
    parts[i as usize]
}

/// `r`, or `0.0` where both operand partials `p` and `q` are zero — the rule `Grad::Zero` has
/// always applied (a slot zero in both operands is zero in the result, whatever the rule would
/// compute there). Exactly `if p == 0.0 && q == 0.0 { 0.0 } else { r }`, `-0.0` and `NaN`
/// included, but as a mask rather than a branch: which partials are zero varies from
/// instruction to instruction, and the branch it replaced was mispredicted often enough to be
/// the evaluator's largest single cost. `r` is computed either way; it is a pure function of
/// finite or non-finite floats, so computing it where it is discarded changes nothing.
#[inline(always)]
fn unless_both_zero(p: f64, q: f64, r: f64) -> f64 {
    let live = (p != 0.0) | (q != 0.0);
    f64::from_bits(r.to_bits() & u64::from(live).wrapping_neg())
}

/// Run `code[start..end]`.
fn exec(
    prog: &Program,
    regs: &mut Regs,
    tv: &[f64],
    ctx: &Ctx,
    start: u32,
    end: u32,
) -> Result<(), CodegenError> {
    let Regs { vals, parts } = regs;
    match &prog.jit {
        Some(j) => j.run(prog, vals, parts, tv, ctx, start, end),
        None => exec_slices(prog, vals, parts, ctx, start, end),
    }
}

/// [`exec`]'s interpreter, over the register file's two halves.
fn exec_slices(
    prog: &Program,
    vals: &mut [f64],
    parts: &mut [f64],
    ctx: &Ctx,
    start: u32,
    end: u32,
) -> Result<(), CodegenError> {
    let mut pc = start as usize;
    let end = end as usize;
    while pc < end {
        match prog.code[pc] {
            Instr::Probe { dst, p, n } => {
                let v = |k: u32| {
                    if k == NONE {
                        0.0
                    } else {
                        let g = ctx.terminals.get(k as usize).copied().unwrap_or(usize::MAX);
                        ctx.x.get(g).copied().unwrap_or(0.0)
                    }
                };
                vals[dst as usize] = v(p) - v(n);
            }
            Instr::Bin { op, dst, a, b, map } => {
                let (sv, ov) = (vals[a as usize], vals[b as usize]);
                let o = prog.off[dst as usize] as usize;
                let l = prog.len[dst as usize] as usize;
                let m = &prog.maps[map as usize..map as usize + l];
                macro_rules! rule {
                    ($f:expr) => {
                        for (k, &(ia, ib)) in m.iter().enumerate() {
                            let p = get(parts, ia);
                            let q = get(parts, ib);
                            parts[o + k] = unless_both_zero(p, q, $f(p, q));
                        }
                    };
                }
                let value = match op {
                    Bin::Add => {
                        rule!(|p: f64, q: f64| p + q);
                        sv + ov
                    }
                    Bin::Sub => {
                        rule!(|p: f64, q: f64| p - q);
                        sv - ov
                    }
                    Bin::Mul => {
                        rule!(|p: f64, q: f64| p * ov + q * sv);
                        sv * ov
                    }
                    Bin::Div => {
                        let inv = 1.0 / ov;
                        let inv2 = inv * inv;
                        rule!(|p: f64, q: f64| (p * ov - sv * q) * inv2);
                        sv * inv
                    }
                    Bin::Pow => {
                        let (u, v) = (sv, ov);
                        let value = libm::pow(u, v);
                        let d_du = v * libm::pow(u, v - 1.0);
                        let bo = prog.off[b as usize] as usize;
                        let bl = prog.len[b as usize] as usize;
                        let exp_varies = parts[bo..bo + bl].iter().any(|g| *g != 0.0);
                        let d_dv = if exp_varies {
                            value * libm::log(u)
                        } else {
                            0.0
                        };
                        rule!(|sg: f64, eg: f64| {
                            let base = if sg == 0.0 { 0.0 } else { d_du * sg };
                            let expo = if eg == 0.0 { 0.0 } else { d_dv * eg };
                            base + expo
                        });
                        value
                    }
                    Bin::Hypot => {
                        let value = libm::hypot(sv, ov);
                        let inv = crate::ad::kink_reciprocal(value);
                        rule!(|sg: f64, og: f64| (sv * sg + ov * og) * inv);
                        value
                    }
                    Bin::Atan2 => {
                        let inv = crate::ad::kink_reciprocal(sv * sv + ov * ov);
                        rule!(|yg: f64, xg: f64| (ov * yg - sv * xg) * inv);
                        libm::atan2(sv, ov)
                    }
                };
                vals[dst as usize] = value;
            }
            Instr::Chain { op, dst, a, map } => {
                let v = vals[a as usize];
                let (value, d) = chain_value(op, v);
                let o = prog.off[dst as usize] as usize;
                let l = prog.len[dst as usize] as usize;
                for (k, &(ia, _)) in prog.maps[map as usize..map as usize + l].iter().enumerate() {
                    let g = get(parts, ia);
                    parts[o + k] = unless_both_zero(g, 0.0, g * d);
                }
                vals[dst as usize] = value;
            }
            Instr::Flat2 { op, dst, a, b } => {
                vals[dst as usize] = flat2_value(op, vals[a as usize], vals[b as usize]);
                zero(prog, parts, dst);
            }
            Instr::Flat1 { op, dst, a } => {
                vals[dst as usize] = flat1_value(op, vals[a as usize]);
                zero(prog, parts, dst);
            }
            Instr::Pick {
                max,
                dst,
                a,
                b,
                map_a,
                map_b,
            } => {
                let (sv, ov) = (vals[a as usize], vals[b as usize]);
                let take_a = pick_takes_a(max, sv, ov);
                let (src, map) = if take_a { (a, map_a) } else { (b, map_b) };
                copy(prog, vals, parts, dst, src, map);
            }
            Instr::Copy { dst, src, map } => copy(prog, vals, parts, dst, src, map),
            Instr::Ddx { dst, idx } => {
                vals[dst as usize] = get(parts, idx);
                zero(prog, parts, dst);
            }
            Instr::Tree { dst, expr } => {
                let d = ad::eval(ctx, expr)?;
                if d.carries_charge() {
                    return Err(ad::internal_error("sched: tree value carries charge"));
                }
                vals[dst as usize] = d.value;
                let o = prog.off[dst as usize] as usize;
                let l = prog.len[dst as usize] as usize;
                for k in 0..l {
                    parts[o + k] = d.grad_at(prog.slots[o + k] as usize);
                }
                if l == 0 && d.depends_on_unknowns() {
                    return Err(ad::internal_error("sched: tree value outside its support"));
                }
            }
            Instr::JumpIfZero { cond, to } => {
                if vals[cond as usize] == 0.0 {
                    pc = to as usize;
                    continue;
                }
            }
            Instr::Jump { to } => {
                pc = to as usize;
                continue;
            }
        }
        pc += 1;
    }
    Ok(())
}

/// A value-only binary operator's result — what [`Instr::Flat2`] computes.
fn flat2_value(op: BinOp, a: f64, b: f64) -> f64 {
    ad::apply_binary(op, Dual::constant(a), Dual::constant(b)).value
}

/// A value-only unary operator's or rounding call's result — what [`Instr::Flat1`] computes.
fn flat1_value(op: Flat1, a: f64) -> f64 {
    let x = Dual::constant(a);
    match op {
        Flat1::Unary(u) => ad::apply_unary(u, x).value,
        Flat1::Call(b) => ad::pure_call1(b, x).map_or(f64::NAN, |d| d.value),
    }
}

/// Whether `min`/`max` selects its first operand — what [`Instr::Pick`] decides.
fn pick_takes_a(max: bool, a: f64, b: f64) -> bool {
    if max {
        a >= b
    } else {
        a <= b
    }
}

/// The value [`Instr::Bin`] computes for `op` — the same expression, operation for operation,
/// so a folded constant is the value the instruction would have produced.
fn bin_value(op: Bin, sv: f64, ov: f64) -> f64 {
    match op {
        Bin::Add => sv + ov,
        Bin::Sub => sv - ov,
        Bin::Mul => sv * ov,
        Bin::Div => sv * (1.0 / ov),
        Bin::Pow => libm::pow(sv, ov),
        Bin::Hypot => libm::hypot(sv, ov),
        Bin::Atan2 => libm::atan2(sv, ov),
    }
}

fn zero(prog: &Program, parts: &mut [f64], dst: u32) {
    let o = prog.off[dst as usize] as usize;
    let l = prog.len[dst as usize] as usize;
    parts[o..o + l].fill(0.0);
}

fn copy(prog: &Program, vals: &mut [f64], parts: &mut [f64], dst: u32, src: u32, map: u32) {
    if dst == src {
        return;
    }
    vals[dst as usize] = vals[src as usize];
    let o = prog.off[dst as usize] as usize;
    let l = prog.len[dst as usize] as usize;
    for (k, &(ia, _)) in prog.maps[map as usize..map as usize + l].iter().enumerate() {
        parts[o + k] = get(parts, ia);
    }
}

/// `(value, derivative)` of each chain-rule function — `ad::Dual`'s own formulas.
///
/// `Neg` multiplies by `-1.0` rather than negating: that is what `Dual::neg` does (it is
/// `scale(-1.0)`), and the two differ in a NaN's sign bit.
#[allow(clippy::neg_multiply)]
fn chain_value(op: Chain, v: f64) -> (f64, f64) {
    match op {
        Chain::Neg => (v * -1.0, -1.0),
        Chain::Exp => {
            let e = libm::exp(v);
            (e, e)
        }
        Chain::Ln => (libm::log(v), 1.0 / v),
        Chain::Log10 => (libm::log10(v), 1.0 / (v * std::f64::consts::LN_10)),
        Chain::Sqrt => {
            let r = v.sqrt();
            (r, 0.5 / r)
        }
        Chain::Abs => (v.abs(), v.signum()),
        Chain::Sin => (libm::sin(v), libm::cos(v)),
        Chain::Cos => (libm::cos(v), -libm::sin(v)),
        Chain::Tan => {
            let t = libm::tan(v);
            (t, 1.0 + t * t)
        }
        Chain::Sinh => (libm::sinh(v), libm::cosh(v)),
        Chain::Cosh => (libm::cosh(v), libm::sinh(v)),
        Chain::Tanh => {
            let t = libm::tanh(v);
            (t, 1.0 - t * t)
        }
        Chain::Asin => (libm::asin(v), 1.0 / (1.0 - v * v).sqrt()),
        Chain::Acos => (libm::acos(v), -1.0 / (1.0 - v * v).sqrt()),
        Chain::Atan => (libm::atan(v), 1.0 / (1.0 + v * v)),
        Chain::Asinh => (libm::asinh(v), 1.0 / (v * v + 1.0).sqrt()),
        Chain::Acosh => (libm::acosh(v), 1.0 / (v * v - 1.0).sqrt()),
        Chain::Atanh => (libm::atanh(v), 1.0 / (1.0 - v * v)),
    }
}
