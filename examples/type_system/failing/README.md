# Intentional Failing Examples

These fixtures are expected to fail and are useful for validating diagnostics.

## Files

- `01_compile_type_mismatch.flx`
  - Expected: compile-time failure (`E055` type mismatch)
- `02_runtime_boundary_arg_violation.flx`
  - Expected: compile-time failure (`E300`) for concrete call-argument mismatch (no runtime boundary fallback)
- `03_runtime_return_violation.flx`
  - Expected: compile-time failure (`E300`) for concrete return-type mismatch (no runtime boundary fallback)
- `04_compile_float_string_arg.flx`
  - Expected: compile-time failure (`E055`) type mismatch (`Float` expected, `String` passed)
- `05_runtime_float_string_arg_via_any.flx`
  - Expected: runtime failure (`E1004`) at typed boundary argument check (`Float` expected)
- `06_runtime_float_string_return.flx`
  - Expected: runtime failure (`E1004`) at typed return boundary check (`Float` expected)
- `07_typed_let_float_into_int.flx`
  - Expected: compile-time failure (`E300`) on typed `let` initializer mismatch (`Int` annotated, `Float` assigned)
- `08_compile_identifier_type_mismatch.flx`
  - Expected: compile-time failure (`E300`) on typed `let` from identifier (`Int` annotated, `String` value)
- `09_compile_typed_call_return_mismatch.flx`
  - Expected: compile-time failure (`E300`) on typed `let` from typed call return (`Int` annotated, `String` return)
- `15_effect_missing_from_caller.flx`
  - Expected: compile-time failure (`E400`) when a typed-pure caller invokes a `with IO` function
- `16_inferred_effect_missing_on_typed_caller.flx`
  - Expected: compile-time failure (`E400`) when caller declares `with Time` but invokes an IO function
- `17_handle_unknown_operation.flx`
  - Expected: compile-time failure (`E401`) when a `handle` arm names an operation not declared by the effect
- `18_handle_incomplete_operation_set.flx`
  - Expected: compile-time failure (`E402`) when a `handle` block misses declared effect operations
- `19_effect_polymorphism_missing_effect.flx`
  - Expected: compile-time failure (`E400`) when `with e` resolves to `IO` but caller declares only `Time`
- `20_direct_builtin_missing_effect.flx`
  - Expected: compile-time failure (`E400`) when `with Time` function directly calls IO builtin (`print`)
- `21_perform_unknown_operation.flx`
  - Expected: compile-time failure (`E404`) when `perform` references an operation not declared by the effect
- `22_effect_polymorphism_chain_missing_effect.flx`
  - Expected: compile-time failure (`E400`) in chained `with e` wrappers when callback resolves to `IO` but caller declares only `Time`
- `23_generic_call_return_mismatch.flx`
  - Expected: compile-time failure (`E300`) for generic typed `let` mismatch (deduplicated against boundary `E055`)
- `24_adt_guarded_non_exhaustive.flx`
  - Expected: compile-time failure (`E083`) because guarded constructor arms do not guarantee exhaustiveness
- `25_adt_mixed_constructors_in_match.flx`
  - Expected: compile-time failure (`E083`) when one `match` mixes constructors from different ADTs
- `26_adt_match_constructor_arity_mismatch.flx`
  - Expected: compile-time failure (`E085`) when constructor pattern field count mismatches declaration arity
- `27_adt_wildcard_guard_not_catchall.flx`
  - Expected: compile-time failure (`E015`) because `_ if ...` is guarded and does not count as a catch-all arm
- `28_adt_nested_guard_non_exhaustive.flx`
  - Expected: compile-time failure (`E083`) when nested constructor arm is guarded and leaves constructors uncovered
- `29_strict_missing_main.flx`
  - Expected: compile-time failure (`E415`) in `--strict` mode because all strict programs require `fn main`
- `30_strict_public_unannotated_effectful.flx`
  - Expected: compile-time failure (`E416`, `E417`, `E418`) in `--strict` mode because a public effectful function must annotate params/return/effects
- `31_direct_time_builtin_missing_effect.flx`
  - Expected: compile-time failure (`E400`) when a `with IO` function directly calls Time builtin (`now_ms`)
- `32_direct_time_hof_missing_effect.flx`
  - Expected: compile-time failure (`E400`) when a `with IO` function directly calls Time builtin (`time`)
- `33_module_qualified_effect_propagation_missing.flx`
  - Expected: compile-time failure (`E400`) when module-qualified call requires `IO` inside a `with Time` function
- `34_generic_effect_propagation_missing.flx`
  - Expected: compile-time failure (`E400`) when generic higher-order wrapper propagates callback `IO` into a `with Time` function
- `35_pure_context_typed_pure_rejects_io.flx`
  - Expected: compile-time failure (`E400`) when typed pure function directly calls `print`
- `36_pure_context_time_only_rejects_io.flx`
  - Expected: compile-time failure (`E400`) when `with Time` function directly calls `print`
- `37_pure_context_unannotated_infers_io_then_rejects_time_caller.flx`
  - Expected: compile-time failure (`E400`) when unannotated callee infers `IO` and a `with Time` caller invokes it
- `38_top_level_effect_rejected.flx`
  - Expected: compile-time failure (`E413`, `E414`) for effectful top-level execution outside `fn main`
- `40_effect_alias_print_in_time_function.flx`
  - Expected: compile-time failure (`E400`) when `print` is called via local alias in a `with Time` function
- `41_effect_alias_now_ms_in_io_function.flx`
  - Expected: compile-time failure (`E400`) when `now_ms` is called via local alias in a `with IO` function
- `42_handle_unknown_effect.flx`
  - Expected: compile-time failure (`E405`) when `handle` references an undeclared effect
- `43_main_unhandled_custom_effect.flx`
  - Skipped: VM currently runs this custom-effect `main`, while LLVM reports an unhandled effect at runtime
- `44_effect_poly_hof_nested_missing_effect.flx`
  - Expected: compile-time failure (`E400`) when nested polymorphic wrappers resolve `e` to `IO` but caller declares only `Time`
- `45_effect_row_subtract_missing_io.flx`
  - Expected: compile-time failure (`E400`) when row subtraction leaves `IO` required but caller declares only `Time`
- `46_duplicate_main_function.flx`
  - Expected: compile-time failure (`E410`) when more than one top-level `fn main` exists
- `47_main_with_parameters.flx`
  - Expected: compile-time failure (`E411`) when `fn main` declares parameters
- `48_main_invalid_return_type.flx`
  - Expected: compile-time failure (`E412`) when `fn main` declares non-`Unit` return type
- `49_top_level_effect_with_existing_main.flx`
  - Expected: compile-time failure (`E413`) for effectful top-level execution even if `fn main` exists
- `50_invalid_main_signature_no_root_discharge_noise.flx`
  - Expected: compile-time failure (`E412`) without redundant `E406` root-discharge cascade
- `51_strict_public_missing_param_annotation.flx`
  - Expected: compile-time failure (`E416`) because `public fn` must annotate all parameters in `--strict`
- `52_strict_public_missing_return_annotation.flx`
  - Expected: compile-time failure (`E417`) because `public fn` must declare return type in `--strict`
- `53_strict_public_effectful_missing_with.flx`
  - Expected: compile-time failure (`E418`) because effectful `public fn` must declare explicit `with ...` in `--strict`
- `54_strict_any_param_rejected.flx`
  - Expected: compile-time failure (`E423`) because `Any` is rejected in `--strict`
