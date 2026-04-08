- Feature Name: Early Operator Desugaring
- Start Date: 2026-04-08
- Proposal PR:
- Flux Issue:
- Depends on: Proposal 0145 Steps 1–5 (done)

## Summary

Desugar overloadable operators (`+`, `-`, `*`, `/`, `==`, `!=`, `<`, `<=`, `>`, `>=`, `++`) to class method calls **during type inference**, before Core IR lowering. This eliminates the dual-path compilation problem where the bytecode AST path and the Core/CFG path disagree on dictionary parameters.

## Motivation

### The problem

Flux currently handles operators in **two separate compilation paths**:

1. **Bytecode AST path** (`bytecode/compiler/expression.rs`): Compiles `Expression::Infix` directly to opcodes (`OpAdd`, `OpEqual`, etc.). No dictionary awareness.
2. **Core/CFG path** (`core/lower_ast/expression.rs`): Converts `Expression::Infix` to either specialized primops or class method calls. Dictionary elaboration runs after this.

When dictionary elaboration adds a dictionary parameter to a constrained function (e.g., `fn max_of<a: Ord>(x: a, y: a) -> a`), the function body is correctly rewritten via the Core path. But if the CFG bytecode compilation fails for the elaborated body and falls back to the AST path, the AST path compiles the function with the **original arity** (no dict param), causing a runtime arity mismatch:

```
error[E1000]: wrong number of arguments: want=2, got=3
```

This architectural mismatch makes it impossible to reliably wire operators to class methods.

### Why bolting-on doesn't work

We attempted to fix this by:
1. Adding missing operators to the class dispatch match in `lower_infix`
2. Adding a fallback path for unresolved polymorphic calls
3. Fixing the bytecode compiler's arity computation

Each fix exposed new edge cases:
- **Circular dispatch**: Generated `__tc_Eq_Bool_eq` bodies use `==`, which triggers class dispatch back to the same function
- **Contextual constraints**: Instance methods with `Eq<a> =>` context get dict params added, but call sites don't pass them
- **Bytecode fuse breakage**: Generated `__tc_*` functions shift global indices, breaking instruction-position-sensitive optimizations
- **Duplicate generation**: Multi-module compilation generates the same `__tc_*` functions multiple times

The root cause is structural: **operators are special-cased in two places** instead of being normalized to function calls once.

### GHC's solution

