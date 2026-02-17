# Proposal 022: AST Traversal Framework

**Status:** Complete (All 5 phases done)
**Priority:** High
**Created:** 2026-02-11
**Supersedes:** Proposal 007 (Visitor Pattern for Multi-Pass Compilation)
**Related:** Proposal 010 (Advanced Linter), Proposal 016 (Tail Call Optimization)

## Overview

A rustc-style AST traversal framework providing two traits — `Visitor<'ast>` for read-only analysis and `Folder` for AST-to-AST rewriting — with centralized `walk_*`/`fold_*` free functions that exhaustively destructure every AST node. Adding a new field or variant to any AST type causes a compile error until the traversal code is updated.

## Architectural Scope

The traversal framework is scoped to **syntax-level (AST) passes only**. Other compiler layers get their own inspection mechanisms:

| Layer | Mechanism | Scope |
|-------|-----------|-------|
| **Syntax (AST)** | `Visitor` + `Folder` traits | This proposal |
| **Bytecode** | Pass-oriented bytecode inspection | Future proposal |
| **Runtime** | Instrumentation hooks | Future proposal |

This separation keeps each layer's traversal aligned with its data structures. The AST Visitor operates on `Expression`/`Statement`/`Pattern` enums; bytecode inspection would operate on `OpCode` sequences; runtime hooks would intercept `Value` operations.

## What Was Implemented

### Module Structure

```
src/ast/
├── mod.rs    # Module declarations, re-exports
├── visit.rs  # Visitor trait + walk_* functions (296 lines)
└── fold.rs   # Folder trait + fold_* functions (255 lines)
```

Registered in `src/lib.rs` as `pub mod ast`.

### Visitor Trait (Read-Only Traversal)

```rust
pub trait Visitor<'ast> {
    fn visit_program(&mut self, program: &'ast Program);
    fn visit_block(&mut self, block: &'ast Block);
    fn visit_stmt(&mut self, stmt: &'ast Statement);
    fn visit_expr(&mut self, expr: &'ast Expression);
    fn visit_pat(&mut self, pat: &'ast Pattern);
    fn visit_match_arm(&mut self, arm: &'ast MatchArm);
    fn visit_string_part(&mut self, part: &'ast StringPart);
    fn visit_identifier(&mut self, ident: &'ast Identifier);
}
```

- Every method defaults to calling its corresponding `walk_*` free function
- `walk_*` functions use exhaustive destructuring — compile-time safety on AST changes
- Generic over `V: Visitor<'ast> + ?Sized` — no dynamic dispatch required
- `visit_identifier` is a no-op leaf by default; override it to intercept all identifier references

### Folder Trait (AST Rewriting)

```rust
pub trait Folder {
    fn fold_program(&mut self, program: Program) -> Program;
    fn fold_block(&mut self, block: Block) -> Block;
    fn fold_stmt(&mut self, stmt: Statement) -> Statement;
    fn fold_expr(&mut self, expr: Expression) -> Expression;
    fn fold_pat(&mut self, pat: Pattern) -> Pattern;
    fn fold_match_arm(&mut self, arm: MatchArm) -> MatchArm;
    fn fold_string_part(&mut self, part: StringPart) -> StringPart;
    fn fold_identifier(&mut self, ident: Identifier) -> Identifier;
}
```

- Takes owned nodes, returns owned nodes — zero clones, uses `into_iter()` throughout
- Every method defaults to calling its corresponding `fold_*` free function
- `fold_identifier` returns the identifier unchanged by default

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

## Completed Integration

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

### Constant Dependency Collection (`src/bytecode/module_constants/dependency.rs`)

**Before:** Manual `collect_constant_refs` function (34 lines) matching Expression variants with `_ => {}` wildcard — silently ignored new variants.

**After:** A `ConstRefCollector` Visitor:

```rust
struct ConstRefCollector<'a> {
    known_constants: &'a HashSet<Symbol>,
    refs: HashSet<Symbol>,
}

impl<'ast> Visitor<'ast> for ConstRefCollector<'_> {
    fn visit_expr(&mut self, expr: &'ast Expression) {
        match expr {
            Expression::Identifier { name, .. } if self.known_constants.contains(name) => {
                self.refs.insert(*name);
            }
            _ => {}
        }
        visit::walk_expr(self, expr);
    }
}
```

