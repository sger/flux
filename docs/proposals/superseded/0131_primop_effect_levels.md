- Feature Name: PrimOp Effect Levels
- Start Date: 2026-03-26
- Status: Partially Implemented, remainder Superseded (2026-04-18) by [Proposal 0161 — Effect System Decomposition and Capabilities](../implemented/0161_effect_system_decomposition_and_capabilities.md) (Phase 3: derive Pure/CanFail/HasEffect from effect-row category instead of a hardcoded match)
- Proposal PR:
- Flux Issue:
- Depends on: None (standalone optimizer improvement)

# Proposal 0130: PrimOp Effect Levels

## Summary

Replace the binary `is_pure() -> bool` classification of primitive operations with a three-level effect system: `Pure`, `CanFail`, and `HasEffect`. This enables the optimizer to eliminate dead bindings involving division, array access, string operations, and type inspection — all currently kept alive because they are conservatively marked impure.

## Motivation

### The current model

Every `CorePrimOp` is classified by a single function in `core/passes/helpers.rs`:

```rust
fn is_primop_pure(op: &CorePrimOp) -> bool {
    match op {
        CorePrimOp::IAdd | CorePrimOp::ISub | ... => true,
        // Everything else:
        CorePrimOp::IDiv | CorePrimOp::TypeOf | CorePrimOp::Print | ... => false,
    }
}
```

This function drives two optimizer passes:
- **Dead let elimination** (`dead_let.rs`): drops `let x = <rhs>` when `x` is unused and `rhs` is pure
- **Inlining** (`inliner.rs`): inlines or drops bindings with zero use count when the RHS is pure

The problem: `false` (impure) conflates three very different situations.

### Three different kinds of "impure"

**1. Can fail, but no side effect** — Division, array indexing, generic arithmetic under gradual typing:

```flux
let x = 10 / n        // IDiv — crashes on n=0, but has no side effect
let y = arr[i]         // ArrayGet — crashes on out-of-bounds
let z = a + b          // Add — may type-mismatch under gradual typing
42                     // x, y, z unused — all three could be safely discarded
```

If the result is unused, discarding the operation is safe: either the operation would have succeeded (no observable effect lost) or it would have crashed (the crash is averted, which is strictly better). The optimizer currently keeps all three.

**2. Observable side effect** — IO operations that change the world:

```flux
let _ = print("hello")   // Print — writes to stdout, must execute
let _ = write_file(p, s) // WriteFile — modifies filesystem, must execute
42
```

These must never be discarded, even if the result is unused. The side effect IS the purpose.

**3. Raises an exception as its purpose** — `panic` exists to crash:

```flux
let _ = panic("invariant violated")   // must execute — the crash is the intent
42
```

`panic` is different from `IDiv`: division crashes *accidentally* (the programmer hoped it would succeed), while `panic` crashes *intentionally*. Discarding a `panic` changes program semantics.

### What the optimizer loses today

The binary model forces conservative choices. These primops are all marked `false` (impure), preventing dead-code elimination when their results are unused:

| PrimOp | Actual effect | Should discard unused? | Current |
|--------|---------------|----------------------|---------|
| `IDiv`, `IMod`, `FDiv` | CanFail (div by zero) | Yes | Kept |
| `Div`, `Mod`, `Add`, `Sub`, `Mul` | CanFail (type mismatch) | Yes | Kept |
| `Index`, `ArrayGet` | CanFail (out of bounds) | Yes | Kept |
| `Lt`, `Gt`, `Le`, `Ge` | CanFail (incomparable types) | Yes | Kept |
| `TypeOf`, `IsInt`, `IsFloat`, `IsBool` | Pure (type inspection) | Yes | Kept |
| `StringLength`, `Len`, `HamtSize` | Pure (read-only query) | Yes | Kept |
| `CmpEq`, `CmpNe`, `HamtContains` | Pure (comparison) | Yes | Kept |
| `StringConcat`, `ToString`, `Trim` | Pure (value construction) | Yes | Kept |
| `Print`, `ReadFile`, `WriteFile` | HasEffect (IO) | No | Kept |
| `Panic` | HasEffect (intentional crash) | No | Kept |

