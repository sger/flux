# Proposal 012: Phase 2 - Advanced Module Split Plan

**Status:** Planning
**Priority:** Medium (Code Quality & Maintainability)
**Created:** 2026-02-04
**Depends on:** Phase 1 Module Split (Proposal 006) ✅

## Overview

Building on Phase 1's successful module organization, Phase 2 focuses on **advanced architectural patterns** and splitting remaining large files. This phase introduces more sophisticated patterns like builder patterns, visitor-based diagnostics, and command-driven CLI architecture.

## Problem Statement

### Achievements from Phase 1 ✅

Phase 1 successfully split the three largest files:
- ✅ `compiler.rs` (1,671 lines) → 5 modules (~300 lines each)
- ✅ `parser.rs` (1,144 lines) → 4 modules (~250 lines each)
- ✅ `vm.rs` (824 lines) → 6 modules (~150 lines each)
- ✅ Plus secondary splits: module_graph, builtins, bytecode_cache

### Remaining Issues (Phase 2 Targets)

**Files Still Too Large:**
1. **diagnostic.rs** - **1,412 lines** (CRITICAL - largest file in codebase!)
2. **compiler/expression.rs** - 789 lines (should be <500)
3. **main.rs** - 604 lines (CLI logic mixed with business logic)
4. **diagnostics/compiler_errors.rs** - 602 lines
5. **parser/expression.rs** - 588 lines
6. **diagnostics/aggregator.rs** - 579 lines
7. **linter.rs** - 402 lines

**Architectural Opportunities:**
- Diagnostics system could use builder pattern
- CLI could use command pattern
- Expression compilation could split by semantic groups
- Linter could split into separate passes

---

## Scope

### In Scope (Phase 2)

**Priority 1 (CRITICAL) - Diagnostics System:**
1. ✅ Split `diagnostic.rs` (1,412 lines) into focused modules
2. ✅ Split `compiler_errors.rs` (602 lines) by error category
3. ✅ Split `aggregator.rs` (579 lines) into aggregation + rendering

**Priority 2 (HIGH) - CLI Architecture:**
4. ✅ Split `main.rs` (604 lines) into command pattern
5. ✅ Extract CLI argument parsing
6. ✅ Extract command implementations

**Priority 3 (MEDIUM) - Expression Refinement:**
7. ✅ Further split `compiler/expression.rs` (789 lines)
8. ✅ Further split `parser/expression.rs` (588 lines)
9. ✅ Split `linter.rs` (402 lines) into passes

### Out of Scope
- ❌ New features or functionality changes
- ❌ Performance optimizations (covered in Proposal 011)
- ❌ Breaking API changes

---

## Detailed Plan

## 1. Diagnostics System Split (CRITICAL PRIORITY)

### Current State
```
src/syntax/diagnostics/
├── diagnostic.rs           # 1,412 lines (TOO BIG!)
├── compiler_errors.rs      # 602 lines (TOO BIG!)
├── runtime_errors.rs       # 177 lines
├── aggregator.rs           # 579 lines (TOO BIG!)
├── registry.rs             # 147 lines
└── types.rs                # 56 lines
```

### Problem Analysis

**diagnostic.rs contains:**
- Type definitions (Severity, HintKind, Hint, Label, etc.) - ~200 lines
- Hint builders and helpers - ~150 lines
- Diagnostic struct and methods - ~300 lines
- Rendering logic (render_diagnostics) - ~600 lines
- Source line extraction and formatting - ~162 lines

**This violates Single Responsibility Principle!**

### Proposed Split

