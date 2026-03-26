# Flux Roadmap to 1.0.0

This is the **full** roadmap to `1.0.0`.

Unlike the narrower earlier draft, this roadmap includes:

- proposals with status `Not Implemented`
- proposals with status `Draft`
- proposals with status `Partially Implemented` when they are still relevant release work

It does **not** treat every proposal as a `1.0.0` blocker. The roadmap separates:

- **Core blockers**: work that should land before `1.0.0`
- **Optional / deferable**: valuable work that should not automatically delay `1.0.0`

## 1.0.0 definition

`1.0.0` should mean:

- a stable pure functional language core
- stable HM typing and algebraic effect semantics
- a coherent standard library and package/module workflow
- actor-based concurrency MVP
- a coherent Aether runtime direction
- trustworthy VM/JIT parity
- usable tests, diagnostics, and debugging workflow

It does **not** require:

- macros
- M:N scheduling
- deterministic replay
- NaN-boxing
- every advanced ergonomics feature

## Release roadmap

| Version | Theme | Core blockers | Optional / stretch |
|---|---|---|---|
| `0.0.5` | Syntax, diagnostics, type/effect closure | `0027`, `0032`, `0057`, `0058`, `0059`, `0060`, `0061`, `0063` | `0037` |
| `0.0.6` | Stdlib and package workflow | `0030`, `0015`, `0003`, `0029` | `0011` |
| `0.0.7` | Tests, linter, language identity | `0035`, `0010`, `0025`, `0043` | `0024` |
| `0.0.8` | Compiler/runtime architecture base | `0044`, `0085`, `0086` | `0023` |
| `0.0.9` | Aether foundation | `0084`, `0070`, `0067` | — |
| `0.1.0` | First coherent preview | Consolidate `0.0.5` to `0.0.9` | — |
| `0.2.0` | Actor concurrency MVP | `0026` (actor-first interpretation), `0065`, `0066` | — |
| `0.3.0` | Reuse and ownership | `0068`, `0069` | `0077` |
| `0.4.0` | Handler runtime maturity | `0072`, `0073` | — |
| `0.5.0` | Records and capability-oriented effects | `0048`, `0075` | — |
| `0.6.0` | Tooling and developer workflow | `0076`, `0083` | `0024`, `0023` if still incomplete |
| `0.7.0` | Standard library and language polish | finish remaining `0003`/`0030` polish, docs/examples alignment, `0052` | `0082` |
| `0.8.0` | Pre-RC stabilization | `0062`, VM/JIT parity sweep, performance and diagnostic freeze | `0041` |
| `0.9.0` | Release candidate line | compatibility policy, docs freeze, final parity hardening | `0071`, `0038` |
| `0.9.5` | Conditional FP abstraction layer | `0053` if the design remains minimal and low-risk | — |
| `1.0.0` | Stable Flux | final stabilization and release promise | macros remain deferred |

## Release notes by version

### `0.0.5` — Syntax, diagnostics, type/effect closure

Core blockers:

- [0027_language_syntax_specification.md](../proposals/0027_language_syntax_specification.md)
- [0032_type_system_with_effects.md](../proposals/0032_type_system_with_effects.md)
- [0057_parser_diagnostics_with_inferred_types.md](../proposals/0057_parser_diagnostics_with_inferred_types.md)
- [0058_contextual_diagnostics_callsite_let_return.md](../proposals/0058_contextual_diagnostics_callsite_let_return.md)
- [0059_parser_error_experience.md](../proposals/0059_parser_error_experience.md)
- [0060_parser_diagnostics_hm_typechecker_hardening.md](../proposals/0060_parser_diagnostics_hm_typechecker_hardening.md)
- [0061_stage_aware_diagnostic_pipeline.md](../proposals/0061_stage_aware_diagnostic_pipeline.md)
- [0063_true_fp_completion_program.md](../proposals/0063_true_fp_completion_program.md)

Optional:

- [0037_grammar_improvements.md](../proposals/0037_grammar_improvements.md)

Exit criteria:

- core syntax is no longer moving casually
- effect/type semantics are clear and test-backed
- parser/type/effect diagnostics feel deliberate rather than transitional

### `0.0.6` — Stdlib and package workflow

Core blockers:

- [0030_flow.md](../proposals/0030_flow.md)
- [0015_package_module_workflow_mvp.md](../proposals/0015_package_module_workflow_mvp.md)
- [0003_stdlib_proposal.md](../proposals/0003_stdlib_proposal.md)
- [0029_base_and_flow.md](../proposals/0029_base_and_flow.md)

