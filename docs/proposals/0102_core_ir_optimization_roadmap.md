- Feature Name: Core IR Optimization Roadmap
- Start Date: 2026-03-17
- Status: Draft
- Proposal PR: pending
- Flux Issue: pending
- Depends on: 0086 (backend-neutral Core IR), 0077 (type-informed optimization), 0101 (effect handler optimizations)

# Proposal 0102: Core IR Optimization Roadmap

## Summary
[summary]: #summary

A six-phase roadmap to mature Flux's Core IR from its current 4-pass pipeline into a
production-grade optimization framework, drawing on proven techniques from GHC Core,
Rust MIR, Koka's evidence translation, and OCaml's Flambda. Each phase is independently
valuable and ships incremental improvements.

**Priority order:**

0. Module split for `lower_ast.rs` — split 1595-line monolith into focused submodules
1. Typed Core binders — attach HM-inferred types to `CoreBinder`
2. Case-of-case transformation — push outer case into inner case arms
3. Inlining with occurrence analysis — inline small/single-use functions
4. ANF normalization pass — flatten nested expressions into let-chains
5. Evidence-passing for effects — compile tail-resumptive handlers to direct calls
6. Worker/wrapper split — unbox arguments in recursive functions

## Motivation
[motivation]: #motivation

Flux's Core IR (proposal 0086) is now the canonical semantic layer between typed AST and
backend lowering. The current Core pass pipeline runs four transformations:

```
beta_reduce → case_of_known_constructor → inline_trivial_lets → elim_dead_let
```

These are correct but limited. The pipeline only handles:

- Direct beta redexes (`App(Lam(x, body), arg)`)
- Statically known constructor scrutinees
- Trivial `Lit`/`Var` copy propagation
- Dead pure let elimination

This leaves significant optimization opportunities on the table:

### What we cannot do today

1. **No type information in Core**: every pass that needs types must thread a separate
   map from the HM inference output. This blocks type-directed specialization, typed
   PrimOp selection at the Core level, and typed validation of Core transformations.

2. **No case-of-case**: when a `Case` scrutinizes another `Case`, the intermediate
   constructor is allocated and immediately destructed. GHC considers this its single
   most impactful Core-to-Core optimization.

3. **No function inlining**: `inline_trivial_lets` only substitutes `Lit` and `Var`.
   Small wrapper functions, single-use functions, and constructor wrappers are never
   inlined. This blocks all downstream simplification that inlining would expose.

4. **No ANF normalization**: nested subexpressions remain tree-shaped in Core, forcing
   `to_ir.rs` (1756 lines) to do implicit ANF conversion during CFG lowering. Making
   ANF explicit in Core would simplify lowering and enable more precise Core analyses.

5. **No effect compilation strategy**: `Perform`/`Handle` remain opaque runtime
   constructs through Core. Proposal 0101 Phase 3 identifies evidence passing as the
   target compilation model but notes it requires typed Core IR.

6. **No unboxing of recursive function arguments**: recursive functions box/unbox
   arguments on every recursive call, even when the argument types are statically known.

### Impact

Programs that chain pattern matches, use small helper functions, or use effects in
loops pay unnecessary overhead. The gap between Flux and production functional language
compilers (GHC, Koka, OCaml) is primarily in optimization depth, not IR architecture.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### How each phase helps

#### Phase 0: Core Module Restructuring

Before adding optimization phases, the Core module's three largest files need
structural cleanup. Together they total 4,855 lines across monolithic files that
are difficult to navigate, review, and extend.

**0a. Split `lower_ast.rs` (1595 lines)**

Split into focused submodules following the pattern established by `type_infer/`
and `diagnostics/`:

```
src/core/lower_ast/
├── mod.rs                — AstLowerer struct, public entry point, top-level/block lowering (~630 lines)
├── expression.rs         — lower_expr() + lower_infix() (~400 lines)
├── pattern.rs            — lower_pattern(), lower_match_arm(), lower_handle_arm(),
│                           expand_destructure_top_level() (~120 lines)
└── binder_resolution.rs  — resolve/validate binder scopes, 6 free functions (~210 lines)
```

Tests remain in `mod.rs` under `#[cfg(test)]` (320 lines). Biggest win:
`lower_expr()` (328 lines, 21 match arms) moves to its own file.

**0b. Split `to_ir.rs` (1756 lines)**

The Core→CFG lowering file is the largest in the Core module. `FnCtx` alone is
~1000 lines with `lower_expr()` (205 lines), `lower_case()` (117 lines),
`lower_primop()` (144 lines), and `emit_pattern_test()` (157 lines).

```
src/core/to_ir/
├── mod.rs                — ToIrCtx struct, lower_core_to_ir(), lower_program(),
│                           lower_core_top_level_item(), finish() (~350 lines)
├── fn_ctx.rs             — FnCtx struct, constructor, block management helpers,
│                           lower_expr(), finish_return() (~350 lines)
├── case.rs               — lower_case(), emit_pattern_test(), bind_pattern() (~310 lines)
├── closure.rs            — lower_lam_as_closure(), lower_handler_arm() (~210 lines)
├── primop.rs             — lower_primop(), primop_to_binop(), lower_lit() (~190 lines)
└── free_vars.rs          — collect_free_vars_core(), free_vars_rec(),
                            collect_pat_binders() (~130 lines)
```

