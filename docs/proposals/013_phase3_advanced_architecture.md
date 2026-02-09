# Proposal 013: Phase 3 - Advanced Architecture & Future Foundations

**Status:** Planning
**Priority:** Medium-Low (Future-Proofing)
**Created:** 2026-02-04
**Depends on:**
- Phase 1 Module Split (Proposal 006) ✅
- Phase 2 Advanced Module Split (Proposal 012) ⏳

## Overview

Phase 3 focuses on **architectural foundations for future features** rather than immediate code organization. This phase introduces advanced patterns (Visitor, Symbol Interning), prepares for a type system, builds tooling infrastructure (LSP, debugger), and implements performance optimizations.

## Problem Statement

### Achievements from Phase 1 & 2 ✅

**Phase 1:** Split monolithic files into focused modules
**Phase 2:** Introduced advanced patterns (builders, commands, passes)

### Remaining Architectural Opportunities (Phase 3)

**1. Visitor Pattern for Multi-Pass Compilation**
- Currently: Compiler mixes traversal with logic
- Needed for: Type checking, optimization passes, linting
- Referenced in: Proposal 007

**2. Symbol Interning for Performance**
- Currently: String-based identifiers everywhere
- Needed for: Memory efficiency, faster comparisons, type system
- Referenced in: Proposal 005

**3. Type System Foundations**
- Currently: Dynamically typed only
- Future: Optional static type checking
- Needed: Type AST, type checking infrastructure

**4. Tooling Infrastructure**
- Currently: No LSP, limited REPL, no debugger protocol
- Needed: IDE integration, better developer experience

**5. AST Modernization**
- Currently: Large enum variants, mixed concerns
- Future: Better structure for tooling and analysis

---

## Scope

### In Scope (Phase 3)

**Priority 1 (HIGH) - Performance & Foundation:**
1. ✅ Symbol interning system (Proposal 005)
2. ✅ Visitor pattern for AST traversal (Proposal 007)
3. ✅ Arena allocation for AST nodes

**Priority 2 (MEDIUM) - Type System Preparation:**
4. ✅ Type AST definitions
5. ✅ Type checking framework (basic)
6. ✅ Type inference foundations

**Priority 3 (MEDIUM) - Tooling Foundations:**
7. ✅ LSP server basics (hover, go-to-definition)
8. ✅ Debugger protocol (DAP) foundations
9. ✅ Enhanced REPL with completions

**Priority 4 (LOW) - AST Modernization:**
10. ✅ AST visitor traits
11. ✅ AST builder pattern
12. ✅ Position tracking improvements

### Out of Scope
- ❌ Full type system implementation (Phase 4+)
- ❌ Full LSP implementation (Phase 4+)
- ❌ Production debugger (Phase 4+)
- ❌ Breaking API changes

---

## Detailed Plan

## 1. Symbol Interning System (HIGH PRIORITY)

**Reference:** [Proposal 005: Symbol Interning](005_symbol_interning.md)

### Current Behavior
```rust
// Expression with String identifiers
Expression::Identifier {
    name: String,  // 24 bytes per identifier
    span: Span,
}

// Compiler with String-based symbol tables
HashMap<String, Object>           // Many clones
HashMap<String, Symbol>           // Slow lookups
```

### Proposed Architecture

```
src/syntax/
├── interner/
│   ├── mod.rs              # Public API (80 lines)
│   ├── symbol_id.rs        # SymbolId type (40 lines)
│   ├── interner.rs         # SymbolInterner (120 lines)
│   └── global.rs           # Global interner instance (60 lines)
```

### Implementation

**Create `syntax/interner/symbol_id.rs`:**
```rust
/// Compact symbol identifier (4 bytes vs 24 bytes for String)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SymbolId(u32);

impl SymbolId {
    pub const NONE: Self = SymbolId(u32::MAX);

    pub fn new(id: u32) -> Self {
        Self(id)
    }

    pub fn as_u32(self) -> u32 {
        self.0
    }
}

// Display for debugging
impl fmt::Display for SymbolId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "sym({})", self.0)
    }
}
```

