//! **The static program compiled to machine code** (prototype, 2.18.0;
//! `docs/proposals/snn-transistor-performance.md` §4.2) — Cranelift, pure Rust.
//!
//! # Why
//!
//! After 2.17.0 the interpreter loop ([`super::exec`]) was still about half of a PSP103 `load()`:
//! every instruction is a `match`, every partial an index load from the operand map, and the
//! dispatch branch mispredicts. Everything those lookups find is fixed when the program is
//! built — register offsets, maps, which operand partials are absent — so this module turns a
//! [`Program`] into **one function** in which each instruction is straight-line code over fixed
//! addresses: a partial is a load, a flop or two and a store.
//!
//! # Bit-identical by construction
//!
//! Each instruction emits the arithmetic [`super::exec`] performs, operation for operation and in
//! the same order: no fused multiply-add (Cranelift never fuses on its own), IEEE `sqrt`, and the
//! transcendental functions called from the same `libm` functions, through small `extern "C"`
//! shims ([`pow`], [`log`], …). What would take more than a few lines to restate — `Instr::Flat2`
//! and `Instr::Flat1` (value-only operators, `ad`'s semantics), the rarer chain-rule functions,
//! and `Instr::Tree` (a call into the tree walker) — calls back into the very Rust function the
//! interpreter uses. `unless_both_zero` is the same mask in integer arithmetic (no branch), giving
//! `+0.0` where the interpreter's gives `+0.0`. Where an operand's partials lie next to each other
//! (most of them), two are done at once as an `f64x2`: the same IEEE operations lane by lane.
//!
//! `cargo xtask jit-check` compares every stamp against the interpreter, bit for bit, except
//! that it counts any two NaNs as equal: IEEE 754 leaves a NaN result's sign unspecified, and LLVM
//! compiles the interpreter's `v * -1.0` as a sign flip where this code multiplies, so the two can
//! produce NaNs of opposite sign (seen on `psp104`). The solver rejects a NaN whatever its bits.
//!
//! # Shape
//!
//! `fn(vals, parts, tv, env, start, end) -> u32`. Each instruction is a block that first returns
//! if its index is `end`; entry is a jump table on `start`. So one function serves every range
//! the program is ever run over (a root, a stretch), and jumps inside it are ordinary branches.
//! `tv` holds the terminal voltages for `Instr::Probe` (`x` at each local slot, `0.0` where the
//! slot has no row). The return value is 0, or 1 if a tree-walked call failed (its error is in
//! `env`).
//!
//! # `unsafe`, and why it is sound here
//!
//! This is the one module of `va-codegen` that uses `unsafe` (owner sign-off, 2026-10-08,
//! CLAUDE.md §5). Three things need it:
//!
//! 1. **Calling the generated function** ([`Jit::run`]). The function was generated from a
//!    `Program` whose every register offset and map index was checked against the register
//!    file's length when it was compiled ([`Jit::compile`] refuses otherwise), it is only ever
//!    called with that program's own register file (whose lengths are checked again before each
//!    call), and it touches no memory but `vals`, `parts`, `tv` and its own stack slots.
//! 2. **Reading back through the pointers the shims receive.** [`tree`] rebuilds the register
//!    slices from the raw pointers and lengths [`Jit::run`] placed in [`Env`], while the
//!    generated code — the only other user of that memory — is suspended inside the call. The
//!    tables [`flat2`], [`flat1`] and [`chain`] read are boxed slices owned by the [`Jit`],
//!    which outlives every call.
//! 3. **`Send`/`Sync` for [`Jit`].** The `JITModule` holds raw pointers to its code memory, so it
//!    is neither. After [`Jit::compile`] finalizes it, that memory is executable and never
//!    written again, and the function keeps no state of its own: every call works only on the
//!    buffers it is given. Sharing it between threads is then sharing an immutable function.
//!
//! **Limitations:** a prototype, off unless `VA_JIT=on` (`crate::jit_enabled`). Compiled once per
//! program — once per distinct model card — at its first evaluation: ~0.9 s for PSP103 with the
//! SG13G2 card, mostly register allocation over one ~6 000-instruction function (~1 MB of code).
//! Never freed (the module lives as long as the process). Only the host ISA; tested on x86-64
//! Linux only. If the host is not supported or the program does not compile, the interpreter is
//! used, and nothing else changes.

#![allow(unsafe_code)]

use super::{
    chain_value, exec_slices, flat1_value, flat2_value, Bin, Chain, Flat1, Instr, Program, NONE,
    ZERO,
};
use crate::ad::Ctx;
use crate::CodegenError;
use cranelift_codegen::ir::condcodes::FloatCC;
use cranelift_codegen::ir::{
    types, AbiParam, InstBuilder, MemFlags, Signature, StackSlotData, StackSlotKind, Value,
};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{default_libcall_names, Linkage, Module};
use va_ir::BinOp;

/// The generated function's type.
type Entry = unsafe extern "C" fn(*mut f64, *mut f64, *const f64, *mut Env, u32, u32) -> u32;

