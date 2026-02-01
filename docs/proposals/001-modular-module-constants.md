# Proposal 001: Modular Module Constants

**Status:** Draft
**Author:** @sgerokostas
**Created:** 2026-02-01
**Related:** Module Constants feature (v0.0.2)

---

## Summary

Extract the module constant analysis logic (Steps 1-3) from `compiler.rs` into `const_eval.rs` to establish a pattern for adding new compiler features without bloating the main compiler file.

---

## Motivation

The module constants feature added ~50 lines to `compile_module_statement()` in `compiler.rs`. As we add more features, this file will grow unwieldy. We need a pattern where:

1. **Pure analysis logic** lives in dedicated modules
2. **compiler.rs** only orchestrates and stores results
3. **New features** follow the same pattern

This keeps `compiler.rs` focused on coordination rather than implementation details.

---

## Current Implementation

**Location:** `src/bytecode/compiler.rs` lines 1321-1376

```rust
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
    Err(cycle) => { /* error handling */ }
};

// Step 4: Evaluate constants (~20 lines)
for const_name in &eval_order {
    let (expr, pos) = constant_exprs.get(const_name).unwrap();
    match eval_const_expr(expr, &local_constants) {
        Ok(value) => { /* store in self.module_constants */ }
        Err(err) => { /* error handling */ }
    }
}
```

**Problem:** Steps 1-3 are pure logic that doesn't need compiler state, but they're mixed into `compiler.rs`.

---

## Proposed Design

### New API in const_eval.rs

```rust
use crate::frontend::{ast::Block, expression::Expression, statement::Statement};
use crate::frontend::position::Position;

/// Result of analyzing module constants
pub struct ModuleConstantAnalysis<'a> {
    /// Constants in evaluation order (dependencies first)
    pub eval_order: Vec<String>,
    /// Map of constant name -> (expression, source position)
    pub expressions: HashMap<String, (&'a Expression, Position)>,
}

/// Analyze module constants: collect, build dependencies, topological sort
///
/// This function performs Steps 1-3 of module constant processing:
/// 1. Collects all `let` statements from the module body
/// 2. Builds a dependency graph using `find_constant_refs`
/// 3. Topologically sorts to determine evaluation order
///
/// # Arguments
/// * `body` - The module's body block containing statements
///
/// # Returns
/// * `Ok(ModuleConstantAnalysis)` - Analysis result with eval order and expressions
/// * `Err(Vec<String>)` - Cycle path if circular dependency detected
///
/// # Example
/// ```ignore
/// let analysis = analyze_module_constants(body)?;
/// for name in &analysis.eval_order {
///     let (expr, pos) = analysis.expressions.get(name).unwrap();
///     // evaluate expr...
/// }
/// ```
pub fn analyze_module_constants(body: &Block) -> Result<ModuleConstantAnalysis<'_>, Vec<String>> {
    // Step 1: Collect let statements
    let mut expressions: HashMap<String, (&Expression, Position)> = HashMap::new();
    let mut names: HashSet<String> = HashSet::new();

    for statement in &body.statements {
        if let Statement::Let { name, value, span, .. } = statement {
            names.insert(name.clone());
            expressions.insert(name.clone(), (value.as_ref(), span.start));
        }
    }

    // Step 2: Build dependency graph
    let mut dependencies: HashMap<String, Vec<String>> = HashMap::new();
    for (name, (expr, _)) in &expressions {
        let refs = find_constant_refs(expr, &names);
        dependencies.insert(name.clone(), refs);
    }

    // Step 3: Topological sort
    let eval_order = topological_sort_constants(&dependencies)?;

    Ok(ModuleConstantAnalysis { eval_order, expressions })
}
```

### Updated compiler.rs

```rust
use super::const_eval::{analyze_module_constants, eval_const_expr};

fn compile_module_statement(&mut self, name: &str, body: &Block, position: Position) -> CompileResult<()> {
    // ... validation code ...

    // PASS 0: Analyze and evaluate module constants
    let analysis = match analyze_module_constants(body) {
        Ok(a) => a,
        Err(cycle) => {
            self.current_module_prefix = previous_module;
            return Err(Self::boxed(self.make_circular_dependency_error(&cycle, position)));
        }
    };

    // Evaluate in dependency order (Step 4 - needs self.module_constants)
    let mut local_constants: HashMap<String, Object> = HashMap::new();
    for const_name in &analysis.eval_order {
        let (expr, pos) = analysis.expressions.get(const_name).unwrap();
        match eval_const_expr(expr, &local_constants) {
            Ok(value) => {
                local_constants.insert(const_name.clone(), value.clone());
                let qualified = format!("{}.{}", binding_name, const_name);
                self.module_constants.insert(qualified, value);
            }
            Err(err) => {
                self.current_module_prefix = previous_module;
                return Err(Self::boxed(self.const_eval_error_to_diagnostic(err, *pos)));
            }
        }
    }

    // PASS 1 & 2: Function compilation (unchanged)
    // ...
}
```

---

## Line Count Impact

| File | Before | After | Delta |
|------|--------|-------|-------|
| `compiler.rs` | ~50 lines | ~18 lines | **-32** |
| `const_eval.rs` | 371 lines | ~405 lines | +34 |
| **Net** | - | - | +2 (tests) |

The total line count is similar, but the **separation of concerns** is the key benefit.

---

## Benefits

### 1. Testability
```rust
#[test]
fn test_analyze_empty_module() {
    let body = Block { statements: vec![] };
    let result = analyze_module_constants(&body).unwrap();
    assert!(result.eval_order.is_empty());
}