```
src/syntax/diagnostics/
├── types/                  # Type definitions
│   ├── mod.rs              # Re-exports
│   ├── severity.rs         # Severity enum (30 lines)
│   ├── hint.rs             # Hint, HintKind, HintChain (200 lines)
│   ├── label.rs            # Label, LabelStyle (80 lines)
│   ├── suggestion.rs       # InlineSuggestion (60 lines)
│   └── related.rs          # RelatedDiagnostic, RelatedKind (70 lines)
│
├── builders/               # Builder pattern for diagnostics
│   ├── mod.rs              # Re-exports
│   ├── diagnostic_builder.rs   # DiagnosticBuilder (150 lines)
│   └── hint_builder.rs     # HintBuilder (80 lines)
│
├── rendering/              # Display and formatting
│   ├── mod.rs              # Re-exports
│   ├── renderer.rs         # Main rendering logic (250 lines)
│   ├── source.rs           # Source line extraction (150 lines)
│   ├── formatter.rs        # Text formatting helpers (100 lines)
│   └── colors.rs           # ANSI color codes (80 lines)
│
├── errors/                 # Error constructors
│   ├── mod.rs              # Re-exports
│   ├── parser_errors.rs    # Parser error constructors (200 lines)
│   ├── compiler_errors.rs  # Compiler error constructors (200 lines)
│   ├── module_errors.rs    # Module/import errors (150 lines)
│   └── runtime_errors.rs   # Runtime error constructors (177 lines - existing)
│
├── aggregator.rs           # Refactored aggregator (200 lines)
├── registry.rs             # Error code registry (147 lines - unchanged)
├── types.rs                # ErrorCode, ErrorType (56 lines - unchanged)
└── diagnostic.rs           # DEPRECATED - re-exports for compatibility
```

### Implementation Details

#### 1a. Extract Type Definitions

**Create `diagnostics/types/hint.rs`:**
```rust
use super::severity::Severity;
use crate::syntax::position::Span;

/// Kind of hint to display
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HintKind {
    Hint,
    Note,
    Help,
    Example,
}

/// A hint with optional source location and label
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    pub kind: HintKind,
    pub text: String,
    pub span: Option<Span>,
    pub label: Option<String>,
    pub file: Option<String>,
}

impl Hint {
    pub fn text(text: impl Into<String>) -> Self { /* ... */ }
    pub fn at(text: impl Into<String>, span: Span) -> Self { /* ... */ }
    pub fn labeled(text: impl Into<String>, span: Span, label: impl Into<String>) -> Self { /* ... */ }
    // ... other constructors
}
```

#### 1b. Create Builder Pattern

**Create `diagnostics/builders/diagnostic_builder.rs`:**
```rust
use crate::syntax::diagnostics::{Diagnostic, Hint, Label, Severity};
use crate::syntax::position::Span;

/// Builder for constructing Diagnostic instances with fluent API
pub struct DiagnosticBuilder {
    code: String,
    message: String,
    severity: Severity,
    file: Option<String>,
    labels: Vec<Label>,
    hints: Vec<Hint>,
    related: Vec<RelatedDiagnostic>,
}

impl DiagnosticBuilder {
    pub fn new(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: String::new(),
            severity: Severity::Error,
            file: None,
            labels: Vec::new(),
            hints: Vec::new(),
            related: Vec::new(),
        }
    }

    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }

    pub fn severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    pub fn file(mut self, file: impl Into<String>) -> Self {
        self.file = Some(file.into());
        self
    }

    pub fn with_label(mut self, span: Span, label: impl Into<String>) -> Self {
        self.labels.push(Label::primary(span, label));
        self
    }

    pub fn with_hint(mut self, hint: Hint) -> Self {
        self.hints.push(hint);
        self
    }

    pub fn build(self) -> Diagnostic {
        Diagnostic {
            code: self.code,
            message: self.message,
            severity: self.severity,
            file: self.file,
            labels: self.labels,
            hints: self.hints,
            related: self.related,
        }
    }
}

// Usage:
// DiagnosticBuilder::new("E101")
//     .message("Undefined variable")
//     .file("main.flx")
//     .with_label(span, "variable not found")
//     .with_hint(Hint::help("Did you mean `count`?"))
//     .build()
```

#### 1c. Extract Rendering Logic

**Create `diagnostics/rendering/renderer.rs`:**
```rust
use super::{source::SourceExtractor, formatter::DiagnosticFormatter};
use crate::syntax::diagnostics::Diagnostic;

pub struct DiagnosticRenderer {
    source_extractor: SourceExtractor,
    formatter: DiagnosticFormatter,
}

impl DiagnosticRenderer {
    pub fn new() -> Self {
        Self {
            source_extractor: SourceExtractor::new(),
            formatter: DiagnosticFormatter::new(),
        }
    }

    pub fn render(&self, diagnostic: &Diagnostic) -> String {
        let mut output = String::new();

        // Render header
        output.push_str(&self.formatter.render_header(diagnostic));

        // Render source lines with highlights
        for label in &diagnostic.labels {
            if let Some(source) = self.source_extractor.extract(diagnostic.file.as_ref(), label.span) {
                output.push_str(&self.formatter.render_source(source, label));
            }
        }

        // Render hints
        for hint in &diagnostic.hints {
            output.push_str(&self.formatter.render_hint(hint));
        }

        output
    }
}
```

