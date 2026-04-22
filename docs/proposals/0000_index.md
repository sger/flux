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
| 0032 | [0032_type_system_with_effects.md](0032_type_system_with_effects.md) | Implemented | have | typed syntax, HM/effect groundwork, effect handlers, and diagnostic baseline are implemented | keep as foundational typed/effects baseline; newer proposals own post-`Any` static-typing policy |
| 0034 | [0034_builtin_primops.md](implemented/0034_builtin_primops.md) | In Progress | partial | `src/primop/mod.rs`, `tests/primop_*`, `examples/primop/all_primops.flx` | continue phase completion |
| 0042 | [0042_effect_rows_and_constraints.md](implemented/0042_effect_rows_and_constraints.md) | Implemented | have | effect-row fixtures `examples/type_system/30..45`, `61`, `162`, compiler effect solver paths in `src/bytecode/compiler/*`, plus closure evidence in proposal 0042 | keep as baseline closure for current scope |
| 0043 | [0043_pure_flux_checklist.md](implemented/0043_pure_flux_checklist.md) | Implemented | have | `tests/purity_vm_jit_parity_snapshots.rs`, `tests/common/purity_parity.rs`, A-G fixtures | keep as closure evidence |
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
| 0086 | [0086_backend_neutral_core_ir.md](implemented/0086_backend_neutral_core_ir.md) | Implemented | have | `src/core/*`, `src/cfg/*`, Core IR lowering, validation, optimization passes (constant_fold, dead_block_elimination, local_cse, type_directed_unboxing), VM+JIT consume Core IR | keep as baseline — canonical backend-neutral IR layer |
| 0099 | [0099_static_purity_completion.md](0099_static_purity_completion.md) | In Progress | partial | Part 2 is closed; Parts 1 and 3 remain active purity/runtime work | keep as purity umbrella, not as the active static-typing roadmap |
| 0123 | [0123_full_static_typing.md](implemented/0123_full_static_typing.md) | Implemented | partial | historical umbrella; current completion and follow-ons are tracked elsewhere | keep for history; defer to `0156` for maintained static-typing status |
| 0127 | [0127_type_inference_ghc_parity.md](implemented/0127_type_inference_ghc_parity.md) | Implemented | have | numeric defaulting, checked signatures/skolems (via `0159`), annotation enforcement, and expression-level validation landed 2026-04-18 under `0160` closure | see delivery commits in the proposal header |
| 0145 | [0145_type_classes.md](implemented/0145_type_classes.md) | Implemented | have | core delivered + follow-ons shipped via 0146 (hardening), 0147 (constrained params + instance contexts), 0149 (operator desugaring), 0150 (HKT resolution), 0127 Phase 3 (Num defaulting); remaining items are method-arity polish, `Monoid` deferred, Foldable stdlib surface | closed 2026-04-18 |
| 0150 | [0150_hkt_instance_resolution.md](implemented/0150_hkt_instance_resolution.md) | Implemented | have | bare constructor instances match HKT arguments; `src/types/class_env.rs::match_instance_type_expr` + resolver unit tests | closed 2026-04-09; feeds 0145 |
| 0155 | [0155_core_ir_parity_simplification.md](implemented/0155_core_ir_parity_simplification.md) | Implemented | have | `core_lint` (E998) enforced after every simplification round plus five new Core passes (algebraic, const_fold, canonicalize, specialize, disciplined_inline) landed 2026-04-18 | see delivery commits in the proposal header |
| 0156 | [0156_static_typing_completion_roadmap.md](implemented/0156_static_typing_completion_roadmap.md) | Implemented | have | maintained front-end static typing is complete and documented here | keep as authoritative static-typing completion proposal |
| 0157 | [0157_explicit_core_types_and_runtime_representation_split.md](implemented/0157_explicit_core_types_and_runtime_representation_split.md) | Implemented | have | `CoreType::Dynamic` removed from `src/core/mod.rs`; `IrType::Dynamic` removed from `src/cfg/mod.rs` (replaced by `Tagged`) — Tracks 1 and 3 landed 2026-04-18 | execution recorded in 0158 |
| 0158 | [0158_core_semantic_types_and_backend_rep_split_execution.md](implemented/0158_core_semantic_types_and_backend_rep_split_execution.md) | Implemented | have | explicit Core semantic residue + rep-oriented CFG/LIR cleanup landed in maintained paths | keep as downstream execution closure |
| 0159 | [0159_signature_directed_checking_and_skolemisation.md](implemented/0159_signature_directed_checking_and_skolemisation.md) | Implemented | have | bidirectional check mode, rigid skolem variables (E305), recursive pre-binding, and call-site lambda propagation landed 2026-04-17 | see `docs/internals/signature_directed_checking.md` and `docs/internals/proposal_0159_investigation.md` |
| 0160 | [0160_static_typing_hardening_closure.md](implemented/0160_static_typing_hardening_closure.md) | Implemented | have | scheme-surface normalization, runtime boundary hardening, expression-level E430, and closure over `0127`/`0155`/`0159` acceptance criteria landed 2026-04-18 | see delivery commits in the proposal header |
| 0164 | [0164_internal_primop_contract_and_stdlib_surface.md](implemented/0164_internal_primop_contract_and_stdlib_surface.md) | Implemented | have | 24 `CorePrimOp` variants removed (Phase 7); `public intrinsic fn = primop …` surface live in `Flow.Array/Map/String`; math + bitwise primops added (Phase 2); 6 text primops reclassified to stay internal per Unicode/C-runtime constraints. Landed 2026-04-21 | keep as baseline for any future primop migration |

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
| 0026 | [0026_concurrency_model.md](0026_concurrency_model.md) | Proposed | gap | proposal-only | historical umbrella; supersede active planning with 0115 |
| 0027 | [0027_language_syntax_specification.md](0027_language_syntax_specification.md) | Proposed | partial | syntax implemented across `src/syntax/*`; spec not fully synchronized | convert to living spec |
| 0028 | [0028_base.md](implemented/0028_base.md) | Proposed | partial | Base APIs + docs exist (`docs/internals/base_api.md`, runtime/base tests) | mark implemented subset |
| 0030 | [0030_flow.md](implemented/0030_flow.md) | Implemented | have | 10 modules in `lib/Flow/` (List, Option, String, Map, Numeric, Array, Assert, FTest, IO, NonZero); module graph + virtual-module resolution operational | closed 2026-04-18; `Flow.Either` redundant with built-in constructors; `Flow.Func` subsumed by `|>` operator |
| 0033 | [0033_jit_cache_compatibility.md](implemented/0033_jit_cache_compatibility.md) | Proposed | partial | strict cache identity checks now exist in strict workstream | align with actual cache keys/tests |
| 0035 | [0035_unit_test_framework.md](0035_unit_test_framework.md) | Draft | partial | test mode and Flux tests exist (`src/main.rs --test`, `tests/test_runner_cli.rs`, `lib/Flow/FTest.flx`) | rewrite as “current + gaps” |
| 0036 | [0036_multiline_strings.md](implemented/0036_multiline_strings.md) | Draft | gap | proposal-only | keep in syntax backlog |
| 0037 | [0037_grammar_improvements.md](0037_grammar_improvements.md) | Draft | gap | proposal-only | keep in parser backlog |
| 0038 | [0038_deterministic_effect_replay.md](0038_deterministic_effect_replay.md) | Draft | gap | proposal-only | keep as advanced runtime/debugging backlog |
| 0039 | [0039_typed_module_contracts.md](implemented/0039_typed_module_contracts.md) | Draft | partial | typed/effect groundwork in 032/042; full contract boundary checks incomplete | continue after 032/043 baseline |
| 0040 | [0040_macro_system.md](0040_macro_system.md) | Draft | gap | proposal-only | canonical macro proposal going forward |
| 0041 | [0041_nan_boxing_runtime_optimization.md](0041_nan_boxing_runtime_optimization.md) | Proposed | gap | proposal-only | keep in runtime perf backlog |
| 0045 | [0045_gc.md](implemented/0045_gc.md) | Draft | gap | proposal-only (renamed from `0010_gc.md`) | continue as canonical GC proposal |
| 0074 | [0074_base_signature_tightening.md](implemented/0074_base_signature_tightening.md) | Draft | gap | proposal-only | tighten BaseHmSignature types from Any to precise polymorphic types for compile-time error detection on builtin calls |
| 0081 | [0081_diagnostic_taxonomy.md](implemented/0081_diagnostic_taxonomy.md) | Draft | gap | proposal-only | add an explicit semantic category layer for diagnostics on top of codes, severity, phase, and error-type metadata |
| 0082 | [0082_effect_directed_pipelines.md](0082_effect_directed_pipelines.md) | Draft | gap | proposal-only | make typed effect handling compose as a first-class pipeline stage via `expr |> handle Effect { ... }` |
| 0083 | [0083_typed_holes.md](0083_typed_holes.md) | Draft | gap | proposal-only | add expression-level typed holes like `?name` so the compiler can report expected types and candidate fits while code is incomplete |
| 0107 | [0107_general_tail_call_elimination.md](0107_general_tail_call_elimination.md) | Partially Implemented | partial | direct tail calls + mutual recursion landed on VM and LLVM; `promote_tail_calls` marks direct calls as `tail call fastcc`; CPS via 0162 evidence path. Indirect closure tail calls on LLVM remain open (calling-convention work). | finish LLVM indirect tail path in v0.0.9 |
| 0109 | [0109_vm_backend_optimizations.md](0109_vm_backend_optimizations.md) | Partially Implemented | partial | Phase 4 NaN-boxing in `src/runtime/nanbox.rs`; partial Phase 2 superinstructions. Phase 1 (computed goto) and Phase 3 (lazy compilation) open | keep as VM perf backlog |
| 0112 | [0112_shared_pipeline_optimizations.md](0112_shared_pipeline_optimizations.md) | Partially Implemented | partial | Phase 1 iterative Core simplifier (`MAX_SIMPLIFIER_ROUNDS = 3`) landed via 0155. Phase 2 (SSA CFG) and Phase 3 (Core IR cache) open | keep for SSA/cache follow-on |
| 0126 | [0126_diagnostic_rendering_improvements.md](0126_diagnostic_rendering_improvements.md) | Partially Implemented | partial | Phase 1 line elision shipped in `src/diagnostics/rendering/source.rs`. Phase 2 (narrow signature spans) and Phase 3 (dedup related notes) open | keep as DX follow-on |
| 0129 | [0129_vm_improvements_ghc_inspired.md](0129_vm_improvements_ghc_inspired.md) | Draft | gap | proposal-only | computed goto, 20+ superinstructions, debugger, FFI, 16-bit bytecode — none landed |
| 0130 | [0130_benchmark_framework.md](0130_benchmark_framework.md) | Draft | gap | proposal-only | Flux-language-level `Flow.Bench` + `clock_ms` primitive (criterion benches in `benches/` do not satisfy this proposal) |
| 0135 | [0135_total_functions_and_safe_arithmetic.md](0135_total_functions_and_safe_arithmetic.md) | Partially Implemented | partial | Phase 1 (`safe_div`/`safe_mod` → `Option<Int>`) shipped across VM + LLVM + C runtime. Phases 2 (NonZero), 3 (operator edition), 4 (refinement types) open | keep as totality follow-on |
| 0161 | [0161_effect_system_decomposition_and_capabilities.md](0161_effect_system_decomposition_and_capabilities.md) | Draft | gap | new umbrella (2026-04-18) | Flow.Effects as source of truth + sealing + optimizer levels; supersedes 0075, 0108, 0131 |
| 0162 | [0162_unified_effect_handler_runtime.md](0162_unified_effect_handler_runtime.md) | Draft | gap | new umbrella (2026-04-18) | Koka-style evidence passing + monomorphic State/Reader + unified yield algorithm; supersedes 0072, 0073, 0141 |
| 0166 | [0166_pattern_exhaustiveness_and_redundancy_checking.md](0166_pattern_exhaustiveness_and_redundancy_checking.md) | Proposed | gap | proposal-only | add real match coverage checking over ADTs, tuples, lists, and constructor families |
| 0167 | [0167_static_typing_contract_hardening.md](0167_static_typing_contract_hardening.md) | Proposed | gap | proposal-only | strengthen static typing as a single HM/Core contract: infinite types, unresolved boundaries, AST fallback reduction, diagnostic ranking |
| 0168 | [0168_hkt_polymorphic_dispatch_completion.md](0168_hkt_polymorphic_dispatch_completion.md) | Proposed | gap | proposal-only | complete polymorphic constructor-headed/HKT class dispatch via dictionary elaboration instead of panic stubs |
| 0143 | [0143_actor_concurrency_roadmap.md](0143_actor_concurrency_roadmap.md) | Draft | gap | realistic phased plan refreshed 2026-04-18 (A–F, ~2-year horizon) | canonical concurrency roadmap; supersedes 0026/0065/0066/0067/0071/0095; depends on 0161 + 0162 |
| 0151 | [0151_module_scoped_type_classes.md](implemented/0151_module_scoped_type_classes.md) | Implemented | have | Phases 1–8 delivered (parsing, ClassId refactor, member collection, orphan rule, visibility, qualified lookup, dispatch, Core lowering); 39 tests green; E455–E458 live; follow-ons (stdlib migration, hard deprecation) not blocking | closed 2026-04-18 |
| 0152 | [0152_named_fields_for_data_types.md](0152_named_fields_for_data_types.md) | Draft | gap | proposal-only | named fields + dot-access + functional update syntax; supersedes 0048 |