**Create `syntax/interner/interner.rs`:**
```rust
use super::SymbolId;
use std::collections::HashMap;

/// Global string interner for identifier deduplication
pub struct SymbolInterner {
    /// All unique strings (append-only)
    strings: Vec<String>,

    /// Fast lookup: string → ID
    lookup: HashMap<String, SymbolId>,
}

impl SymbolInterner {
    pub fn new() -> Self {
        let mut interner = Self {
            strings: Vec::new(),
            lookup: HashMap::new(),
        };

        // Pre-intern common keywords
        interner.intern("let");
        interner.intern("fun");
        interner.intern("if");
        interner.intern("match");
        interner.intern("return");
        interner.intern("import");
        interner.intern("module");

        interner
    }

    /// Intern a string, returning its unique ID
    pub fn intern(&mut self, s: &str) -> SymbolId {
        if let Some(&id) = self.lookup.get(s) {
            return id;
        }

        let id = SymbolId::new(self.strings.len() as u32);
        self.strings.push(s.to_string());
        self.lookup.insert(s.to_string(), id);
        id
    }

    /// Resolve a symbol ID back to its string
    pub fn resolve(&self, id: SymbolId) -> &str {
        &self.strings[id.as_u32() as usize]
    }

    /// Check if a symbol ID exists
    pub fn contains(&self, id: SymbolId) -> bool {
        (id.as_u32() as usize) < self.strings.len()
    }

    /// Get number of interned symbols
    pub fn len(&self) -> usize {
        self.strings.len()
    }
}
```

**Update AST to use SymbolId:**
```rust
// Before
pub enum Expression {
    Identifier {
        name: String,  // 24 bytes
        span: Span,
    },
}

// After
pub enum Expression {
    Identifier {
        name: SymbolId,  // 4 bytes (83% reduction!)
        span: Span,
    },
}
```

**Benefits:**
- 70-80% less memory for identifiers
- 2-3x faster identifier comparisons (integer equality)
- 3-5x faster HashMap lookups
- Foundation for type system

**Estimated Effort:** 1 week (5 days)
- Day 1-2: Implement SymbolInterner
- Day 3: Update AST types
- Day 4-5: Update parser, compiler, tests

---

## 2. Visitor Pattern for AST Traversal (HIGH PRIORITY)

**Reference:** [Proposal 007: Visitor Pattern](007_visitor_pattern.md)

### Current Problem
```rust
// Compiler, linter, type checker all duplicate traversal logic
impl Compiler {
    fn compile_expression(&mut self, expr: &Expression) {
        match expr {
            Expression::Binary { left, right, .. } => {
                self.compile_expression(left);   // Traversal
                self.compile_expression(right);  // Traversal
                self.emit(OpCode::Add);          // Logic
            }
        }
    }
}

// Linter duplicates the same pattern
impl Linter {
    fn check_expression(&mut self, expr: &Expression) {
        match expr {
            Expression::Binary { left, right, .. } => {
                self.check_expression(left);   // Same traversal!
                self.check_expression(right);  // Same traversal!
                // Different logic
            }
        }
    }
}
```

### Proposed Architecture

```
src/syntax/
├── visitor/
│   ├── mod.rs              # Visitor trait (150 lines)
│   ├── walk.rs             # Default traversal implementations (200 lines)
│   └── visitors/           # Concrete visitors
│       ├── pretty_printer.rs   # Debug printing (100 lines)
│       └── ast_validator.rs    # AST validation (120 lines)
│
src/bytecode/
└── codegen_visitor.rs      # Code generation as visitor (250 lines)

src/syntax/
└── type_checker_visitor.rs # Type checking as visitor (300 lines)
```

### Implementation