- `55_strict_any_return_rejected.flx`
  - Expected: compile-time failure (`E423`) because `Any` is rejected in `--strict`
- `56_strict_any_nested_rejected.flx`
  - Expected: compile-time failure (`E423`) because nested `Any` is rejected in `--strict`
- `57_strict_entry_path_parity.flx`
  - Expected: compile-time failure (`E416`) consistently across `run`, `--test`, and `bytecode` strict paths
- `58_strict_public_underscore_missing_annotation.flx`
  - Expected: compile-time failure (`E416`) because underscore naming is style-only and `public fn` still enforces strict API annotations
- `59_strict_module_public_effect_missing_with.flx`
  - Expected: compile-time failure (`E400`) because strict/pure context rejects effectful body without matching effect annotation
- `61_strict_generic_unresolved_boundary.flx`
  - Expected: compile-time failure (`E425`) in `--strict` when runtime boundary type resolution is generic/unresolved
- `62_list_boundary_runtime_violation.flx`
  - Expected: runtime failure (`E1004`) because `List<Int>` boundary receives a non-list dynamic value from module boundary
- `63_either_boundary_runtime_violation.flx`
  - Expected: runtime failure (`E1004`) because `Either<String, Int>` boundary receives a non-Either dynamic value from module boundary
- `185_runtime_boundary_arg_e1004.flx`
  - Expected: runtime failure (`E1004`) for typed argument boundary from module-qualified dynamic source
- `186_runtime_boundary_return_e1004.flx`
  - Expected: runtime failure (`E1004`) for typed return boundary from module-qualified dynamic source
- `187_runtime_list_boundary_e1004.flx`
  - Expected: runtime failure (`E1004`) because `List<Int>` boundary receives a `String` from module boundary
- `188_runtime_either_boundary_e1004.flx`
  - Expected: runtime failure (`E1004`) because `Either<String, Int>` boundary receives a `String` from module boundary
- `189_contextual_boundary_unresolved_strict_e425.flx`
  - Expected: compile-time failure (`E425`) in `--strict` for unresolved generic boundary checks on module-qualified calls
- `190_contextual_boundary_arg_runtime_e1004.flx`
  - Expected: runtime failure (`E1004`) for typed argument boundary mismatch from module-qualified dynamic source
- `191_contextual_effect_missing_module_call_e400.flx`
  - Expected: compile-time failure (`E400`) when module-qualified call requires `IO` inside a `with Time` function
- `64_hm_inferred_call_mismatch.flx`
  - Expected: compile-time failure (`E300`) from HM-inferred numeric function called with `String`
- `65_adt_nested_constructor_non_exhaustive.flx`
  - Expected: compile-time failure (`E083`) because nested constructor space under unary wrapper is not fully covered
- `66_module_constructor_not_public_api.flx`
  - Expected: strict-mode compile-time failure (`E086`) because direct cross-module constructor access is rejected (use module `public fn` factories)
- `67_adt_multi_arity_nested_non_exhaustive.flx`
  - Expected: compile-time failure (`E083`) because nested constructor-space under multi-arity constructor is not fully covered
- `68_adt_nested_list_non_exhaustive.flx`
  - Expected: compile-time failure (`E083`) because nested list patterns miss non-empty branch
- `69_hm_typed_let_infix_compile_mismatch.flx`
  - Expected: compile-time failure (`E300`) because typed-let infix operands are known incompatible at compile time
- `70_hm_prefix_non_numeric_compile_mismatch.flx`
  - Expected: compile-time failure (`E300`) because unary `-` operand is known non-numeric at compile time
- `71_hm_if_known_type_compile_mismatch.flx`
  - Expected: compile-time failure (`E300`) because `if` branch join is statically `String` and cannot unify with expected `Int`
- `72_hm_match_known_type_compile_mismatch.flx`
  - Expected: compile-time failure (`E300`) because `match` arm join is statically `String` and cannot unify with expected `Int`
- `73_hm_index_non_int_compile_mismatch.flx`
  - Expected: compile-time failure (`E300`) because index expression is statically non-`Int` for array/list/tuple access
- `74_hm_index_non_indexable_compile_mismatch.flx`
  - Expected: compile-time failure (`E300`) because indexed value is statically non-indexable
- `75_hm_if_non_bool_condition_compile_mismatch.flx`
  - Expected: compile-time failure (`E300`) because HM rejects non-`Bool` `if` condition (`Int` vs `Bool`)
- `76_hm_match_guard_non_bool_compile_mismatch.flx`
  - Expected: compile-time failure (`E300`) because `match` guard is statically non-`Bool`
- `77_hm_logical_non_bool_compile_mismatch.flx`
  - Expected: compile-time failure (`E300`) because HM rejects non-`Bool` operand in logical expression
- `78_hm_inline_call_no_runtime_fallback.flx`
  - Expected: compile-time failure (`E300`) proving typed validation uses HM strict-path and does not fall back to runtime-boundary compatibility inference for inline function-expression calls
- `79_hm_module_generic_call_mismatch.flx`
  - Expected: compile-time failure (`E300`) because module-qualified generic return type is inferred as `String` and cannot unify with expected `Int`
- `80_type_adt_constructor_arity_mismatch.flx`
  - Expected: compile-time failure (`E085`) showing `type ... = ... | ...` ADT sugar reuses constructor-pattern arity checks
- `81_match_bool_missing_false.flx`
  - Expected: compile-time failure (`E015`) because Bool match misses `false`
- `82_match_list_missing_empty.flx`
  - Expected: compile-time failure (`E015`) because list match misses `[]`
- `83_match_guarded_wildcard_only_non_exhaustive.flx`
  - Expected: compile-time failure (`E015`) because guarded wildcard is not unconditional coverage
- `84_match_tuple_gap_no_fallback.flx`
  - Expected: compile-time failure (`E015`) because tuple match without unguarded fallback is conservatively non-exhaustive
- `88_effect_op_signature_argument_mismatch.flx`
  - Expected: compile-time failure (`E300`) because `perform` argument type does not match the declared effect operation signature
- `89_adt_generic_constructor_hm_mismatch.flx`
  - Expected: compile-time failure (`E300`) because generic ADT constructor argument does not unify with the instantiated annotation
- `90_adt_module_constructor_alias_not_exported.flx`
  - Expected: strict-mode compile-time failure (`E086`) because direct ADT constructor access is rejected across module boundaries even through alias imports
- `91_adt_nested_pattern_binding_type_mismatch.flx`
  - Expected: compile-time failure (`E300`) because nested constructor-pattern binding keeps concrete `Int` field typing
- `92_hm_if_branch_contextual_mismatch.flx`
  - Expected: compile-time failure (`E300`) with contextual if-branch mismatch message and dual labels
- `93_hm_match_arm_contextual_mismatch.flx`
  - Expected: compile-time failure (`E300`) with contextual match-arm mismatch message and dual labels
- `94_wrong_argument_count_too_many.flx`
  - Expected: compile-time failure (`E056`) for statically-known function call with too many arguments
- `95_wrong_argument_count_too_few.flx`
  - Expected: compile-time failure (`E056`) for statically-known function call with too few arguments
- `96_hm_fun_param_mismatch_contextual.flx`
  - Expected: compile-time failure (`E300`) with function parameter mismatch diagnostics naming the mismatching parameter index and types
- `97_hm_fun_return_mismatch_contextual.flx`
  - Expected: compile-time failure (`E300`) with function return mismatch diagnostics naming expected vs actual return types