Tests remain in `mod.rs` (~220 lines). The split follows natural boundaries:
global context vs per-function context vs pattern/case compilation vs closures.

**0c. Split `passes.rs` (1065 lines)**

The optimization pass file will grow significantly with Phases 2-6. Current
structure is already straining: `subst()` alone is 146 lines, `map_children()`
is 94 lines, and the test suite is 386 lines. Split by pass + shared infrastructure:

```
src/core/passes/
├── mod.rs                — run_core_passes() pipeline, re-exports (~30 lines)
├── beta.rs               — beta_reduce() (~60 lines)
├── cokc.rs               — case_of_known_constructor(), match_con_pat(),
│                           match_lit_pat() (~160 lines)
├── inline.rs             — inline_trivial_lets() (~30 lines)
├── dead_let.rs           — elim_dead_let() (~30 lines)
├── helpers.rs            — subst(), subst_handler(), map_children(), is_pure(),
│                           appears_free(), pat_binds() (~310 lines)
└── tests.rs              — all unit tests (#[cfg(test)]) (~386 lines)
```

This split is especially valuable because Phases 2-6 each add a new pass file
(e.g., `case_of_case.rs`, `inliner.rs`, `anf.rs`, `evidence.rs`, `worker_wrapper.rs`)
without touching existing pass files.

**0d. `display.rs` (436 lines) — no split needed**

At 436 lines `display.rs` is manageable. It has a clean single-struct design
(`Formatter`) with well-separated concerns (`write_expr`, `write_pat`, `write_alt`,
`write_handler`, plus standalone helpers). No action required now, but if typed
binders (Phase 1) add type-printing logic, consider extracting a `type_display.rs`
at that point.

All Phase 0 work is pure structural refactoring — no behavioral changes.

#### Phase 1: Typed Core Binders

Every `CoreBinder` gains an optional type annotation from HM inference:

```rust
pub struct CoreBinder {
    pub id: CoreBinderId,
    pub name: Identifier,
    pub ty: Option<CoreType>,  // NEW
}
```

This is invisible to Flux users but unlocks all subsequent type-directed passes.
Compiler contributors can now write Core passes that inspect types without threading
external maps.

#### Phase 2: Case-of-Case

Eliminates intermediate constructor allocation when pattern matches are nested:

```flux
-- Before optimization:
fn classify(x) {
    match (match x { 0 -> None; n -> Some(n) }) {
        None    -> "zero"
        Some(n) -> to_string(n)
    }
}

-- After case-of-case:
fn classify(x) {
    match x {
        0 -> "zero"
        n -> to_string(n)
    }
}
```

The intermediate `None`/`Some` values are never constructed.

#### Phase 3: Inlining with Occurrence Analysis

Small functions and single-use bindings are inlined at their call sites:

```flux
fn double(x) { x * 2 }
fn main() { double(21) }

-- After inlining + beta reduction:
fn main() { 21 * 2 }

-- After constant folding:
fn main() { 42 }
```

The inliner uses occurrence analysis to count how many times each binder is
referenced, avoiding code bloat from inlining multiply-used large functions.

#### Phase 4: ANF Normalization

Nested expressions become flat let-chains:

```
-- Before (direct style Core):
PrimOp(Add, [App(f, [x]), Lit(1)])

-- After (ANF Core):
Let(t1, App(f, [x]),
  Let(t2, PrimOp(Add, [Var(t1), Lit(1)]),
    Var(t2)))
```

Every intermediate value has a name, making analysis and lowering simpler.

#### Phase 5: Evidence-Passing for Effects

Tail-resumptive handlers compile to direct function calls:

```flux
-- Source:
handle {
    let x = get()      -- currently allocates continuation
    set(x + 1)         -- currently allocates continuation
} with State { ... }

-- After evidence passing (internal):
-- get() and set() become direct calls with evidence parameter
-- no continuation allocation
```

This builds on proposal 0101 Phase 3 and requires typed Core (Phase 1).

#### Phase 6: Worker/Wrapper Split

Recursive functions get split into a wrapper (handles calling convention, boxing)
and a worker (operates on unboxed values):

```flux
fn sum(xs) {
    match xs {
        []     -> 0
        [h|t]  -> h + sum(t)
    }
}

-- After worker/wrapper (internal):
-- sum_worker operates on unboxed Int accumulator
-- sum is a thin wrapper that boxes the result
```

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Phase 0: Core Module Restructuring

**Scope**: `src/core/lower_ast.rs`, `src/core/to_ir.rs`, `src/core/passes.rs`

All Phase 0 work is pure structural refactoring — no behavioral changes.

#### 0a. Split `lower_ast.rs` (1595 lines) → `src/core/lower_ast/`

##### Current file anatomy