**Create `syntax/visitor/mod.rs`:**
```rust
use crate::syntax::{Expression, Statement, Program, Block};

/// Generic visitor trait for AST traversal
///
/// Type parameter `R` is the return type of visit methods.
/// - TypeChecker returns `Type`
/// - CodeGenerator returns `()`
/// - Linter returns `Vec<Diagnostic>`
pub trait Visitor<R = ()> {
    type Error;

    // ===== Expression Visitors =====

    /// Visit any expression (dispatcher)
    fn visit_expression(&mut self, expr: &Expression) -> Result<R, Self::Error> {
        walk_expression(self, expr)
    }

    fn visit_identifier(&mut self, name: SymbolId, span: Span) -> Result<R, Self::Error>;
    fn visit_integer(&mut self, value: i64, span: Span) -> Result<R, Self::Error>;
    fn visit_binary(&mut self, left: &Expression, op: &str, right: &Expression, span: Span) -> Result<R, Self::Error>;
    // ... other expression types

    // ===== Statement Visitors =====

    fn visit_statement(&mut self, stmt: &Statement) -> Result<R, Self::Error> {
        walk_statement(self, stmt)
    }

    fn visit_let(&mut self, name: SymbolId, value: &Expression, span: Span) -> Result<R, Self::Error>;
    fn visit_function(&mut self, name: SymbolId, params: &[SymbolId], body: &Block, span: Span) -> Result<R, Self::Error>;
    // ... other statement types

    // ===== Program Visitor =====

    fn visit_program(&mut self, program: &Program) -> Result<Vec<R>, Self::Error> {
        walk_program(self, program)
    }
}

/// Default traversal for expressions
pub fn walk_expression<V, R>(visitor: &mut V, expr: &Expression) -> Result<R, V::Error>
where
    V: Visitor<R>,
{
    match expr {
        Expression::Identifier { name, span } => {
            visitor.visit_identifier(*name, *span)
        }
        Expression::Binary { left, operator, right, span } => {
            visitor.visit_binary(left, operator, right, *span)
        }
        Expression::If { condition, consequence, alternative, span } => {
            visitor.visit_expression(condition)?;
            visitor.visit_block(consequence)?;
            if let Some(alt) = alternative {
                visitor.visit_block(alt)?;
            }
            Ok(/* combine results */)
        }
        // ... other expression types
    }
}
```

**Example: Type Checker as Visitor:**
```rust
use crate::syntax::visitor::Visitor;

pub struct TypeChecker {
    types: HashMap<SymbolId, Type>,
}

impl Visitor<Type> for TypeChecker {
    type Error = TypeError;

    fn visit_identifier(&mut self, name: SymbolId, span: Span) -> Result<Type, TypeError> {
        self.types.get(&name)
            .cloned()
            .ok_or_else(|| TypeError::UndefinedVariable(name, span))
    }

    fn visit_binary(&mut self, left: &Expression, op: &str, right: &Expression, span: Span) -> Result<Type, TypeError> {
        let left_ty = self.visit_expression(left)?;
        let right_ty = self.visit_expression(right)?;

        match (left_ty, right_ty, op) {
            (Type::Int, Type::Int, "+") => Ok(Type::Int),
            (Type::Float, Type::Float, "+") => Ok(Type::Float),
            (Type::String, Type::String, "+") => Ok(Type::String),
            _ => Err(TypeError::TypeMismatch(span, left_ty, right_ty)),
        }
    }

    // ... other methods
}
```

**Benefits:**
- Separate traversal from logic
- Easy to add new passes (type checking, optimization)
- Reusable traversal code
- Better testability

**Estimated Effort:** 1 week (5 days)
- Day 1-2: Implement Visitor trait + walk functions
- Day 3: Migrate linter to visitor
- Day 4: Create example visitors (pretty printer, validator)
- Day 5: Tests and documentation

---

## 3. Arena Allocation for AST Nodes (HIGH PRIORITY)

### Current Problem
```rust
// Heavy cloning of AST nodes
let program = parser.parse_program();  // Allocates AST
compiler.compile(&program);            // Clones nodes
linter.lint(&program);                 // Clones nodes again
type_checker.check(&program);          // More clones!
```

### Proposed Solution

**Use arena allocator for AST:**
```
src/syntax/
└── arena/
    ├── mod.rs              # Arena allocator (100 lines)
    └── ast_arena.rs        # AST-specific arena (80 lines)
```

**Create `syntax/arena/mod.rs`:**
```rust
use std::cell::RefCell;

/// Simple bump allocator for AST nodes
pub struct Arena {
    chunks: RefCell<Vec<Vec<u8>>>,
}

impl Arena {
    const CHUNK_SIZE: usize = 4096;

    pub fn new() -> Self {
        Self {
            chunks: RefCell::new(vec![Vec::with_capacity(Self::CHUNK_SIZE)]),
        }
    }

    /// Allocate space for a value and return a reference with arena lifetime
    pub fn alloc<T>(&self, value: T) -> &mut T {
        let size = std::mem::size_of::<T>();
        let align = std::mem::align_of::<T>();

        // Implementation details...
        // Returns reference with arena's lifetime
    }
}
```