/// A [`Program`] compiled to machine code.
pub(crate) struct Jit {
    /// Kept alive for its code memory; never dropped before the process ends (see the module
    /// doc's limitations).
    _module: JITModule,
    entry: Entry,
    /// The operators the shims read through the pointers baked into the code.
    _flat2: Box<[BinOp]>,
    _flat1: Box<[Flat1]>,
    _chain: Box<[Chain]>,
    /// The register-file lengths the code was generated for.
    n_vals: usize,
    n_parts: usize,
    /// `tv`'s length: the program's local slots.
    n_slots: usize,
}

// SAFETY: see the module doc, point 3 — finalized code memory is immutable and the function is
// stateless; the tables it reads are immutable boxed slices.
unsafe impl Send for Jit {}
// SAFETY: as for `Send`.
unsafe impl Sync for Jit {}

impl std::fmt::Debug for Jit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Jit")
            .field("n_vals", &self.n_vals)
            .field("n_parts", &self.n_parts)
            .finish_non_exhaustive()
    }
}

/// What a callback needs that the generated code does not carry: the program, the register
/// file (as the raw parts the code itself was given), the context, and where to leave an error.
pub(crate) struct Env {
    prog: *const Program,
    vals: *mut f64,
    n_vals: usize,
    parts: *mut f64,
    n_parts: usize,
    /// The evaluation's context, its lifetime erased: only dereferenced inside a call that
    /// [`Jit::run`] — which holds the real borrow — is waiting on.
    ctx: *const Ctx<'static>,
    err: Option<CodegenError>,
}

impl Jit {
    /// Compile `prog`, or `None` if the host is not supported or Cranelift refuses the code.
    pub(crate) fn compile(prog: &Program) -> Option<Jit> {
        let n_vals = prog.init_vals.len();
        let n_parts = prog.init_parts.len();
        // Every index the code will use must lie inside the register file it is built for.
        let in_vals = |r: u32| (r as usize) < n_vals;
        let in_parts = |r: u32| {
            let o = prog.off.get(r as usize).copied().unwrap_or(u32::MAX) as usize;
            let l = prog.len.get(r as usize).copied().unwrap_or(0) as usize;
            o.saturating_add(l) <= n_parts
        };
        let maps_ok = prog
            .maps
            .iter()
            .all(|&(a, b)| (a as usize) < n_parts && (b as usize) < n_parts);
        if !maps_ok
            || !prog
                .code
                .iter()
                .all(|i| instr_ok(i, &in_vals, &in_parts, prog))
        {
            return None;
        }

        let mut flags = settings::builder();
        flags.set("opt_level", "speed").ok()?;
        flags.set("use_colocated_libcalls", "false").ok()?;
        flags.set("is_pic", "false").ok()?;
        let isa = cranelift_native::builder()
            .ok()?
            .finish(settings::Flags::new(flags))
            .ok()?;
        let mut module = JITModule::new(JITBuilder::with_isa(isa, default_libcall_names()));
        let ptr = module.target_config().pointer_type();

        let mut sig = module.make_signature();
        for _ in 0..4 {
            sig.params.push(AbiParam::new(ptr));
        }
        sig.params.push(AbiParam::new(types::I32));
        sig.params.push(AbiParam::new(types::I32));
        sig.returns.push(AbiParam::new(types::I32));
        let id = module
            .declare_function("va_static_program", Linkage::Local, &sig)
            .ok()?;

        // The shims' operator tables, reserved to their final length: the code holds addresses
        // into them, so they must never reallocate (and `into_boxed_slice` of a full `Vec` keeps
        // its buffer).
        let count = |f: fn(&Instr) -> bool| prog.code.iter().filter(|i| f(i)).count();
        let mut tables = Tables {
            flat2: Vec::with_capacity(count(|i| matches!(i, Instr::Flat2 { .. }))),
            flat1: Vec::with_capacity(count(|i| matches!(i, Instr::Flat1 { .. }))),
            chain: Vec::with_capacity(count(
                |i| matches!(i, Instr::Chain { op, .. } if !inline_chain(*op)),
            )),
        };
        let mut ctx = module.make_context();
        ctx.func.signature = sig;
        {
            let mut fctx = FunctionBuilderContext::new();
            let b = FunctionBuilder::new(&mut ctx.func, &mut fctx);
            Emitter::new(b, &module, prog, &mut tables).emit();
        }
        if tables.flat2.len() != tables.flat2.capacity()
            || tables.flat1.len() != tables.flat1.capacity()
            || tables.chain.len() != tables.chain.capacity()
        {
            return None;
        }
        define(&mut module, id, &mut ctx)?;
        module.finalize_definitions().ok()?;
        let code = module.get_finalized_function(id);
        // SAFETY: `code` is the finalized function declared with `sig`, which is `Entry`'s ABI
        // (four pointers, two `u32`s, a `u32` result, the platform's C calling convention).
        let entry: Entry = unsafe { std::mem::transmute::<*const u8, Entry>(code) };
        Some(Jit {
            _module: module,
            entry,
            _flat2: tables.flat2.into_boxed_slice(),
            _flat1: tables.flat1.into_boxed_slice(),
            _chain: tables.chain.into_boxed_slice(),
            n_vals,
            n_parts,
            n_slots: prog.n_slots,
        })
    }

