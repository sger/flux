# Error Catalog for Flux v0.0.2

This document defines all new error codes and diagnostic messages for features introduced in v0.0.2.

---

## Error Code Ranges

| Range | Category |
|-------|----------|
| E001-E099 | Lexer errors |
| E100-E199 | Parser errors |
| E200-E299 | Compiler errors |
| E300-E399 | Runtime errors |
| E400-E499 | Type errors |
| E500-E599 | Module errors |

---

## New Errors for v0.0.2

### Lexer Errors (E001-E099)

```toml
[errors.E010]
title = "INVALID OPERATOR"
message = "Unexpected character sequence `{chars}`."
hint = "Did you mean `{suggestion}`?"

[errors.E011]
title = "UNTERMINATED OPERATOR"
message = "Expected `>` after `|` for pipe operator."
hint = "Use `|>` for the pipe operator."
```

### Parser Errors (E100-E199)

```toml
[errors.E120]
title = "INVALID LAMBDA SYNTAX"
message = "Expected parameter list after `\\`."
hint = "Lambda syntax: \\x -> x * 2 or \\a, b -> a + b"

[errors.E121]
title = "MISSING LAMBDA ARROW"
message = "Expected `->` after lambda parameters."
hint = "Lambda syntax: \\{params} -> expression"

[errors.E122]
title = "EMPTY LAMBDA PARAMETERS"
message = "Lambda must have at least one parameter."
hint = "Use `\\_` for a lambda that ignores its argument."

[errors.E130]
title = "INVALID PIPE EXPRESSION"
message = "Right side of `|>` must be a function or function call."
hint = "Example: x |> f or x |> f(y)"

[errors.E131]
title = "PIPE TO NON-CALLABLE"
message = "Cannot pipe to `{expr}` - it's not callable."
hint = "The right side of |> must be a function name or call."

[errors.E140]
title = "INVALID EITHER CONSTRUCTOR"
message = "Expected expression after `{constructor}(`."
hint = "{constructor} requires a value: {constructor}(value)"

[errors.E141]
title = "UNCLOSED EITHER CONSTRUCTOR"
message = "Missing `)` after {constructor} value."
hint = "Add closing parenthesis: {constructor}(value)"
```

### Compiler Errors (E200-E299)

```toml
[errors.E220]
title = "SHORT-CIRCUIT OPERAND NOT BOOLEAN"
message = "Operand of `{op}` must be a boolean, got `{actual}`."
hint = "The `&&` and `||` operators require boolean operands."

[errors.E230]
title = "NOT A CONSTANT EXPRESSION"
message = "Module constant `{name}` must be a constant expression."
hint = "Only literals, basic operations, and references to earlier constants are allowed."

[errors.E231]
title = "CONSTANT FORWARD REFERENCE"
message = "`{name}` references `{referenced}` which is not yet defined."
hint = "Move `{referenced}` before `{name}`, or reorder the constants."

[errors.E232]
title = "CONSTANT CIRCULAR DEPENDENCY"
message = "Circular dependency detected: {cycle}."
hint = "Constants cannot reference each other in a cycle."

[errors.E233]
title = "INVALID CONSTANT OPERATION"
message = "Cannot apply `{op}` to `{left_type}` and `{right_type}` at compile time."
hint = "Module constants only support basic arithmetic and string concatenation."

[errors.E234]
title = "FUNCTION CALL IN CONSTANT"
message = "Cannot call function `{name}` in module constant."
hint = "Module constants are evaluated at compile time and cannot call functions."

[errors.E235]
title = "VARIABLE IN CONSTANT"
message = "Cannot reference variable `{name}` in module constant."
hint = "Module constants can only reference other module constants."

[errors.E240]
title = "LAMBDA IN INVALID CONTEXT"
message = "Lambda expression not allowed here."
hint = "Lambdas can be used in variable bindings, function arguments, and return statements."
```

### Runtime Errors (E300-E399)

