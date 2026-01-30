# Static Module Let Bindings Proposal

This document proposes adding static `let` bindings to Flux modules using **compile-time constant evaluation**, inspired by Elixir's module attributes.

---

## Current State

### What Modules Support Today

Flux modules currently only support **function declarations**:

```flux
module Math {
    fun add(a, b) { a + b }
    fun multiply(a, b) { a * b }
}
```

### What's Missing

Module-level constants/values are not supported:

```flux
module Math {
    let PI = 3.141592653589793;  // ERROR: Not currently allowed
    let E = 2.718281828459045;

    fun circle_area(r) {
        PI * r * r;
    }
}
```

### Current Validation (compiler.rs:1051-1076)

The compiler explicitly rejects non-function statements in modules:

```rust
// Only functions are allowed in modules
for stmt in &body.statements {
    match stmt {
        Statement::Function { .. } => { /* OK */ }
        _ => { /* ERROR: only function declarations allowed */ }
    }
}
```

---

## Chosen Approach: Compile-Time Constants

### Why Compile-Time?

| Aspect | Compile-Time | Load-Time | Lazy |
|--------|--------------|-----------|------|
| **Runtime cost** | Zero | Once per load | On access |
| **Predictability** | High | Medium | Low |
| **Expression support** | Constant exprs | Any expr | Any expr |
| **Implementation** | Const evaluator | Use VM | Thunks |
| **FP Philosophy** | True immutability | Mutable in memory | Deferred |

**Compile-time wins because:**

1. **Zero runtime cost** - values baked into bytecode
2. **True constants** - can never change, ever
3. **Predictable** - no hidden initialization code
4. **Sufficient for stdlib** - Flow.Math just needs PI, E, etc.
5. **Proven approach** - Elixir uses this successfully

---

## Syntax

```flux
module Flow.Math {
    // Compile-time constants
    let PI = 3.141592653589793;
    let E = 2.718281828459045;
    let TAU = PI * 2;  // Can reference earlier constants

    // Functions can use module constants
    fun circle_area(r) {
        PI * r * r;
    }

    fun circle_circumference(r) {
        TAU * r;
    }
}

// Usage
print(Flow.Math.PI);              // 3.141592653589793
print(Flow.Math.circle_area(5));  // 78.53981633974483
```

---

## Semantics

### What's Allowed (Constant Expressions)

```flux
module Constants {
    // Literals
    let PI = 3.141592653589793;
    let NAME = "math";
    let ENABLED = true;
    let COUNT = 42;

    // Arithmetic on numbers
    let TAU = PI * 2;
    let DOUBLE_E = E + E;
    let HALF_PI = PI / 2;

    // String concatenation
    let GREETING = "Hello, " + "World!";

    // Boolean operations
    let BOTH = true && false;
    let EITHER = true || false;

    // Constant arrays
    let PRIMES = [2, 3, 5, 7, 11, 13];
    let EMPTY = [];

    // Constant hashes
    let CONFIG = {
        "timeout": 30000,
        "retries": 3
    };

    // References to earlier constants
    let TAU_SQUARED = TAU * TAU;
}
```

### What's NOT Allowed

```flux
module Invalid {
    // ERROR: Function calls
    let X = compute_value();

    // ERROR: Variables from outside
    let Y = some_global;

    // ERROR: Forward references
    let A = B * 2;
    let B = 10;

    // ERROR: Non-constant expressions
    let Z = if condition { 1 } else { 2 };
}
```

### Order Matters

Constants must be defined before use:

```flux
module Example {
    let A = 1;
    let B = A + 1;      // OK: A is defined
    let C = B * 2;      // OK: B is defined
    let D = E;          // ERROR: E not yet defined
    let E = 5;

    // Functions can use ANY constant (evaluated after all constants)
    fun use_all() {
        A + B + C + E;  // OK
    }
}
```

### Privacy

Use `_` prefix for private constants (existing convention):

