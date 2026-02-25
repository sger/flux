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
  - Expected: compile-time failure (`E055`) for typed `let` mismatch through generic call return instantiation

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
```
