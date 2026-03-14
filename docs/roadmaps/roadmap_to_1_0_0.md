# Flux Roadmap to 1.0.0

This is the **full** roadmap to `1.0.0`.

Unlike the narrower earlier draft, this roadmap includes:

- proposals with status `Not Implemented`
- proposals with status `Draft`
- proposals with status `Partially Implemented` when they are still relevant release work

It does **not** treat every proposal as a `1.0.0` blocker. The roadmap separates:

- **Core blockers**: work that should land before `1.0.0`
- **Optional / deferable**: valuable work that should not automatically delay `1.0.0`

## Design philosophy

**Language features before runtime infrastructure.** Users will forgive a slow GC; they won't
forgive the inability to write `sort(myList)` generically. Typeclasses, records, and FFI are
load-bearing for everything that follows — stdlib, actors, handlers, tooling — and must land
early enough that later work builds on them rather than around them.

## 1.0.0 definition

`1.0.0` should mean:

- a stable pure functional language core with bounded polymorphism (typeclasses)
- stable HM typing and algebraic effect semantics
- records and named fields as a first-class data model
- a coherent standard library and package/module workflow
- actor-based concurrency MVP
- FFI for calling into native code
- trustworthy VM/JIT parity
- usable tests, diagnostics, and debugging workflow

It does **not** require:

- macros
- M:N scheduling
- deterministic replay
- NaN-boxing
- Perceus/Aether GC replacement (design direction should be explicit, implementation is post-1.0)
- every advanced ergonomics feature

## Release roadmap

| Version | Theme | Core blockers | Optional / stretch |
|---|---|---|---|
| `0.0.5` | Syntax freeze, diagnostics, type/effect closure | `0027`, `0032`, `0057`, `0058`, `0059`, `0060`, `0061`, `0063` | `0037` |
| `0.0.6` | Typeclasses and records | `0053`, `0048` | — |
| `0.0.7` | Stdlib, package workflow, tests | `0030`, `0015`, `0003`, `0029`, `0035`, `0010` | `0011` |
| `0.0.8` | Compiler architecture and Core IR | `0044`, `0085`, `0086`, `0025`, `0043` | `0023` |
| `0.1.0` | First coherent preview | Consolidate `0.0.5` to `0.0.8` | — |
| `0.2.0` | Actor concurrency MVP | `0026` (actor-first), `0065`, `0066`, `0067` | — |
| `0.3.0` | Handler runtime maturity | `0072`, `0073`, `0075` | `0077` |
| `0.4.0` | FFI and interop | FFI proposal (new), `0052` | `0082` |
| `0.5.0` | Tooling and developer workflow | `0076`, `0083` | `0024`, `0023` if still incomplete |
| `0.6.0` | Performance and stabilization | `0062`, VM/JIT parity sweep, diagnostics freeze | `0041` |
| `0.7.0-rc` | Release candidate | compatibility policy, docs freeze, final parity hardening | `0071`, `0038` |
| `1.0.0` | Stable Flux | final stabilization and release promise | macros remain deferred |

## Release notes by version

### `0.0.5` — Syntax freeze, diagnostics, type/effect closure

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

### `0.0.6` — Typeclasses and records

**Rationale for early placement:** Typeclasses and records are load-bearing for everything
that follows. Without typeclasses, every stdlib function that needs equality (`assert_eq`,
`contains`, `dedup`), ordering (`sort`), or display (`print`, string interpolation) must be
hardcoded or use runtime dispatch. Without records, every data type is either a positional
tuple or a verbose ADT. Both features fundamentally shape:

- stdlib API design (`0.0.7`) — generic `map`, `filter`, `sort`, `show`
- actor message types (`0.2.0`) — need both named fields and serialization constraints
- handler optimizations (`0.3.0`) — evidence passing benefits from typeclass-based dispatch
- tooling (`0.5.0`) — typed holes and auto-complete need to understand constraints

Shipping them late means redesigning everything built on top.

Core blockers:

- [0053_traits_and_typeclasses.md](../proposals/0053_traits_and_typeclasses.md)
- [0048_typed_record_types.md](../proposals/0048_typed_record_types.md)

Exit criteria:

- typeclasses support at minimum: `Eq`, `Ord`, `Show`, with `deriving` for ADTs and records
- the typeclass system is small, teachable, and unsurprising
- records support named field access, update syntax, and pattern matching
- HM inference integrates typeclass constraints without breaking existing programs

### `0.0.7` — Stdlib, package workflow, tests

