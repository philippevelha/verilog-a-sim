# Frozen Interfaces (ratified at kickoff)

These are the **v0 contracts** (CLAUDE.md §4). They are mirrored into the `va-ir` and
`va-abi` crates. Changing either is a coordinated event (§6): open an issue listing every
downstream crate, get owner agreement, and update this file together with the crate in one
PR with stub adapters so the workspace keeps compiling. **Never** silently widen or reshape
them in a feature PR — a broken contract blocks every sibling thesis at once.

> This file holds the **verbatim v0 sketches**. The full semantic specifications — meaning,
> invariants, conventions, worked examples, and evolution rules — live in
> [`bridges/`](bridges/README.md): [Bridge α](bridges/interface-alpha-ir.md) and
> [Bridge β](bridges/interface-beta-abi.md).

## Interface α — elaborated IR (`va-ir`)

Arena/index representation is mandatory (§5). Expressions and statements are stored in
`Vec`s and referenced by index types, never by `&` references or `Box` graphs.

```rust
// va-ir/src/lib.rs  (sketch — flesh out, do not restructure casually)
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)] pub struct NodeId(pub u32);
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)] pub struct ParamId(pub u32);
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)] pub struct ExprId(pub u32);
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)] pub struct BranchId(pub u32);

pub struct Module {
    pub name: String,
    pub ports: Vec<Vec<NodeId>>,  // one entry per declared port; >1 NodeId for a vector port
    pub nodes: Vec<NodeDecl>,
    pub branches: Vec<Branch>,
    pub params: Vec<Param>,
    pub exprs: Vec<Expr>,         // arena
    pub functions: Vec<Function>, // user-defined analog functions
    pub analog: Vec<Stmt>,        // top-level analog block
}

pub enum Expr {
    Const(f64),
    Param(ParamId),
    Probe(Access),                 // V(b) or I(b)
    Unary(UnOp, ExprId),
    Binary(BinOp, ExprId, ExprId),
    Call(Builtin, Vec<ExprId>),    // exp, ln, ddt, idt, $vt, $temperature, ...
    CallUser(FuncId, Vec<ExprId>), // user-defined analog function call
    Select(ExprId, ExprId, ExprId),// ternary cond ? then : else
    Ddx(ExprId, Access),           // ddx(expr, probe): partial derivative w.r.t. probe's node
    ParamGiven(ParamId),           // $param_given: resolved at the instantiation boundary
    PortConnected(u32),            // $port_connected: likewise, by port index
}

pub enum Stmt {
    Contribute { target: Access, value: ExprId },  // <+
    If { cond: ExprId, then_: Vec<Stmt>, else_: Vec<Stmt> },
    Assign { lhs: VarId, rhs: ExprId },
    Block(Vec<Stmt>),
    While { cond: ExprId, body: Vec<Stmt> },
    For { init: Box<Stmt>, cond: ExprId, step: Box<Stmt>, body: Vec<Stmt> },
    Repeat { count: ExprId, body: Vec<Stmt> },
    Case { selector: ExprId, arms: Vec<CaseArm>, default: Vec<Stmt> },
}

// CaseArm { labels: Vec<ExprId>, body: Vec<Stmt> }
// Function { name: String, args: Vec<VarId>, arg_dirs: Vec<ArgDir>, ret: VarId, body: Vec<Stmt> }
// ArgDir { Input, Output, Inout }  // LRM `input`/`output`/`inout` on a function argument
```

The shipped `va-ir` fleshes this out (adds `VarId`, `VarDecl`, `FuncId`, `Discipline`,
`AccessKind`, helper methods) without restructuring the contract.

> **Revision (§6 change, 2026-06-30):** added the analog control-flow statements (`While`,
> `For`, `Repeat`, `Case` + `CaseArm`) and user-defined analog functions (`Module.functions`,
> `Function`, `Expr::CallUser`, `FuncId`). The frontend lowers all of them; `va-codegen` v0
> still rejects them during its own lowering (stub adapters), so the workspace keeps
> compiling. The `Box<Stmt>` in `For` is a finite-size tree node, not a shared graph, so it
> respects the §5 arena rule.

> **Not a §6 change: module instantiation (Annex C.8).** `va-frontend` now supports
> `Item::Instance` (`resistor r1(p, n);`, `#(...)` overrides, named `.port(net)` connections —
> see `docs/token-reference.md` §2.1b). It does **not** appear here because it never touches
> this contract: the elaborator resolves a whole instantiation hierarchy by recursively
> elaborating each referenced submodule and inlining its arenas into the instantiating
> module's own, entirely inside `va-frontend`, before Interface α's boundary. `Module` above is
> still exactly what `va-codegen`/`va-core`/`va-abi` receive — one flat module, no hierarchy
> concept, unchanged in shape. Hierarchy is a `va-frontend`-internal concern, not an IR one.

> **Not a §6 change (at the time — see the 2026-07-09 revision below): discipline/nature
> declarations.** `discipline...enddiscipline`/`nature...endnature` blocks are now genuinely
> parsed (`docs/token-reference.md` §1.5, §2.17, §2.25) into a small `va-frontend`-internal
> table (`disciplines::{NatureDecl, DisciplineDecl}`), instead of discarded as an opaque token
> span. This didn't touch Interface α either, *as of this note*: net *declarations* still only
> accept the `electrical`/`thermal` keyword tokens (unchanged `ast::Discipline`/
> `va_ir::Discipline`), so `Module`/`NodeDecl` were exactly as before — `va_ir::Discipline::Other`
> still existed as a forward-looking placeholder, still never constructed. The only real effect
> was parser-internal: a parsed discipline's bound nature `access` names widen
> `Parser::known_access` beyond the hardcoded `V`/`I`/`Temp`/`Pwr` baseline, additively. (This
> stopped being fully accurate once `NodeDecl` itself gained a field sourced from this same
> metadata — see the 2026-07-09 revision.)