    /// Run `code[start..end]` — what [`super::exec`] does, in machine code. `tv` is the
    /// terminal voltages ([`super::Run::new`] gathers them as the interpreter's `Instr::Probe`
    /// finds them: `x` through each local slot's terminal, `0.0` where there is none).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn run(
        &self,
        prog: &Program,
        vals: &mut [f64],
        parts: &mut [f64],
        tv: &[f64],
        ctx: &Ctx,
        start: u32,
        end: u32,
    ) -> Result<(), CodegenError> {
        if vals.len() != self.n_vals
            || parts.len() != self.n_parts
            || tv.len() != self.n_slots
            || start as usize > prog.code.len()
            || end as usize > prog.code.len()
        {
            return Err(crate::ad::internal_error(
                "jit: register file or range does not match the compiled program",
            ));
        }
        let mut env = Env {
            prog,
            vals: vals.as_mut_ptr(),
            n_vals: vals.len(),
            parts: parts.as_mut_ptr(),
            n_parts: parts.len(),
            ctx: (ctx as *const Ctx<'_>).cast::<Ctx<'static>>(),
            err: None,
        };
        // SAFETY: module doc, point 1 — the code was generated for this program, the register
        // file's lengths were just checked against the ones it was generated for, and `tv` holds
        // `n_slots` values. `vals`/`parts` are exclusively borrowed for the call.
        let status =
            unsafe { (self.entry)(env.vals, env.parts, tv.as_ptr(), &mut env, start, end) };
        match (status, env.err.take()) {
            (0, _) => Ok(()),
            (_, Some(e)) => Err(e),
            (_, None) => Err(crate::ad::internal_error("jit: failed without an error")),
        }
    }
}

/// Every register and partial index `i` will touch lies inside the register file.
fn instr_ok(
    i: &Instr,
    in_vals: &impl Fn(u32) -> bool,
    in_parts: &impl Fn(u32) -> bool,
    prog: &Program,
) -> bool {
    let reg = |r: u32| in_vals(r) && in_parts(r);
    let map = |m: u32, dst: u32| {
        let l = prog.len.get(dst as usize).copied().unwrap_or(0) as usize;
        (m as usize).saturating_add(l) <= prog.maps.len()
    };
    let to = |t: u32| (t as usize) <= prog.code.len();
    match *i {
        Instr::Probe { dst, .. } => reg(dst),
        Instr::Bin {
            dst, a, b, map: m, ..
        } => reg(dst) && reg(a) && reg(b) && map(m, dst),
        Instr::Chain { dst, a, map: m, .. } => reg(dst) && reg(a) && map(m, dst),
        Instr::Flat2 { dst, a, b, .. } => reg(dst) && reg(a) && reg(b),
        Instr::Flat1 { dst, a, .. } => reg(dst) && reg(a),
        Instr::Pick {
            dst,
            a,
            b,
            map_a,
            map_b,
            ..
        } => reg(dst) && reg(a) && reg(b) && map(map_a, dst) && map(map_b, dst),
        Instr::Copy { dst, src, map: m } => reg(dst) && reg(src) && map(m, dst),
        Instr::Ddx { dst, idx } => reg(dst) && (idx as usize) < prog.init_parts.len(),
        Instr::Tree { dst, .. } => reg(dst),
        Instr::JumpIfZero { cond, to: t } => reg(cond) && to(t),
        Instr::Jump { to: t } => to(t),
    }
}

fn define(module: &mut JITModule, id: cranelift_module::FuncId, ctx: &mut Context) -> Option<()> {
    module.define_function(id, ctx).ok()
}

/// Whether partials `i` and `j` of an operand (absolute indices, `ZERO` for none) can be read as
/// one packed pair: both absent, or `j` right after `i`.
fn pairs(i: u32, j: u32) -> bool {
    (i == ZERO && j == ZERO) || (i != ZERO && j != ZERO && j == i + 1)
}

/// Whether [`Emitter::chain`] emits chain-rule function `op` inline (no table entry).
fn inline_chain(op: Chain) -> bool {
    matches!(op, Chain::Neg | Chain::Exp | Chain::Ln | Chain::Sqrt)
}

/// The operators the code passes to the shims by address.
#[derive(Default)]
struct Tables {
    flat2: Vec<BinOp>,
    flat1: Vec<Flat1>,
    chain: Vec<Chain>,
}

