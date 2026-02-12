# AST Visitor Pattern Guide

This guide explains how the visitor pattern works in the Flux compiler and how to use it for AST traversal and analysis.

## Quick Start

```rust
use crate::ast::{Visitor, walk_expr};
use crate::syntax::expression::Expression;

struct MyAnalyzer {
    function_count: usize,
}

impl<'ast> Visitor<'ast> for MyAnalyzer {
    fn visit_expr(&mut self, expr: &'ast Expression) {
        if let Expression::Function { .. } = expr {
            self.function_count += 1;
        }
        walk_expr(self, expr);  // Continue traversal
    }
}
```

## The Core Concept

The visitor pattern separates **traversal logic** (walking the tree) from **custom actions** (what you want to do at each node). This makes it easy to add new analysis passes without modifying the AST structure.

### Architecture Overview

```
┌─────────────────────────────────────────┐
│  Visitor Trait (src/ast/visit.rs)      │
│  - Defines hooks for each AST node type │
│  - Default: calls corresponding walk_*  │
└─────────────────────────────────────────┘
                ▲
                │ implements
                │
┌─────────────────────────────────────────┐
│  Your Custom Visitor                    │
│  - Override specific visit_* methods    │
│  - Collect data, validate, analyze, etc │
└─────────────────────────────────────────┘
                │
                │ calls
                ▼
┌─────────────────────────────────────────┐
│  walk_* Functions                       │
│  - Handle recursive traversal           │
│  - Call visitor hooks for child nodes   │
└─────────────────────────────────────────┘
```

## How It Works: Step-by-Step

### 1. The Visitor Trait

Located in `src/ast/visit.rs`:

```rust
pub trait Visitor<'ast> {
    fn visit_program(&mut self, program: &'ast Program) {
        walk_program(self, program);  // Default: just walk
    }

    fn visit_expr(&mut self, expr: &'ast Expression) {
        walk_expr(self, expr);  // Default: just walk
    }

    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        walk_stmt(self, stmt);
    }

    fn visit_pat(&mut self, pat: &'ast Pattern) {
        walk_pat(self, pat);
    }

    fn visit_match_arm(&mut self, arm: &'ast MatchArm) {
        walk_match_arm(self, arm);
    }

    fn visit_identifier(&mut self, _ident: &'ast Identifier) {}
}
```

**Key points:**
- `'ast` lifetime: Ensures visited nodes live long enough
- `&mut self`: Allows accumulating state (diagnostics, counters, etc.)
- Default implementations delegate to `walk_*` functions
- Override only the methods you care about

### 2. The Walk Functions

```rust
pub fn walk_expr<'ast, V: Visitor<'ast> + ?Sized>(
    visitor: &mut V,
    expr: &'ast Expression
) {
    match expr {
        Expression::Match { scrutinee, arms, span: _ } => {
            visitor.visit_expr(scrutinee);  // Recursively visit children
            for arm in arms {
                visitor.visit_match_arm(arm);
            }
        }
        Expression::Call { function, arguments, span: _ } => {
            visitor.visit_expr(function);
            for arg in arguments {
                visitor.visit_expr(arg);
            }
        }
        Expression::If { condition, consequence, alternative, span: _ } => {
            visitor.visit_expr(condition);
            visitor.visit_block(consequence);
            if let Some(alt) = alternative {
                visitor.visit_block(alt);
            }
        }
        // ... exhaustive match on all expression types
    }
}
```

**Key points:**
- Exhaustive pattern matching ensures all AST nodes are covered
- Recursively calls `visitor.visit_*` on child nodes
- Adding a new AST variant causes a compile error until this is updated
- You call this from your override to "continue" default traversal

### 3. Your Custom Visitor

Example from `src/syntax/pattern_validate.rs`:

```rust
struct PatternValidator<'a> {
    ctx: PatternValidationContext<'a>,
    diagnostics: Vec<Diagnostic>,  // Accumulates errors
}

impl<'ast> Visitor<'ast> for PatternValidator<'_> {
    fn visit_expr(&mut self, expr: &'ast Expression) {
        // 1. Intercept specific node types
        if let Expression::Match { arms, span, .. } = expr {
            // 2. Do custom validation
            validate_match_arms(arms, *span, &self.ctx, &mut self.diagnostics);

            for arm in arms {
                validate_pattern(&arm.pattern, &self.ctx, &mut self.diagnostics);
            }
        }

        // 3. Continue traversing children (IMPORTANT!)
        visit::walk_expr(self, expr);
    }
}
```

**Key points:**
- Override only the methods you care about
- Accumulate state in `self` (like `diagnostics`)
- Call `walk_expr` to continue traversing nested expressions

