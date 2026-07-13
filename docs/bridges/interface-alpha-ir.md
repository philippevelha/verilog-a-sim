# Bridge α — the elaborated IR (`va-ir`)

> Status: **draft**, describing the **frozen v0** types in `crates/va-ir/src/lib.rs`.
> Type sketch of record: [`../interfaces.md` §α](../interfaces.md). Changes follow §6.
>
> Revised 2026-06-30 (§6): added analog control-flow statements (`While`/`For`/`Repeat`/
> `Case`) and user-defined analog functions (`Function`, `Expr::CallUser`, `Module.functions`,
> `FuncId`). Lowered by `va-frontend`; rejected (stub adapters) by `va-codegen` v0.
> Also added the trig/hyperbolic and `hypot`/`atan2`/`min`/`max` math `Builtin`s, with AD
> derivatives in `va-codegen` (FD-validated per §5). Added `Expr::Select` for the ternary
> `?:` (only the taken branch is evaluated; its gradient flows through), and the logical/
> inequality `BinOp`s `Ne`/`And`/`Or` (boolean-valued `0.0`/`1.0`, zero gradient). Added the
> rounding `Builtin`s `Floor`/`Ceil`/`Round`/`Int` (piecewise constant, zero gradient).
>
> Revised 2026-07-01 (§6, missed at the time — recorded here retroactively): added the
> bitwise/shift `BinOp`s `BitAnd`/`BitOr`/`BitXor`/`BitXnor`/`Shl`/`Shr` and `UnOp::BitNot`,
> each truncating its operand(s) to `i64` (no bit-vector type in this IR) and, in
> `va-codegen`'s AD, zero-gradient like the comparison/logical operators.
>
> Revised 2026-07-02 (§6): `Module.ports` reshaped from `Vec<NodeId>` to `Vec<Vec<NodeId>>` —
> one entry per declared port, holding >1 `NodeId` (ascending index order) for a vector port
> (`electrical [msb:lsb] name;` on a port net). Closes the "vector ports" backlog item in
> `../roadmap.md`. `va-codegen` did not read `Module.ports` in its actual lowering path (only
> `module.nodes.len()` via the `terminals` argument to `build_instance`), so this was a
> low-blast-radius change — only three `va-codegen` test fixtures needed updating.
>
> Revised 2026-07-02 (§6, later same day): added `Expr::Ddx(ExprId, Access)`, the analog
> partial-derivative operator (LRM §4.5.13). Confirmed needed by 10+ real corpus files on first
> attempt (the entire PSP102 MOSFET family, MVSG, JFET, MOSVAR); implemented exactly per the
> LRM's own worked examples, not approximated — `va-codegen`'s forward-mode `Dual` already
> carries a full per-node gradient, so `ddx(expr, V(p, n))` is just "read the gradient
> component at node `p`'s slot," with no new numerical method needed. The `Access` is carried
> structurally rather than as another `ExprId`, since it names *which* unknown to differentiate
> against and is never itself evaluated to a value.
>
> Revised 2026-07-06 (§6): added `Function::arg_dirs: Vec<ArgDir>` (`ArgDir` = `Input`/`Output`/
> `Inout`), recording the LRM's argument-direction qualifiers — previously parsed by
> `va-frontend` (`ast::FuncArg::dir`) but dropped during elaboration, so an `output`/`inout`
> argument's result had nowhere to go. Confirmed needed by `mvsg_cmc_1.1.1.va`/
> `mvsg_cmc_2.1.0.va`'s `calc_iq`/`calc_capt` idiom (a function computing several results at
> once). Additive: every existing `Function` construction site only needed
> `arg_dirs: vec![ArgDir::Input; args.len()]`, an exact behavioral no-op.

## 1. Role

Bridge α is the seam between the **language half** and the **model-generation half** of the
pipeline. It carries one fully **elaborated** Verilog-A module: source that has been lexed,
parsed, name-resolved, and reduced to a flat, arena-shaped data structure with every
identifier already bound to an index.

"Elaborated" is the key word. Everything that is a *source-language* concern has already
been resolved before a value crosses this bridge:

- identifiers → `NodeId` / `ParamId` / `VarId` / `BranchId`;
- `module` instantiation, `parameter` overrides, `genvar`/`generate` → already expanded;
- macros, includes, `localparam` folding → already done.

Downstream (`va-codegen`) never sees a string identifier it has to resolve. It sees indices
and arena handles. That is what keeps the differentiation pass simple.

## 2. Producers & consumers

| Party              | Crate         | Promise across this bridge                                       |
|--------------------|---------------|-----------------------------------------------------------------|
| **Producer**       | `va-frontend` (T1) | Emits a `Module` satisfying every invariant in §4. Never emits dangling handles. |
| **Consumer**       | `va-codegen` (T2)  | Treats `Module` as read-only. Differentiates expressions, lowers statements to a `ModelInstance`. May assume the invariants without re-checking. |

`va-ir` itself owns only the data types and trivial arena helpers (`push_expr`, `expr`).
It contains **no** lexing, parsing, or codegen logic — that would couple the two halves and
defeat the point of the leaf crate.

## 3. The contract

The authoritative definition is `crates/va-ir/src/lib.rs`. The shipped types flesh out the
§4 sketch (adding `VarId`, `VarDecl`, `FuncId`, `Discipline`, `AccessKind`, and arena
helpers) without restructuring it. The shape:

```
Module
├── name:      String
├── ports:     Vec<Vec<NodeId>>     // one entry per declared port, in declaration order;
│                                   // >1 NodeId (ascending index order) for a vector port
├── nodes:     Vec<NodeDecl>        // { name, discipline }
├── branches:  Vec<Branch>          // { p: NodeId, n: NodeId }
├── params:    Vec<Param>           // { name, default, min?, max? }
├── vars:      Vec<VarDecl>         // local analog variables (incl. function args/locals)
├── exprs:     Vec<Expr>            // the expression ARENA
├── functions: Vec<Function>        // user-defined analog functions
└── analog:    Vec<Stmt>            // the top-level analog block (flat list)
```

- **Handles** (`NodeId`, `ParamId`, `ExprId`, `BranchId`, `VarId`, `FuncId`) are `Copy`
  newtypes over `u32`. They are positions in the correspondingly-named `Vec`.
- **`Expr`** is an arena node: `Const`, `Param`, `Var`, `Probe(Access)`, `Unary`, `Binary`,
  `Call(Builtin, …)`, `CallUser(FuncId, …)`, `Select(cond, then, else)` (the ternary `?:`),
  `Ddx(ExprId, Access)` (`ddx(expr, probe)`, the analog partial-derivative operator — `Access`
  is carried directly, not as another `ExprId`, since it names which unknown to differentiate
  against rather than being evaluated to a value). Children are `ExprId`s — never `Box`, never
  `&`.
- **`Stmt`** is `Contribute { target, value }` (`<+`), `If`, `Assign { lhs, rhs }`, `Block`,
  and the analog control-flow forms `While`, `For`, `Repeat`, `Case` (with `CaseArm`).
  Control flow nests via owned `Vec<Stmt>`; `For` boxes its single `init`/`step` statements
  (a finite-size tree node, not a shared graph), so the §5 arena rule still holds.