/// Emits one program's function.
struct Emitter<'a, 'b> {
    b: FunctionBuilder<'a>,
    prog: &'b Program,
    ptr: types::Type,
    vals: Value,
    parts: Value,
    tv: Value,
    env: Value,
    blocks: Vec<cranelift_codegen::ir::Block>,
    tables: &'b mut Tables,
    sig_f1: cranelift_codegen::ir::SigRef,
    sig_f2: cranelift_codegen::ir::SigRef,
    sig_op1: cranelift_codegen::ir::SigRef,
    sig_op2: cranelift_codegen::ir::SigRef,
    sig_chain: cranelift_codegen::ir::SigRef,
    sig_tree: cranelift_codegen::ir::SigRef,
}

const FLAGS: MemFlags = MemFlags::trusted();

impl<'a, 'b> Emitter<'a, 'b> {
    fn new(
        mut b: FunctionBuilder<'a>,
        module: &JITModule,
        prog: &'b Program,
        tables: &'b mut Tables,
    ) -> Self {
        let ptr = module.target_config().pointer_type();
        let cc = module.target_config().default_call_conv;
        let mk = |params: &[types::Type], ret: types::Type| {
            let mut s = Signature::new(cc);
            s.params.extend(params.iter().map(|t| AbiParam::new(*t)));
            s.returns.push(AbiParam::new(ret));
            s
        };
        let sig_f1 = b.import_signature(mk(&[types::F64], types::F64));
        let sig_f2 = b.import_signature(mk(&[types::F64, types::F64], types::F64));
        let sig_op1 = b.import_signature(mk(&[ptr, types::F64], types::F64));
        let sig_op2 = b.import_signature(mk(&[ptr, types::F64, types::F64], types::F64));
        let sig_chain = b.import_signature(mk(&[ptr, types::F64, ptr], types::F64));
        let sig_tree = b.import_signature(mk(&[ptr, types::I32], types::I32));

        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        let params = b.block_params(entry).to_vec();
        let n = prog.code.len();
        // One block per instruction, plus the end.
        let blocks: Vec<_> = (0..=n).map(|_| b.create_block()).collect();
        Emitter {
            b,
            prog,
            ptr,
            vals: params[0],
            parts: params[1],
            tv: params[2],
            env: params[3],
            blocks,
            tables,
            sig_f1,
            sig_f2,
            sig_op1,
            sig_op2,
            sig_chain,
            sig_tree,
        }
    }

    fn emit(mut self) {
        let n = self.prog.code.len();
        let entry = self.b.current_block().expect("entry block");
        let start = self.b.block_params(entry)[4];
        let end = self.b.block_params(entry)[5];
        // Entry: jump to instruction `start` (out of range: straight to the end).
        let targets: Vec<_> = self
            .blocks
            .iter()
            .map(|blk| self.b.func.dfg.block_call(*blk, &[]))
            .collect();
        let default = targets[n];
        let jt = self
            .b
            .create_jump_table(cranelift_codegen::ir::JumpTableData::new(default, &targets));
        self.b.ins().br_table(start, jt);

        let ret = self.blocks[n];
        let fail = self.b.create_block();
        // Where a range can end: a root's code, a statement's, a stretch's. Only there does an
        // instruction check whether it is the end; elsewhere it runs straight on.
        let mut ends = vec![false; n + 1];
        for blk in &self.prog.blocks {
            for e in [blk.end, blk.stmt_end, blk.run_end] {
                if let Some(x) = ends.get_mut(e as usize) {
                    *x = true;
                }
            }
        }
        for (pc, &at) in ends.iter().enumerate().take(n) {
            let blk = self.blocks[pc];
            self.b.switch_to_block(blk);
            if at {
                // Stop here if this is the end of the range.
                let at_end = self.b.ins().icmp_imm(
                    cranelift_codegen::ir::condcodes::IntCC::Equal,
                    end,
                    pc as i64,
                );
                let body = self.b.create_block();
                self.b.ins().brif(at_end, ret, &[], body, &[]);
                self.b.switch_to_block(body);
            }
            self.instr(pc, fail);
        }
        self.b.switch_to_block(ret);
        let zero = self.b.ins().iconst(types::I32, 0);
        self.b.ins().return_(&[zero]);
        self.b.switch_to_block(fail);
        let one = self.b.ins().iconst(types::I32, 1);
        self.b.ins().return_(&[one]);
        self.b.seal_all_blocks();
        self.b.finalize();
    }

    fn f(&mut self, v: f64) -> Value {
        self.b.ins().f64const(v)
    }

    fn val(&mut self, r: u32) -> Value {
        self.b
            .ins()
            .load(types::F64, FLAGS, self.vals, (r as i32) * 8)
    }

    fn set_val(&mut self, r: u32, v: Value) {
        self.b.ins().store(FLAGS, v, self.vals, (r as i32) * 8);
    }

    /// A partial by absolute index; the zero cell is the constant it holds.
    fn part(&mut self, i: u32) -> Value {
        if i == ZERO {
            self.f(0.0)
        } else {
            self.b
                .ins()
                .load(types::F64, FLAGS, self.parts, (i as i32) * 8)
        }
    }