**Create `diagnostics/rendering/source.rs`:**
```rust
use crate::syntax::position::Span;
use std::collections::HashMap;

pub struct SourceExtractor {
    cache: HashMap<String, String>,
}

impl SourceExtractor {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    pub fn extract(&mut self, file: Option<&String>, span: Span) -> Option<SourceContext> {
        let file = file?;
        let source = self.load_source(file)?;
        Some(SourceContext::from_source(source, span))
    }

    fn load_source(&mut self, file: &str) -> Option<&str> {
        if !self.cache.contains_key(file) {
            let content = std::fs::read_to_string(file).ok()?;
            self.cache.insert(file.to_string(), content);
        }
        self.cache.get(file).map(|s| s.as_str())
    }
}

pub struct SourceContext {
    pub line_number: usize,
    pub line_content: String,
    pub column_start: usize,
    pub column_end: usize,
}
```

#### 1d. Split Error Constructors by Category

**Create `diagnostics/errors/parser_errors.rs`:**
```rust
use crate::syntax::diagnostics::{Diagnostic, DiagnosticBuilder, Hint};
use crate::syntax::position::Span;

// Parsing errors (E001-E099)

pub fn expected_token_error(expected: &str, got: &str, span: Span) -> Diagnostic {
    DiagnosticBuilder::new("E001")
        .message(format!("Expected {}, got {}", expected, got))
        .with_label(span, "unexpected token")
        .with_hint(Hint::help(format!("Try using {} here", expected)))
        .build()
}

pub fn unexpected_eof_error(span: Span) -> Diagnostic {
    DiagnosticBuilder::new("E002")
        .message("Unexpected end of file")
        .with_label(span, "unexpected EOF")
        .build()
}

// ... more parser errors
```

**Create `diagnostics/errors/compiler_errors.rs`:**
```rust
// Compilation errors (E100-E199)

pub fn undefined_variable_error(name: &str, span: Span) -> Diagnostic {
    DiagnosticBuilder::new("E101")
        .message(format!("Undefined variable `{}`", name))
        .with_label(span, "not found in this scope")
        .with_hint(Hint::help("Did you forget to declare this variable?"))
        .build()
}

// ... more compiler errors
```

**Create `diagnostics/errors/module_errors.rs`:**
```rust
// Module/import errors (E200-E299)

pub fn import_not_found_error(module: &str, span: Span) -> Diagnostic {
    DiagnosticBuilder::new("E201")
        .message(format!("Cannot find module `{}`", module))
        .with_label(span, "module not found")
        .with_hint(Hint::help("Check that the module path is correct"))
        .build()
}

// ... more module errors
```

### Migration Strategy

1. Create new directory structure
2. Extract types first (no dependencies)
3. Extract builders
4. Extract rendering (most complex)
5. Split error constructors
6. Update `diagnostic.rs` to re-export everything
7. Gradually update imports across codebase
8. Remove old `diagnostic.rs` in v0.2.0

**Estimated Effort:** 1 week (5 days)

---

## 2. CLI Architecture Split (HIGH PRIORITY)

### Current State
```
src/
└── main.rs                 # 604 lines - everything mixed together
```

### Problem
- Argument parsing mixed with command logic
- Hard to test commands in isolation
- Difficult to add new commands
- Help text generation is manual

### Proposed Split

```
src/
├── cli/
│   ├── mod.rs              # CLI orchestrator (100 lines)
│   ├── args.rs             # Argument parsing (150 lines)
│   ├── help.rs             # Help text generation (80 lines)
│   └── commands/           # Command implementations
│       ├── mod.rs          # Command trait + registry (80 lines)
│       ├── run.rs          # Run command (120 lines)
│       ├── tokens.rs       # Tokens command (60 lines)
│       ├── bytecode.rs     # Bytecode command (70 lines)
│       ├── lint.rs         # Lint command (80 lines)
│       ├── fmt.rs          # Format command (90 lines)
│       ├── cache_info.rs   # Cache info command (80 lines)
│       └── repl.rs         # REPL command (100 lines)
│
└── main.rs                 # Entry point only (40 lines)
```