**Update AST to use arena references:**
```rust
// Before
pub enum Expression {
    Binary {
        left: Box<Expression>,    // Heap allocation
        right: Box<Expression>,   // Heap allocation
        span: Span,
    },
}

// After (with arena)
pub enum Expression<'arena> {
    Binary {
        left: &'arena Expression<'arena>,   // Arena reference
        right: &'arena Expression<'arena>,  // Arena reference
        span: Span,
    },
}
```

**Benefits:**
- 50-70% less memory allocations
- 2-3x faster parsing (no Box allocations)
- Better cache locality
- Simpler memory management

**Estimated Effort:** 1 week (5-7 days)
- Day 1-2: Implement Arena allocator
- Day 3-5: Update AST types with lifetimes
- Day 6-7: Update parser, compiler, tests

---

## 4-6. Type System Foundations (MEDIUM PRIORITY)

### Proposed Architecture

```
src/syntax/
├── types/
│   ├── mod.rs              # Public API (80 lines)
│   ├── type_ast.rs         # Type AST (150 lines)
│   ├── type_env.rs         # Type environment (100 lines)
│   └── unification.rs      # Type unification (180 lines)
│
└── type_checker/
    ├── mod.rs              # Type checker orchestrator (120 lines)
    ├── inference.rs        # Type inference (250 lines)
    ├── checker.rs          # Type checking logic (200 lines)
    └── errors.rs           # Type errors (100 lines)
```

### Type AST

**Create `syntax/types/type_ast.rs`:**
```rust
/// Type representation
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    /// Integer type
    Int,

    /// Float type
    Float,

    /// String type
    String,

    /// Boolean type
    Bool,

    /// Array type
    Array(Box<Type>),

    /// Function type: (param types) -> return type
    Function {
        params: Vec<Type>,
        return_type: Box<Type>,
    },

    /// Type variable for inference
    Var(TypeVar),

    /// Named type (for user-defined types)
    Named(SymbolId),

    /// Option type
    Option(Box<Type>),

    /// Result type (Either)
    Result {
        ok: Box<Type>,
        err: Box<Type>,
    },
}

/// Type variable for unification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeVar(u32);

impl Type {
    pub fn is_numeric(&self) -> bool {
        matches!(self, Type::Int | Type::Float)
    }

    pub fn is_compatible_with(&self, other: &Type) -> bool {
        match (self, other) {
            (Type::Int, Type::Float) | (Type::Float, Type::Int) => true,
            (a, b) => a == b,
        }
    }
}
```

### Type Inference

**Create `syntax/type_checker/inference.rs`:**
```rust
use crate::syntax::types::{Type, TypeVar};
use std::collections::HashMap;

pub struct TypeInference {
    /// Type variable counter
    next_var: u32,

    /// Substitutions: type var → concrete type
    substitutions: HashMap<TypeVar, Type>,
}

impl TypeInference {
    pub fn new() -> Self {
        Self {
            next_var: 0,
            substitutions: HashMap::new(),
        }
    }

    /// Create a fresh type variable
    pub fn fresh_var(&mut self) -> Type {
        let var = TypeVar(self.next_var);
        self.next_var += 1;
        Type::Var(var)
    }

    /// Unify two types
    pub fn unify(&mut self, a: &Type, b: &Type) -> Result<(), TypeError> {
        match (a, b) {
            (Type::Var(v), t) | (t, Type::Var(v)) => {
                self.substitutions.insert(*v, t.clone());
                Ok(())
            }
            (Type::Int, Type::Int) => Ok(()),
            (Type::Function { params: p1, return_type: r1 },
             Type::Function { params: p2, return_type: r2 }) => {
                if p1.len() != p2.len() {
                    return Err(TypeError::ArityMismatch);
                }
                for (t1, t2) in p1.iter().zip(p2) {
                    self.unify(t1, t2)?;
                }
                self.unify(r1, r2)
            }
            _ => Err(TypeError::TypeMismatch(a.clone(), b.clone())),
        }
    }

    /// Apply substitutions to a type
    pub fn apply(&self, ty: &Type) -> Type {
        match ty {
            Type::Var(v) => {
                if let Some(t) = self.substitutions.get(v) {
                    self.apply(t)
                } else {
                    ty.clone()
                }
            }
            Type::Function { params, return_type } => {
                Type::Function {
                    params: params.iter().map(|p| self.apply(p)).collect(),
                    return_type: Box::new(self.apply(return_type)),
                }
            }
            _ => ty.clone(),
        }
    }
}
```

