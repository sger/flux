- Feature Name: Base Function Effect Purity Audit
- Start Date: 2026-03-18
- Status: Superseded (2026-04-18) by [Proposal 0161 — Effect System Decomposition and Capabilities](../implemented/0161_effect_system_decomposition_and_capabilities.md) (Phase 1.5: audit absorbed once primop signatures live in `Flow.Primops`)
- Proposal PR:
- Flux Issue:
- Depends on: 0074 (base signature tightening)

# Proposal 0108: Base Function Effect Purity Audit

## Summary

Systematically audit all 77 base functions in `src/runtime/base/` to verify that every function performing side effects declares the correct effect in its HM signature, and that every function declared pure is genuinely pure. This audit produces a verified checklist, a CI-enforced test suite, and identifies functions that need effect annotation corrections — closing the last gap in Flux's static effect tracking before proposal 0099 (IO as algebraic effect) can proceed.

## Motivation

Flux's effect system enforces that user-defined functions declare their side effects (`with IO`, `with Time`, etc.). The type checker rejects code that calls an effectful function without propagating the effect. This is the foundation of Flux's purity guarantee.

However, the guarantee is only as strong as the base function signatures. If a base function performs IO but its HM signature declares `row(vec![], None)` (pure/no effects), then user code can perform side effects without any effect annotation — silently breaking the purity invariant.

### Current state

From inspecting `src/runtime/base/helpers.rs` (`signature_for_id`), the current effect declarations are:

**Functions declaring `IO`:**
- `print` — `row(vec!["IO"], None)`
- `read_file` — `row(vec!["IO"], None)`
- `read_lines` — `row(vec!["IO"], None)`
- `read_stdin` — `row(vec!["IO"], None)`

**Functions declaring `Time`:**
- `now_ms` — `row(vec!["Time"], None)`
- `time` — `row(vec!["Time"], None)`

**Functions declaring pure (no effects):**
- All other 71 functions — `row(vec![], None)` or `row(vec![], Some("e"))` (effect-polymorphic for HOFs)

### The risk

The audit must answer three questions for each of the 71 "pure" functions:

1. **Does the Rust implementation actually perform side effects?** Some functions like `type_of` or `to_string` may internally call `format!()` or access system state — this is fine if the side effect is not observable from Flux code. But if a function reads from stdin, writes to stdout, accesses the filesystem, reads the clock, generates random numbers, or mutates global state, it must declare an effect.

2. **Do assert functions need a `Test` effect?** The 10 assert functions (`assert_eq`, `assert_neq`, `assert_true`, `assert_false`, `assert_throws`, `assert_msg`, `assert_gt`, `assert_lt`, `assert_gte`, `assert_lte`, `assert_len`) currently declare no effects but they panic/abort on failure. In a pure FP model, aborting is a side effect. Koka models this as a `div` (divergence) effect. Flux could introduce a `Test` or `Fail` effect, or accept that assertion failure is a controlled panic outside the effect system.

3. **Does `try` need an effect?** `try` catches runtime errors, which is exception handling — an effect in Koka's model (`exn`). Currently declared pure.

### Why this matters

Proposal 0099 plans to make `IO` a first-class algebraic effect and migrate all base functions to use `perform IO.*`. Before that migration, we need a verified baseline: which functions are correctly annotated today, which need corrections, and which need new effect categories.

Without this audit, 0099 may miss functions that should be migrated, or incorrectly migrate pure functions to use `IO`.

## Guide-level explanation

This proposal produces three deliverables:

### 1. Effect classification checklist

Every base function is classified into one of these categories:

| Category | Meaning | Example |
|----------|---------|---------|
| **Pure** | No observable side effects. Deterministic output for same input. | `len`, `first`, `push`, `reverse` |
| **IO** | Reads from or writes to external world (stdout, filesystem, stdin). | `print`, `read_file`, `read_stdin` |
| **Time** | Reads the system clock. Non-deterministic but no external mutation. | `now_ms`, `time` |
| **Fail** | May abort execution (panic, assertion failure). | `assert_eq`, `assert_true` |
| **Exn** | Catches runtime errors (exception handling). | `try` |
| **Effect-polymorphic** | Propagates effects from callback arguments. | `map`, `filter`, `fold` |

### 2. Audit findings table

