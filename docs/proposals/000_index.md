# Proposal Index and Canonical State

**Status:** Implemented  
**Date:** 2026-02-26  
**Depends on:** None

---

This is the canonical inventory for `docs/proposals`.  
Feature state is evidence-driven:
- `have` = implemented and exercised in `src/` plus tests/examples
- `partial` = implemented in part or implemented without full hardening/coverage
- `gap` = proposal-only or no convincing implementation evidence yet

## Active Track (Implemented / In Progress)

| ID | Proposal | Status | Feature State | Evidence | Action |
|---|---|---|---|---|---|
| 001 | [001_module_constants.md](001_module_constants.md) | Implemented | have | `src/bytecode/module_constants/*`, `src/bytecode/module_constants/module_constants_test.rs` | keep as baseline |
| 002 | [002_error_code_registry.md](002_error_code_registry.md) | Implemented | have | `src/diagnostics/compiler_errors.rs`, `tests/error_codes_registry_tests.rs` | keep as baseline |
| 014 | [014_lexer_parser_code_review.md](014_lexer_parser_code_review.md) | Implemented | have | `src/syntax/lexer/*`, `src/syntax/parser/*`, `tests/parser_tests.rs` | keep as baseline |
| 016 | [016_tail_call_accumulator_optimization.md](016_tail_call_accumulator_optimization.md) | Implemented | have | `tests/tail_call_tests.rs`, `src/main.rs` (`analyze-tail-calls`) | keep as baseline |
| 018 | [018_runtime_language_evolution_roadmap.md](018_runtime_language_evolution_roadmap.md) | In Progress | partial | roadmap-only umbrella with selective landed items (tail-calls, module constants) | split into smaller executable proposals |
| 019 | [019_zero_copy_value_passing.md](019_zero_copy_value_passing.md) | Implemented | partial | runtime ownership paths in `src/runtime/*`; no dedicated regression matrix tied to 019 | add explicit perf/regression evidence |
| 021 | [021_stack_overflow_fix_for_builtins.md](021_stack_overflow_fix_for_builtins.md) | Implemented | have | `src/runtime/vm/mod.rs`, `tests/vm_tests.rs`, `tests/tail_call_tests.rs` | keep as baseline |
| 022 | [022_ast_traversal_framework.md](022_ast_traversal_framework.md) | Implemented | have | `tests/ast_visit_smoke.rs`, `tests/ast_fold_smoke.rs`, AST traversal tests | keep as baseline |
| 031 | [031_cranelift_jit_backend.md](031_cranelift_jit_backend.md) | Draft | partial | JIT compile path in CLI/tests (`--features jit`, `tests/jit_phase3_tests.rs`, `tests/jit_phase4_tests.rs`) | align proposal with actual backend scope |
| 032 | [032_type_system_with_effects.md](032_type_system_with_effects.md) | Implemented | have | `src/bytecode/compiler/mod.rs`, `src/bytecode/compiler/expression.rs`, `examples/type_system/*` | keep as semantic source of truth |
| 034 | [034_builtin_primops.md](034_builtin_primops.md) | In Progress | partial | `src/primop/mod.rs`, `tests/primop_*`, `examples/primop/all_primops.flx` | continue phase completion |
| 042 | [042_effect_rows_and_constraints.md](042_effect_rows_and_constraints.md) | In Progress | partial | effect-row fixtures `examples/type_system/30..45`, compiler effect solver paths in `src/bytecode/compiler/*` | continue row hardening |
| 043 | [043_pure_flux_checklist.md](043_pure_flux_checklist.md) | Implemented | have | `tests/purity_vm_jit_parity_snapshots.rs`, `tests/common/purity_parity.rs`, A-G fixtures | keep as closure evidence |
| 044 | [044_compiler_phase_pipeline_refactor.md](044_compiler_phase_pipeline_refactor.md) | Draft | gap | proposal-only | next architecture/perf workstream |
| 046 | [046_typed_ast_hm_architecture.md](046_typed_ast_hm_architecture.md) | Draft | gap | proposal-only | post-0.0.4 HM architecture deepening |
| 049 | [049_effect_rows_completeness.md](049_effect_rows_completeness.md) | Draft | gap | proposal-only | complete row solver semantics and deterministic diagnostics after 042 baseline |
| 050 | [050_totality_and_exhaustiveness_hardening.md](050_totality_and_exhaustiveness_hardening.md) | Draft | gap | proposal-only | stage-1 exhaustiveness hardening owner under roadmap 054 |
| 051 | [051_any_fallback_reduction.md](051_any_fallback_reduction.md) | Draft | gap | proposal-only | stage-1 HM zero-fallback owner under roadmap 054 |
| 052 | [052_auto_currying_and_partial_application.md](052_auto_currying_and_partial_application.md) | Draft | gap | proposal-only | stage-2 post-0.0.4 currying/placeholder feature track under roadmap 054 |
| 053 | [053_traits_and_typeclasses.md](053_traits_and_typeclasses.md) | Draft | gap | proposal-only | stage-2 post-0.0.4 trait/typeclass feature track under roadmap 054 |
| 054 | [054_0_0_4_hm_adt_exhaustiveness_critical_path.md](054_0_0_4_hm_adt_exhaustiveness_critical_path.md) | Draft | gap | proposal-only | canonical sequencing and gates for 0.0.4 HM/ADT/exhaustiveness + post-0.0.4 feature tracks |
| 055 | [055_lexer_performance_and_architecture.md](055_lexer_performance_and_architecture.md) | Draft | gap | proposal-only | phased lexer perf + architecture hardening with parser-contract and benchmark gates |
| 056 | [056_parser_performance_and_architecture.md](056_parser_performance_and_architecture.md) | Draft | gap | proposal-only | phased parser perf + architecture hardening with lexer/parser contract and benchmark gates |
| 057 | [057_parser_diagnostics_with_inferred_types.md](057_parser_diagnostics_with_inferred_types.md) | Implemented | have | `src/ast/type_infer.rs`, `src/types/unify_error.rs`, `src/diagnostics/compiler_errors.rs`, fixtures 92–105, `tests/type_inference_tests.rs`, `tests/compiler_rules_tests.rs` | completed — ReportContext, if/match contextual E300, E056 arity, fun decomposition, parser recovery, multi-error continuation |
| 058 | [058_contextual_diagnostics_callsite_let_return.md](058_contextual_diagnostics_callsite_let_return.md) | Draft | gap | proposal-only | named call-site argument diagnostics, let-annotation dual-label, function return dual-label — direct continuation of 057 |

