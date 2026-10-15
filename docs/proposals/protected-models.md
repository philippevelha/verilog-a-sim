# Proposal: protected models — signed, licensed WebAssembly behind Interface β

**Status: PROPOSED, 2026-10-15.** Nothing here is implemented.

**Affects (when implemented):** a new crate `va-wasm-model` (loader), `va-codegen` (a WASM
backend), `va-cli` and `va-netlist` (loading a protected model), `xtask` (issuing licenses),
`va-diag`/`messages/` (new codes). `va-abi` (Interface β) and `va-ir` (Interface α) are **not**
changed — see §11.

This is a decision document. It contains no code changes. It is the written description
CLAUDE.md §6 step 1 asks for before the affected owners (T2 for `va-codegen`, T6 for
`va-cli`/`va-netlist`) are consulted.

---

## 1. What this is for, and what it is not for

**For:** distributing models *we* wrote, under a license, to users who have signed a contract,
so that:

1. the user can simulate with the model but does not receive its Verilog-A source, its
   equations in readable form, or the parameter values we choose not to expose;
2. every copy is tied to one license, so a leaked copy can be traced back to it;
3. running a model file someone handed you is safe for *the user* — it cannot read their files,
   open the network, or crash the simulator.

**Not for: third-party IP received under an NDA.** That is deliberately out of scope. An NDA
decides who may receive the material and through which tools; encryption does not create a right
to redistribute, and a client-side scheme in an open-source simulator cannot meet the "reasonable
measures" standard an IP owner would hold us to (§2). NDA material stays out of this repository,
out of `golden/`, and out of this mechanism. If that changes, it changes through the IP owner's
written terms and a separate proposal, not by reusing this one.

**`` `pragma protect `` stays a refusal** (`docs/token-reference.md`). Implementing an IEEE 1735
envelope would require a decryption key in public source, i.e. advertising a protection we do not
provide.

---

## 2. Threat model

The simulator is Apache-2.0. Anything it does on the user's machine, the user can read and change.
That one fact decides the whole design: **no secret can live in the simulator.** What can be done is
to make the shipped artefact contain as little as possible, make it attributable, and make the
contract enforceable.

| Asset | Protected? | By what |
|---|---|---|
| Verilog-A source, comments, names, structure | **Yes** — not shipped | Only compiled code leaves our hands (§4) |
| The equations | **Raised cost, not prevented** | Compilation to unnamed straight-line WASM; recovering them is reverse engineering, comparable to a compiled `.osdi` |
| Hidden parameter values (our extraction) | **Raised cost, not prevented** | Folded into constants (§4.3); no parameter table exists in the binary |
| The model's terminal behaviour | **No** | Any user can sweep a black box and fit a surrogate. Nothing client-side prevents this |
| Integrity (the model is the one we issued) | **Yes** | Ed25519 signature (§7) |
| Attribution (whose copy leaked) | **Yes, as evidence** | Signed license ID in the header *and* inside the binary (§7.4) |
| Enforcement of license terms (expiry, use) | **No, technically** | A modified simulator skips any check. Enforcement is the contract (§8) |

Adversaries considered:

- **The careless licensee** who forwards a file to a colleague. Addressed: the colleague sees
  someone else's license ID on every output; the file traces back.
- **The determined licensee** who wants the equations. Slowed, not stopped. Contractually
  prohibited (no reverse engineering, no circumvention) and attributable.
- **A malicious model file** (a forged or tampered `.vam`). Addressed: signature check plus the
  sandbox (§6) — a bad file can fail, it cannot harm the user's machine.

Explicitly **not** addressed: someone with our private signing key (key handling, §7.3), and a
licensee who measures the model's behaviour.

---

## 3. Design overview

```
 OUR SIDE (private)                                    USER SIDE (open-source simulator)
 model.va ─► va-frontend ─► IR ─► va-codegen            model.vam
                                   └─ wasm backend           │
                                        │                    ▼
                               model.wasm               [va-wasm-model]
                               (no names, hidden        1. verify Ed25519 signature
                                params folded)          2. check format + ABI version
                                        │               3. check expiry, show licensee
 license record ─► xtask license issue ─► model.vam     4. instantiate in sandbox
                   (offline signing key)                     │
                                                             ▼
                                              WasmInstance: impl ModelInstance ─► va-core