### Superseded Historical Docs

| ID | Proposal | Status | Canonical Successor | Action |
|---|---|---|---|---|
| 0009 | [0009_macro_system.md](superseded/0009_macro_system.md) | Superseded | [0040_macro_system.md](0040_macro_system.md) | keep for history with supersession banner |
| 0026 | [0026_concurrency_model.md](superseded/0026_concurrency_model.md) | Superseded | [0143_actor_concurrency_roadmap.md](0143_actor_concurrency_roadmap.md) | async/await+actors umbrella replaced by Aether-aware roadmap (2026-04-18) |
| 0029 | [0029_base_and_flow.md](superseded/0029_base_and_flow.md) | Superseded | [0028_base.md](implemented/0028_base.md), [0030_flow.md](implemented/0030_flow.md) | keep for history with supersession banner |
| 0048 | [0048_typed_record_types.md](superseded/0048_typed_record_types.md) | Superseded | [0152_named_fields_for_data_types.md](0152_named_fields_for_data_types.md) | header already flagged supersession (2026-04-18) |
| 0065 | [0065_actor_effect_stdlib.md](superseded/0065_actor_effect_stdlib.md) | Superseded | [0143_actor_concurrency_roadmap.md](0143_actor_concurrency_roadmap.md) | replaced by Aether-aware roadmap (2026-04-18) |
| 0066 | [0066_thread_per_actor_handler.md](superseded/0066_thread_per_actor_handler.md) | Superseded | [0143_actor_concurrency_roadmap.md](0143_actor_concurrency_roadmap.md) | replaced by Aether-aware roadmap (2026-04-18) |
| 0067 | [0067_gchandle_actor_boundary_error.md](superseded/0067_gchandle_actor_boundary_error.md) | Superseded | [0143_actor_concurrency_roadmap.md](0143_actor_concurrency_roadmap.md) | predated Aether; header already flagged (2026-04-18) |
| 0071 | [0071_mn_scheduler_actor_handler.md](superseded/0071_mn_scheduler_actor_handler.md) | Superseded | [0143_actor_concurrency_roadmap.md](0143_actor_concurrency_roadmap.md) | replaced by Aether-aware roadmap (2026-04-18) |
| 0072 | [0072_evidence_passing_handlers.md](superseded/0072_evidence_passing_handlers.md) | Superseded | [0162_unified_effect_handler_runtime.md](0162_unified_effect_handler_runtime.md) | folded into 0162 Phase 1 (2026-04-18) |
| 0073 | [0073_state_reader_continuation_elim.md](superseded/0073_state_reader_continuation_elim.md) | Superseded | [0162_unified_effect_handler_runtime.md](0162_unified_effect_handler_runtime.md) | folded into 0162 Phase 2 (2026-04-18) |
| 0075 | [0075_effect_sealing.md](superseded/0075_effect_sealing.md) | Superseded | [0161_effect_system_decomposition_and_capabilities.md](0161_effect_system_decomposition_and_capabilities.md) | folded into 0161 Phase 2 (2026-04-18) |
| 0108 | [0108_base_function_effect_audit.md](superseded/0108_base_function_effect_audit.md) | Superseded | [0161_effect_system_decomposition_and_capabilities.md](0161_effect_system_decomposition_and_capabilities.md) | folded into 0161 Phase 1.5 (2026-04-18) |
| 0131 | [0131_primop_effect_levels.md](superseded/0131_primop_effect_levels.md) | Superseded | [0161_effect_system_decomposition_and_capabilities.md](0161_effect_system_decomposition_and_capabilities.md) | folded into 0161 Phase 3 — derive from effect rows (2026-04-18) |
| 0141 | [0141_unified_effect_handlers.md](superseded/0141_unified_effect_handlers.md) | Superseded | [0162_unified_effect_handler_runtime.md](0162_unified_effect_handler_runtime.md) | folded into 0162 Phase 3 (2026-04-18) |
| 0095 | [0095_actor_runtime_architecture.md](superseded/0095_actor_runtime_architecture.md) | Superseded | [0143_actor_concurrency_roadmap.md](0143_actor_concurrency_roadmap.md) | replaced by Aether-aware roadmap (2026-04-18) |

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
