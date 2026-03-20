- Feature Name: CFG Bytecode Full Coverage
- Start Date: 2026-03-20
- Proposal PR:
- Flux Issue:

## Summary
[summary]: #summary

Migrate the remaining AST-fallback functions to the CFG bytecode compilation path so that all functions benefit from Aether memory optimizations (reuse, drop specialization, borrowed parameters). Currently 6 of 8 general IrExpr variants are enabled; 2 are gated, and several guard conditions force AST fallback.

## Motivation
[motivation]: #motivation

The Flux bytecode compiler has two compilation paths:

1. **CFG path** — compiles from Core IR → CFG IR → bytecode. This path benefits from all Aether optimizations (dup/drop insertion, drop specialization, dup/drop fusion, reuse analysis, borrowed parameters).
2. **AST path** — compiles directly from the AST → bytecode. No Aether optimizations. Used as fallback when the CFG path can't handle a function.

When a function falls back to the AST path, it loses all Perceus-derived memory optimizations. Every argument is Rc::cloned, no reuse tokens fire, no drop specialization occurs. For list-heavy code this is a ~500x performance gap between VM and JIT/LLVM (which always use Core IR).

### Current state (after proposal 0084 + CFG expansion work)

**IrExpr variants enabled in `supported_expr()`:** Const, Var, None, TupleFieldAccess, TupleArityTest, TagTest, TagPayload, ListTest, ListHead, ListTail, AdtTagTest, AdtField, MakeClosure, Perform, Cons, Some, Left, Right, EmptyList, MakeAdt, MakeTuple, DropReuse, ReuseCons, ReuseAdt, ReuseSome, ReuseLeft, ReuseRight, IsUnique, Binary (10 ops), **MakeArray, MakeHash, MakeList, Index, InterpolatedString, MemberAccess**.

**IrExpr variants with handlers written but NOT enabled:**

| Variant | Blocker |
|---------|---------|
| `Prefix { operator, right }` | AST path validates operand types (E300 for `-"string"`) |
| `LoadName(Identifier)` | Undefined names produce degenerate Core IR; CFG compiles the degenerate IR silently |

**Guard conditions that force AST fallback:**

| Guard | Reason | Impact |
|-------|--------|--------|
| `block_contains_typed_let_ast` | `let x: Int = ...` needs AST-path type annotation validation | High — common in typed code |
| `has_hm_diagnostics` | HM inference errors make Core IR unreliable | Low — only erroneous code |
| `block_contains_cfg_incompatible_statements_ast` | Module/Import inside function body | Low — rare pattern |
| `quick_validate_function_body` | Semantic/effect errors detected pre-codegen | Low — only erroneous code |

The biggest remaining opportunity is **typed let bindings** — many real-world Flux functions use type annotations, and they all fall back to the AST path today.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

This proposal is entirely internal to the compiler. Users see no syntax changes. The observable effects are:

- **Faster VM execution** for functions that currently fall back to AST compilation
- **Better Aether stats** — `--dump-aether` shows more functions with reuse/drop-spec/borrow annotations
- **No change in error messages** — all error detection must remain identical

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Phase 1: Enable `Prefix` in CFG path

**Problem:** The AST path's `validate_prefix_operator_types()` checks that unary `-` operands are numeric (Int or Float). The CFG path emits `OpMinus`/`OpBang` without type checking.

**Solution:** Add a prefix type check to `quick_validate_function_body`. Walk the AST for `Expression::Prefix` with operator `-`, check the operand's HM type via `hm_expr_type_strict_path`. If the operand is known non-numeric, return `true` (has error → force AST fallback).

```rust
// In expr_has_effect_row_error or a new expr_has_type_error walker:
Expression::Prefix { operator, right, .. } if operator == "-" => {
    if let HmExprTypeResult::Known(ty) = self.hm_expr_type_strict_path(right) {
        // Check if ty is known non-numeric
        !matches!(ty, InferType::Con(c, _) if c == int_sym || c == float_sym)
            && ty.is_concrete()
    } else {
        false
    }
}
```

**Files:** `src/bytecode/compiler/statement.rs` (~15 lines), `src/bytecode/compiler/cfg_bytecode.rs` (enable `IrExpr::Prefix` in `supported_expr`).

### Phase 2: Enable `LoadName` in CFG path

**Problem:** When HM inference encounters an undefined name, it gives it a fresh type variable. The Core IR lowering may produce a degenerate body (e.g., `()`). The CFG path compiles this degenerate IR successfully, hiding the error.

**Solution:** The `has_hm_diagnostics` guard already blocks CFG when there are HM errors. But undefined names don't always produce HM diagnostics (HM just assigns a type variable).

Two approaches:
- **A (simple):** Add an undefined-name walker that checks if every `Expression::Identifier` in the body resolves via `symbol_table.resolve()` or is a known base/global. If any fail, force AST fallback.
- **B (deeper):** Compare the IR function's complexity against the AST body. If the IR body is trivially `Unit` but the AST body is non-trivial, force AST fallback.

Recommend **Approach A** — it's a direct check and catches the exact problem.

**Files:** `src/bytecode/compiler/statement.rs` (~20 lines), `src/bytecode/compiler/cfg_bytecode.rs` (enable `IrExpr::LoadName`).

### Phase 3: Remove `block_contains_typed_let_ast` guard

This is the highest-impact change. Currently, ANY function with a typed let binding (`let x: Int = expr`) falls back to the AST path.