- `98_hm_fun_arity_mismatch_contextual.flx`
  - Expected: compile-time failure (`E300`) with function arity mismatch diagnostics naming expected vs actual parameter counts
- `99_multi_error_continuation.flx`
  - Expected: compile-time failure with multiple independent diagnostics in one compile run (e.g. `E002` and `E300`) preserved in source order
- `100_unclosed_string_recovery.flx`
  - Expected: parse/compile failure with `E071` and deterministic recovery that preserves subsequent statements
- `101_missing_colon_let_annotation.flx`
  - Expected: parser diagnostic with targeted missing-colon message for let annotation
- `102_missing_colon_function_param.flx`
  - Expected: parser diagnostic with targeted missing-colon message for function parameter annotation
- `103_missing_colon_lambda_param.flx`
  - Expected: parser diagnostic with targeted missing-colon message for lambda parameter annotation
- `104_missing_colon_effect_op.flx`
  - Expected: parser diagnostic with targeted missing-colon message for effect operation signature
- `105_unknown_effect_suggestion.flx`
  - Expected: compile-time failure (`E407`) for unknown function `with ...` effect annotation, with hint suggesting `IO`
- `106_let_annotation_int_string.flx`
  - Expected: compile-time failure (`E300`) with dual-label contextual let-annotation mismatch (`Int` annotation vs `String` initializer)
- `107_let_annotation_bool_int.flx`
  - Expected: compile-time failure (`E300`) with dual-label contextual let-annotation mismatch (`Bool` annotation vs `Int` initializer)
- `108_fun_return_string_vs_int.flx`
  - Expected: compile-time failure (`E300`) with dual-label function return annotation mismatch (`Int` declared vs `String` returned)
- `109_fun_return_bool_vs_unit.flx`
  - Expected: compile-time failure (`E300`) with dual-label function return annotation mismatch (`Bool` declared vs non-`Bool` return expression)
- `110_call_arg_named_fn.flx`
  - Expected: compile-time failure (`E300`) for call-site argument mismatch naming `greet` with definition-site secondary label
- `111_call_arg_anonymous_fn.flx`
  - Expected: compile-time failure (`E300`) for anonymous call-site argument mismatch with primary argument label only
- `112_keyword_alias_def.flx`
  - Expected: parser diagnostic (`E030`) suggesting `fn` for foreign keyword `def`
- `113_keyword_alias_var.flx`
  - Expected: parser diagnostic (`E030`) suggesting `let` for foreign keywords `var`/`const`/`val`
- `114_keyword_alias_case.flx`
  - Expected: parser diagnostic (`E030`) suggesting `match` for foreign keywords `case`/`switch`/`when`
- `115_keyword_alias_elif.flx`
  - Expected: parser diagnostic (`E030`) suggesting `else if` for `elif`/`elsif`
- `116_keyword_alias_end.flx`
  - Expected: parser diagnostic (`E034`) explaining `end` is invalid and `}` should be used
- `117_if_missing_brace.flx`
  - Expected: parser diagnostic (`E034`) with contextual missing-`{` message for `if` body
- `118_let_missing_eq.flx`
  - Expected: parser diagnostic (`E034`) with contextual missing-`=` message for `let` binding
- `119_fn_missing_parens.flx`
  - Expected: parser diagnostic (`E034`) with contextual missing parameter-list message
- `120_match_pipe_separator.flx`
  - Expected: parser diagnostic (`E034`) suggesting `,` instead of `|` between match arms
- `121_match_fat_arrow.flx`
  - Expected: parser diagnostic (`E034`) suggesting `->` instead of `=>` in match arms
- `122_hash_missing_close_brace.flx`
  - Expected: parser diagnostic (`E034`) naming missing `}` for hash literal close
- `123_array_missing_close_bracket.flx`
  - Expected: parser diagnostic (`E034`) naming missing `]` for array literal close
- `124_lambda_missing_close_paren.flx`
  - Expected: parser diagnostic (`E034`) naming missing `)` for lambda parameter list close
- `125_string_interpolation_missing_close_brace.flx`
  - Expected: parser diagnostic (`E034`) naming missing `}` for string interpolation close
- `126_list_comprehension_missing_close_bracket.flx`
  - Expected: parser diagnostic (`E034`) naming missing `]` for list-comprehension close
- `127_match_missing_arrow.flx`
  - Expected: parser diagnostic (`E034`) naming missing `->` in match arm
- `128_lambda_missing_arrow.flx`
  - Expected: parser diagnostic (`E034`) naming missing `->` in lambda
- `129_orphan_constructor_pattern_statement.flx`
  - Expected: parser diagnostic (`E034`) for built-in constructor-pattern-like statement outside `match`
- `130_do_missing_brace.flx`
  - Expected: parser diagnostic (`E034`) naming missing `{` to begin `do` block
- `131_call_arg_span_precision.flx`
  - Expected: compile-time failure (`E300`) with primary label on the mismatching call argument expression span
- `132_let_initializer_span_precision.flx`
  - Expected: compile-time failure (`E300`) with primary label on the typed-let initializer expression span
- `133_if_branch_value_span_precision.flx`
  - Expected: compile-time failure (`E300`) with primary label on the mismatching `if` branch value expression span
- `134_if_concrete_branch_mismatch.flx`
  - Expected: compile-time failure (`E300`) with contextual if-branch mismatch message and dual labels
- `135_if_any_branch_suppressed.flx`
  - Expected: compile-time failure (e.g. `E056` from the follow-up call) while suppressing contextual if-branch mismatch for nested `Any`
- `136_tuple_projection_precise_mismatch.flx`
  - Expected: compile-time failure (`E300`) proving known tuple projection resolves to precise projected type (`String`) instead of unresolved fallback
- `137_tuple_projection_unresolved_path_unchanged.flx`
  - Expected: compile-time failure with unresolved-source behavior unchanged:
    - default mode: `E004` (undefined source symbol)
    - strict typed-validation path: `E425` unresolved boundary in compiler-rules regression test
- `138_match_scrutinee_constraint_propagates.flx`
  - Expected: compile-time failure (`E300`) where family-consistent match arms constrain scrutinee shape and produce a concrete downstream typed-let mismatch
- `139_match_scrutinee_constraint_no_propagation_mixed_family.flx`
  - Expected: compile-time failure (e.g. `E056` follow-up) while heterogeneous constructor families do not force scrutinee-family propagation
- `140_recursive_self_reference_return_precision.flx`
  - Expected: compile-time failure (`E300`) where unannotated self recursion refines return type and downstream typed-let mismatch remains concrete
- `141_recursive_self_reference_negative_guard.flx`
  - Expected: compile-time failure (`E056`) from independent follow-up call, with no recursion-specific diagnostic noise
- `142_match_bool_missing_true.flx`
  - Expected: compile-time failure (`E015`) because Bool match misses `true`
- `143_match_bool_missing_false.flx`
  - Expected: compile-time failure (`E015`) because Bool match misses `false`
- `144_guarded_wildcard_only_non_exhaustive_targeted.flx`
  - Expected: compile-time failure (`E015`) with targeted guarded-wildcard non-exhaustive message
- `146_constructor_pattern_arity_some_too_many.flx`
  - Expected: compile-time failure (`E085`) for constructor-pattern arity mismatch (`BoxI(a, b)`)
- `147_constructor_pattern_arity_none_too_many.flx`
  - Expected: compile-time failure (`E085`) for constructor-pattern arity mismatch (`NoneI(v)`)
- `148_constructor_pattern_arity_left_too_many.flx`
  - Expected: compile-time failure (`E085`) for constructor-pattern arity mismatch (`LeftI(a, b, c)`)