| # | Function | Current Effect | Actual Effect | Status | Action |
|---|----------|---------------|---------------|--------|--------|
| 1 | `print` | IO | IO | Correct | None |
| 2 | `len` | Pure | Pure | Correct | None |
| 3 | `first` | Pure | Pure | Correct | None |
| 4 | `last` | Pure | Pure | Correct | None |
| 5 | `rest` | Pure | Pure | Correct | None |
| 6 | `push` | Pure | Pure | Correct | None |
| 7 | `to_string` | Pure | Pure | Correct | None |
| 8 | `concat` | Pure | Pure | Correct | None |
| 9 | `reverse` | Pure | Pure | Correct | None |
| 10 | `contains` | Pure | Pure | Correct | None |
| 11 | `slice` | Pure | Pure | Correct | None |
| 12 | `sort` | Pure | Pure | Correct | None |
| 13 | `split` | Pure | Pure | Correct | None |
| 14 | `join` | Pure | Pure | Correct | None |
| 15 | `trim` | Pure | Pure | Correct | None |
| 16 | `upper` | Pure | Pure | Correct | None |
| 17 | `lower` | Pure | Pure | Correct | None |
| 18 | `starts_with` | Pure | Pure | Correct | None |
| 19 | `ends_with` | Pure | Pure | Correct | None |
| 20 | `replace` | Pure | Pure | Correct | None |
| 21 | `chars` | Pure | Pure | Correct | None |
| 22 | `substring` | Pure | Pure | Correct | None |
| 23 | `keys` | Pure | Pure | Correct | None |
| 24 | `values` | Pure | Pure | Correct | None |
| 25 | `has_key` | Pure | Pure | Correct | None |
| 26 | `merge` | Pure | Pure | Correct | None |
| 27 | `delete` | Pure | Pure | Correct | None |
| 28 | `abs` | Pure | Pure | Correct | None |
| 29 | `min` | Pure | Pure | Correct | None |
| 30 | `max` | Pure | Pure | Correct | None |
| 31 | `type_of` | Pure | Pure | Correct | None — inspects runtime tag, no IO |
| 32 | `is_int` | Pure | Pure | Correct | None |
| 33 | `is_float` | Pure | Pure | Correct | None |
| 34 | `is_string` | Pure | Pure | Correct | None |
| 35 | `is_bool` | Pure | Pure | Correct | None |
| 36 | `is_array` | Pure | Pure | Correct | None |
| 37 | `is_hash` | Pure | Pure | Correct | None |
| 38 | `is_none` | Pure | Pure | Correct | None |
| 39 | `is_some` | Pure | Pure | Correct | None |
| 40 | `map` | Effect-poly | Effect-poly | Correct | None |
| 41 | `filter` | Effect-poly | Effect-poly | Correct | None |
| 42 | `fold` | Effect-poly | Effect-poly | Correct | None |
| 43 | `hd` | Pure | Pure | Correct | None |
| 44 | `tl` | Pure | Pure | Correct | None |
| 45 | `is_list` | Pure | Pure | Correct | None |
| 46 | `to_list` | Pure | Pure | Correct | None |
| 47 | `to_array` | Pure | Pure | Correct | None |
| 48 | `put` | Pure | Pure | Correct | None |
| 49 | `get` | Pure | Pure | Correct | None |
| 50 | `is_map` | Pure | Pure | Correct | None |
| 51 | `list` | Pure | Pure | Correct | None |
| 52 | `read_file` | IO | IO | Correct | None |
| 53 | `read_lines` | IO | IO | Correct | None |
| 54 | `read_stdin` | IO | IO | Correct | None |
| 55 | `parse_int` | Pure | Pure | Correct | None — pure string parsing |
| 56 | `now_ms` | Time | Time | Correct | None |
| 57 | `time` | Time | Time | Correct | None |
| 58 | `range` | Pure | Pure | Correct | None |
| 59 | `sum` | Pure | Pure | Correct | None |
| 60 | `product` | Pure | Pure | Correct | None |
| 61 | `parse_ints` | Pure | Pure | Correct | None |
| 62 | `split_ints` | Pure | Pure | Correct | None |
| 63 | `flat_map` | Effect-poly | Effect-poly | Correct | None |
| 64 | `any` | Effect-poly | Effect-poly | Correct | None |
| 65 | `all` | Effect-poly | Effect-poly | Correct | None |
| 66 | `find` | Effect-poly | Effect-poly | Correct | None |
| 67 | `sort_by` | Effect-poly | Effect-poly | Correct | None |
| 68 | `zip` | Pure | Pure | Correct | None |
| 69 | `flatten` | Pure | Pure | Correct | None |
| 70 | `count` | Effect-poly | Effect-poly | Correct | None |
| 71 | `assert_eq` | Pure | **Fail** | **MISMATCH** | Decide: add `Fail` effect or accept |
| 72 | `assert_neq` | Pure | **Fail** | **MISMATCH** | Decide: add `Fail` effect or accept |
| 73 | `assert_true` | Pure | **Fail** | **MISMATCH** | Decide: add `Fail` effect or accept |
| 74 | `assert_false` | Pure | **Fail** | **MISMATCH** | Decide: add `Fail` effect or accept |
| 75 | `assert_throws` | Effect-poly | **Effect-poly + Fail** | **MISMATCH** | Decide: add `Fail` to row |
| 76 | `assert_msg` | Pure | **Fail** | **MISMATCH** | Decide: add `Fail` effect or accept |
| 77 | `try` | Effect-poly | **Effect-poly + Exn** | **REVIEW** | Decide: model exception catching as effect |
| 78 | `str_contains` | Pure | Pure | Correct | None |
| 79 | `assert_gt` | Pure | **Fail** | **MISMATCH** | Decide: add `Fail` effect or accept |
| 80 | `assert_lt` | Pure | **Fail** | **MISMATCH** | Decide: add `Fail` effect or accept |
| 81 | `assert_gte` | Pure | **Fail** | **MISMATCH** | Decide: add `Fail` effect or accept |
| 82 | `assert_lte` | Pure | **Fail** | **MISMATCH** | Decide: add `Fail` effect or accept |
| 83 | `assert_len` | Pure | **Fail** | **MISMATCH** | Decide: add `Fail` effect or accept |