## Backlog (Draft / Proposed)

| ID | Proposal | Status | Feature State | Evidence | Action |
|---|---|---|---|---|---|
| 003 | [003_stdlib_proposal.md](003_stdlib_proposal.md) | Draft | gap | proposal-only | re-scope vs current `Base`/`Flow` direction |
| 004 | [004_language_features_proposal.md](004_language_features_proposal.md) | Draft | gap | proposal-only | split into smaller proposals |
| 005 | [005_symbol_interning.md](005_symbol_interning.md) | Draft | gap | proposal-only | move to perf track if still desired |
| 006 | [006_phase1_module_split_plan.md](006_phase1_module_split_plan.md) | Proposed | partial | module compiler is split in practice (`statement.rs`, `expression.rs`) | mark implemented parts, close leftovers |
| 007 | [007_visitor_pattern.md](007_visitor_pattern.md) | Proposed | partial | largely superseded by 022 traversal framework | mark superseded by 022 |
| 008 | [008_builtins_module_architecture.md](008_builtins_module_architecture.md) | Proposed | partial | base/primop separation exists (`src/base/*`, `src/primop/*`) | sync proposal to current architecture |
| 010 | [010_advanced_linter.md](010_advanced_linter.md) | Draft | gap | current linter exists but not this full spec (`src/syntax/linter.rs`) | reduce scope and re-propose |
| 011 | [011_phase2_module_system_enhancements.md](011_phase2_module_system_enhancements.md) | Proposed | partial | module imports/type fixtures exist in `examples/type_system/TypeSystem/*` | decompose into deliverable increments |
| 012 | [012_phase2_module_split_plan.md](012_phase2_module_split_plan.md) | Proposed | partial | module compiler split partially landed | merge with 011/006 cleanup |
| 013 | [013_phase3_advanced_architecture.md](013_phase3_advanced_architecture.md) | Proposed | gap | umbrella planning proposal | split into concrete proposals |
| 015 | [015_package_module_workflow_mvp.md](015_package_module_workflow_mvp.md) | Proposed | partial | module roots (`--root`) exist; package workflow not fully present | narrow to package manager MVP |
| 017 | [017_persistent_collections_and_gc.md](017_persistent_collections_and_gc.md) | Proposed | gap | proposal-only | supersede by targeted runtime proposals |
| 020 | [020_map_filter_fold_builtins.md](020_map_filter_fold_builtins.md) | Proposed | partial | base functions and tests exist (`tests/base_functions_tests.rs`) | mark landed APIs vs pending |
| 023 | [023_bytecode_decode_passes.md](023_bytecode_decode_passes.md) | Proposed | gap | proposal-only | keep as compiler tooling backlog |
| 024 | [024_runtime_instrumentation_and_value_tracer.md](024_runtime_instrumentation_and_value_tracer.md) | Proposed | gap | proposal-only | keep as observability backlog |
| 025 | [025_pure_fp_language_vision.md](025_pure_fp_language_vision.md) | Proposed | partial | vision reflected by 032/042/043 completion | keep as vision, not delivery plan |
| 026 | [026_concurrency_model.md](026_concurrency_model.md) | Proposed | gap | proposal-only | next-phase research |
| 027 | [027_language_syntax_specification.md](027_language_syntax_specification.md) | Proposed | partial | syntax implemented across `src/syntax/*`; spec not fully synchronized | convert to living spec |
| 028 | [028_base.md](028_base.md) | Proposed | partial | Base APIs + docs exist (`docs/internals/base_api.md`, runtime/base tests) | mark implemented subset |
| 030 | [030_flow.md](030_flow.md) | Proposed | partial | Flow module usage/examples exist (`examples/type_system/TypeSystem/*`) | mark implemented subset |
| 033 | [033_jit_cache_compatibility.md](033_jit_cache_compatibility.md) | Proposed | partial | strict cache identity checks now exist in strict workstream | align with actual cache keys/tests |
| 035 | [035_unit_test_framework.md](035_unit_test_framework.md) | Draft | partial | test mode and Flux tests exist (`src/main.rs --test`, `tests/test_runner_cli.rs`, `lib/Flow/FTest.flx`) | rewrite as “current + gaps” |
| 036 | [036_multiline_strings.md](036_multiline_strings.md) | Draft | gap | proposal-only | keep in syntax backlog |
| 037 | [037_grammar_improvements.md](037_grammar_improvements.md) | Draft | gap | proposal-only | keep in parser backlog |
| 038 | [038_deterministic_effect_replay.md](038_deterministic_effect_replay.md) | Draft | gap | proposal-only | keep as advanced runtime/debugging backlog |
| 039 | [039_typed_module_contracts.md](039_typed_module_contracts.md) | Draft | partial | typed/effect groundwork in 032/042; full contract boundary checks incomplete | continue after 032/043 baseline |
| 040 | [040_macro_system.md](040_macro_system.md) | Draft | gap | proposal-only | canonical macro proposal going forward |
| 041 | [041_nan_boxing_runtime_optimization.md](041_nan_boxing_runtime_optimization.md) | Proposed | gap | proposal-only | keep in runtime perf backlog |
| 045 | [045_gc.md](045_gc.md) | Draft | gap | proposal-only (renamed from `010_gc.md`) | continue as canonical GC proposal |
| 047 | [047_adt_semantics_deepening.md](047_adt_semantics_deepening.md) | Draft | gap | proposal-only | stage-1 ADT hardening owner under roadmap 054 |
| 048 | [048_typed_record_types.md](048_typed_record_types.md) | Draft | gap | proposal-only | typed immutable records with compile-time field checking and spread update |