**Problem:** The AST path performs two checks for typed lets:
1. `let_annotation_type_mismatch` — checks if the HM-inferred type of the initializer conflicts with the declared type (e.g., `let x: Int = "hello"`)
2. `validate_expr_expected_type_with_policy` — runtime boundary insertion for typed lets in strict mode

**Solution:** Add a typed-let validation walker to `quick_validate_function_body`:

```rust
fn block_has_typed_let_error(&self, body: &Block) -> bool {
    body.statements.iter().any(|s| {
        if let Statement::Let { type_annotation: Some(annotation), value, .. } = s {
            // Check if HM types conflict with the annotation
            if let Some(expected) = TypeEnv::infer_type_from_type_expr(annotation, ...) {
                self.known_concrete_expr_type_mismatch(&expected, value).is_some()
            } else {
                false
            }
        } else {
            false
        }
    })
}
```

When no type mismatch is detected, the function can safely use the CFG path. The annotation is validated but doesn't affect bytecode generation for correct programs.

**Note:** `validate_expr_expected_type_with_policy` (runtime boundary checks in strict mode) is more complex. For strict mode, we should keep the AST fallback until the CFG path can emit boundary checks. For non-strict mode, the type mismatch check alone is sufficient.

**Files:** `src/bytecode/compiler/statement.rs` (~30 lines).

### Phase 4: PrimOp parity for CFG path

The CFG path now emits PrimOp opcodes for named base function calls (added during the CFG expansion work). But the AST path has additional PrimOp optimizations:

- `OpConsumeLocal` — move semantics for last use of a local (instead of clone+drop)
- `OpReturnLocal` — fused get+return for local variables
- Various superinstructions (GetLocal+Pop → ReturnLocal, etc.)

These are VM-specific peephole optimizations. Adding them to the CFG path's bytecode emission would further close the performance gap.

**This is optional** — the CFG path's bytecode is correct without these, just slightly slower for local variable access patterns.

### Phase 5: Handle expression in CFG path

`IrExpr::Handle` is the only IrExpr variant without a handler. However, it's unreachable from the Core→IR lowering path — handler expressions are lowered as `IrInstr::HandleScope` (instruction-level, not expression-level). So this is a non-issue in practice.

If future IR changes make `IrExpr::Handle` reachable, it would need:
- `OpHandle` + `OpClosure` emission for handler arms
- Effect frame setup
- Continuation management

**This is deferred** — no functions currently hit this path.

## Implementation order

| Phase | What | Impact | Effort |
|-------|------|--------|--------|
| 1 | Enable `Prefix` | Low — few functions have prefix as the only blocker | ~15 lines |
| 2 | Enable `LoadName` | Low — same | ~20 lines |
| 3 | Remove typed-let guard | **High** — many real functions have type annotations | ~30 lines |
| 4 | CFG PrimOp/superinstruction parity | Medium — VM perf for base calls | ~100 lines |
| 5 | Handle expression (deferred) | None — unreachable path | N/A |

## Drawbacks
[drawbacks]: #drawbacks

- Each phase adds complexity to `quick_validate_function_body`, which is already a multi-concern validation function. If it grows too large, it should be extracted into a dedicated `src/bytecode/compiler/pre_validation.rs` module.
- The pre-validation approach is inherently a "deny-list" strategy — it blocks CFG only for known error patterns. New AST-path checks added in the future could be silently bypassed if the pre-validation isn't updated.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Why pre-validation (deny-list) over dual-path compilation?

The alternative is **dual-path compilation**: always run both CFG and AST paths, use CFG's bytecode but AST's diagnostics. This was prototyped and reverted because:
- The AST path has side effects on compiler state (symbol table, constants, debug info)
- Save/restore of all state is fragile and expensive
- The AST path's `compile_block_with_tail_collect_errors` modifies scope tracking

Pre-validation is simpler: a boolean check that routes to the right path. The downside is maintaining parity as new checks are added to the AST path.

### Why not move all validation to a shared pre-pass?

A shared validation pass that runs before both paths would be ideal but requires:
- Extracting ALL semantic checks from the AST compiler (effect rows, type annotations, boundary checks, pattern exhaustiveness, etc.)
- Duplicating the diagnostic-producing code
- Risk of diagnostic divergence

The incremental approach (add checks to pre-validation as needed) is more pragmatic.

## Prior art
[prior-art]: #prior-art

- **GHC** has a similar two-path architecture: the STG machine (optimized) and the byte-code interpreter (fallback). GHC's approach is to run all validation in a shared frontend before either backend.
- **Koka** compiles everything through a single Core IR path, avoiding the two-path problem entirely. Flux's AST path exists for historical reasons and should eventually be removed.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- Should the typed-let guard (Phase 3) be removed for non-strict mode only, or for both modes? Strict mode boundary checks may need the AST path.
- Should `quick_validate_function_body` be extracted to its own module when it exceeds ~100 lines?
- Is there a way to detect degenerate Core IR (Phase 2) without walking the AST for unresolved names?

## Future possibilities
[future-possibilities]: #future-possibilities

- **Remove the AST compilation path entirely.** Once all functions go through CFG, the AST compiler becomes dead code. This would be a major simplification — removing ~3000 lines from `expression.rs` and ~500 lines of statement compilation.
- **Shared validation pass.** Extract all semantic validation into a pass that runs before codegen, independent of the compilation path. This eliminates the pre-validation deny-list approach.
- **CFG-level type checking.** Add type annotations to the CFG IR so the CFG path can perform its own type validation without relying on AST-path checks.
