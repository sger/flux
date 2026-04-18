- Feature Name: Core IR Parity Simplification
- Start Date: 2026-04-14
- Status: Implemented (2026-04-18)
- Proposal PR:
- Flux Issue:
- Depends on: 0102 (Core IR Optimization Roadmap), 0153 (Decouple Aether from Core IR)
- Closed under: [0160 (Static Typing Hardening Closure)](0160_static_typing_hardening_closure.md)
- Related implemented work: [0127 (Type Inference Completion Roadmap)](0127_type_inference_ghc_parity.md), [0159 (Signature-Directed Checking and Skolemisation)](0159_signature_directed_checking_and_skolemisation.md)
- Delivery commits: `9347ac8c` (core_lint introduced), `38259d1c` (E998 promotion + five new passes), `979cfbb3` (parity fixes)

# Proposal 0155: Core IR Parity Simplification

## Summary
[summary]: #summary

Add a focused Core IR optimization package that improves VM/native parity without changing the IR architecture. The package introduces six concrete pieces:

1. `core_lint` — validate Core invariants after major passes
2. `algebraic_simplify` — apply backend-independent arithmetic and boolean rewrites
3. `constant_fold` — fold Core-level literal and known-branch expressions introduced after AST lowering
4. `call_and_case_canonicalize` — normalize semantically equivalent call and match shapes before `aether`
5. `disciplined_inline` — tighten Core inlining policy around dead, single-use, and tiny pure bindings
6. `specialize_known_shapes` — specialize narrow Core patterns that are semantic and backend-independent

This proposal does **not** add a new IR and does **not** unify `cfg` and `lir`. It strengthens `core` as the shared semantic contract that feeds both backends.

## Motivation
[motivation]: #motivation

Flux already has one canonical semantic pipeline:

```text
AST -> Core -> Aether
                -> CFG -> bytecode/vm
                -> LIR -> llvm/native
```

That architecture is sound. The remaining problem is backend parity, not a missing IR stage.

Today, many parity mismatches happen because `core` still permits too many equivalent shapes to reach backend lowering:

- arithmetic identities survive into backend IR
- constant conditions introduced by inlining are not always removed in `core`
- constructor/literal knowledge is reduced in some cases but not normalized aggressively enough across all equivalent forms
- backend lowerings still have to clean up semantic noise that should have been removed before `aether`

This has three costs:

1. **Parity risk**: VM and native may lower semantically equivalent Core expressions through different backend-specific cleanup paths.
2. **Optimization duplication**: `cfg` and `lir` both inherit avoidable work from `core`.
3. **Debugging friction**: when `--dump-core` still contains administrative noise, it is harder to tell whether a bug is semantic or backend-local.

The problem to solve is therefore:

> Make `core` smaller, stricter, and more normalized so that `aether`, `cfg`, and `lir` consume fewer semantically redundant shapes.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

Compiler contributors should think of this proposal as a **Core cleanup package**, not a redesign.

After this proposal, the expectation is:

- `--dump-core` should already remove obvious semantic noise
- `aether` should focus on ownership/reuse lowering, not on compensating for weak Core normalization
- VM/native parity work should start from `core` and `aether`, not from backend-local patching

### What gets added

#### 1. `core_lint`

`core_lint` runs after major semantic passes and checks that Core stays inside its intended contract.

Examples of checks:

- no backend-only constructs appear in Core
- binder references remain valid after rewrites
- case branches remain type/shape-consistent
- rewrites do not introduce malformed let or application structure

This is a contributor-facing debugging tool and should be used the same way `--dump-core` is used today: early and often.

#### 2. `algebraic_simplify`

This pass performs small, local rewrites that are true regardless of backend:

```text
x + 0  -> x
0 + x  -> x
x - 0  -> x
x * 1  -> x
1 * x  -> x
x * 0  -> 0
0 * x  -> 0
x / 1  -> x
x % 1  -> 0
not (not x) -> x
x && true   -> x
x && false  -> false
x || false  -> x
x || true   -> true
```

These rules belong in `core` because they are semantic equalities, not backend peepholes.

#### 3. `constant_fold`

This pass folds Core expressions that become constant only after Core rewriting.

Examples:

```text
if true then a else b      -> a
if false then a else b     -> b
case 3 of 3 -> a; _ -> b   -> a
case Some(1) of ...        -> reduced by known-scrutinee matching
3 + 4                      -> 7
5 < 9                      -> true
```

Flux already performs some folding earlier in the pipeline, but this pass is specifically for opportunities that appear **after** inlining, beta reduction, or case rewriting.

#### 4. `call_and_case_canonicalize`

This pass reduces semantically equivalent Core shapes to a preferred form before `aether`.

Targets include:

- normalize known builtin/primop calls to the canonical Core representation expected by later passes
- normalize simple wrapper applications exposed by inlining
- collapse case structure when the same semantic match is represented through equivalent administrative forms

This is intentionally narrower than a general optimizer. Its purpose is to reduce parity-sensitive shape variance.

#### 5. `disciplined_inline`

This proposal keeps inlining in `core`, but narrows it to shapes that are high-value and low-risk for parity:

- dead bindings
- single-use pure bindings
- tiny pure wrappers
- trivial forwarders around constructors, primops, or direct calls

The goal is not “inline more everywhere”. The goal is:

- expose simplification opportunities earlier
- avoid backend-local wrapper cleanup
- keep code-size growth under control

#### 6. `specialize_known_shapes`

This proposal also adds a narrow specialization layer for Core shapes that are already semantically known before `aether`.

Initial targets:

- known constructor applications
- known builtin/primop calls
- trivial wrapper functions whose body is just a specialized call or constructor forwarder

This is not general monomorphization and not backend specialization. It is only specialization of already-known Core forms so later passes and both backends see fewer indirect or administrative shapes.

### What does not change

- no new IR stage is introduced
- no backend semantics move into Core
- no VM-specific bytecode optimizations move into Core
- no LLVM-specific lowering details move into Core
- `cfg` and `lir` remain separate

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### New passes

Add the following passes under `src/core/passes/`:

- `algebraic.rs`
- `const_fold.rs`
- `canonicalize.rs`
- `disciplined_inline.rs`
- `specialize.rs`
- `lint.rs`

Suggested public entry points:

```rust
pub use algebraic::algebraic_simplify;
pub use canonicalize::call_and_case_canonicalize;
pub use const_fold::constant_fold;
pub use disciplined_inline::disciplined_inline;
pub use lint::core_lint;
pub use specialize::specialize_known_shapes;
```

### Pass order

The semantic Core pipeline should become:

```text
promote_builtins
repeat up to N rounds:
  beta_reduce
  case_of_case
  case_of_known_constructor
  call_and_case_canonicalize
  specialize_known_shapes
  algebraic_simplify
  constant_fold
  disciplined_inline
  inline_lets
  elim_dead_let
  core_lint
evidence_pass
anf_normalize
core_lint
```

Rationale for the order:

- `call_and_case_canonicalize` runs before simplification so later passes see fewer equivalent forms
- `specialize_known_shapes` runs before algebraic and constant simplification so specialized forms can be simplified immediately
- `algebraic_simplify` runs before `constant_fold` so identity rewrites expose literal-only expressions
- `constant_fold` runs before inlining so single-use constants become easier to inline
- `disciplined_inline` runs before `inline_lets` so the new policy owns the non-trivial but still safe inline cases
- `core_lint` runs at pass boundaries, not only at the end, so regressions are localized to one pass

### Scope of `algebraic_simplify`

This proposal limits the initial rule set to:

- additive identity
- multiplicative identity
- multiplicative annihilator
- division/modulo by one
- double negation for boolean `not`
- short-circuit boolean identities with literal operands

Out of scope for the first implementation:

- reassociation (`(x + 1) + 2 -> x + 3`)
- commutative operand reordering for cost modeling
- strength reduction (`x * 8 -> x << 3`)
- backend-sensitive integer-width rewrites

### Scope of `constant_fold`

This pass should fold only when:

- all required operands are Core literals
- the operation is pure
- the result is representable directly as a Core literal or known constructor/literal branch

Initial supported forms:

- integer arithmetic on literal operands
- integer comparisons on literal operands
- boolean operators on literal operands
- `if`/`case` on known boolean or literal conditions
- reduction of constructor/literal scrutinees already normalized by earlier passes

It must not:

- duplicate work with effects
- speculate about partial operations beyond existing language semantics
- encode backend-specific overflow behavior

### Scope of `call_and_case_canonicalize`

This pass is intentionally conservative.

Examples of acceptable normalization:

- prefer canonical primop call encoding over backend-specific call-shaped equivalents
- strip trivial administrative wrappers around known calls when the wrapper body is a direct forwarder
- normalize known-case scrutinee forms before `case_of_known_constructor`

Examples of non-goals:

- full devirtualization
- closure-conversion-style rewrites
- backend calling-convention shaping

### Scope of `disciplined_inline`

This pass should inline only when one of the following holds:

- the binding is dead and pure
- the binding has one use and is pure
- the RHS is a tiny pure expression under a configurable size threshold
- the RHS is a direct wrapper around:
  - a constructor
  - a primop
  - a known direct call

It must avoid:

- recursive inlining
- effectful RHS duplication
- large-expression code-size blowup
- backend-sensitive heuristics

