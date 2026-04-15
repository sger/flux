- Feature Name: Static Typing Completion Roadmap
- Start Date: 2026-04-14
- Status: Draft
- Proposal PR:
- Flux Issue:
- Depends on: 0123 (Full Static Typing), 0147 (Constrained Type Params and Instance Contexts)

# Proposal 0156: Static Typing Completion Roadmap

## Summary
[summary]: #summary

Record the completion of Flux's transition from a gradual HM system with `Any` escape hatches to a genuinely static type system across parsing, inference, module boundaries, and runtime type translation.

This proposal does **not** restart static typing from scratch. It treats the current repo state as the baseline:

- typed Core/Aether/native infrastructure exists
- operator desugaring is implemented
- HKT instance resolution is implemented
- base HM signature tightening is implemented
- type classes, deriving, and dictionary elaboration exist

The maintained front-end semantic work is complete. Related follow-on work is
now split clearly:

1. `0155` owns Core validation follow-on work such as `core_lint`
2. `0157` explains the semantic-vs-representation split
3. `0158` executes removal of downstream semantic `Dynamic` placeholders
4. `0159` owns signature-directed checking and skolemisation follow-on work
5. `0160` owns the final hardening and closure criteria across scheme surfaces,
   Core validation, and checked-signature completion

## Implementation status
[implementation-status]: #implementation-status

Last updated: 2026-04-14

### Completed prerequisites

| Item | Status | Notes |
|---|---|---|
| 0149 operator desugaring | Done | Implemented in AST/type inference pipeline |
| 0150 HKT instance resolution | Done | Constructor-headed HKT instances resolve |
| 0074 base HM signatures | Done | Core builtins are substantially tighter than old `Any` signatures |
| typed Core/Aether/native groundwork | Done enough | Present in proposal 0123 and current code |

### Remaining phases

| Phase | Focus | Status |
|---|---|---|
| 0 | Roadmap alignment | This proposal |
| 1 | Source-level class semantics completion | Complete |
| 2 | HM `Any` elimination | Complete |
| 3 | Strict mode becomes semantic | Complete |
| 4 | Remove stdlib/module exclusions | Complete |
| 5 | Runtime translation + structural gaps | Complete |

## Motivation
[motivation]: #motivation

Flux used to have a split personality:

- **the proposal/docs story** says full static typing is complete
- **the implementation reality** still contained a live gradual escape hatch via `Any`

That implementation gap has now been closed:

- source annotations no longer accept `Any`
- HM inference no longer uses `Any` as a maintained-path fallback
- runtime boundary lowering no longer reintroduces `Any`
- module/interface strictness is part of semantic/cache identity

The remaining distinction is architectural:

1. **Static typing is now semantic truth in the maintained front-end pipeline**
   - strict typing is enforced during inference and validation
   - `Any` is no longer a source-language or HM/runtime escape hatch

2. **Downstream representation cleanup is no longer part of this roadmap**
   - the remaining architectural work moved into `0157` + `0158`
   - that work is about semantic-vs-runtime-representation separation, not source-level gradual typing

Downstream execution of that representation cleanup is now tracked by
[0158_core_semantic_types_and_backend_rep_split_execution.md](/Users/s.gerokostas/Downloads/Github/flux/docs/proposals/0158_core_semantic_types_and_backend_rep_split_execution.md:1),
with [0157_explicit_core_types_and_runtime_representation_split.md](/Users/s.gerokostas/Downloads/Github/flux/docs/proposals/0157_explicit_core_types_and_runtime_representation_split.md:1)
as the architectural rationale.

3. **Proposal state is stale**
   - the code has moved further than the old text
   - the roadmap must distinguish completed static-typing work from future backend representation tightening

### Corrected critical path

The old static-typing critical path is no longer accurate. The corrected status is now:

```text
complete HM/module/runtime `Any` removal
  -> close static typing in the maintained front-end pipeline
  -> treat downstream representation cleanup as `0157` + `0158`
```

The central blocker is no longer broad type-class infrastructure, and it is no longer live `Any` cleanup. The remaining downstream work is tracked separately from this completed static-typing roadmap.

## Current State
[current-state]: #current-state

### Already implemented

These items were previously on the static-typing critical path but are already done in the repo:

- **0149 operator desugaring** — implemented
  - [0149_operator_desugaring.md](/Users/s.gerokostas/Downloads/Github/flux/docs/proposals/implemented/0149_operator_desugaring.md:1)
- **0150 HKT instance resolution** — complete
  - [0150_hkt_instance_resolution.md](/Users/s.gerokostas/Downloads/Github/flux/docs/proposals/0150_hkt_instance_resolution.md:1)
- **0074 base HM signature tightening** — implemented
  - [0074_base_signature_tightening.md](/Users/s.gerokostas/Downloads/Github/flux/docs/proposals/implemented/0074_base_signature_tightening.md:1)
- **typed Core/Aether/native pipeline work** — largely implemented
  - [0123_full_static_typing.md](/Users/s.gerokostas/Downloads/Github/flux/docs/proposals/implemented/0123_full_static_typing.md:1)

### Still missing

Within the scope of this roadmap, nothing remains open.

The major follow-on work is now outside this proposal:

- `0155` for Core validation and `core_lint`
- `0158` for the downstream semantic-`Dynamic` cleanup that has now been implemented
- `0159` for inference-completeness follow-on work around checked signatures
- `0160` for the final static-typing hardening closure and scheme-surface
  normalization criteria

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

Compiler contributors should think of this proposal as the plan to make static typing **true by construction**, not merely **enforced at the edges**.

After this roadmap is complete:

- strict typing should no longer be “HM inference plus cleanup”
- `Any` should not silently mask unresolved typing inside maintained compiler paths
- the stdlib should type-check under the same strict rules as user code
- Core/Aether/backend work should rely on a truly static upstream contract
- backend `Dynamic` should be understood as representation, not as a source-language type escape hatch

The key implementation rule that closed this roadmap was:

> remove `Any` from semantic fallback paths before claiming static typing is complete.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Phase 0 — Baseline alignment

This phase does not add features. It updates the static-typing roadmap to reflect reality.

#### Goals

- treat the following as complete prerequisites:
  - 0149 operator desugaring
  - 0150 HKT instance resolution
  - 0074 base HM signatures
- stop treating those items as blockers for static typing completion
- re-center the roadmap on:
  - 0147 completion
  - `Any` elimination
  - strict-mode semantics

#### Deliverables

- proposal updates only
- no compiler behavior change

#### Exit criteria

- the proposal corpus no longer lists already-implemented items as blockers for static typing completion
- future work references this roadmap instead of the stale critical path

---

### Phase 1 — Complete source-level class semantics

The remaining type-class work is no longer broad infrastructure. It is the last source-language gap that still affects strict typing and class-soundness at the source level.

This phase is complete.

Implemented baseline:

- constrained generic parameter syntax
- explicit-bound constraint emission
- generalized constrained schemes
- contextual instance dictionary threading
- contextual instance lowering through Core and native maintained paths

#### Scope

1. **Confirm and preserve the already-landed constrained-type-param path**
   - `fn f<a: Eq>(x: a, y: a) -> Bool`
   - parsed generic constraints
   - emitted explicit-bound class constraints
   - generalized scheme constraints
   - call-site scheme constraint re-emission

2. **Instance context enforcement**
   - `instance Eq<a> => Eq<List<a>>`
   - instance method bodies must see and use the context dictionaries implied by the instance head

3. **Close the gap between “context parses” and “context is semantically enforced”**
   - contextual instance methods must not rely on panic stubs for operations that the instance context should justify
   - dictionary threading for contextual instance methods must be end-to-end

#### Current evidence

Already implemented:

- constrained generic parameter parsing:
  - [statement.rs](/Users/s.gerokostas/Downloads/Github/flux/src/syntax/parser/statement.rs:1721)
- `FunctionTypeParam.constraints` in the AST:
  - [statement.rs](/Users/s.gerokostas/Downloads/Github/flux/src/syntax/statement.rs:21)
- explicit-bound constraint emission during function inference:
  - [function.rs](/Users/s.gerokostas/Downloads/Github/flux/src/ast/type_infer/function.rs:88)
- generalized scheme constraints:
  - [function.rs](/Users/s.gerokostas/Downloads/Github/flux/src/ast/type_infer/function.rs:286)
  - [scheme.rs](/Users/s.gerokostas/Downloads/Github/flux/src/types/scheme.rs:193)