### 3. CI test suite

A dedicated test file `tests/base_effect_audit_tests.rs` that:
- Verifies each base function's HM signature matches the expected effect classification.
- Prevents regressions: adding a new base function without an effect classification fails the test.
- Serves as the living audit document.

## Reference-level explanation

### Decision required: Assert functions and the `Fail` effect

The 10 assert functions are the primary finding. There are three approaches:

**Option A: Accept panics as outside the effect system (recommended for now).**

Assertion failures are programmer errors, not recoverable effects. Like Rust's `panic!()`, they indicate a bug, not a program state that a handler should intercept. Keep assert functions as pure.

Rationale: Koka's `div` (divergence) effect is useful for totality checking but adds annotation burden to test code. Flux's test runner already expects assertion panics. Adding a `Fail` effect would require every test function to declare `with Fail`, which is pure ceremony.

**Option B: Introduce a `Fail` effect for assertions.**

```flux
effect Fail {
    fail: String -> Never
}

// assert_eq would become:
fn assert_eq(a: Any, b: Any) -> Unit with Fail { ... }

// Test functions would need:
fn test_math() with Fail {
    assert_eq(1 + 1, 2)
}
```

This is more principled but adds friction to the test framework.

**Option C: Introduce a `Fail` effect but auto-handle it in test runner.**

The `--test` runner implicitly handles `Fail` for all `test_*` functions, so test authors don't need to write `with Fail`. This keeps the effect system honest without burdening test ergonomics.

### Decision required: `try` and exception catching

`try` wraps a thunk and catches runtime errors, returning a `(String, Any)` tuple. In Koka's model, this is the `exn` effect handler. Two approaches:

**Option A: Keep `try` as effect-polymorphic only (recommended for now).**

`try` is a controlled escape hatch. Its effect-polymorphic signature correctly propagates effects from the callback. The exception-catching behavior is a runtime mechanism, not an algebraic effect in Flux's current model.

**Option B: Model exception catching as an `Exn` effect.**

This requires making runtime errors algebraic effects, which is a much larger change that overlaps with proposal 0099's IO-as-algebraic-effect work.

### Implementation: CI test suite