The first three groups (20+ primops) are incorrectly preserved when dead.

### Impact on optimization

This matters most in code that performs exploratory computation:

```flux
fn process(data) {
    let total = length(data)           // Len — pure, but marked impure
    let ratio = total / batch_size     // IDiv — can fail, but safe to discard
    let tag = type_of(data)            // TypeOf — pure, but marked impure
    // ... programmer changes their mind, only uses `data` directly
    transform(data)
}
```

All three dead bindings survive into the compiled output. With three-level effects, all three are eliminated.

## Guide-level explanation

### The three effect levels

Every primitive operation in Flux is classified into one of three levels:

| Level | Meaning | Example | Dead binding? |
|-------|---------|---------|---------------|
| `Pure` | Cannot fail, no side effects. Always produces the same result for the same inputs. | `IAdd`, `Eq`, `MakeList`, `TypeOf`, `StringLength` | Eliminated |
| `CanFail` | May crash at runtime on bad input, but has no side effect if it succeeds. | `IDiv`, `Index`, `ArrayGet`, `Add`, `Lt` | Eliminated |
| `HasEffect` | Has an observable side effect (IO, intentional crash). | `Print`, `WriteFile`, `Panic`, `ClockNow` | Kept |

The key insight: **a dead `CanFail` operation is safe to discard.** If the operation would have succeeded, nothing is lost (no side effect). If it would have failed, the crash is averted — which is strictly better than crashing on a value nobody uses.

### What this means for Flux programmers

Nothing changes in the language itself. This is a compiler-internal optimization. Programs produce the same results. The only observable difference is:
- Slightly faster code (fewer unnecessary operations)
- Slightly smaller compiled output (fewer dead bindings in Core IR)

### What this means for the optimizer

The `is_pure` check in dead-let elimination and inlining changes from:

```
Can discard? = is_pure(rhs)
```

To:

```
Can discard? = effect_level(rhs) != HasEffect
```

This is a strictly weaker condition — everything that was eliminated before is still eliminated, plus `CanFail` operations are now also eliminated when dead.

A second distinction becomes available for future optimization passes:

```
Can speculate? = effect_level(rhs) == Pure
```

**Speculation** means moving an operation before a branch that might not execute it. This is only safe for `Pure` operations — speculating a `CanFail` operation could introduce a crash on a code path that the original program never took.

## Reference-level explanation

### New enum in `core/passes/helpers.rs`

```rust
/// Effect classification for primitive operations.
///
/// Determines what optimizer transformations are legal:
/// - Pure:      discard, speculate, duplicate — all safe
/// - CanFail:   discard, duplicate — safe; speculate — unsafe
/// - HasEffect: none safe — must preserve execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PrimOpEffect {
    /// No effects. Always produces same result for same inputs.
    /// Safe to discard, speculate, and duplicate.
    Pure = 0,
    /// May fail at runtime (division by zero, out of bounds, type mismatch).
    /// Safe to discard (if unused) and duplicate, but NOT to speculate.
    CanFail = 1,
    /// Observable side effect (IO, intentional crash).
    /// Must be preserved — never discard, speculate, or duplicate.
    HasEffect = 2,
}
```

The `Ord` derivation is intentional — `Pure < CanFail < HasEffect` — so combining effects is just `max(a, b)`.

### Classification of all CorePrimOp variants