- scheme constraint re-emission at use sites:
  - [mod.rs](/Users/s.gerokostas/Downloads/Github/flux/src/ast/type_infer/mod.rs:306)
- parser and integration tests for constrained functions:
  - [parser_test.rs](/Users/s.gerokostas/Downloads/Github/flux/src/syntax/parser/parser_test.rs:669)
  - [constrained_type_params_integration.rs](/Users/s.gerokostas/Downloads/Github/flux/tests/constrained_type_params_integration.rs:32)

Phase-1-complete evidence:

- contextual instance runtime/Core tests:
  - [ir_pipeline_tests.rs](/Users/s.gerokostas/Downloads/Github/flux/tests/ir_pipeline_tests.rs:143)
- strict typing contextual instance tests:
  - [static_type_validation_tests.rs](/Users/s.gerokostas/Downloads/Github/flux/tests/static_type_validation_tests.rs:1)
- native lowering contextual instance test:
  - [llvm_type_class.rs](/Users/s.gerokostas/Downloads/Github/flux/tests/llvm_type_class.rs:200)

#### Files

- [0147_constrained_type_params_and_instance_contexts.md](/Users/s.gerokostas/Downloads/Github/flux/docs/proposals/implemented/0147_constrained_type_params_and_instance_contexts.md:1)
- likely implementation in:
  - `src/syntax/parser/statement.rs`
  - `src/syntax/type_class.rs`
  - `src/ast/type_infer/*`
  - `src/core/passes/dict_elaborate.rs`
  - `src/types/class_env.rs`
  - `src/types/class_solver.rs`

#### Success condition

- constrained polymorphic functions remain expressible and enforced
- instance contexts are semantic, not decorative
- no class-method call in a valid constrained instance context falls back to an untyped/panic path in maintained paths
- contextual instances pass the required dictionaries through generated instance methods and call sites

#### Non-goals

- new type-class features beyond existing roadmap scope
- redesign of dictionary elaboration
- generalized overhaul of class dispatch naming or mangling

#### Verification used to close Phase 1

- `cargo test --test constrained_type_params_integration --features llvm`
- `cargo test --test ir_pipeline_tests --features llvm`
- `cargo test --test static_type_validation_tests --features llvm`
- focused contextual/native tests in `llvm_type_class`

---

### Phase 2 — Remove `Any` from HM escape hatches

This is the main static-typing phase.

This phase is complete.

#### Implemented work

1. **Strict-mode HM plumbing**
   - maintained HM fallback sites were tightened so `Any` stopped acting as the maintained-path recovery model
   - later cleanup removed the separate `--strict-types` switch and made unresolved-residue validation default

2. **Heterogeneous collection handling**
   - strict-mode heterogeneous array inference now emits a diagnostic instead of degrading to `Array<Any>`

3. **Branch-join behavior**
   - strict-mode HM no longer uses `Any` as the recovery sink for branch/result joins in maintained paths
   - branch conflicts now keep diagnostics and recover with a fresh inference variable when needed

4. **Member/index access**
   - unsupported strict-mode member/index/tuple-field access now emits typed diagnostics instead of silently inferring `Any`

5. **Effect nodes**
   - unresolved or unsupported `perform`/`handle` inference now emits strict-mode diagnostics instead of degrading to `Any`

6. **ADT decomposition and pattern typing**
   - unresolved constructor calls and constructor-pattern arity mismatches now emit strict-mode diagnostics instead of binding through `Any`

#### Notes

- Phase 2 closed the maintained HM fallback sites that previously depended on `Any`.
- Later cleanup removed the remaining unifier/runtime/source-level `Any` compatibility paths instead of keeping them as a legacy escape hatch.

#### Success condition

- maintained HM inference no longer depends on `Any` at the fallback sites covered by this phase
- unsupported maintained HM paths no longer degrade to `Any`
- targeted regressions cover the prior HM `Any` sinks

#### Non-goals

- removal of `Any` from all legacy or compatibility-oriented code in one step
- advanced new inference features unrelated to `Any` elimination

---

### Phase 3 — Make strict typing part of inference semantics

Today, strict typing is primarily a post-pass:

- [static_type_validation.rs](/Users/s.gerokostas/Downloads/Github/flux/src/ast/type_infer/static_type_validation.rs:1)

It checks top-level binding schemes after HM completes. That is useful, but too shallow.