Optional:

- [0011_phase2_module_system_enhancements.md](../proposals/0011_phase2_module_system_enhancements.md)

Exit criteria:

- `Base` and `Flow` become a coherent public standard library story
- users can structure projects with a real module/package workflow

### `0.0.7` — Tests, linter, language identity

Core blockers:

- [0035_unit_test_framework.md](../proposals/0035_unit_test_framework.md)
- [0010_advanced_linter.md](../proposals/0010_advanced_linter.md)
- [0025_pure_fp_language_vision.md](../proposals/0025_pure_fp_language_vision.md)
- [0043_pure_flux_checklist.md](../proposals/0043_pure_flux_checklist.md)

Optional:

- [0024_runtime_instrumentation_and_value_tracer.md](../proposals/0024_runtime_instrumentation_and_value_tracer.md)

Exit criteria:

- Flux has a usable self-hosted testing workflow
- linting starts enforcing the language’s intended style
- the project’s identity is explicit: pure FP with effects

### `0.0.8` — Compiler/runtime architecture base

Core blockers:

- [0044_compiler_phase_pipeline_refactor.md](../proposals/0044_compiler_phase_pipeline_refactor.md)
- [0085_primop_base_flow_boundary.md](../proposals/0085_primop_base_flow_boundary.md)
- [0086_backend_neutral_core_ir.md](../proposals/0086_backend_neutral_core_ir.md)

Optional:

- [0023_bytecode_decode_passes.md](../proposals/0023_bytecode_decode_passes.md)

Exit criteria:

- compiler phases are more explicit
- PrimOp/Base/Flow responsibilities are clearer
- Flux has a real shared lowering story before actors and Aether become more complex

### `0.0.9` — Aether foundation

Core blockers:

- [0084_aether_memory_model.md](../proposals/0084_aether_memory_model.md)
- [0070_perceus_gc_heap_replacement.md](../proposals/0070_perceus_gc_heap_replacement.md)
- [0067_gchandle_actor_boundary_error.md](../proposals/0067_gchandle_actor_boundary_error.md)

Exit criteria:

- Aether is the official runtime direction
- `GcHandle`-style architecture is no longer the long-term model
- actor/message boundary safety is defined before actors ship

### `0.1.0` — First coherent preview

This milestone means:

- core syntax and effect semantics are credible
- stdlib and module workflow are credible
- tests/linting exist
- compiler/runtime architecture direction is credible
- Aether direction is explicit

### `0.2.0` — Actor concurrency MVP

Core blockers:

- [0026_concurrency_model.md](../proposals/0026_concurrency_model.md)
- [0065_actor_effect_stdlib.md](../proposals/0065_actor_effect_stdlib.md)
- [0066_thread_per_actor_handler.md](../proposals/0066_thread_per_actor_handler.md)

Important interpretation:

- `0026` should be implemented **actor-first**
- general `async/await` should not be a `1.0.0` blocker

Exit criteria:

- Flux has a real concurrency story
- actor operations are effect-aware and work across VM/JIT

### `0.3.0` — Reuse and ownership

Core blockers:

- [0068_perceus_uniqueness_analysis.md](../proposals/0068_perceus_uniqueness_analysis.md)
- [0069_rcget_mut_fast_path.md](../proposals/0069_rcget_mut_fast_path.md)

Optional:

- [0077_type_informed_optimization.md](../proposals/0077_type_informed_optimization.md)
- [0114_aether_perceus_completion_plan.md](../proposals/0114_aether_perceus_completion_plan.md)

Exit criteria:

- Aether starts paying off in execution, not just design docs
- reuse legality is compiler-guided and testable

### `0.4.0` — Handler runtime maturity

Core blockers:

- [0072_evidence_passing_handlers.md](../proposals/0072_evidence_passing_handlers.md)
- [0073_state_reader_continuation_elim.md](../proposals/0073_state_reader_continuation_elim.md)

Exit criteria:

- effects/handlers are not merely surface syntax
- handler runtime strategy is mature enough for long-term confidence

### `0.5.0` — Records and capability-oriented effects

Core blockers:

- [0048_typed_record_types.md](../proposals/0048_typed_record_types.md)
- [0075_effect_sealing.md](../proposals/0075_effect_sealing.md)

Exit criteria:

- records fill a major missing data-model feature
- Flux’s effect system gains a serious capability/security dimension

### `0.6.0` — Tooling and developer workflow

