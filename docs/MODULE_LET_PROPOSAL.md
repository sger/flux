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

### Current Validation (compiler.rs:1273-1280)

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
    // Compile-time constants (order doesn't matter!)
    let TAU = PI * 2;  // Can reference PI even though it's defined below
    let PI = 3.141592653589793;
    let E = 2.718281828459045;

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
    let NOTHING = None;
    let SOMETHING = Some(42);

    // Arithmetic on numbers
    let TAU = PI * 2;
    let DOUBLE_E = E + E;
    let HALF_PI = PI / 2;

    // Unary operators
    let NEG_ONE = -1;
    let NOT_TRUE = !true;

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

    // References to other constants (any order!)
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

    // ERROR: Circular dependencies
    let A = B + 1;
    let B = A + 1;  // A and B depend on each other

    // ERROR: Non-constant expressions
    let Z = if condition { 1 } else { 2 };
}
```

### Order Independence (Automatic Dependency Resolution)

Constants can be defined in **any order**. The compiler automatically resolves dependencies:

```flux
module Example {
    let C = B * 2;      // OK: B will be resolved
    let B = A + 1;      // OK: A will be resolved
    let A = 1;          // Base constant

    // All resolve to: A=1, B=2, C=4
}
```

This matches how functions already work in Flux (forward references allowed).

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
    let TAU = PI * 2;  // Compile-time, order independent
    let PI = 3.141592653589793;

    fun circle_area(r) {
        PI * r * r;  // PI is inlined here
    }
}
```

| Aspect | Flux |
|--------|------|
| Syntax | `let name = value;` |
| Evaluation | **Compile-time (inlined)** |
| Ordering | **Any order (auto-resolved)** |
| Visibility | `_` prefix for private |

---

## Comparison Matrix

| Feature | Haskell | Elixir | **Flux** |
|---------|---------|--------|----------|
| **Syntax** | `name = value` | `@name value` | `let name = value;` |
| **Evaluation** | Lazy | Compile-time | **Compile-time** |
| **Expressions** | Any | Constant only | **Constant only** |
| **Ordering** | Any | Sequential | **Any (auto-resolved)** |
| **Runtime cost** | On access | Zero | **Zero** |
| **Private** | Export list | `defp` | `_` prefix |

---

## Implementation

### Step 1: Collect Constants and Build Dependency Graph

```rust
/// Find all constant references in an expression
fn find_constant_refs(expr: &Expression, known_constants: &HashSet<String>) -> Vec<String> {
    let mut refs = Vec::new();
    match expr {
        Expression::Identifier(name) => {
            if known_constants.contains(&name.value) {
                refs.push(name.value.clone());
            }
        },
        Expression::Infix { left, right, .. } => {
            refs.extend(find_constant_refs(left, known_constants));
            refs.extend(find_constant_refs(right, known_constants));
        },
        Expression::Prefix { right, .. } => {
            refs.extend(find_constant_refs(right, known_constants));
        },
        Expression::Array(elements) => {
            for elem in elements {
                refs.extend(find_constant_refs(elem, known_constants));
            }
        },
        Expression::Hash(pairs) => {
            for (key, value) in pairs {
                refs.extend(find_constant_refs(key, known_constants));
                refs.extend(find_constant_refs(value, known_constants));
            }
        },
        _ => {}
    }
    refs
}
```

### Step 2: Topological Sort with Cycle Detection

