# Intentional Failing Examples

These fixtures are expected to fail and are useful for validating diagnostics.

## Files

- `01_compile_type_mismatch.flx`
  - Expected: compile-time failure (`E055` type mismatch)
- `02_runtime_boundary_arg_violation.flx`
  - Expected: runtime failure (`E1004`) at typed boundary argument check
- `03_runtime_return_violation.flx`
  - Expected: runtime failure (`E1004`) at typed return boundary check
- `07_typed_let_float_into_int.flx`
  - Expected: compile-time failure (`E055`) on typed `let` initializer mismatch (`Int` annotated, `Float` assigned)

## Run

```bash
cargo run -- --no-cache examples/type_system/failing/01_compile_type_mismatch.flx
cargo run -- --no-cache examples/type_system/failing/02_runtime_boundary_arg_violation.flx
cargo run -- --no-cache examples/type_system/failing/03_runtime_return_violation.flx
cargo run -- --no-cache examples/type_system/failing/07_typed_let_float_into_int.flx
```

JIT (compile-time failure examples):

```bash
cargo run --features jit -- --no-cache examples/type_system/failing/01_compile_type_mismatch.flx --jit
cargo run --features jit -- --no-cache examples/type_system/failing/07_typed_let_float_into_int.flx --jit
```