```

`va-core` cannot tell a protected model from a reference model or a JIT-compiled one. That is the
point of the §2 invariant in CLAUDE.md: Interface β is the boundary, and this proposal plugs into it
from the outside.

---

## 4. The compiled module

### 4.1 Why not "compile the interpreter to WASM and embed the IR"

That is the shortest path and it protects nothing: the serialized IR is data in the module's data
section, dumpable in one command, and it is close to the source (names aside). The module must
contain **code**, not a description of the model.

### 4.2 The backend

`va-codegen` already lowers a model's tapes to machine code through Cranelift (`sched::jit`). A WASM
backend is a second target for the same lowering, emitting one function per entry point (§5) with:

- the primal values and the forward-mode derivatives interleaved, as the JIT path does — the
  Jacobian is not a separate, readable expression;
- straight-line code for the statement schedule, `if`/`else` lowered to WASM blocks;
- every identifier dropped (no name section; no custom sections other than §7.4's);
- emitted with the pure-Rust `wasm-encoder` crate.

Constructs the backend cannot lower yet are an `R04xx` refusal at **issue** time, on our side —
never a silently different model on the user's side.

### 4.3 Parameters

The issuer chooses, per model, which parameters the licensee may set. Those become inputs to
`instantiate`, with their declared ranges checked by the module. Every other parameter is folded
into the code as a constant and then optimised with the rest of it, so it does not survive as a
recognisable value table.

### 4.4 Honest limit

A WASM decompiler returns arithmetic without names, structure or comments. A patient person can
recover the equations from it. We state this to licensees rather than imply otherwise; the
contract (§8) is what makes doing so a breach.

---

## 5. The guest ABI

Interface β's sinks are trait objects; they cannot cross a WASM boundary. The module therefore works
on flat buffers in its own linear memory and sees **only its local terminals** — never global
indices. The host adapter does the mapping.

### 5.1 Exports

| Export | Mirrors | Notes |
|---|---|---|
| `vam_abi_version() -> u32` | — | Host refuses an unknown major version |
| `vam_meta() -> (ptr, len)` | `unknown_kind`, `unknown_is_junction`, `unknown_abstol`, `state_len`, `event_count`, `is_frequency_dependent`, `is_pure_in_x` | A small CBOR document: port count, internal-unknown count, exposed parameters with ranges, per-unknown flags, noise source shapes |
| `vam_instantiate(params_ptr) -> handle` | instance construction | Range-checks parameters; error code on failure |
| `vam_load(handle, in_ptr, out_ptr)` | `load` | `in`: local `x`, the `AnalysisCtx` fields (`kind`, `time`, `temp`, `is_initial_step`), the state slice. `out`: dense local residual, Jacobian, charge, `dcharge`, plus the optional channels below |
| `vam_noise(handle, in_ptr, out_ptr)` | `noise` | White / flicker / table sources, local terminal pairs |
| `vam_events(handle, in_ptr, out_ptr)` | `events_with_state` | Monitors, breakpoints, timers |

Optional `load` channels — AC `excitation`, `bound_step` — are present only if `vam_meta` declares
them, so a plain resistive model pays for none of it.

The adapter calls `vam_load` once and replays the output buffers into the real `StampSink` through
`unknowns()`. One boundary crossing per `load`, not one per stamp.

### 5.2 Imports

- **Maths only**: `exp`, `ln`, `log10`, `pow`, `sqrt`, the trigonometric and hyperbolic families —
  whatever the tape's pure-maths set uses. WASM has no transcendental instructions. Importing the
  host's implementations is a requirement, not a convenience: it is what lets §10's bit-identity
  check against the JIT path hold.
- **Nothing else.** No WASI: no files, clock, environment or network.

### 5.3 System tasks

`$fopen`/`$fwrite`/`$fclose` and other file operations are refused at issue time for a protected
model. `$strobe`/`$display`/`$debug` are an issuer choice per model (default: refused), because they
can print internal values; if allowed, the text is passed through a `task` output channel.
`$finish` and `$error` pass through as codes.

---

## 6. Sandbox and runtime

### 6.1 Engine

| | `wasmi` | `wasmtime` |
|---|---|---|
| Implementation | Interpreter, safe Rust | Compiles to native code via Cranelift (already a dependency family of `va-codegen`) |
| `unsafe` | None in our code; none needing sign-off | Uses `unsafe` internally; needs the same CLAUDE.md §5 sign-off `sched::jit` got (2026-10-08) |
| Speed | Clearly slower than native | Near JIT speed |

**Start with `wasmi`.** It needs no sign-off and proves the ABI. Move to `wasmtime` only if
`bench-scale` shows the interpreter matters on real circuits. Either must pass `cargo deny check`
before it is added; that has not been checked yet.

### 6.2 Limits

- linear memory capped (per module, from `vam_meta`, with a host-wide ceiling);
- fuel (`wasmi`) or epoch interruption (`wasmtime`) bounds every call — a module that loops forever
  becomes an error, not a hung simulation;
- every trap becomes a `Result` error with a diagnostic code. No panic crosses into the host
  (CLAUDE.md §5: libraries never panic on bad input).

### 6.3 `Send + Sync`

`ModelInstance: Send + Sync`, and `va_core::par` calls `load` from several threads. A WASM store is
not `Sync`. The adapter keeps a small pool of module instances, one per worker thread, all
instantiated with the same parameters. State lives in the host's `ModelState`, copied in and out on
each call, so no per-thread module instance carries anything between calls.

---

## 7. The `.vam` container and licensing

### 7.1 Layout

```
"VAM1"  | u32 header_len | header (CBOR) | u32 wasm_len | wasm bytes | 64-byte Ed25519 signature
header: format_version, abi_version, model_name, model_version,
        license_id, licensee_display, issued, expires (optional), permitted_use,
        exposed_parameters, sha256(wasm), signing_key_id