### Implementation

#### 2a. Command Pattern

**Create `cli/commands/mod.rs`:**
```rust
use std::path::PathBuf;

pub trait Command {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn usage(&self) -> &str;
    fn run(&self, args: &CommandArgs) -> Result<(), Box<dyn std::error::Error>>;
}

pub struct CommandArgs {
    pub file: Option<PathBuf>,
    pub verbose: bool,
    pub trace: bool,
    pub no_cache: bool,
    pub roots: Vec<PathBuf>,
    pub max_errors: usize,
    // ... other common args
}

pub struct CommandRegistry {
    commands: HashMap<String, Box<dyn Command>>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            commands: HashMap::new(),
        };

        // Register all commands
        registry.register(Box::new(RunCommand));
        registry.register(Box::new(TokensCommand));
        registry.register(Box::new(BytecodeCommand));
        registry.register(Box::new(LintCommand));
        registry.register(Box::new(FmtCommand));
        registry.register(Box::new(CacheInfoCommand));

        registry
    }

    pub fn register(&mut self, command: Box<dyn Command>) {
        self.commands.insert(command.name().to_string(), command);
    }

    pub fn get(&self, name: &str) -> Option<&dyn Command> {
        self.commands.get(name).map(|c| c.as_ref())
    }

    pub fn all_commands(&self) -> Vec<&dyn Command> {
        self.commands.values().map(|c| c.as_ref()).collect()
    }
}
```

#### 2b. Command Implementations

**Create `cli/commands/run.rs`:**
```rust
use super::{Command, CommandArgs};
use flux::syntax::parser::Parser;
use flux::runtime::vm::VM;

pub struct RunCommand;

impl Command for RunCommand {
    fn name(&self) -> &str {
        "run"
    }

    fn description(&self) -> &str {
        "Run a Flux source file"
    }

    fn usage(&self) -> &str {
        "flux run [OPTIONS] <file.flx>"
    }

    fn run(&self, args: &CommandArgs) -> Result<(), Box<dyn std::error::Error>> {
        let file = args.file.as_ref()
            .ok_or("No file specified")?;

        // Parse
        let source = std::fs::read_to_string(file)?;
        let lexer = Lexer::new(&source);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program()?;

        // Compile
        let mut compiler = Compiler::new();
        compiler.compile(&program)?;
        let bytecode = compiler.bytecode();

        // Run
        let mut vm = VM::new();
        if args.trace {
            vm.enable_trace();
        }
        vm.run(bytecode)?;

        Ok(())
    }
}
```

**Create `cli/commands/lint.rs`:**
```rust
pub struct LintCommand;

impl Command for LintCommand {
    fn name(&self) -> &str {
        "lint"
    }

    fn description(&self) -> &str {
        "Check code for potential issues"
    }

    fn usage(&self) -> &str {
        "flux lint <file.flx>"
    }

    fn run(&self, args: &CommandArgs) -> Result<(), Box<dyn std::error::Error>> {
        let file = args.file.as_ref().ok_or("No file specified")?;

        let source = std::fs::read_to_string(file)?;
        let lexer = Lexer::new(&source);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program()?;

        let mut linter = Linter::new();
        let diagnostics = linter.lint(&program)?;

        if !diagnostics.is_empty() {
            render_diagnostics_multi(&diagnostics, args.max_errors);
            return Err("Lint errors found".into());
        }

        println!("✓ No issues found");
        Ok(())
    }
}
```

#### 2c. Simplified main.rs

**New `main.rs` (40 lines):**
```rust
use flux::cli::{CommandRegistry, parse_args};

fn main() {
    let args = match parse_args(std::env::args().collect()) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("Error: {}", e);
            print_help();
            std::process::exit(1);
        }
    };

    let registry = CommandRegistry::new();

    // If no command specified, treat first arg as file and run it
    let command_name = if args.command.is_none() && args.file.is_some() {
        "run"
    } else {
        args.command.as_deref().unwrap_or("help")
    };

    let command = match registry.get(command_name) {
        Some(cmd) => cmd,
        None => {
            eprintln!("Unknown command: {}", command_name);
            print_help();
            std::process::exit(1);
        }
    };

    if let Err(e) = command.run(&args) {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
```