**Result:** Replaced 34-line manual walker. The `visit_expr` override only collects `Expression::Identifier` nodes in expression position — bare `Identifier` fields (function parameters, `MemberAccess.member`) are routed through `visit_identifier` (default no-op) by `walk_expr`, so no false dependencies are introduced.

## New Passes Enabled by the Framework

### Visitor-Based Passes (Read-Only Analysis)

#### 1. Free Variable Collector

Identifies variables captured by closures — needed for closure optimization or future closure-conversion passes.

```rust
struct FreeVarCollector {
    scopes: Vec<HashSet<Symbol>>,
    free: HashSet<Symbol>,
}

impl<'ast> Visitor<'ast> for FreeVarCollector {
    fn visit_expr(&mut self, expr: &'ast Expression) {
        match expr {
            Expression::Identifier { name, .. } => {
                if !self.scopes.iter().rev().any(|s| s.contains(name)) {
                    self.free.insert(*name);
                }
            }
            Expression::Function { parameters, .. } => {
                self.scopes.push(parameters.iter().copied().collect());
                walk_expr(self, expr);
                self.scopes.pop();
                return;
            }
            _ => {}
        }
        walk_expr(self, expr);
    }
}
```

#### 2. Tail Position Analyzer

Tags which `Call` expressions are in tail position. Feeds into the existing tail-call optimization (Proposal 016).

```rust
struct TailCallFinder {
    in_tail: bool,
    tail_calls: Vec<Span>,
}

impl<'ast> Visitor<'ast> for TailCallFinder {
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::Return { value: Some(expr), .. } => {
                self.in_tail = true;
                self.visit_expr(expr);
                self.in_tail = false;
            }
            _ => walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &'ast Expression) {
        if self.in_tail {
            if let Expression::Call { span, .. } = expr {
                self.tail_calls.push(*span);
            }
        }
        let was_tail = self.in_tail;
        self.in_tail = false;
        walk_expr(self, expr);
        self.in_tail = was_tail;
    }
}
```

#### 3. Complexity Metrics

Computes per-function metrics: nesting depth, cyclomatic complexity, number of match arms. Useful for linter warnings or IDE hints.

```rust
struct ComplexityAnalyzer {
    depth: usize,
    max_depth: usize,
    branches: usize,  // cyclomatic complexity
}

impl<'ast> Visitor<'ast> for ComplexityAnalyzer {
    fn visit_expr(&mut self, expr: &'ast Expression) {
        match expr {
            Expression::If { .. } => {
                self.branches += 1;
                self.depth += 1;
                self.max_depth = self.max_depth.max(self.depth);
                walk_expr(self, expr);
                self.depth -= 1;
                return;
            }
            Expression::Match { arms, .. } => {
                self.branches += arms.len().saturating_sub(1);
                self.depth += 1;
                self.max_depth = self.max_depth.max(self.depth);
                walk_expr(self, expr);
                self.depth -= 1;
                return;
            }
            _ => {}
        }
        walk_expr(self, expr);
    }
}
```

#### 4. Import Graph Extraction

Collects all import symbols from a program without full module resolution. Useful for fast dependency analysis in watch mode or IDE integration.

```rust
struct ImportCollector {
    imports: Vec<(Symbol, Option<Symbol>)>,
}

impl<'ast> Visitor<'ast> for ImportCollector {
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        if let Statement::Import { name, alias, .. } = stmt {
            self.imports.push((*name, *alias));
        }
        walk_stmt(self, stmt);
    }
}
```

#### 5. Source Map Builder

Collects span-to-node mappings for IDE integration (hover, go-to-definition, highlight references).

```rust
struct SourceMapBuilder {
    expr_spans: Vec<(Span, ExprKind)>,
    ident_refs: Vec<(Span, Symbol)>,
}

impl<'ast> Visitor<'ast> for SourceMapBuilder {
    fn visit_expr(&mut self, expr: &'ast Expression) {
        self.expr_spans.push((expr.span(), classify(expr)));
        walk_expr(self, expr);
    }

    fn visit_identifier(&mut self, ident: &'ast Identifier) {
        // Collect all identifier reference locations
        self.ident_refs.push((/* span from context */, *ident));
    }
}
```