## Concrete Execution Example

Given this Flux code:

```flux
fun main() {
    match x {
        Some(y) => print(y),
        _ => print("none")
    };
}
```

**AST Structure (simplified):**
```
Program
└── Statement::Function (main)
    └── Block
        └── Statement::Expression
            └── Expression::Match
                ├── scrutinee: Expression::Identifier("x")
                └── arms: [
                    MatchArm { pattern: Some(y), body: Call(print, [y]) }
                    MatchArm { pattern: Wildcard, body: Call(print, ["none"]) }
                ]
```

**Execution Flow:**

```
1. validator.visit_program(&program)
   │
   └──> walk_program()
        └──> for stmt in statements:
             └──> visitor.visit_stmt(stmt)  // Statement::Function
                  │
                  └──> walk_stmt()
                       └──> visitor.visit_block(&body)
                            │
                            └──> walk_block()
                                 └──> visitor.visit_stmt(stmt)  // Statement::Expression
                                      │
                                      └──> walk_stmt()
                                           └──> visitor.visit_expr(&expression)  // Match!
                                                │
                                                ▼
                                   ┌────────────────────────────────────┐
                                   │ YOUR OVERRIDE EXECUTES HERE!       │
                                   │                                    │
2. visit_expr() override:          │ if let Expression::Match { .. } => │
   │                               │   validate_match_arms(...) ✓      │
   │                               │   validate_pattern(...) ✓         │
   └──> Checks: Is this a Match?  └────────────────────────────────────┘
        YES! Execute custom logic          │
                                           ▼
3. walk_expr(self, expr)  // Continue traversal
   │
   └──> match expr {
        Expression::Match { scrutinee, arms, .. } => {
            visitor.visit_expr(scrutinee)  // Visit identifier "x"
            for arm in arms {
                visitor.visit_match_arm(arm)  // Visit each arm
                │
                └──> walk_match_arm()
                     └──> visitor.visit_pat(&arm.pattern)
                     └──> visitor.visit_expr(&arm.body)  // Visit Call expressions
                          │
                          └──> walk_expr() again for nested calls
        }
```

## Why This Design is Powerful

### ❌ **Before Visitor Pattern** (Manual Recursion)

```rust
fn validate_patterns(expr: &Expression) {
    match expr {
        Expression::Match { scrutinee, arms, .. } => {
            // Validate match
            validate_match_arms(arms);

            // Manually recurse into children - EASY TO FORGET!
            validate_patterns(scrutinee);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    validate_patterns(guard);  // Don't forget guards!
                }
                validate_patterns(&arm.body);
            }
        }
        Expression::Call { function, arguments, .. } => {
            validate_patterns(function);  // Don't forget!
            for arg in arguments {
                validate_patterns(arg);  // Don't forget each arg!
            }
        }
        Expression::If { condition, consequence, alternative, .. } => {
            validate_patterns(condition);
            // Must manually traverse blocks...
            for stmt in &consequence.statements {
                // ... and so on
            }
        }
        // ... 20+ more expression types to handle manually!
    }
}
```

**Problems:**
- ❌ Must handle ALL expression types
- ❌ Easy to forget nested expressions
- ❌ Duplicated traversal logic across validators
- ❌ When AST changes, ALL validators break

### ✅ **With Visitor Pattern**

```rust
impl<'ast> Visitor<'ast> for PatternValidator<'_> {
    fn visit_expr(&mut self, expr: &'ast Expression) {
        if let Expression::Match { arms, .. } = expr {
            validate_match_arms(arms);  // Only care about matches
        }
        walk_expr(self, expr);  // Automatic traversal!
    }
}
```

**Benefits:**
- ✅ Override only what you care about
- ✅ `walk_expr` handles ALL expression types automatically
- ✅ Can't forget to visit children
- ✅ When AST changes, only `walk_*` functions need updating
- ✅ Reusable across multiple analysis passes

## The Lifetime Magic

```rust
impl<'ast> Visitor<'ast> for PatternValidator<'_> {
    //   ^^^^                                    ^^
    //   AST data lifetime                       Don't care about self lifetime

    fn visit_expr(&mut self, expr: &'ast Expression) {
        //                         ^^^^^ Borrows from AST

        // This is safe because:
        // 1. We only borrow AST data, don't store it in self
        // 2. The validator struct can have a different lifetime
        // 3. All AST references live for 'ast
    }
}
```

**Why two lifetimes?**
- `'ast`: How long AST nodes live
- `'_` (in `PatternValidator<'_>`): How long the context/data in the validator lives
- They're independent! Context might be shorter than AST

