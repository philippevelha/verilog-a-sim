# Proposal: evaluate device instances in parallel, reproducibly

**Status:** proposed, 2026-09-25. The maths-library choice (§3.3, option b: `libm`) is decided by
the user; everything else is open.
**Affects:** `va-abi` (**Interface β** — a `Send + Sync` bound on `ModelInstance`, a CLAUDE.md §6
change), `va-codegen` (shared-state types, maths calls), `va-core` (assembly, linear-solve
configuration), `va-transient` (assembly), `va-cli` (thread-count option). `va-ir` untouched.
**Follows:** `references/eespice.pdf` (Bao & Chitnis, arXiv:2604.03079, 2026);
`docs/proposals/gradient-storage.md` and `evaluator-tree-walk.md`, whose single-thread work this
does not replace but multiplies.
**Evidence:** a prototype on the local branch `proto/parallel-assembly` (not for merging). Every
number below is from it, release build, i7-1185G7 (4 cores / 8 threads), Windows 11, GNU
toolchain, one run per configuration unless stated (power: see the caution below). This laptop's wall times vary up
to ~2× run to run; read them as ranges.

> **Caution — how far to trust these results.**
>
> - **Firm:** the correctness findings. Bit-identity (serial vs parallel, `deck-diff --all` and a
>   full-precision dump), the MinGW per-thread maths behaviour, `faer`'s core-count-dependent
>   rounding, the `libm` accuracy table, and `validate` 28/28 are deterministic checks: they give the
>   same answer on every rerun.
> - **Preliminary:** every timing. One laptop (4 cores / 8 threads), **one run per configuration**,
>   and the machine's power was unstable on the day — it dropped to battery at least twice, which on
>   this laptop has measured up to 3.5× slower. The c432 and chain160 figures were taken with AC
>   confirmed before the sequence, not during it. **The c17 transient timing is void** (§4.4) and
>   must be redone. Treat speed-ups as "roughly 2× on this machine", not as figures to plan on.
> - **Extrapolated, not measured:** anything about 32 cores or the rest of ISCAS'85 (§5).
> - **Not reproducible from the repository alone:** the prototype is not committed — it is a local
>   branch (`proto/parallel-assembly`) and a local stash on the author's machine. The numbers can
>   be regenerated only by rebuilding it from those, or from this document's description.

---

## 1. Where the time goes

A DC operating point of ISCAS'85 c432 (910 PSP103, 15 416 unknowns, 558 Newton iterations),
prototype run serially:

| phase | time | share |
|---|---:|---:|
| evaluating every instance (`load()`, incl. stamping) | 62.4 s | 77% |
| numeric LU factorization | 16.0 s | 20% |
| triangular solves | 1.1 s | 1% |
| symbolic LU, setup, output | 1.4 s | 2% |
| **wall** | **80.9 s** | |

A week of single-thread work on the evaluator (1.13.0–1.16.0) took PSP103's `load()` from ~176 to
~100 µs. It is still three quarters of the run, and every instance is evaluated independently of
every other — the textbook case for parallelism.

## 2. What EEspice does, and what we take from it

EEspice splits each Newton iteration's device work into *compute* (evaluate a device) and *stamp*
(add its contributions into the shared matrix). Compute parallelises trivially; stamp does not,
because two devices on a shared node write the same matrix entry. EEspice builds a conflict graph
(devices sharing a non-ground node), greedy-colours it, and runs colours in sequence with every
device of one colour evaluated **and** stamped in parallel, lock-free.

Its own numbers argue for the simpler variant here:

- 45× on 64 cores holds only for **one colour** (no shared nodes). On its realistic benchmark, a
  64-bit adder with 896 colours, colouring took 127 s of device time against 12.1 s for
  "loadomp" (parallel compute, serial stamping). Our ISCAS and inverter-chain circuits share nodes
  the same way.
- loadomp capped at 1.5× there because EEspice's BSIM4 compute is fast C and stamping was the
  larger share. **Ours is the opposite:** a PSP103 evaluation is ~100 µs and its ~80 stamps are a
  few percent of it. Serial stamping costs us little (§4: replay 2.1–2.4 s of a 42–46 s c432 run).
- Colouring changes the order in which contributions to one matrix entry are summed, so results
  would depend on the colouring. The scheme below keeps them bit-identical to the serial loop.

The paper's closing point — once devices are fast, the sparse solve dominates — is borne out
here (§4) and is why §5 exists.

## 3. Design

### 3.1 Record, then replay in instance order

Each Newton iteration:

1. evaluate every instance in parallel (rayon), each into its **own** stamp buffer — a
   `StampSink` that records `(row, col, value)` operations in the order the model made them;
2. replay the buffers into the real system **on one thread, in instance order**.

Every matrix entry therefore receives exactly the same additions in exactly the same order as the
serial loop, so the assembled system is bit-identical, whatever the thread count. Nothing about the
sink, the pattern, or the solver changes. The same scheme applies to the transient integrator's
assembly, whose per-instance state slices (`StateBuffers`) are split into disjoint `(committed,
scratch)` pairs so each worker owns its instance's.

Cost: the buffers (a `Vec` per instance per iteration, unmeasured separately) and the serial replay
(c432: 2.1–2.4 s over 558 iterations, ~4 µs per instance per iteration). Replay is serial and
becomes the next Amdahl limit at high core counts (§6).

### 3.2 Interface β: `ModelInstance: Send + Sync` (a §6 change)

Worker threads share `&dyn ModelInstance`, so the trait must promise `Sync` (and `Send`, to move
built instances). The prototype added the bound and let the compiler list everything that broke.
It was very little:

| where | was | now | why it was not thread-safe |
|---|---|---|---|
| `va-codegen` `CompiledModel`/`GeneratedModel::shared` | `Rc<SharedModel>` | `Arc` | reference count shared across instances |
| `va-codegen` `SharedModel::setup` | `RefCell<Option<Setup>>` | `OnceLock<Setup>` | filled lazily on first `load` — exactly what `OnceLock` is for |
| `va-codegen` `Grad::Dense` | `Rc<[f64]>` | `Arc<[f64]>` | a `Setup`'s duals live in shared state |
| `va-transient` test `CountingCapacitor` | `Cell<usize>` | `AtomicUsize` | test-only counter |

Every reference model in `va-abi`, every wrapper in `va-core`/`va-transient`, and every netlist
source was already `Send + Sync`. The `Setup` duals carry no gradient in practice (setup runs
before any probe), so `Arc` there costs no cross-thread reference-count traffic; per-evaluation
duals are thread-local, so the atomic count on them is uncontended.

Downstream effect for §6: any future `ModelInstance` implementation must be thread-safe. For an
implementation that is not, the answer is `Mutex`/atomics or keeping its interior state per call —
the existing ones show none needed it.

### 3.3 Thread-independent maths: `libm` (decided)

> **Landed in 1.17.0** (2026-09-26) — Step 2: every transcendental that can reach a simulated
> number goes through `libm` in all library crates, tests included, and `clippy.toml`'s
> `disallowed-methods` keeps it that way. The one-time shift against 1.16.1 is in release.txt.

The first prototype's parallel results differed from serial in the last bits, even with **one**
worker thread, and even though record/replay on the main thread was bit-identical. Cause, measured:
on the `x86_64-pc-windows-gnu` toolchain (this project's), `exp` and `pow` come from MinGW and use
the x87 unit, whose precision-control setting is **per thread**: the main thread starts with
64-bit precision (control word `0x037f`, set by the C runtime), a new Windows thread with 53-bit
(`0x027f`). Same input, different bits, depending on which thread ran it.

Accuracy against correctly rounded values (mpmath at 200 bits), 20 000 points per function:

| function | MinGW, main thread | MinGW, other thread | `libm` crate |
|---|---|---|---|
| `exp` | 99.97% correctly rounded, ≤1 ulp | 82.4%, ≤1 ulp | 90.2%, ≤1 ulp |
| `pow` (x^1.7) | 99.80%, ≤1 ulp | **11.2%, ≤11 ulp** | 90.0%, ≤1 ulp |
| `atanh` | 62.3%, **≤19 ulp** | 54.6%, ≤19 ulp | 81.7%, ≤1 ulp |
| `acosh` | 88.8%, ≤8 ulp | 88.8%, ≤8 ulp | 91.6%, ≤1 ulp |
| others tested (ln, log10, sin, cos, tan, asin, acos, atan, sinh, cosh, tanh, asinh, atan2, hypot) | ≤1 ulp, thread-independent | same | ≤1 ulp |

And speed (ns per call, same machine): `exp` 26.6 → 6.8, `ln` 18.2 → 6.3, `pow` 57 → 47,
`sin`/`cos` 32 → 6; `sinh`/`tanh` slightly slower (8.8 → 13.2, 9.2 → 11.9).

Two ways to make workers agree with the serial loop: reset each worker's x87 unit (`fninit`, one
line of inline assembly — `unsafe`, which `va-core` forbids), or stop using the platform maths for
model evaluation. **Decided: the second** — the pure-Rust `libm` crate (a port of musl's libm), for
every transcendental call in the evaluation path. It needs no `unsafe`, gives the same bits on
every thread **and every platform** (Linux, macOS, MSVC and GNU Windows no longer each bring their
own libm), is ≤1 ulp everywhere measured — including where MinGW is 8–19 ulp out — and is faster
where compact models spend their time (`exp`, `ln`, `pow`). `sqrt` stays `f64::sqrt`: IEEE 754
requires it correctly rounded, so it is already identical everywhere.

The cost is a one-time shift of results against today's, measured in §4.2.

Prototype scope: `va-codegen`'s `ad.rs`/`lib.rs` (the dual-number rules and the Laplace/Z-domain
helpers) and `va-abi`'s reference models and noise tables. **The proposal extends it to** every
maths call that can affect a simulated number — `va-frontend`'s constant folding, `va-transient`,
`va-acnoise`, `va-core`'s convergence aids — for the cross-platform guarantee, not for threading
(those run on the main thread). A clippy `disallowed_methods` rule for `f64::exp`/`ln`/`powf`/…
in library crates keeps it that way.

