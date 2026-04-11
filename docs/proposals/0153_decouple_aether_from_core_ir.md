- Feature Name: Decouple Aether from Core IR
- Start Date: 2026-04-11
- Status: Not Implemented
- Proposal PR:
- Flux Issue:

## Summary
[summary]: #summary

Remove the five Aether-specific expression variants (`Dup`, `Drop`, `Reuse`, `DropSpecialized`, `AetherCall`) from `CoreExpr` and represent them as regular `PrimOp` calls or metadata annotations instead. The goal is clear: **Aether is only used by the VM and LLVM backends.** Core IR must be backend-agnostic so that future backends can consume it directly without running or knowing about the Aether pass.

## Motivation
[motivation]: #motivation

Today, `CoreExpr` (the central IR type in `src/core/mod.rs`) contains five variants that exist solely for the Aether reference-counting pass:

```rust
CoreExpr::Dup { var, body, span }
CoreExpr::Drop { var, body, span }
CoreExpr::Reuse { token, tag, fields, field_mask, span }
CoreExpr::DropSpecialized { scrutinee, unique_body, shared_body, span }
CoreExpr::AetherCall { func, args, arg_modes, span }
```

These are inserted by the Aether pass (`src/aether/insert.rs`) **into** the existing Core IR, mutating the IR type in-place. This creates several problems:

### 1. Every pass must handle Aether variants

Every function that pattern-matches on `CoreExpr` — simplification passes, ANF normalization, primop promotion, dead let elimination — must include arms for `Dup`, `Drop`, `Reuse`, `DropSpecialized`, and `AetherCall`, even though these variants are irrelevant to those passes. This is visible throughout `src/core/passes/` where Aether arms do nothing but recursively walk children.

### 2. Core IR cannot be consumed before Aether

The Aether pass is the final stage of `run_core_passes_with_optional_interner` in `src/core/passes/mod.rs` (lines 210-239). There is no function that produces a fully optimized, clean Core IR without Aether annotations. Any new backend that doesn't need reference counting (a GC-based target, a JavaScript emitter) would need to either:
- Run the Aether pass and ignore all Aether nodes (wasteful, error-prone)
- Duplicate the pass pipeline up to the Aether stage (fragile)

### 3. Violates the architecture invariant

CLAUDE.md states: "`src/core/` is the only semantic IR. Do not add a second one." But by embedding ownership directives into Core, the IR serves two roles: semantic program representation AND memory management plan. These concerns should be separated.

### 4. Blocks multi-backend strategy

Adding any backend that uses a different memory model (tracing GC, no-op GC for a managed runtime, region-based allocation) requires either ignoring Aether nodes or splitting the pipeline. The current design makes the implicit assumption that all backends use Perceus-style reference counting.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Before (current)

Core IR is a single enum with both semantic and memory-management variants:

```rust
enum CoreExpr {
    // Semantic (11 variants)
    Var, Lit, Lam, App, Let, LetRec, LetRecGroup,
    Case, Con, PrimOp, MemberAccess, TupleField,
    Return, Perform, Handle,
    // Memory management (5 variants)
    Dup, Drop, Reuse, DropSpecialized, AetherCall,
}
```

The Aether pass mutates `CoreExpr` → `CoreExpr` (same type, new variants injected).

### After (proposed)

Core IR contains only semantic variants. Aether annotations are represented as regular `PrimOp` calls and metadata:

```rust
enum CoreExpr {
    // Semantic only (11 variants)
    Var, Lit, Lam, App, Let, LetRec, LetRecGroup,
    Case, Con, PrimOp, MemberAccess, TupleField,
    Return, Perform, Handle,
}
```

The Aether pass inserts `PrimOp(Dup, ...)`, `PrimOp(Drop, ...)`, etc. — regular primop calls that RC-based backends emit and GC-based backends ignore.

### Backend pipeline

```
Source → AST → Type Inference → CoreExpr
  → Core passes (beta, cokc, inline, dead_let, evidence, ANF, primop_promote, dict_elaborate)
  → Clean CoreExpr
      │
      ├── VM backend:   Aether pass → CoreExpr (with PrimOp::AetherDup/Drop) → CFG → Bytecode
      ├── LLVM backend: Aether pass → CoreExpr (with PrimOp::AetherDup/Drop) → LIR → LLVM IR
      │
      └── Future backends: consume clean CoreExpr directly (no Aether, no RC)
```

