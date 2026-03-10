- Feature Name: AST Traversal Framework
- Start Date: 2026-02-11
- Status: Implemented
- Proposal PR:
- Flux Issue:

# Proposal 0022: AST Traversal Framework

## Summary
[summary]: #summary

A rustc-style AST traversal framework providing two traits — `Visitor<'ast>` for read-only analysis and `Folder` for AST-to-AST rewriting — with centralized `walk_*`/`fold_*` free functions that exhaustively destructure every AST node. Adding a new field or variant to any AST type causes a compile error until the traversal code is updated.

## Motivation
[motivation]: #motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Design Decisions

**1. No `accept()` methods on AST nodes.** AST types remain plain enums/structs. Traversal logic lives entirely in `walk_*`/`fold_*` free functions.

**2. Exhaustive destructuring in walkers.** Every `walk_*` and `fold_*` function binds or explicitly ignores (`_`) every field of every variant:

```rust
// If someone adds a new field to Statement::Let, this won't compile
Statement::Let { name, value, span: _ } => {
    visitor.visit_identifier(name);
    visitor.visit_expr(value);
}
```

**3. Two separate traits instead of one generic.** `Visitor` borrows nodes (`&'ast T`); `Folder` consumes and returns nodes (`T -> T`). Merging them would require cloning for read-only passes.

**4. Span fields are ignored (`span: _`) in walkers.** Spans carry no child nodes, so they pass through unchanged. The exhaustive binding ensures new non-span fields are caught.

### Pattern Validator (`src/syntax/pattern_validate.rs`)

**Before:** Three manual traversal functions (119 lines) that walked the entire AST just to find `Expression::Match` nodes:
- `validate_statement_patterns` — matched all 7 Statement variants
- `validate_block_patterns` — iterated block statements
- `validate_expression_patterns` — matched all 20+ Expression variants

**After:** A 12-line Visitor implementation:

```rust
struct PatternValidator<'a> {
    ctx: PatternValidationContext<'a>,
    diagnostics: Vec<Diagnostic>,
}

impl<'ast> Visitor<'ast> for PatternValidator<'_> {
    fn visit_expr(&mut self, expr: &'ast Expression) {
        if let Expression::Match { arms, span, .. } = expr {
            validate_match_arms(arms, *span, &self.ctx, &mut self.diagnostics);
            for arm in arms {
                validate_pattern(&arm.pattern, &self.ctx, &mut self.diagnostics);
            }
        }
        visit::walk_expr(self, expr);
    }
}
```

**Result:** 119 lines of boilerplate removed, public API unchanged, all 9 pattern validation tests pass.

### Linter (`src/syntax/linter.rs`)

**Before:** Manual `lint_statement` (70 lines, 7-variant match) and `lint_expression` (96 lines, 20+ variant match) methods with explicit recursion into every child node.

**After:** Implements `Visitor` with overrides for `visit_program`, `visit_block`, `visit_stmt`, and `visit_expr`:

```rust
impl<'ast, 'a> Visitor<'ast> for Linter<'a> {
    fn visit_program(&mut self, program: &'ast Program) {
        self.lint_block_statements(&program.statements);
    }

    fn visit_block(&mut self, block: &'ast Block) {
        self.lint_block_statements(&block.statements);
    }

    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::Let { .. } => { /* define binding + visit value */ }
            Statement::Assign { .. } => { /* mark_used + visit value */ }
            Statement::Function { .. } => { /* complexity check, scope mgmt */ }
            Statement::Module { .. } => { /* scope mgmt */ }
            Statement::Import { .. } => { /* naming check, define binding */ }
            Statement::Return { .. } | Statement::Expression { .. } => {
                visit::walk_stmt(self, stmt);
            }
        }
    }

    fn visit_expr(&mut self, expr: &'ast Expression) {
        match expr {
            Expression::Identifier { name, .. } => { self.mark_used(*name); }
            Expression::Function { .. } => { /* scope mgmt, complexity */ }
            Expression::Match { .. } => { /* scope per arm, pattern bindings */ }
            _ => visit::walk_expr(self, expr),
        }
    }
}
```

