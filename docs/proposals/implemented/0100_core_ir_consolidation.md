- Feature Name: Core IR Consolidation
- Start Date: 2026-03-12
- Proposal PR: —
- Flux Issue: —
- Status: Implemented

# Proposal 0100: Core IR Consolidation

## Summary
[summary]: #summary

Consolidate Flux's three intermediate representations (Core IR, Structured IR, CFG IR) into a two-stage pipeline: **N-ary Core → CFG IR**. Remove the legacy AST → Structured IR lowering path, eliminate the curry/uncurry dance, and make Core the single source of truth for all optimization and analysis passes before backend code generation.

## Motivation
[motivation]: #motivation

### The current state

Flux currently has three IRs and two lowering paths:

```
                    ┌─── Core IR ──── to_ir.rs ───┐
AST ─── type infer ─┤                              ├─ CFG IR ─── bytecode / JIT
                    └─── Structured IR (ir/lower) ─┘
```

**Core IR** (`src/core/`): A lambda-calculus representation with ~12 `CoreExpr` variants. Functions are curried (single-arg `Lam`/`App` chains). Runs 4 optimization passes: beta reduction, case-of-known-constructor, inline trivial lets, dead let elimination.

**Structured IR** (`IrStructuredExpr`, 28 variants): A near-copy of the AST with different names. Used by the bytecode compiler's "structured path" as the primary compilation target.

**CFG IR** (`IrExpr` + `IrFunction`): Basic blocks with SSA variables and terminators. The actual backend target for both bytecode and JIT.

### The problems

**1. Two lowering paths means double the work.**

Every new expression type needs implementation in both `core/lower_ast.rs` (AST → Core) and `ir/lower.rs` (AST → Structured IR). Every bug fix potentially needs applying twice. The "fallback" architecture means you can never be sure which path compiled a given function, making debugging harder.

**2. Structured IR is the AST with extra steps.**

`IrStructuredExpr::Identifier` maps to `Expression::Identifier`. `IrStructuredExpr::Call` maps to `Expression::Call`. `IrStructuredExpr::If` maps to `Expression::If`. The conversion functions `ir_structured_expr_to_expression` and `expression_to_ir_structured_expr` exist precisely because the two representations are interchangeable. This is ~1000 lines of code that adds no semantic value.

**3. Currying and uncurrying wastes effort.**

Core IR normalizes `fn add(a, b) { a + b }` into `Lam(a, Lam(b, PrimOp(Add, [a, b])))`. Then `to_ir.rs` immediately un-curries it via `strip_lams` back into a 2-parameter function. We added `declared_arity` to `CoreDef` specifically to limit how many `Lam` nodes get stripped — a workaround for a problem the curried representation created.

**4. Core IR doesn't transform effects.**

The primary justification for a Core IR layer is to lower complex features into simpler ones. Koka's Core compiles effects into evidence passing. Flux's Core passes `Perform` and `Handle` through unchanged — they enter Core and exit Core as the same nodes. The optimization passes (beta reduction, COKC) are generic lambda calculus rewrites that don't interact with Flux's effect system at all.

**5. Handle cannot be compiled in the CFG path.**

The Core → CFG lowering emits body instructions *before* the `Handle` instruction (because `lower_expr(body)` recursively emits instructions, then Handle is emitted after). At runtime, the handler must be installed *before* the body runs. This ordering mismatch forces Handle to stay on the structured path — the main remaining blocker for removing the fallback.

### Impact

Every change to the compiler pipeline — adding new expression types, fixing bugs, expanding the CFG path — is harder than it should be because of the dual-path architecture. Removing it is a prerequisite for velocity on everything else.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Target architecture

After this proposal, the compilation pipeline becomes:

```
AST ─── type infer ─── N-ary Core ─── passes ─── CFG IR ─── bytecode / JIT
```

One path. No fallbacks. No structured IR.

### What changes for compiler contributors

