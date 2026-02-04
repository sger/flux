# Hint Span Demo Examples

These examples demonstrate the **independent hint positioning** feature in Flux's diagnostic system. When you compile these files with errors, you'll see multi-location error messages similar to Rust's compiler.

## Examples

### 1. `duplicate_variable.flx` - Duplicate Variable Error
**What it demonstrates:** When a variable is defined twice in the same scope, the error points to the duplicate definition, and a hint points back to where it was first defined.

**Expected output:**
```
Error: Duplicate variable 'x'

  --> duplicate_variable.flx:10:5
   |
10 | let x = 30;
   |     ^

   = note: first defined here
  --> duplicate_variable.flx:3:5
   |
 3 | let x = 10;
   |     ^

Hint:
  Use a different name or remove the previous definition
```

### 2. `function_arg_mismatch.flx` - Argument Count Mismatch
**What it demonstrates:** When calling a function with the wrong number of arguments, the error shows the call site, and a hint points back to the function definition with the correct parameter count.

**Expected output:**
```
Error: Argument count mismatch

  --> function_arg_mismatch.flx:17:15
   |
17 | let result3 = add(1, 2, 3);
   |               ^^^^^^^^^^^^

   = note: function defined here with 2 parameters
  --> function_arg_mismatch.flx:4:9
   |
 4 | fun add(a, b) {
   |         ^^^^^

Hint:
  Remove the extra argument
```

### 3. `type_mismatch.flx` - Type Mismatch Error
**What it demonstrates:** When trying to use incompatible types together, the error shows the problematic operation, and hints point to where each variable was defined with its type.

**Expected output:**
```
Error: Type mismatch

  --> type_mismatch.flx:14:15
   |
14 | let invalid = name + age;
   |               ^^^^^^^^^^

   = note: 'name' has type String
  --> type_mismatch.flx:4:5
   |
 4 | let name = "Alice";
   |     ^^^^

   = note: 'age' has type Int
  --> type_mismatch.flx:5:5
   |
 5 | let age = 25;
   |     ^^^

Hint:
  Cannot add String and Int types
```

### 4. `shadowing_ok.flx` - Valid Shadowing (No Errors)
**What it demonstrates:** Shows that variable shadowing in different scopes is allowed and doesn't produce errors. This helps users understand when duplicate names are OK.

**Expected:** Compiles successfully with no errors.

## Key Features Demonstrated

1. **Multi-location error reporting** - Errors can reference multiple locations in the source code
2. **Contextual hints** - Hints include source code snippets from different locations
3. **Clear labels** - Each hint location has a descriptive label (e.g., "first defined here", "function defined here")
4. **Better debugging** - Users can see all relevant code locations without searching through files

## Usage

To see these errors in action:

```bash
# Compile the files (they will error intentionally)
flux compile examples/hint_demos/duplicate_variable.flx
flux compile examples/hint_demos/function_arg_mismatch.flx
flux compile examples/hint_demos/type_mismatch.flx

# This one should compile successfully
flux compile examples/hint_demos/shadowing_ok.flx
```

## Implementation

This feature is implemented in:
- [`src/frontend/diagnostics/diagnostic.rs`](../../src/frontend/diagnostics/diagnostic.rs) - Core `Hint` struct and rendering logic
- [`src/frontend/diagnostics/mod.rs`](../../src/frontend/diagnostics/mod.rs) - Public API exports
- [`tests/diagnostic_span_render_tests.rs`](../../tests/diagnostic_span_render_tests.rs) - Comprehensive tests

## API Usage

For compiler developers, here's how to create multi-location errors:

```rust
use flux::frontend::diagnostics::{Diagnostic, Hint};
use flux::frontend::position::Span;

// Create an error with a hint pointing to another location
let diagnostic = Diagnostic::error("Duplicate variable")
    .with_code("E001")
    .with_span(current_definition_span)
    .with_hint_text("Use a different name")
    .with_hint_labeled(
        "Previous definition here",
        previous_definition_span,
        "first defined here"
    );
```
