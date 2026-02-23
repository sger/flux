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