| Section | Lines | Key Methods | Issue |
|---------|-------|-------------|-------|
| Public entry + struct | 1–109 | `lower_program_ast()`, `AstLowerer` | Fine |
| Top-level lowering | 111–307 | `lower_top_level()` (101 lines), `lower_decl_item()` (65 lines) | Moderate |
| Block lowering | 309–532 | `prepend_one_stmt()` (134 lines) | Large method |
| **Expression lowering** | **534–863** | **`lower_expr()` (328 lines, 21 match arms)** | **Largest method** |
| Infix lowering | 865–938 | `lower_infix()` (72 lines) | Coupled to expression |
| Pattern lowering | 940–1061 | 4 methods (~104 lines) | Self-contained |
| Binder resolution | 1064–1273 | 6 free functions (~210 lines) | Self-contained, no `self` |
| Tests | 1275–1595 | 8 tests + 4 helpers | 320 lines |

##### Target module structure

```
src/core/lower_ast/
├── mod.rs                 — AstLowerer struct, constructor, helpers, lower_program_ast(),
│                            lower_top_level(), lower_functions_in_module(), lower_decl_item(),
│                            lower_block(), lower_stmts(), prepend_one_stmt(),
│                            lower_stmt_as_expr(), tests (~630 lines)
├── expression.rs          — lower_expr(), lower_infix() (~400 lines)
├── pattern.rs             — lower_pattern(), lower_match_arm(), lower_handle_arm(),
│                            expand_destructure_top_level() (~120 lines)
└── binder_resolution.rs   — resolve_program_binders(), validate_program_binders(),
                             resolve_expr_binders(), validate_expr_binders(),
                             scope_for_binders(), collect_pattern_binders(),
                             lookup_binder() (~210 lines)
```

##### Extraction order

1. **`binder_resolution.rs`** first — all 6 functions are free functions (no `self`),
   zero coupling to `AstLowerer`. Cleanest extraction.

2. **`pattern.rs`** second — `lower_pattern()`, `lower_match_arm()`, `lower_handle_arm()`,
   and `expand_destructure_top_level()` are `&mut self` methods but only use
   `bind_name()` and `lower_expr()`. Move as `impl AstLowerer` methods in a separate file.

3. **`expression.rs`** third — `lower_expr()` and `lower_infix()` are the largest
   methods. They call into pattern lowering (`lower_match_arm`, `lower_handle_arm`)
   and block lowering (`lower_block`), so they depend on the other modules via `self`.
   Move as `impl AstLowerer` methods with `pub(super)` visibility.

4. **`mod.rs`** retains the struct definition, constructor, `lower_program_ast()`,
   top-level/block lowering, and tests. Visibility: `AstLowerer` fields become
   `pub(super)` so submodules can access them.

##### Visibility changes

```rust
// mod.rs
pub(super) struct AstLowerer<'a> {
    pub(super) hm_expr_types: &'a HashMap<ExprId, InferType>,
    pub(super) fresh: u32,
    pub(super) next_binder_id: u32,
}
```

Methods that are called cross-module become `pub(super)`:
- `bind_name()`, `fresh_binder()`, `expr_type()` — used by expression.rs and pattern.rs
- `lower_expr()` — used by mod.rs (block lowering) and pattern.rs
- `lower_block()` — used by expression.rs (DoBlock lowering)
- `lower_match_arm()`, `lower_handle_arm()` — used by expression.rs

#### 0b. Split `to_ir.rs` (1756 lines) → `src/core/to_ir/`

##### Current file anatomy

| Section | Lines | Key Methods | Issue |
|---------|-------|-------------|-------|
| Public entry + `ToIrCtx` | 26–228 | `lower_core_to_ir()`, `lower_program()`, `finish()` | Moderate |
| Top-level item lowering | 229–346 | `lower_core_top_level_item()`, `bind_function_id_in_items()`, `find_function_decl_metadata()` | Self-contained free functions |
| **`FnCtx` struct + `lower_expr()`** | **348–695** | **`lower_expr()` (205 lines), `emit()`, `with_bound_var()`** | **Largest section** |
| Handler/closure lowering | 696–903 | `lower_handler_arm()` (95 lines), `lower_lam_as_closure()` (113 lines) | Self-contained |
| **PrimOp lowering** | **904–1047** | **`lower_primop()` (144 lines)** | **Large match** |
| **Case compilation** | **1048–1354** | **`lower_case()` (117 lines), `emit_pattern_test()` (157 lines), `bind_pattern()` (60 lines)** | **Largest combined** |
| Pattern/lit/op helpers | 1355–1407 | `is_irrefutable()`, `lower_lit()`, `primop_to_binop()` | Small utilities |
| Free variable analysis | 1408–1536 | `collect_free_vars_core()`, `free_vars_rec()` (105 lines), `collect_pat_binders()` | Self-contained |
| Tests | 1538–1756 | 6 tests + helpers | 218 lines |

##### Target module structure