> **Revision (§6 change, 2026-07-06):** added `Function::arg_dirs: Vec<ArgDir>` (`ArgDir` a new
> three-variant enum: `Input`/`Output`/`Inout`), same length and order as `Function::args`,
> recording the LRM's `input`/`output`/`inout` direction on each analog function argument —
> previously parsed by `va-frontend` (`ast::FuncArg::dir`) but discarded during elaboration,
> which bound every argument as a plain input with no way to write a result back to the caller.
> Real compact models use `output`/`inout` arguments for a function that computes several
> results at once (`mvsg_cmc_*.va`'s `calc_iq`/`calc_capt`); `va-codegen`'s `call_function`
> reads this to decide whether to bind the caller's actual-argument value in before running the
> body (`Input`/`Inout`) and/or write the parameter's final binding back into the caller's own
> variable after (`Output`/`Inout` — enforced to be a plain `Expr::Var`, per the LRM's own
> restriction on output/inout actual arguments). Additive and backward compatible: every existing
> `Function` construction site needed only `arg_dirs: vec![ArgDir::Input; args.len()]` added,
> preserving its exact prior behavior.

> **Revision (§6 change, 2026-07-09):** added `NodeDecl.abstol: Option<f64>` (§ nature-metadata
> wiring, `docs/roadmap.md` backlog item 5) — the node's discipline's **potential** nature's
> `abstol`, if a parsed `discipline...enddiscipline`/`nature...endnature` preamble resolves one
> (`va_frontend::disciplines::resolve_abstol`), else `None`. This is the change the
> "not a §6 change" note above predates: a parsed discipline's metadata now reaches `Module`
> itself, not just `Parser::known_access`. Additive and backward compatible: every existing
> `NodeDecl { name, discipline }` construction site needed only `abstol: None` added, preserving
> its exact prior behavior; `va-frontend`'s public entry points stayed source-compatible too —
> `elaborate`/`elaborate_with_library` are now thin wrappers over the new
> `elaborate_with_library_and_disciplines`, passing empty tables. `None` is `va-core`'s signal to
> fall back to its own configured default (see Interface β's matching `unknown_abstol` revision,
> below) — there is deliberately no equivalent field for a discipline's *flow* nature (e.g.
> `Current`'s `abstol`): only a `Node`-kind unknown (a KCL potential) has a natural per-`NodeDecl`
> home for one.

> **Revision (§6 change, 2026-09-05):** added
> `ModelInstance::unknown_is_junction(i) -> bool`, defaulting to `false` (§ junction limiting).
> Says whether `unknowns()[i]` sits across an exponential junction and so wants `va-core`'s
> `limit_junction` step clamp. Additive in the same shape as `unknown_kind`/`unknown_abstol`: a
> defaulted method, so no existing implementor changed.
>
> This fixed a real bug. `va-core` applied the clamp **blanket to every unknown**, including
> branch-current rows. The clamp replaces a Newton step with a logarithmically compressed one,
> which is what stops `exp(V/vt)` overflowing on the way to a diode's operating point; applied
> to an unknown with no exponential in it, it throttles a good linear step to roughly
> `vt·ln(...)` ≈ 0.2 V per iteration, so a node whose solution is 100 V needs ~475 iterations
> and `NewtonConfig::max_iters` (100) cuts it off first. A plain linear resistor divider —
> bring-up rung 1 — failed to converge above about 20 V, and a 100 A branch current was clamped
> as though it were a junction voltage. Every golden circuit runs at 5 V or less, which is why
> 25/25 stayed green over it.
>
> Opting in rather than out is the safe direction: a model that forgets to claim a junction
> converges more slowly or needs damping, whereas the previous default made every circuit
> above a few volts unsolvable. `va-abi`'s `Diode`/`Bjt` claim their terminals; a
> `va-codegen`-generated model claims its node unknowns exactly when its source contains an
> exponential (`exp`, or `limexp`, which the frontend lowers to the same `Builtin::Exp`),
> scanned once at build time. Auxiliary branch/`idt` unknowns never claim it — they carry
> flows, and clamping a current with a voltage-shaped rule is meaningless.

> **Revision (§6 change, 2026-09-05):** added `NodeDecl.access: Option<String>` and
> `NodeDecl.units: Option<String>` (§ quantity reporting) — the node's discipline's **potential**
> nature's `access` function name (`"V"`, `"Temp"`, `"Omega"`) and `units` string (`"V"`, `"K"`,
> `"rads/s"`), resolved from a parsed preamble by
> `va_frontend::disciplines::resolve_potential_nature` and `None` under exactly the conditions
> the `abstol` revision above lists. Same one-hop resolution, factored out so both take it
> together and a node can never end up with one and not the other.
>
> The motivation is that a solution vector is not necessarily electrical. `va-cli` previously
> printed every node as `V(name) = … V`, which for a mechanical or thermal node is not a
> formatting blemish but a false statement about what was computed; `va_ir::Discipline` could not
> fix it, since it collapses every custom discipline to `Other` and so cannot distinguish
> `rotational_omega` from `optical`. Additive and backward compatible: every existing `NodeDecl`
> construction site needed only `access: None, units: None` added, and a consumer that ignores
> both fields behaves exactly as before. The `Some`/`None` split carries real information —
> `None` means "no preamble reached this node", which for a deck of built-in `R`/`C`/`L`/`V`
> primitives correctly leaves the electrical spelling as the fallback.
>
> There is deliberately **no** field for the discipline's *flow* nature, for the same reason the
> `abstol` revision gives: only a `Node`-kind unknown has a natural per-`NodeDecl` home. The
> visible consequence is that `va-cli` reports a compiled model's auxiliary branch rows with a
> bare name and no unit rather than guessing — a branch row carries a flow, but an `idt`
> accumulator carries that flow's time integral, so labelling the pair alike would be
> confidently wrong about half of them.

> **Revision (§6 change, 2026-08-04):** added `Builtin::NoiseTable`, the IR spelling of
> Verilog-A's `noise_table()` (LRM §4.6.4.3). Additive in the strictest sense — one new variant
> on an existing enum, matched exhaustively in exactly two places (`va-codegen`'s `ad::eval`,
> where it evaluates to `0.0` like the other two noise builtins, and `lower::noise_term_shape`,
> which pulls it into the noise channel).
>
> The table travels as the call's **flattened, sorted, const-folded arguments** —
> `Call(NoiseTable, [Const(f1), Const(p1), Const(f2), Const(p2), …])` — rather than as a new
> `Expr` variant owning a `Vec<(f64, f64)>`. That choice is what keeps this a one-variant change:
> every arena walk, clone, validity check and pretty-printer in the pipeline already handles a
> `Call` with constant arguments, so none of them needed touching. `va-frontend` does all the
> table-shaped work once, at elaboration, where it still has a source file to name in an error
> message: it const-folds each entry, rejects an odd count, a duplicate frequency (the LRM
> requires uniqueness), a negative frequency or power, and the file-name form of the argument;
> then sorts the pairs ascending, which is the invariant `va_abi::noise::table_psd_at` reads them
> under. See Interface β's matching `table_current` revision, below.