- `149_cross_module_constructor_access_strict.flx`
  - Expected: strict-mode compile-time failure (`E086`) for cross-module constructor access
  - Note: in `examples_fixtures_snapshots`, this fixture may show `E018` due to harness roots; canonical T14 assertions are in `compiler_rules_tests` and focused `cargo run --root examples/type_system ...` commands.
- `150_cross_module_constructor_access_nonstrict_warning.flx`
  - Expected: non-strict warning (`W201`) for cross-module constructor access; compilation continues
  - Note: in `examples_fixtures_snapshots`, this fixture may show `E018` due to harness roots; canonical T14 assertions are in `compiler_rules_tests` and focused `cargo run --root examples/type_system ...` commands.
- Strict-only expectation policy for `150..184`:
  - Canonical assertions for strict-only fixtures (`154`, `155`, `156`, `162`, `168`) must run with `--strict`.
  - Non-strict runs of those fixtures may report unresolved baseline diagnostics (`E004`) first; this is expected and not a regression.
- `151_array_literal_concrete_conflict_prefers_e300.flx`
  - Expected: compile-time failure (`E300`) for concrete heterogeneous array literal conflict (strict-first 051)
- `152_array_literal_callarg_conflict_prefers_e300.flx`
  - Expected: compile-time failure (`E300`) for concrete heterogeneous array argument conflict at call boundary
- `153_match_branch_conflict_prefers_e300.flx`
  - Expected: compile-time failure (`E300`) for concrete `match` arm type disagreement in typed let path
- `154_unresolved_projection_strict_e425.flx`
  - Expected: strict-mode compile-time failure (`E425`) for genuinely unresolved tuple projection source
  - Note: non-strict runs may surface baseline unresolved symbol diagnostics (`E004`) before strict-boundary checks.
- `155_unresolved_member_access_strict_e425.flx`
  - Expected: strict-mode compile-time failure (`E425`) for genuinely unresolved member access source
  - Note: non-strict runs may surface baseline unresolved symbol diagnostics (`E004`) before strict-boundary checks.
- `156_unresolved_call_arg_strict_e425.flx`
  - Expected: strict-mode compile-time failure (`E425`) for genuinely unresolved call argument source
  - Note: non-strict runs may surface baseline unresolved symbol diagnostics (`E004`) before strict-boundary checks.
- `157_match_tuple_missing_catchall_general.flx`
  - Expected: compile-time failure (`E015`) with tuple-conservative non-exhaustive message (unguarded catch-all required)
- `158_match_tuple_guarded_only_non_exhaustive.flx`
  - Expected: compile-time failure (`E015`) because guarded tuple arms are conditional and do not prove exhaustiveness
- `159_match_nested_tuple_mixed_shape_non_exhaustive.flx`
  - Expected: compile-time failure (`E083`) for nested tuple mixed-shape conservative non-exhaustiveness in ADT nested checking
- `161_tuple_destructure_concrete_mismatch_prefers_e300.flx`
  - Expected: strict-mode compile-time failure (`E300`) for concrete tuple-destructure shape mismatch
- `162_tuple_destructure_unresolved_strict_e425.flx`
  - Expected: strict-mode compile-time failure (`E425`) when tuple-destructure source is genuinely unresolved
  - Note: non-strict runs may surface baseline unresolved symbol diagnostics (`E004`) before strict-boundary checks.
- `163_match_concrete_disagreement_prefers_e300.flx`
  - Expected: strict-mode compile-time failure (`E300`) for concrete `match` arm disagreement
- `164_match_unresolved_arm_stays_suppressed.flx`
  - Expected: unresolved arm path suppresses contextual `match` arm mismatch diagnostics (no new false-positive `E300`)
- `165_self_recursive_precision_prefers_e300.flx`
  - Expected: compile-time failure (`E300`) showing self-recursive return precision remains concrete at typed use site
- `166_self_recursive_guard_stable_unresolved.flx`
  - Expected: compile-time failure (`E004`) from unresolved symbol baseline, without recursion-specific false-positive mismatch diagnostics
- `167_tuple_destructure_ordered_concrete_conflict_e300.flx`
  - Expected: strict-mode compile-time failure (`E300`) for concrete tuple-destructure conflict (arity/shape mismatch)
- `168_tuple_destructure_unresolved_guard_strict_e425.flx`
  - Expected: strict-mode compile-time failure (`E425`) when tuple-destructure source remains genuinely unresolved
  - Note: non-strict runs may surface baseline unresolved symbol diagnostics (`E004`) before strict-boundary checks.
- `169_match_disagreement_first_arm_unresolved_still_e300.flx`
  - Expected: strict-mode compile-time failure (`E300`) for concrete `match` arm disagreement even when first arm is unresolved
- `170_match_disagreement_all_concrete_ordering_invariant_e300.flx`
  - Expected: strict-mode compile-time failure (`E300`) for all-concrete `match` arm disagreement (ordering-invariant)
- `171_self_recursive_refinement_concrete_chain_e300.flx`
  - Expected: compile-time failure (`E300`) for typed mismatch using refined concrete self-recursive return chain
- `172_self_recursive_unresolved_guard_no_false_positive.flx`
  - Expected: compile-time failure (`E004`) baseline unresolved symbol only; no recursion-specific false-positive mismatch diagnostics

## A3 Pure-Context Matrix

| Context | Expected | Fixture |
|---|---|---|
| Typed pure (`fn f(...) -> T`) + `print` | Reject (`E400`) | `35_pure_context_typed_pure_rejects_io.flx` |
| Typed `with Time` + `print` | Reject (`E400`) | `36_pure_context_time_only_rejects_io.flx` |
| Unannotated callee (infers `IO`) called from typed `with Time` | Reject (`E400`) | `37_pure_context_unannotated_infers_io_then_rejects_time_caller.flx` |

## A4 Top-Level Policy Matrix

| Context | Expected | Fixture |
|---|---|---|
| Pure top-level only (no `main`) | Allow | `../27_top_level_pure_ok.flx` |
| Effectful top-level expression | Reject (`E413`, `E414`) | `38_top_level_effect_rejected.flx` |
| Effectful expression inside `fn main() with ...` | Allow | `../28_effect_inside_main_allowed.flx` |

## A1 Alias Edge Cases

| Context | Expected | Fixture |
|---|---|---|
| `let p = print; p(...)` in typed pure function | Allow | `../205_effect_alias_print_in_pure_function_ok.flx` |
| `let p = print; p(...)` in `with Time` function | Reject (`E400`) | `40_effect_alias_print_in_time_function.flx` |
| `let n = now_ms; n()` in `with IO` function | Reject (`E400`) | `41_effect_alias_now_ms_in_io_function.flx` |

## B Handle/Perform Matrix

| Context | Expected | Fixture |
|---|---|---|
| `perform` unknown operation | Reject (`E404`) | `21_perform_unknown_operation.flx` |
| `handle` unknown effect | Reject (`E405`) | `42_handle_unknown_effect.flx` |
| `handle` unknown operation arm | Reject (`E401`) | `17_handle_unknown_operation.flx` |
| `handle` missing operation arms | Reject (`E402`) | `18_handle_incomplete_operation_set.flx` |
| Root boundary with custom effect in `main` | Skip (VM/LLVM runtime gap) | `43_main_unhandled_custom_effect.flx` |
| Root boundary with explicit handle discharge | Allow | `../29_main_handles_custom_effect.flx` |