### Folder-Based Passes (AST Rewriting)

#### 6. Constant Folding

Evaluates compile-time-constant expressions, reducing `1 + 2` to `3`:

```rust
struct ConstantFolder;

impl Folder for ConstantFolder {
    fn fold_expr(&mut self, expr: Expression) -> Expression {
        let expr = fold_expr(self, expr); // fold children first
        match &expr {
            Expression::Infix {
                left: box Expression::Integer { value: a, .. },
                operator,
                right: box Expression::Integer { value: b, .. },
                span,
            } => {
                let result = match operator.as_str() {
                    "+" => Some(a + b),
                    "-" => Some(a - b),
                    "*" => Some(a * b),
                    "/" if *b != 0 => Some(a / b),
                    _ => None,
                };
                match result {
                    Some(value) => Expression::Integer { value, span: *span },
                    None => expr,
                }
            }
            _ => expr,
        }
    }
}
```

#### 7. Identifier Renaming

Systematically renames identifiers — useful for hygiene in macro expansion or minification:

```rust
struct Renamer {
    map: HashMap<Symbol, Symbol>,
}

impl Folder for Renamer {
    fn fold_identifier(&mut self, ident: Identifier) -> Identifier {
        self.map.get(&ident).copied().unwrap_or(ident)
    }
}
```

#### 8. Desugaring

Rewrites syntactic sugar into core constructs. For example, if pipe operators (`a |> f`) were parsed as `Expression::Infix` with operator `"|>"`, a desugaring pass could rewrite them:

```rust
struct PipeDesugarer;

impl Folder for PipeDesugarer {
    fn fold_expr(&mut self, expr: Expression) -> Expression {
        let expr = fold_expr(self, expr); // fold children first
        match expr {
            Expression::Infix {
                left, operator, right, span,
            } if operator == "|>" => {
                Expression::Call {
                    function: right,
                    arguments: vec![*left],
                    span,
                }
            }
            other => other,
        }
    }
}
```

## How It Works

### Traversal Flow

```
visitor.visit_program(program)
  └─ walk_program       // iterates program.statements
       └─ visit_stmt    // YOUR override runs here
            └─ walk_stmt // destructures Statement, recurses
                 └─ visit_expr   // YOUR override runs here
                      └─ walk_expr   // destructures Expression, recurses
                           └─ visit_block
                                └─ walk_block
                                     └─ visit_stmt → ...
```

Every `visit_*` defaults to `walk_*`. Every `walk_*` destructures the node and calls `visit_*` on children. Override the hook you care about, do your work, call `walk_*` to continue descent.

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

### Compile-Time Safety

If a new variant `Expression::Range { start, end, step, span }` is added to the AST:

1. `walk_expr` in `visit.rs` fails to compile (non-exhaustive match)
2. `fold_expr` in `fold.rs` fails to compile (non-exhaustive match)
3. Developer adds the new arm to both walkers
4. All existing Visitor/Folder implementations automatically handle the new variant through `walk_expr`/`fold_expr` — no changes needed unless they specifically care about `Range`

## Tests

### Existing Tests (11 total)

**`tests/ast_visit_smoke.rs`** — 6 tests with a `NodeCounter` visitor:
- `counts_simple_let` — verifies stmt/expr/ident counts for `let x = 1`
- `counts_infix_expression` — 3 exprs for `1 + 2`
- `counts_function_call` — function declaration + call traversal
- `counts_multiple_statements` — three let statements
- `counts_nested_if` — if/else block traversal
- `counts_array_elements` — array expression with elements

**`tests/ast_fold_smoke.rs`** — 5 tests with a `RenameIdent` folder:
- `rename_ident_in_let` — rewrites let binding name
- `rename_ident_in_expression` — rewrites identifier in infix expression
- `rename_preserves_other_idents` — only renames target, leaves others intact
- `rename_in_function_parameters` — renames params and body references
- `identity_fold_preserves_structure` — default Folder is a no-op

