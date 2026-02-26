# Type System Examples (MVP Core)

Start with `00_START_HERE.md` for quick setup and run commands.

These examples target the current typed-syntax + contract-metadata milestone:

- typed `let` annotations
- typed function parameters/returns
- `with` effect clauses
- typed lambda parameters
- function-type annotations
- boundary-first contract style
- higher-order functions (including module-based usage)

## Files

- `01_typed_let_and_functions.flx` - basic typed bindings and function signatures
- `02_effect_signatures.flx` - `with IO` / `with Time` signatures
- `03_typed_lambdas_and_function_types.flx` - typed lambdas + function-type values
- `04_boundary_first_contract_style.flx` - boundary-first contract style in a script
- `05_mixed_typed_untyped.flx` - gradual typing style (typed + dynamic together)
- `06_higher_order_typed.flx` - typed higher-order functions over lists
- `07_modules_and_hof.flx` - module import + typed higher-order functions
- `08_effectful_hof_callbacks.flx` - effectful callback signatures (`with IO`)
- `09_static_propagation_success.flx` - typed `let` checks from identifier + typed-call returns
- `10_boundary_runtime_success.flx` - dynamic value crossing typed boundary (successful runtime check)
- `19_effect_call_propagation.flx` - effect propagation across typed function calls
- `20_effect_inference_unannotated.flx` - effect inference for unannotated functions
- `21_effect_polymorphism_with_e.flx` - effect polymorphism in higher-order functions (`with e`)
- `22_handle_discharges_effect.flx` - static `handle` coverage discharges required effects for wrapped calls
- `23_effect_polymorphism_chain_with_e.flx` - chained higher-order wrappers preserve `with e` effects
- `24_unit_return_effectful.flx` - `Unit` return in an effectful function (`with IO`)
- `25_none_return_compat.flx` - `None` return (currently accepted as unit-like)
- `26_any_boundary_success.flx` - `Any` flowing through dynamic code and printed safely
- `27_top_level_pure_ok.flx` - pure top-level declarations are allowed without `main`
- `28_effect_inside_main_allowed.flx` - effectful operations are allowed inside `fn main() with ...`
- `29_main_handles_custom_effect.flx` - custom effect is discharged by a handle in `main`
- `30_effect_poly_hof_nested_ok.flx` - nested higher-order wrappers preserve polymorphic `with e`
- `31_effect_poly_partial_handle_ok.flx` - polymorphic wrapper + custom effect discharged via `handle`
- `32_effect_poly_mixed_io_time_ok.flx` - mixed `IO`/`Time` context with polymorphic callback
- `33_effect_row_subtract_surface_syntax.flx` - explicit row syntax using subtraction (`with IO + Console - Console`)
- `58_strict_private_unannotated_allowed.flx` - strict mode allows private/internal `fn`; strict API checks target `public fn`
- `59_strict_underscore_public_still_checked.flx` - underscore naming is style-only; `public fn` remains strict API boundary
- `60_strict_module_public_checked.flx` - module-scoped `public fn` participates in strict API checks
- `61_strict_module_private_unannotated_allowed.flx` - module private helper `fn` remains internal and strict-allowed
- `62_real_program_module_pipeline.flx` - real program pipeline using modules, ADT classification, and typed HOF utilities
- `63_real_program_effect_rows_and_handle.flx` - custom effect flow with handler discharge and explicit row wrapper usage
- `64_real_program_strict_public_app.flx` - strict-friendly app that consumes `public fn` module APIs
- `65_real_program_primop_pipeline.flx` - primop-heavy typed pipeline through module wrappers
- `66_real_program_base_module_integration.flx` - base-interop module wrappers used in a typed app
- `67_real_program_domain_module_test.flx` - Flow.FTest unit tests for `TypeSystem.RealProgramDomain`
- `68_real_program_effects_module_test.flx` - Flow.FTest unit tests for `TypeSystem.RealProgramEffects`
- `69_real_program_public_api_test.flx` - Flow.FTest unit tests for strict/public API behavior in `TypeSystem.RealProgramPublicApi`
- `70_real_program_primops_module_test.flx` - Flow.FTest unit tests for primop wrapper module behavior
- `71_real_program_base_interop_module_test.flx` - Flow.FTest unit tests for base-interop wrapper module behavior