```
src/core/to_ir/
├── mod.rs                 — ToIrCtx struct, lower_core_to_ir(), lower_program(),
│                            lower_core_top_level_item(), bind_function_id_in_items(),
│                            find_function_decl_metadata(), finish(), tests (~570 lines)
├── fn_ctx.rs              — FnCtx struct, constructor, emit(), with_bound_var(),
│                            new_block(), set_terminator(), current_block_is_open(),
│                            finish_return(), lower_expr() (~350 lines)
├── case.rs                — lower_case(), emit_pattern_test(), bind_pattern(),
│                            is_irrefutable() (~340 lines)
├── closure.rs             — lower_lam_as_closure(), lower_handler_arm() (~210 lines)
├── primop.rs              — lower_primop(), primop_to_binop(), lower_lit() (~190 lines)
└── free_vars.rs           — collect_free_vars_core(), free_vars_rec(),
                             collect_pat_binders() (~130 lines)
```

##### Extraction order

1. **`free_vars.rs`** first — all free functions, no dependency on `ToIrCtx` or `FnCtx`.
   Only imports Core types and `HashSet`. Cleanest extraction.

2. **`primop.rs`** second — `lower_primop()` is a `&mut FnCtx` method but only calls
   `self.emit()` and `self.lower_expr()`. The helpers `primop_to_binop()` and
   `lower_lit()` are free functions.

3. **`case.rs`** third — `lower_case()`, `emit_pattern_test()`, `bind_pattern()` are
   `&mut FnCtx` methods that call each other and `lower_expr()`. Move together.

4. **`closure.rs`** fourth — `lower_lam_as_closure()` and `lower_handler_arm()` call
   `collect_free_vars_core()` (from `free_vars.rs`) and `ToIrCtx` methods for
   function allocation.

5. **`fn_ctx.rs`** fifth — the `FnCtx` struct and its core methods (`lower_expr()`,
   block management). Depends on case/closure/primop via `self` method calls.

##### Visibility changes

```rust
// mod.rs
pub(super) struct ToIrCtx { ... }  // fields pub(super) for fn_ctx.rs/closure.rs access

// fn_ctx.rs
pub(super) struct FnCtx<'a> { ... }  // fields pub(super) for case.rs/primop.rs/closure.rs
```

Key cross-module calls:
- `FnCtx::lower_expr()` — called from case.rs, closure.rs, primop.rs
- `FnCtx::emit()`, `FnCtx::new_block()` — called from case.rs, primop.rs
- `ToIrCtx::alloc_var()`, `alloc_block()`, `alloc_function()` — called from fn_ctx.rs, closure.rs
- `collect_free_vars_core()` — called from closure.rs

#### 0c. Split `passes.rs` (1065 lines) → `src/core/passes/`

##### Current file anatomy

| Section | Lines | Key Functions | Issue |
|---------|-------|---------------|-------|
| Pass pipeline | 1–27 | `run_core_passes()` | Small |
| Beta reduction | 29–94 | `beta_reduce()` | Clean |
| Dead let elimination | 96–130 | `elim_dead_let()` | Clean |
| **Substitution** | **132–294** | **`subst()` (146 lines), `subst_handler()`** | **Largest function** |
| **COKC** | **296–469** | **`case_of_known_constructor()` (91 lines), `match_con_pat()`, `match_lit_pat()`** | **Large combined** |
| Trivial let inlining | 471–515 | `inline_trivial_lets()` | Clean |
| **Helpers** | **517–676** | **`map_children()` (94 lines), `is_pure()`, `appears_free()` (45 lines), `pat_binds()`** | **Shared infrastructure** |
| **Tests** | **678–1065** | **12 tests** | **386 lines (36% of file)** |

##### Target module structure

```
src/core/passes/
├── mod.rs              — run_core_passes() pipeline, re-exports (~30 lines)
├── beta.rs             — beta_reduce() (~60 lines)
├── cokc.rs             — case_of_known_constructor(), match_con_pat(),
│                         match_lit_pat() (~160 lines)
├── inline.rs           — inline_trivial_lets() (~30 lines)
├── dead_let.rs         — elim_dead_let() (~30 lines)
├── helpers.rs          — subst(), subst_handler(), map_children(), is_pure(),
│                         appears_free(), pat_binds() (~310 lines)
└── tests.rs            — all unit tests (#[cfg(test)]) (~386 lines)
```

##### Extraction order

1. **`helpers.rs`** first — `subst()`, `subst_handler()`, `map_children()`, `is_pure()`,
   `appears_free()`, `pat_binds()` are all module-level free functions used by multiple
   passes. Make them `pub(super)`.

2. **`dead_let.rs`** + **`inline.rs`** + **`beta.rs`** second — each pass is a single
   public function that calls into `helpers.rs`. No cross-dependencies between passes.

3. **`cokc.rs`** third — `case_of_known_constructor()` + `match_con_pat()` +
   `match_lit_pat()` form a self-contained unit. Uses `subst()` from helpers.

4. **`tests.rs`** last — move all `#[cfg(test)]` code. Tests import from sibling modules.

5. **`mod.rs`** retains only `run_core_passes()` and `pub use` re-exports for each pass.

##### Why split now (not later)?

