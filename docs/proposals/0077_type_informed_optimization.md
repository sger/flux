- Feature Name: Type-Informed Optimization Pass
- Start Date: 2026-03-04
- Completion Date: pending
- Status: Draft
- Proposal PR: pending
- Flux Issue: pending
- Depends on: 0032 (type system), 0051 (HM zero-fallback), 0074 (base signatures)

# Proposal 0077: Type-Informed Optimization Pass

## Summary

Introduce a second optimization phase that runs **after** HM type inference, enabling
type-directed AST transformations that the current purely-syntactic passes cannot perform.
The key enabler is a **two-phase inference** model: a first inference pass produces a
`TypeEnv` used to guide optimization; a second inference pass runs on the optimized AST
to produce the pointer-stable expression ID map consumed by PASS 2 codegen.

## Motivation

The current compiler pipeline is:

```
desugar → constant_fold → rename → free_vars → find_tail_calls
    → PASS 1 → infer_program → PASS 2
```

All optimization happens before type information exists. This creates a one-way barrier:

- **Constant folding** is purely syntactic — it cannot fold `identity(42)` to `42`
  because it does not know that `identity` is a pure, single-expression wrapper.
- **Free variable analysis** is conservative — it cannot distinguish whether a captured
  variable is used in a type-erased (`Any`) position vs. a concrete typed position.
- **Tail call detection** is structural — it does not use inferred return types to
  validate that TCO is semantically safe across recursive call chains.

There is also a missed opportunity in the other direction: `infer_program` already
produces a `TypeEnv` stored on the `Compiler` struct, but nothing between HM inference
and PASS 2 uses it to simplify the AST before codegen. PASS 2 reads `TypeEnv` for
validation, never for transformation.

The root blocker for post-inference optimization is the pointer-identity invariant on
`ExprNodeId`:

> "HM expression IDs are keyed by expression allocation addresses. Any AST rewrites must
> happen before `infer_program` is called."
> — `bytecode/compiler/mod.rs`

Two-phase inference dissolves this constraint by decoupling the "type discovery" inference
run from the "codegen" inference run.

## Guide-level explanation

From the compiler maintainer's perspective, the pipeline becomes:

```
desugar → constant_fold → rename → free_vars → find_tail_calls
    → PASS 1 → infer_program (Phase 1: type discovery)
    → type_informed_fold       ← NEW
    → infer_program (Phase 2: pointer-stable codegen IDs)
    → PASS 2
```

Phase 1 inference runs cheaply over the pre-optimized AST. Its only output consumed by
the next step is the `TypeEnv` — diagnostics are discarded (they will re-emerge from the
Phase 2 run on the final AST). `expr_types` and `expr_ptr_to_id` from Phase 1 are also
discarded.

`type_informed_fold` is a new `AstFolder` pass that takes `(&Program, &TypeEnv)` and
returns an owned `Program`. It applies transformations that require knowing the type of
sub-expressions. The resulting program is the input to Phase 2 inference and PASS 2.

Phase 2 inference runs on the optimized AST and produces the definitive `expr_types` and
`expr_ptr_to_id` maps. These are pointer-stable relative to the optimized AST, satisfying
the invariant for PASS 2.

From the Flux user's perspective, programs compile to better bytecode with no syntax
changes. Specifically, calls to pure single-expression functions may be inlined, reducing
call overhead and enabling downstream constant folding.

## Reference-level explanation

### Two-phase inference model

```rust
// In Compiler::compile():

// Phase 1: type discovery (TypeEnv only; expr maps discarded)
let type_env_phase1 = {
    let hm = infer_program(program, &self.interner, config.clone());
    hm.type_env
};

// Type-informed fold
let optimized = type_informed_fold(program, &type_env_phase1, &self.interner);

// Phase 2: inference on optimized AST (pointer-stable for PASS 2)
let mut hm_diagnostics = {
    let hm = infer_program(&optimized, &self.interner, config);
    self.type_env = hm.type_env;
    self.hm_expr_types = hm.expr_types;
    self.expr_ptr_to_id = hm.expr_ptr_to_id;
    hm.diagnostics
};

// PASS 2 runs on `optimized`, same allocation as Phase 2 inference ✓
```