### 3.4 A reproducible linear solve (an existing bug this found)

> **Landed in 1.16.1** (2026-09-26), ahead of the rest of Step 2 because it is a bug on main,
> not part of the feature: every `va-core` solve now pins `faer` to `Par::Seq`. Verified on
> chain160 (identical at 1, 4 and 8 threads) and by a unit test that fails without the pin.

With `libm` in place, parallel evaluation was bit-identical to serial at 8 threads but not at 1, 2
or 4. The serial binary showed the same thing under `RAYON_NUM_THREADS=1`, and so does **main**:
chain160's printed output changes with `RAYON_NUM_THREADS` (6 lines at 1, 159 at 4). `faer`
parallelises its factorization over rayon's global pool by default, and the rounding follows the
thread count. **Today, the same deck gives different last digits on machines with different core
counts.**

Fix: set `faer`'s global parallelism explicitly. The prototype uses `Par::Seq` — and that turns
out to be *faster* here (§5.1): `faer`'s parallel sparse LU is slower on these matrices. A fixed
`Par::rayon(n)` is also reproducible (checked: `n = 4` gives identical bits under pools of 1, 2, 4
and 8 threads), so if a parallel LU is ever worth it, it must be a fixed project-wide `n`, never
"all cores".

### 3.5 Threads

rayon's global pool, default size = `available_parallelism()` (logical cores), overridable with
`RAYON_NUM_THREADS` and a `va-cli --threads <n>` flag. On this machine 8 threads beat 4 slightly
once the LU was pinned sequential (c432 42.1 vs 46.5 s); the earlier result that 8 was *slower*
than 4 was `faer`'s parallel LU competing with the evaluator for cores. Below some instance count
the parallel path is overhead only; a threshold (serial below it, like the dense/sparse switch) is
to be measured from the c17 transient and the small decks, not guessed.

## 4. Measurements

### 4.1 Correctness

- **Serial vs parallel, all 72 decks** (including c432's `.op` and c17's 13 466-point transient),
  `cargo xtask deck-diff` of two copies of one build: **72 identical**.
- **Full precision**, chain160 (5 286 unknowns, solution vector dumped at round-trip precision):
  bit-identical to serial at 1, 2, 4 and 8 threads.
- `cargo test --workspace`: 890 passed. `cargo clippy -D warnings`: clean.

### 4.2 The one-time shift against main (`libm` + sequential LU)

- `deck-diff` main vs prototype: **59 of 70 identical**, 11 differ.
- 8 of the 11 differ in the 7th printed digit, or in values that are zero to rounding
  (1e-35…1e-55).
- 3 adaptive-step transients take a different number of steps (`psp103_inverter_tran` 1861 → 1900,
  `psp103_inverter_card_tran` 1810 → 1808, `ring_osc` 2243 → 2246). Waveform difference, resampled
  onto main's timebase: RMS ≤ 3.7e-4 of the swing (§7's transient tolerance is 1e-3), maxima on
  switching edges where the two time grids differ (the known input pulse itself shows 4.9e-4 there
  — interpolation, not physics).
- `cargo xtask validate` against QSPICE: **28/28 on both, every error figure unchanged** at the
  precision it is printed.

### 4.3 Speed (DC operating point)