> **Revision (§6 change, ratified 2026-09-01):** added `Builtin::Absdelay`, the IR spelling of
> `absdelay(value, delay [, maxdelay])` (LRM §4.5.9). Additive, in the same shape as the two
> revisions below. Proposal: `docs/proposals/absdelay.md`.
>
> Arguments are normalized at elaboration to `[value, delay]`; a third `maxdelay` is accepted
> and dropped, because only a time-domain implementation needs it (to size a history buffer).
>
> **Why this needed a §6 event at all.** `absdelay` used to be *folded away at elaboration*
> — lowered to its own value argument, delay discarded. That is correct at DC, where a delayed
> signal equals its input, and silently wrong everywhere else. Because the fold happened in the
> frontend, nothing survived into the IR for any later pass to implement, so putting the
> operator back is by definition an Interface α change.
>
> **No Interface β change was needed**, verified rather than assumed: the AC path already
> exists. A pure delay is the transfer function `H(jw) = exp(-jw*tau)`, so `va-codegen` stamps
> it through the identical `G = Re(H)`, `C = Im(H)/w` route the `laplace_*` family already
> uses, reading the frequency from `AnalysisCtx::freq`. That is **exact**, not a rational
> approximation of a delay: the response repeats every `1/tau` in frequency, which no
> finite-order filter reproduces.
>
> `absdelay` rides `lower::LaplaceTerm` (which gained an `Option<ExprId>` delay field) rather
> than a parallel type of its own, because it *is* a frequency-domain term: it needs the same
> stamping, the same "must be a bare top-level additive term" restriction (a complex gain has
> nowhere to live in a real-valued `Dual`), and the same "this model is frequency-dependent"
> flag.
>
> **In transient it still folds to its undelayed input**, and says so: `va-cli` warns, and
> `check` counts such files separately from plain passes. A real time-domain delay needs an
> interpolated history buffer — stage 2 of the proposal.
>
> **Revision (§6 change, 2026-08-05):** added `Builtin::NoiseTableLog`, the IR spelling of
> `noise_table_log()` (LRM §4.6.4.4) — the same table, interpolated in `log₁₀ f`/`log₁₀ power`.
> Additive in exactly the same way as the revision above, and matched in exactly the same two
> places. **No Interface β change was needed**: `TableInterp::Log` shipped with `table_current`
> on 2026-08-04 precisely so that wiring the second spelling would cost no further coordination,
> and it didn't.
>
> A separate variant rather than an interpolation flag argument: the two are separate LRM
> functions with separate spellings, and a flag would have to be encoded as a magic `Const`
> sitting among real `(frequency, power)` data — indistinguishable from a table entry to every
> generic arena walk that currently needs no special case at all. `va-codegen`'s
> `lower::NoiseTerm::Table` resolves the variant into a `TableInterp` once, at lowering, so
> nothing downstream re-inspects the call.

> **Revision (§6 change, 2026-08-06):** added `Builtin::{Abstime, Analysis, AcStim}`,
> `Stmt::BoundStep(ExprId)`, and the shared phase encoding `ANALYSIS_PHASES`/`phase_bit`/
> `phase_mask`. This is **Tier A** of `docs/proposals/analysis-context.md` — the constructs that
> are a pure function of *what analysis is running*, and nothing else. It lands together with
> Interface β's analysis-context revision below; neither half is useful alone.
>
> These four used to be **const-folded at elaboration**, correctly, back when DC was the only
> analysis this project had. Once `va-transient` and `va-acnoise` landed, each fold became a
> silent wrong answer: `analysis("tran")` was permanently `false` and `analysis("dc")`
> permanently `true`, so a DC-initialization branch fired at *every* transient timepoint while
> the transient branch never fired at all; `$abstime` was pinned to `0.0`, freezing every
> time-dependent model at t = 0; `ac_stim` folded to a bare `0.0`, discarding the magnitude and
> phase that are the entire point of it; and `bound_step` was discarded with the system tasks.
> None of this was visible in `cargo xtask validate`, because all 13 gated circuits use textbook
> devices containing no analysis-dependent construct.
>
> **The phase encoding lives here, in Interface α, and that placement is forced.** `analysis()`'s
> arguments are string literals (the LRM requires it), and `Expr::Call` carries `Vec<ExprId>` of
> numeric expressions. Rather than add a string-carrying `Expr` variant — which would pollute
> every arena walk in the pipeline for one construct — elaboration folds the argument list to a
> **bitmask over `ANALYSIS_PHASES`**, exactly the trade `Builtin::NoiseTable` makes with its
> flattened table, and for the same payoff: every existing walk, clone and validity check keeps
> working untouched. The string-shaped work happens once, at the one place that can still name a
> source file when a phase name is misspelled — and an unrecognized name is a **hard error**, not
> a mask of zero, because a mask of zero would disable whatever branch it guards forever with no
> diagnostic.
>
> The bit order therefore cannot live in `va-abi`: `va-frontend` produces the mask and may depend
> only on `va-ir` (§3). `va-abi` answers the complementary runtime question from a `&str`
> (`AnalysisKind::matches_phase`) and never needs to know which bit carries which name.
> `va-codegen` — the one crate that depends on both — joins the two ends, in exactly one
> function (`ad::phase_mask_active`). That split is what keeps the encoding defined once.
>
> `ac_stim` normalizes to **exactly three arguments** (mask, `mag`, `phase`) regardless of how
> many the source wrote, so no consumer re-derives the LRM's defaults. `bound_step` is a
> `Stmt`, not an `Expr`, because that is what it is: it produces no value and asks the simulator
> for something. It stays a *statement in the control-flow walk* rather than a module-level
> property because it may sit inside an `if` — whether a bound applies at all can depend on the
> operating point.
>
> **Explicitly not delivered** (Tiers B and C of the same proposal): `transition`, `slew`,
> `absdelay`, `$limit`, `@(initial_step)` and `idt` initial conditions still fold, because they
> need per-instance *state* across evaluations and Interface β is deliberately stateless;
> `laplace_*`/`zi_*` still fold to their DC gain, because they need per-frequency
> re-linearization. `docs/token-reference.md` says so per construct rather than implying the
> whole family is fixed.

## Interface β — model instance ABI (`va-abi`)

The project's internal "OSDI." `va-core` calls `load`; both `va-codegen`'s generated models
and `va-abi`'s hand-written reference models implement it. DC ignores the charge channel;
the transient integrator consumes it via a companion model.

