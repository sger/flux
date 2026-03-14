- Feature Name: Index
- Start Date: 2026-02-26
- Proposal PR:
- Flux Issue:

# Proposal 0000: Index

## Summary

This proposal defines the scope and delivery model for Index in Flux. It consolidates the legacy specification into the canonical proposal template while preserving technical and diagnostic intent.

## Motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

## Guide-level explanation

This proposal should be read as a user-facing and contributor-facing guide for the feature.

- The feature goals, usage model, and expected behavior are preserved from the legacy text.
- Examples and migration expectations follow existing Flux conventions.
- Diagnostics and policy boundaries remain aligned with current proposal contracts.

## Reference-level explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- ****preamble**:** This is the canonical inventory for `docs/proposals`. Feature state is evidence-driven: - `have` = implemented and exercised in `src/` plus tests/examples - `partial` = implemen...
- **Active Track (Implemented / In Progress):** | ID | Proposal | Status | Feature State | Evidence | Action | |---|---|---|---|---|---| | 0001 | [0001_module_constants.md](implemented/0001_module_constants.md) | Implemented | have | `sr...
- **Backlog (Draft / Proposed):** | ID | Proposal | Status | Feature State | Evidence | Action | |---|---|---|---|---|---| | 0003 | [0003_stdlib_proposal.md](0003_stdlib_proposal.md) | Draft | gap | proposal-onl...
- **Superseded Historical Docs:** | ID | Proposal | Status | Canonical Successor | Action | |---|---|---|---|---| | 0009 | [0009_macro_system.md](0009_macro_system.md) | Superseded | [0040_macro_system.md](0040_...
- **Archived Docs:** `docs/proposals/archive/` exists for future archival moves. No proposal is archived in this cleanup.
- **Renamed File Map:** - `0010_gc.md` -> `0045_gc.md`

### Detailed specification (migrated legacy content)

## Proposal Index and Canonical State

This is the canonical inventory for `docs/proposals`.
Feature state is evidence-driven:

- `have` = implemented and exercised in `src/` plus tests/examples
- `partial` = implemented in part or implemented without full hardening/coverage
- `gap` = proposal-only or no convincing implementation evidence yet

> Note: Proposal corpus normalized to the `0000_template.md` section model.
> Implemented proposals now live under `docs/proposals/implemented/`.

### Active Track (Implemented / In Progress)

| ID | Proposal | Status | Feature State | Evidence | Action |
|---|---|---|---|---|---|
| 0001 | [0001_module_constants.md](implemented/0001_module_constants.md) | Implemented | have | `src/bytecode/module_constants/*`, `src/bytecode/module_constants/module_constants_test.rs` | keep as baseline |
| 0002 | [0002_error_code_registry.md](implemented/0002_error_code_registry.md) | Implemented | have | `src/diagnostics/compiler_errors.rs`, `tests/error_codes_registry_tests.rs` | keep as baseline |
| 0014 | [0014_lexer_parser_code_review.md](implemented/0014_lexer_parser_code_review.md) | Implemented | have | `src/syntax/lexer/*`, `src/syntax/parser/*`, `tests/parser_tests.rs` | keep as baseline |
| 0016 | [0016_tail_call_accumulator_optimization.md](implemented/0016_tail_call_accumulator_optimization.md) | Implemented | have | `tests/tail_call_tests.rs`, `src/main.rs` (`analyze-tail-calls`) | keep as baseline |
| 0018 | [0018_runtime_language_evolution_roadmap.md](0018_runtime_language_evolution_roadmap.md) | In Progress | partial | roadmap-only umbrella with selective landed items (tail-calls, module constants) | split into smaller executable proposals |
| 0019 | [0019_zero_copy_value_passing.md](implemented/0019_zero_copy_value_passing.md) | Implemented | partial | runtime ownership paths in `src/runtime/*`; no dedicated regression matrix tied to 019 | add explicit perf/regression evidence |
| 0021 | [0021_stack_overflow_fix_for_builtins.md](implemented/0021_stack_overflow_fix_for_builtins.md) | Implemented | have | `src/runtime/vm/mod.rs`, `tests/vm_tests.rs`, `tests/tail_call_tests.rs` | keep as baseline |
| 0022 | [0022_ast_traversal_framework.md](implemented/0022_ast_traversal_framework.md) | Implemented | have | `tests/ast_visit_smoke.rs`, `tests/ast_fold_smoke.rs`, AST traversal tests | keep as baseline |
| 0031 | [0031_cranelift_jit_backend.md](implemented/0031_cranelift_jit_backend.md) | Draft | partial | JIT compile path in CLI/tests (`--features jit`, `tests/jit_phase3_tests.rs`, `tests/jit_phase4_tests.rs`) | align proposal with actual backend scope |
| 0032 | [0032_type_system_with_effects.md](0032_type_system_with_effects.md) | Implemented | have | `src/bytecode/compiler/mod.rs`, `src/bytecode/compiler/expression.rs`, `examples/type_system/*` | keep as semantic source of truth |
| 0034 | [0034_builtin_primops.md](implemented/0034_builtin_primops.md) | In Progress | partial | `src/primop/mod.rs`, `tests/primop_*`, `examples/primop/all_primops.flx` | continue phase completion |
| 0042 | [0042_effect_rows_and_constraints.md](implemented/0042_effect_rows_and_constraints.md) | Implemented | have | effect-row fixtures `examples/type_system/30..45`, `61`, `162`, compiler effect solver paths in `src/bytecode/compiler/*`, plus closure evidence in proposal 0042 | keep as baseline closure for current scope |
| 0043 | [0043_pure_flux_checklist.md](0043_pure_flux_checklist.md) | Implemented | have | `tests/purity_vm_jit_parity_snapshots.rs`, `tests/common/purity_parity.rs`, A-G fixtures | keep as closure evidence |
| 0044 | [0044_compiler_phase_pipeline_refactor.md](0044_compiler_phase_pipeline_refactor.md) | Draft | gap | proposal-only | next architecture/perf workstream |
| 0046 | [0046_typed_ast_hm_architecture.md](implemented/0046_typed_ast_hm_architecture.md) | Draft | gap | proposal-only | post-0.0.4 HM architecture deepening |
| 0047 | [0047_adt_semantics_deepening.md](implemented/0047_adt_semantics_deepening.md) | Implemented | have | ADT semantics deepening completed; deterministic constructor typing, generic field mismatch diagnostics, module constructor boundary policy, nested ADT pass/fail consistency; all stage-1 gates passed | completed — track 2 under roadmap 054 |
| 0049 | [0049_effect_rows_completeness.md](implemented/0049_effect_rows_completeness.md) | Implemented | have | full subtraction/absence solver closure (including deferred multi-arg `Absent` evaluation), deterministic diagnostics `E419/E420/E421/E422`, fixtures `162..166` + `194..200`, and HM/compiler seam coverage in compiler/type tests | keep as baseline closure for principal row-solving scope |
| 0050 | [0050_totality_and_exhaustiveness_hardening.md](implemented/0050_totality_and_exhaustiveness_hardening.md) | Implemented | have | exhaustiveness hardening completed; deterministic compile-time totality over supported domains; guard semantics locked; all stage-1 gates passed | completed — track 3 under roadmap 054 |
| 0051 | [0051_any_fallback_reduction.md](implemented/0051_any_fallback_reduction.md) | Implemented | have | HM zero-fallback completed; disallowed Any-fallback sites tightened in typed/HM-known contexts; deterministic E300/E425 diagnostics for mismatch and unresolved boundaries | completed — track 1 under roadmap 054 |
| 0052 | [0052_auto_currying_and_partial_application.md](0052_auto_currying_and_partial_application.md) | Draft | gap | proposal-only | stage-2 post-0.0.4 currying/placeholder feature track under roadmap 054 |
| 0053 | [0053_traits_and_typeclasses.md](0053_traits_and_typeclasses.md) | Draft | gap | proposal-only | stage-2 post-0.0.4 trait/typeclass feature track under roadmap 054 |
| 0054 | [0054_0_0_4_hm_adt_exhaustiveness_critical_path.md](implemented/0054_0_0_4_hm_adt_exhaustiveness_critical_path.md) | Implemented | have | all three 0.0.4 tracks completed: track 1 (0051 HM zero-fallback), track 2 (0047 ADT semantics), track 3 (0050 exhaustiveness hardening) | roadmap closure — all stage-1 gates passed; post-0.0.4 tracks (0052, 0053, 0048, 0063) remain sequenced |
| 0055 | [0055_lexer_performance_and_architecture.md](implemented/0055_lexer_performance_and_architecture.md) | Draft | gap | proposal-only | phased lexer perf + architecture hardening with parser-contract and benchmark gates |
| 0056 | [0056_parser_performance_and_architecture.md](implemented/0056_parser_performance_and_architecture.md) | Draft | gap | proposal-only | phased parser perf + architecture hardening with lexer/parser contract and benchmark gates |
| 0057 | [0057_parser_diagnostics_with_inferred_types.md](0057_parser_diagnostics_with_inferred_types.md) | Implemented | have | `src/ast/type_infer.rs`, `src/types/unify_error.rs`, `src/diagnostics/compiler_errors.rs`, fixtures 92–105, `tests/type_inference_tests.rs`, `tests/compiler_rules_tests.rs` | completed — ReportContext, if/match contextual E300, E056 arity, fun decomposition, parser recovery, multi-error continuation |
| 0058 | [0058_contextual_diagnostics_callsite_let_return.md](0058_contextual_diagnostics_callsite_let_return.md) | Implemented | have | `src/ast/type_infer.rs`, `src/bytecode/compiler/statement.rs`, `src/types/type_env.rs`, `src/diagnostics/compiler_errors.rs`, fixtures 106–111 | completed — call-site arg dual-label, let-annotation dual-label, function return dual-label, TypeEnv span plumbing |
| 0059 | [0059_parser_error_experience.md](0059_parser_error_experience.md) | Implemented | have | `src/syntax/parser/statement.rs`, `src/syntax/parser/expression.rs`, `src/diagnostics/compiler_errors.rs`, `tests/parser_tests.rs`, `tests/parser_recovery.rs`, fixtures 112–121 | completed — keyword alias diagnostics, contextual structural parser messages, match `|`/`=>` suggestions + recovery |
| 0060 | [0060_parser_diagnostics_hm_typechecker_hardening.md](0060_parser_diagnostics_hm_typechecker_hardening.md) | Draft | gap | proposal-only | four-track hardening: parser recovery breadth (P1–P4), diagnostic precision (D1–D3), HM Any-fallback reduction (H1–H4), type checker completeness (T1–T4); fixtures 122–150, new codes E085/E086/W201 |
| 0062 | [0062_performance_stabilization_program.md](0062_performance_stabilization_program.md) | Draft | gap | proposal-only | stabilization-first perf gates and evidence consolidation across compiler throughput, runtime parity, and cache/harness determinism |
| 0063 | [0063_true_fp_completion_program.md](0063_true_fp_completion_program.md) | Draft | gap | proposal-only | true-FP feature closure program across principal effect rows, typed determinism/exhaustiveness, typed immutable records, and core FP abstractions (currying + traits) |
| 0064 | [0064_effect_row_variables.md](implemented/0064_effect_row_variables.md) | Implemented | have | explicit `EffectExpr::RowVar`, parser-enforced `|e` tails, implicit-row rejection with migration diagnostics, HM open-row function-effect unification, runtime-enforced strict public function-typed boundaries (closure/jit closure + effect subset check), and effect-row fixture migration/test coverage | keep as baseline for explicit row-tail semantics and strict runtime function contracts |

### Backlog (Draft / Proposed)

| ID | Proposal | Status | Feature State | Evidence | Action |
|---|---|---|---|---|---|
| 0003 | [0003_stdlib_proposal.md](0003_stdlib_proposal.md) | Draft | gap | proposal-only | re-scope vs current `Base`/`Flow` direction |
| 0004 | [0004_language_features_proposal.md](0004_language_features_proposal.md) | Draft | gap | proposal-only | split into smaller proposals |
| 0005 | [0005_symbol_interning.md](implemented/0005_symbol_interning.md) | Draft | gap | proposal-only | move to perf track if still desired |
| 0006 | [0006_phase1_module_split_plan.md](implemented/0006_phase1_module_split_plan.md) | Proposed | partial | module compiler is split in practice (`statement.rs`, `expression.rs`) | mark implemented parts, close leftovers |
| 0007 | [0007_visitor_pattern.md](implemented/0007_visitor_pattern.md) | Proposed | partial | largely superseded by 022 traversal framework | mark superseded by 022 |
| 0008 | [0008_builtins_module_architecture.md](implemented/0008_builtins_module_architecture.md) | Proposed | partial | base/primop separation exists (`src/base/*`, `src/primop/*`) | sync proposal to current architecture |
| 0010 | [0010_advanced_linter.md](0010_advanced_linter.md) | Draft | gap | current linter exists but not this full spec (`src/syntax/linter.rs`) | reduce scope and re-propose |
| 0011 | [0011_phase2_module_system_enhancements.md](0011_phase2_module_system_enhancements.md) | Proposed | partial | module imports/type fixtures exist in `examples/type_system/TypeSystem/*` | decompose into deliverable increments |
| 0012 | [0012_phase2_module_split_plan.md](implemented/0012_phase2_module_split_plan.md) | Proposed | partial | module compiler split partially landed | merge with 011/006 cleanup |
| 0013 | [0013_phase3_advanced_architecture.md](0013_phase3_advanced_architecture.md) | Proposed | gap | umbrella planning proposal | split into concrete proposals |
| 0015 | [0015_package_module_workflow_mvp.md](0015_package_module_workflow_mvp.md) | Proposed | partial | module roots (`--root`) exist; package workflow not fully present | narrow to package manager MVP |
| 0017 | [0017_persistent_collections_and_gc.md](implemented/0017_persistent_collections_and_gc.md) | Proposed | gap | proposal-only | supersede by targeted runtime proposals |
| 0020 | [0020_map_filter_fold_builtins.md](implemented/0020_map_filter_fold_builtins.md) | Proposed | partial | base functions and tests exist (`tests/base_functions_tests.rs`) | mark landed APIs vs pending |
| 0023 | [0023_bytecode_decode_passes.md](0023_bytecode_decode_passes.md) | Proposed | gap | proposal-only | keep as compiler tooling backlog |
| 0024 | [0024_runtime_instrumentation_and_value_tracer.md](0024_runtime_instrumentation_and_value_tracer.md) | Proposed | gap | proposal-only | keep as observability backlog |
| 0025 | [0025_pure_fp_language_vision.md](0025_pure_fp_language_vision.md) | Proposed | partial | vision reflected by 032/042/043 completion | keep as vision, not delivery plan |
| 0026 | [0026_concurrency_model.md](0026_concurrency_model.md) | Proposed | gap | proposal-only | next-phase research |
| 0027 | [0027_language_syntax_specification.md](0027_language_syntax_specification.md) | Proposed | partial | syntax implemented across `src/syntax/*`; spec not fully synchronized | convert to living spec |
| 0028 | [0028_base.md](implemented/0028_base.md) | Proposed | partial | Base APIs + docs exist (`docs/internals/base_api.md`, runtime/base tests) | mark implemented subset |
| 0030 | [0030_flow.md](0030_flow.md) | Proposed | partial | Flow module usage/examples exist (`examples/type_system/TypeSystem/*`) | mark implemented subset |
| 0033 | [0033_jit_cache_compatibility.md](implemented/0033_jit_cache_compatibility.md) | Proposed | partial | strict cache identity checks now exist in strict workstream | align with actual cache keys/tests |
| 0035 | [0035_unit_test_framework.md](0035_unit_test_framework.md) | Draft | partial | test mode and Flux tests exist (`src/main.rs --test`, `tests/test_runner_cli.rs`, `lib/Flow/FTest.flx`) | rewrite as “current + gaps” |
| 0036 | [0036_multiline_strings.md](implemented/0036_multiline_strings.md) | Draft | gap | proposal-only | keep in syntax backlog |
| 0037 | [0037_grammar_improvements.md](0037_grammar_improvements.md) | Draft | gap | proposal-only | keep in parser backlog |
| 0038 | [0038_deterministic_effect_replay.md](0038_deterministic_effect_replay.md) | Draft | gap | proposal-only | keep as advanced runtime/debugging backlog |
| 0039 | [0039_typed_module_contracts.md](implemented/0039_typed_module_contracts.md) | Draft | partial | typed/effect groundwork in 032/042; full contract boundary checks incomplete | continue after 032/043 baseline |
| 0040 | [0040_macro_system.md](0040_macro_system.md) | Draft | gap | proposal-only | canonical macro proposal going forward |
| 0041 | [0041_nan_boxing_runtime_optimization.md](0041_nan_boxing_runtime_optimization.md) | Proposed | gap | proposal-only | keep in runtime perf backlog |
| 0045 | [0045_gc.md](implemented/0045_gc.md) | Draft | gap | proposal-only (renamed from `0010_gc.md`) | continue as canonical GC proposal |
| 0048 | [0048_typed_record_types.md](0048_typed_record_types.md) | Draft | gap | proposal-only | typed immutable records with compile-time field checking and spread update |
| 0074 | [0074_base_signature_tightening.md](implemented/0074_base_signature_tightening.md) | Draft | gap | proposal-only | tighten BaseHmSignature types from Any to precise polymorphic types for compile-time error detection on builtin calls |
| 0081 | [0081_diagnostic_taxonomy.md](implemented/0081_diagnostic_taxonomy.md) | Draft | gap | proposal-only | add an explicit semantic category layer for diagnostics on top of codes, severity, phase, and error-type metadata |
| 0082 | [0082_effect_directed_pipelines.md](0082_effect_directed_pipelines.md) | Draft | gap | proposal-only | make typed effect handling compose as a first-class pipeline stage via `expr |> handle Effect { ... }` |
| 0083 | [0083_typed_holes.md](0083_typed_holes.md) | Draft | gap | proposal-only | add expression-level typed holes like `?name` so the compiler can report expected types and candidate fits while code is incomplete |

### Superseded Historical Docs

| ID | Proposal | Status | Canonical Successor | Action |
|---|---|---|---|---|
| 0009 | [0009_macro_system.md](0009_macro_system.md) | Superseded | [0040_macro_system.md](0040_macro_system.md) | keep for history with supersession banner |
| 0029 | [0029_base_and_flow.md](0029_base_and_flow.md) | Superseded | [0028_base.md](implemented/0028_base.md), [0030_flow.md](0030_flow.md) | keep for history with supersession banner |

### Archived Docs

`docs/proposals/archive/` exists for future archival moves. No proposal is archived in this cleanup.

### Renamed File Map

- `0010_gc.md` -> `0045_gc.md`

### Cleanup Report (2026-02-26)

1. Canonical status vocabulary normalized in proposal headers.
2. Required header metadata (`Status`, `Date`, `Depends on`) added to all proposals.
3. Numbering collision resolved by renaming `0010_gc.md` to `0045_gc.md`.
4. Macro track canonicalized: `040` is canonical, `009` is superseded.
5. Historical split retained: `029` superseded by `028` and `030`.
6. Cross-links updated for canonical macro references in roadmap/proposal docs.

### Migration Notes

If an old link still points to `0010_gc.md`, update it to `0045_gc.md`.
If an old link still points to macro roadmap proposal `009`, prefer `040` unless historical context is intentional.

### Historical notes

- **Status:** Implemented
- **Date:** 2026-02-26
- **Depends on:** None
- **Status:** Implemented
- **Date:** 2026-02-26
- **Depends on:** None

## Drawbacks

1. Restructuring legacy material into a strict template can reduce local narrative flow.
2. Consolidation may temporarily increase document length due to historical preservation.
3. Additional review effort is required to keep synthesized sections aligned with implementation changes.

## Rationale and alternatives

1. A strict template improves comparability and review quality across proposals.
2. Preserving migrated technical content avoids loss of implementation context.
3. Historical notes keep prior status decisions auditable without duplicating top-level metadata.

## Prior art

No additional prior art identified beyond references already listed in the legacy content.

## Unresolved questions

- No unresolved questions were explicitly listed in the legacy text.
- Follow-up questions should be tracked in Proposal PR and Flux Issue fields when created.

## Future possibilities

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