- **One place to add new expressions**: `core/lower_ast.rs` for AST → Core lowering, `core/to_ir.rs` for Core → CFG lowering.
- **N-ary functions in Core**: `Lam(Vec<Identifier>, Box<CoreExpr>)` and `App(Box<CoreExpr>, Vec<CoreExpr>)`. No more `strip_lams`, no more `declared_arity`.
- **Handle as a scope in CFG**: Handle becomes a block-level scope marker in CFG IR rather than an expression, so the handler is installed before the body's instructions.
- **Validation is a pre-pass**: Immutability checks (E001), duplicate name checks (E002), and other validations run as a pre-pass on the AST or Core, not interleaved with bytecode emission.

### What does NOT change

- The CFG IR representation (`IrExpr`, `IrFunction`, `IrBlock`, etc.) stays the same.
- The bytecode compiler's CFG compilation (`compile_ir_cfg_expr`, etc.) stays the same.
- The JIT compiler stays the same.
- Runtime semantics are unchanged — effects still use `OpPerform`/`OpHandle` at the VM level.
- The type inference and HM system are completely unaffected.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Phase 1: Extract validation from the structured path

**Goal**: Remove the dependency on the structured path for validation checks, so that the CFG path can handle all functions.

**Steps**:

1. **Identify all validations in the structured path** that don't exist in the CFG path:
   - E001: Immutable variable reassignment (`IrTopLevelItem::Assign` targeting a parameter)
   - E002: Duplicate name in scope (`IrTopLevelItem::Let` shadowing a parameter)
   - Return-type annotation mismatch (already in `compile_ir_function_statement`)
   - Effect annotation validation (already in `compile_ir_function_statement`)

2. **Implement a pre-validation pass** that runs on the structured body *before* choosing the compilation path. Currently `structured_body_has_assign_or_let_shadow` is a boolean gate — expand it to emit the actual diagnostics.

3. **Remove the `structured_body_has_assign_or_let_shadow` guard** from the CFG path entry point. The pre-validation pass handles these cases now.

**Result**: The CFG path can compile all functions. The structured path is no longer needed for correctness.

### Phase 2: Handle as a CFG scope

**Goal**: Allow `Handle` expressions to be compiled via the CFG path.

**Problem**: In Core → CFG lowering, `lower_expr(body)` emits all body instructions into the current block before the Handle assign is emitted. But the handler must be active during body execution.

**Solution**: Introduce `IrInstr::HandleScope` as a block-level construct:

```rust
enum IrInstr {
    // ... existing variants ...
    HandleScope {
        effect: Identifier,
        arms: Vec<IrHandleArm>,
        /// Block range [begin, end) containing the handled body.
        body_begin: BlockId,
        body_end: BlockId,
        dest: IrVar,
        metadata: IrMetadata,
    },
}
```

In `to_ir.rs`, when lowering `CoreExpr::Handle`:
1. Emit `HandleScope` instruction with a new body block range.
2. Lower the body expression *inside* the body blocks.
3. The CFG bytecode compiler emits: arm closures → `OpHandle` → body bytecode → `OpEndHandle`.

This preserves the correct execution order while keeping the CFG representation.

### Phase 3: N-ary Core IR

**Goal**: Eliminate the curry/uncurry overhead.

**Changes to `CoreExpr`**:

```rust
// Before (curried)
Lam { param: Identifier, body: Box<CoreExpr>, span: Span }
App { func: Box<CoreExpr>, arg: Box<CoreExpr>, span: Span }

// After (n-ary)
Lam { params: Vec<Identifier>, body: Box<CoreExpr>, span: Span }
App { func: Box<CoreExpr>, args: Vec<CoreExpr>, span: Span }
```

**Changes to lowering** (`lower_ast.rs`):
- `lower_function` produces `Lam(params, body)` directly instead of a chain of single-param `Lam` nodes.
- `lower_call` produces `App(func, args)` directly instead of left-folding `App`.