The `config` struct (`InferProgramConfig`) is cloned for Phase 1 and consumed by Phase 2.

### `type_informed_fold` — transformation scope

The fold is an `AstFolder` (existing trait in `src/ast/`) that carries a `&TypeEnv` and
`&Interner`. It applies the following transformations in a single AST walk:

#### T1: Pure function inlining

A call `f(x₁, ..., xₙ)` is inlined when all of:
1. `f` resolves to a top-level `Scheme::mono(InferType::Fun(...))` in `TypeEnv` — monomorphic, so we have a concrete body.
2. The function body is a single expression (no `do`-block, no early returns).
3. The inferred effect row for `f` is closed-empty — `f` is pure.
4. Arity of the call matches the function signature.

Inlining replaces the `Expression::Call` node with a beta-reduced expression.
Beta reduction is substitution of actual arguments for formal parameters, which the
existing `rename` pass infrastructure can support.

**Initial scope**: limit inlining to zero-argument and one-argument pure functions to
bound the complexity of beta reduction and avoid argument duplication.

#### T2: Constant propagation for typed identifiers

When `TypeEnv` reveals that a `let`-bound identifier has a monomorphic concrete type
(e.g., `Int`, `String`) and its binding is a literal expression, usages of that identifier
in the same scope can be replaced with the literal. This extends syntactic constant folding
to bindings that went through type inference.

**Initial scope**: only `Int` and `String` typed literals. No heap-allocated values.

#### T3: Dead branch elimination on typed conditions

When `TypeEnv` infers that an `if` condition expression has a fully resolved `Bool` type
AND the condition is a statically-known literal (already a `true`/`false` node after
syntactic folding), the dead branch is removed. This handles cases where Phase 1 constant
folding produced `true`/`false` nodes but the branch structure remained.

**Initial scope**: only literal boolean conditions after prior folding. No attempt to
evaluate non-literal conditions.

### `TypeInformedFoldCtx` — implementation structure

```rust
// src/ast/type_informed_fold.rs

pub struct TypeInformedFoldCtx<'a> {
    type_env: &'a TypeEnv,
    interner: &'a Interner,
    // Resolved top-level function bodies for inlining (T1)
    fn_bodies: HashMap<Identifier, &'a Block>,
}

impl<'a> TypeInformedFoldCtx<'a> {
    pub fn fold_program(program: &Program, type_env: &'a TypeEnv, interner: &'a Interner) -> Program;
}
```

The fold operates on `Expression` nodes via the existing `AstFolder` trait pattern. It
does not need mutable access to `TypeEnv` — it is a read-only consumer.

### Interaction with `InferProgramConfig` cloning

`InferProgramConfig` owns `HashMap` values. To avoid deep copies, Phase 1 uses references
where possible, or the config is constructed twice from the same source data rather than
cloned. The `Compiler` already builds these maps in helper methods
(`build_preloaded_base_schemes`, etc.) so calling them twice is the zero-duplication path.

### Updated pipeline in `compile_with_opts`

The `optimize` flag gates the current syntactic passes. The type-informed fold is gated
by a new `type_optimize` flag (or extends the existing `optimize` flag — TBD based on
benchmarking cost of the second inference pass).

### Free variable analysis upgrade (follow-up)

`collect_free_vars_in_program` currently returns a flat `HashSet<Symbol>`. A follow-up
sub-task changes its return type to `HashMap<Span, HashSet<Symbol>>` (free vars keyed by
function definition span). This enables:

- Pruning `TypeEnv` scope during `generalize()` in `finalize_and_bind_function_scheme`:
  instead of scanning all scopes, scan only bindings for names that appear free in the
  current function body.
