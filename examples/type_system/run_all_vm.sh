#!/usr/bin/env bash
set -euo pipefail

examples=(
  "examples/type_system/01_typed_let_and_functions.flx"
  "examples/type_system/02_effect_signatures.flx"
  "examples/type_system/03_typed_lambdas_and_function_types.flx"
  "examples/type_system/04_boundary_first_contract_style.flx"
  "examples/type_system/05_mixed_typed_untyped.flx"
  "examples/type_system/06_higher_order_typed.flx"
  "examples/type_system/07_modules_and_hof.flx"
  "examples/type_system/08_effectful_hof_callbacks.flx"
  "examples/type_system/09_static_propagation_success.flx"
  "examples/type_system/10_boundary_runtime_success.flx"
  "examples/type_system/19_effect_call_propagation.flx"
  "examples/type_system/20_effect_inference_unannotated.flx"
  "examples/type_system/21_effect_polymorphism_with_e.flx"
  "examples/type_system/22_handle_discharges_effect.flx"
  "examples/type_system/23_effect_polymorphism_chain_with_e.flx"
  "examples/type_system/24_unit_return_effectful.flx"
  "examples/type_system/25_none_return_compat.flx"
  "examples/type_system/26_any_boundary_success.flx"
  "examples/type_system/27_top_level_pure_ok.flx"
  "examples/type_system/28_effect_inside_main_allowed.flx"
  "examples/type_system/29_main_handles_custom_effect.flx"
  "examples/type_system/72_list_boundary_runtime_check_ok.flx"
  "examples/type_system/73_either_boundary_runtime_check_ok.flx"
  "examples/type_system/74_generic_adt_module_ok.flx"
  "examples/type_system/75_hm_polymorphic_compose_ok.flx"
  "examples/type_system/76_adt_nested_exhaustive_ok.flx"
  "examples/type_system/77_adt_multi_arity_nested_exhaustive_ok.flx"
  "examples/type_system/78_hm_typed_let_infix_ok.flx"
  "examples/type_system/79_hm_prefix_numeric_ok.flx"
  "examples/type_system/80_hm_if_known_type_ok.flx"
  "examples/type_system/81_hm_match_known_type_ok.flx"
  "examples/type_system/82_hm_index_known_type_ok.flx"
  "examples/type_system/83_hm_bool_condition_and_guard_ok.flx"
  "examples/type_system/84_hm_logical_bool_ok.flx"
  "examples/type_system/85_hm_module_generic_call_ok.flx"
  "examples/type_system/88_match_bool_exhaustive_ok.flx"
  "examples/type_system/89_match_list_exhaustive_ok.flx"
  "examples/type_system/90_match_guarded_with_fallback_ok.flx"
  "examples/type_system/91_match_tuple_with_catchall_ok.flx"
)

for file in "${examples[@]}"; do
  echo "== $file =="
  if [[ "$file" == *"07_modules_and_hof.flx" || "$file" == *"74_generic_adt_module_ok.flx" || "$file" == *"85_hm_module_generic_call_ok.flx" ]]; then
    cargo run -- --root examples/type_system "$file"
  else
    cargo run -- "$file"
  fi
  echo
 done