## C Effect-Polymorphism Matrix

| Context | Expected | Fixture |
|---|---|---|
| Nested HOF wrappers with pure callback | Allow | `../30_effect_poly_hof_nested_ok.flx` |
| Polymorphic callback + local custom handle discharge | Allow | `../31_effect_poly_partial_handle_ok.flx` |
| Mixed `IO`/`Time` row extension with polymorphic callback | Allow | `../32_effect_poly_mixed_io_time_ok.flx` |
| Nested HOF wrappers resolve `e` to `IO` in `with Time` caller | Reject (`E400`) | `44_effect_poly_hof_nested_missing_effect.flx` |
| Explicit row subtraction (`IO + Console - Console`) still requires `IO` | Reject (`E400`) | `45_effect_row_subtract_missing_io.flx` |

## D Entry-Point Policy Matrix

| Context | Expected | Fixture |
|---|---|---|
| Duplicate top-level `fn main` | Reject (`E410`) | `46_duplicate_main_function.flx` |
| `fn main` with parameters | Reject (`E411`) | `47_main_with_parameters.flx` |
| `fn main` with non-`Unit` return type | Reject (`E412`) | `48_main_invalid_return_type.flx` |
| Effectful top-level expression, no `main` | Reject (`E413`, `E414`) | `38_top_level_effect_rejected.flx` |
| Effectful top-level expression, valid `main` present | Reject (`E413` only) | `49_top_level_effect_with_existing_main.flx` |
| `fn main` with invalid signature and custom root effect | Reject (`E412`), no redundant `E406` | `50_invalid_main_signature_no_root_discharge_noise.flx` |
| Custom effect in valid `main` boundary | Skip (VM/LLVM runtime gap) | `43_main_unhandled_custom_effect.flx` |
| Strict mode without `main` | Reject (`E415`) | `29_strict_missing_main.flx` |

## E Strict Mode Matrix

| Context | Expected | Fixture |
|---|---|---|
| `--strict` missing `main` | Reject (`E415`) | `29_strict_missing_main.flx` |
| `public fn` missing parameter annotations | Reject (`E416`) | `51_strict_public_missing_param_annotation.flx` |
| `public fn` missing return annotation | Reject (`E417`) | `52_strict_public_missing_return_annotation.flx` |
| effectful `public fn` missing `with` annotation | Reject (`E418`) | `53_strict_public_effectful_missing_with.flx` |
| `Any` in strict annotations (param/return/nested) | Reject (`E423`) | `54_strict_any_param_rejected.flx`, `55_strict_any_return_rejected.flx`, `56_strict_any_nested_rejected.flx` |
| strict checks across run/test/bytecode | Same diagnostic (`E416`) | `57_strict_entry_path_parity.flx` |
| private/internal `fn` allowed in strict API checks | Allow | `../58_strict_private_unannotated_allowed.flx` |

## F Public API Boundary Matrix

| Context | Expected | Fixture |
|---|---|---|
| underscore prefix on `public fn` does not make it private | Reject (`E416`) | `58_strict_public_underscore_missing_annotation.flx` |
| effectful `public fn` missing `with` | Reject (`E400`) | `59_strict_module_public_effect_missing_with.flx` |
| strict `public fn` with underscore and full annotations | Allow | `../59_strict_underscore_public_still_checked.flx` |
| strict module `public fn` fully annotated | Allow | `../60_strict_module_public_checked.flx` |
| strict module private helper unannotated | Allow | `../61_strict_module_private_unannotated_allowed.flx` |

Note:
- Visibility is explicit (`public fn`).
- `_name` is style-only and has no strict/public semantics.

## G Backend Parity Matrix (Curated)

| Category | Context | Fixture |
|---|---|---|
| A | direct effect rejection in typed/time contexts | `35_pure_context_typed_pure_rejects_io.flx`, `36_pure_context_time_only_rejects_io.flx`, `37_pure_context_unannotated_infers_io_then_rejects_time_caller.flx` |
| A | module-qualified effect propagation | `33_module_qualified_effect_propagation_missing.flx` |
| B | handle/perform static failures | `17_handle_unknown_operation.flx`, `18_handle_incomplete_operation_set.flx`, `42_handle_unknown_effect.flx` |
| B | handle discharge pass | `../22_handle_discharges_effect.flx` |
| C | effect polymorphism pass/fail | `../30_effect_poly_hof_nested_ok.flx`, `44_effect_poly_hof_nested_missing_effect.flx`, `45_effect_row_subtract_missing_io.flx` |
| D | entry policy pass/fail | `38_top_level_effect_rejected.flx`, `43_main_unhandled_custom_effect.flx`, `../29_main_handles_custom_effect.flx` |
| E/F | strict/public boundary pass/fail | `29_strict_missing_main.flx`, `57_strict_entry_path_parity.flx`, `58_strict_public_underscore_missing_annotation.flx`, `../61_strict_module_private_unannotated_allowed.flx` |

Parity rule:
- VM and JIT must match on diagnostic tuple: `error code + title + primary label`.

Run parity suite:

```bash
cargo test --all --all-features purity_vm_jit_parity_snapshots
INSTA_UPDATE=always cargo test --all --all-features purity_vm_jit_parity_snapshots
```

## Run