#[test]
fn test_analyze_with_dependencies() {
    // let A = 1; let B = A + 1;
    let body = make_test_block(vec![
        make_let("A", int_expr(1)),
        make_let("B", infix_expr(ident("A"), "+", int_expr(1))),
    ]);
    let result = analyze_module_constants(&body).unwrap();
    assert_eq!(result.eval_order, vec!["A", "B"]);
}
```

### 2. Reusability
The analysis function could be used by:
- **Linter** - Warn about unused constants
- **IDE/LSP** - Show constant dependencies
- **Documentation generator** - List module constants

### 3. Pattern for Future Features
When adding new compiler features:
1. Create pure analysis functions in dedicated module
2. Add orchestration to `compiler.rs` (just call + store)
3. Write unit tests for the pure functions

---

## Implementation Plan

### Phase 1: Add New Function
1. Add `ModuleConstantAnalysis` struct to `const_eval.rs`
2. Add `analyze_module_constants` function
3. Add new imports (`Block`, `Statement`, `Position`)
4. Add unit tests

### Phase 2: Update Compiler
1. Import the new function in `compiler.rs`
2. Replace Steps 1-3 with single function call
3. Keep Step 4 (evaluation loop) in place
4. Run existing tests to verify

### Phase 3: Verify
1. Run `cargo test`
2. Run integration tests
3. Test examples: `using_constants.flx`, `using_modules.flx`

---

## New Dependencies in const_eval.rs

```rust
use crate::frontend::{
    ast::Block,
    expression::Expression,
    statement::Statement,
};
use crate::frontend::position::Position;
```

These are already used elsewhere in the codebase, so no new external dependencies.

---

## Tests to Add

```rust
#[cfg(test)]
mod tests {
    use super::*;
    // ... existing tests ...

    #[test]
    fn test_analyze_empty_module() {
        let body = Block { statements: vec![] };
        let result = analyze_module_constants(&body).unwrap();
        assert!(result.eval_order.is_empty());
        assert!(result.expressions.is_empty());
    }

    #[test]
    fn test_analyze_single_constant() {
        // let X = 42;
        // Result: eval_order = ["X"]
    }

    #[test]
    fn test_analyze_independent_constants() {
        // let A = 1; let B = 2;
        // Result: eval_order contains both (order may vary)
    }

    #[test]
    fn test_analyze_dependent_constants() {
        // let A = 1; let B = A * 2; let C = B + A;
        // Result: eval_order = ["A", "B", "C"]
    }

    #[test]
    fn test_analyze_circular_dependency() {
        // let A = B; let B = A;
        // Result: Err with cycle
    }
}
```

---

## Alternatives Considered

### A. Create module_compiler.rs
Move all module-related code to a new file.

**Rejected:** The constant analysis is tightly coupled with `const_eval.rs` (uses `find_constant_refs`, `topological_sort_constants`). Splitting across files would create more coupling.

### B. Use Extension Trait
```rust
pub trait ModuleCompiler {
    fn compile_module_statement(&mut self, ...) -> CompileResult<()>;
}
impl ModuleCompiler for Compiler { ... }
```

**Rejected:** More complex, requires visibility changes, overkill for this use case.

### C. Keep Inline
Leave the code in `compiler.rs`.

**Rejected:** Doesn't address the maintainability goal. As more features are added, `compiler.rs` becomes harder to navigate.

---

## Future Work

This proposal establishes a pattern. Future features should follow:

| Feature | Analysis Module | Compiler Integration |
|---------|-----------------|---------------------|
| Module constants | `const_eval.rs` | Call `analyze_module_constants` |
| Pattern matching | `pattern_eval.rs` (future) | Call `analyze_patterns` |
| Type inference | `type_infer.rs` (future) | Call `infer_types` |

---

## Decision

**Pending review.**

---

## References

- [COMPILER_ARCHITECTURE.md](../COMPILER_ARCHITECTURE.md) - Phase 1 module split plan
- [const_eval.rs](../../src/bytecode/const_eval.rs) - Current implementation
- [compiler.rs](../../src/bytecode/compiler.rs) - Lines 1321-1376