```rust
// va-abi/src/stamps.rs
pub trait StampSink {
    fn residual(&mut self, row: usize, value: f64);            // current into node `row`
    fn jacobian(&mut self, row: usize, col: usize, value: f64); // dResidual[row]/dx[col]
    fn charge(&mut self, row: usize, value: f64);              // Q at `row`  (transient)
    fn dcharge(&mut self, row: usize, col: usize, value: f64); // dQ[row]/dx[col]
}

// va-abi/src/instance.rs
pub enum UnknownKind {
    Node,   // a KCL current-sum row; safe for `gmin` to shunt to ground
    Branch, // a constraint row (e.g. a source's V(p)-V(n)=value); never shunt this
}

// va-abi/src/noise.rs
pub trait NoiseSink {
    /// A white current-noise source of one-sided PSD `psd` (A²/Hz) across the branch `p`-`n`.
    fn white_current(&mut self, p: usize, n: usize, psd: f64);
    /// A flicker source across `p`-`n`, PSD `coeff / f^exponent` (A²/Hz). Default: none.
    fn flicker_current(&mut self, p: usize, n: usize, coeff: f64, exponent: f64) {}
    /// A tabulated source across `p`-`n`: `(frequency, power)` pairs, ascending, interpolated
    /// per `interp` (linear in `f` or in `log f`). Default: none.
    fn table_current(&mut self, p: usize, n: usize, points: &[(f64, f64)], interp: TableInterp) {}
}

pub trait ModelInstance {
    /// Global unknown indices this instance contributes to (nodes + internal unknowns).
    fn unknowns(&self) -> &[usize];
    /// Structural kind of `unknowns()[i]`. Default `UnknownKind::Node`.
    fn unknown_kind(&self, i: usize) -> UnknownKind { UnknownKind::Node }
    /// Per-unknown abstol override for `unknowns()[i]`. Default `None` (solver's own default).
    fn unknown_abstol(&self, i: usize) -> Option<f64> { None }
    /// Evaluate at solution vector `x`; emit residual + Jacobian (+ charge in transient).
    fn load(&self, x: &[f64], sink: &mut dyn StampSink);
    /// Emit this instance's own noise sources at `x` and temperature `temp` (K).
    /// Default: none (a noiseless element).
    fn noise(&self, x: &[f64], temp: f64, sink: &mut dyn NoiseSink) {}
    /// How many monitored-event slots this instance reports. Default 0.
    fn event_count(&self) -> usize { 0 }
    /// Report transient event registrations at an *accepted* timepoint. Default: none.
    fn events(&self, x: &[f64], ctx: &AnalysisCtx, sink: &mut dyn EventSink) {}
}

// va-abi/src/events.rs
pub enum CrossDir { Rising, Falling, Either }

pub trait EventSink {
    /// Expression `slot` currently has value `value`; watch it for a sign change in `dir`.
    fn monitor(&mut self, slot: usize, value: f64, dir: CrossDir);
    /// Ask for a solve point at absolute time `t` seconds.
    fn breakpoint(&mut self, t: f64);
}
```

`va-abi` ships **working** `resistor`, `capacitor`, and `diode` reference models against this
trait at bootstrap, so `va-core` has something real to solve on commit #1.

> **Revision (§6 change, 2026-07-04):** added `UnknownKind` and `ModelInstance::unknown_kind`,
> a **default trait method** (`docs/bridges/interface-beta-abi.md` §8's own recommendation for
> an optional addition), so every existing implementor — `va-abi::reference`'s `Resistor`/
> `Capacitor`/`Diode`, and every `va-codegen`-generated model — kept compiling unchanged.
> `va_abi::reference::VSource` overrides it for its branch-current unknown (`Branch`, everything
> else `Node`). This unblocks `va-core`'s `gmin`-stepping convergence aid
> (`crate::mna::classify_unknowns`/`System::shunt_gmin`, `NewtonConfig::gmin_steps`): it needs
> to know which rows are KCL sums (safe to shunt a conductance to ground) versus constraint
> rows like a source's `V(p) − V(n) = value` (which shunting would silently corrupt) — see
> `docs/roadmap.md`'s T3.3 for the full account of why this was previously listed as
> blocked on exactly this change.

> **Revision (§6 change, 2026-07-09):** added `ModelInstance::unknown_abstol`, another **default
> trait method** in exactly the same shape as `unknown_kind` above — every existing implementor
> kept compiling unchanged. `va-codegen`'s generated models override it, reading the matching
> `va_ir::NodeDecl::abstol` (Interface α's paired revision, above) for any of their node-kind
> unknowns; every hand-written `va-abi::reference` device (none compiled from Verilog-A, so none
> has discipline metadata) and any auxiliary (branch-current/`idt` accumulator) unknown beyond a
> generated model's own node count keep the default `None`. `va-core::mna::classify_abstol`
> collects this into a per-unknown tolerance vector (mirroring `classify_unknowns`), which
> `newton::solve_from`'s per-unknown convergence check now consults instead of always using
> `NewtonConfig::abstol` — see `docs/roadmap.md` backlog item 5 for the full account and its
> stated v1 limits (no flow-nature/branch-unknown wiring; the residual-norm gate stays global).

> **Revision (§6 change, 2026-08-01):** added the **noise channel** — a new `NoiseSink` trait
> (`va-abi/src/noise.rs`) and `ModelInstance::noise`, a third **default trait method** in the
> same additive shape as the two above, so every existing implementor kept compiling untouched.
> This unblocks T5.2's adjoint noise analysis (`va_acnoise::noise`).
>
> **Why a new channel rather than deriving noise from the Jacobian.** A device's noise is
> physics the assembled matrices no longer carry. A 200 Ω resistor and a diode biased to a 200 Ω
> small-signal resistance stamp *identical* `G` entries, but the resistor's noise is thermal
> (`4kTg`, bias-independent) and the diode's is shot (`2q|Id|`, bias-dependent) — for that pair
> they differ by exactly a factor of two, and in general by whatever the bias makes them. Only
> the instance knows which it is, the same argument `UnknownKind` rests on: invisible from a
> global index, or here from a matrix entry, because it depends on what the device *is*.
>
> Overridden by `va-abi::reference`'s `Resistor` (thermal), `Diode` and `Bjt` (shot). Kept at the
> default — genuinely correct, not a stub — by `Capacitor` and `VSource`: an ideal reactance
> dissipates nothing and passes no carriers across a barrier, so it has neither noise mechanism.
>
> **Stated limits of this v1 channel**, both deliberate and both additive to fix later:
> `white_current` is the only source kind, so flicker (`1/f`) noise has no representation (a
> `flicker_current` sibling would be the next revision of exactly this shape); and
> `va-codegen`-generated models take the default, since Verilog-A's `white_noise()`/
> `flicker_noise()` are not lowered yet — a circuit built from a compiled model therefore
> computes zero noise, which is why the noise validation circuit uses the hand-written reference
> devices. See `va_abi::noise`'s own module doc and `docs/roadmap.md`'s T5.2 section.

> **Revision (§6 change, 2026-08-01b):** added `NoiseSink::flicker_current`, closing **both**
> limits the revision immediately above stated — and it closed them together, because they were
> the same gap seen from two ends. Lowering Verilog-A's `flicker_noise()` in `va-codegen` (T1/T2)
> is pointless if the ABI can only carry white sources, and a flicker channel is untestable if no
> model can declare one.
>
> `flicker_current(p, n, coeff, exponent)` describes a source whose one-sided PSD at frequency
> `f` is `coeff / f^exponent`. The frequency dependence is carried as a **shape plus
> coefficients** rather than as a closure or a pre-evaluated number: `coeff` already includes
> whatever bias dependence the model applies (SPICE's `KF·I^AF`, evaluated at the operating
> point), and only the `f` dependence is deferred to the analysis, which evaluates
> `NoiseSource::psd_at(f)` per sweep point. That keeps the channel a plain data contract with no
> callbacks and no re-entry into the model per frequency.
>
> Another **default trait method**, so every sink written against the previous revision — and
> every model that emits only white noise — kept compiling untouched; a white-only sink simply
> drops flicker sources, and a test in `va_abi::noise` pins exactly that. The one non-additive
> ripple is internal to `va-abi`: `CollectedNoise::sources` now records a `NoiseSource` enum
> instead of a bare `f64` PSD, since a flicker source has no single PSD to record.
>
> `va-abi`'s own reference models still emit only white sources (a textbook resistor/diode/BJT
> has no flicker term to declare); the channel's users are `va-codegen`-generated models, whose
> `flicker_noise()` calls now reach it. See `docs/validation.md`'s flicker-gate section for the
> QSPICE comparison this made possible.