| | c432 wall | eval | replay | numeric LU | chain160 wall |
|---|---:|---:|---:|---:|---:|
| main (1.16.0+1) | 87.5 s | — | — | — | 8.9 s |
| prototype, serial | 80.9 s | 62.4 s | — | 16.0 s | 7.6 s |
| prototype, 4 threads | 46.5 s | 23.2 s | 2.1 s | 18.3 s | 4.1 s |
| prototype, 8 threads | **42.1 s** | 17.5 s | 2.4 s | 19.3 s | **4.0 s** |

c432: **2.1× over main** on 8 threads; chain160 2.2×. Evaluation scales 3.6× on 8 threads.
The serial prototype is 8% faster than main on its own (`libm`'s `exp`/`pow`, and no parallel-LU
contention). At 8 threads the numeric LU (19.3 s) now exceeds evaluation (17.5 s).

### 4.4 Speed (transient)

c17 transient (12 ns at 1 ps, 24 PSP103, 420 unknowns, ~54 000 assemblies): **not yet measured
reliably.** The first attempt (2026-09-25) is void: the laptop went onto battery during the
sequence (main 144 s, prototype serial 222 s, 4 threads 221 s, 8 threads 97 s — the same parallel
binary 2.3× apart between 4 and 8 threads, and serial slower than main, the reverse of every AC
run). To repeat on AC. What it did show: `libm` moves c17's step count 13 466 → 13 506 (the
§4.2 kind of shift), and the transient is bit-identical serial vs parallel (§4.1). A circuit this
small (81 devices after flattening) is where the small-circuit
threshold of §3.5 will be decided.

## 5. The solve is next

After §3, c432 on 8 threads spends 46% of its time in numeric factorization, and on a larger machine
it would be most of it (a 32-core extrapolation to all of ISCAS'85 puts the solve at 80–90% of each
iteration; that estimate is soft — device counts guessed from c432, solve growth fitted to two
points). Two candidates, neither prototyped for speed yet.

### 5.1 A parallel sparse LU

**`faer`'s built-in parallelism does not help here — it hurts** (c432 numeric factorization:
sequential 16.0 s, 4-way 22.5 s, 8-way 36.5 s; chain160 1.5 / 2.9 / 4.5 s). Why is not measured.
The likely reason: parallel sparse LU gets its concurrency from dense sub-blocks (supernodes), and
circuit matrices are so sparse that those blocks are tiny, leaving synchronisation as the main
cost. It is the reason circuit simulators generally use circuit-specific solvers (KLU:
left-looking, no supernodes, block-triangular pre-ordering).

Options, in order of cost:

- **(a) Block-triangular form (BTF) first.** Permute the matrix to block upper triangular form
  (strongly connected components of its graph); factor each diagonal block independently. Blocks
  with no dependency between them factor in parallel, and many circuit matrices decompose into
  many small blocks. Pure Rust, deterministic (block schedule fixed by the pattern, not the core
  count). Unknown until measured: how many independent blocks c432's matrix has — **step 1 is to
  count them** (a Tarjan SCC over the pattern, cheap).
- **(b) Parallel left-looking LU over the column-elimination DAG** (the NICSLU approach, Chen et
  al.): columns whose dependencies are complete factor concurrently. Larger effort (the numeric
  kernel is ours to write) and reproducible only if each column's updates are applied in a fixed
  order.
- **(c) Another pure-Rust solver crate.** §5 forbids FFI (no KLU/PARDISO/SuiteSparse); a survey
  of pure-Rust sparse LU crates with circuit-matrix performance would go first.

Reproducibility is a hard requirement for all three: a fixed schedule derived from the pattern,
never from the thread count (§3.4).

### 5.2 Reusing one factorization across several Newton iterations

Today every Newton iteration assembles a fresh Jacobian and factors it. A *chord* (modified)
Newton step instead reuses the last factorization with a fresh residual: `J_k⁻¹` applied to
`−F(x_{k+m})`. It costs a residual and two triangular solves (c432: 1.1 s across 558 solves, i.e.
~2 ms each vs ~29 ms for a numeric factorization) — but convergence becomes linear instead of
quadratic, so it takes more iterations.

It pays only if the saved factorizations outweigh the extra iterations, and each extra iteration
costs an evaluation (the dominant cost). The lever that makes it attractive:

- **A chord iteration does not need the Jacobian.** Most of a `load()` is dual-number gradient
  work (gradient storage, the `Grad` rules; `docs/proposals/gradient-storage.md`). A **value-only
  evaluation** mode — the same tape, `f64` instead of `Dual` — for chord iterations would make them
  much cheaper than a full iteration. Not measured; to be bounded first by timing a value-only
  PSP103 `load()` (`xtask bench-model`).

