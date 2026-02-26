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
)

for file in "${examples[@]}"; do
  echo "== $file =="
  if [[ "$file" == *"07_modules_and_hof.flx" ]]; then
    cargo run -- --root examples/type_system "$file"
  else
    cargo run -- "$file"
  fi
  echo
 done