This phase is complete.

#### Implemented work

1. **Inference-mode strictness**
   - expression-level unresolved-residue validation is now part of the default static-typing contract
   - maintained paths no longer depend on a separate `--strict-types` CLI mode to reject leftover fallback residue

2. **Expression-level validation**
   - `validate_static_types` now walks expression trees as well as binding schemes
   - it reports nested unresolved residue at the smallest surviving expression site

3. **Better error provenance**
   - maintained typing diagnostics report the failing subexpression directly on supported paths
   - focused regressions cover nested failures in ordinary expressions and handler arms

4. **Boundary-sensitive enforcement**
   - expression diagnostics are exercised through normal function bodies and handler-arm boundaries
   - post-validation still checks top-level binding schemes and nested expressions for residual unresolved residue

#### Success condition

- static typing means “inference is non-gradual here”, not “we checked for leftover `Any` only behind an opt-in flag”
- maintained failures report the real expression site rather than only the enclosing binding

#### Non-goals

- changing language semantics outside strict paths during the first landing
- redesigning HM inference wholesale

#### Verification used to close Phase 3

- `cargo test --test static_type_validation_tests`
- `cargo test --features llvm --test static_type_validation_tests`
- `cargo test --test type_inference_tests`

---

### Phase 4 — Remove stdlib and module-boundary exclusions

This phase is complete.

#### Implemented work

1. **Removed unconditional Flow stdlib carve-outs**
   - serial module compilation, parallel VM/native module compilation, and test-mode module compilation now honor the requested strictness settings for Flow stdlib modules instead of forcibly disabling them

2. **Made strict typing part of module/interface cache identity**
   - module interface semantic hashes include strictness
   - module bytecode/native cache strict hashes include strictness
   - strict builds no longer reuse non-strict module/interface artifacts as if they were equivalent

3. **Added regression coverage for enforced stdlib strictness**
   - strict CLI runs against Flow stdlib test fixtures now surface real stdlib diagnostics instead of silently compiling through the old carve-out
   - interface validation tests reject stale `.flxi` files when semantic strictness changes

4. **Preserved remaining work as explicit diagnostics**
   - several `lib/Flow` modules still fail under strict mode today
   - those failures are now visible and actionable rather than being hidden behind driver policy
   - fixing the library/API/runtime causes of those diagnostics is future cleanup, not a reason to keep the carve-out

#### Related areas

- proposal 0039 (module contracts / partial)
- module interface and cache/type boundary code

#### Success condition

- no unconditional stdlib strict-mode carve-out remains
- strict typing remains meaningful across imports and cached module boundaries

#### Non-goals

- broad stdlib API redesign
- package-manager or edition-policy changes

#### Verification used to close Phase 4

- `cargo test --lib bytecode::compiler::module_interface`
- `cargo test --test test_runner_cli test_mode_flow_list_module_fixture_reports_strict_stdlib_diagnostics -- --nocapture`
- `cargo test --test test_runner_cli test_mode_flow_array_module_fixture_reports_strict_stdlib_diagnostics -- --nocapture`
- `cargo check --lib`

---

### Phase 5 — Fix runtime type lowering and structural gaps

This phase is complete.

#### Implemented work

1. **Added checked runtime lowering**
   - `TypeEnv::try_to_runtime` now distinguishes:
     - unresolved type variables
     - open function effect rows
     - unsupported nominal/HKT shapes
   - representable runtime types are preserved instead of being flattened immediately

2. **Preserved concrete function boundary types**
   - closed HM function types now lower to `RuntimeType::Function`
   - this keeps runtime-facing type information for higher-order values that are actually representable

3. **Stopped silent fallback in maintained lowering consumers**
   - inferred binding/static-type plumbing now only binds runtime types when lowering succeeds
   - diagnostics that previously printed `Any` for known-but-unsupported HM shapes now fall back to the HM type display instead of masking the real shape

4. **Documented unsupported runtime-boundary shapes explicitly**
   - open effect rows, unresolved HKT parameters, and unsupported nominal types now have a first-class lowering outcome instead of only collapsing through a gradual fallback
   - the checked path is now the canonical runtime-boundary translation surface

#### Success condition

- runtime type lowering no longer reintroduces “hidden graduality” after HM has succeeded

#### Non-goals