> **Revision (§6 change, 2026-08-04):** added `NoiseSink::table_current` and the `TableInterp`
> enum, closing the last stated limit of the noise channel — Verilog-A's `noise_table()`
> (LRM §4.6.4.3), a PSD given as interpolated `(frequency, power)` pairs. Same shape as the two
> revisions above: a **default trait method**, so every existing sink and every model that emits
> no table kept compiling untouched, and a sink that ignores tables silently drops them (pinned
> by a test in `va_abi::noise`).
>
> **Why a table is data, not two more coefficients.** White and flicker are *closed-form*
> shapes — a sink stores one or two numbers and evaluates the formula per frequency. A table
> has no formula; it is the data. So `table_current` takes the points by slice and the sink owns
> a copy, and `NoiseSource` gains a `Table { points, interp }` variant. That is the one
> non-additive ripple, again internal to `va-abi`: **`NoiseSource` is no longer `Copy`**, since a
> table owns a `Vec`. The three call sites that destructured it by value now borrow instead; no
> clone happens inside any frequency loop.
>
> `TableInterp` carries both of the LRM's interpolation rules — `Linear` (`noise_table`,
> piecewise-linear in `f`) and `Log` (`noise_table_log`, piecewise-linear in `log₁₀ f` and
> `log₁₀ power`, §4.6.4.4). Both are implemented and tested in `va_abi::noise::table_psd_at`.
> At the time of this revision only `Linear` had a Verilog-A spelling that reached it, both
> rules going in together specifically so that wiring the second would need **no further
> Interface β revision**. *(That bet paid off the next day: `noise_table_log` was lexed and
> lowered on 2026-08-05 as an Interface α change alone — see the α revision of that date.)*
>
> **Stated limits of this revision:** a table is constant data (the LRM's own restriction — an
> array parameter or an array assignment pattern), so a tabulated PSD cannot track `$temperature`
> or a netlist-overridden parameter the way a `white_noise()` argument can; correlation between
> sources is still unrepresentable, unchanged from the first revision. See `va_abi::noise`'s
> module doc, `docs/roadmap.md`'s T5.6 section, and `models/resistor_noise_table.va`'s header
> for what that means for a model author.

> **Revision (§6 change, 2026-08-06):** added the **analysis context** — a new `AnalysisCtx`
> struct (with `AnalysisKind`) threaded through **both** entry points:
>
> ```rust
> pub enum AnalysisKind { Dc, Transient, Ac, Noise }
>
> pub struct AnalysisCtx {
>     pub kind: AnalysisKind,
>     pub time: f64,   // `$abstime`; 0.0 outside transient
>     pub temp: f64,   // `$temperature`, in kelvin
> }
>
> fn load(&self, x: &[f64], ctx: &AnalysisCtx, sink: &mut dyn StampSink);
> fn noise(&self, x: &[f64], ctx: &AnalysisCtx, sink: &mut dyn NoiseSink) { }
> ```
>
> — plus two **defaulted** `StampSink` methods, `excitation(row, re, im)` and `bound_step(dt)`.
> This is Interface β's half of Tier A; see Interface α's revision of the same date for the
> problem being fixed and what is deliberately left unfixed.
>
> **This channel points the opposite way to the other three.** `UnknownKind`, `unknown_abstol`
> and the noise channel all carry what only the *instance* knows upward into the solver. This
> carries what only the *solver* knows downward into the instance. Without it a model could not
> be told what was running, which is precisely why `va-frontend` had no option but to guess at
> elaboration — and, when it was written, it guessed right.
>
> **This revision broke an existing signature rather than adding a defaulted method**, unlike the
> four before it. That was deliberate. The previous revisions were genuinely optional additions;
> this one is not. A defaulted `load_with_ctx` falling back to a context-free `load` would leave
> two ways to write a model, one of which is quietly wrong in transient — and every implementor
> seeing the context is the entire point. The blast radius was small and fully enumerated
> beforehand: six production implementors, four production call sites, and mechanical test
> updates. One PR, no adapters, no deprecation window; the trait is internal to this workspace.
>
> **There is no `freq` field, and its absence is load-bearing.** Nothing this channel serves is
> frequency-dependent — `analysis("ac")` asks *which* analysis, not at what frequency — and
> `ac::linearize` calls `load` exactly once, outside the frequency loop, because `G` and `C` are
> frequency-independent by construction. A `freq` field would be meaningless at the one call site
> that would most obviously want it, and *a field that is usually a lie is how the DC-only folds
> happened in the first place*. Frequency arrives with Tier C, alongside the per-frequency
> re-linearization that would make it true.
>
> **`temp` folded in from `noise`'s own argument.** `noise` took a bare `temp: f64`; unifying it
> here means both entry points agree on where simulation conditions live, and gives `load` a
> temperature it never had. *Compiled models do not read it yet*: `$temperature`/`$vt` still
> resolve to the temperature the model was compiled at (`ad::Ctx::temp`), unchanged, because
> re-sourcing them would move every compiled model's answer the moment a caller passes a
> non-nominal temperature — a real improvement, and a separate change.
>
> **`excitation` is `ac_stim`'s channel, and its sign convention is `residual`'s, deliberately.**
> A model writing `I(p,n) <+ ac_stim(mag, phase)` emits `+A` at `p` and `−A` at `n`, exactly as
> it would stamp any other flow contribution, and need not know which side of the equals sign its
> term lands on. Since the small-signal system is `(G + jω·C)·X = B`, the assembler moves it
> across and **negates** it — the same relationship a `VSource` already has between its
> `residual(b, vp − vn − value)` constraint row and the `+value` its AC excitation carries. A
> stimulus is a constant, not a function of `x`, which is why it needs its own channel: emitted
> through `residual` its value would be folded into `G` and double-counted.
>
> **`bound_step` is a hint, and only ever downward.** Implementors take the minimum of every
> request received; one model can tighten the step, none can loosen another's bound. Non-positive
> and non-finite requests are discarded rather than honored — the LRM gives them no meaning, and
> a zero bound would wedge the timestep controller against its own floor. Because it is emitted
> from the analog block it may sit inside an `if`, so an assembler must read it from the
> evaluation at the **accepted** point, never from a rejected candidate.
>
> **The payoff, and the evidence.** `va_transient::integrator::run_dynamic` — a near-duplicate of
> `run_with_events` that existed solely to re-box a freshly-parameterized `VSource` at every step
> *attempt*, because `load` had no time parameter — is **deleted**, along with
> `va_cli::build_instances_split` that fed it. A `SIN` source is now an ordinary stateless
> `ModelInstance` (`va_cli::WaveformSource`) reading `ctx.time`, and every device takes one path.
> That removes an allocation per timestep and roughly a hundred lines of duplicated integrator.
> The check that it was behaviour-preserving: **all 13 golden gates reproduce their previous
> numbers to the last recorded digit**, including `rectifier.net` (6.766e-4), the one gate that
> actually exercised `run_dynamic`.