**Result:** Eliminated `lint_statement` and `lint_expression` methods entirely. Special cases (Identifier/Function/Match) have explicit handling; everything else falls through to `walk_stmt`/`walk_expr`. Dead-code detection preserved via `lint_block_statements` called from `visit_block`. All linter tests pass.

### Usage Pattern

```rust
use flux::ast::visit::{self, Visitor};

struct MyAnalysis { /* state */ }

impl<'ast> Visitor<'ast> for MyAnalysis {
    fn visit_expr(&mut self, expr: &'ast Expression) {
        // 1. Pre-order: inspect the node
        if let Expression::Match { .. } = expr {
            // ... do analysis ...
        }

        // 2. Recurse into children
        visit::walk_expr(self, expr);

        // 3. Post-order: finalize (optional)
    }
}

// Entry point
let mut analysis = MyAnalysis { /* ... */ };
analysis.visit_program(&program);
```

### Design Decisions

### Pattern Validator (`src/syntax/pattern_validate.rs`)

**After:** A 12-line Visitor implementation:

### Linter (`src/syntax/linter.rs`)

### Usage Pattern

### Design Decisions

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **Architectural Scope:** The traversal framework is scoped to **syntax-level (AST) passes only**. Other compiler layers get their own inspection mechanisms: - **Module Structu...
- **Architectural Scope:** The traversal framework is scoped to **syntax-level (AST) passes only**. Other compiler layers get their own inspection mechanisms: | Layer | Mechanism | Scope | |-------|------...
- **Module Structure:** ``` src/ast/ ├── mod.rs # Module declarations, re-exports ├── visit.rs # Visitor trait + walk_* functions (296 lines) └── fold.rs # Folder trait + fold_* functions (255 lines) ```
- **Visitor Trait (Read-Only Traversal):** - Every method defaults to calling its corresponding `walk_*` free function - `walk_*` functions use exhaustive destructuring — compile-time safety on AST changes - Generic over...
- **Folder Trait (AST Rewriting):** - Takes owned nodes, returns owned nodes — zero clones, uses `into_iter()` throughout - Every method defaults to calling its corresponding `fold_*` free function - `fold_identif...
- **Constant Dependency Collection (`src/bytecode/module_constants/dependency.rs`):** **Before:** Manual `collect_constant_refs` function (34 lines) matching Expression variants with `_ => {}` wildcard — silently ignored new variants.

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

1. Restructuring legacy material into a strict template can reduce local narrative flow.
2. Consolidation may temporarily increase document length due to historical preservation.
3. Additional review effort is required to keep synthesized sections aligned with implementation changes.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Design Decisions

**1. No `accept()` methods on AST nodes.** AST types remain plain enums/structs. Traversal logic lives entirely in `walk_*`/`fold_*` free functions.

**2. Exhaustive destructuring in walkers.** Every `walk_*` and `fold_*` function binds or explicitly ignores (`_`) every field of every variant:

```rust
// If someone adds a new field to Statement::Let, this won't compile
Statement::Let { name, value, span: _ } => {
    visitor.visit_identifier(name);
    visitor.visit_expr(value);
}
```

**3. Two separate traits instead of one generic.** `Visitor` borrows nodes (`&'ast T`); `Folder` consumes and returns nodes (`T -> T`). Merging them would require cloning for read-only passes.

**4. Span fields are ignored (`span: _`) in walkers.** Spans carry no child nodes, so they pass through unchanged. The exhaustive binding ensures new non-span fields are caught.

### Design Decisions

### Design Decisions

## Prior art
[prior-art]: #prior-art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### References

- [rustc AST visitor](https://doc.rust-lang.org/nightly/nightly-rustc/rustc_ast/visit/index.html) — the model for this design
- [rustc AST fold (MutVisitor)](https://doc.rust-lang.org/nightly/nightly-rustc/rustc_ast/mut_visit/index.html) — owned-node rewriting approach
- Proposal 0007 — original visitor proposal (superseded by this implementation)
- Proposal 0010 — Advanced Linter (benefits from Visitor integration)
- Proposal 0016 — Tail Call Optimization (benefits from tail position analysis pass)

### References

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions were explicitly listed in the legacy text.
- Follow-up questions should be tracked in Proposal PR and Flux Issue fields when created.

## Future possibilities
[future-possibilities]: #future-possibilities

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