All backends consume the same `CoreExpr` type. The VM and LLVM backends run the Aether pass to insert RC operations. Future backends skip it entirely.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Step 1: Add Aether primops to CorePrimOp

Add the following variants to the existing `CorePrimOp` enum in `src/core/mod.rs`:

```rust
// Aether RC operations (inserted by Aether pass, consumed by RC backends)
AetherDup = 140,
AetherDrop = 141,
AetherDropReuse = 142,      // drop and return reuse token
AetherAllocAt = 143,        // allocate using reuse token, fallback to fresh alloc
AetherDropSpecial = 144,    // branch on unique vs shared refcount
```

These follow the same convention as all other primops: they're opaque operations that backends emit as they see fit.

### Step 2: Represent dup/drop as PrimOp calls

The Aether pass (`src/aether/insert.rs`) currently produces:

```rust
CoreExpr::Dup { var: x, body: ... }
CoreExpr::Drop { var: x, body: ... }
```

After this change, it produces:

```rust
CoreExpr::Let {
    var: _unused,
    rhs: CoreExpr::PrimOp { op: AetherDup, args: [Var(x)] },
    body: ...
}
// Drop as a sequenced side-effect before the continuation
CoreExpr::Let {
    var: _unused,
    rhs: CoreExpr::PrimOp { op: AetherDrop, args: [Var(x)] },
    body: ...
}
```

This is a `Let` with a side-effecting RHS — the same pattern used for `Print` and other effectful primops.

### Step 3: Represent reuse as PrimOp calls

Current:

```rust
CoreExpr::Reuse { token, tag, fields, field_mask, span }
```

After:

```rust
// Step 1: try to reuse the token's allocation
CoreExpr::Let {
    var: reuse_ptr,
    rhs: CoreExpr::PrimOp { op: AetherAllocAt, args: [Var(token)] },
    body:
        // Step 2: construct using the reuse pointer (or fresh alloc if token was shared)
        CoreExpr::Con { tag, fields, span }
}
```

The `AetherAllocAt` primop returns a reuse token that the backend can use for in-place construction. The `field_mask` metadata (which fields are unchanged from the reused value) is carried as an annotation on the `AetherAllocAt` primop args or as a separate metadata field on `CorePrimOp`.

### Step 4: Represent DropSpecialized as PrimOp

Current:

```rust
CoreExpr::DropSpecialized { scrutinee, unique_body, shared_body, span }
```

After:

```rust
CoreExpr::PrimOp {
    op: AetherDropSpecial,
    args: [Var(scrutinee), Lam([], unique_body), Lam([], shared_body)]
}
```

The `AetherDropSpecial` primop takes the scrutinee and two thunks (unique path, shared path). The backend emits a refcount check and branches.

### Step 5: Represent AetherCall as metadata on App

Current:

```rust
CoreExpr::AetherCall { func, args, arg_modes, span }
```

After: Use a regular `App` node. Borrow mode information is stored in a side table (`BorrowRegistry`) that RC backends consult during code generation. GC backends ignore the registry entirely.

```rust
// Before: AetherCall { func, args, arg_modes: [Owned, Borrowed, Owned] }
// After:  App { func, args }
//         + BorrowRegistry maps this call site → [Owned, Borrowed, Owned]
```

### Step 6: Remove Aether variants from CoreExpr

Delete from `CoreExpr`:

```rust
// DELETE these 5 variants:
AetherCall { func, args, arg_modes, span }
Dup { var, body, span }
Drop { var, body, span }
Reuse { token, tag, fields, field_mask, span }
DropSpecialized { scrutinee, unique_body, shared_body, span }
```

### Step 7: Split the pass pipeline

In `src/core/passes/mod.rs`, split `run_core_passes_with_optional_interner` into two public functions:

```rust
/// Run all semantic Core passes. Produces clean, optimized CoreExpr
/// suitable for any backend.
pub fn run_core_passes(program: &mut CoreProgram, interner: &Interner, optimize: bool)
    -> Result<Vec<Diagnostic>, Diagnostic>
{
    // Stage 0: primop_promote
    // Stage 1: simplification loop (beta, case_of_case, cokc, inline, dead_let)
    // Stage 2: normalization (evidence, ANF)
}

/// Run Aether RC passes on already-optimized Core IR.
/// Only called by RC-based backends (VM, LLVM native).
pub fn run_aether_passes(program: &mut CoreProgram, interner: &Interner,
                         registry: BorrowRegistry)
    -> Result<(), Diagnostic>
{
    // Borrow inference
    // Aether dup/drop/reuse insertion (now emits PrimOp calls)
    // Refined re-inference
    // FBIP check
}
```

### Step 8: Update consumers

Files that pattern-match on `CoreExpr` and currently handle Aether variants:

| File | Change |
|------|--------|
| `src/core/passes/primop_promote.rs` | Remove 5 Aether match arms (~50 lines) |
| `src/core/passes/mod.rs` (helpers) | Remove Aether arms from `expr_size`, `collect_max_binder_id` |
| `src/cfg/` | Handle `PrimOp(AetherDup/Drop/...)` in lowering instead of dedicated arms |
| `src/lir/lower.rs` | Same — match on primop variants |
| `src/lir/emit_llvm.rs` | Emit `flux_dup`/`flux_drop` C calls for Aether primops |
| `src/bytecode/vm/core_dispatch.rs` | Dispatch Aether primops to `Rc::clone`/drop |
| `src/aether/insert.rs` | Emit `PrimOp(AetherDup, ...)` instead of `CoreExpr::Dup { ... }` |
| `src/aether/borrow_infer.rs` | Recognize `PrimOp(AetherDup/Drop)` for analysis |

### Verification of Aether contracts

The existing `verify_aether_contract_stage` function in `src/core/passes/mod.rs` checks that Aether nodes are well-formed after each pass. After this refactor, it would check for `PrimOp(AetherDup/Drop/...)` nodes instead of `CoreExpr::Dup/Drop` variants. The logic is identical; only the pattern match changes.

### Backward compatibility

- **Clean Core IR dumps** (`--dump-core`): Will no longer show `Dup`/`Drop` as first-class expressions. They appear as `PrimOp(AetherDup, ...)` calls. `--dump-aether` continues to show them.
- **Snapshot tests**: Tests that inspect Core IR with Aether annotations will need updating to match the new PrimOp representation.
- **No semantic change**: The same dup/drop/reuse operations are performed; only their IR encoding changes.

## Drawbacks
[drawbacks]: #drawbacks

1. **Refactoring scope**: Touching `CoreExpr` affects most of the compiler. Every `match` on `CoreExpr` needs updating, though most changes are deletions (removing Aether arms).

2. **Loss of type-level distinction**: With dedicated variants, the Rust type system prevents accidentally constructing a `Dup` node before the Aether pass. With PrimOps, a pass could accidentally emit `PrimOp(AetherDup, ...)` at the wrong pipeline stage. Mitigation: the `verify_aether_contract_stage` function catches this at runtime.

3. **PrimOp enum grows**: Adding 5 Aether variants to `CorePrimOp` (already ~130 variants) increases its size. However, this is offset by removing 5 variants from `CoreExpr`, which is the more impactful enum.

4. **Reuse and DropSpecialized are not simple calls**: `Reuse` carries a field mask and `DropSpecialized` carries two continuations. Encoding these as PrimOp args (with thunks) is slightly less natural than dedicated variants. However, this matches how other languages represent these constructs.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Why PrimOps instead of a separate AetherExpr type?

An alternative design introduces a new `AetherExpr` enum that wraps `CoreExpr`:

```rust
enum AetherExpr {
    Core(CoreExpr),
    Dup { var, body: Box<AetherExpr> },
    Drop { var, body: Box<AetherExpr> },
    // ...
}
```

This provides stronger type-level guarantees (pre-Aether code is `CoreExpr`, post-Aether is `AetherExpr`) but requires:
- Duplicating all pattern match logic for the `Core(...)` wrapper
- Converting between types at the Aether boundary
- Updating CFG and LIR lowering to consume `AetherExpr` instead of `CoreExpr`