> **Revision (§6 change, 2026-08-07):** added the **per-instance state channel** — a defaulted
> `ModelInstance::state_len() -> usize` and a `ModelState` threaded through `load`:
>
> ```rust
> fn state_len(&self) -> usize { 0 }
> fn load(&self, x: &[f64], ctx: &AnalysisCtx, state: &mut ModelState, sink: &mut dyn StampSink);
> ```
>
> plus `AnalysisCtx::is_initial_step`. This is **Tier B** of `docs/proposals/model-state.md`,
> which is itself the deferred half of the analysis-context proposal. It is what lets a compiled
> model implement `transition` and `slew`.
>
> **Read-old, write-new — and this is what *preserves* `load`'s purity rather than weakening
> it.** `ModelState::get` always reads the value committed at the last **accepted** timepoint;
> `set` always writes a separate proposal buffer. A model therefore cannot observe another
> iteration's proposal, so `load` remains a pure function of `(x, ctx, committed-state)`. What
> changed is that it gained an output channel besides the sink.
>
> **The storage is solver-owned, and the alternative is worth naming.** Giving the model a
> `RefCell<Vec<f64>>` is the tempting implementation and it breaks three consumers at once:
> Newton re-enters `load` many times per timepoint (the equations would move under the solver);
> the LTE controller solves every candidate step *twice* and discards rejected ones entirely (a
> self-mutating model would commit history for a timepoint that never happened); and
> finite-difference Jacobian checks perturb `x` and re-evaluate (the check would measure the
> side effect). So the instance declares a size, the consumer allocates and slices, and only the
> consumer decides when a proposal becomes history — `committed` on accept, nothing on reject,
> and `scratch` re-seeded from `committed` before every sweep so an unwritten slot means
> "unchanged" rather than inheriting a rejected candidate's value.
>
> **`is_initial_step` is always `true` in DC/AC/noise.** A static solve is definitionally its own
> initial step, which is also what keeps `transition`/`slew` settling immediately to their input
> there — the LRM-correct steady state, and bit-for-bit the answer the old elaboration-time folds
> produced. That is why all 14 golden gates are unchanged by this revision.
>
> **Deliberately not in scope**, each for a *different* reason rather than one blanket
> "later" — the decomposition is the proposal's main content:
>
> - **`$limit`** is the corpus's most-used construct (10 files / 72 uses) and is excluded because
>   its fold is **not a wrong answer**: a converged Newton solve is a fixed point of the
>   unlimited equations, so a limiter changes the path, not the answer. Its lifetime is the
>   Newton *iterate*, not the timestep, so this channel is the wrong shape for it.
> - **`absdelay`** needs an unbounded interpolated **trajectory**, which no fixed-size state
>   vector holds. A second design; the channel is shaped not to preclude it.
> - **True event-scheduled `transition`** — approximated here via `bound_step` rather than exact
>   corner breakpoints, and `docs/token-reference.md` says so at the construct.

> **Revision (§6 change, 2026-08-07b):** added `AnalysisCtx::freq` and a defaulted
> `ModelInstance::is_frequency_dependent() -> bool`. This is **Tier C** of
> `docs/proposals/frequency-domain.md` — the last of the three tiers, and the one both earlier
> proposals called "largest".
>
> **`freq` is the field Tier A refused, and the refusal was conditional.** That revision said a
> `freq` field "would be meaningless at the one call site that would most obviously want it …
> Frequency arrives with Tier C, together with the re-linearization that makes it meaningful."
> `va_acnoise::ac::run` now re-linearizes per frequency point, so the field is honest. Both the
> Tier A revision above and `docs/bridges/interface-beta-abi.md` §7 were updated rather than
> left contradicting the code.
>
> **No complex stamp channel was needed, and that is the whole reason this tier turned out
> small.** At a single frequency `ω`, any complex admittance `H = a + jb` is *exactly*
> representable by the **real** pair `G = a`, `C = b/ω`, because the assembler forms
> `G + jω·C = a + jb`. So `jacobian`/`dcharge` already span the complex plane at a given
> frequency; all that was missing was telling the model which frequency. `ddt` is the special
> case `H(s) = s` (`a = 0`, `b = ω`, so `C += grad`), which is a reassuring consistency check on
> the general rule rather than a coincidence.
>
> **`is_frequency_dependent` is a cost switch, and it is opt-in on purpose.** When every
> instance reports `false` — every ordinary circuit — `ac::run` linearizes **once**, exactly as
> before, and the existing AC gates are bit-identical. Only a circuit containing a real filter
> pays the O(points) cost. Assuming `true` would have taxed every AC sweep in the project for a
> feature almost none of them use.
>
> **Deliberately not in scope:**
>
> - **`zi_*`** (the Z-domain family) — **zero** uses across all 150 corpus files, and it needs a
>   sampling interval, i.e. a clock this simulator does not have.
> - **Transient Laplace** — a convolution needing a state-space realization, not a stamping
>   problem. In DC and transient a filter still evaluates to `H(0)`, exactly as before, so this
>   tier makes AC right and leaves transient where it was. Stated at the construct in
>   `docs/token-reference.md`, not implied.
> - **Laplace-shaped noise** (`laplace_np(white_noise(...), …)`, one real corpus use) — the
>   filter would have to multiply a PSD, which is the noise channel's business.