**Estimated Effort:** 2 weeks (10 days)
- Week 1: Type AST, type environment, basic inference
- Week 2: Type checker visitor, integration, tests

---

## 7-9. Tooling Foundations (MEDIUM PRIORITY)

### LSP Server Basics

```
src/tools/
├── lsp/
│   ├── mod.rs              # LSP server (200 lines)
│   ├── protocol.rs         # LSP protocol types (150 lines)
│   ├── handlers/           # Request handlers
│   │   ├── hover.rs        # Hover information (100 lines)
│   │   ├── goto_def.rs     # Go-to-definition (120 lines)
│   │   ├── completion.rs   # Completions (150 lines)
│   │   └── diagnostics.rs  # Diagnostics publishing (80 lines)
│   └── server.rs           # Main server loop (180 lines)
```

**Create `tools/lsp/handlers/hover.rs`:**
```rust
use tower_lsp::lsp_types::*;

pub async fn hover(
    position: Position,
    source: &str,
    interner: &SymbolInterner,
) -> Option<Hover> {
    // Parse source
    let program = parse(source)?;

    // Find symbol at position
    let symbol = find_symbol_at_position(&program, position)?;

    // Get type information
    let ty = infer_type(symbol)?;

    // Format hover text
    let markdown = format!(
        "```flux\n{}: {}\n```",
        interner.resolve(symbol.name),
        ty
    );

    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: markdown,
        }),
        range: Some(symbol.span.to_lsp_range()),
    })
}
```

### Debugger Protocol (DAP) Foundations

```
src/tools/
└── debugger/
    ├── mod.rs              # Debugger interface (150 lines)
    ├── dap_server.rs       # DAP protocol server (200 lines)
    ├── breakpoints.rs      # Breakpoint management (120 lines)
    └── stepping.rs         # Step, continue, pause (100 lines)
```

### Enhanced REPL

```
src/repl/
├── mod.rs                  # REPL orchestrator (150 lines)
├── completion.rs           # Tab completion (120 lines)
├── history.rs              # Command history (80 lines)
└── highlighting.rs         # Syntax highlighting (100 lines)
```

**Estimated Effort:** 2-3 weeks (15 days)
- Week 1: LSP server basics (hover, go-to-def)
- Week 2: Debugger protocol foundations
- Week 3: Enhanced REPL

---

## 10-12. AST Modernization (LOW PRIORITY)

### AST Visitor Traits

**Create `syntax/ast/visitor.rs`:**
```rust
/// Mutable visitor that can transform AST
pub trait MutVisitor {
    fn visit_expression_mut(&mut self, expr: &mut Expression) {
        walk_expression_mut(self, expr)
    }

    fn visit_statement_mut(&mut self, stmt: &mut Statement) {
        walk_statement_mut(self, stmt)
    }
}

/// Example: Constant folder
pub struct ConstantFolder;

impl MutVisitor for ConstantFolder {
    fn visit_expression_mut(&mut self, expr: &mut Expression) {
        // Fold constant expressions
        if let Expression::Binary { left, operator, right, span } = expr {
            if let (Expression::Integer { value: a, .. },
                    Expression::Integer { value: b, .. }) = (&**left, &**right) {
                if operator == "+" {
                    *expr = Expression::Integer {
                        value: a + b,
                        span: *span,
                    };
                    return;
                }
            }
        }

        walk_expression_mut(self, expr)
    }
}
```

**Estimated Effort:** 1 week (5 days)

---

## Implementation Roadmap

### Month 1: Performance & Foundation (Weeks 1-4)

#### Week 1: Symbol Interning
**Deliverable:** SymbolId-based AST