```toml
[errors.E320]
title = "DIVISION BY ZERO"
message = "Cannot divide by zero."
hint = "Check the divisor before dividing: if b != 0 { a / b } else { ... }"

[errors.E321]
title = "MODULO BY ZERO"
message = "Cannot compute modulo with zero divisor."
hint = "Check the divisor before computing modulo."

[errors.E330]
title = "UNWRAP ON LEFT"
message = "Called `unwrap_right` on a Left value."
hint = "Use pattern matching to safely handle both cases: match result { Right(v) -> ... Left(e) -> ... }"

[errors.E331]
title = "UNWRAP ON RIGHT"
message = "Called `unwrap_left` on a Right value."
hint = "Use pattern matching to safely handle both cases."
```

### Type Errors (E400-E499)

```toml
[errors.E420]
title = "COMPARISON TYPE MISMATCH"
message = "Cannot compare `{left_type}` with `{right_type}` using `{op}`."
hint = "Comparison operators require operands of the same type."

[errors.E421]
title = "MODULO TYPE ERROR"
message = "Modulo operator `%` requires numeric operands, got `{left_type}` and `{right_type}`."
hint = "Use integers or floats with the % operator."

[errors.E430]
title = "PIPE TYPE MISMATCH"
message = "Function `{name}` expects `{expected}` but received `{actual}` from pipe."
hint = "The piped value becomes the first argument of the function."
```

### Module Errors (E500-E599)

```toml
[errors.E520]
title = "DUPLICATE MODULE CONSTANT"
message = "Module constant `{name}` is already defined."
hint = "Each constant in a module must have a unique name."

[errors.E521]
title = "CONSTANT SHADOWS FUNCTION"
message = "Module constant `{name}` shadows function of the same name."
hint = "Use different names for constants and functions."

[errors.E522]
title = "PRIVATE CONSTANT ACCESS"
message = "Cannot access private constant `{module}.{name}` from outside the module."
hint = "Constants starting with `_` are private to the module."
```

---

## Error Message Examples

### E120: Invalid Lambda Syntax

```
error[E120]: INVALID LAMBDA SYNTAX
  --> examples/bad.flx:5:10
   |
 5 |     let f = \ -> x * 2;
   |             ^^^^^^^^^^
   |
   = error: Expected parameter list after `\`.
   = hint: Lambda syntax: \x -> x * 2 or \a, b -> a + b
```

### E230: Not a Constant Expression

```
error[E230]: NOT A CONSTANT EXPRESSION
  --> src/flow/Math.flx:4:15
   |
 4 |     let VALUE = compute_value();
   |                 ^^^^^^^^^^^^^^^^
   |
   = error: Module constant `VALUE` must be a constant expression.
   = hint: Only literals, basic operations, and references to earlier constants are allowed.
```

### E231: Constant Forward Reference

```
error[E231]: CONSTANT FORWARD REFERENCE
  --> src/flow/Math.flx:2:15
   |
 2 |     let PI = TAU / 2;
   |              ^^^
 3 |     let TAU = 6.283185;
   |
   = error: `PI` references `TAU` which is not yet defined.
   = hint: Move `TAU` before `PI`, or reorder the constants.
```

### E130: Invalid Pipe Expression

```
error[E130]: INVALID PIPE EXPRESSION
  --> examples/bad.flx:3:20
   |
 3 |     let result = x |> 42;
   |                      ^^
   |
   = error: Right side of `|>` must be a function or function call.
   = hint: Example: x |> f or x |> f(y)
```

### E330: Unwrap on Left

```
error[E330]: UNWRAP ON LEFT
  --> examples/bad.flx:5:5
   |
 5 |     let value = result.unwrap_right();
   |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
   |
   = error: Called `unwrap_right` on a Left value.
   = hint: Use pattern matching to safely handle both cases:
           match result { Right(v) -> ... Left(e) -> ... }
```

---

## TOML Catalog File

Full catalog for v0.0.2 features:

```toml
# resources/error_catalog_v0.0.2.toml
# Error catalog for Flux v0.0.2 features

# ============================================================================
# LEXER ERRORS (E0xx)
# ============================================================================

[errors.E010]
title = "INVALID OPERATOR"
message = "Unexpected character sequence `{chars}`."
hint = "Did you mean `{suggestion}`?"

[errors.E011]
title = "UNTERMINATED OPERATOR"
message = "Expected `>` after `|` for pipe operator."
hint = "Use `|>` for the pipe operator."

# ============================================================================
# PARSER ERRORS (E1xx)
# ============================================================================

[errors.E120]
title = "INVALID LAMBDA SYNTAX"
message = "Expected parameter list after `\\`."
hint = "Lambda syntax: \\x -> x * 2 or \\a, b -> a + b"

[errors.E121]
title = "MISSING LAMBDA ARROW"
message = "Expected `->` after lambda parameters."
hint = "Lambda syntax: \\{params} -> expression"

[errors.E122]
title = "EMPTY LAMBDA PARAMETERS"
message = "Lambda must have at least one parameter."
hint = "Use `\\_` for a lambda that ignores its argument."

[errors.E130]
title = "INVALID PIPE EXPRESSION"
message = "Right side of `|>` must be a function or function call."
hint = "Example: x |> f or x |> f(y)"

[errors.E131]
title = "PIPE TO NON-CALLABLE"
message = "Cannot pipe to `{expr}` - it's not callable."
hint = "The right side of |> must be a function name or call."

[errors.E140]
title = "INVALID EITHER CONSTRUCTOR"
message = "Expected expression after `{constructor}(`."
hint = "{constructor} requires a value: {constructor}(value)"

[errors.E141]
title = "UNCLOSED EITHER CONSTRUCTOR"
message = "Missing `)` after {constructor} value."
hint = "Add closing parenthesis: {constructor}(value)"

# ============================================================================
# COMPILER ERRORS (E2xx)
# ============================================================================

[errors.E220]
title = "SHORT-CIRCUIT OPERAND NOT BOOLEAN"
message = "Operand of `{op}` must be a boolean, got `{actual}`."
hint = "The `&&` and `||` operators require boolean operands."

[errors.E230]
title = "NOT A CONSTANT EXPRESSION"
message = "Module constant `{name}` must be a constant expression."
hint = "Only literals, basic operations, and references to earlier constants are allowed."

[errors.E231]
title = "CONSTANT FORWARD REFERENCE"
message = "`{name}` references `{referenced}` which is not yet defined."
hint = "Move `{referenced}` before `{name}`, or reorder the constants."

[errors.E232]
title = "CONSTANT CIRCULAR DEPENDENCY"
message = "Circular dependency detected: {cycle}."
hint = "Constants cannot reference each other in a cycle."

[errors.E233]
title = "INVALID CONSTANT OPERATION"
message = "Cannot apply `{op}` to `{left_type}` and `{right_type}` at compile time."
hint = "Module constants only support basic arithmetic and string concatenation."

[errors.E234]
title = "FUNCTION CALL IN CONSTANT"
message = "Cannot call function `{name}` in module constant."
hint = "Module constants are evaluated at compile time and cannot call functions."

[errors.E235]
title = "VARIABLE IN CONSTANT"
message = "Cannot reference variable `{name}` in module constant."
hint = "Module constants can only reference other module constants."

[errors.E240]
title = "LAMBDA IN INVALID CONTEXT"
message = "Lambda expression not allowed here."
hint = "Lambdas can be used in variable bindings, function arguments, and return statements."

# ============================================================================
# RUNTIME ERRORS (E3xx)
# ============================================================================

[errors.E320]
title = "DIVISION BY ZERO"
message = "Cannot divide by zero."
hint = "Check the divisor before dividing: if b != 0 { a / b } else { ... }"

[errors.E321]
title = "MODULO BY ZERO"
message = "Cannot compute modulo with zero divisor."
hint = "Check the divisor before computing modulo."

[errors.E330]
title = "UNWRAP ON LEFT"
message = "Called `unwrap_right` on a Left value."
hint = "Use pattern matching to safely handle both cases: match result { Right(v) -> ... Left(e) -> ... }"

[errors.E331]
title = "UNWRAP ON RIGHT"
message = "Called `unwrap_left` on a Right value."
hint = "Use pattern matching to safely handle both cases."

# ============================================================================
# TYPE ERRORS (E4xx)
# ============================================================================

[errors.E420]
title = "COMPARISON TYPE MISMATCH"
message = "Cannot compare `{left_type}` with `{right_type}` using `{op}`."
hint = "Comparison operators require operands of the same type."

[errors.E421]
title = "MODULO TYPE ERROR"
message = "Modulo operator `%` requires numeric operands, got `{left_type}` and `{right_type}`."
hint = "Use integers or floats with the % operator."

[errors.E430]
title = "PIPE TYPE MISMATCH"
message = "Function `{name}` expects `{expected}` but received `{actual}` from pipe."
hint = "The piped value becomes the first argument of the function."

# ============================================================================
# MODULE ERRORS (E5xx)
# ============================================================================

[errors.E520]
title = "DUPLICATE MODULE CONSTANT"
message = "Module constant `{name}` is already defined."
hint = "Each constant in a module must have a unique name."

[errors.E521]
title = "CONSTANT SHADOWS FUNCTION"
message = "Module constant `{name}` shadows function of the same name."
hint = "Use different names for constants and functions."

[errors.E522]
title = "PRIVATE CONSTANT ACCESS"
message = "Cannot access private constant `{module}.{name}` from outside the module."
hint = "Constants starting with `_` are private to the module."
```