- **`Function`** = `{ name, args: Vec<VarId>, arg_dirs: Vec<ArgDir>, ret: VarId, body: Vec<Stmt>
  }`, where `ArgDir` is `Input`/`Output`/`Inout` (the LRM's argument-direction qualifiers),
  same length and order as `args`. A function's arguments, return variable, and locals all
  live in `Module.vars`; `CallUser` binds the argument expressions positionally to `args` — an
  `Input`/`Inout` argument's caller-side expression is read in as the initial value, and an
  `Output`/`Inout` argument's final binding is written back to the caller's own variable once
  the call returns (which the LRM restricts an `output`/`inout` actual argument to being in the
  first place: a plain variable, never a general expression).
- **`Access`** = `{ kind: AccessKind, branch: BranchId }`, where `AccessKind` is `Potential`
  (`V(b)`) or `Flow` (`I(b)`). It is used both as a probe (`Expr::Probe`) and as a
  contribution target (`Stmt::Contribute`).

## 4. Semantics & invariants

A `Module` is **valid** iff all of the following hold. The producer guarantees them; the
consumer may rely on them without re-checking.

1. **No dangling handles.** Every `ExprId` is `< exprs.len()`; every `NodeId < nodes.len()`;
   every `ParamId < params.len()`; every `BranchId < branches.len()`; every `VarId <
   vars.len()`. There is no `null`/sentinel handle.
2. **Arena is acyclic and bottom-up.** An `Expr` only references children with strictly
   smaller `ExprId` than itself is *not* required, but the arena **must** be a DAG (no
   cycle). Codegen relies on being able to evaluate/differentiate each node from its
   children.
3. **Ports are nodes.** Every `NodeId` in every `ports` entry is a valid index into `nodes`.
   Ports come first in declaration order; internal nodes follow. A port entry with more than
   one `NodeId` is a vector port; its nodes are listed in ascending declared-index order
   (`bus[0]`, `bus[1]`, …), independent of whether the source wrote `[msb:lsb]` or `[lsb:msb]`
   — the original direction is not tracked, since no consumer has an opinion on connection
   order yet (`va-netlist` doesn't parse multi-terminal port connections at all).
4. **Branch endpoints exist.** For every `Branch`, `p` and `n` are valid `NodeId`s. `p == n`
   is illegal (a zero-span branch has no meaning).
5. **Discipline agreement.** The two nodes of a branch share a compatible `Discipline`
   (electrical–electrical, thermal–thermal). Cross-discipline branches are out of scope for
   v0 and must be rejected by the frontend, not passed across the bridge.
6. **`Assign` precedes use.** Within the flattened `analog` block, a `Var` is read only after
   an `Assign` to it dominates the read on every control-flow path. (The frontend resolves
   Verilog-A's procedural-assignment semantics; codegen treats `Var` as already SSA-able.)
7. **Contribution targets are branch accesses.** `Stmt::Contribute.target` is an `Access`
   over a declared `Branch`. Potential and flow contributions to the same branch follow
   Verilog-A's switch-branch rules; mixing them is the frontend's problem to have already
   resolved.
8. **Parameter ranges are consistent.** If both present, `min <= default <= max`.
9. **`Builtin` arity is correct.** Each `Call` carries exactly the argument count its
   `Builtin` requires: the unary functions (`Exp`/`Ln`/`Log`/`Sqrt`/`Abs`/`Floor`/`Ceil`/
   `Round`/`Int`/`Ddt`/`Idt` and the trig/hyperbolic family `Sin`/`Cos`/`Tan`/`Sinh`/`Cosh`/
   `Tanh`/`Asin`/`Acos`/`Atan`/`Asinh`/`Acosh`/`Atanh`) take 1; the binary functions
   (`Pow`/`Hypot`/`Atan2`/`Min`/`Max`) take 2; `Vt`/`Temperature` take 0.
10. **`CallUser` is well-formed.** Every `FuncId` is `< functions.len()`, and the argument
    count equals the callee's `args.len()`. A function's `args`/`ret`/local `VarId`s are valid
    indices into `vars`. Functions are pure and non-recursive (no `CallUser` to itself or a
    cycle); the frontend resolves calls in source order, so the call graph is a DAG.
11. **`Ddx`'s `Access` is a valid potential probe.** `Expr::Ddx(_, access)`'s `access.branch` is
    a valid `BranchId` (as any `Access` must be), and `access.kind == AccessKind::Potential` —
    the frontend never emits a flow-kind `Ddx` (v0 codegen has no independent unknown for a
    branch current to differentiate against; the frontend rejects `ddx(..., I(...))` before it
    would reach this bridge).

> These invariants are the draft acceptance criteria for `va-frontend`'s elaboration output
> and should become a `va-ir::validate(&Module) -> Result<(), IrError>` checker (open item,
> §8) so both teams can assert the same thing.

## 5. Conventions

- **Disciplines** carry the physical meaning (§ `Discipline`): `Electrical` → potential is
  voltage, flow is current; `Thermal` → potential is temperature, flow is power. `Other` is
  reserved for the multi-physics roadmap and must not be relied on by v0 codegen.
- **`ddt`/`idt`.** `Ddt` marks a quantity that becomes a **charge contribution** downstream
  (Bridge β's charge channel); `Idt` is the time integral. The IR records the operator; it
  does **not** pre-lower it to a companion model — that is codegen's job.
- **System functions.** `$vt`, `$temperature` enter as `Builtin::Vt` / `Builtin::Temperature`
  with no arguments. Their numeric values are supplied at solve time, not baked into the IR.
- **No units in the type system.** Values are `f64` in SI base units by convention
  (volts, amperes, kelvin, coulombs). The bridge does not carry unit tags; correctness is by
  convention and validated against QSPICE downstream (§7).

## 6. Worked example — `I(b) <+ V(b)/R`

A linear resistor module `R` with one branch `b` between ports `p` and `n`, one parameter
`R`, and one contribution. Built through the arena API:

```rust
let mut m = Module::new("resistor");
let p = NodeId(0); let n = NodeId(1);
m.nodes = vec![
    NodeDecl { name: "p".into(), discipline: Discipline::Electrical },
    NodeDecl { name: "n".into(), discipline: Discipline::Electrical },
];
m.ports = vec![vec![p], vec![n]];             // two scalar ports; a vector port would list
                                               // all of its NodeIds in one inner Vec instead
m.branches = vec![Branch { p, n }];                       // BranchId(0)
m.params  = vec![Param { name: "R".into(), default: 1e3, min: Some(0.0), max: None }];

let v   = m.push_expr(Expr::Probe(Access { kind: AccessKind::Potential, branch: BranchId(0) }));
let r   = m.push_expr(Expr::Param(ParamId(0)));
let i   = m.push_expr(Expr::Binary(BinOp::Div, v, r));    // V(b) / R
m.analog = vec![Stmt::Contribute {
    target: Access { kind: AccessKind::Flow, branch: BranchId(0) },
    value: i,
}];
```

What crosses the bridge is exactly this `Module`. `va-codegen` reads it, differentiates the
contribution `V(b)/R` with respect to the branch voltage to get the conductance `1/R`, and
emits a `ModelInstance` (Bridge β) whose `load` stamps `residual = V/R` and `jacobian =
1/R`. Trace the continuation in [interface-beta-abi.md §6](interface-beta-abi.md).

## 7. Edge cases & non-goals

- **Single module only.** No instance hierarchy, no submodules. Multi-module elaboration is
  out of scope for v0; the frontend flattens or rejects.
- **No strings past the bridge.** `name` fields are for diagnostics only. Codegen must not
  parse or pattern-match on names to recover semantics.
- **No source spans (yet).** The v0 IR carries no location info. Diagnostics that need
  source positions stay inside `va-frontend`. Adding spans later is an additive §6 change.
- **`Other` discipline.** Present in the enum for roadmap reasons; v0 codegen may treat it as
  unsupported and error, rather than guess a conserved quantity.

## 8. Evolution (per §6)

Bridge α is frozen. To change it:

1. Open an issue naming the change and **every** downstream effect (realistically only
   `va-codegen` consumes α, but list it explicitly).
2. Get `va-frontend` and `va-codegen` owners to agree.
3. Update this document, `../interfaces.md`, and `va-ir` in **one** PR, with stub adapters so
   the workspace keeps compiling.

Additive changes (new `Builtin` variant, new optional field, new `Expr`/`Stmt` arm behind a
codegen `todo!()`) are lower-risk but still go through §6 because adding an enum arm is a
breaking change for exhaustive `match`es in `va-codegen`.

**Open items** (draft backlog, not yet contract):
- [ ] `va-ir::validate(&Module) -> Result<(), IrError>` implementing §4's invariants.
- [ ] Decide whether to carry optional source spans for diagnostics.
- [ ] Pin down switch-branch (potential vs flow contribution) resolution rules in prose.