## Superseded Historical Docs

| ID | Proposal | Status | Canonical Successor | Action |
|---|---|---|---|---|
| 009 | [009_macro_system.md](009_macro_system.md) | Superseded | [040_macro_system.md](040_macro_system.md) | keep for history with supersession banner |
| 029 | [029_base_and_flow.md](029_base_and_flow.md) | Superseded | [028_base.md](028_base.md), [030_flow.md](030_flow.md) | keep for history with supersession banner |

## Archived Docs

`docs/proposals/archive/` exists for future archival moves. No proposal is archived in this cleanup.

## Renamed File Map

- `010_gc.md` -> `045_gc.md`

## Cleanup Report (2026-02-26)

1. Canonical status vocabulary normalized in proposal headers.
2. Required header metadata (`Status`, `Date`, `Depends on`) added to all proposals.
3. Numbering collision resolved by renaming `010_gc.md` to `045_gc.md`.
4. Macro track canonicalized: `040` is canonical, `009` is superseded.
5. Historical split retained: `029` superseded by `028` and `030`.
6. Cross-links updated for canonical macro references in roadmap/proposal docs.

## Migration Notes

If an old link still points to `010_gc.md`, update it to `045_gc.md`.
If an old link still points to macro roadmap proposal `009`, prefer `040` unless historical context is intentional.