```bash
cargo run -- --no-cache examples/type_system/failing/01_compile_type_mismatch.flx
cargo run -- --no-cache examples/type_system/failing/02_runtime_boundary_arg_violation.flx
cargo run -- --no-cache examples/type_system/failing/03_runtime_return_violation.flx
cargo run -- --no-cache examples/type_system/failing/04_compile_float_string_arg.flx
cargo run -- --no-cache examples/type_system/failing/05_runtime_float_string_arg_via_any.flx
cargo run -- --no-cache examples/type_system/failing/06_runtime_float_string_return.flx
cargo run -- --no-cache examples/type_system/failing/07_typed_let_float_into_int.flx
cargo run -- --no-cache examples/type_system/failing/08_compile_identifier_type_mismatch.flx
cargo run -- --no-cache examples/type_system/failing/09_compile_typed_call_return_mismatch.flx
cargo run -- --no-cache examples/type_system/failing/15_effect_missing_from_caller.flx
cargo run -- --no-cache examples/type_system/failing/16_inferred_effect_missing_on_typed_caller.flx
cargo run -- --no-cache examples/type_system/failing/17_handle_unknown_operation.flx
cargo run -- --no-cache examples/type_system/failing/18_handle_incomplete_operation_set.flx
cargo run -- --no-cache examples/type_system/failing/19_effect_polymorphism_missing_effect.flx
cargo run -- --no-cache examples/type_system/failing/20_direct_builtin_missing_effect.flx
cargo run -- --no-cache examples/type_system/failing/21_perform_unknown_operation.flx
cargo run -- --no-cache examples/type_system/failing/22_effect_polymorphism_chain_missing_effect.flx
cargo run -- --no-cache examples/type_system/failing/23_generic_call_return_mismatch.flx
cargo run -- --no-cache examples/type_system/failing/24_adt_guarded_non_exhaustive.flx
cargo run -- --no-cache examples/type_system/failing/25_adt_mixed_constructors_in_match.flx
cargo run -- --no-cache examples/type_system/failing/26_adt_match_constructor_arity_mismatch.flx
cargo run -- --no-cache examples/type_system/failing/27_adt_wildcard_guard_not_catchall.flx
cargo run -- --no-cache examples/type_system/failing/28_adt_nested_guard_non_exhaustive.flx
cargo run -- --no-cache --strict examples/type_system/failing/29_strict_missing_main.flx
cargo run -- --no-cache --strict examples/type_system/failing/30_strict_public_unannotated_effectful.flx
cargo run -- --no-cache examples/type_system/failing/31_direct_time_builtin_missing_effect.flx
cargo run -- --no-cache examples/type_system/failing/32_direct_time_hof_missing_effect.flx
cargo run -- --no-cache --root examples/type_system examples/type_system/failing/33_module_qualified_effect_propagation_missing.flx
cargo run -- --no-cache examples/type_system/failing/34_generic_effect_propagation_missing.flx
cargo run -- --no-cache examples/type_system/failing/35_pure_context_typed_pure_rejects_io.flx
cargo run -- --no-cache examples/type_system/failing/36_pure_context_time_only_rejects_io.flx
cargo run -- --no-cache examples/type_system/failing/37_pure_context_unannotated_infers_io_then_rejects_time_caller.flx
cargo run -- --no-cache examples/type_system/failing/38_top_level_effect_rejected.flx
cargo run -- --no-cache examples/type_system/failing/40_effect_alias_print_in_time_function.flx
cargo run -- --no-cache examples/type_system/failing/41_effect_alias_now_ms_in_io_function.flx

# Boundary-soundness additions
cargo run -- --no-cache --strict examples/type_system/failing/61_strict_generic_unresolved_boundary.flx
cargo run -- --no-cache --root examples/type_system examples/type_system/failing/62_list_boundary_runtime_violation.flx
cargo run -- --no-cache --root examples/type_system examples/type_system/failing/63_either_boundary_runtime_violation.flx
cargo run -- --no-cache examples/type_system/failing/64_hm_inferred_call_mismatch.flx
cargo run -- --no-cache examples/type_system/failing/65_adt_nested_constructor_non_exhaustive.flx
cargo run -- --no-cache --strict --root examples/type_system examples/type_system/failing/66_module_constructor_not_public_api.flx
cargo run -- --no-cache examples/type_system/failing/67_adt_multi_arity_nested_non_exhaustive.flx
cargo run -- --no-cache examples/type_system/failing/68_adt_nested_list_non_exhaustive.flx
cargo run -- --no-cache examples/type_system/failing/69_hm_typed_let_infix_compile_mismatch.flx
cargo run -- --no-cache examples/type_system/failing/70_hm_prefix_non_numeric_compile_mismatch.flx
cargo run -- --no-cache examples/type_system/failing/71_hm_if_known_type_compile_mismatch.flx
cargo run -- --no-cache examples/type_system/failing/72_hm_match_known_type_compile_mismatch.flx
cargo run -- --no-cache examples/type_system/failing/73_hm_index_non_int_compile_mismatch.flx
cargo run -- --no-cache examples/type_system/failing/74_hm_index_non_indexable_compile_mismatch.flx
cargo run -- --no-cache examples/type_system/failing/75_hm_if_non_bool_condition_compile_mismatch.flx
cargo run -- --no-cache examples/type_system/failing/76_hm_match_guard_non_bool_compile_mismatch.flx
cargo run -- --no-cache examples/type_system/failing/77_hm_logical_non_bool_compile_mismatch.flx
cargo run -- --no-cache examples/type_system/failing/78_hm_inline_call_no_runtime_fallback.flx
cargo run -- --no-cache --root examples/type_system examples/type_system/failing/79_hm_module_generic_call_mismatch.flx
cargo run -- --no-cache examples/type_system/failing/80_type_adt_constructor_arity_mismatch.flx
cargo run -- --no-cache examples/type_system/failing/81_match_bool_missing_false.flx
cargo run -- --no-cache examples/type_system/failing/82_match_list_missing_empty.flx
cargo run -- --no-cache examples/type_system/failing/83_match_guarded_wildcard_only_non_exhaustive.flx
cargo run -- --no-cache examples/type_system/failing/84_match_tuple_gap_no_fallback.flx
cargo run -- --no-cache examples/type_system/failing/88_effect_op_signature_argument_mismatch.flx
cargo run -- --no-cache examples/type_system/failing/89_adt_generic_constructor_hm_mismatch.flx
cargo run -- --no-cache --strict --root examples/type_system examples/type_system/failing/90_adt_module_constructor_alias_not_exported.flx
cargo run -- --no-cache examples/type_system/failing/91_adt_nested_pattern_binding_type_mismatch.flx
cargo run -- --no-cache examples/type_system/failing/92_hm_if_branch_contextual_mismatch.flx
cargo run -- --no-cache examples/type_system/failing/93_hm_match_arm_contextual_mismatch.flx
cargo run -- --no-cache examples/type_system/failing/94_wrong_argument_count_too_many.flx
cargo run -- --no-cache examples/type_system/failing/95_wrong_argument_count_too_few.flx
cargo run -- --no-cache examples/type_system/failing/96_hm_fun_param_mismatch_contextual.flx
cargo run -- --no-cache examples/type_system/failing/97_hm_fun_return_mismatch_contextual.flx
cargo run -- --no-cache examples/type_system/failing/98_hm_fun_arity_mismatch_contextual.flx
cargo run -- --no-cache examples/type_system/failing/99_multi_error_continuation.flx
cargo run -- --no-cache examples/type_system/failing/100_unclosed_string_recovery.flx
cargo run -- --no-cache examples/type_system/failing/101_missing_colon_let_annotation.flx
cargo run -- --no-cache examples/type_system/failing/102_missing_colon_function_param.flx
cargo run -- --no-cache examples/type_system/failing/103_missing_colon_lambda_param.flx
cargo run -- --no-cache examples/type_system/failing/104_missing_colon_effect_op.flx
cargo run -- --no-cache examples/type_system/failing/105_unknown_effect_suggestion.flx
cargo run -- --no-cache examples/type_system/failing/106_let_annotation_int_string.flx
cargo run -- --no-cache examples/type_system/failing/107_let_annotation_bool_int.flx
cargo run -- --no-cache examples/type_system/failing/108_fun_return_string_vs_int.flx
cargo run -- --no-cache examples/type_system/failing/109_fun_return_bool_vs_unit.flx
cargo run -- --no-cache examples/type_system/failing/110_call_arg_named_fn.flx
cargo run -- --no-cache examples/type_system/failing/111_call_arg_anonymous_fn.flx
cargo run -- --no-cache examples/type_system/failing/112_keyword_alias_def.flx
cargo run -- --no-cache examples/type_system/failing/113_keyword_alias_var.flx
cargo run -- --no-cache examples/type_system/failing/114_keyword_alias_case.flx
cargo run -- --no-cache examples/type_system/failing/115_keyword_alias_elif.flx
cargo run -- --no-cache examples/type_system/failing/116_keyword_alias_end.flx
cargo run -- --no-cache examples/type_system/failing/117_if_missing_brace.flx
cargo run -- --no-cache examples/type_system/failing/118_let_missing_eq.flx
cargo run -- --no-cache examples/type_system/failing/119_fn_missing_parens.flx
cargo run -- --no-cache examples/type_system/failing/120_match_pipe_separator.flx
cargo run -- --no-cache examples/type_system/failing/121_match_fat_arrow.flx
cargo run -- --no-cache examples/type_system/failing/142_match_bool_missing_true.flx
cargo run -- --no-cache examples/type_system/failing/143_match_bool_missing_false.flx
cargo run -- --no-cache examples/type_system/failing/144_guarded_wildcard_only_non_exhaustive_targeted.flx
cargo run -- --no-cache examples/type_system/failing/146_constructor_pattern_arity_some_too_many.flx
cargo run -- --no-cache examples/type_system/failing/147_constructor_pattern_arity_none_too_many.flx
cargo run -- --no-cache examples/type_system/failing/148_constructor_pattern_arity_left_too_many.flx
cargo run -- --no-cache --strict --root examples/type_system examples/type_system/failing/149_cross_module_constructor_access_strict.flx
cargo run -- --no-cache --root examples/type_system examples/type_system/failing/150_cross_module_constructor_access_nonstrict_warning.flx
cargo run -- --no-cache examples/type_system/failing/42_handle_unknown_effect.flx
cargo run -- --no-cache examples/type_system/failing/43_main_unhandled_custom_effect.flx
cargo run -- --no-cache examples/type_system/failing/44_effect_poly_hof_nested_missing_effect.flx
cargo run -- --no-cache examples/type_system/failing/45_effect_row_subtract_missing_io.flx
cargo run -- --no-cache examples/type_system/failing/46_duplicate_main_function.flx
cargo run -- --no-cache examples/type_system/failing/47_main_with_parameters.flx
cargo run -- --no-cache examples/type_system/failing/48_main_invalid_return_type.flx
cargo run -- --no-cache examples/type_system/failing/49_top_level_effect_with_existing_main.flx
cargo run -- --no-cache examples/type_system/failing/50_invalid_main_signature_no_root_discharge_noise.flx
cargo run -- --no-cache --strict examples/type_system/failing/51_strict_public_missing_param_annotation.flx
cargo run -- --no-cache --strict examples/type_system/failing/52_strict_public_missing_return_annotation.flx
cargo run -- --no-cache --strict examples/type_system/failing/53_strict_public_effectful_missing_with.flx
cargo run -- --no-cache --strict examples/type_system/failing/54_strict_any_param_rejected.flx
cargo run -- --no-cache --strict examples/type_system/failing/55_strict_any_return_rejected.flx
cargo run -- --no-cache --strict examples/type_system/failing/56_strict_any_nested_rejected.flx
cargo run -- --no-cache --strict examples/type_system/failing/57_strict_entry_path_parity.flx
cargo run -- --no-cache --strict examples/type_system/failing/58_strict_public_underscore_missing_annotation.flx
cargo run -- --no-cache --strict examples/type_system/failing/59_strict_module_public_effect_missing_with.flx
```