    fn set_part(&mut self, i: usize, v: Value) {
        self.b.ins().store(FLAGS, v, self.parts, (i as i32) * 8);
    }

    fn off_len(&self, r: u32) -> (usize, usize) {
        (
            self.prog.off[r as usize] as usize,
            self.prog.len[r as usize] as usize,
        )
    }

    fn zero_parts(&mut self, dst: u32) {
        let (o, l) = self.off_len(dst);
        let z = self.f(0.0);
        for k in 0..l {
            self.set_part(o + k, z);
        }
    }

    /// `super::unless_both_zero(p, q, r)`: `r`, or `+0.0` where both partials are zero —
    /// in integer arithmetic, without a branch: `x != 0.0` exactly when `x`'s bits shifted left
    /// by one (dropping the sign) are non-zero (`NaN` included), and `r`'s bits are cleared
    /// with a mask built from that, which leaves `+0.0`.
    fn unless_both_zero(
        &mut self,
        p: Value,
        p_zero: bool,
        q: Value,
        q_zero: bool,
        r: Value,
    ) -> Value {
        let bits = |e: &mut Self, v: Value| e.b.ins().bitcast(types::I64, MemFlags::new(), v);
        let any = match (p_zero, q_zero) {
            (true, true) => return self.f(0.0),
            (false, true) => bits(self, p),
            (true, false) => bits(self, q),
            (false, false) => {
                let (pb, qb) = (bits(self, p), bits(self, q));
                self.b.ins().bor(pb, qb)
            }
        };
        let mag = self.b.ins().ishl_imm(any, 1);
        let live = self
            .b
            .ins()
            .icmp_imm(cranelift_codegen::ir::condcodes::IntCC::NotEqual, mag, 0);
        let wide = self.b.ins().uextend(types::I64, live);
        let mask = self.b.ins().ineg(wide);
        let rb = bits(self, r);
        let kept = self.b.ins().band(rb, mask);
        self.b.ins().bitcast(types::F64, MemFlags::new(), kept)
    }

    fn call_addr(
        &mut self,
        sig: cranelift_codegen::ir::SigRef,
        addr: usize,
        args: &[Value],
    ) -> Value {
        let callee = self.b.ins().iconst(self.ptr, addr as i64);
        let call = self.b.ins().call_indirect(sig, callee, args);
        self.b.inst_results(call)[0]
    }

    fn call1(&mut self, f: extern "C" fn(f64) -> f64, a: Value) -> Value {
        let sig = self.sig_f1;
        self.call_addr(sig, f as usize, &[a])
    }

    fn call2(&mut self, f: extern "C" fn(f64, f64) -> f64, a: Value, b: Value) -> Value {
        let sig = self.sig_f2;
        self.call_addr(sig, f as usize, &[a, b])
    }

    /// An address the code passes to a shim: element `i` of one of the tables, which
    /// [`Jit::compile`] reserved to their final length so that they never move.
    fn table_addr<T>(&mut self, base: *const T, i: usize) -> Value {
        let addr = base as usize + i * std::mem::size_of::<T>();
        self.b.ins().iconst(self.ptr, addr as i64)
    }