With typeclasses and records now available, the standard library can be designed properly
with generic interfaces from the start.

Core blockers:

- [0030_flow.md](../proposals/0030_flow.md)
- [0015_package_module_workflow_mvp.md](../proposals/0015_package_module_workflow_mvp.md)
- [0003_stdlib_proposal.md](../proposals/0003_stdlib_proposal.md)
- [0029_base_and_flow.md](../proposals/0029_base_and_flow.md)
- [0035_unit_test_framework.md](../proposals/0035_unit_test_framework.md)
- [0010_advanced_linter.md](../proposals/0010_advanced_linter.md)

Optional:

- [0011_phase2_module_system_enhancements.md](../proposals/0011_phase2_module_system_enhancements.md)

Exit criteria:

- `Base` and `Flow` use typeclasses for generic operations (`Eq`, `Ord`, `Show`, `Functor`)
- users can structure projects with a real module/package workflow
- Flux has a usable self-hosted testing workflow
- linting enforces the language's intended style

### `0.0.8` — Compiler architecture and Core IR

Core blockers:

- [0044_compiler_phase_pipeline_refactor.md](../proposals/0044_compiler_phase_pipeline_refactor.md)
- [0085_primop_base_flow_boundary.md](../proposals/0085_primop_base_flow_boundary.md)
- [0086_backend_neutral_core_ir.md](../proposals/0086_backend_neutral_core_ir.md)
- [0025_pure_fp_language_vision.md](../proposals/0025_pure_fp_language_vision.md)
- [0043_pure_flux_checklist.md](../proposals/0043_pure_flux_checklist.md)

Optional:

- [0023_bytecode_decode_passes.md](../proposals/0023_bytecode_decode_passes.md)

Exit criteria:

- compiler phases are explicit and well-separated
- PrimOp/Base/Flow responsibilities are clear
- Core IR is the single shared lowering path for both VM and JIT
- the project's identity is explicit: pure FP with effects

### `0.1.0` — First coherent preview

This milestone means:

- core syntax and effect semantics are stable
- typeclasses and records exist and are integrated
- stdlib and module workflow are coherent
- compiler/runtime architecture is clean
- tests and linting exist

This is the first version worth showing to external users.

### `0.2.0` — Actor concurrency MVP

Core blockers:

- [0026_concurrency_model.md](../proposals/0026_concurrency_model.md)
- [0065_actor_effect_stdlib.md](../proposals/0065_actor_effect_stdlib.md)
- [0066_thread_per_actor_handler.md](../proposals/0066_thread_per_actor_handler.md)
- [0067_gchandle_actor_boundary_error.md](../proposals/0067_gchandle_actor_boundary_error.md)

Important interpretation:

- `0026` should be implemented **actor-first**
- general `async/await` should not be a `1.0.0` blocker
- actor message types should use records and typeclass constraints (e.g. `Message` typeclass)

Exit criteria:

- Flux has a real concurrency story
- actor operations are effect-aware and work across VM/JIT
- actor/message boundary safety is defined and enforced

### `0.3.0` — Handler runtime maturity

Core blockers:

- [0072_evidence_passing_handlers.md](../proposals/0072_evidence_passing_handlers.md)
- [0073_state_reader_continuation_elim.md](../proposals/0073_state_reader_continuation_elim.md)
- [0075_effect_sealing.md](../proposals/0075_effect_sealing.md)

Optional:

- [0077_type_informed_optimization.md](../proposals/0077_type_informed_optimization.md)

Exit criteria:

- effects/handlers are not merely surface syntax — runtime strategy is mature
- evidence-passing translation replaces continuation capture for common patterns
- Flux's effect system gains a serious capability/security dimension via sealing
- handler runtime strategy is mature enough for long-term confidence

### `0.4.0` — FFI and interop

**Rationale:** Without FFI, every I/O operation must be a built-in primop. A minimal C FFI
(or Rust FFI via `extern` declarations) makes Flux practically useful for real programs:
database drivers, HTTP clients, GUI bindings, system calls.

Core blockers:

- FFI proposal (to be written) — at minimum: declare external functions with type signatures,
  marshal between Flux values and C types, link shared libraries at runtime
- [0052_auto_currying_and_partial_application.md](../proposals/0052_auto_currying_and_partial_application.md)

Optional:

- [0082_effect_directed_pipelines.md](../proposals/0082_effect_directed_pipelines.md)

Exit criteria:

- Flux programs can call external C functions with type-safe declarations
- auto-currying and partial application support the pure FP style
- the FFI is effect-aware (foreign calls require appropriate effect annotations)