Module source used by `07`:
- `TypeSystem/Hof.flx`

Intentional failure fixtures:
- `failing/` - compile/runtime contract failure examples
  - includes entry-point policy coverage (`E410`-`E415`) for `main`/top-level purity boundary rules

## Run

```bash
cargo run -- examples/type_system/01_typed_let_and_functions.flx
cargo run -- examples/type_system/02_effect_signatures.flx
cargo run -- examples/type_system/03_typed_lambdas_and_function_types.flx
cargo run -- examples/type_system/04_boundary_first_contract_style.flx
cargo run -- examples/type_system/05_mixed_typed_untyped.flx
cargo run -- examples/type_system/06_higher_order_typed.flx
cargo run -- --root examples/type_system examples/type_system/07_modules_and_hof.flx
cargo run -- examples/type_system/08_effectful_hof_callbacks.flx
cargo run -- examples/type_system/09_static_propagation_success.flx
cargo run -- examples/type_system/10_boundary_runtime_success.flx
cargo run -- examples/type_system/19_effect_call_propagation.flx
cargo run -- examples/type_system/20_effect_inference_unannotated.flx
cargo run -- examples/type_system/21_effect_polymorphism_with_e.flx
cargo run -- examples/type_system/22_handle_discharges_effect.flx
cargo run -- examples/type_system/23_effect_polymorphism_chain_with_e.flx
cargo run -- examples/type_system/24_unit_return_effectful.flx
cargo run -- examples/type_system/25_none_return_compat.flx
cargo run -- examples/type_system/26_any_boundary_success.flx
cargo run -- examples/type_system/27_top_level_pure_ok.flx
cargo run -- examples/type_system/28_effect_inside_main_allowed.flx
cargo run -- examples/type_system/29_main_handles_custom_effect.flx
cargo run -- examples/type_system/30_effect_poly_hof_nested_ok.flx
cargo run -- examples/type_system/31_effect_poly_partial_handle_ok.flx
cargo run -- examples/type_system/32_effect_poly_mixed_io_time_ok.flx
cargo run -- examples/type_system/33_effect_row_subtract_surface_syntax.flx
cargo run -- --no-cache --strict examples/type_system/58_strict_private_unannotated_allowed.flx
cargo run -- --no-cache --strict examples/type_system/59_strict_underscore_public_still_checked.flx
cargo run -- --no-cache --strict --root examples/type_system examples/type_system/60_strict_module_public_checked.flx
cargo run -- --no-cache --strict --root examples/type_system examples/type_system/61_strict_module_private_unannotated_allowed.flx
cargo run -- --root examples/type_system examples/type_system/62_real_program_module_pipeline.flx
cargo run -- --root examples/type_system examples/type_system/63_real_program_effect_rows_and_handle.flx
cargo run -- --no-cache --strict --root examples/type_system examples/type_system/64_real_program_strict_public_app.flx
cargo run -- --root examples/type_system examples/type_system/65_real_program_primop_pipeline.flx
cargo run -- --root examples/type_system examples/type_system/66_real_program_base_module_integration.flx
```

JIT:

```bash
cargo run --features jit -- examples/type_system/06_higher_order_typed.flx --jit
cargo run --features jit -- --root examples/type_system examples/type_system/07_modules_and_hof.flx --jit
cargo run --features jit -- examples/type_system/09_static_propagation_success.flx --jit
cargo run --features jit -- examples/type_system/10_boundary_runtime_success.flx --jit
cargo run --features jit -- examples/type_system/19_effect_call_propagation.flx --jit
cargo run --features jit -- examples/type_system/20_effect_inference_unannotated.flx --jit
cargo run --features jit -- examples/type_system/21_effect_polymorphism_with_e.flx --jit
cargo run --features jit -- examples/type_system/22_handle_discharges_effect.flx --jit
cargo run --features jit -- examples/type_system/23_effect_polymorphism_chain_with_e.flx --jit
cargo run --features jit -- examples/type_system/24_unit_return_effectful.flx --jit
cargo run --features jit -- examples/type_system/25_none_return_compat.flx --jit
cargo run --features jit -- examples/type_system/26_any_boundary_success.flx --jit
cargo run --features jit -- examples/type_system/27_top_level_pure_ok.flx --jit
cargo run --features jit -- examples/type_system/28_effect_inside_main_allowed.flx --jit
cargo run --features jit -- examples/type_system/29_main_handles_custom_effect.flx --jit
cargo run --features jit -- examples/type_system/30_effect_poly_hof_nested_ok.flx --jit
cargo run --features jit -- examples/type_system/31_effect_poly_partial_handle_ok.flx --jit
cargo run --features jit -- examples/type_system/32_effect_poly_mixed_io_time_ok.flx --jit
cargo run --features jit -- examples/type_system/33_effect_row_subtract_surface_syntax.flx --jit
cargo run --features jit -- --no-cache --strict examples/type_system/58_strict_private_unannotated_allowed.flx --jit
cargo run --features jit -- --no-cache --strict examples/type_system/59_strict_underscore_public_still_checked.flx --jit
cargo run --features jit -- --no-cache --strict --root examples/type_system examples/type_system/60_strict_module_public_checked.flx --jit
cargo run --features jit -- --no-cache --strict --root examples/type_system examples/type_system/61_strict_module_private_unannotated_allowed.flx --jit
cargo run --features jit -- --root examples/type_system examples/type_system/62_real_program_module_pipeline.flx --jit
cargo run --features jit -- --root examples/type_system examples/type_system/63_real_program_effect_rows_and_handle.flx --jit
cargo run --features jit -- --no-cache --strict --root examples/type_system examples/type_system/64_real_program_strict_public_app.flx --jit
cargo run --features jit -- --root examples/type_system examples/type_system/65_real_program_primop_pipeline.flx --jit
cargo run --features jit -- --root examples/type_system examples/type_system/66_real_program_base_module_integration.flx --jit
```

## Flow.FTest Unit Tests

These unit fixtures target the real-program modules directly using `Flow.FTest` helpers. They run in test mode and require both library and module roots.

VM:

```bash
cargo run -- --test examples/type_system/67_real_program_domain_module_test.flx --root lib --root examples/type_system
cargo run -- --test examples/type_system/68_real_program_effects_module_test.flx --root lib --root examples/type_system
cargo run -- --test examples/type_system/69_real_program_public_api_test.flx --root lib --root examples/type_system
cargo run -- --test examples/type_system/70_real_program_primops_module_test.flx --root lib --root examples/type_system
cargo run -- --test examples/type_system/71_real_program_base_interop_module_test.flx --root lib --root examples/type_system
```

JIT:

```bash
cargo run --features jit -- --test examples/type_system/67_real_program_domain_module_test.flx --root lib --root examples/type_system --jit
cargo run --features jit -- --test examples/type_system/68_real_program_effects_module_test.flx --root lib --root examples/type_system --jit
cargo run --features jit -- --test examples/type_system/69_real_program_public_api_test.flx --root lib --root examples/type_system --jit
cargo run --features jit -- --test examples/type_system/70_real_program_primops_module_test.flx --root lib --root examples/type_system --jit
cargo run --features jit -- --test examples/type_system/71_real_program_base_interop_module_test.flx --root lib --root examples/type_system --jit
```

Strict check (public API test):

```bash
cargo run -- --test examples/type_system/69_real_program_public_api_test.flx --strict --root lib --root examples/type_system
cargo run --features jit -- --test examples/type_system/69_real_program_public_api_test.flx --strict --root lib --root examples/type_system --jit
```

Run everything:

```bash
bash examples/type_system/run_all_vm.sh
bash examples/type_system/run_all_jit.sh
```

## G Backend Parity (VM/JIT)

Run the curated purity-critical parity suite:

```bash
cargo test --all --all-features purity_vm_jit_parity_snapshots
```

Update parity snapshots intentionally:

```bash
INSTA_UPDATE=always cargo test --all --all-features purity_vm_jit_parity_snapshots
```