    fn instr(&mut self, pc: usize, fail: cranelift_codegen::ir::Block) {
        let next = self.blocks[pc + 1];
        match self.prog.code[pc] {
            Instr::Probe { dst, p, n } => {
                let v = |e: &mut Self, k: u32| {
                    if k == NONE || k as usize >= e.prog.n_slots {
                        e.f(0.0)
                    } else {
                        e.b.ins().load(types::F64, FLAGS, e.tv, (k as i32) * 8)
                    }
                };
                let vp = v(self, p);
                let vn = v(self, n);
                let d = self.b.ins().fsub(vp, vn);
                self.set_val(dst, d);
            }
            Instr::Bin {
                op, dst, a, b, map, ..
            } => self.bin(op, dst, a, b, map),
            Instr::Chain {
                op, dst, a, map, ..
            } => self.chain(op, dst, a, map),
            Instr::Flat2 { op, dst, a, b } => {
                let i = self.tables.flat2.len();
                self.tables.flat2.push(op);
                let at = self.table_addr(self.tables.flat2.as_ptr(), i);
                let (va, vb) = (self.val(a), self.val(b));
                let sig = self.sig_op2;
                let r = self.call_addr(sig, flat2 as usize, &[at, va, vb]);
                self.set_val(dst, r);
                self.zero_parts(dst);
            }
            Instr::Flat1 { op, dst, a } => {
                let i = self.tables.flat1.len();
                self.tables.flat1.push(op);
                let at = self.table_addr(self.tables.flat1.as_ptr(), i);
                let va = self.val(a);
                let sig = self.sig_op1;
                let r = self.call_addr(sig, flat1 as usize, &[at, va]);
                self.set_val(dst, r);
                self.zero_parts(dst);
            }
            Instr::Pick {
                max,
                dst,
                a,
                b,
                map_a,
                map_b,
            } => {
                let (sv, ov) = (self.val(a), self.val(b));
                let cc = if max {
                    FloatCC::GreaterThanOrEqual
                } else {
                    FloatCC::LessThanOrEqual
                };
                let take_a = self.b.ins().fcmp(cc, sv, ov);
                let (o, l) = self.off_len(dst);
                let mut out = Vec::with_capacity(l);
                for k in 0..l {
                    let pa = self.part(self.prog.maps[map_a as usize + k].0);
                    let pb = self.part(self.prog.maps[map_b as usize + k].0);
                    out.push(self.b.ins().select(take_a, pa, pb));
                }
                let v = self.b.ins().select(take_a, sv, ov);
                self.set_val(dst, v);
                for (k, p) in out.into_iter().enumerate() {
                    self.set_part(o + k, p);
                }
            }
            Instr::Copy { dst, src, map } => {
                if dst != src {
                    let v = self.val(src);
                    self.set_val(dst, v);
                    let (o, l) = self.off_len(dst);
                    let mut k = 0;
                    while k < l {
                        let ia = self.prog.maps[map as usize + k].0;
                        if k + 1 < l && pairs(ia, self.prog.maps[map as usize + k + 1].0) {
                            let p = self.part2(ia);
                            self.set_part2(o + k, p);
                            k += 2;
                        } else {
                            let p = self.part(ia);
                            self.set_part(o + k, p);
                            k += 1;
                        }
                    }
                }
            }
            Instr::Ddx { dst, idx } => {
                let p = self.part(idx);
                self.set_val(dst, p);
                self.zero_parts(dst);
            }
            Instr::Tree { .. } => {
                let pcv = self.b.ins().iconst(types::I32, pc as i64);
                let sig = self.sig_tree;
                let callee = self.b.ins().iconst(self.ptr, tree as usize as i64);
                let call = self.b.ins().call_indirect(sig, callee, &[self.env, pcv]);
                let status = self.b.inst_results(call)[0];
                self.b.ins().brif(status, fail, &[], next, &[]);
                return;
            }
            Instr::JumpIfZero { cond, to } => {
                let v = self.val(cond);
                let z = self.f(0.0);
                let c = self.b.ins().fcmp(FloatCC::Equal, v, z);
                let target = self.blocks[to as usize];
                self.b.ins().brif(c, target, &[], next, &[]);
                return;
            }
            Instr::Jump { to } => {
                let target = self.blocks[to as usize];
                self.b.ins().jump(target, &[]);
                return;
            }
        }
        self.b.ins().jump(next, &[]);
    }