### `0.5.0` — Tooling and developer workflow

Core blockers:

- [0076_debug_toolkit.md](../proposals/0076_debug_toolkit.md)
- [0083_typed_holes.md](../proposals/0083_typed_holes.md)

Optional if still unfinished:

- [0024_runtime_instrumentation_and_value_tracer.md](../proposals/0024_runtime_instrumentation_and_value_tracer.md)
- [0023_bytecode_decode_passes.md](../proposals/0023_bytecode_decode_passes.md)

Exit criteria:

- users can debug Flux programs without dropping into Rust internals
- typed holes provide interactive type-driven development

### `0.6.0` — Performance and stabilization

Core blockers:

- [0062_performance_stabilization_program.md](../proposals/0062_performance_stabilization_program.md)
- VM/JIT parity sweep
- performance and diagnostics freeze

Optional:

- [0041_nan_boxing_runtime_optimization.md](../proposals/0041_nan_boxing_runtime_optimization.md)

Exit criteria:

- Flux is being hardened for release rather than widened
- performance and behavior are predictable enough for release candidates
- VM and JIT produce identical results for all supported programs

### `0.7.0-rc` — Release candidate

Core blockers:

- compatibility policy
- docs freeze
- examples freeze
- final backend parity and release hardening
- finish any remaining stdlib polish under [0003_stdlib_proposal.md](../proposals/0003_stdlib_proposal.md) and [0030_flow.md](../proposals/0030_flow.md)

Optional:

- [0071_mn_scheduler_actor_handler.md](../proposals/0071_mn_scheduler_actor_handler.md)
- [0038_deterministic_effect_replay.md](../proposals/0038_deterministic_effect_replay.md)

Exit criteria:

- only stabilization work remains
- stdlib/docs/examples reflect the actual intended Flux style
- optional ambitious features either prove themselves or move out of the release path

### `1.0.0` — Stable Flux

`1.0.0` means:

- stable pure FP identity with typeclasses and records
- stable type/effect core
- stable Base/Flow/package workflow with generic interfaces
- stable actor concurrency MVP
- mature effect handler runtime (evidence passing)
- FFI for native interop
- strong diagnostics, testing, and debugging workflow
- trustworthy VM/JIT parity

## Recommended non-blockers for `1.0.0`

These should be treated as deferable unless they become unexpectedly easy and low-risk:

- [0009_macro_system.md](../proposals/0009_macro_system.md)
- [0040_macro_system.md](../proposals/0040_macro_system.md)
- [0041_nan_boxing_runtime_optimization.md](../proposals/0041_nan_boxing_runtime_optimization.md)
- [0071_mn_scheduler_actor_handler.md](../proposals/0071_mn_scheduler_actor_handler.md)
- [0038_deterministic_effect_replay.md](../proposals/0038_deterministic_effect_replay.md)
- [0084_aether_memory_model.md](../proposals/0084_aether_memory_model.md) — design direction should be documented, implementation is post-1.0
- [0070_perceus_gc_heap_replacement.md](../proposals/0070_perceus_gc_heap_replacement.md) — ambitious runtime rewrite, ship 1.0 with working GC first
- [0068_perceus_uniqueness_analysis.md](../proposals/0068_perceus_uniqueness_analysis.md) — depends on Perceus, defer together
- [0069_rcget_mut_fast_path.md](../proposals/0069_rcget_mut_fast_path.md) — incremental optimization, can land anytime

## Biggest risks to the roadmap

The most likely ways to delay `1.0.0` are:

1. treating `0026` as "must ship full async/await" instead of actor-first
2. delaying typeclasses until after stdlib and actors are designed without them, forcing redesign
3. trying to make macros and advanced scheduler work mandatory before release
4. blocking 1.0 on Aether/Perceus GC replacement — the current GC works and can ship
5. shipping an overly complex typeclass design that fights Flux's minimal syntax goal
6. designing records and stdlib APIs without each other, then discovering they don't fit
7. lacking FFI, forcing every I/O operation to be a built-in primop

## Recommended project priority order

If schedule pressure appears, protect this order:

1. syntax/type/effect stability
2. typeclasses and records
3. stdlib/package workflow (designed with typeclasses)
4. compiler/runtime architecture cleanup
5. actor concurrency MVP
6. handler runtime maturity (evidence passing)
7. FFI
8. tooling and stabilization

Everything else is negotiable. In particular, Aether/Perceus is valuable long-term work
but should not gate 1.0 — ship with the current GC, optimize later.