```rust
/// Topologically sort constants based on dependencies
fn topological_sort(
    dependencies: &HashMap<String, Vec<String>>
) -> Result<Vec<String>, CompileError> {
    let mut result = Vec::new();
    let mut visited = HashSet::new();
    let mut in_progress = HashSet::new();

    fn visit(
        name: &str,
        deps: &HashMap<String, Vec<String>>,
        visited: &mut HashSet<String>,
        in_progress: &mut HashSet<String>,
        result: &mut Vec<String>,
    ) -> Result<(), Vec<String>> {
        if visited.contains(name) {
            return Ok(());
        }
        if in_progress.contains(name) {
            // Found a cycle - return the cycle path
            return Err(vec![name.to_string()]);
        }

        in_progress.insert(name.to_string());

        if let Some(dependencies) = deps.get(name) {
            for dep in dependencies {
                if let Err(mut cycle) = visit(dep, deps, visited, in_progress, result) {
                    cycle.push(name.to_string());
                    return Err(cycle);
                }
            }
        }

        in_progress.remove(name);
        visited.insert(name.to_string());
        result.push(name.to_string());
        Ok(())
    }

    for name in dependencies.keys() {
        if let Err(cycle) = visit(name, dependencies, &mut visited, &mut in_progress, &mut result) {
            return Err(CompileError::circular_dependency(cycle));
        }
    }

    Ok(result)
}
```

### Step 3: Constant Expression Evaluator

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
        Expression::None => Ok(Object::None),
        Expression::Some(inner) => {
            let value = self.eval_const_expr(inner, defined)?;
            Ok(Object::Some(Box::new(value)))
        },

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
        Expression::Infix { left, operator, right, .. } => {
            let l = self.eval_const_expr(left, defined)?;
            let r = self.eval_const_expr(right, defined)?;
            self.eval_const_binary_op(&l, operator, &r)
        },

        // Unary operations
        Expression::Prefix { operator, right, .. } => {
            let r = self.eval_const_expr(right, defined)?;
            self.eval_const_unary_op(operator, &r)
        },

        // Grouped expressions
        Expression::Grouped(inner) => self.eval_const_expr(inner, defined),

        // Reference to another constant
        Expression::Identifier(name) => {
            defined.get(&name.value)
                .cloned()
                .ok_or_else(|| CompileError::new(
                    format!("'{}' is not a module constant", name.value)
                ))
        },

        // Anything else is not a constant expression
        _ => Err(CompileError::new(
            "not a constant expression (only literals, basic operations, and references to module constants allowed)"
        )),
    }
}