JIT (compile-time failure examples):

```bash
cargo run --features jit -- --no-cache examples/type_system/failing/01_compile_type_mismatch.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/07_typed_let_float_into_int.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/08_compile_identifier_type_mismatch.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/09_compile_typed_call_return_mismatch.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/15_effect_missing_from_caller.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/16_inferred_effect_missing_on_typed_caller.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/17_handle_unknown_operation.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/18_handle_incomplete_operation_set.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/19_effect_polymorphism_missing_effect.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/20_direct_builtin_missing_effect.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/21_perform_unknown_operation.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/22_effect_polymorphism_chain_missing_effect.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/23_generic_call_return_mismatch.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/24_adt_guarded_non_exhaustive.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/25_adt_mixed_constructors_in_match.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/26_adt_match_constructor_arity_mismatch.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/27_adt_wildcard_guard_not_catchall.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/28_adt_nested_guard_non_exhaustive.flx --jit
cargo run --features jit -- --no-cache --strict examples/type_system/failing/29_strict_missing_main.flx --jit
cargo run --features jit -- --no-cache --strict examples/type_system/failing/30_strict_public_unannotated_effectful.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/31_direct_time_builtin_missing_effect.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/32_direct_time_hof_missing_effect.flx --jit
cargo run --features jit -- --no-cache --root examples/type_system examples/type_system/failing/33_module_qualified_effect_propagation_missing.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/34_generic_effect_propagation_missing.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/35_pure_context_typed_pure_rejects_io.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/36_pure_context_time_only_rejects_io.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/37_pure_context_unannotated_infers_io_then_rejects_time_caller.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/38_top_level_effect_rejected.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/40_effect_alias_print_in_time_function.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/41_effect_alias_now_ms_in_io_function.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/42_handle_unknown_effect.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/43_main_unhandled_custom_effect.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/44_effect_poly_hof_nested_missing_effect.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/45_effect_row_subtract_missing_io.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/46_duplicate_main_function.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/47_main_with_parameters.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/48_main_invalid_return_type.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/49_top_level_effect_with_existing_main.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/50_invalid_main_signature_no_root_discharge_noise.flx --jit
cargo run --features jit -- --no-cache --strict examples/type_system/failing/51_strict_public_missing_param_annotation.flx --jit
cargo run --features jit -- --no-cache --strict examples/type_system/failing/52_strict_public_missing_return_annotation.flx --jit
cargo run --features jit -- --no-cache --strict examples/type_system/failing/53_strict_public_effectful_missing_with.flx --jit
cargo run --features jit -- --no-cache --strict examples/type_system/failing/54_strict_any_param_rejected.flx --jit
cargo run --features jit -- --no-cache --strict examples/type_system/failing/55_strict_any_return_rejected.flx --jit
cargo run --features jit -- --no-cache --strict examples/type_system/failing/56_strict_any_nested_rejected.flx --jit
cargo run --features jit -- --no-cache --strict examples/type_system/failing/57_strict_entry_path_parity.flx --jit
cargo run --features jit -- --no-cache --strict examples/type_system/failing/58_strict_public_underscore_missing_annotation.flx --jit
cargo run --features jit -- --no-cache --strict examples/type_system/failing/59_strict_module_public_effect_missing_with.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/64_hm_inferred_call_mismatch.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/65_adt_nested_constructor_non_exhaustive.flx --jit
cargo run --features jit -- --no-cache --strict --root examples/type_system examples/type_system/failing/66_module_constructor_not_public_api.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/67_adt_multi_arity_nested_non_exhaustive.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/68_adt_nested_list_non_exhaustive.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/69_hm_typed_let_infix_compile_mismatch.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/70_hm_prefix_non_numeric_compile_mismatch.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/71_hm_if_known_type_compile_mismatch.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/72_hm_match_known_type_compile_mismatch.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/73_hm_index_non_int_compile_mismatch.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/74_hm_index_non_indexable_compile_mismatch.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/75_hm_if_non_bool_condition_compile_mismatch.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/76_hm_match_guard_non_bool_compile_mismatch.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/77_hm_logical_non_bool_compile_mismatch.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/78_hm_inline_call_no_runtime_fallback.flx --jit
cargo run --features jit -- --no-cache --root examples/type_system examples/type_system/failing/79_hm_module_generic_call_mismatch.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/80_type_adt_constructor_arity_mismatch.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/81_match_bool_missing_false.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/82_match_list_missing_empty.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/83_match_guarded_wildcard_only_non_exhaustive.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/84_match_tuple_gap_no_fallback.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/88_effect_op_signature_argument_mismatch.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/89_adt_generic_constructor_hm_mismatch.flx --jit
cargo run --features jit -- --no-cache --strict --root examples/type_system examples/type_system/failing/90_adt_module_constructor_alias_not_exported.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/91_adt_nested_pattern_binding_type_mismatch.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/92_hm_if_branch_contextual_mismatch.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/93_hm_match_arm_contextual_mismatch.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/94_wrong_argument_count_too_many.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/95_wrong_argument_count_too_few.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/96_hm_fun_param_mismatch_contextual.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/97_hm_fun_return_mismatch_contextual.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/98_hm_fun_arity_mismatch_contextual.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/99_multi_error_continuation.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/100_unclosed_string_recovery.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/101_missing_colon_let_annotation.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/102_missing_colon_function_param.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/103_missing_colon_lambda_param.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/104_missing_colon_effect_op.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/105_unknown_effect_suggestion.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/106_let_annotation_int_string.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/107_let_annotation_bool_int.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/108_fun_return_string_vs_int.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/109_fun_return_bool_vs_unit.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/110_call_arg_named_fn.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/111_call_arg_anonymous_fn.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/112_keyword_alias_def.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/113_keyword_alias_var.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/114_keyword_alias_case.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/115_keyword_alias_elif.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/116_keyword_alias_end.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/117_if_missing_brace.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/118_let_missing_eq.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/119_fn_missing_parens.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/120_match_pipe_separator.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/121_match_fat_arrow.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/142_match_bool_missing_true.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/143_match_bool_missing_false.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/144_guarded_wildcard_only_non_exhaustive_targeted.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/146_constructor_pattern_arity_some_too_many.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/147_constructor_pattern_arity_none_too_many.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/148_constructor_pattern_arity_left_too_many.flx --jit
cargo run --features jit -- --no-cache --strict --root examples/type_system examples/type_system/failing/149_cross_module_constructor_access_strict.flx --jit
cargo run --features jit -- --no-cache --root examples/type_system examples/type_system/failing/150_cross_module_constructor_access_nonstrict_warning.flx --jit
```
- `173_perform_missing_dot.flx`
  - Expected: parser diagnostic (`E034`) with contextual `perform` structure message (missing `.` between effect and operation)
