# Semicolon Rules in Flux

Quick reference for when semicolons are optional vs required in Flux.

---

## TL;DR

**OPTIONAL:** Top-level statements on separate lines
**REQUIRED:** Inside function bodies (except last expression)

---

## Rules

### ✓ Semicolons OPTIONAL

#### 1. Top-level statements on separate lines
```flux
let x = 42
let y = 10
print(x + y)
```

#### 2. Last expression in function/block
```flux
fun add(a, b) {
    a + b  // No semicolon (last expression)
}
```

#### 3. Function definitions at top level
```flux
fun multiply(x, y) { x * y }  // No semicolon after function
fun divide(x, y) { x / y }    // No semicolon
```

---

### ✗ Semicolons REQUIRED

#### 1. Multiple statements on same line
```flux
let a = 1; let b = 2; let c = 3  // Semicolons needed
```

#### 2. Inside function bodies (except last statement)
```flux
fun calculate(n) {
    let doubled = n * 2;     // REQUIRED
    let squared = n * n;     // REQUIRED
    doubled + squared        // OPTIONAL (last)
}
```

#### 3. Return statements (except when last)
```flux
fun get_max(a, b) {
    if (a > b) {
        return a;            // REQUIRED (not last in function)
    }
    return b                 // OPTIONAL (last in function)
}
```

---

## Common Patterns

### Pattern 1: Clean Top-Level (Recommended)
```flux
let name = "Flux"
let version = "0.0.3"
print(name + " " + version)
```

### Pattern 2: Traditional Style (Also Valid)
```flux
let name = "Flux";
let version = "0.0.3";
print(name + " " + version);
```

### Pattern 3: Functions with Multiple Statements
```flux
fun process(data) {
    let cleaned = trim(data);      // REQUIRED
    let uppercased = upper(cleaned);  // REQUIRED
    uppercased                      // OPTIONAL (last)
}
```

### Pattern 4: Early Returns
```flux
fun validate(x) {
    if (x < 0) {
        return Left("negative");    // REQUIRED
    }
    if (x > 100) {
        return Left("too large");   // REQUIRED
    }
    Right(x)                         // OPTIONAL (last)
}
```

---

## Parser Quirk

**Consecutive `let` blocks** without semicolons need separation:

```flux
// ✗ Won't parse:
let x = 1
let y = 2

let a = 10
let b = 20

// ✓ Works: Add semicolons to one block
let x = 1;
let y = 2;

let a = 10
let b = 20

// ✓ Works: Add statement between blocks
let x = 1
let y = 2

print("separator")  // Non-let statement

let a = 10
let b = 20
```

---

## Recommendation

**Top-level scripts:** Skip semicolons (cleaner)
```flux
let x = 42
let y = 10
print(x + y)
```

**Function bodies:** Use semicolons (except last expression)
```flux
fun calculate(n) {
    let result = n * 2;
    result
}
```

---

## Examples

See [examples/basics/semicolons.flx](../examples/basics/semicolons.flx) for comprehensive examples demonstrating all rules.

---

## Testing

Regression tests in `tests/parser_tests.rs` ensure semicolon behavior remains consistent:
- `test_optional_semicolons_let_statements`
- `test_optional_semicolons_expressions`
- `test_optional_semicolons_function_calls`
- `test_mixed_semicolons`
- `test_optional_semicolons_return`
- `test_optional_semicolons_if_statements`
- `test_optional_semicolons_multiple_expressions`

---

## Summary Table

| Context | Semicolon | Example |
|---------|-----------|---------|
| Top-level statements | Optional | `let x = 1` |
| Same line statements | **Required** | `let x = 1; let y = 2` |
| Inside function (not last) | **Required** | `let temp = x;` |
| Last expression in function | Optional | `x + y` |
| Return (not last) | **Required** | `return x;` |
| Return (last) | Optional | `return x` |
| Function definition | Optional | `fun f() { ... }` |

---

**Version:** 0.0.3
**Last Updated:** 2026-02-01
