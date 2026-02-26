# Intentional Failing Examples

These fixtures are expected to fail and are useful for validating diagnostics.

## Files

- `01_compile_type_mismatch.flx`
  - Expected: compile-time failure (`E055` type mismatch)
- `02_runtime_boundary_arg_violation.flx`
  - Expected: runtime failure (`E1004`) at typed boundary argument check
- `03_runtime_return_violation.flx`
  - Expected: runtime failure (`E1004`) at typed return boundary check
- `04_compile_float_string_arg.flx`
  - Expected: compile-time failure (`E055`) type mismatch (`Float` expected, `String` passed)
- `05_runtime_float_string_arg_via_any.flx`
  - Expected: runtime failure (`E1004`) at typed boundary argument check (`Float` expected)
- `06_runtime_float_string_return.flx`
  - Expected: runtime failure (`E1004`) at typed return boundary check (`Float` expected)
- `07_typed_let_float_into_int.flx`
  - Expected: compile-time failure (`E055`) on typed `let` initializer mismatch (`Int` annotated, `Float` assigned)
- `08_compile_identifier_type_mismatch.flx`
  - Expected: compile-time failure (`E055`) on typed `let` from identifier (`Int` annotated, `String` value)
- `09_compile_typed_call_return_mismatch.flx`
  - Expected: compile-time failure (`E055`) on typed `let` from typed call return (`Int` annotated, `String` return)
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
  - Expected: compile-time failure (`E082`) when constructor pattern field count mismatches declaration arity
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
- `39_effect_alias_print_in_pure_function.flx`
  - Expected: compile-time failure (`E400`) when `print` is called via local alias in a typed pure function
- `40_effect_alias_print_in_time_function.flx`
  - Expected: compile-time failure (`E400`) when `print` is called via local alias in a `with Time` function
- `41_effect_alias_now_ms_in_io_function.flx`
  - Expected: compile-time failure (`E400`) when `now_ms` is called via local alias in a `with IO` function
- `42_handle_unknown_effect.flx`
  - Expected: compile-time failure (`E405`) when `handle` references an undeclared effect
- `43_main_unhandled_custom_effect.flx`
  - Expected: compile-time failure (`E406`) when `fn main` exits with undischarged custom effects
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
| `let p = print; p(...)` in typed pure function | Reject (`E400`) | `39_effect_alias_print_in_pure_function.flx` |
| `let p = print; p(...)` in `with Time` function | Reject (`E400`) | `40_effect_alias_print_in_time_function.flx` |
| `let n = now_ms; n()` in `with IO` function | Reject (`E400`) | `41_effect_alias_now_ms_in_io_function.flx` |

## B Handle/Perform Matrix

| Context | Expected | Fixture |
|---|---|---|
| `perform` unknown operation | Reject (`E404`) | `21_perform_unknown_operation.flx` |
| `handle` unknown effect | Reject (`E405`) | `42_handle_unknown_effect.flx` |
| `handle` unknown operation arm | Reject (`E401`) | `17_handle_unknown_operation.flx` |
| `handle` missing operation arms | Reject (`E402`) | `18_handle_incomplete_operation_set.flx` |
| Root boundary with undischarged custom effect in `main` | Reject (`E406`) | `43_main_unhandled_custom_effect.flx` |
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
| Custom effect escapes valid `main` boundary | Reject (`E406`) | `43_main_unhandled_custom_effect.flx` |
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
cargo run -- --no-cache examples/type_system/failing/39_effect_alias_print_in_pure_function.flx
cargo run -- --no-cache examples/type_system/failing/40_effect_alias_print_in_time_function.flx
cargo run -- --no-cache examples/type_system/failing/41_effect_alias_now_ms_in_io_function.flx
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
cargo run --features jit -- --no-cache examples/type_system/failing/39_effect_alias_print_in_pure_function.flx --jit
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
```