The existing `inline_lets` pass may remain as a trivial fallback/cleanup pass, but the policy for non-trivial Core inlining should be centralized here.

### Scope of `specialize_known_shapes`

This pass is intentionally narrow.

Allowed specializations:

- rewrite known builtin wrappers to direct canonical Core primop forms
- rewrite trivial constructor wrappers to the constructor application directly
- simplify direct-forwarding wrappers that only repackage known arguments without adding semantics

Not allowed:

- type-driven monomorphization
- representation-specific unboxing
- backend calling-convention specialization
- closure conversion or environment splitting

### `core_lint` contract

`core_lint` should verify at least:

- all binders are in scope where referenced
- `Let` groups remain structurally valid after substitution and inlining
- `Case` expressions have non-empty branch sets when required by the Core contract
- branch guards and result expressions are well-formed Core expressions
- no backend-only ownership/runtime constructs appear in Core

`core_lint` is not a typechecker replacement. It is a structural and semantic-shape validator for post-lowering Core.

### Testing plan

Add:

- focused unit tests for each rewrite family under `src/core/passes/tests.rs`
- snapshot coverage for `--dump-core` on examples that exercise:
  - arithmetic identities
  - literal branch folding
  - known-constructor case reduction after inlining
  - canonicalized call shapes
  - disciplined wrapper inlining
  - known-shape specialization
- parity fixtures where the old behavior produced different VM/native backend shapes downstream

Recommended new fixtures:

- `tests/parity/core_algebraic_identities.flx`
- `tests/parity/core_constant_branching.flx`
- `tests/parity/core_wrapper_call_normalization.flx`
- `tests/parity/core_disciplined_inline.flx`
- `tests/parity/core_known_shape_specialization.flx`

## Drawbacks
[drawbacks]: #drawbacks

- Adds more passes to the Core pipeline.
- Introduces more pass-order coupling, especially around inlining and case simplification.
- A weakly specified `canonicalize` pass could become a vague cleanup bucket unless its scope stays narrow.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Why this design

This proposal targets the highest-leverage place in the current architecture:

- `core` is already the shared semantic IR
- `aether` already sits at the shared ownership/reuse boundary
- `cfg` and `lir` are intentionally backend-specific

Improving `core` therefore benefits both maintained backend families without forcing a new low-level IR.

### Why not add a new IR

That would increase surface area without solving the immediate problem. The current issue is insufficient normalization of shared semantics, not absence of another compiler stage.

### Why not move these rewrites to `cfg` or `lir`

Because these rewrites are semantic equalities. If they are delayed until backend lowering, parity relies on both backends rediscovering the same facts independently.

### Why not do only `core_lint`

Lint alone improves debugging, but it does not reduce backend work or parity-sensitive shape variance. The simplification passes are the actual optimization payload.

### Why include disciplined inlining here

Inlining is already part of Core optimization. The issue is not whether Flux should inline, but whether the inlining policy is explicit and parity-friendly. This proposal narrows inlining to cases that reliably expose shared simplification opportunities without pushing cost-model complexity into backends.

### Why include specialization here

The proposal includes only specialization that is already semantically justified in Core. This improves parity by reducing indirect wrapper shapes before backend lowering, while avoiding the broader complexity of type-driven specialization.

## Prior art
[prior-art]: #prior-art

This proposal follows a common compiler pattern:

- keep one shared semantic IR
- normalize it aggressively before backend-specific lowering
- validate the IR at pass boundaries
- reserve backend-specific cleanup for backend-specific concerns

The proposal intentionally avoids importing another compiler's stage structure directly.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- Should `core_lint` run unconditionally in all builds, or only in debug/dev configurations?
- Should `constant_fold` remain a standalone pass, or should some of its rules fold into `algebraic_simplify` once stabilized?
- Which exact Core call forms should `call_and_case_canonicalize` own in its first iteration?
- Should `disciplined_inline` replace `inline_lets`, or should both remain with separate responsibilities?
- Should `specialize_known_shapes` run before or after `case_of_known_constructor` for best simplification exposure?
- Should `--dump-core=debug` annotate which simplification pass last changed each node or binding?

## Future possibilities
[future-possibilities]: #future-possibilities

- Extend `algebraic_simplify` with safe reassociation once Core cost modeling is stronger.
- Add a tiny pure-expression CSE pass if canonicalization makes structural equality cheap enough.
- Add per-pass dump hooks for easier parity debugging.
- Add a dedicated `core_lint` CLI/debug surface if pass-boundary validation becomes central to debugging workflow.
- Expand disciplined inlining into a more formal occurrence-based policy once Core size/cost metrics are stronger.
- Broaden specialization only if a clear semantic-only subset remains distinct from backend optimization.