**Changes to passes** (`passes.rs`):
- Beta reduction: `App(Lam([a, b, c], body), [x, y, z])` → `body[a:=x, b:=y, c:=z]`. Partial application (`App(Lam([a, b], body), [x])`) produces `Lam([b], body[a:=x])`.
- COKC: unchanged (operates on `Case`, not `Lam`/`App`).
- Inline trivial lets: unchanged.
- Dead let elimination: unchanged.

**Removals**:
- Delete `declared_arity` field from `CoreDef`.
- Delete `strip_lams` function from `to_ir.rs`.
- In `to_ir.rs`, `lower_program` reads params directly from `Lam` node instead of stripping them.

### Phase 4: Delete the structured path

**Goal**: Remove `IrStructuredExpr` and the old `ir/lower.rs` lowering.

**Steps**:

1. **Ensure all bytecode compilation goes through CFG**: every function compiles via `try_compile_ir_cfg_function_body` with no fallback.

2. **Delete `IrStructuredExpr`** and all 28 of its variants from `ir/mod.rs`.

3. **Delete `ir/lower.rs`** (AST → Structured IR lowering, ~2800 lines).

4. **Delete conversion functions**: `ir_structured_expr_to_expression`, `ir_structured_block_to_block`, `ir_structured_pattern_to_pattern` and their inverses.

5. **Remove `IrStructuredBlock`/`IrTopLevelItem`** — replace with direct CFG compilation from Core.

6. **Update bytecode compiler** (`compile_ir_top_level_item`, `compile_ir_module_statement`) to work directly from Core definitions + CFG functions.

**Expected deletion**: ~4000-5000 lines of code.

### Migration order

The phases must be done in order because each unblocks the next:

| Phase | Prerequisite | Unblocks |
|-------|-------------|----------|
| 1. Extract validation | None | Phase 4 (removes last structured-path dependency) |
| 2. Handle as CFG scope | None | Phase 4 (removes last expression gap) |
| 3. N-ary Core | None | Cleaner Phase 4 (simpler to_ir.rs) |
| 4. Delete structured path | Phases 1 + 2 | Everything else |

Phases 1, 2, and 3 can be done in parallel since they are independent.

## Drawbacks
[drawbacks]: #drawbacks

**Risk of regressions during migration.** The structured path has been battle-tested through ~900 tests. Replacing it with a CFG-only path means any gap in CFG expression coverage will cause failures. Mitigation: maintain a "bail to error" fallback during transition that clearly identifies unsupported expressions.

**N-ary Core complicates partial application.** Curried Core makes partial application trivially representable. With n-ary Core, `compose(f, g)` returning `\x -> f(g(x))` requires the lowering to distinguish between "all args provided" and "partial application returns a closure." This is already handled today (via `declared_arity`) — n-ary form just makes it explicit in the `Lam` node rather than implicit in the chain length.

**Handle as a CFG scope is unusual.** Most CFG IRs don't have scoped instructions. This is a pragmatic choice that trades IR purity for implementation simplicity. The alternative — lowering Handle to continuation-passing or evidence-passing — is a much larger project.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Why not keep both paths?

The dual-path architecture was appropriate during initial development of the Core IR pipeline. Now that the CFG path handles most expressions (with MakeClosure, MemberAccess, and Perform recently added), the structured path's remaining role is:
1. Handle compilation (fixable with Phase 2)
2. Validation side-effects (fixable with Phase 1)

Keeping both paths permanently doubles the maintenance cost of every compiler change.

### Why not compile effects away (Koka-style)?

Koka transforms effects into evidence passing at the Core level, producing plain lambda calculus with no effect primitives by the time it reaches the backend. This is elegant but requires:
- Evidence vector threading through every function call
- Selective CPS translation for polymorphic effect rows
- Yield bubbling with continuation accumulation
- Monadic translation that distinguishes pure vs effectful code

This is multiple PhD papers worth of implementation work (Leijen 2017, 2020, 2021). Flux's current runtime-based approach (`OpPerform`/`OpHandle` with a handler stack) is simpler, correct, and used by production systems like OCaml 5. Effect compilation could be a future optimization pass but should not gate the architectural cleanup.