---

## Integration with Existing Catalog

The v0.0.2 errors should be merged into the main `error_catalog.toml`:

```toml
# resources/error_catalog.toml

# Existing v0.0.1 errors
[errors.E007]
title = "UNDEFINED VARIABLE"
message = "I can't find a value named `{name}`."
hint = "Define it first: let {name} = ...;"

[errors.E021]
title = "PRIVATE MEMBER"
message = "Cannot access private member `{member}`."
hint = "Private members can only be accessed within the same module."

# ... existing errors ...

# v0.0.2 errors (add below)
# Include all errors from this document
```

---

## Error Code Summary

| Code | Feature | Description |
|------|---------|-------------|
| E010 | Lexer | Invalid operator character |
| E011 | Lexer | Unterminated pipe operator |
| E120 | Parser | Invalid lambda syntax |
| E121 | Parser | Missing lambda arrow |
| E122 | Parser | Empty lambda parameters |
| E130 | Parser | Invalid pipe expression |
| E131 | Parser | Pipe to non-callable |
| E140 | Parser | Invalid Either constructor |
| E141 | Parser | Unclosed Either constructor |
| E220 | Compiler | Short-circuit non-boolean |
| E230 | Compiler | Not a constant expression |
| E231 | Compiler | Constant forward reference |
| E232 | Compiler | Constant circular dependency |
| E233 | Compiler | Invalid constant operation |
| E234 | Compiler | Function call in constant |
| E235 | Compiler | Variable in constant |
| E240 | Compiler | Lambda in invalid context |
| E320 | Runtime | Division by zero |
| E321 | Runtime | Modulo by zero |
| E330 | Runtime | Unwrap on Left |
| E331 | Runtime | Unwrap on Right |
| E420 | Type | Comparison type mismatch |
| E421 | Type | Modulo type error |
| E430 | Type | Pipe type mismatch |
| E520 | Module | Duplicate module constant |
| E521 | Module | Constant shadows function |
| E522 | Module | Private constant access |

---

## Testing Strategy

Each error should have a corresponding test:

```rust
#[test]
fn test_e230_not_constant_expression() {
    let code = r#"
        module Test {
            let X = compute();
        }
    "#;
    let result = compile(code);
    assert!(result.is_err());
    assert!(result.unwrap_err().code == "E230");
}

#[test]
fn test_e231_constant_forward_reference() {
    let code = r#"
        module Test {
            let A = B + 1;
            let B = 10;
        }
    "#;
    let result = compile(code);
    assert!(result.is_err());
    assert!(result.unwrap_err().code == "E231");
}

#[test]
fn test_e120_invalid_lambda_syntax() {
    let code = r#"
        let f = \ -> 42;
    "#;
    let result = parse(code);
    assert!(result.is_err());
    assert!(result.unwrap_err().code == "E120");
}
```