    fn bin(&mut self, op: Bin, dst: u32, a: u32, b: u32, map: u32) {
        let (sv, ov) = (self.val(a), self.val(b));
        let (o, l) = self.off_len(dst);
        // Per-op values the partial rule closes over (`caps`, read through the rule's second
        // argument so that the same rule serves a scalar partial and a packed pair), computed
        // before the partials as the interpreter does.
        type Rule<'r, 'a, 'b> =
            Box<dyn Fn(&mut Emitter<'a, 'b>, &[Value], Value, Value) -> Value + 'r>;
        let (value, caps, rule, packs): (Value, Vec<Value>, Rule<'_, 'a, 'b>, bool) = match op {
            Bin::Add => (
                self.b.ins().fadd(sv, ov),
                vec![],
                Box::new(|e: &mut Self, _: &[Value], p, q| e.b.ins().fadd(p, q)),
                true,
            ),
            Bin::Sub => (
                self.b.ins().fsub(sv, ov),
                vec![],
                Box::new(|e: &mut Self, _: &[Value], p, q| e.b.ins().fsub(p, q)),
                true,
            ),
            Bin::Mul => (
                self.b.ins().fmul(sv, ov),
                vec![sv, ov],
                Box::new(|e: &mut Self, c: &[Value], p, q| {
                    let x = e.b.ins().fmul(p, c[1]);
                    let y = e.b.ins().fmul(q, c[0]);
                    e.b.ins().fadd(x, y)
                }),
                true,
            ),
            Bin::Div => {
                let one = self.f(1.0);
                let inv = self.b.ins().fdiv(one, ov);
                let inv2 = self.b.ins().fmul(inv, inv);
                (
                    self.b.ins().fmul(sv, inv),
                    vec![sv, ov, inv2],
                    Box::new(|e: &mut Self, c: &[Value], p, q| {
                        let x = e.b.ins().fmul(p, c[1]);
                        let y = e.b.ins().fmul(c[0], q);
                        let d = e.b.ins().fsub(x, y);
                        e.b.ins().fmul(d, c[2])
                    }),
                    true,
                )
            }
            Bin::Pow => {
                let value = self.call2(pow, sv, ov);
                let one = self.f(1.0);
                let vm1 = self.b.ins().fsub(ov, one);
                let pw = self.call2(pow, sv, vm1);
                let d_du = self.b.ins().fmul(ov, pw);
                // `exp_varies`: any partial of the exponent's own non-zero.
                let (bo, bl) = self.off_len(b);
                let z = self.f(0.0);
                let d_dv = if bl == 0 {
                    z
                } else {
                    let mut varies = None;
                    for k in 0..bl {
                        let g = self.part((bo + k) as u32);
                        let nz = self.b.ins().fcmp(FloatCC::NotEqual, g, z);
                        varies = Some(match varies {
                            None => nz,
                            Some(acc) => self.b.ins().bor(acc, nz),
                        });
                    }
                    let lg = self.call1(log, sv);
                    let dv = self.b.ins().fmul(value, lg);
                    let varies = varies.expect("bl > 0");
                    self.b.ins().select(varies, dv, z)
                };
                (
                    value,
                    vec![d_du, d_dv],
                    Box::new(|e: &mut Self, c: &[Value], sg, eg| {
                        let z = e.f(0.0);
                        let bx = e.b.ins().fmul(c[0], sg);
                        let sz = e.b.ins().fcmp(FloatCC::Equal, sg, z);
                        let base = e.b.ins().select(sz, z, bx);
                        let ex = e.b.ins().fmul(c[1], eg);
                        let ez = e.b.ins().fcmp(FloatCC::Equal, eg, z);
                        let expo = e.b.ins().select(ez, z, ex);
                        e.b.ins().fadd(base, expo)
                    }),
                    false,
                )
            }
            Bin::Hypot => {
                let value = self.call2(hypot, sv, ov);
                (
                    value,
                    vec![sv, ov, value],
                    Box::new(|e: &mut Self, c: &[Value], sg, og| {
                        let x = e.b.ins().fmul(c[0], sg);
                        let y = e.b.ins().fmul(c[1], og);
                        let s = e.b.ins().fadd(x, y);
                        e.b.ins().fdiv(s, c[2])
                    }),
                    false,
                )
            }
            Bin::Atan2 => {
                let a2 = self.b.ins().fmul(sv, sv);
                let b2 = self.b.ins().fmul(ov, ov);
                let denom = self.b.ins().fadd(a2, b2);
                let value = self.call2(atan2, sv, ov);
                (
                    value,
                    vec![sv, ov, denom],
                    Box::new(|e: &mut Self, c: &[Value], yg, xg| {
                        let x = e.b.ins().fmul(c[1], yg);
                        let y = e.b.ins().fmul(c[0], xg);
                        let d = e.b.ins().fsub(x, y);
                        e.b.ins().fdiv(d, c[2])
                    }),
                    false,
                )
            }
        };
        let m = &self.prog.maps[map as usize..map as usize + l];
        let mut caps_v: Option<Vec<Value>> = None;
        let mut k = 0;
        while k < l {
            let pair = packs && k + 1 < l && pairs(m[k].0, m[k + 1].0) && pairs(m[k].1, m[k + 1].1);
            if pair {
                if caps_v.is_none() {
                    caps_v = Some(
                        caps.iter()
                            .map(|c| self.b.ins().splat(types::F64X2, *c))
                            .collect(),
                    );
                }
                let cv = caps_v.clone().unwrap_or_default();
                let (ia, ib) = m[k];
                let p = self.part2(ia);
                let q = self.part2(ib);
                let r = rule(self, &cv, p, q);
                let out = self.unless_both_zero2(p, ia == ZERO, q, ib == ZERO, r);
                self.set_part2(o + k, out);
                k += 2;
            } else {
                let (ia, ib) = m[k];
                let p = self.part(ia);
                let q = self.part(ib);
                let r = rule(self, &caps, p, q);
                let out = self.unless_both_zero(p, ia == ZERO, q, ib == ZERO, r);
                self.set_part(o + k, out);
                k += 1;
            }
        }
        self.set_val(dst, value);
    }

    /// Two consecutive partials (from index `i`) as one `f64x2`; the zero cell as zeros.
    fn part2(&mut self, i: u32) -> Value {
        if i == ZERO {
            let z = self.f(0.0);
            self.b.ins().splat(types::F64X2, z)
        } else {
            self.b.ins().load(
                types::F64X2,
                MemFlags::new().with_notrap(),
                self.parts,
                (i as i32) * 8,
            )
        }
    }

    fn set_part2(&mut self, i: usize, v: Value) {
        self.b
            .ins()
            .store(MemFlags::new().with_notrap(), v, self.parts, (i as i32) * 8);
    }