Core blockers:

- [0076_debug_toolkit.md](../proposals/0076_debug_toolkit.md)
- [0083_typed_holes.md](../proposals/0083_typed_holes.md)

Optional if still unfinished:

- [0024_runtime_instrumentation_and_value_tracer.md](../proposals/0024_runtime_instrumentation_and_value_tracer.md)
- [0023_bytecode_decode_passes.md](../proposals/0023_bytecode_decode_passes.md)

Exit criteria:

- users can debug Flux programs and Flux compilation stages without immediately dropping
  into Rust internals

### `0.7.0` — Standard library and language polish

Core blockers:

- finish any remaining stdlib polish still open under [0003_stdlib_proposal.md](../proposals/0003_stdlib_proposal.md) and [0030_flow.md](../proposals/0030_flow.md)

Optional:

- [0082_effect_directed_pipelines.md](../proposals/0082_effect_directed_pipelines.md)
- [0052_auto_currying_and_partial_application.md](../proposals/0052_auto_currying_and_partial_application.md)

Exit criteria:

- stdlib/docs/examples reflect the actual intended Flux style
- optional ergonomics can land here if stable enough

Revision:

- [0052_auto_currying_and_partial_application.md](../proposals/0052_auto_currying_and_partial_application.md)
  is now considered strategically important for Flux's pure functional style and should be
  treated as a likely pre-`1.0.0` feature, provided the design stays predictable and
  minimal.

### `0.8.0` — Pre-RC stabilization

Core blockers:

- [0062_performance_stabilization_program.md](../proposals/0062_performance_stabilization_program.md)
- VM/JIT parity sweep
- performance and diagnostics freeze

Optional:

- [0041_nan_boxing_runtime_optimization.md](../proposals/0041_nan_boxing_runtime_optimization.md)

Exit criteria:

- Flux is being hardened for release rather than widened
- performance and behavior are predictable enough for release candidates

### `0.9.0` — Release candidate line

Core blockers:

- compatibility policy
- docs freeze
- examples freeze
- final backend parity and release hardening

Optional:

- [0071_mn_scheduler_actor_handler.md](../proposals/0071_mn_scheduler_actor_handler.md)
- [0038_deterministic_effect_replay.md](../proposals/0038_deterministic_effect_replay.md)

Exit criteria:

- only stabilization work remains
- optional ambitious features either prove themselves or move out of the release path

### `0.9.5` — Conditional FP abstraction layer

This is a conditional milestone, only if the design remains minimal and does not destabilize
the path to `1.0.0`.

Candidate:

- [0053_traits_and_typeclasses.md](../proposals/0053_traits_and_typeclasses.md)

Exit criteria if shipped:

- the typeclass system remains small, teachable, and unsurprising
- it does not force large redesigns in typing, diagnostics, or standard library policy
- it supports Flux's pure FP direction without bloating the language surface

### `1.0.0` — Stable Flux

`1.0.0` means:

- stable pure FP identity
- stable type/effect core
- stable Base/Flow/package workflow
- stable actor concurrency MVP
- stable Aether direction
- strong diagnostics, testing, and debugging workflow
- trustworthy VM/JIT parity

## Recommended non-blockers for `1.0.0`

These should be treated as deferable unless they become unexpectedly easy and low-risk:

- [0009_macro_system.md](../proposals/0009_macro_system.md)
- [0040_macro_system.md](../proposals/0040_macro_system.md)
- [0041_nan_boxing_runtime_optimization.md](../proposals/0041_nan_boxing_runtime_optimization.md)
- [0071_mn_scheduler_actor_handler.md](../proposals/0071_mn_scheduler_actor_handler.md)
- [0038_deterministic_effect_replay.md](../proposals/0038_deterministic_effect_replay.md)

## Biggest risks to the roadmap

The most likely ways to delay `1.0.0` are:

1. treating `0026` as “must ship full async/await” instead of actor-first
2. delaying Core IR / architecture work until after actors and Aether are already complex
3. trying to make macros and advanced scheduler work mandatory before release
4. shipping Aether-related implementation tracks without first locking the Aether model
5. shipping an overly complex typeclass design that fights Flux's minimal syntax goal

## Recommended project priority order

If schedule pressure appears, protect this order:

1. syntax/type/effect stability
2. stdlib/package workflow
3. compiler/runtime architecture cleanup
4. Aether foundation
5. actor concurrency MVP
6. reuse optimization
7. handler runtime maturity
8. tooling and stabilization

Everything else is negotiable.
