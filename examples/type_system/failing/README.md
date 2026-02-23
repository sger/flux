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
```

JIT (compile-time failure examples):

```bash
cargo run --features jit -- --no-cache examples/type_system/failing/01_compile_type_mismatch.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/07_typed_let_float_into_int.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/08_compile_identifier_type_mismatch.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/09_compile_typed_call_return_mismatch.flx --jit
```
