# Proposal 001: Module Constants

**Status:** ✅ Implemented (v0.0.2)
**Author:** @sgerokostas
**Created:** 2026-01-30
**Updated:** 2026-02-01
**Related:** Module-level `let` bindings with compile-time evaluation

---

## Summary

This proposal covers **two aspects** of module constants in Flux:

1. **Feature Proposal**: Add static `let` bindings to modules using compile-time constant evaluation
2. **Architecture Proposal**: Organize module constant analysis into dedicated modules (`bytecode/module_constants/`)

Both aspects have been implemented in v0.0.2.

---

# Part 1: Feature Proposal - Module Let Bindings

## Motivation

Flux modules originally only supported function declarations. Module-level constants enable:

1. **Mathematical constants** (PI, E, TAU) for stdlib
2. **Configuration defaults** (timeouts, buffer sizes, API versions)
3. **Lookup tables** (factorials, error codes, hex digits)
4. **Zero runtime overhead** (compile-time evaluation)

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

## Design Comparison

### Chosen Approach: Compile-Time Constants

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

### Language Comparison

| Feature | Haskell | Elixir | **Flux** |
|---------|---------|--------|----------|
| **Syntax** | `name = value` | `@name value` | `let name = value;` |
| **Evaluation** | Lazy | Compile-time | **Compile-time** |
| **Expressions** | Any | Constant only | **Constant only** |
| **Ordering** | Any | Sequential | **Any (auto-resolved)** |
| **Runtime cost** | On access | Zero | **Zero** |
| **Private** | Export list | `defp` | `_` prefix |

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

---

# Part 2: Architecture Proposal - Modular Organization

## Motivation

The module constants feature added ~50 lines to `compile_module_statement()` in `compiler.rs`. As we add more features, this file will grow unwieldy. We need a pattern where:

1. **Pure analysis logic** lives in dedicated modules
2. **compiler.rs** only orchestrates and stores results
3. **New features** follow the same pattern

This keeps `compiler.rs` focused on coordination rather than implementation details.

---

## Implementation Architecture

### Module Structure

```
src/bytecode/module_constants/
├── mod.rs                  # Public API and re-exports
├── analysis.rs             # ModuleConstantAnalysis + analyze_module_constants()
├── eval.rs                 # eval_const_expr() - compile-time evaluator
├── dependency.rs           # find_constant_refs() + topological_sort_constants()
└── error.rs                # ConstEvalError type
```

### Module: `analysis.rs`

**Purpose:** Consolidate Steps 1-3 of module constant processing

```rust
/// Result of analyzing module constants
pub struct ModuleConstantAnalysis<'a> {
    /// Constants in evaluation order (dependencies first)
    pub eval_order: Vec<String>,
    /// Map of constant name -> (expression, source position)
    pub expressions: HashMap<String, (&'a Expression, Position)>,
}

/// Analyze module constants: collect, build dependencies, topological sort
pub fn analyze_module_constants(
    body: &Block
) -> Result<ModuleConstantAnalysis<'_>, Vec<String>> {
    // Step 1: Collect let statements
    // Step 2: Build dependency graph
    // Step 3: Topological sort
}
```

### Module: `dependency.rs`

**Purpose:** Dependency analysis and topological sorting

```rust
/// Find all constant references in an expression
pub fn find_constant_refs(
    expression: &Expression,
    known_constants: &HashSet<String>,
) -> Vec<String>

/// Topologically sort constants based on dependencies
pub fn topological_sort_constants(
    dependencies: &HashMap<String, Vec<String>>,
) -> Result<Vec<String>, Vec<String>>
```

### Module: `eval.rs`

**Purpose:** Compile-time constant evaluation

```rust
/// Evaluate a constant expression at compile time
pub fn eval_const_expr(
    expr: &Expression,
    defined: &HashMap<String, Object>,
) -> Result<Object, ConstEvalError>

// Helper functions
fn eval_const_unary_op(op: &str, right: &Object) -> Result<Object, ConstEvalError>
fn eval_const_binary_op(left: &Object, op: &str, right: &Object) -> Result<Object, ConstEvalError>
```

### Module: `error.rs`

**Purpose:** Error types for constant evaluation

```rust
/// Error during compile-time constant evaluation
pub struct ConstEvalError {
    pub code: &'static str,
    pub message: String,
    pub hint: Option<String>,
}
```

---

## Compiler Integration

### Before (compiler.rs - ~50 lines inline)