- Day 1-2: Implement SymbolInterner
- Day 3: Update AST types to use SymbolId
- Day 4-5: Update parser, compiler, all call sites

#### Week 2: Visitor Pattern
**Deliverable:** Visitor trait + examples

- Day 6-7: Implement Visitor trait + walk functions
- Day 8: Migrate linter to visitor
- Day 9-10: Example visitors (pretty printer, validator)

#### Week 3-4: Arena Allocation
**Deliverable:** Arena-based AST

- Day 11-12: Implement Arena allocator
- Day 13-15: Update AST with lifetimes
- Day 16-17: Update parser, compiler
- Day 18-20: Tests, performance validation

### Month 2: Type System Foundations (Weeks 5-8)

#### Week 5: Type AST
**Deliverable:** Type representation

- Day 21-22: Define Type enum and TypeVar
- Day 23-24: Type environment
- Day 25: Type unification algorithm

#### Week 6: Type Inference
**Deliverable:** Basic type inference

- Day 26-27: Hindley-Milner inference
- Day 28-29: Constraint generation
- Day 30: Constraint solving

#### Week 7-8: Type Checker Integration
**Deliverable:** Optional type checking

- Day 31-33: Type checker as visitor
- Day 34-35: Integration with compiler
- Day 36-40: Tests, examples, documentation

### Month 3: Tooling (Weeks 9-12)

#### Week 9-10: LSP Server
**Deliverable:** Basic LSP features

- Day 41-43: LSP protocol implementation
- Day 44-45: Hover provider
- Day 46-48: Go-to-definition
- Day 49-50: Completions

#### Week 11: Debugger Protocol
**Deliverable:** DAP foundations

- Day 51-53: DAP server
- Day 54-55: Breakpoint management

#### Week 12: Enhanced REPL
**Deliverable:** Better REPL UX

- Day 56-57: Tab completion
- Day 58-59: Syntax highlighting
- Day 60: Command history

---

## Success Metrics

### Performance (Symbol Interning + Arena)
- ✅ **70-80% less memory** for identifiers
- ✅ **50-70% fewer allocations** for AST nodes
- ✅ **2-3x faster parsing** with arena allocation
- ✅ **2-3x faster identifier lookups** with interning

### Architecture (Visitor Pattern)
- ✅ **Zero duplication** of traversal logic
- ✅ **Easy to add passes** (< 100 lines per pass)
- ✅ **Clean separation** of concerns
- ✅ **Independently testable** passes

### Type System
- ✅ **Optional type annotations** work
- ✅ **Type inference** for simple cases
- ✅ **Type errors** have good messages
- ✅ **Foundation** for full type system in Phase 4

### Tooling
- ✅ **LSP hover** shows types and docs
- ✅ **Go-to-definition** works for functions/variables
- ✅ **Completions** suggest available symbols
- ✅ **REPL** has tab completion

### Code Quality
- ✅ All existing tests pass
- ✅ Performance benchmarks show improvements
- ✅ Documentation for new systems
- ✅ Example code for using visitor pattern

---

## Performance Comparison

### Before Phase 3
| Metric | Value |
|--------|-------|
| Parse 1000-line file | ~50ms |
| Memory for AST | ~2MB |
| Identifier lookups | String comparison |
| Adding new passes | Duplicate traversal |

### After Phase 3
| Metric | Value | Improvement |
|--------|-------|-------------|
| Parse 1000-line file | ~20ms | **2.5x faster** |
| Memory for AST | ~600KB | **70% less** |
| Identifier lookups | Integer comparison | **3-5x faster** |
| Adding new passes | Use Visitor trait | **5-10x easier** |

---

## Risks and Mitigation

### Risk 1: Lifetime Complexity (Arena)
**Likelihood:** High
**Impact:** Medium
**Mitigation:**
- Start with simple arena
- Extensive testing
- Good documentation with examples
- Can roll back to Box if needed

### Risk 2: Breaking Changes (SymbolId)
**Likelihood:** High
**Impact:** Medium
**Mitigation:**
- Migration in steps
- Keep String-based API alongside
- Deprecation warnings
- Remove old API in v0.2.0

### Risk 3: Type System Scope Creep
**Likelihood:** Medium
**Impact:** High
**Mitigation:**
- Strictly limit to foundations
- No full type system in Phase 3
- Focus on infrastructure, not features