    /// [`Self::unless_both_zero`] on a packed pair, lane by lane.
    fn unless_both_zero2(
        &mut self,
        p: Value,
        p_zero: bool,
        q: Value,
        q_zero: bool,
        r: Value,
    ) -> Value {
        let le = MemFlags::new().with_endianness(cranelift_codegen::ir::Endianness::Little);
        let bits = |e: &mut Self, v: Value| e.b.ins().bitcast(types::I64X2, le, v);
        let any = match (p_zero, q_zero) {
            (true, true) => {
                let z = self.f(0.0);
                return self.b.ins().splat(types::F64X2, z);
            }
            (false, true) => bits(self, p),
            (true, false) => bits(self, q),
            (false, false) => {
                let (pb, qb) = (bits(self, p), bits(self, q));
                self.b.ins().bor(pb, qb)
            }
        };
        let mag = self.b.ins().ishl_imm(any, 1);
        let zi = self.b.ins().iconst(types::I64, 0);
        let zv = self.b.ins().splat(types::I64X2, zi);
        let live = self
            .b
            .ins()
            .icmp(cranelift_codegen::ir::condcodes::IntCC::NotEqual, mag, zv);
        let rb = bits(self, r);
        let kept = self.b.ins().band(rb, live);
        self.b.ins().bitcast(types::F64X2, le, kept)
    }

    fn chain(&mut self, op: Chain, dst: u32, a: u32, map: u32) {
        let v = self.val(a);
        let (value, d) = match op {
            Chain::Neg => {
                let m1 = self.f(-1.0);
                (self.b.ins().fmul(v, m1), m1)
            }
            Chain::Exp => {
                let e = self.call1(exp, v);
                (e, e)
            }
            Chain::Ln => {
                let one = self.f(1.0);
                (self.call1(log, v), self.b.ins().fdiv(one, v))
            }
            Chain::Sqrt => {
                let r = self.b.ins().sqrt(v);
                let half = self.f(0.5);
                (r, self.b.ins().fdiv(half, r))
            }
            _ => {
                let i = self.tables.chain.len();
                self.tables.chain.push(op);
                let at = self.table_addr(self.tables.chain.as_ptr(), i);
                let slot = self.b.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    8,
                    3,
                ));
                let out = self.b.ins().stack_addr(self.ptr, slot, 0);
                let sig = self.sig_chain;
                let value = self.call_addr(sig, chain as usize, &[at, v, out]);
                let d = self.b.ins().stack_load(types::F64, slot, 0);
                (value, d)
            }
        };
        let (o, l) = self.off_len(dst);
        let m = &self.prog.maps[map as usize..map as usize + l];
        let mut dv = None;
        let mut k = 0;
        while k < l {
            let ia = m[k].0;
            if k + 1 < l && pairs(ia, m[k + 1].0) {
                let d2 = *dv.get_or_insert_with(|| self.b.ins().splat(types::F64X2, d));
                let g = self.part2(ia);
                let r = self.b.ins().fmul(g, d2);
                let out = self.unless_both_zero2(g, ia == ZERO, g, true, r);
                self.set_part2(o + k, out);
                k += 2;
            } else {
                let g = self.part(ia);
                let r = self.b.ins().fmul(g, d);
                let z = self.f(0.0);
                let out = self.unless_both_zero(g, ia == ZERO, z, true, r);
                self.set_part(o + k, out);
                k += 1;
            }
        }
        self.set_val(dst, value);
    }
}

// The shims the generated code calls: each is the function the interpreter uses.

extern "C" fn pow(a: f64, b: f64) -> f64 {
    libm::pow(a, b)
}
extern "C" fn log(a: f64) -> f64 {
    libm::log(a)
}
extern "C" fn exp(a: f64) -> f64 {
    libm::exp(a)
}
extern "C" fn hypot(a: f64, b: f64) -> f64 {
    libm::hypot(a, b)
}
extern "C" fn atan2(a: f64, b: f64) -> f64 {
    libm::atan2(a, b)
}

extern "C" fn flat2(op: *const BinOp, a: f64, b: f64) -> f64 {
    // SAFETY: module doc, point 2 — `op` points into the `Jit`'s boxed `flat2` table.
    flat2_value(unsafe { *op }, a, b)
}

extern "C" fn flat1(op: *const Flat1, a: f64) -> f64 {
    // SAFETY: as `flat2`, into the `flat1` table.
    flat1_value(unsafe { *op }, a)
}

extern "C" fn chain(op: *const Chain, v: f64, d: *mut f64) -> f64 {
    // SAFETY: as `flat2`, into the `chain` table; `d` is an 8-byte stack slot of the caller.
    let (value, deriv) = chain_value(unsafe { *op }, v);
    unsafe { *d = deriv };
    value
}

/// `Instr::Tree` at `pc`: the interpreter runs that one instruction.
extern "C" fn tree(env: *mut Env, pc: u32) -> u32 {
    // SAFETY: module doc, point 2 — `env` is `Jit::run`'s, alive for the whole call; the
    // register slices are rebuilt from the pointers and lengths the generated code was given,
    // and the code does not touch them until this returns.
    let env = unsafe { &mut *env };
    let (prog, ctx) = unsafe { (&*env.prog, &*env.ctx) };
    let vals = unsafe { std::slice::from_raw_parts_mut(env.vals, env.n_vals) };
    let parts = unsafe { std::slice::from_raw_parts_mut(env.parts, env.n_parts) };
    match exec_slices(prog, vals, parts, ctx, pc, pc + 1) {
        Ok(()) => 0,
        Err(e) => {
            env.err = Some(e);
            1
        }
    }
}