**What you CAN'T do:**
```rust
struct BadVisitor<'ast> {
    stored_expr: Option<&'ast Expression>,  // ❌ Don't store AST references
}
```

**What you SHOULD do:**
```rust
struct GoodVisitor {
    expr_count: usize,              // ✅ Accumulate data
    diagnostics: Vec<Diagnostic>,   // ✅ Collect results
    identifiers: HashSet<Symbol>,   // ✅ Track symbols (copy)
}
```

## Common Patterns

### Pattern 1: Counting Nodes

```rust
use crate::ast::{Visitor, walk_expr};
use crate::syntax::expression::Expression;

struct ExpressionCounter {
    count: usize,
}

impl<'ast> Visitor<'ast> for ExpressionCounter {
    fn visit_expr(&mut self, expr: &'ast Expression) {
        self.count += 1;
        walk_expr(self, expr);  // Continue to children
    }
}

// Usage:
let mut counter = ExpressionCounter { count: 0 };
counter.visit_program(&program);
println!("Total expressions: {}", counter.count);
```

### Pattern 2: Finding Specific Nodes

```rust
struct FunctionCallFinder {
    call_count: usize,
}

impl<'ast> Visitor<'ast> for FunctionCallFinder {
    fn visit_expr(&mut self, expr: &'ast Expression) {
        if let Expression::Call { .. } = expr {
            self.call_count += 1;
        }
        walk_expr(self, expr);
    }
}
```

### Pattern 3: Collecting Information

```rust
use std::collections::HashSet;
use crate::syntax::symbol::Symbol;

struct IdentifierCollector {
    identifiers: HashSet<Symbol>,
}

impl<'ast> Visitor<'ast> for IdentifierCollector {
    fn visit_identifier(&mut self, ident: &'ast Identifier) {
        self.identifiers.insert(*ident);  // Symbol is Copy
    }
}
```

### Pattern 4: Validation (Production Example)

From `src/syntax/pattern_validate.rs`:

```rust
struct PatternValidator<'a> {
    ctx: PatternValidationContext<'a>,
    diagnostics: Vec<Diagnostic>,
}

impl<'ast> Visitor<'ast> for PatternValidator<'_> {
    fn visit_expr(&mut self, expr: &'ast Expression) {
        if let Expression::Match { arms, span, .. } = expr {
            // Validate match exhaustiveness
            validate_match_arms(arms, *span, &self.ctx, &mut self.diagnostics);

            // Validate each pattern
            for arm in arms {
                validate_pattern(&arm.pattern, &self.ctx, &mut self.diagnostics);
            }
        }

        walk_expr(self, expr);
    }
}

// Public API:
pub fn validate_program_patterns(
    program: &Program,
    file_path: &str,
    interner: &Interner,
) -> Vec<Diagnostic> {
    let ctx = PatternValidationContext::new(file_path, interner);
    let mut validator = PatternValidator {
        ctx,
        diagnostics: Vec::new(),
    };
    validator.visit_program(program);
    validator.diagnostics
}
```

### Pattern 5: Pre-order vs Post-order Traversal

```rust
impl<'ast> Visitor<'ast> for MyVisitor {
    fn visit_expr(&mut self, expr: &'ast Expression) {
        // ========== PRE-ORDER ==========
        // Process THIS node before children
        println!("Entering: {:?}", expr);

        walk_expr(self, expr);  // Visit children

        // ========== POST-ORDER ==========
        // Process THIS node after children
        println!("Leaving: {:?}", expr);
    }
}
```

**Use cases:**
- **Pre-order**: Symbol table building, pushing scopes
- **Post-order**: Type checking, popping scopes, cleanup

### Pattern 6: Context/Scope Tracking

```rust
struct ScopeTracker {
    scope_depth: usize,
}

impl<'ast> Visitor<'ast> for ScopeTracker {
    fn visit_block(&mut self, block: &'ast Block) {
        self.scope_depth += 1;  // Enter scope
        walk_block(self, block);
        self.scope_depth -= 1;  // Exit scope
    }

    fn visit_identifier(&mut self, ident: &'ast Identifier) {
        println!("Identifier at depth {}: {:?}", self.scope_depth, ident);
    }
}
```

## Best Practices

### ✅ DO

1. **Always call the corresponding `walk_*` function** unless you want to skip subtrees:
   ```rust
   fn visit_expr(&mut self, expr: &'ast Expression) {
       // ... your logic ...
       walk_expr(self, expr);  // ✅ Don't forget this!
   }
   ```

2. **Accumulate data in the visitor struct:**
   ```rust
   struct MyVisitor {
       diagnostics: Vec<Diagnostic>,  // ✅ Good
       count: usize,                  // ✅ Good
   }
   ```