fn eval_const_unary_op(&self, op: &str, right: &Object) -> Result<Object, CompileError> {
    match (op, right) {
        ("-", Object::Integer(n)) => Ok(Object::Integer(-n)),
        ("-", Object::Float(f)) => Ok(Object::Float(-f)),
        ("!", Object::Boolean(b)) => Ok(Object::Boolean(!b)),
        _ => Err(CompileError::new(
            format!("cannot apply '{}' to {:?} at compile time", op, right)
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
        // Integer operations
        (Object::Integer(a), "+", Object::Integer(b)) => Ok(Object::Integer(a + b)),
        (Object::Integer(a), "-", Object::Integer(b)) => Ok(Object::Integer(a - b)),
        (Object::Integer(a), "*", Object::Integer(b)) => Ok(Object::Integer(a * b)),
        (Object::Integer(a), "/", Object::Integer(b)) => Ok(Object::Integer(a / b)),
        (Object::Integer(a), "%", Object::Integer(b)) => Ok(Object::Integer(a % b)),

        // Float operations
        (Object::Float(a), "+", Object::Float(b)) => Ok(Object::Float(a + b)),
        (Object::Float(a), "-", Object::Float(b)) => Ok(Object::Float(a - b)),
        (Object::Float(a), "*", Object::Float(b)) => Ok(Object::Float(a * b)),
        (Object::Float(a), "/", Object::Float(b)) => Ok(Object::Float(a / b)),

        // Mixed numeric (promote to float)
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
        (Object::Integer(a), "<=", Object::Integer(b)) => Ok(Object::Boolean(a <= b)),
        (Object::Integer(a), ">=", Object::Integer(b)) => Ok(Object::Boolean(a >= b)),

        _ => Err(CompileError::new(
            format!("cannot apply '{}' to {:?} and {:?} at compile time", op, left, right)
        )),
    }
}
```

### Step 4: Module Compilation Changes

```rust
fn compile_module_statement(&mut self, name: &str, body: &Block) -> Result<(), CompileError> {
    // Step 1: Collect all constant definitions
    let mut constant_exprs: HashMap<String, &Expression> = HashMap::new();
    let mut constant_names: HashSet<String> = HashSet::new();

    for stmt in &body.statements {
        if let Statement::Let { name: let_name, value, .. } = stmt {
            constant_names.insert(let_name.clone());
            constant_exprs.insert(let_name.clone(), value);
        }
    }

    // Step 2: Build dependency graph
    let mut dependencies: HashMap<String, Vec<String>> = HashMap::new();
    for (const_name, expr) in &constant_exprs {
        let refs = find_constant_refs(expr, &constant_names);
        dependencies.insert(const_name.clone(), refs);
    }

    // Step 3: Topological sort (detect cycles)
    let eval_order = topological_sort(&dependencies)?;

    // Step 4: Evaluate constants in dependency order
    let mut module_constants: HashMap<String, Object> = HashMap::new();
    for const_name in &eval_order {
        let expr = constant_exprs.get(const_name).unwrap();
        let const_value = self.eval_const_expr(expr, &module_constants)?;

        // Store locally for later constants
        module_constants.insert(const_name.clone(), const_value.clone());

        // Store with qualified name for access
        let qualified_name = format!("{}.{}", name, const_name);
        self.module_constants.insert(qualified_name, const_value);
    }

    // Step 5: Predeclare all functions (for forward references)
    for stmt in &body.statements {
        if let Statement::Function { name: fn_name, .. } = stmt {
            let qualified_name = format!("{}.{}", name, fn_name);
            self.symbol_table.define(&qualified_name);
        }
    }

    // Step 6: Compile all functions
    for stmt in &body.statements {
        if let Statement::Function { .. } = stmt {
            self.compile_function_statement(stmt, Some(name))?;
        }
    }

    Ok(())
}
```

### Step 5: Constant Access (Inlining)

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
    // Order doesn't matter!
    let TAU = PI * 2;
    let HALF_PI = PI / 2;
    let PI = 3.141592653589793;
    let E = 2.718281828459045;
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

### Circular Dependency

```
Error: circular dependency in module constants
  --> Math.flx
   |
 2 |     let A = B + 1;
   |             ^ A depends on B
 3 |     let B = A + 1;
   |             ^ B depends on A
   |
   = note: constants form a cycle: A -> B -> A
   = help: break the cycle by using a literal value
```

### Non-Constant Expression

```
Error: not a constant expression
  --> Config.flx:2:18
   |
 2 |     let VALUE = get_env("KEY");
   |                 ^^^^^^^^^^^^^^
   |
   = note: only literals, basic operations, and references to module constants allowed
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

### Unknown Identifier

```
Error: 'UNKNOWN' is not a module constant
  --> Bad.flx:2:15
   |
 2 |     let X = UNKNOWN + 1;
   |             ^^^^^^^
   |
   = help: only references to other module constants are allowed
```

---

## Summary

| Aspect | Chosen for Flux |
|--------|-----------------|
| **Approach** | Compile-time constants (like Elixir) |
| **Syntax** | `let name = expr;` |
| **Evaluation** | Compile-time (inlined at use sites) |
| **Expressions** | Constant expressions only |
| **Ordering** | **Any order (auto-resolved)** |
| **Visibility** | `_` prefix for private |
| **Runtime cost** | Zero |

### Benefits

1. **Zero runtime overhead** - constants are baked into bytecode
2. **True immutability** - values literally cannot change
3. **Order independence** - write constants in logical order, not dependency order
4. **Cycle detection** - clear errors for circular dependencies
5. **Matches function behavior** - both support forward references
6. **Predictable behavior** - no hidden initialization
7. **Simple mental model** - constants are just values

### Trade-offs

1. **Limited expressions** - can't call functions
2. **New compiler code** - constant evaluator + topological sort (~150 lines)

This approach covers 95%+ of real use cases (stdlib constants, config defaults, lookup tables) while maintaining Flux's functional programming principles and providing a great developer experience.