### Risk 4: Tooling Complexity
**Likelihood:** Medium
**Impact:** Low
**Mitigation:**
- Start with minimal LSP features
- Can defer debugger to Phase 4
- REPL enhancements are optional

---

## Future Considerations (Phase 4+)

### Building on Phase 3 Foundations

**Phase 4: Full Type System**
- Gradual typing (optional annotations)
- Type inference everywhere
- Algebraic data types
- Type classes/traits

**Phase 5: Advanced Tooling**
- Full LSP implementation (refactoring, rename)
- Visual debugger integration
- Code formatter using AST
- Documentation generator

**Phase 6: Optimizations**
- Cross-module optimization (using visitor)
- Constant folding (using visitor)
- Dead code elimination (using visitor)
- JIT compilation

---

## Integration with Other Proposals

### Builds On:
- **Proposal 005:** Symbol Interning (implemented in Phase 3)
- **Proposal 006:** Phase 1 Module Split (prerequisite)
- **Proposal 007:** Visitor Pattern (implemented in Phase 3)
- **Proposal 012:** Phase 2 Advanced Split (prerequisite)

### Enables:
- **Proposal 011:** Module system enhancements (symbol interning helps)
- **Future proposals:** Type system, optimizations, advanced tooling

---

## References

- [Proposal 005: Symbol Interning](005_symbol_interning.md)
- [Proposal 006: Phase 1 Module Split](006_phase1_module_split_plan.md)
- [Proposal 007: Visitor Pattern](007_visitor_pattern.md)
- [Proposal 012: Phase 2 Advanced Split](012_phase2_module_split_plan.md)
- [Compiler Architecture](../architecture/compiler_architecture.md)
- **Hindley-Milner Type Inference:** *Algorithm W*
- **Arena Allocation:** [Rust typed-arena crate](https://crates.io/crates/typed-arena)
- **LSP Protocol:** [Language Server Protocol](https://microsoft.github.io/language-server-protocol/)

---

## Approval Checklist

### Performance Improvements
- [ ] Symbol interning strategy approved
- [ ] Arena allocation approach reviewed
- [ ] Performance benchmarks baseline established
- [ ] Memory usage targets agreed upon

### Architectural Patterns
- [ ] Visitor pattern design approved
- [ ] AST lifetime strategy reviewed
- [ ] Trade-offs documented and accepted

### Type System
- [ ] Type AST design approved
- [ ] Type inference scope limited to foundations
- [ ] No full type system in Phase 3 (deferred)

### Tooling
- [ ] LSP feature set prioritized
- [ ] Debugger scope defined (minimal)
- [ ] REPL enhancements approved

### Implementation
- [ ] 3-month timeline agreed upon
- [ ] Can defer tooling if needed
- [ ] Performance targets realistic
- [ ] Ready to implement

---

## Summary: What Phase 3 Delivers

### Performance Foundation
1. ✅ **Symbol Interning** - 70% less memory, 3x faster lookups
2. ✅ **Arena Allocation** - 2.5x faster parsing, better cache locality

### Architectural Foundation
3. ✅ **Visitor Pattern** - Clean multi-pass compilation
4. ✅ **AST Modernization** - Better structure for tooling

### Type System Foundation
5. ✅ **Type AST** - Representation for types
6. ✅ **Type Inference** - Basic Hindley-Milner
7. ✅ **Type Checker** - Optional type checking

### Tooling Foundation
8. ✅ **LSP Server** - Hover, go-to-def, completions
9. ✅ **Debugger Protocol** - DAP foundations
10. ✅ **Enhanced REPL** - Better developer experience

### Impact
- **Performance:** 2-3x faster, 70% less memory
- **Maintainability:** Visitor pattern for clean passes
- **Future-Ready:** Foundations for type system, LSP, debugger
- **Developer Experience:** Better tooling across the board

---

**Recommendation:** Implement in three phases:
1. **Performance first** (Symbol Interning + Arena) - 1 month
2. **Type foundations** (Type AST + Inference) - 1 month
3. **Tooling** (LSP + REPL) - 1 month (optional, can defer)

This provides immediate performance benefits while building foundations for future features.