- Passing the per-function free var map into `InferProgramConfig` so `InferCtx` can use
  it during inference.

This is a separate proposal item but shares the motivation of using pre-computed analysis
data to speed up inference.

## Drawbacks

**Two inference runs**: Phase 1 adds inference cost on top of the existing single run.
For large programs with complex type graphs, this may be noticeable. Mitigation: Phase 1
discards `expr_types` and `expr_ptr_to_id` immediately — it only needs `TypeEnv`, which
is cheaper to produce than the full expression map.

**Fold correctness**: Beta reduction for inlining (T1) must be capture-avoiding. The
`rename` pass already handles alpha-renaming; the fold must invoke it correctly for each
inlined function body to avoid variable capture. Getting this wrong produces subtle
semantic bugs. Mitigation: start with zero-argument functions only (no beta reduction
needed, just body substitution), then extend to one-argument after test coverage is solid.

**Diagnostic stability**: Phase 1 diagnostics are discarded. If Phase 1 and Phase 2
produce different errors (because the optimized AST has different structure), the user
only sees Phase 2 errors. For the initial scope (constant propagation, dead branch
elimination), the optimized AST is a strict subset of the original, so this is benign.

## Alternatives

**Single-pass with metadata**: Instead of rewriting the AST, Phase 1 could produce
metadata (e.g., `pure_fns: HashSet<Identifier>`) consumed by PASS 2 for better codegen
without touching the AST. This avoids the second inference run but cannot improve the
constant folding or free var passes, which require AST rewrites.

**Whole-program inlining at PASS 2 level**: Inline at bytecode emit time rather than AST
level. Simpler because no beta reduction in the AST, but limits the benefit — subsequent
AST-level passes (e.g., constant folding on the inlined body) cannot fire.

**Lazy normalization in TypeSubst**: The primary performance bottleneck in `infer_program`
is `TypeSubst::compose` re-applying substitutions eagerly. A lazy normalization strategy
(normalize on lookup rather than on compose) would reduce inference cost, making the
two-pass model cheaper. This is orthogonal and can proceed in parallel.

## Unresolved questions

1. Should `type_optimize` be a separate CLI flag (`--type-optimize`) or bundled with
   `--optimize` / `-O`? The two-inference cost may be unacceptable without the flag.

2. What is the inlining depth limit? Recursive functions must not be inlined
   (infinite expansion). Mutual recursion must also be detected.

3. Should inlined function bodies be re-run through syntactic constant folding, or is
   that deferred to the next compile cycle?

4. How does inlining interact with the bytecode cache (`.fxc` files)? The cache key
   must incorporate whether type-informed optimization was enabled.

## Implementation sequence

| Step | File(s) | Description |
|------|---------|-------------|
| S1 | `src/ast/type_informed_fold.rs` | Scaffold `TypeInformedFoldCtx`, implement T3 (dead branch) first as it requires no beta reduction |
| S2 | `src/ast/mod.rs` | Export `type_informed_fold` from the `ast` module |
| S3 | `src/bytecode/compiler/mod.rs` | Wire two-phase inference in `compile()` behind `--optimize` |
| S4 | `src/ast/type_informed_fold.rs` | Add T2 (constant propagation for typed literals) |
| S5 | `src/ast/type_informed_fold.rs` | Add T1 (zero-argument pure function inlining) |
| S6 | `src/ast/type_informed_fold.rs` | Extend T1 to single-argument functions with capture-avoiding substitution |
| S7 | `src/ast/free_vars.rs` | Upgrade to per-function free var map (`HashMap<Span, HashSet<Symbol>>`) |
| S8 | `src/ast/type_infer/mod.rs` | Thread per-function free vars through `InferProgramConfig` for scope pruning |

S1–S3 can ship as a flag-gated no-op (fold produces the same AST) to validate the
two-phase pipeline wiring before any transformations are active.