```rust
pub fn primop_effect(op: &CorePrimOp) -> PrimOpEffect {
    use CorePrimOp::*;
    use PrimOpEffect::*;
    match op {
        // ── Pure: typed arithmetic (proven types, cannot mismatch) ──
        IAdd | ISub | IMul | FAdd | FSub | FMul => Pure,

        // ── Pure: boolean/equality ──
        And | Or | Not | Eq | NEq => Pure,

        // ── Pure: constructors ──
        MakeList | MakeArray | MakeTuple | MakeHash
        | Concat | Interpolate => Pure,

        // ── Pure: type inspection (read-only, cannot fail) ──
        TypeOf | IsInt | IsFloat | IsString | IsBool
        | IsArray | IsNone | IsSome | IsList | IsMap => Pure,

        // ── Pure: read-only queries ──
        StringLength | StringConcat | ToString | Len
        | CmpEq | CmpNe | HamtSize | HamtContains
        | Trim | Upper | Lower | StartsWith | EndsWith
        | Replace | Chars | StrContains | Split | Join => Pure,

        // ── Pure: conversion (always succeeds) ──
        ToList | ToArray => Pure,

        // ── CanFail: division (zero divisor) ──
        Div | IDiv | FDiv | Mod | IMod => CanFail,

        // ── CanFail: generic arithmetic (type mismatch under gradual typing) ──
        Add | Sub | Mul | Neg => CanFail,

        // ── CanFail: comparisons (incomparable types) ──
        Lt | Le | Gt | Ge => CanFail,

        // ── CanFail: indexing (out of bounds, missing key) ──
        Index | ArrayGet | ArraySet | ArrayPush | ArraySlice
        | ArrayConcat | ArrayLen => CanFail,

        // ── CanFail: hash map access (missing key) ──
        HamtGet | HamtSet | HamtDelete | HamtKeys
        | HamtValues | HamtMerge => CanFail,

        // ── CanFail: string slicing (out of bounds) ──
        StringSlice | Substring => CanFail,

        // ── CanFail: parsing (malformed input) ──
        ParseInt => CanFail,

        // ── HasEffect: IO ──
        Print | Println | ReadFile | WriteFile
        | ReadStdin | ClockNow => HasEffect,

        // ── HasEffect: intentional crash ──
        Panic => HasEffect,

        // ── HasEffect: exception handling ──
        Try | AssertThrows => HasEffect,

        // ── CanFail: member/tuple access (may fail on wrong shape) ──
        MemberAccess(_) | TupleField(_) => CanFail,
    }
}
```

### Changes to `is_pure` and `is_expr_effect`

The existing `is_pure` function is replaced with a more general `expr_effect`:

```rust
/// Returns the effect level of an expression.
pub fn expr_effect(expr: &CoreExpr) -> PrimOpEffect {
    match expr {
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => PrimOpEffect::Pure,
        CoreExpr::Lam { .. } => PrimOpEffect::Pure,
        CoreExpr::Con { fields, .. } | CoreExpr::Reuse { fields, .. } => {
            fields.iter().map(expr_effect).max().unwrap_or(PrimOpEffect::Pure)
        }
        CoreExpr::PrimOp { op, args, .. } => {
            let op_effect = primop_effect(op);
            let arg_effect = args.iter().map(expr_effect).max().unwrap_or(PrimOpEffect::Pure);
            op_effect.max(arg_effect)
        }
        _ => PrimOpEffect::HasEffect, // App, Let, LetRec, Case, Perform, Handle, Return
    }
}

/// Backward-compatible wrapper: returns true when an expression
/// can be safely speculated (moved before a branch).
pub fn is_pure(expr: &CoreExpr) -> bool {
    expr_effect(expr) == PrimOpEffect::Pure
}

/// Returns true when an expression can be safely discarded if unused.
pub fn is_discardable(expr: &CoreExpr) -> bool {
    expr_effect(expr) != PrimOpEffect::HasEffect
}
```

### Changes to optimizer passes

**`dead_let.rs`** — widen the elimination check:

```rust
// Before:
if is_pure(&rhs) && !appears_free(var.id, &body) {
    return *body;
}

// After:
if is_discardable(&rhs) && !appears_free(var.id, &body) {
    return *body;
}
```

**`inliner.rs`** — same change for zero-use bindings:

```rust
// Before:
if count == 0 && is_pure(&rhs) {
    return *body;
}

// After:
if count == 0 && is_discardable(&rhs) {
    return *body;
}
```

**No changes to other passes.** The `case_of_case` and `beta_reduce` passes that need speculation safety continue to use `is_pure()`, which still means `Pure` only.

### Interaction with Aether (Perceus RC)

Aether's borrowing elision already has its own classification (`primop_borrows_args` in `primop/mod.rs`). The effect level is orthogonal — borrowing is about ownership, effects are about optimization legality. No changes needed to Aether.

### Interaction with the algebraic effect system