The PrimOp approach avoids all of this. It keeps one IR type, one set of pattern matches, and one lowering path. The distinction between "pre-Aether" and "post-Aether" Core IR is a pipeline concern, not a type concern.

Other functional language compilers with Perceus-style RC (including compilers in this space) use the same approach: dup/drop are regular function applications in Core, not special IR nodes. This is proven to work well and keeps the IR simple.

### Why not keep Aether in CoreExpr?

The current design works for a two-backend compiler (VM + LLVM) where both backends need RC. But Flux's architecture section in CLAUDE.md states that `src/core/` is the only semantic IR. Memory management directives are not semantics — they're an optimization for a specific class of backends. Embedding them in Core IR conflates concerns and blocks the multi-backend future.

### Impact of not doing this

Without this refactor, adding any non-RC backend requires one of:
- Running the Aether pass and ignoring its output (wasted work, Aether nodes pollute the IR)
- Forking the pipeline before Aether (fragile, two code paths to maintain)
- Special-casing Aether nodes in every new backend ("if Dup, skip; if Drop, skip; ...")

All three options accumulate technical debt. The PrimOp approach eliminates the problem at the root.

## Prior art
[prior-art]: #prior-art

### Functional language compilers with RC

Several functional language compilers that use compile-time reference counting represent dup/drop as regular function calls in their Core IR rather than dedicated expression variants:

- **Compilers in this design space** represent dup/drop as ordinary function applications (e.g., `App(Var "dup", [arg])`) with backend-specific inline annotations. The Core IR type has no special RC variants. Backends that don't need RC simply never insert these calls.

- **Lean 4** uses a similar approach where RC operations are regular IR nodes that the C backend emits and other backends can ignore.

### GHC (Haskell)

GHC's Core IR (`CoreExpr`) is purely semantic: `Var`, `Lit`, `App`, `Lam`, `Let`, `Case`, `Cast`, `Tick`, `Type`, `Coercion`. No memory management nodes. GC directives are a runtime concern, not an IR concern. This clean separation enables GHC's multiple backends (native codegen, LLVM, bytecode interpreter).

### GRIN (Graph Reduction Intermediate Notation)

GRIN separates heap operations (`store`, `fetch`, `update`) from the functional IR. Memory management is explicit but handled through a separate set of operations, not mixed into the functional expression type.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

1. **Field mask encoding for Reuse**: The current `Reuse` variant carries a `field_mask: Option<Vec<bool>>` indicating which fields are unchanged from the reused value. When Reuse becomes a PrimOp, this metadata needs encoding. Options: (a) encode as an integer bitmask argument to `AetherAllocAt`, (b) store in a side table keyed by span or binder ID. Lean toward (a) for simplicity.

2. **BorrowRegistry threading**: `AetherCall` currently carries borrow modes inline. Moving them to a `BorrowRegistry` side table means the registry must be threaded through CFG/LIR lowering. This is already partially the case (`borrow_infer.rs` produces a registry), but the lowering code currently reads modes from the `AetherCall` node. Needs wiring.

3. **Snapshot test volume**: Core IR snapshot tests that include Aether annotations will all change format. The diff will be large but mechanical. Consider accepting all snapshots in one pass (`cargo insta test --accept`).

## Future possibilities
[future-possibilities]: #future-possibilities

1. **GC-based backends**: With clean Core IR, adding a backend targeting a GC runtime (BEAM, JVM, JavaScript) becomes straightforward — consume Core IR directly, skip Aether entirely.

2. **Region-based memory management**: A future alternative to Perceus could use region inference instead of RC. This would be a different pass that inserts different PrimOps (`RegionAlloc`, `RegionFree`), consuming the same clean Core IR.

3. **Backend-specific optimization passes**: Each backend could run its own optimization passes on Core IR before code generation, without needing to handle irrelevant Aether nodes.

4. **Parallel backend compilation**: With a clean Core IR snapshot, multiple backends could compile the same program concurrently from the same IR, each applying their own memory management strategy.

5. **Core IR serialization**: A clean Core IR (without backend-specific annotations) could be serialized as a compilation artifact, enabling separate compilation, caching, and cross-compilation scenarios.