**`tests/pattern_validation.rs`** — 9 tests (pre-existing, validates the integration):
- Tests for empty match, catchall ordering, exhaustiveness, nested patterns, multi-error reporting

## Implementation Phases

### Phase 1: Foundation (Completed)

- [x] `src/ast/visit.rs` — Visitor trait with 8 hooks, 7 walk functions
- [x] `src/ast/fold.rs` — Folder trait with 8 hooks, 7 fold functions
- [x] `src/ast/mod.rs` — Module declarations and re-exports
- [x] Smoke tests for both traits
- [x] All 369+ tests passing

### Phase 2: Pattern Validator Integration (Completed)

- [x] Refactored `src/syntax/pattern_validate.rs` to use Visitor
- [x] Removed 119 lines of manual traversal boilerplate
- [x] Public API unchanged, all tests pass

### Phase 3: Linter & Dependency Integration (Completed)

- [x] Refactored `src/syntax/linter.rs` to use Visitor
- [x] Eliminated `lint_statement` (70 lines) and `lint_expression` (96 lines) manual walkers
- [x] Preserved scope management, dead-code detection, and all existing warnings
- [x] Refactored `src/bytecode/module_constants/dependency.rs` to use Visitor
- [x] Eliminated 34-line `collect_constant_refs` manual walker with `_ => {}` drift risk
- [x] All tests pass

### Phase 4: New Analysis Passes (Completed)

- [x] Free variable collector (enables closure optimization)
- [x] Complexity metrics (feeds into linter warnings W009/W010)
- [x] Tail position analyzer (feeds into Proposal 016)

### Phase 5: Folder-Based Rewriting Passes (Completed)

- [x] Constant folding (evaluate `1 + 2` → `3` at compile time)
- [x] Identifier renaming (for future macro hygiene)
- [x] Desugaring (for future syntactic sugar)

## Success Criteria

- [x] Compile-time safety: adding an AST variant fails the build until walkers are updated
- [x] No dynamic dispatch: all walkers accept `V: Visitor<'ast> + ?Sized`
- [x] Zero clones in Folder: owned-in → owned-out with `into_iter()`
- [x] AST nodes untouched: no `accept()` methods added
- [x] Backward compatible: existing code continues to work, integration is incremental
- [x] All existing tests pass after each integration

## Known Gaps vs rustc (Deferred)

These are intentional divergences from rustc's AST traversal. Each is deferred until a concrete pass needs it.

**1. No early-termination return type (`ControlFlow`)**
rustc's `Visitor` has `type Result: VisitorResult = ()` allowing `ControlFlow<T>` to bail out mid-traversal. Our `visit_*` methods always return `()` — every traversal is exhaustive. Adding this would require changing the Visitor trait signature, all walk functions, and all consumers. **Trigger:** a pass that needs "find first X and stop" semantics.

**2. No `flat_map_stmt` / node deletion in Folder**
rustc's `MutVisitor` provides `flat_map_stmt(Stmt) -> SmallVec<[Stmt; 1]>` allowing passes to delete or expand statements (essential for macro expansion). Our Folder is strictly 1:1 — every input node produces exactly one output. **Trigger:** a desugaring or dead-code-elimination pass that needs to remove or expand statements.

**3. Folder uses owned `T -> T` instead of in-place `&mut T`**
rustc's `MutVisitor` mutates nodes in place to avoid reallocating `NodeId`-bearing, interned AST nodes. Our Folder reconstructs nodes from owned values. This is the right trade-off for Flux: no `NodeId`s, no interning in AST nodes, enum-heavy types where in-place variant mutation is awkward. Not a gap to close — a deliberate design choice.

## References

- [rustc AST visitor](https://doc.rust-lang.org/nightly/nightly-rustc/rustc_ast/visit/index.html) — the model for this design
- [rustc AST fold (MutVisitor)](https://doc.rust-lang.org/nightly/nightly-rustc/rustc_ast/mut_visit/index.html) — owned-node rewriting approach
- Proposal 007 — original visitor proposal (superseded by this implementation)
- Proposal 010 — Advanced Linter (benefits from Visitor integration)
- Proposal 016 — Tail Call Optimization (benefits from tail position analysis pass)