**Estimated Effort:** 3-4 days

---

## 3. Expression Compilation Refinement (MEDIUM PRIORITY)

### Current State
```
src/bytecode/compiler/
└── expression.rs           # 789 lines (still large)
```

### Problem
- All expression types in one file
- Hard to find specific expression logic
- Could group by semantic categories

### Proposed Split

```
src/bytecode/compiler/expression/
├── mod.rs                  # Main dispatcher (100 lines)
├── literals.rs             # Integer, Float, String, Boolean, Array, Hash (100 lines)
├── operators.rs            # Binary, Unary operations (120 lines)
├── control_flow.rs         # If, Match expressions (150 lines)
├── functions.rs            # Function literals, closures (120 lines)
├── calls.rs                # Function calls, member access (80 lines)
└── patterns.rs             # Pattern matching compilation (120 lines)
```

### Implementation

**Create `expression/mod.rs`:**
```rust
use crate::bytecode::compiler::Compiler;
use crate::syntax::expression::Expression;

mod literals;
mod operators;
mod control_flow;
mod functions;
mod calls;
mod patterns;

impl Compiler {
    pub fn compile_expression(&mut self, expr: &Expression) -> Result<()> {
        match expr {
            // Literals
            Expression::Integer { .. } => literals::compile_integer(self, expr),
            Expression::Float { .. } => literals::compile_float(self, expr),
            Expression::String { .. } => literals::compile_string(self, expr),
            Expression::Array { .. } => literals::compile_array(self, expr),
            Expression::Hash { .. } => literals::compile_hash(self, expr),

            // Operators
            Expression::Infix { .. } => operators::compile_infix(self, expr),
            Expression::Prefix { .. } => operators::compile_prefix(self, expr),

            // Control flow
            Expression::If { .. } => control_flow::compile_if(self, expr),
            Expression::Match { .. } => control_flow::compile_match(self, expr),

            // Functions
            Expression::FunctionLiteral { .. } => functions::compile_function_literal(self, expr),

            // Calls
            Expression::Call { .. } => calls::compile_call(self, expr),
            Expression::MemberAccess { .. } => calls::compile_member_access(self, expr),

            // ... other expression types
        }
    }
}
```

**Create `expression/control_flow.rs`:**
```rust
use crate::bytecode::compiler::Compiler;
use crate::syntax::expression::Expression;
use crate::bytecode::op_code::OpCode;

pub fn compile_if(compiler: &mut Compiler, expr: &Expression) -> Result<()> {
    if let Expression::If { condition, consequence, alternative, .. } = expr {
        // Compile condition
        compiler.compile_expression(condition)?;

        // Jump if false
        let jump_if_false_pos = compiler.emit(OpCode::JumpIfFalse, &[9999]);

        // Compile consequence
        compiler.compile_block_statement(consequence)?;

        // ... rest of if compilation logic
    }
    Ok(())
}

pub fn compile_match(compiler: &mut Compiler, expr: &Expression) -> Result<()> {
    if let Expression::Match { scrutinee, arms, .. } = expr {
        // Compile scrutinee
        compiler.compile_expression(scrutinee)?;

        // Compile each arm
        for arm in arms {
            // ... match compilation logic
        }
    }
    Ok(())
}
```

**Estimated Effort:** 2-3 days

---

## 4. Linter Split by Passes (MEDIUM PRIORITY)

### Current State
```
src/syntax/
└── linter.rs               # 402 lines - all lint checks mixed
```

### Proposed Split

```
src/syntax/linter/
├── mod.rs                  # Linter orchestrator (100 lines)
├── passes/                 # Individual lint passes
│   ├── mod.rs              # Pass trait + registry (60 lines)
│   ├── unused_vars.rs      # Unused variable detection (80 lines)
│   ├── unused_imports.rs   # Unused import detection (60 lines)
│   ├── shadowing.rs        # Variable shadowing checks (80 lines)
│   ├── naming.rs           # Naming convention checks (70 lines)
│   └── dead_code.rs        # Dead code detection (70 lines)
└── visitor.rs              # AST visitor for linting (80 lines)
```