```rust
fn compile_module_statement(&mut self, name: &str, body: &Block, position: Position) -> CompileResult<()> {
    // Step 1: Collect all constant definitions (~15 lines)
    let mut constant_exprs: HashMap<String, (&Expression, Position)> = HashMap::new();
    let mut constant_names: HashSet<String> = HashSet::new();
    for statement in &body.statements {
        if let Statement::Let { name, value, span, .. } = statement {
            constant_names.insert(name.clone());
            constant_exprs.insert(name.clone(), (value, span.start));
        }
    }

    // Step 2: Build dependency graph (~5 lines)
    let mut dependencies: HashMap<String, Vec<String>> = HashMap::new();
    for (const_name, (expr, _)) in &constant_exprs {
        let refs = find_constant_refs(expr, &constant_names);
        dependencies.insert(const_name.clone(), refs);
    }

    // Step 3: Topological sort (~10 lines)
    let eval_order = match topological_sort_constants(&dependencies) {
        Ok(order) => order,
        Err(cycle) => {
            return Err(Self::boxed(self.make_circular_dependency_error(&cycle, position)));
        }
    };

    // Step 4: Evaluate constants (~20 lines)
    // ...
}
```

### After (compiler.rs - ~10 lines)

```rust
fn compile_module_statement(&mut self, name: &str, body: &Block, position: Position) -> CompileResult<()> {
    // START: MODULE CONSTANTS (bytecode/module_constants/)

    // Steps 1-3: Analysis (delegated to module_constants::analyze_module_constants)
    let analysis = match analyze_module_constants(body) {
        Ok(a) => a,
        Err(cycle) => {
            return Err(Self::boxed(self.make_circular_dependency_error(&cycle, position)));
        }
    };

    // Step 4: Evaluate constants in dependency order
    let mut local_constants: HashMap<String, Object> = HashMap::new();
    for const_name in &analysis.eval_order {
        let (expr, pos) = analysis.expressions.get(const_name).unwrap();
        match eval_const_expr(expr, &local_constants) {
            Ok(const_value) => {
                local_constants.insert(const_name.clone(), const_value.clone());
                let qualified_name = format!("{}.{}", binding_name, const_name);
                self.module_constants.insert(qualified_name, const_value);
            }
            Err(err) => {
                return Err(Self::boxed(self.const_eval_error_to_diagnostic(err, *pos)));
            }
        }
    }

    // END: MODULE CONSTANTS

    // PASS 1: Predeclare functions...
    // PASS 2: Compile functions...
}
```

---

## Benefits

### 1. Separation of Concerns
- Pure analysis logic in `module_constants/`
- Compiler orchestration in `compiler.rs`
- Clear API boundary

### 2. Testability
- Can test analysis logic without Compiler instance
- Easier to write comprehensive test cases
- Faster test execution (no full compiler setup)

### 3. Reusability
Future tools can use `analyze_module_constants`:
- **Linter**: Warn about unused constants
- **IDE/LSP**: Show constant dependencies, autocomplete
- **Documentation Generator**: Extract module constants
- **Dependency Visualizer**: Show constant reference graph

### 4. Pattern for Future Features
Establishes template for adding new compiler features:
1. Create analysis function in dedicated module (pure logic)
2. Add orchestration call in compiler.rs (stateful operations)
3. Write unit tests for pure functions

**Future applications:**
- Pattern matching analysis (`pattern_eval/`)
- Type inference (`type_infer/`)
- Effect checking (`effect_check/`)

---

## Summary

| Aspect | Chosen for Flux |
|--------|-----------------|
| **Feature** | Compile-time constants (like Elixir) |
| **Syntax** | `let name = expr;` |
| **Evaluation** | Compile-time (inlined at use sites) |
| **Expressions** | Constant expressions only |
| **Ordering** | Any order (auto-resolved with topological sort) |
| **Visibility** | `_` prefix for private |
| **Runtime cost** | Zero (values baked into bytecode) |
| **Architecture** | Modular (`bytecode/module_constants/`) |
| **Implementation** | ~300 lines across 5 focused modules |

### Benefits

**Feature Benefits:**
1. Zero runtime overhead - constants baked into bytecode
2. True immutability - values literally cannot change
3. Order independence - write constants in logical order
4. Cycle detection - clear errors for circular dependencies
5. Matches function behavior - both support forward references
6. Simple mental model - constants are just values

**Architecture Benefits:**
1. Separation of concerns - analysis vs orchestration
2. Testability - pure functions easy to test
3. Reusability - tools can use analysis modules
4. Maintainability - focused, single-responsibility modules
5. Extensibility - pattern for future compiler features

### Implementation Status

- ✅ Feature implemented in v0.0.2
- ✅ Modular architecture refactoring complete
- ✅ 6 unit tests for dependency analysis
- ✅ Integration tests with examples
- ✅ Documentation complete

This approach covers 95%+ of real use cases (stdlib constants, config defaults, lookup tables) while maintaining Flux's functional programming principles, providing excellent developer experience, and establishing a sustainable architecture pattern for future compiler features.