Phases 2-6 each add a new optimization pass. Without the split, each new pass would
append ~100-300 lines to an already 1065-line file. With the split, each new pass gets
its own file (`case_of_case.rs`, `inliner.rs`, `anf.rs`, `evidence.rs`,
`worker_wrapper.rs`) and `mod.rs` simply adds it to the pipeline.

#### 0d. `display.rs` (436 lines) — no split needed

At 436 lines with a clean single-struct design (`Formatter`), `display.rs` is
manageable. If typed binders (Phase 1) add significant type-printing logic,
consider extracting `type_display.rs` at that point.

#### Phase 0 validation

After each sub-phase (0a, 0b, 0c), run:
```bash
cargo test --all --all-features    # Full suite (~915 tests)
cargo test --test parser_tests     # Heavy user of Core lowering
cargo clippy --all-targets --all-features -- -D warnings
```

No behavioral changes — all splits are pure structural refactoring.

### Phase 1: Typed Core Binders

**Scope**: `src/core/mod.rs`, `src/core/lower_ast/`

#### New types

```rust
/// Core-level type representation.
/// Simplified from HM InferType — no unification variables, no quantifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreType {
    Int,
    Float,
    Bool,
    String,
    Unit,
    List(Box<CoreType>),
    Array(Box<CoreType>),
    Tuple(Vec<CoreType>),
    Function(Vec<CoreType>, Box<CoreType>),
    Adt(Identifier, Vec<CoreType>),
    /// Type variable (polymorphic, not yet monomorphized)
    Var(u32),
    /// Unknown / not yet populated
    Any,
}
```

#### Modified structures

```rust
pub struct CoreBinder {
    pub id: CoreBinderId,
    pub name: Identifier,
    pub ty: Option<CoreType>,  // Added
}

pub struct CoreDef {
    pub name: Identifier,
    pub binder: CoreBinder,
    pub expr: CoreExpr,
    pub result_ty: Option<CoreType>,  // Added: return type
    pub is_anonymous: bool,
    pub is_recursive: bool,
    pub span: Span,
}
```

#### Population strategy

Types are populated during `lower_ast` when HM inference results are available.
The `lower_program_ast` function already receives the typed program; it should
propagate resolved types onto binders during lowering. When type information is
not available (e.g., polymorphic or external), `ty` remains `None`.

#### Validation

Add a Core validation pass that checks:
- All binders in monomorphic positions have `ty = Some(...)`
- PrimOp argument types match the operation (e.g., `IAdd` requires `Int` operands)
- `Case` scrutinee type is consistent with alternative patterns

### Phase 2: Case-of-Case Transformation

**Scope**: `src/core/passes.rs`

#### Algorithm

```
case_of_case(expr):
  match expr:
    Case { scrutinee: Case { scrutinee: inner, alts: inner_alts }, alts: outer_alts }:
      // Push outer case into each inner arm
      for alt in inner_alts:
        alt.rhs = Case { scrutinee: alt.rhs, alts: outer_alts.clone() }
      return Case { scrutinee: inner, alts: inner_alts }
    _ -> map_children(expr, case_of_case)
```

#### Constraints

- Only apply when inner `Case` alternatives are **not guarded** (guards may have
  side effects that should not be duplicated).
- The outer alternatives are cloned into each inner arm. This is acceptable because
  Core expressions are cheaply cloneable (they use `Identifier` which is `Copy`).
- After case-of-case, run `case_of_known_constructor` again — the transformation
  often exposes new COKC opportunities.
- Add a size limit: do not duplicate outer alternatives if their combined size
  exceeds a threshold (prevents code explosion).

#### Interaction with existing passes

Insert after `beta_reduce`, before `case_of_known_constructor`:

```
beta_reduce → case_of_case → case_of_known_constructor → inline_trivial_lets → elim_dead_let
```

### Phase 3: Inlining with Occurrence Analysis

**Scope**: `src/core/passes.rs` (new functions)

#### Occurrence analysis

Walk the Core program and count, for each `CoreBinderId`:

```rust
pub enum OccInfo {
    Dead,           // 0 occurrences
    Once(bool),     // exactly 1 occurrence; bool = inside lambda?
    Multi,          // 2+ occurrences
    LoopBreaker,    // recursive binding in SCC — do not inline
}
```

Store as `HashMap<CoreBinderId, OccInfo>`.

#### Inlining decisions

| OccInfo | Size | Decision |
|---------|------|----------|
| `Dead` | any | Eliminate (dead code) |
| `Once(false)` | any | Always inline (no duplication) |
| `Once(true)` | any | Inline only if small (inside lambda = may execute multiple times) |
| `Multi` | ≤ threshold | Inline (small enough to duplicate) |
| `Multi` | > threshold | Do not inline |
| `LoopBreaker` | any | Never inline (would create infinite loop) |

Size is measured as `CoreExpr` node count. Suggested initial threshold: 10 nodes.

#### SCC analysis for recursive bindings

