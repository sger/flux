# Developer Guide: Using Hint Positioning in Flux

This guide explains how to use the hint positioning feature when implementing compiler diagnostics.

## When to Use Multi-Location Hints

Use hints with independent spans when:

1. **Referencing previous definitions**
   - Duplicate variable/function names
   - Name shadowing in same scope
   - Redeclaration errors

2. **Type mismatches**
   - Show where incompatible types were defined
   - Highlight conflicting type annotations

3. **Function/method calls**
   - Show function definition when argument count is wrong
   - Show parameter types when types don't match

4. **Import/module errors**
   - Point to where a module was imported
   - Show conflicting imports

5. **Scope-related errors**
   - Show where a variable is out of scope
   - Reference where it was actually defined

## API Reference

### 1. Creating Simple Text Hints (Backward Compatible)

```rust
Diagnostic::error("Something went wrong")
    .with_hint_text("Try this fix")
```

### 2. Creating Hints with Source Locations

```rust
// Option A: Using with_hint_at() convenience method
Diagnostic::error("Duplicate variable 'x'")
    .with_span(current_span)
    .with_hint_at("first defined here", previous_span)

// Option B: Using Hint::at() explicitly
Diagnostic::error("Duplicate variable 'x'")
    .with_span(current_span)
    .with_hint(Hint::at("first defined here", previous_span))
```

### 3. Creating Hints with Labels

Labels appear as `= note: your label` in the output:

```rust
// Option A: Using with_hint_labeled() convenience method
Diagnostic::error("Type mismatch")
    .with_span(operation_span)
    .with_hint_labeled(
        "Convert to the correct type",
        variable_span,
        "variable has type String"
    )

// Option B: Using Hint::labeled() explicitly
Diagnostic::error("Type mismatch")
    .with_span(operation_span)
    .with_hint(Hint::labeled(
        "Convert to the correct type",
        variable_span,
        "variable has type String"
    ))
```

### 4. Multiple Hints

You can add multiple hints - they render in order:

```rust
Diagnostic::error("Complex error")
    .with_span(error_span)
    .with_hint_text("General advice")           // Renders first (text-only)
    .with_hint_at("detail 1", span1)           // Renders second (with location)
    .with_hint_labeled("detail 2", span2, "label")  // Renders third (with location)
```

### 5. Building Hints Separately

For complex scenarios:

```rust
let hint1 = Hint::text("Simple hint");
let hint2 = Hint::at("More context", some_span);
let hint3 = Hint::labeled("Even more", other_span, "important");

Diagnostic::error("Complex error")
    .with_hint(hint1)
    .with_hint(hint2)
    .with_hint(hint3)
```

### 6. Adding Labels to Existing Hints

```rust
let hint = Hint::at("defined here", span)
    .with_label("first occurrence");

Diagnostic::error("Duplicate")
    .with_hint(hint)
```

## Output Format

### Text-Only Hint
```
Error: Something went wrong [E001]

  --> file.flx:10:5
   |
10 | problematic code
   |     ^^^^

Hint:
  Try this fix
```

### Hint with Location (no label)
```
Error: Something went wrong [E001]

  --> file.flx:10:5
   |
10 | problematic code
   |     ^^^^

   = note:
  --> file.flx:5:5
   |
 5 | related code
   |     ^^^^

Hint:
  Context about the hint
```

### Hint with Location and Label
```
Error: Duplicate variable 'x' [E001]

  --> file.flx:10:5
   |
10 | let x = 20;
   |     ^

   = note: first defined here
  --> file.flx:5:5
   |
 5 | let x = 10;
   |     ^

Hint:
  Use a different name
```

### Multiple Hints
```
Error: Type mismatch [E020]

  --> file.flx:8:10
   |
 8 | let z = x + y;
   |         ^^^^^

Hint:
  Convert types or change operation

   = note: 'x' has type String
  --> file.flx:3:5
   |
 3 | let x = "hello";
   |     ^

   = note: 'y' has type Int
  --> file.flx:4:5
   |
 4 | let y = 42;
   |     ^
```

## Best Practices

### DO ✅

1. **Use descriptive labels**
   ```rust
   .with_hint_labeled("", span, "first defined here")
   .with_hint_labeled("", span, "conflicting type annotation")
   .with_hint_labeled("", span, "function expects 2 parameters")
   ```

2. **Provide actionable hints**
   ```rust
   .with_hint_text("Use a different variable name")
   .with_hint_text("Remove the extra argument")
   .with_hint_text("Convert to Int using .parse()")
   ```

3. **Show relevant context**
   ```rust
   // Good: Show where both variables were defined
   diagnostic
       .with_hint_at("", var1_span).with_label("'x' defined as String")
       .with_hint_at("", var2_span).with_label("'y' defined as Int")
   ```