In GHC (Haskell's compiler), operators are **not special after the typechecker**:

```
Source:       x + y
Parser:       OpApp x (+) y        — infix syntax preserved
Renamer:      OpApp x (+) y        — name resolved, still infix
Typechecker:  (+) x y              — DESUGARED TO PREFIX (splitHsApps)
                + wrapper: WpTyApp Int · WpEvApp $dNumInt
Desugarer:    App(App(App(App (+) @Int) $dNum) x) y   — SINGLE Core IR
→ ALL backends consume this Core
```

The critical step is `splitHsApps` in the typechecker, which converts `OpApp` to prefix application form. After that, `(+)` is just a function — the constraint solver and evidence insertion machinery handle it uniformly.

**GHC never has two paths** for the same expression. Core IR is the single source of truth for all backends.

## Guide-level explanation

### No user-visible changes

This proposal changes **compiler internals only**. Flux source code is unchanged:

```flux
// These all continue to work exactly as before:
let x = 1 + 2              // concrete Int: fast-path primop
let y = 3.14 * 2.0          // concrete Float: fast-path primop
print(x == y)               // concrete: fast-path primop

// This NOW works (previously broken):
fn max_of<a: Ord>(x: a, y: a) -> a {
    if x > y { x } else { y }   // > desugared to gt(x, y)
}
print(max_of(3, 7))   // prints 7
```

### What changes for compiler contributors

After this proposal, the compilation pipeline becomes:

```
Source (.flx)
  → Parser → AST with Expression::Infix
  → HM Inference → types inferred, constraints emitted
  → ★ Operator Desugaring (AST → AST) ★
      Expression::Infix { op: ">", left: x, right: y }
      → Expression::Call { function: "gt", args: [x, y] }
      (only when operands are NOT concrete Int/Float)
  → Core IR lowering (fewer Infix nodes to handle)
  → Dict elaboration (gt is a normal function call — handled uniformly)
  → Aether / CFG / Bytecode / LLVM
```

The key invariant: **by the time any compilation path sees the AST, polymorphic operators are already function calls**. Both the AST bytecode path and the Core/CFG path see the same `Expression::Call` node, so they agree on arity and dictionary parameters.

Concrete operators (`1 + 2`, `3.0 < 4.0`) remain as `Expression::Infix` and hit the existing fast-path primops during Core lowering. No performance change.

## Reference-level explanation

### Operator-to-method mapping

| Operator | Class | Method | Condition for desugaring |
|----------|-------|--------|------------------------|
| `+` | `Num` | `add` | operands not concrete Int/Float |
| `-` | `Num` | `sub` | operands not concrete Int/Float |
| `*` | `Num` | `mul` | operands not concrete Int/Float |
| `/` | `Num` | `div` | operands not concrete Int/Float |
| `==` | `Eq` | `eq` | operands not concrete Int/Float |
| `!=` | `Eq` | `neq` | operands not concrete Int/Float |
| `<` | `Ord` | `lt` | operands not concrete Int/Float |
| `<=` | `Ord` | `lte` | operands not concrete Int/Float |
| `>` | `Ord` | `gt` | operands not concrete Int/Float |
| `>=` | `Ord` | `gte` | operands not concrete Int/Float |
| `++` | `Semigroup` | `append` | always |

Operators not in this table (`&&`, `||`, `|>`, `%`) are not overloadable and remain as `Expression::Infix` always.

### New class methods required

The current class registrations need to be extended:

- **Eq**: Add `neq` method (currently `!=` desugars to `!eq(l, r)` which is fragile)
- **Ord**: Add `lt`, `gt`, `lte`, `gte` methods (currently only has `compare`)
- **Num**: Add `div` method (currently only has `add`, `sub`, `mul`)

### Implementation: the desugaring pass

A new AST→AST pass runs after HM inference and before Core lowering. It walks all `Expression::Infix` nodes and rewrites overloadable operators to `Expression::Call` when the operand types are not concrete.

**Location**: New function `desugar_operators` in `src/ast/`, invoked from `phase_type_inference` in the bytecode pipeline and from `lower_to_core` in the LLVM pipeline.

**Input**: The type-inferred program + `hm_expr_types` map (for checking operand concreteness) + `ClassEnv` (for checking if classes are registered).

**Algorithm**:

```
for each Expression::Infix { left, operator, right, id, span }:
    if operator is overloadable:
        left_type = hm_expr_types[left.expr_id()]
        right_type = hm_expr_types[right.expr_id()]
        if left_type and right_type are both concrete Int → KEEP as Infix (fast path)
        if left_type and right_type are both concrete Float → KEEP as Infix (fast path)
        else:
            method_name = operator_to_method(operator)  // e.g., ">" → "gt"
            REWRITE to Expression::Call {
                function: Expression::Identifier { name: method_name },
                arguments: [left, right]
            }
            // Special case: != → Call(neq, [left, right])
            // No longer needs Not(eq(l, r)) wrapping
```

**What this eliminates**:

1. The `class_method` match in `lower_infix` (lines 427–464 of `expression.rs`) — replaced by the earlier desugaring
2. The `try_resolve_class_call` path for operators — normal function call resolution handles it
3. The type-variable fallback path — `Expression::Call` to a class method naturally flows through dict elaboration
4. The `!=` → `Not(eq(l, r))` wrapping — `neq` is a first-class method
5. All operator-specific logic in `lower_infix` for non-concrete types

### Builtin instance functions

For each builtin class instance (e.g., `Eq<Int>`, `Ord<Float>`), generate `__tc_*` functions for the new methods. These function bodies use **concrete typed operators** which will hit the fast-path primops:

```rust
// __tc_Ord_Int_gt(x: Int, y: Int) -> Bool { x > y }
// Since x and y are typed Int, the operator stays as Expression::Infix
// (concrete fast path) and lower_infix emits ICmpGt.
```

This avoids the circular dispatch problem: the generated bodies use operators on concrete types, which are never desugared to class methods.

### Changes to `lower_infix`

After this proposal, `lower_infix` only handles:
1. **Concrete Int/Float operators** → specialized primops (unchanged)
2. **Non-overloadable operators** (`&&`, `||`, `|>`, `%`) → generic primops (unchanged)
3. **Gradual-typing fallback** (`Any`-typed operands) → generic primops (unchanged)

The class dispatch block (lines 427–464) is removed entirely.

### Pipeline integration

```
phase_type_inference:
    1. Run HM inference → hm_expr_types, type_env, class_constraints
    2. Run type_informed_fold (existing optimizations)
    3. ★ Run desugar_operators (NEW) ★
    4. Re-infer on desugared program (existing step for type_informed_fold)
    5. Return TypeInferenceResult with desugared program
```

The re-inference step (already done for `type_informed_fold`) ensures that `hm_expr_types` is consistent with the desugared AST, and that HM picks up the scheme constraints for the newly-introduced function calls.

## Drawbacks

1. **Extra AST pass**: One more walk over the AST. But it's simple pattern matching on `Expression::Infix`, and the existing `type_informed_fold` already does a full AST walk.

2. **Re-inference cost**: The desugared program needs re-inference (already done for `type_informed_fold`). Could be avoided by updating `hm_expr_types` incrementally, but the current architecture already pays this cost.

3. **Method name pollution**: Names like `add`, `eq`, `gt` become resolvable identifiers. If a user defines their own `fn add(x, y)`, it could shadow the class method. Mitigated by the polymorphic stub generation which already handles this.

## Rationale and alternatives

### Why this design?

- **Eliminates the dual-path problem entirely**: Both compilation paths see `Expression::Call`, agreeing on arity and dictionary parameters.
- **Minimal invasion**: The new pass is a simple AST rewrite. No changes to the parser, type inference, Core IR, dict elaboration, or bytecode compiler.
- **Follows GHC's proven architecture**: GHC's `splitHsApps` does exactly this transformation, and it's been battle-tested for 30+ years.
- **Naturally composable**: Dict elaboration, call-site resolution, and polymorphic dispatch all already work for `Expression::Call`. We're just feeding operators into the existing machinery.

### Alternatives considered

**A. Bolt operator dispatch onto `lower_infix` (attempted, failed)**

Add operators to the class dispatch match in `lower_infix` + add a fallback path for unresolved calls. This was attempted and produced cascading failures:
- Circular dispatch in builtin instance bodies
- AST/CFG path disagreement on arity
- Contextual constraint handling breaks
- Generated function count bloats all bytecode

**B. Force all compilation through Core (eliminate AST fallback)**

Make the CFG bytecode compiler handle all expressions so the AST path never fires. This would work but is a much larger change — the AST fallback exists for error recovery, strict-mode checking, and edge cases. Removing it would require significant CFG compiler work.

**C. Duplicate dict elaboration in the AST path**

Add dictionary parameter handling to the bytecode compiler's AST path. This duplicates logic between two compilation paths, risking subtle divergences, and doesn't solve the fundamental problem of operators being special-cased.

## Prior art

### GHC (Haskell)

GHC's `splitHsApps` function in `compiler/GHC/Tc/Gen/Head.hs` (lines 339–347) converts `OpApp arg1 op arg2` to prefix `op arg1 arg2` during type checking. After this point, operators are ordinary function applications. Evidence insertion (`WpEvApp`) and Core desugaring handle them uniformly. GHC has used this architecture since 1991.

### PureScript

PureScript also desugars operators to function calls early in the pipeline. Operators are just infix aliases for named functions, resolved during parsing/desugaring.

### Elm

Elm compiles operators to function calls during canonicalization (similar to renaming). The `(+)` operator becomes `Basics.add` before type inference.

### Rust (trait method dispatch)

Rust resolves operator overloading (via `Add`, `Eq`, etc. traits) during type checking. The MIR/LLVM backends see fully-resolved method calls, never raw operators.

## Unresolved questions

1. **Should `%` (modulo) be overloadable?** Currently it's not mapped to any class. It could be added to `Num` or a separate `Integral` class.

2. **Should the desugaring run before or after `type_informed_fold`?** Running after is simpler (fold doesn't need to handle the new `Call` nodes), but running before means the fold could optimize the desugared calls.

3. **String concatenation**: `++` maps to `Semigroup.append`. Should string `+` also desugar? Currently `"a" + "b"` uses `Add` primop for strings. This is a separate concern from operator desugaring.

## Future possibilities

1. **User-defined operators**: With this architecture, user-defined infix operators (`infixl 6 <+>`) would naturally desugar to function calls during the same pass.

2. **Specialization / rewrite rules**: Once operators are function calls, the Core optimizer can specialize them (e.g., rewrite `add @Int` to `IAdd` during a Core-to-Core pass, analogous to GHC's `RULES`).

3. **Removing generic primops**: The generic `Add`, `Eq`, `Lt`, etc. primops become unnecessary once all operators route through class methods. They can be kept as a gradual-typing fallback or removed when strict typing becomes the default.

4. **Num defaulting**: With operators as class method calls, unconstrained `Num` variables can default to `Int` (like Haskell), improving ergonomics for numeric literals.