Use Tarjan's SCC on `LetRec` binding groups. In each SCC, pick one binding as the
"loop breaker" that will not be inlined. Prefer the largest binding as loop breaker
(minimizes total code size after inlining the rest).

#### Pipeline position

```
occurrence_analysis → beta_reduce → case_of_case → case_of_known_constructor
    → inline → elim_dead_let
```

Run the entire pipeline iteratively (up to N rounds, default 4) until no changes
occur, following GHC's simplifier model.

### Phase 4: ANF Normalization Pass

**Scope**: `src/core/passes.rs` (new function)

#### Transformation

For each `CoreExpr` in non-tail position, if the subexpression is "non-trivial"
(not `Var`, `Lit`, or `Con` with trivial fields), bind it to a fresh `CoreBinder`:

```rust
fn anf_normalize(expr: CoreExpr, gen: &mut BinderGen) -> CoreExpr {
    match expr {
        CoreExpr::App { func, args, span } => {
            let (func_binds, func_var) = anf_atom(func, gen);
            let (arg_binds, arg_vars) = anf_atoms(args, gen);
            let app = CoreExpr::App { func: func_var, args: arg_vars, span };
            wrap_lets(func_binds.chain(arg_binds), app)
        }
        CoreExpr::PrimOp { op, args, span } => {
            let (binds, arg_vars) = anf_atoms(args, gen);
            wrap_lets(binds, CoreExpr::PrimOp { op, args: arg_vars, span })
        }
        // ... similar for other compound forms
    }
}
```

#### Trivial expressions (no binding needed)

- `Var`
- `Lit`
- `Con` with all-trivial fields

#### Impact on `to_ir.rs`

After ANF, every non-trivial subexpression is `Let`-bound. This means `to_ir.rs`
can lower each `Let` to a single `IrInstr` assignment without recursive flattening.
Estimated reduction: ~400-500 lines from `to_ir.rs`.

### Phase 5: Evidence-Passing for Effects

**Scope**: `src/core/passes.rs` (new pass), new `src/core/evidence.rs`

This phase implements proposal 0101 Phase 3, which requires typed Core (Phase 1).

#### Tail-resumptive detection

A handler is tail-resumptive if every arm has the form:

```
handler_arm(params..., resume):
    resume(some_expr)
```

Where `resume` appears exactly once, in tail position, with a single argument.

#### Evidence translation

For tail-resumptive handlers, rewrite:

```
Handle {
    body: ... Perform(Effect, op, args) ...,
    effect: Effect,
    handlers: [handler_arms]
}
```

Into:

```
Let(ev, Lam([op_tag, args...], dispatch_to_handler_arms),
  Let(body_result, App(body_as_fn, [ev]),
    body_result))
```

Where `Perform(Effect, op, args)` inside the body becomes `App(ev, [op_tag, args])`.

#### Prerequisites

- Phase 1 (typed binders): needed to verify that handler arm return types are
  consistent and that evidence parameters are correctly typed.
- Proposal 0101 Phases 1-2: tail-resumptive detection and static handler resolution
  should land first at the bytecode level before the Core-level evidence translation.

### Phase 6: Worker/Wrapper Split

**Scope**: `src/core/passes.rs` (new pass)

#### When to apply

A recursive function is a worker/wrapper candidate when:
1. It has typed binders (Phase 1)
2. At least one parameter has a known concrete type (`Int`, `Float`, `Bool`)
3. The function is self-recursive (not mutually recursive, initially)

#### Transformation

```
-- Original:
LetRec f = Lam([x: Int, acc: Int], body)

-- After split:
Let f = Lam([x, acc], f_worker(x, acc))   -- wrapper
LetRec f_worker = Lam([x: Int, acc: Int],
    body[f -> f_worker])                    -- worker with unboxed args
```

The worker operates on unboxed values. The wrapper handles the boxing/unboxing
at the entry boundary. Recursive calls within the worker call the worker directly,
avoiding re-boxing.

#### Interaction with JIT

The JIT backend already has `JitValueKind::Int` for unboxed integers. Worker/wrapper
at the Core level feeds directly into the JIT's unboxed representation, making the
JIT's own unboxing logic simpler.

### Revised Pass Pipeline

After all phases, the Core pass pipeline becomes:

```
Phase 0 (existing):
  lower_ast → Core

Phase 1-4 (Core-to-Core simplifier, iterated up to 4 rounds):
  occurrence_analysis
  → beta_reduce
  → case_of_case            (Phase 2)
  → case_of_known_constructor
  → inline                  (Phase 3)
  → inline_trivial_lets
  → elim_dead_let

Phase 4:
  → anf_normalize

Phase 5 (effect compilation):
  → evidence_translate      (only for tail-resumptive handlers)

Phase 6 (specialization):
  → worker_wrapper_split

Post-optimization:
  → core_validate (type-aware validation, Phase 1)
  → lower_core_to_ir
```

### Implementation Schedule