- `174_handle_missing_lbrace.flx`
  - Expected: parser diagnostic (`E034`) with contextual `handle` message (missing `{` before handler arms)
- `175_handle_arm_missing_arrow.flx`
  - Expected: parser diagnostic (`E034`) with contextual handle-arm message (missing `->`)
- `176_match_missing_open_brace.flx`
  - Expected: parser diagnostic (`E034`) with contextual `match` message (missing `{`)
- `177_module_missing_open_brace.flx`
  - Expected: parser diagnostic (`E034`) with contextual `module` message (missing `{`)
- `178_import_except_missing_open_bracket.flx`
  - Expected: parser diagnostic (`E034`) with contextual import `except` list message (missing `[... ]`)
- `179_data_missing_open_brace.flx`
  - Expected: parser diagnostic (`E034`) with contextual `data` declaration message (missing `{`)
- `180_type_adt_missing_assign.flx`
  - Expected: parser diagnostic (`E034`) with contextual `type` ADT-sugar message (missing `=`)
- `181_effect_missing_colon.flx`
  - Expected: parser diagnostic (`E034`) with contextual effect operation signature message (missing `:`)
- `182_list_comprehension_missing_left_arrow.flx`
  - Expected: parser diagnostic (`E034`) with contextual list-comprehension generator message (missing generator identifier)
- `183_hash_missing_colon.flx`
  - Expected: parser diagnostic (`E034`) with contextual hash key/value separator message (missing `:`)
- `184_type_expr_missing_close_paren.flx`
  - Expected: parser diagnostic (`E034`) with contextual type-expression closing delimiter message (missing `)`) 

Policy note for parser-context fixtures (`173..184`):
- These fixtures intentionally lock contextual parser diagnostics (`E034`) after P5.
- When parser wording improves, snapshot transcript updates are expected and should be accepted as the new baseline.
- `examples_fixtures_snapshots` remains a mixed harness; unrelated churn must be explicitly attributed by path and owning task.

Strict-only focused commands (`154/155/156/162/168`):
- VM:
  - `cargo run -- --no-cache --strict --root examples/type_system examples/type_system/failing/154_unresolved_projection_strict_e425.flx`
  - `cargo run -- --no-cache --strict --root examples/type_system examples/type_system/failing/155_unresolved_member_access_strict_e425.flx`
  - `cargo run -- --no-cache --strict --root examples/type_system examples/type_system/failing/156_unresolved_call_arg_strict_e425.flx`
  - `cargo run -- --no-cache --strict --root examples/type_system examples/type_system/failing/162_tuple_destructure_unresolved_strict_e425.flx`
  - `cargo run -- --no-cache --strict --root examples/type_system examples/type_system/failing/168_tuple_destructure_unresolved_guard_strict_e425.flx`
- JIT:
  - `cargo run --features jit -- --no-cache --strict --root examples/type_system examples/type_system/failing/154_unresolved_projection_strict_e425.flx --jit`
  - `cargo run --features jit -- --no-cache --strict --root examples/type_system examples/type_system/failing/155_unresolved_member_access_strict_e425.flx --jit`
  - `cargo run --features jit -- --no-cache --strict --root examples/type_system examples/type_system/failing/156_unresolved_call_arg_strict_e425.flx --jit`
  - `cargo run --features jit -- --no-cache --strict --root examples/type_system examples/type_system/failing/162_tuple_destructure_unresolved_strict_e425.flx --jit`
  - `cargo run --features jit -- --no-cache --strict --root examples/type_system examples/type_system/failing/168_tuple_destructure_unresolved_guard_strict_e425.flx --jit`

Runtime E1004 focused commands (`185..188`):
- VM:
  - `cargo run -- --no-cache --root examples/type_system examples/type_system/failing/185_runtime_boundary_arg_e1004.flx`
  - `cargo run -- --no-cache --root examples/type_system examples/type_system/failing/186_runtime_boundary_return_e1004.flx`
  - `cargo run -- --no-cache --root examples/type_system examples/type_system/failing/187_runtime_list_boundary_e1004.flx`
  - `cargo run -- --no-cache --root examples/type_system examples/type_system/failing/188_runtime_either_boundary_e1004.flx`
- JIT:
  - `cargo run --features jit -- --no-cache --root examples/type_system examples/type_system/failing/185_runtime_boundary_arg_e1004.flx --jit`
  - `cargo run --features jit -- --no-cache --root examples/type_system examples/type_system/failing/186_runtime_boundary_return_e1004.flx --jit`
  - `cargo run --features jit -- --no-cache --root examples/type_system examples/type_system/failing/187_runtime_list_boundary_e1004.flx --jit`
  - `cargo run --features jit -- --no-cache --root examples/type_system examples/type_system/failing/188_runtime_either_boundary_e1004.flx --jit`
  - Note: in `examples_fixtures_snapshots`, these may show `E018` due harness roots; canonical E1004 parity assertions live in `runtime_vm_jit_parity_release`.

0058 follow-up: contextual boundary/effect commands (`189..191`):
- VM:
  - `cargo run -- --no-cache --strict --root examples/type_system examples/type_system/failing/189_contextual_boundary_unresolved_strict_e425.flx`
  - `cargo run -- --no-cache --root examples/type_system examples/type_system/failing/190_contextual_boundary_arg_runtime_e1004.flx`
  - `cargo run -- --no-cache --root examples/type_system examples/type_system/failing/191_contextual_effect_missing_module_call_e400.flx`
- JIT:
  - `cargo run --features jit -- --no-cache --strict --root examples/type_system examples/type_system/failing/189_contextual_boundary_unresolved_strict_e425.flx --jit`
  - `cargo run --features jit -- --no-cache --root examples/type_system examples/type_system/failing/190_contextual_boundary_arg_runtime_e1004.flx --jit`
  - `cargo run --features jit -- --no-cache --root examples/type_system examples/type_system/failing/191_contextual_effect_missing_module_call_e400.flx --jit`