The `HasEffect` level covers IO primops that are already tracked by the `with IO` effect annotation. When Proposal 0099 (Static Purity Completion) promotes IO to a first-class algebraic effect, the `HasEffect` primops become `Perform` nodes in Core IR and are no longer direct primops. The effect level system remains valid — it just has fewer `HasEffect` entries.

## Drawbacks

**Discarding a `CanFail` operation hides a latent bug.** If a programmer writes `let x = 10 / 0` and never uses `x`, the current compiler crashes at runtime (useful signal). With this change, the division is eliminated silently. The programmer's bug (dividing by zero) is still present conceptually but never manifests.

Counter-argument: this is the standard behavior in optimizing compilers. The division was dead code — the programmer's intent was clearly not to use the result. Crashing on dead code is surprising, not helpful.

**Classification disputes.** Some primops sit on the boundary:
- `ArraySet` — returns a new array (functional update), but is it `CanFail` or `Pure`? If the index is out of bounds, it fails. Classification: `CanFail`.
- `HamtGet` — returns `Option`, so it "can't fail" in the crash sense. But under gradual typing the key comparison might type-mismatch. Classification: `CanFail` (conservative).
- `ToList` / `ToArray` — conversion between representations. Can these ever fail? Currently no, but future array representations might have size limits. Classification: `Pure` (current semantics).

These boundary cases require careful review but are resolvable by choosing the more conservative classification.

## Rationale and alternatives

### Why three levels instead of two or four?

**Two levels (current):** Too coarse. Loses optimization opportunities on 20+ primops.

**Three levels (proposed):** Captures the meaningful distinction. `CanFail` is the only interesting new category — it's the set of operations where "discard if dead" is safe but "speculate past a branch" is not.

**Four levels** (splitting `HasEffect` into `ThrowsException` + `ReadWriteEffect`): The fourth level would allow duplicating exception-throwing code across branches while preventing duplication of IO code. Flux does not currently have a `case-of-case` pass that duplicates code into branches, and the algebraic effect system already sequences IO. The fourth level adds complexity without enabling any current optimization.

### Alternative: annotation-based

Instead of hardcoding effect levels in the compiler, primops could carry an `effect` field in their definition. This would make the classification data-driven and easier to audit. However, Flux's primop count (~60) is small enough that a match statement is clear and easy to maintain. A data-driven approach makes more sense if the primop count grows significantly (100+).

### Impact of not doing this

Dead `CanFail` bindings survive in compiled output. For most programs, the performance impact is negligible — a few extra instructions. The real cost is in the Core IR dump (`--dump-core`), where dead bindings add noise that makes the IR harder to read and reason about.

## Unresolved questions

1. **Should `ArraySet` and `ArrayPush` be `CanFail` or `HasEffect`?** They perform allocation (creating a new array). Allocation is technically a side effect (memory pressure), but functional languages universally treat allocation as pure. Current classification: `CanFail` (due to index bounds, not allocation).

2. **Should `ParseInt` be `CanFail` or `Pure`?** It returns `Option` (never crashes), but the parse can "fail" by returning `None`. Since it never crashes and has no side effects, it could be `Pure`. Current classification: `CanFail` (conservative — the "failure" semantics might change).

3. **Interaction with strict mode.** In `--strict` mode, type mismatches are caught at compile time, so generic arithmetic (`Add`, `Sub`, `Mul`) cannot type-mismatch at runtime. Should strict mode promote these from `CanFail` to `Pure`? This is a future optimization opportunity but not required for this proposal.

## Future possibilities

- **Speculation in `case_of_case`:** The `is_pure()` function (now meaning `Pure` only) can gate code motion in the case-of-case transform, preventing speculation of `CanFail` operations while allowing `Pure` operations to float freely.

- **Effect level as IR metadata:** Instead of recomputing `primop_effect()` on every query, store the effect level on `CoreExpr::PrimOp` nodes during lowering. This is a minor performance optimization for the compiler itself.

- **Four-level effects:** If Flux adds a `case-of-case` transform that duplicates code across branches, the `HasEffect` level could be split into `ThrowsException` (duplicable) and `ReadWriteEffect` (not duplicable).

- **User-visible effect annotations:** Long-term, the effect level could be exposed in Flux's type system as part of the algebraic effect row, allowing users to write functions that are generic over "pure or can-fail" effects.