### Why n-ary instead of ANF or CPS?

- **ANF** (A-Normal Form): Would require all subexpressions to be let-bound. More normalized than necessary for Flux's optimization passes and adds verbosity.
- **CPS** (Continuation-Passing Style): Powerful but makes optimization passes harder to read and write. CPS would be justified if we were compiling effects away (as in Koka), but we're not.
- **N-ary Core**: Minimal change from current curried form. Same passes work with minor adjustments. Directly maps to the n-ary CFG IR without the strip/reconstruct step.

### What if we do nothing?

Every new expression type or optimization will continue to require dual implementations. The `declared_arity` / `strip_lams` pattern will accumulate more workarounds. The Handle ordering issue will permanently block full CFG path coverage for the bytecode compiler.

## Prior art
[prior-art]: #prior-art

**GHC (Haskell)**: Uses a Core IR (System FC) that is a typed lambda calculus. Functions are curried in Core but GHC's STG machine handles arity natively. GHC's simplifier runs on Core and performs beta reduction, case-of-known-constructor, inlining — the same passes Flux implements. Key difference: GHC Core is typed (preserves type information), Flux Core is untyped (types tracked separately in HM maps).

**Koka**: Uses a lambda calculus Core with monadic translation for effects. Effect operations become evidence-vector lookups. Multiple papers (POPL 2017, ICFP 2020, ICFP 2021) document the lowering. This is the gold standard for effect compilation but requires significantly more implementation effort.

**OCaml 5**: Uses runtime-based effect handlers with fiber switching, similar to Flux's `OpPerform`/`OpHandle` approach. Does not compile effects away at the IR level. Demonstrates that runtime effect handling is viable for production use.

**Chez Scheme (Racket)**: Uses CPS conversion for continuations. Significantly more complex IR but enables first-class continuations. Flux's algebraic effects are more limited (no first-class continuations) and don't require CPS.

**Erlang/BEAM**: Uses Core Erlang (n-ary functions, pattern matching, let bindings) as its primary IR. Similar in spirit to this proposal's target: a normalized but not over-reduced representation that maps cleanly to the backend.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- **Module compilation**: The bytecode compiler's `compile_ir_module_statement` currently takes `IrStructuredBlock` for module bodies. Phase 4 needs to decide how modules are represented without structured IR — likely as a list of `CoreDef` items that lower to CFG functions.

- **HandleScope representation**: Should `HandleScope` be an `IrInstr` variant or a new `IrTerminator` variant? The terminator approach is cleaner (body blocks are successors) but changes control flow semantics.

- **Partial application in n-ary Core**: When a function with 3 params is called with 2 args, the lowering needs to produce a closure capturing the provided args. The current curried representation handles this implicitly. The n-ary representation needs explicit closure-wrapping logic in `to_ir.rs`. The exact strategy needs design.

- **Debug info and source mapping**: The structured IR carries `ExprId` from the parser for source mapping to HM types. The CFG path uses `IrMetadata` with spans. Need to verify that source mapping quality doesn't degrade after removing structured IR.

## Future possibilities
[future-possibilities]: #future-possibilities

**Known-call specialization**: When a `Call` target is a statically known function, inline its effect requirements at compile time and emit a direct call instead of going through the handler stack. This gives 80% of evidence passing's performance benefit with 5% of the complexity.

**Inlining**: With a single Core → CFG pipeline, function inlining becomes a straightforward Core pass: replace `App(known_fn, args)` with the function body after substitution. The n-ary representation makes arity matching trivial.

**Effect evidence passing (long-term)**: If performance profiling shows handler stack lookup is a bottleneck, the Core layer is the right place to add evidence passing. The n-ary representation and single-path architecture make this feasible as an incremental addition rather than an architectural rewrite.

**Perceus-style reference counting**: Koka's Perceus analysis determines where `Rc` clones and drops can be eliminated. This analysis operates on the Core IR. The single-path architecture would make adding Perceus analysis straightforward.