```flux
module Math {
    let _INTERNAL = 42;     // Private
    let PI = 3.14159;       // Public

    fun _helper() { ... }   // Private
    fun add(a, b) { a + b } // Public
}
```

---

## Language Comparison

### Haskell

```haskell
module Math where

pi :: Double
pi = 3.141592653589793

tau :: Double
tau = pi * 2  -- Lazy evaluation

circleArea :: Double -> Double
circleArea r = pi * r * r
```

| Aspect | Haskell |
|--------|---------|
| Syntax | `name = value` (no distinction from functions) |
| Evaluation | Lazy (on first access) |
| Ordering | Any order (mutual recursion allowed) |
| Visibility | Export list |

### Elixir

```elixir
defmodule Math do
  @pi 3.141592653589793
  @tau @pi * 2  # Compile-time

  def circle_area(r) do
    @pi * r * r  # @pi is inlined here
  end
end
```

| Aspect | Elixir |
|--------|--------|
| Syntax | `@name value` |
| Evaluation | Compile-time (inlined) |
| Ordering | Must define before use |
| Visibility | `def` vs `defp` |

### Flux (Chosen)

```flux
module Math {
    let PI = 3.141592653589793;
    let TAU = PI * 2;  // Compile-time

    fun circle_area(r) {
        PI * r * r;  // PI is inlined here
    }
}
```

| Aspect | Flux |
|--------|------|
| Syntax | `let name = value;` |
| Evaluation | **Compile-time (inlined)** |
| Ordering | Must define before use |
| Visibility | `_` prefix for private |

---

## Comparison Matrix

| Feature | Haskell | Elixir | **Flux** |
|---------|---------|--------|----------|
| **Syntax** | `name = value` | `@name value` | `let name = value;` |
| **Evaluation** | Lazy | Compile-time | **Compile-time** |
| **Expressions** | Any | Constant only | **Constant only** |
| **Ordering** | Any | Sequential | **Sequential** |
| **Runtime cost** | On access | Zero | **Zero** |
| **Private** | Export list | `defp` | `_` prefix |

---

## Implementation

### Constant Expression Evaluator

Add a new function to the compiler that evaluates constant expressions at compile time:

```rust
fn eval_const_expr(
    &self,
    expr: &Expression,
    defined: &HashMap<String, Object>
) -> Result<Object, CompileError> {
    match expr {
        // Literals
        Expression::Integer(n) => Ok(Object::Integer(*n)),
        Expression::Float(f) => Ok(Object::Float(*f)),
        Expression::String(s) => Ok(Object::String(s.clone())),
        Expression::Boolean(b) => Ok(Object::Boolean(*b)),

        // Arrays of constants
        Expression::Array(elements) => {
            let values: Result<Vec<_>, _> = elements
                .iter()
                .map(|e| self.eval_const_expr(e, defined))
                .collect();
            Ok(Object::Array(values?))
        },

        // Hashes of constants
        Expression::Hash(pairs) => {
            let mut map = HashMap::new();
            for (key, value) in pairs {
                let k = self.eval_const_expr(key, defined)?;
                let v = self.eval_const_expr(value, defined)?;
                map.insert(k.to_hash_key()?, v);
            }
            Ok(Object::Hash(map))
        },

        // Binary operations
        Expression::Infix { left, operator, right } => {
            let l = self.eval_const_expr(left, defined)?;
            let r = self.eval_const_expr(right, defined)?;
            self.eval_const_binary_op(&l, operator, &r)
        },

        // Unary operations
        Expression::Prefix { operator, right } => {
            let r = self.eval_const_expr(right, defined)?;
            self.eval_const_unary_op(operator, &r)
        },

        // Reference to earlier constant
        Expression::Identifier(name) => {
            defined.get(&name.value)
                .cloned()
                .ok_or_else(|| CompileError::new(
                    format!("'{}' is not a constant or not yet defined", name.value)
                ))
        },

        // Anything else is not a constant expression
        _ => Err(CompileError::new(
            "not a constant expression (only literals, basic operations, and references to earlier constants allowed)"
        )),
    }
}

fn eval_const_binary_op(
    &self,
    left: &Object,
    op: &str,
    right: &Object
) -> Result<Object, CompileError> {
    match (left, op, right) {
        // Numeric operations
        (Object::Integer(a), "+", Object::Integer(b)) => Ok(Object::Integer(a + b)),
        (Object::Integer(a), "-", Object::Integer(b)) => Ok(Object::Integer(a - b)),
        (Object::Integer(a), "*", Object::Integer(b)) => Ok(Object::Integer(a * b)),
        (Object::Integer(a), "/", Object::Integer(b)) => Ok(Object::Integer(a / b)),

        (Object::Float(a), "+", Object::Float(b)) => Ok(Object::Float(a + b)),
        (Object::Float(a), "-", Object::Float(b)) => Ok(Object::Float(a - b)),
        (Object::Float(a), "*", Object::Float(b)) => Ok(Object::Float(a * b)),
        (Object::Float(a), "/", Object::Float(b)) => Ok(Object::Float(a / b)),

        // Mixed numeric
        (Object::Integer(a), op, Object::Float(b)) => {
            self.eval_const_binary_op(&Object::Float(*a as f64), op, right)
        },
        (Object::Float(a), op, Object::Integer(b)) => {
            self.eval_const_binary_op(left, op, &Object::Float(*b as f64))
        },

        // String concatenation
        (Object::String(a), "+", Object::String(b)) => {
            Ok(Object::String(format!("{}{}", a, b)))
        },

        // Boolean operations
        (Object::Boolean(a), "&&", Object::Boolean(b)) => Ok(Object::Boolean(*a && *b)),
        (Object::Boolean(a), "||", Object::Boolean(b)) => Ok(Object::Boolean(*a || *b)),

        // Comparisons
        (Object::Integer(a), "==", Object::Integer(b)) => Ok(Object::Boolean(a == b)),
        (Object::Integer(a), "!=", Object::Integer(b)) => Ok(Object::Boolean(a != b)),
        (Object::Integer(a), "<", Object::Integer(b)) => Ok(Object::Boolean(a < b)),
        (Object::Integer(a), ">", Object::Integer(b)) => Ok(Object::Boolean(a > b)),

        _ => Err(CompileError::new(
            format!("cannot apply '{}' to {:?} and {:?} at compile time", op, left, right)
        )),
    }
}
```

### Module Compilation Changes

```rust
fn compile_module_statement(&mut self, name: &str, body: &Block) -> Result<(), CompileError> {
    // Track constants defined so far
    let mut module_constants: HashMap<String, Object> = HashMap::new();

    // First pass: evaluate all let bindings
    for stmt in &body.statements {
        if let Statement::Let { name: let_name, value, .. } = stmt {
            let qualified_name = format!("{}.{}", name, let_name.value);

            // Evaluate at compile time
            let const_value = self.eval_const_expr(value, &module_constants)?;

            // Store for later constants to reference
            module_constants.insert(let_name.value.clone(), const_value.clone());

            // Add to constant pool (will be inlined at use sites)
            self.module_constants.insert(qualified_name, const_value);
        }
    }

    // Second pass: predeclare all functions (for forward references)
    for stmt in &body.statements {
        if let Statement::Function { name: fn_name, .. } = stmt {
            let qualified_name = format!("{}.{}", name, fn_name.value);
            self.symbol_table.define(&qualified_name);
        }
    }

    // Third pass: compile all functions
    for stmt in &body.statements {
        if let Statement::Function { .. } = stmt {
            self.compile_function_statement(stmt, Some(name))?;
        }
    }

    Ok(())
}
```

### Constant Access (Inlining)

When a constant is accessed, inline it directly:

```rust
fn compile_member_access(&mut self, object: &str, member: &str) -> Result<(), CompileError> {
    let qualified_name = format!("{}.{}", object, member);

    // Check if it's a compile-time constant
    if let Some(const_value) = self.module_constants.get(&qualified_name) {
        // Inline the constant value
        let idx = self.add_constant(const_value.clone());
        self.emit(OpConstant, &[idx]);
        return Ok(());
    }

    // Otherwise, it's a function - load from globals
    // ... existing code ...
}
```

---

## Use Cases

### 1. Mathematical Constants (Flow.Math)

```flux
module Flow.Math {
    let PI = 3.141592653589793;
    let E = 2.718281828459045;
    let TAU = PI * 2;
    let PHI = 1.618033988749895;
    let SQRT2 = 1.4142135623730951;
    let LN2 = 0.6931471805599453;
    let LN10 = 2.302585092994046;

    fun circle_area(r) { PI * r * r }
    fun circle_circumference(r) { TAU * r }
    fun degrees_to_radians(d) { d * PI / 180 }
    fun radians_to_degrees(r) { r * 180 / PI }
}
```

### 2. Configuration Defaults

```flux
module Config {
    let DEFAULT_TIMEOUT = 30000;
    let MAX_RETRIES = 3;
    let BUFFER_SIZE = 4096;
    let API_VERSION = "v2";
}
```

### 3. Lookup Tables

```flux
module Lookup {
    let FACTORIALS = [1, 1, 2, 6, 24, 120, 720, 5040, 40320, 362880];
    let FIBONACCI = [0, 1, 1, 2, 3, 5, 8, 13, 21, 34, 55, 89];
    let HEX_CHARS = "0123456789ABCDEF";
}
```

### 4. Error Codes

```flux
module ErrorCode {
    let OK = 0;
    let NOT_FOUND = 404;
    let UNAUTHORIZED = 401;
    let SERVER_ERROR = 500;

    let MESSAGES = {
        "0": "Success",
        "404": "Not Found",
        "401": "Unauthorized",
        "500": "Internal Server Error"
    };
}
```

---

## Error Messages

### Forward Reference

```
Error: 'TAU' is not a constant or not yet defined
  --> Math.flx:3:15
   |
 2 |     let PI = TAU / 2;  // ERROR
   |              ^^^
 3 |     let TAU = 6.28;
   |
   = help: move 'TAU' before 'PI', or reorder the constants
```

### Non-Constant Expression

```
Error: not a constant expression
  --> Config.flx:2:18
   |
 2 |     let VALUE = get_env("KEY");
   |                 ^^^^^^^^^^^^^^
   |
   = note: only literals, basic operations, and references to earlier constants allowed
   = help: module constants must be evaluable at compile time
```

### Type Mismatch

```
Error: cannot apply '+' to String and Integer at compile time
  --> Bad.flx:2:15
   |
 2 |     let X = "hello" + 42;
   |             ^^^^^^^^^^^^
   |
   = help: use string interpolation or convert types explicitly
```

---

## Summary

| Aspect | Chosen for Flux |
|--------|-----------------|
| **Approach** | Compile-time constants (like Elixir) |
| **Syntax** | `let name = expr;` |
| **Evaluation** | Compile-time (inlined at use sites) |
| **Expressions** | Constant expressions only |
| **Ordering** | Must define before use |
| **Visibility** | `_` prefix for private |
| **Runtime cost** | Zero |

### Benefits

1. **Zero runtime overhead** - constants are baked into bytecode
2. **True immutability** - values literally cannot change
3. **Predictable behavior** - no hidden initialization
4. **Simple mental model** - constants are just values
5. **Matches FP philosophy** - immutable by design

### Trade-offs

1. **Limited expressions** - can't call functions
2. **Sequential ordering** - must define before use
3. **New compiler code** - constant expression evaluator (~100 lines)

This approach covers 95%+ of real use cases (stdlib constants, config defaults, lookup tables) while maintaining Flux's functional programming principles.
