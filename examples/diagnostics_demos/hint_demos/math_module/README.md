# Math Module - Multi-File Error Demo

This directory contains a module-based example demonstrating multi-file error reporting.

## Files

- **math.flx** - Library module with mathematical functions
- **main.flx** - Main file that imports and uses the math module

## Purpose

Demonstrates how errors can reference code across multiple files when using modules.

## The Error

The main.flx file calls `math.average(x, y, z)` with 3 arguments, but the function is defined in math.flx with only 2 parameters.

## How to Test

Once the compiler is updated to use multi-file hints for function signature errors:

```bash
cd examples/hint_demos/math_module
cargo run -- main.flx
```

## Expected Output

```
-- Compiler error: Function signature mismatch [E050]

Expected 2 arguments, found 3

  --> examples/hint_demos/math_module/main.flx:19:11
   |
19 | let avg = math.average(x, y, z);
   |           ^^^^^^^^^^^^^^^^^^^^^

   = note: function defined here
  --> examples/hint_demos/math_module/math.flx:15:5
   |
15 | fn average(num1, num2) {
   |     ^^^^^^^^^^^^^^^^^^^
```

## Implementation for Compiler Developers

When detecting a function call with wrong arity in an imported function:

```rust
// In the compiler's function call type checking
if actual_arg_count != expected_arg_count {
    let mut diag = Diagnostic::error("Function signature mismatch")
        .with_code("E050")
        .with_file(&call_site_file)
        .with_span(call_span)
        .with_message(format!(
            "Expected {} arguments, found {}",
            expected_arg_count, actual_arg_count
        ));

    // Add hint pointing to the function definition in another file
    if let Some((def_file, def_span)) = get_function_definition(&func_name) {
        diag = diag.with_hint(
            Hint::at("function defined here", def_span)
                .with_label("function defined here")
                .with_file(def_file)
        );
    }

    return Err(diag);
}
```

## Current Status

✅ Multi-file hint infrastructure is complete
⏳ Waiting for compiler integration with type checking
⏳ Requires function signature tracking across modules

Both files currently compile successfully since the full type system integration is pending.