4. **Order hints logically**
   - Put general advice first (text-only hints)
   - Then show specific locations (hints with spans)

### DON'T ❌

1. **Don't overuse hints**
   ```rust
   // Bad: Too many hints
   diagnostic
       .with_hint_text("hint 1")
       .with_hint_text("hint 2")
       .with_hint_text("hint 3")
       .with_hint_text("hint 4")  // Overwhelming!
   ```

2. **Don't use vague labels**
   ```rust
   // Bad: Not helpful
   .with_hint_labeled("", span, "here")
   .with_hint_labeled("", span, "see this")

   // Good: Specific and clear
   .with_hint_labeled("", span, "function defined here with 2 parameters")
   .with_hint_labeled("", span, "first use of variable 'x'")
   ```

3. **Don't show the same location twice**
   ```rust
   // Bad: Redundant
   diagnostic
       .with_span(error_span)
       .with_hint_at("error location", error_span)  // Already shown!
   ```

4. **Don't use hints for error messages**
   ```rust
   // Bad: Error details in hints
   diagnostic
       .with_hint_text("Variable 'x' is not defined")  // Should be in .with_message()

   // Good: Clear separation
   diagnostic
       .with_message("Variable 'x' is not defined")
       .with_hint_text("Check for typos or import the module")
   ```

## Examples from the Codebase

### Duplicate Variable Detection
```rust
pub fn check_duplicate_variable(
    name: &str,
    current_span: Span,
    previous_span: Span,
    file: &str,
) -> Diagnostic {
    Diagnostic::error("Duplicate variable")
        .with_code("E001")
        .with_error_type(ErrorType::Compiler)
        .with_message(format!("Variable '{}' is already defined", name))
        .with_file(file)
        .with_span(current_span)
        .with_hint_text("Use a different name or remove the previous definition")
        .with_hint_labeled("", previous_span, "first defined here")
}
```

### Function Argument Mismatch
```rust
pub fn check_arg_count(
    function_name: &str,
    expected: usize,
    found: usize,
    call_span: Span,
    definition_span: Span,
    file: &str,
) -> Diagnostic {
    Diagnostic::error("Argument count mismatch")
        .with_code("E015")
        .with_error_type(ErrorType::Compiler)
        .with_message(format!(
            "Function '{}' expects {} arguments, but {} were provided",
            function_name, expected, found
        ))
        .with_file(file)
        .with_span(call_span)
        .with_hint_text(if found > expected {
            "Remove the extra arguments"
        } else {
            "Add the missing arguments"
        })
        .with_hint_labeled(
            "",
            definition_span,
            format!("function defined here with {} parameters", expected)
        )
}
```

### Type Mismatch
```rust
pub fn type_mismatch_binary_op(
    op: &str,
    left_type: &str,
    right_type: &str,
    op_span: Span,
    left_span: Span,
    right_span: Span,
    file: &str,
) -> Diagnostic {
    Diagnostic::error("Type mismatch")
        .with_code("E020")
        .with_error_type(ErrorType::Compiler)
        .with_message(format!(
            "Cannot apply operator '{}' to {} and {}",
            op, left_type, right_type
        ))
        .with_file(file)
        .with_span(op_span)
        .with_hint_text("Both operands must have compatible types")
        .with_hint_labeled("", left_span, format!("has type {}", left_type))
        .with_hint_labeled("", right_span, format!("has type {}", right_type))
}
```

## Testing

When writing tests for diagnostics with hints:

```rust
#[test]
fn test_diagnostic_with_hint() {
    let source = "let x = 1;\nlet x = 2;\n";
    let diagnostic = create_duplicate_var_diagnostic(...);
    let output = diagnostic.render(Some(source), None);

    // Test main error location
    assert!(output.contains("  --> file.flx:2:5"));

    // Test hint location
    assert!(output.contains("   = note: first defined here"));
    assert!(output.contains("  --> file.flx:1:5"));

    // Test hint text
    assert!(output.contains("Hint:\n  Use a different name"));
}
```

## Migration Guide

If you have existing code using the old text-only hints:

```rust
// Old way (still works!)
.with_hint("Try this fix")

// New way (if no location needed)
.with_hint_text("Try this fix")

// New way (if location needed)
.with_hint_at("Try this fix", relevant_span)
```

The old `.with_hint()` method now takes a `Hint` struct, but `.with_hint_text()` provides backward compatibility.

## Summary

| Method | Use Case | Example |
|--------|----------|---------|
| `.with_hint_text()` | Simple text hint | General advice |
| `.with_hint_at()` | Hint with location | Point to related code |
| `.with_hint_labeled()` | Hint with location + label | Show specific context |
| `.with_hint(Hint::...)` | Complex scenarios | Full control |

Use multi-location hints to create professional, helpful error messages that guide users to the root cause of their issues! 🎯
