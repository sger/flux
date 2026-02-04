# Multi-File Support for Hints

## Overview

The multi-file support feature allows error messages to reference code across different files. This is essential for module imports and cross-file references.

## Feature Status

✅ **Infrastructure Complete** - The `Hint` struct now supports a `file` field
⏳ **Awaiting Module System** - Requires Flux module/import system to be fully implemented

## Example Files

- `multi_file_main.flx` - Main file that imports and uses functions
- `multi_file_lib.flx` - Library file with function definitions

## How It Works

When the compiler detects an error that involves code from multiple files, it can create hints that point to different files:

```rust
// In the compiler, when detecting a function signature mismatch:
let hint = Hint::at("Function defined with 2 parameters", def_span)
    .with_label("defined here")
    .with_file("multi_file_lib.flx");

Diagnostic::error("Function signature mismatch")
    .with_file("multi_file_main.flx")
    .with_span(call_span)
    .with_message("Expected 2 arguments, found 3")
    .with_hint(hint)
```

## Expected Error Output

When the module system is implemented and the error is triggered:

```
-- Compiler error: Function signature mismatch [E050]

Expected 2 arguments, found 3

  --> examples/hint_demos/multi_file_main.flx:25:1
   |
25 | calculate(x, y, z)
   | ^^^^^^^^^^^^^^^^^^

   = note: defined with 2 parameters
  --> examples/hint_demos/multi_file_lib.flx:8:15
   |
 8 | fun calculate(a, b) {
   |               ^^^^^^
```

## API Reference

### Hint with File

```rust
// Create a hint that points to a different file
let hint = Hint::at("message", span)
    .with_file("path/to/file.flx");

// Or with a label
let hint = Hint::labeled("message", span, "label")
    .with_file("path/to/file.flx");
```

### Diagnostic with Multiple Files

```rust
Diagnostic::error("Error in main file")
    .with_file("main.flx")          // Primary error location
    .with_span(error_span)
    .with_hint(
        Hint::at("Related code", span)
            .with_file("lib.flx")    // Hint points to different file
    )
```

## Use Cases

1. **Function Signature Mismatches**
   - Error in calling file
   - Hint shows definition in imported file

2. **Type Mismatches**
   - Error where type is used
   - Hint shows type definition in other file

3. **Variable References**
   - Error where variable is used incorrectly
   - Hint shows where it was imported from

4. **Module Exports**
   - Error about missing export
   - Hint shows what's actually exported

## Testing

Currently, these files compile successfully since imports aren't implemented. To see the multi-file error format, run:

```bash
cargo run --example demo_hint_rendering
```

And look at "Example 7: Multi-File Support" which demonstrates the rendering.

## For Compiler Developers

To integrate multi-file hints into your error:

1. Determine the primary error location (file + span)
2. Find related code in other files
3. Create hints with `.with_file()` pointing to those locations
4. The rendering will automatically show both files

Example integration:

```rust
// In module resolution or import checking
if let Some(definition) = find_definition_in_module(&name, &imported_file) {
    return Err(Diagnostic::error("Type mismatch")
        .with_file(current_file)
        .with_span(usage_span)
        .with_hint(
            Hint::at("Type defined here", definition.span)
                .with_label("original definition")
                .with_file(imported_file)
        ));
}
```
