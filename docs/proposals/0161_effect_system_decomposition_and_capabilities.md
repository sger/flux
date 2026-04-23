- Feature Name: Effect System Decomposition and Capabilities
- Start Date: 2026-04-18
- Status: Partially Implemented (2026-04-23 slice: alias expansion, compiler-seeded decomposition, strict Flow audit, and shared compiler/Aether Phase 3 registry cleanup; sealing and `Flow.Primops` deferred)
- Proposal PR:
- Flux Issue:
- Depends on: [0160](implemented/0160_static_typing_hardening_closure.md) (Static Typing Hardening Closure), [0145](0145_type_classes.md) (Type Classes), [0086](implemented/0086_backend_neutral_core_ir.md) (Backend-Neutral Core IR)
- Supersedes: [0075](superseded/0075_effect_sealing.md) (Phase 2), [0108](superseded/0108_base_function_effect_audit.md) (Phase 1.5), [0131](superseded/0131_primop_effect_levels.md) (Phase 3)

# Proposal 0161: Effect System Decomposition and Capabilities

## Summary
[summary]: #summary

Decompose the monolithic `IO` label into fine-grained labels (`Console`,
`FileSystem`, `Stdin`, `Clock`, `Random`, `Div`, `Panic`, `Exn`) and make that
decomposition available through compiler-seeded aliases plus the documented
`Flow.Effects` stdlib surface. Add call-site capability restriction (`expr
sealing { … }`) in a later phase. Derive optimizer Pure/CanFail/HasEffect
classification from the builtin effect registry rather than a hardcoded match.

This closes the three-way asymmetry where effect *labels*, effect *capability
restrictions*, and effect *optimizer levels* each live in their own ad-hoc
place. In the currently implemented slice, the operational source of truth is a
shared compiler registry plus seeded aliases; `Flow.Effects` is the user-facing
spec that must stay in sync. A later follow-up may move the executable source
of truth fully into stdlib declarations. The current implementation also routes
compiler ambient-effect checks, strict-mode missing-effect checks, and
Aether/FBIP builtin-call classification through that shared registry.

## Motivation
[motivation]: #motivation

### Today's effect system has three hardcoded layers