- feature-complete structural typing
- solving every future record/module contract feature in this proposal

#### Verification used to close Phase 5

- `cargo test --lib types::type_env`
- `cargo test --test type_inference_tests`
- `cargo test --lib bytecode::compiler::compiler_test`
- `cargo check --lib`

## Historical gap inventory
[detailed-gap-inventory]: #detailed-gap-inventory

### Gap A — downstream semantic `Dynamic` cleanup

This was the major downstream caveat at the time this roadmap was closed.

It is now historical:

- `0157` explained the semantic-vs-representation split
- `0158` executed the maintained-path cleanup

### Gap B — unsupported structural/runtime shapes are still future feature work

- typed records / named structural field access
- richer nominal ADT runtime contracts
- fuller module contract coverage for currently unsupported boundary forms

These are no longer hidden graduality inside the maintained runtime-lowering path, but they remain unsupported features outside the scope of this roadmap.

### Gap C — docs and proposal state were behind implementation reality

- `0123` required Phase 0 correction and now points remaining work here
- older roadmap text described live `Any` fallback zones that have since been removed

The roadmap must align the proposal corpus with the actual compiler.

## Recommended implementation order
[recommended-implementation-order]: #recommended-implementation-order

The implementation order recorded here is now historical.

Recommended order:

1. Phase 2 removed HM `Any` fallback sites.
2. Phase 3 made strictness semantic during inference and validation.
3. Phases 4 and 5 removed stdlib/module carve-outs and runtime-lowering fallback behavior.

Future work should now be tracked separately from this completed static-typing roadmap.

## Testing plan
[testing-plan]: #testing-plan

### Phase 1

- constrained function syntax tests
- instance context enforcement tests
- class method resolution tests in constrained instance bodies

### Phase 2

- strict-mode regression tests for prior HM fallback sites
- focused HM tests for:
  - heterogeneous arrays
  - unresolved member/index access
  - unsupported effect signatures
  - constructor-pattern arity mismatches

Verification used to close Phase 2:

- `cargo test --test static_type_validation_tests`
- `cargo test --features llvm --test static_type_validation_tests`
- `cargo test --test type_inference_tests`
- `cargo test --lib`

### Phase 3

- subexpression-level strict diagnostics
- negative tests ensuring strict mode errors are emitted at the true failing expression

### Phase 4

- strict stdlib compilation tests
- cross-module strict typing tests
- cache/interface strict typing tests

### Phase 5

- runtime type translation tests
- strict-boundary translation tests
- typed record / structural-access regressions where relevant

## Drawbacks
[drawbacks]: #drawbacks

- Tightening the source/HM/runtime contract exposes latent stdlib and test-suite issues that were previously masked.
- The proposal boundary is now narrower than a blanket “every IR node is fully concrete” claim.
- Follow-on work now lives in separate proposals instead of remaining implicit here.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Why not declare static typing “done”

Within the maintained source/HM/runtime semantic pipeline, it is done. The
historical caution here was about not confusing that closure with every possible
downstream representation refinement.

### Why not keep `Any` and just harden validation

Because a post-check cannot fully replace a non-gradual inference engine. The completed work removed `Any` from normal source/HM/runtime behavior instead of relying on validation alone.

### Why not solve only backend typing now

Because typed backends are downstream consumers. The highest-value prerequisite was removing hidden graduality from the source/HM/runtime path first.

## Post-completion `Any` policy
[post-completion-any-policy]: #post-completion-any-policy

With this roadmap complete in maintained paths, Flux should treat `Any` as removed from intended normal user-visible language semantics.

That means:

- guide-level docs, examples, and user-facing typing explanations should stop teaching `Any` as a normal escape hatch
- reintroduction of `Any` as a normal source-language feature would be a regression against this proposal
- downstream representation cleanup should use `Dynamic` or other backend-specific terminology instead of reviving `Any`

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- Should strict typing become the default immediately, or after a compatibility cycle?
- Which structural typing features should be considered mandatory for future language work versus explicitly out of scope for static-typing completion?

## Future possibilities
[future-possibilities]: #future-possibilities

- `0158` already covered the maintained-path semantic-`Dynamic` cleanup.
- A later proposal can further reduce or specialize runtime representation where that materially improves optimization or code generation.
- A later proposal can narrow the language’s compatibility story around strict mode once the project wants to change defaults.