> **Revision (§6 change, 2026-08-29):** added `AnalysisCtx::ddt_coeff` and
> `AnalysisCtx::ddt_prev_rate_weight`, plus the `with_ddt` builder. Purely **additive** — both
> default to `0.0` in every existing constructor, so no caller breaks and every existing
> evaluation is bit-identical.
>
> Together they let a model evaluate `ddt(q)` as a **number** consistent with the discretization
> actually being solved:
>
> ```text
> dq/dt  =  ddt_coeff * (q - q_prev)  -  ddt_prev_rate_weight * dq/dt|_prev
> ```
>
> Backward Euler supplies `(1/h, 0.0)`, trapezoidal `(2/h, 1.0)`. Both are `0.0` in DC, AC and
> noise — not a placeholder, but the correct operating-point charge rate for a static or
> small-signal solve.
>
> **Why the model cannot derive this.** `h` is not `time` minus anything a model knows; the
> adaptive controller changes it per step; and the LTE estimator solves *the same step twice with
> two different methods*, so the coefficient is a property of the solve in progress, not of the
> run. `va_transient::integrator::Companion` gained a `prev_rate_weight` field and passes both
> through `assemble`.
>
> **What this is not for.** Stamping an ordinary `ddt(q)` still goes through the charge channel,
> where the consumer supplies `d/dt` and the result stays method-independent and exact. This
> exists only for what that channel cannot express: a `ddt` read as a number, or one scaled by a
> bias-dependent coefficient, where the product rule needs the operating-point charge *rate*.
>
> Per-site history (`q_prev`, `rate_prev`) rides on the **existing** state channel — `va-codegen`
> allocates two slots per `ddt` call site via `StatefulKind::Ddt` — so read-old/write-new,
> commit-on-accept and rollback-on-reject all apply unchanged, and `load` stays a pure function
> of `(x, ctx, committed-state)`.

> **Revision (§6 change, ratified 2026-09-01):** added `AnalysisCtx::ddt_prev2_weight` and the
> `with_ddt_prev2` builder, so `Method::Gear` (variable-step BDF2) could be implemented.
> Proposal and full analysis: `docs/proposals/bdf2-interface-change.md`. Purely **additive**
> — defaults to `0.0` in every constructor, so no implementor breaks and every existing
> evaluation is bit-identical (checked: all 24 gates reproduced their previous numbers exactly).
>
> The reconstruction becomes:
>
> ```text
> dq/dt  =  ddt_coeff * (q - q_prev)
>          -  ddt_prev_rate_weight * dq/dt|_prev
>          +  ddt_prev2_weight     * (q_prev - q_prev2)
> ```
>
> Backward Euler supplies `(1/h, 0, 0)`, trapezoidal `(2/h, 1, 0)`, Gear
> `((1+2r)/((1+r)h), 0, -r^2/((1+r)h))` with `r = h_n/h_(n-1)`.
>
> **Why a third field was genuinely necessary, rather than arithmetic on the existing two.**
> Backward Euler and trapezoidal share a *shape*: a one-step recursion on the previous **rate**.
> Trapezoidal's closed form `Q_n - Q_(n-1) = h/2*(rate_n + rate_prev)` *is* that recursion, which
> is why two fields sufficed for both. BDF2 is a three-point finite difference on `Q`, and it
> cannot be recovered from one charge plus one derived rate: at the previous step, `rate_(n-1)`
> was itself built under a *different* step ratio, so no algebraic identity recovers
> `Q_(n-2)`'s weight from `rate_(n-1)` alone. One new field is also *enough* — matching both
> history coefficients against BDF2's formula yields the same `w2` from either, so a fourth
> term is not needed.
>
> **The failure this exists to prevent** is silent, not loud. The *stamped* charge channel needs
> no interface change to do BDF2 (the two history charges fold into the companion's `offset`
> exactly as trapezoidal's already do). Shipping Gear without this field would therefore give an
> exact BDF2 charge stamp alongside a stale two-point reconstruction on the bare-`ddt`-value
> path — two different orders inside one model, at one row, still converging. `va-transient`'s
> `a_bias_dependent_rate_is_second_order_under_gear_on_varying_steps` is the gate for exactly
> that: with the weight forced to `0.0` the observed order collapses from **1.997 to -0.016**.
>
> `va-codegen`'s `StatefulKind::Ddt` went from 2 state slots to 3 (`q_prev`, `rate_prev`,
> `q_prev2`), which is that crate's own bookkeeping rather than a channel change: `state_len()`
> was already instance-declared. The third slot is written under *every* method so a compiled
> model stays method-agnostic and its history is already correct whenever a run reaches Gear.

> **Revision (§6 change, ratified 2026-09-06):** added the **event-registration channel** — a
> new `EventSink` trait and `CrossDir` enum (`va-abi/src/events.rs`), plus two more **default
> trait methods** on `ModelInstance`, `event_count` and `events`. Additive in the same sense as
> the noise channel: every existing implementor keeps compiling untouched, and a model that
> reports nothing behaves exactly as before.
>
> **Why a separate channel rather than more calls on `StampSink`.** `StampSink::bound_step` is
> already a model→engine scheduling hint, so adding `cross` beside it is the obvious move. It is
> wrong on three counts, and they are the same three that gave noise its own channel in T5.2:
>
> 1. **Cadence.** `load` runs once per Newton iteration and again for every *rejected* timestep.
>    A registration is meaningful once per **accepted** timepoint — it is a statement about the
>    trajectory, and a rejected candidate is not on the trajectory. `bound_step` gets away with
>    riding `load` only because it is idempotent and identity-free ("no longer than `dt`",
>    minimum-of-all, no bookkeeping).
> 2. **Identity.** A crossing is detected by comparing *this* accepted value of an expression
>    against *the previous* accepted value of the same expression. That needs a stable per-site
>    slot, which `bound_step`'s shape has nowhere to put.
> 3. **Purity.** `load` must stay a pure function of `(x, ctx, committed state)`. A crossing is
>    inherently a statement about two timepoints, so the history belongs to the consumer — the
>    same read-old/write-new split the state channel uses.
>
> **What it carries, and what it deliberately does not.** Registration only: the model says what
> to watch (`monitor`) and when it wants solving (`breakpoint`), and the consumer reports what
> fired (`va_transient::Waveform::model_crossings`). It does **not** yet carry *notification*
> back into `load`, so the body of an `@(cross(...))` statement still cannot be run at the
> firing timepoint — which is why `va-frontend` continues to refuse that construct (v0.9.1)
> rather than accept it and mis-run it. Notification is the next step and needs a per-instance
> "these fired" input alongside `state`, not a change to this channel.
>
> The value reported is the expression already reduced to distance-from-threshold, so
> `cross(V(out) - 2.5, +1)` reports `V(out) - 2.5` and a crossing is simply a change of sign;
> the consumer needs no separate threshold. Landing exactly on zero counts as arrival, so a
> signal settling at the threshold fires once rather than twice. The crossing *time* is linearly
> interpolated between the two bracketing accepted points rather than re-solved there — the same
> honest simplification the consumer-supplied watches already document, and sound for the same
> reason: the LTE control that bounds the state's error between two accepted points bounds the
> interpolation error too.