### Implementation

**Create `linter/passes/mod.rs`:**
```rust
use crate::syntax::{program::Program, diagnostics::Diagnostic};

pub trait LintPass {
    fn name(&self) -> &str;
    fn run(&mut self, program: &Program) -> Vec<Diagnostic>;
}

pub struct LintPassRegistry {
    passes: Vec<Box<dyn LintPass>>,
}

impl LintPassRegistry {
    pub fn default_passes() -> Self {
        let mut registry = Self { passes: Vec::new() };

        registry.register(Box::new(UnusedVarsPass::new()));
        registry.register(Box::new(UnusedImportsPass::new()));
        registry.register(Box::new(ShadowingPass::new()));
        registry.register(Box::new(NamingPass::new()));

        registry
    }

    pub fn register(&mut self, pass: Box<dyn LintPass>) {
        self.passes.push(pass);
    }

    pub fn run_all(&mut self, program: &Program) -> Vec<Diagnostic> {
        let mut all_diagnostics = Vec::new();

        for pass in &mut self.passes {
            let diagnostics = pass.run(program);
            all_diagnostics.extend(diagnostics);
        }

        all_diagnostics
    }
}
```

**Create `linter/passes/unused_vars.rs`:**
```rust
use super::LintPass;
use crate::syntax::{program::Program, diagnostics::Diagnostic};
use std::collections::HashSet;

pub struct UnusedVarsPass {
    declared: HashSet<String>,
    used: HashSet<String>,
}

impl LintPass for UnusedVarsPass {
    fn name(&self) -> &str {
        "unused-vars"
    }

    fn run(&mut self, program: &Program) -> Vec<Diagnostic> {
        self.collect_declared_and_used(program);

        let unused: Vec<_> = self.declared
            .difference(&self.used)
            .collect();

        unused.iter()
            .map(|var| self.create_warning(var))
            .collect()
    }
}

impl UnusedVarsPass {
    pub fn new() -> Self {
        Self {
            declared: HashSet::new(),
            used: HashSet::new(),
        }
    }

    fn collect_declared_and_used(&mut self, program: &Program) {
        // Walk AST and collect declarations and usages
        // ...
    }

    fn create_warning(&self, var: &str) -> Diagnostic {
        DiagnosticBuilder::new("W201")
            .message(format!("Unused variable `{}`", var))
            .severity(Severity::Warning)
            .build()
    }
}
```

**Create `linter/mod.rs`:**
```rust
use crate::syntax::{program::Program, diagnostics::Diagnostic};
use passes::LintPassRegistry;

mod passes;
mod visitor;

pub struct Linter {
    passes: LintPassRegistry,
}

impl Linter {
    pub fn new() -> Self {
        Self {
            passes: LintPassRegistry::default_passes(),
        }
    }

    pub fn lint(&mut self, program: &Program) -> Result<Vec<Diagnostic>, Box<Diagnostic>> {
        Ok(self.passes.run_all(program))
    }
}
```

**Estimated Effort:** 2 days

---

## Implementation Roadmap

### Week 1: Diagnostics System (Priority 1)
**Deliverable:** Clean, modular diagnostics architecture

- **Day 1-2:** Extract type definitions
  - Create `types/` directory
  - Move Severity, HintKind, Hint, Label, etc.
  - Update imports

- **Day 3:** Create builder pattern
  - Implement DiagnosticBuilder
  - Implement HintBuilder

- **Day 4-5:** Extract rendering logic
  - Create `rendering/` directory
  - Split into renderer, source, formatter, colors
  - Test rendering still works

### Week 2: Error Constructors & CLI (Priority 1-2)
**Deliverable:** Categorized errors, command-based CLI

- **Day 6-7:** Split error constructors
  - Create `errors/` directory
  - Group by category (parser, compiler, module, runtime)
  - Update all call sites

- **Day 8-9:** CLI command pattern
  - Create `cli/commands/` structure
  - Implement Command trait
  - Extract run, tokens, bytecode commands

- **Day 10:** Finish CLI migration
  - Extract remaining commands (lint, fmt, cache-info)
  - Simplify main.rs
  - Test all commands work

### Week 3: Expression & Linter Refinement (Priority 3)
**Deliverable:** Better organized expression compilation and linting