| Phase | Estimated Scope | Prerequisites | Risk |
|-------|----------------|---------------|------|
| 0. Module split | ~0 new lines (restructure only) | None | Very low — pure refactoring |
| 1. Typed binders | ~300 lines new/modified | Phase 0 (cleaner diffs) | Low — additive change |
| 2. Case-of-case | ~150 lines new | None (benefits from Phase 1) | Low — well-understood transform |
| 3. Inlining | ~400 lines new | Phase 2 (for full benefit) | Medium — needs size heuristics |
| 4. ANF | ~200 lines new | None | Low — mechanical transform |
| 5. Evidence passing | ~500 lines new | Phase 1, proposal 0101 P1-P2 | Medium — semantic complexity |
| 6. Worker/wrapper | ~300 lines new | Phase 1 | Medium — needs JIT coordination |

### Testing Strategy

Each phase adds:

1. **Unit tests**: Core-to-Core transformation correctness (input Core → expected output Core)
2. **Round-trip tests**: `display_program_debug` snapshots before/after each pass
3. **End-to-end tests**: Flux programs that exercise the optimization, verified via
   `--test` runner (same output with and without optimization)
4. **Regression tests**: Programs where the optimization must *not* fire (e.g., side-effectful
   scrutinees in case-of-case)

Use `insta` snapshot tests for Core IR dumps, following existing `tests/snapshots/` convention.

### Metrics

Track optimization impact via `--stats` flag additions:

- Core pass timings (per-pass and total)
- Optimization counts (beta reductions, inlines, case-of-case fires, etc.)
- Core program size before/after (node count)
- Effect handler compilation mode (continuation vs evidence) per handler

## Drawbacks
[drawbacks]: #drawbacks

1. **Complexity**: Six phases add substantial compiler complexity. Each pass must be
   correct, and pass interactions must be tested.

2. **Compile time**: More Core passes increase compilation time. Mitigation: each pass
   is optional and can be gated behind `-O` levels.

3. **Debugging difficulty**: More transformations between source and execution make it
   harder to correlate runtime behavior with source code. Mitigation: Core IR dumps
   (`--core-ir`) and span preservation.

4. **Typed binder maintenance**: Every future Core transformation must preserve type
   annotations. This is a perpetual maintenance cost.

5. **ANF increases Core size**: Every subexpression gets a `Let` binding, increasing
   the node count. This is offset by simpler lowering and analysis.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Why this ordering?

The module split (Phase 0) is first because every subsequent phase adds code to the
Core module. Working in a 1595-line monolith makes diffs harder to review, increases
merge conflict risk, and obscures logical boundaries. The split is zero-risk (pure
restructure, no behavioral change) and pays for itself immediately.

Typed binders are next because they **unlock** Phases 2, 3, 5, and 6. Without types,
case-of-case cannot validate that the transformation preserves typing, inlining cannot
make type-directed decisions, and evidence passing requires type information.

Case-of-case is second because it is the highest-value single optimization (per GHC
experience) and is relatively simple to implement.

Inlining is third because it exposes opportunities for all other passes (beta reduction,
COKC, dead code elimination) but needs case-of-case in place to fully benefit.

ANF is fourth (not first) because the existing `to_ir.rs` handles non-ANF Core
correctly. ANF simplifies lowering but is not a prerequisite for correctness.

Evidence passing is fifth because it depends on typed binders and benefits from the
earlier optimization passes reducing handler complexity before the translation.

Worker/wrapper is last because it is the most specialized optimization, benefiting
primarily recursive numeric code.

### Why not CPS instead of ANF?

CPS (continuation-passing style) is an alternative to ANF that makes control flow
equally explicit. However:

- ANF is simpler to implement and reason about
- ANF preserves direct-style structure, making Core IR dumps more readable
- ANF and CPS have equivalent expressiveness for our purposes (Flanagan et al. 1993)
- Koka uses CPS internally but this is tied to its effect compilation; Flux can
  achieve the same effect optimization via evidence passing without full CPS

### Why not implement all passes at the CFG level?

The CFG (backend IR) level already has 7 passes. However:

- CFG passes operate on low-level instructions, making high-level transformations
  (inlining, case-of-case) harder to express
- Core is closer to source semantics, so transformations are easier to verify
- Core passes benefit both backends (VM and JIT) equally
- GHC, Koka, and OCaml all do their heaviest optimization at the Core/high-level IR
  layer, not at the CFG/low-level layer

### What is the impact of not doing this?

Flux programs will continue to work correctly but will:
- Allocate unnecessary intermediate constructors in nested pattern matches
- Miss inlining opportunities that expose further simplification
- Pay full continuation cost for tail-resumptive effect handlers
- Box/unbox recursive function arguments unnecessarily

## Prior art
[prior-art]: #prior-art

### GHC (Haskell) — Core Simplifier

GHC's simplifier is the most mature functional language optimizer. Key lessons:

- **Iterative simplification**: GHC runs its simplifier 4+ times, interleaving
  inlining, beta reduction, case-of-case, and dead code elimination. Each round
  exposes opportunities for the next. Flux should adopt this iterative model.

- **Occurrence analysis**: GHC counts occurrences before each simplifier round to
  guide inlining. This is more principled than size-based heuristics alone.