Design points that need deciding:

- **When to refactor:** after `m` chord steps, or when the step fails to contract (‖dx_{k+1}‖ >
  ρ‖dx_k‖, ρ ≈ 0.3–0.5), or immediately when a convergence aid (limiting, damping, a `gmin`
  step) changes the system. The first iterations of a solve are far from the solution, where
  Newton needs fresh Jacobians most; reuse belongs near convergence and across transient
  timesteps whose Jacobian barely changes.
- **Convergence test:** today's test is the applied step (`|dx| ≤ reltol·|x| + abstol`,
  `newton.rs`). A linearly converging sequence can take a small step while still an error
  `ρ/(1−ρ)` times larger away, so a chord solve must **accept only after a full-Newton step**
  (fresh Jacobian) passes the test — which keeps the converged answer the one full Newton gives,
  to within tolerance.
- **Transient:** the natural home — consecutive timesteps with small `h` changes see nearly the
  same Jacobian. A reuse across timesteps must refactor on any step-size change that alters the companion-model conductances by more than a
  threshold.
- **Answers move:** unlike §3, this changes the Newton path, so results shift within the Newton
  tolerance (like the §4.2 shift). It needs its own deck-diff magnitude report and `validate`,
  and an opt-out flag.

## 6. Steps

1. **Ratify the §6 interface change** (ratified 2026-09-26; `docs/interfaces.md`) (`Send + Sync` on `ModelInstance`) with the owners of
   `va-codegen` (T2), `va-transient` (T4), `va-acnoise` (T5), `va-netlist`/`va-cli` (T6).
2. **Reproducibility first, as its own release:** `libm` for all evaluation-path maths (§3.3) +
   `faer` pinned sequential (§3.4 — done in 1.16.1) + the `disallowed_methods` lint (done in
   1.17.0). No parallelism yet. Its
   release entry states the §4.2 shift. A feature that moves answers → a minor bump.
3. **Parallel assembly** (done in 1.18.0 — `va_core::par`; the threshold turned out to need
   measured *cost*, not an instance count, see release.txt): record/replay in `va-core` (DC,
   dense and sparse — the prototype did sparse only) and `va-transient`; `--threads`; the
   small-circuit threshold. Gate: `deck-diff`
   serial vs parallel at 1/2/4/8 threads, identical, plus a unit test that a model's stamps
   replay to the same system as direct stamping.
4. **Solve, measured before built:** count BTF blocks on c432/chains (§5.1a); time a value-only
   `load()` (§5.2). Then a decision document per option, as this one was.

## 7. How this is proved not to break anything

- Steps 3 and 4 claim bit-identity: `deck-diff --all` between the serial and parallel paths of one
  build, at several thread counts, plus the full-precision dump for a large circuit.
- Step 2 claims a bounded shift: `deck-diff` against the previous release with magnitudes
  reported per deck (§4.2's method), `validate` 28/28 with the error figures compared, and the
  corpus count unchanged.
- Timing: AC power checked before every run (`Win32_Battery.BatteryStatus` = 2); `--logfull`'s
  counters must be off for parallel timing (they are shared atomics and contend).

## 8. Risks and limitations

- **One machine.** All scaling is from a 4-core laptop. A 32-core machine will show the serial
  replay and the solve earlier; the ISCAS extrapolation is soft.
- **Replay is serial.** ~4 µs per instance per iteration; at high core counts it rivals
  evaluation. Parallelising it without losing bit-identity needs a per-row ordering scheme — not
  proposed.
- **Memory:** one stamp buffer per instance per iteration (c432: ~2 500 buffers) — small, not
  measured.
- **`Send + Sync` is permanent:** every future model implementation carries it.
- **The `libm` shift is permanent and one-way:** after it, results no longer match 1.16.x in the
  last digits; they will match across platforms instead.
- **`libm` is not correctly rounded:** ~90% of `exp`/`pow` results are, against 99.9% for MinGW's
  main thread. All are ≤1 ulp. The trade is reproducibility and worst-case accuracy for a few
  percent more 1-ulp results.

## 9. Decisions needed

1. Ratify `ModelInstance: Send + Sync` (§6 process).
2. Step 2 as a separate release before any parallelism — recommended.
3. Default thread count: all logical cores (measured best here) or physical cores.
4. §5: approve the two measurements (BTF block count; value-only `load()` timing) before any solver
   work.