```

The signature covers every byte before it.

### 7.2 Loading

1. Signature verifies against one of the public keys compiled into the simulator, selected by
   `signing_key_id` → otherwise refuse.
2. `format_version` and `abi_version` are known → otherwise refuse, naming the versions.
3. `sha256(wasm)` matches → otherwise refuse.
4. `expires` not passed → otherwise refuse (and say so plainly: the user's clock decides this).
5. Print `licensed model <model_name> <model_version> — license <license_id> (<licensee_display>)`
   in the run banner, the header of every output file, and plot metadata.

### 7.3 Keys

- The **private** key never touches the repository or CI. It lives offline, ideally on a hardware
  token, and is used only by `cargo xtask license issue`.
- The **public** keys ship in the simulator; publishing them is harmless — they only verify.
- A list, not one key, so a key can be rotated. A compromised private key lets someone **forge**
  licenses; it does not expose any model. Response: drop the key from the list in the next release.

### 7.4 Per-licensee builds

Each license gets its own build. The license ID is embedded **inside** the WASM as well as in the
header — in a custom section and as harmless, deterministic variation in code layout derived from
the ID — so a copy whose header was stripped and re-wrapped is still identifiable.

**Rejected: numeric watermarking** (perturbing constants per licensee). It would make two
licensees' results differ, which breaks reproducibility and every golden comparison.

### 7.5 No encryption

The WASM is not encrypted. The key would have to be in the open-source simulator, so encryption
would add complexity and a false impression of protection and nothing else. Compilation (§4) and
attribution (§7.4) are the real measures.

### 7.6 Personal data

`licensee_display` is whatever the license agreement says may be shown, and may be empty. The
mapping from `license_id` to a person or institution stays in our private license register. This
keeps personal data out of files designed to be copied (GDPR data minimisation).

---

## 8. The contract (outline — not legal advice)

The technical measures produce evidence; the contract is the enforcement. To be drafted with the
University of Trento's technology-transfer or legal office, not from this document. Points to raise
with them:

- counterparty: the licensee's **institution** as well as the individual;
- permitted use; no redistribution; no reverse engineering, decompilation or circumvention; no
  characterisation intended to reproduce the model;
- confidentiality surviving termination; return or destruction on termination;
- what the `license_id` in a file evidences, and that the licensee is responsible for copies bearing
  it;
- remedies (possibly liquidated damages, so a breach does not require proving loss);
- whether the University or the author is the licensor.

---

## 9. Diagnostics

New codes, allocated in the implementing PRs with English entries in `messages/en/` (CLAUDE.md §5):

- **errors:** bad signature; unknown signing key; hash mismatch; malformed container; unknown
  format/ABI version; module trap; fuel exhausted; memory limit; parameter out of range;
- **refusals** (on our side, at issue time): a construct the WASM backend cannot lower; a disallowed
  system task in a protected model;
- **communication:** the licensee banner.

A protected model's diagnostics never include source text, line numbers or names — the module has
none to give. Errors say which **model** and which **license** failed and why, which is what the
user needs to contact us.

---

## 10. Validation

Before any `.vam` is issued, on our side:

1. **Bit-identity with the JIT path:** the WASM model and the same model through `va-codegen`'s JIT
   produce identical stamps at a sweep of operating points. This depends on §5.2's host maths.
2. **Finite differences** (CLAUDE.md §5): central-difference Jacobian check on the WASM model's own
   output, not only on the source model.
3. **Golden comparison** against QSPICE where applicable — with the goldens kept in our **private**
   model repository, never in this repository's `golden/`.

In this repository: the loader is validated with hand-written WAT versions of the `va-abi`
reference resistor, capacitor and diode, which must match the reference models exactly. That needs
no proprietary material.

---

## 11. Crates and interfaces

- **Interface β is unchanged.** `va-wasm-model` implements `ModelInstance` like any other model.
  If implementation finds β needs something (say, a way to report the license for the banner),
  that becomes a separate §6 proposal.
- **New crate `va-wasm-model`**, depending on `va-abi` and `va-diag` only. It needs a row in
  CLAUDE.md §3's table and `docs/thesis-map.md` (proposed owner: staff, like `va-core`).
- **`va-codegen` (T2):** the WASM backend, behind a cargo feature so a normal build carries none of
  it.
- **`va-netlist` / `va-cli` (T6):** syntax to reference a `.vam` (e.g. `.model` pointing at the file),
  and the banner/output headers.
- **`xtask`:** `license issue` (needs the offline key) and `license inspect` (prints a header without
  verifying, for support).

---

## 12. Alternatives considered

| Alternative | Why not |
|---|---|
| Encrypted `.va` / IEEE 1735 in the open-source tool | The key would be public. Advertises protection that does not exist (§1) |
| Native shared libraries (`.so`/`.dll`, OSDI-style) | Arbitrary code execution on the user's machine; `unsafe` FFI; per-platform builds; conflicts with CLAUDE.md §5's native-link rule |
| Interpreter + embedded IR | Ships the model in near-readable form (§4.1) |
| License check inside the WASM only | Patchable out of the module just as the host check is; adds nothing over §7.2 |
| Numeric watermarking | Breaks reproducibility (§7.4) |
| Hosted simulation (model never leaves our server) | The only option that truly keeps the model secret. Costs infrastructure and requires users to send us their circuits. Kept as the answer for any model that must not be reverse-engineered at all |
| Shipping a table or surrogate model instead | Complementary, not a replacement: the right choice when the behaviour alone is what we want to share |

---

## 13. Stages

Each stage is its own PR with its own `release.txt` entry.

1. **Ratify this proposal** with T2 and T6. Exit: §11's crate row agreed.
2. **`va-wasm-model` on `wasmi`** with the ABI of §5, loading hand-written WAT resistor/capacitor/
   diode. Exit: exact stamp match with `va-abi::reference`; fuel and memory limits tested;
   `cargo deny check` green.
3. **`.vam` container, signature check, banner**, with a test key generated inside the test (never a
   committed private key). Exit: tampered header, tampered WASM, wrong key, expired license and
   unknown version each refused with their own code.
4. **WASM backend in `va-codegen`.** Exit: §10 checks 1 and 2 pass on every model in `models/`.
5. **`xtask license issue/inspect`.** Exit: an issued `.vam` of a zoo model runs a zoo circuit and
   matches the plain `.va` run bit for bit.
6. **Optional: `wasmtime`**, if `bench-scale` shows the interpreter is the bottleneck. Needs the
   `unsafe` sign-off.

---

## 14. Open questions

- Is the University or the author the licensor? (Decides who holds the signing key.)
- Should a licensee be able to **compose** protected models into their own Verilog-A modules
  (hierarchy), or only instantiate them from a netlist? The latter is much simpler and is assumed
  here.
- Should `expires` exist at all, given that it is advisory against a modified simulator or a wrong
  clock? Kept for honest users and as a contractual reference date.
- Do any planned licensed models need analog events or AC-only constructs on day one? That decides
  how much of §5.1's optional channels stage 2 must cover.
