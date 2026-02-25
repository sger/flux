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

Module source used by `07`:
- `TypeSystem/Hof.flx`

Intentional failure fixtures:
- `failing/` - compile/runtime contract failure examples

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
```

JIT:

```bash
cargo run --features jit -- examples/type_system/06_higher_order_typed.flx --jit
cargo run --features jit -- --root examples/type_system examples/type_system/07_modules_and_hof.flx --jit
cargo run --features jit -- examples/type_system/09_static_propagation_success.flx --jit
cargo run --features jit -- examples/type_system/10_boundary_runtime_success.flx --jit
cargo run --features jit -- examples/type_system/19_effect_call_propagation.flx --jit
```

Run everything:

```bash
bash examples/type_system/run_all_vm.sh
bash examples/type_system/run_all_jit.sh
```