| Concern | Where it lives | Problem |
|---|---|---|
| Which labels exist | [`PrimEffect`](../../src/core/mod.rs#L397) enum + [`AetherBuiltinEffect`](../../src/aether/mod.rs#L42) enum + 29 `intern("IO"/"Time")` sites | Duplicated across three crates, adding a label means editing the compiler |
| What effect a primop has | Hardcoded match in [`src/core/mod.rs:763`](../../src/core/mod.rs#L763) + name-based fallback in [`src/aether/mod.rs:602`](../../src/aether/mod.rs#L602) | Source of truth lives inside the compiler, not in a user-readable stdlib |
| Whether optimizer can drop a dead primop | Binary `is_pure()` in [`src/core/passes/helpers.rs`](../../src/core/passes/helpers.rs) | 20+ primops conservatively kept alive when dead |

### Koka already solved this

In Koka, effect labels are declared in [`std/core/*.kk`](/Users/s.gerokostas/Downloads/Github/koka/lib/std/core/) as regular types, and primitives are `extern fn` with typed effect signatures. `io` is an alias over a row of fine-grained labels (`console`, `fsys`, `net`, `ui`, `blocking`, `ndet`, `div`, `exn`). The compiler only knows row-polymorphism; everything else lives in stdlib.

### Why this matters for Flux

1. **Fine-grained effect tracking.** A test helper can declare `with Console` without also claiming filesystem access.
2. **Call-site capability grants.** Once labels are decomposed, `fetch(url) sealing { Network }` is a meaningful constraint, not a tautology.
3. **Fewer compiler hotspots.** Adding an effect today means touching multiple compiler sites. This proposal consolidates those sites behind one registry and documents the labels in `Flow.Effects`, which is already a big reduction in drift.
4. **Optimizer discipline.** Dead `10 / n` and `arr[i]` bindings stop surviving into compiled output. The optimizer derives legality from the effect row, not a separate table that can drift.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### The mental model

Flux has three kinds of "effect thing", all declared in stdlib:

| Construct | Syntax | Operations? | Handleable? | Purpose |
|---|---|---|---|---|
| **Label** | `effect Console` (no body) | No | No | Track that a function performs this kind of effect |
| **Full effect** | `effect Console { print : String -> () }` | Yes | Yes (via `handle`) | User-intercepted operations |
| **Alias** | `alias IO = <Console \| FileSystem \| Stdin>` | — | — | Compose labels into named rows |

For this proposal, Phase 1 introduces the label form (labels without bodies). Phase 3 (of later proposal 0162) covers the full-effect machinery that already exists for user-declared effects.

### What changes for users

**Before:**
```flux
fn greet(name: String) with IO {
    print("hi, " + name)     // IO is one monolithic label
}
```

**After:**
```flux
// Option 1 — fine-grained:
fn greet(name: String) with Console {
    print("hi, " + name)
}

// Option 2 — use the alias (unchanged from today):
fn greet(name: String) with IO {
    print("hi, " + name)
}

// Option 3 — sealed call:
fn read_safely(path: String) -> String {
    unsafe_read(path) sealing { FileSystem }   // callee may not print, network, etc.
}
```

### What changes for the compiler

- The `PrimEffect` enum and `AetherBuiltinEffect` enum are deleted.
- `src/core/mod.rs::effect_kind()` and `src/aether/mod.rs::builtin_effect_for_name()` are replaced by a shared builtin-effect registry helper.
- Existing primop HM injection and the builtin-effect registry remain the authority in this slice; `Flow.Primops`/`extern fn` are deferred to a later follow-up.
- Existing `with IO` annotations continue to work through the `IO` alias.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Phase 1 — Compiler-seeded decomposition, documented in Flow.Effects

**Stdlib spec file: `lib/Flow/Effects.flx`**

```flux
module Flow.Effects {
    // I/O labels (phantom — no operations, tracked via row only)
    public effect Console       // print, println
    public effect FileSystem    // read_file, read_lines, write_file
    public effect Stdin         // read_stdin
    public effect Clock         // clock_now, now_ms

    // Non-determinism + randomness
    public effect Random
    public effect NonDet

    // Failure labels (used by optimizer to decide CanFail)
    public effect Div           // division by zero, index OOB
    public effect Exn           // recoverable exceptions
    public effect Panic         // intentional crash (HasEffect — cannot discard)

    // Aliases for backward compatibility and ergonomics
    public alias IO   = <Console | FileSystem | Stdin>
    public alias Time = <Clock>
}
```

This file is documentation in the current implementation. The compiler seeds
the builtin aliases and label registry programmatically, and the stdlib file is
kept as the user-facing spec.

**Deferred follow-up: `lib/Flow/Primops.flx`**

```flux
module Flow.Primops {
    public extern fn print(s: String)         -> ()             with Console
    public extern fn println(s: String)       -> ()             with Console
    public extern fn read_file(path: String)  -> String         with FileSystem
    public extern fn read_lines(path: String) -> List<String>   with FileSystem
    public extern fn write_file(p: String, s: String) -> ()     with FileSystem
    public extern fn read_stdin()             -> String         with Stdin
    public extern fn clock_now()              -> Int            with Clock
    public extern fn now_ms()                 -> Int            with Clock

    // CanFail primops — effect row carries a failure label
    public extern fn idiv(a: Int, b: Int)     -> Int            with Div
    public extern fn imod(a: Int, b: Int)     -> Int            with Div
    public extern fn index<a>(xs: List<a>, i: Int) -> a         with Div
    public extern fn array_get<a>(arr: Array<a>, i: Int) -> a   with Div

    // HasEffect without being I/O — intentional crash
    public extern fn panic<a>(msg: String)    -> a              with Panic
}
```

**New syntax**: bare `effect Name` (no body) declares a phantom label. The
`extern fn` portion of the original design is deferred from this slice.

**Compiler changes**:
- Parser: accept `effect Name` without `{ … }`.
- Seed builtin aliases (`IO = <Console | FileSystem | Stdin>`, `Time = <Clock>`) before effect-row inference so user code sees the decomposed rows uniformly.
- Delete `PrimEffect` enum ([src/core/mod.rs:397](../../src/core/mod.rs#L397)) and `AetherBuiltinEffect` enum ([src/aether/mod.rs:42](../../src/aether/mod.rs#L42)).
- Route compiler consumers through a shared builtin-effect registry helper instead of duplicating effect classification logic. This now includes ambient builtin-call checks, strict missing-effect checks for builtin aliases, and Aether/FBIP builtin-call summaries.
- Keep `Flow.Primops`/`extern fn` out of scope for this slice; existing primop HM injection remains authoritative for builtin signatures.

### Phase 1.5 — Base signature audit (absorbs 0108)

With the current compiler-seeded primop HM signatures and alias expansion in
place, walk the effect-focused `lib/Flow/*.flx` modules under strict mode and
assert that exported functions declare the effects they actually use. This
captures the intended audit without waiting for `Flow.Primops`.

- Extend the `static_typing_contract_tests.rs` harness with a `base_effect_audit` test that compiles the effect-focused Flow modules under strict mode and asserts no effect-row residue.
- Start with `Flow.Effects`, `Flow.IO`, and similar wrappers around console/file/stdin/clock primops.
- Add a CI gate: adding a new base function without an effect annotation is an error.

### Phase 2 — Effect sealing at call sites (absorbs 0075)

**Syntax:**

```flux
expr sealing { E1 | E2 | … }          // explicit allowed set
expr sealing (ambient - FileSystem)   // algebraic restriction
```

**Semantics:**
- `expr sealing R` adds a constraint: every effect emitted by `expr` must be in `R`.
- The row solver enforces this at compile time. Violations produce a new diagnostic code reserved specifically for sealing; it must not reuse `E460`, which already means Missing Named Field.
- No runtime cost. Sealing is a compile-time capability restriction, not a dynamic check.

**Interaction with Phase 1:**
- Fine-grained labels make sealing actually useful. `sealing { Console }` is meaningful after decomposition; before it's either `sealing { IO }` (tautology) or impossible (IO is everything).

**Implementation steps:**
1. Parser: `sealing { … }` postfix expression.
2. Type inference: emit a row-subset constraint at the call site.
3. Row solver: extend with `Subset<R1, R2>` constraint kind; existing absence machinery from 0049 covers most of the solving.
4. Diagnostic: introduce a dedicated sealing code with a clear "this call was sealed to allow {…}; the callee performs {…}" message.

### Phase 3 — Optimizer levels from effect rows (rewrites 0131)

The optimizer's Pure/CanFail/HasEffect classification becomes:

```rust
pub enum PrimOpEffect { Pure, CanFail, HasEffect }

pub fn primop_effect(op: &CorePrimOp, registry: &BuiltinEffectRegistry) -> PrimOpEffect {
    let row = registry.effect_row(op);
    if row.is_empty() {
        PrimOpEffect::Pure
    } else if row.iter().all(|l| is_failure_label(l)) {
        // Only Div, Exn — callee can fail but has no observable side effect
        PrimOpEffect::CanFail
    } else {
        // Any I/O label, or Panic (intentional crash)
        PrimOpEffect::HasEffect
    }
}

fn is_failure_label(label: Symbol) -> bool {
    matches!(label.name(), "Div" | "Exn")
}
```

`Panic` stays HasEffect — an intentional crash is semantically different from accidental failure and must not be discarded. Everything else falls out of the row.

**Affected passes:**
- [`dead_let.rs`](../../src/core/passes/dead_let.rs) — use `primop_effect(op) != HasEffect` instead of `is_pure(op)` for drop legality.
- [`inliner.rs`](../../src/core/passes/inliner.rs) — same.
- [`case_of_case.rs`](../../src/core/passes/case_of_case.rs) and [`beta.rs`](../../src/core/passes/beta.rs) — continue using the narrower `PrimOpEffect::Pure` for speculation-safety.

Net win: dead `10 / n`, `arr[i]`, `type_of(x)`, etc. are eliminated. Correctness of existing passes is preserved — the `Pure`-only gate still exists, just computed from the effect row.

## Exit Criteria
[exit-criteria]: #exit-criteria

Phase 1 (decomposition) ships when:
- `Flow.Effects` documents the decomposed labels and seeded aliases.
- `PrimEffect` and `AetherBuiltinEffect` enums are deleted, and compiler consumers route through the shared builtin-effect registry.
- The old scattered `intern("IO"/"Time")` call sites are collapsed behind the registry and alias seeding helpers.
- `with IO` still compiles (through the alias).
- `tests/static_typing_closure.rs` and the full VM/LLVM parity sweep remain green.

Phase 1.5 (base audit) ships when:
- `base_effect_audit` passes on the effect-focused `lib/Flow/` modules covered by this slice.
- No audited Flow export has a missing or mismatched effect annotation.
- Known temporary boundary: strict mode still requires `with IO` on wrappers that call builtin `print`/`println`, even when their logical decomposed label is only `Console`. This remains acceptable in the current slice and should be retired when the builtin primop surface is fully decomposed.

Phase 2 (sealing) ships when:
- `expr sealing { … }` parses, type-checks, and rejects row violations with its dedicated sealing diagnostic.
- At least five fixture tests in `examples/sealing/` covering allow, deny, algebraic subtraction, nested seals, and polymorphic callees.

Phase 3 (optimizer levels) ships when:
- `primop_effect` derives from `BuiltinEffectRegistry`.
- Aether/FBIP and compiler builtin-call effect checks use the same shared registry helpers rather than local string matches.
- `dead_let` and `inliner` eliminate dead `Div`-tagged primops; `HasEffect` primops continue to survive.
- No regression in parity sweep; measurable shrinkage in `--dump-core` output on programs with exploratory computation.

## Drawbacks
[drawbacks]: #drawbacks

- Adds one new syntactic form in this slice (`effect Name` without body). `extern fn` remains deferred.
- Users who wrote `with IO` get an implicit alias resolution; the displayed effect row in diagnostics will show the decomposed form. This is an observable (but benign) change in error messages.
- Sealing adds a new diagnostic code and a new row-solver constraint kind. Both fit the existing machinery.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- **Why not lift every primitive to a handleable effect (0099 Part 1 literal reading)?** Because most I/O primitives don't benefit from handleability. Koka makes the same call: `console`/`fsys`/`net`/`ui`/`blocking` are phantom labels (`:: X`). Only `exn`, `random`, `parse`, `utc` carry operations. A blanket lift pays for evidence-passing on every `print`.
- **Why not leave labels hardcoded and only add sealing?** Sealing without decomposition is nearly useless — `sealing { IO }` grants everything. Decomposition is the content of the feature.
- **Why not keep 0131 separate?** Because after Phase 1 lands, the optimizer's classification is a five-line derivation from effect rows, not a 200-line match. Separating them would mean implementing a hardcoded table now that gets thrown away next quarter.
- **Why defer `Flow.Primops` and `extern fn`?** Because the current slice already delivers the decomposition and optimizer cleanup using existing primop HM injection. Pulling `extern fn` into the same batch would turn a focused cleanup into a larger parser/stdlib refactor.
- **Why keep some `with IO` annotations after narrowing helpers to `Console`?** Because strict mode still treats builtin `print`/`println` through the legacy `IO` boundary. Narrowing assertion helpers to `Console` is still worthwhile, but wrapper functions that directly print must keep `with IO` until builtin declarations become the executable source of truth.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- **Should `NonDet` be a phantom label or a handleable effect?** Koka treats `ndet` as phantom. Flux could follow, or lift it for PBT-style seeded determinism. Decision: phantom in Phase 1; revisit if PBT lands.
- **Should `Panic` be handleable?** Koka's `exn` is. That would let users install a top-level panic handler. Tempting but not for this proposal; tracked separately.
- **Row polymorphism in sealing.** `sealing (ambient - E)` requires algebraic row subtraction. The 0049 machinery supports `Absent<E>`; this proposal just wires it to the sealing syntax. Edge cases (sealing a row variable) need fixture coverage.
- **Migration tooling.** `with IO` in existing code keeps working via the alias, but a `--fix` flag that rewrites `with IO` to the fine-grained row where known could speed adoption.
- **Builtin print boundary.** The current compiler-seeded decomposition is not yet the full executable source of truth for strict-mode builtin checking: `print`/`println` still force an `IO` annotation in some wrappers. That gap is now documented and covered by the `Flow.FTest.describe` shape; closing it belongs to the later `Flow.Primops`/`extern fn` follow-up.

## Future possibilities
[future-possibilities]: #future-possibilities

- **Selective lift to handleable.** Once labels live in stdlib, lifting a specific label (e.g. `Random`) into a full effect with operations is a mechanical change in `Flow.Effects` + a root-handler install. No compiler work beyond the existing handler machinery.
- **Effect-parameterised libraries.** With decomposed labels, a library can write `fn serve<e>(req: Request) with <Network | e> -> Response` and callers supply the extra effects they need.
- **Capability-oriented design.** Sealing at call sites is the building block for capability-passing APIs — `fn run_untrusted(f: () -> (), caps: Console)` becomes expressible as a typed pattern.