```rust
// tests/base_effect_audit_tests.rs

use flux::runtime::base::helpers::{signature_for_id, BaseHmSignatureId};

/// Verify that every base function's effect signature matches the audited classification.
#[test]
fn base_function_effects_match_audit() {
    let pure_functions = vec![
        BaseHmSignatureId::Len,
        BaseHmSignatureId::First,
        BaseHmSignatureId::Last,
        // ... all 51 pure functions
    ];

    let io_functions = vec![
        BaseHmSignatureId::Print,
        BaseHmSignatureId::ReadFile,
        BaseHmSignatureId::ReadLines,
        BaseHmSignatureId::ReadStdin,
    ];

    let time_functions = vec![
        BaseHmSignatureId::NowMs,
        BaseHmSignatureId::Time,
    ];

    for id in pure_functions {
        let sig = signature_for_id(id);
        assert!(
            sig.effects.concrete.is_empty() && sig.effects.tail.is_none(),
            "Base function {:?} should be pure but declares effects {:?}",
            id, sig.effects
        );
    }

    for id in io_functions {
        let sig = signature_for_id(id);
        assert!(
            sig.effects.concrete.contains(&"IO"),
            "Base function {:?} should declare IO effect",
            id
        );
    }

    for id in time_functions {
        let sig = signature_for_id(id);
        assert!(
            sig.effects.concrete.contains(&"Time"),
            "Base function {:?} should declare Time effect",
            id
        );
    }
}

/// Ensure every BaseHmSignatureId variant is covered by the audit.
#[test]
fn all_base_functions_audited() {
    // Count total variants vs audited variants
    // Fails if a new base function is added without being classified
}
```

### Implementation: Rust-level purity verification

For each "pure" base function, verify the Rust implementation does not:
- Call `println!()`, `print!()`, `eprintln!()`, or any stdout/stderr write.
- Call `std::fs::*` or `std::io::*` (file/network IO).
- Call `std::time::*` or `Instant::*` (clock reads).
- Call `rand::*` or any RNG.
- Mutate static/global state.

This can be enforced by code review or by a `#[deny(unused_io)]` style lint (future possibility).

## Drawbacks

1. **Maintenance burden:** Every new base function requires updating the audit checklist and CI test. This is intentional — it forces effect classification at definition time.

2. **Assert decision deferred:** This proposal identifies the assert/Fail mismatch but recommends deferring the fix. This leaves a known impurity in the effect system until a decision is made.

3. **No automated Rust-level verification:** The Rust purity check is manual. A static analysis tool could automate this but is out of scope.

## Rationale and alternatives

### Why a dedicated audit proposal?

Proposal 0099 covers the migration of IO base functions to algebraic effects but does not include a systematic audit of all 77 functions. The audit must happen first to establish the baseline. Bundling it into 0099 would make that proposal even larger.

### Why not just grep for effects?

Grepping `signature_for_id` shows the declared effects but does not verify they match reality. The audit cross-references the Rust implementation with the HM signature to find mismatches.

### What if we skip the audit?

Without the audit, 0099's migration may:
- Miss functions that should gain `IO` (if a "pure" function actually does IO).
- Incorrectly migrate pure functions (wasting engineering effort).
- Leave the assert/Fail question unresolved, creating confusion about the purity boundary.

## Prior art

- **Haskell:** The `base` library's type signatures are the canonical effect documentation. Every IO function is in the `IO` monad. Pure functions are guaranteed pure by the type system. GHC's `unsafePerformIO` is the escape hatch, explicitly marked unsafe.
- **Koka:** Built-in functions declare their effects in the standard library. `println` has effect `io`, `random-int` has effect `ndet`, etc. The effect system is the source of truth.
- **PureScript:** The `Effect` type tracks all side effects. The standard library is systematically annotated. New FFI bindings require explicit effect declarations.

## Unresolved questions

1. **Assert functions: Fail effect or not?** This proposal recommends Option A (accept panics as outside the effect system) for now, but this should be revisited when Flux gets a more complete effect hierarchy.

2. **`try` and exception semantics:** Should `try` be modeled as an effect handler? This depends on whether Flux wants to make runtime errors algebraic effects (a larger question for 0099).

3. **Future base functions:** Should there be a lint or CI check that prevents adding a new base function without an effect classification? This proposal recommends yes, via the `all_base_functions_audited` test.

4. **`type_of` and reflection:** `type_of` inspects the runtime Value tag. Is runtime type inspection a side effect? In Koka's model, no — it reads information that is structurally present. In a parametrically polymorphic language (System F), it would violate parametricity. Flux is not parametrically polymorphic (it has `type_of`), so this is consistent.

## Future possibilities

- **`Fail` effect for totality:** If Flux adds totality checking (proving functions terminate and are total), assertion failure and division by zero would need `Fail`/`div` effects. This audit's classification would be the starting point.
- **`Ndet` (nondeterminism) effect:** If Flux adds `random()` or similar, it would need a `Ndet` effect. The audit framework would catch this.
- **Automated Rust purity lint:** A custom clippy lint that flags IO operations inside functions registered as pure base functions.
- **Effect coverage metric:** Report the percentage of base functions with verified effect annotations as a project health metric.