> **Revision (§6 change, ratified 2026-09-06b):** added the event channel's **notification**
> half — `ModelState::with_events` and `ModelState::event_fired(slot)`. This is what makes an
> `@(cross(...))` body actually run: the consumer determines which of an instance's monitored
> events fired at the timepoint being solved, and the model reads that back to gate the body.
>
> **Why it rides `ModelState` rather than a fifth `load` parameter.** It is the same kind of
> thing `prev`/`next` already are — a per-instance, consumer-owned view of *this* evaluation,
> constructed at the same site with the same lifetime. Adding a `load` argument would have the
> wider blast radius and buy nothing. Note this is the opposite call to the one made for
> `AnalysisCtx`, deliberately: the analysis context justified changing `load`'s signature
> because a model that ignored it was *quietly wrong in transient*, so every implementor had to
> see it. A model with no events has nothing here to ignore.
>
> It does **not** weaken `load`'s purity. Which events fired is an input the consumer fixes
> before the evaluation and holds constant across every Newton iteration of that timepoint,
> exactly like `prev` — so `load` stays a pure function of
> `(x, ctx, committed state, fired events)`.
>
> **The consumer's obligation**, met by `va_transient::integrator`: poll the registration half
> at an accepted timepoint, decide which slots fired, and then **re-solve that same timepoint**
> with the firings set, because the body changes the equations and the accepted solution must be
> the one solved with it. Clear the flags afterwards — an event fires *at* a timepoint, not
> continuously.
>
> **Stated limitation.** The body runs at the accepted timepoint that *ends* the bracketing
> step, not at the interpolated crossing time inside it. A model needing the crossing resolved
> more tightly controls the step the way any model does — `bound_step`, or a breakpoint through
> the registration half.

> **Revision (§6 change, ratified 2026-09-06c):** added `EventSink::timer(slot, next)`, a
> **default method** forwarding to `breakpoint`. A breakpoint only asks the solver to stop
> somewhere; a timer firing must also say *which* of the instance's events fires when it does,
> so the consumer can report it back through `ModelState::event_fired` and the body can run.
> The default keeps every existing implementor working — a consumer that only places timepoints
> still lands on the right one and merely cannot attribute the firing.
>
> `next` is **strictly after** the current evaluation time, which is what lets a periodic timer
> be re-registered at every accepted timepoint: the model reports its next occurrence as a pure
> function of `(start, period, now)` and needs no memory of its own schedule. Having just fired
> at `now`, it must report the *following* occurrence or the consumer would fire it forever.
> Consequence, stated rather than hidden: an occurrence falling exactly on the run's initial
> timepoint is not delivered, because that point is a seed rather than a solved step.
>
> Interface α's `Module::cross_sites` became `event_sites: Vec<EventSite>` in the same change
> (`EventSite::Cross`/`Timer`), and `Expr::CrossFired` became `Expr::EventFired`. One flat slot
> space across event kinds, so the consumer's fired-flag buffer has a single source of truth
> and does not care which kind claimed a slot.

> **Revision (§6 change, ratified 2026-09-06d):** `EventSink::monitor` gained a `CrossTol`
> argument (`{ time, expr }`, both `Option<f64>`), carrying Verilog-A's `time_tol`/`expr_tol`.
>
> A **signature change** rather than a defaulted companion method, and for the reason the
> analysis context was one: a consumer that ignores a tolerance is *silently failing to honour
> a request the source made*. There is no correct way to drop it, so no implementor should be
> able to miss it. Two in-tree implementors were affected.
>
> `va-transient` honours it with a **bracketing retry**: a candidate step that reveals a
> crossing it has overshot by more than the tolerance is rejected and re-taken aiming just past
> the interpolated crossing, reusing the LTE controller's own rejection path. Aiming *past* it
> rather than at it is deliberate — the LRM requires the event to fire after the crossing, and
> landing a hair before would hide it. A site that requested no tolerance never brackets, so
> every pre-existing run is bit-identical.
>
> What cannot be delivered is reported, not hidden: `Waveform::unresolved_events` counts
> crossings that fired without meeting their request, which happens when the tolerance is finer
> than f64 spacing at that time or the retry cap is reached. The LRM permits ignoring a
> tolerance below the tool's time precision; it does not permit ignoring one in silence.

> **Revision (§6 change, ratified 2026-09-06e):** `EventSink::monitor`'s `dir`/`tol` arguments
> were folded into a single `CrossSpec { dir, tol, at_initialization }`, and `FiredEvents` — the
> consumer's per-instance fired-flag buffer — moved into `va-abi` so both consumers share one
> shape.
>
> The spec struct is a response to churn: `monitor` took `dir` at v0.9.2, `tol` at v0.9.6, and
> `at_initialization` would have been a third widening in four versions. A struct absorbs the
> next one without touching any implementor.
>
> `at_initialization` is what makes `above` more than a one-sided `cross`. Per LRM §5.10.2 it
> "also triggers during initialization or dc", and that is the whole reason the construct
> exists: a signal already past its threshold never crosses it, so a `cross` on it never fires
> at all. Honouring it meant a **static solve had to grow events**, which it had none of —
> hence `va_core::mna::assemble_with_events`, `newton::solve_with_events` and
> `dc::operating_point_with_events`, all additive (the existing entry points pass an empty set
> and are unchanged). The DC path is two-phase and iterates to a fixed point, because which
> events fire is a property of the solution while the bodies they guard change the equations
> that produce it.

> **Revision (§6 change, ratified 2026-09-07):** added `AnalysisCtx::is_final_step` and the
> `with_final_step` builder, so `@(final_step)` could be implemented (v0.9.8). Purely
> **additive**, and the exact mirror of `is_initial_step`: a zero-argument piece of solver
> knowledge, no state, no gradient.
>
> **It is `true` in DC, AC and noise**, for the same reason `is_initial_step` is — a static solve
> is definitionally its own final step, having no later timepoint to be followed by. A swept DC
> therefore reports `true` at every point, because each point is its own static solve rather
> than a step within one; that is the rule `is_initial_step` has always followed, stated here
> because it is the one place the two flags surprise a reader.
>
> **The asymmetry is entirely on the consumer's side, and it is the whole content of this
> change.** A driver knows its first evaluation before making it; it knows its last one only
> after the run has ended. So a transient consumer cannot simply set the flag on the post-accept
> evaluation and record the pre-body solution: an `@(final_step)` body may write a variable that
> feeds a contribution, and the recorded point would then be one at which the model's own
> statements do not hold. `va_transient::integrator` therefore **re-solves** the last accepted
> timepoint with the flag set, exactly as it re-solves a timepoint where a `cross` fired.
>
> **What keeps that from costing every run.** The re-solve is *probed*, not assumed: the
> integrator assembles the last accepted point twice, once with the flag and once without, and
> only re-solves if the stamps actually differ. `newton_step` applies its update before testing
> convergence, so an unconditional re-solve would shift the final point of every existing run by
> a step's worth of round-off in exchange for nothing. Measured: all 27 gates reproduce their
> previous numbers exactly.