3. **Use context patterns for read-only data:**
   ```rust
   struct MyVisitor<'a> {
       ctx: &'a MyContext,  // ✅ Good - borrowed context
       results: Vec<Thing>, // ✅ Good - owned results
   }
   ```

4. **Provide a clean public API:**
   ```rust
   pub fn analyze_program(program: &Program) -> Analysis {
       let mut visitor = MyVisitor::new();
       visitor.visit_program(program);
       visitor.into_analysis()
   }
   ```

### ❌ DON'T

1. **Don't store AST references in the visitor:**
   ```rust
   struct BadVisitor<'ast> {
       expr: Option<&'ast Expression>,  // ❌ Bad - lifetime issues
   }
   ```

2. **Don't duplicate traversal logic:**
   ```rust
   fn visit_expr(&mut self, expr: &'ast Expression) {
       match expr {
           Expression::Call { function, arguments, .. } => {
               self.visit_expr(function);  // ❌ Bad - duplicates walk_expr
               for arg in arguments {
                   self.visit_expr(arg);
               }
           }
           // ...
       }
   }
   ```
   Use `walk_expr` instead!

3. **Don't forget the `?Sized` bound when taking visitors as parameters:**
   ```rust
   // ❌ Bad:
   fn my_helper<V: Visitor>(visitor: &mut V) { }

   // ✅ Good:
   fn my_helper<V: Visitor + ?Sized>(visitor: &mut V) { }
   ```

## Advanced: Short-Circuiting with TryVisitor

For cases where you need early-exit (find first error, search for specific node):

```rust
use std::ops::ControlFlow;
use crate::ast::TryVisitor;

struct FirstCallFinder;

impl<'ast> TryVisitor<'ast> for FirstCallFinder {
    type Break = &'ast Expression;

    fn visit_expr(&mut self, expr: &'ast Expression) -> ControlFlow<Self::Break> {
        if let Expression::Call { .. } = expr {
            return ControlFlow::Break(expr);  // Found it! Stop traversal.
        }
        try_walk_expr(self, expr)  // Continue until Break
    }
}
```

See `src/ast/visit.rs` for full `TryVisitor` implementation.

## Comparison with Folder Pattern

The Flux compiler also provides a `Folder` pattern for **owned transformations**:

| Pattern | Use Case | Input | Output | Example |
|---------|----------|-------|--------|---------|
| **Visitor** | Read-only analysis | `&'ast T` | `()` or accumulated data | Validation, linting, counting |
| **Folder** | AST transformation | `T` (owned) | `T` (owned) | Desugaring, optimization |

**Folder example:**
```rust
use crate::ast::{Folder, fold_expr};

struct ConstantFolder;

impl Folder for ConstantFolder {
    fn fold_expr(&mut self, expr: Expression) -> Expression {
        let expr = fold_expr(self, expr);  // Fold children first

        // Constant folding: 2 + 3 => 5
        match expr {
            Expression::Infix { left, operator, right, span }
                if matches!((&*left, &*right),
                    (Expression::Integer{..}, Expression::Integer{..})) =>
            {
                // Evaluate and return new expression
                evaluate_const_binop(*left, operator, *right, span)
            }
            _ => expr,
        }
    }
}
```

## Current Uses in Flux

| File | Visitor | Purpose |
|------|---------|---------|
| `syntax/pattern_validate.rs` | `PatternValidator` | Validate match exhaustiveness, duplicate bindings |
| `syntax/linter.rs` | (Could migrate) | Unused variables, shadowing, naming conventions |
| (Future) | `SymbolCollector` | Build symbol tables |
| (Future) | `TypeChecker` | Type inference and validation |

## Performance Considerations

- **Visitor overhead is minimal**: Trait method calls inline aggressively
- **Single-pass design**: Visit each node exactly once
- **Stack usage**: Recursive; deep nesting can cause stack overflow (rare in practice)
- **Alternative for deep trees**: Consider iterative traversal with explicit stack

## Summary

The visitor pattern gives you:
- ✅ **Automatic traversal** - don't manually recurse
- ✅ **Override only what you need** - ignore the rest
- ✅ **Type-safe** - compiler ensures exhaustiveness
- ✅ **Reusable** - one implementation, many uses
- ✅ **Maintainable** - AST changes update `walk_*`, visitors keep working
- ✅ **Composable** - multiple visitors for different concerns

## Further Reading

- `src/ast/visit.rs` - Full visitor implementation
- `src/ast/fold.rs` - Folder pattern for transformations
- `src/syntax/pattern_validate.rs` - Production visitor example
- `docs/proposals/007_visitor_pattern.md` - Original design proposal