- **Case-of-case is critical**: Simon Peyton Jones identifies this as the single
  most impactful optimization in GHC. It eliminates intermediate constructors that
  arise naturally from composed functions.

- **Worker/wrapper**: GHC's demand analysis + worker/wrapper achieves significant
  speedups for recursive numeric code by avoiding boxing on every recursive call.

Reference: "Secrets of the Glasgow Haskell Compiler inliner" (Peyton Jones & Marlow, JFP 2002)

### Koka — Evidence Passing

Koka compiles algebraic effects via evidence passing, translating handler install
sites into evidence vector construction and `perform` operations into evidence
lookups. Key lessons:

- **Tail-resume optimization is the 80/20**: most real-world handlers are
  tail-resumptive. Optimizing this case eliminates the bulk of effect overhead.

- **Evidence is first-class**: evidence values can be passed, stored, and
  specialized. This enables further optimization (inlining handler code).

- **FBIP (functional but in place)**: Koka's reuse analysis for RC'd values
  maps directly to Flux's `Rc::try_unwrap` pattern. This could be a Phase 7.

Reference: "Evidence Passing Semantics for Effect Handlers" (Xie & Leijen, 2021)

### OCaml — Flambda

OCaml's Flambda optimizer sits between Lambda IR and Cmm (C--). Key lessons:

- **Closure representation matters**: Flambda specializes closure layouts based on
  capture count and usage patterns. Flux's `MakeClosure` could benefit similarly.

- **Unboxing floats**: Flambda2 aggressively unboxes float arrays and float-returning
  functions. Flux's `type_directed_unboxing` CFG pass is analogous.

- **ANF as the optimization form**: Flambda2 uses ANF internally, validating the
  choice for Phase 4.

### Rust — MIR

Rust's MIR is a CFG-based IR designed for borrow checking and optimization. Key lessons:

- **Place projections**: MIR's `Place` system (local + field/deref/index projections)
  is relevant if Flux ever adds mutable records or linear types.

- **Drop elaboration**: MIR explicitly represents destructor calls. Flux's GC makes
  this unnecessary, but the principle of making resource management explicit in IR
  applies to Flux's effect handler lifetime management.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

1. **CoreType representation**: Should `CoreType` be a simplified version of
   `InferType` (as proposed) or a direct reuse of the existing type system types?
   A simplified type avoids coupling Core to the inference engine but requires a
   translation step.

2. **Inlining threshold tuning**: The initial threshold of 10 nodes is a guess.
   What is the right threshold for Flux programs? This should be determined
   empirically using the benchmark suite.

3. **ANF timing**: Should ANF normalize before or after inlining? Before: inlining
   sees uniform structure. After: avoids creating lets that inlining would immediately
   remove. GHC does not use ANF (it uses direct style with a simplifier); Flambda2
   normalizes before optimization. Proposed: normalize after the iterative simplifier,
   before lowering.

4. **Evidence passing scope**: Should Phase 5 handle only tail-resumptive handlers
   (conservative, high-value) or also multi-shot/non-tail handlers (full generality,
   higher complexity)? Proposal 0101 suggests starting conservative.

5. **Iterative pass count**: How many simplifier rounds are needed? GHC uses 4 by
   default. Flux programs are typically smaller, so 2-3 may suffice. Should be
   configurable via `-O` levels.

6. **`CoreType` for polymorphic code**: How should polymorphic bindings be typed?
   `CoreType::Var(u32)` is proposed but the interaction with monomorphization (if
   ever added) needs consideration.

## Future possibilities
[future-possibilities]: #future-possibilities

- **Demand analysis**: GHC-style strictness/demand analysis to identify which
  function arguments are always evaluated, enabling call-by-value optimization
  and further worker/wrapper opportunities.

- **Reuse analysis (FBIP)**: Koka-style functional-but-in-place optimization,
  detecting when `Rc` values have refcount 1 and can be mutated in place. This
  extends the existing `Rc::try_unwrap` pattern in `OpAdtField` to a systematic
  Core-level analysis. See proposal 0068 (Perceus).

- **Specialization / monomorphization**: Generate type-specialized versions of
  polymorphic functions to eliminate boxing entirely. Worker/wrapper (Phase 6)
  is a stepping stone toward this.

- **Join points**: GHC-style join points — let-bindings that are always called in
  tail position — enable case-of-case without code duplication. Instead of cloning
  outer alternatives into each inner arm, create a join point and jump to it.

- **Constructor specialization (SpecConstr)**: When a recursive function always
  pattern-matches on a constructor argument, specialize the function for each
  constructor, eliminating the match overhead entirely.

- **Common subexpression elimination at Core level**: The CFG layer already has
  local CSE; a Core-level CSE could catch opportunities before lowering.

- **`-O` optimization levels**: Gate optimization aggressiveness:
  - `-O0`: no Core passes (fastest compilation)
  - `-O1`: beta + COKC + inline trivial + DCE (current default)
  - `-O2`: add case-of-case, inlining, ANF, evidence passing
  - `-O3`: add worker/wrapper, extra simplifier rounds