- **Day 11-13:** Split expression compilation
  - Create `expression/` subdirectory
  - Group by semantic categories
  - Test compilation still works

- **Day 14-15:** Split linter into passes
  - Create `linter/passes/` structure
  - Implement LintPass trait
  - Extract individual passes

---

## Success Metrics

### Code Quality
- ✅ **Largest file < 400 lines** (down from 1,412 lines)
- ✅ **Average module size: 100-200 lines**
- ✅ **Clear separation of concerns**
- ✅ **Builder pattern for complex objects**

### Maintainability
- ✅ **Diagnostics easily extensible** (add new types without touching core)
- ✅ **CLI commands independently testable**
- ✅ **Linter passes independently testable**
- ✅ **Expression compilation easier to understand**

### Developer Experience
- ✅ **Easier to add new commands**
- ✅ **Easier to add new lint passes**
- ✅ **Easier to add new error types**
- ✅ **Clear navigation (file name = responsibility)**

### Stability
- ✅ **100% backward compatibility** (re-exports)
- ✅ **All tests pass**
- ✅ **No performance regressions**

---

## File Size Comparison

### Before Phase 2
| File | Lines | Status |
|------|-------|--------|
| diagnostic.rs | 1,412 | 🔴 CRITICAL |
| compiler/expression.rs | 789 | 🟡 TOO LARGE |
| main.rs | 604 | 🟡 TOO LARGE |
| compiler_errors.rs | 602 | 🟡 TOO LARGE |
| parser/expression.rs | 588 | 🟡 TOO LARGE |
| aggregator.rs | 579 | 🟡 TOO LARGE |
| linter.rs | 402 | 🟡 LARGE |

**Total: 4,976 lines** in 7 files

### After Phase 2
| Component | Files | Avg Lines/File | Total Lines |
|-----------|-------|----------------|-------------|
| Diagnostics types | 5 | 88 | 440 |
| Diagnostics builders | 2 | 115 | 230 |
| Diagnostics rendering | 4 | 145 | 580 |
| Error constructors | 4 | 150 | 600 |
| CLI commands | 8 | 85 | 680 |
| Expression compilation | 6 | 120 | 720 |
| Linter passes | 6 | 75 | 450 |

**Total: ~4,700 lines** in 35 focused modules
**Savings:** 276 lines (better organization, less duplication)
**Average file size:** 134 lines (vs 710 lines before)

---

## Risks and Mitigation

### Risk 1: Import Churn
**Likelihood:** High
**Impact:** Low
**Mitigation:**
- Keep old files as re-export wrappers
- Gradual migration, not big bang
- Use IDE refactoring tools

### Risk 2: Breaking Changes
**Likelihood:** Low
**Impact:** High
**Mitigation:**
- All old imports still work (re-exports)
- Deprecation warnings in v0.1.x
- Remove old structure in v0.2.0

### Risk 3: Over-Splitting
**Likelihood:** Medium
**Impact:** Low
**Mitigation:**
- Follow Single Responsibility Principle
- Group related functionality
- Don't split below 50 lines

---

## Future Considerations (Phase 3+)

### Potential Future Splits
- **AST types** - Split expression.rs, statement.rs into semantic groups
- **Bytecode generation** - Further split compiler/builder.rs
- **VM instructions** - Split op_code.rs by instruction category

### Architectural Patterns to Consider
- **Visitor pattern** for AST traversal (Proposal 007)
- **Strategy pattern** for different backends
- **Factory pattern** for object creation

---

## References

- [Phase 1 Module Split](006_phase1_module_split_plan.md)
- [Visitor Pattern Proposal](007_visitor_pattern.md)
- [Compiler Architecture](../architecture/compiler_architecture.md)
- Rust API Guidelines: [Module Organization](https://rust-lang.github.io/api-guidelines/organization.html)

---

## Approval Checklist

- [ ] Diagnostics split strategy approved
- [ ] CLI command pattern approved
- [ ] Expression split categories agreed upon
- [ ] Linter pass pattern approved
- [ ] Timeline agreed upon (3 weeks)
- [ ] Migration strategy approved
- [ ] Ready to implement

---

**Next Steps:**
1. Review and approve proposal
2. Begin Week 1: Diagnostics system split
3. Track progress with test coverage
4. Measure file size reduction
