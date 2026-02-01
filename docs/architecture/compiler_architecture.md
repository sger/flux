# Flux Compiler Architecture

This document outlines compiler architecture patterns and the roadmap for Flux's compiler evolution.

---

## Current Architecture (v0.0.2)

Flux uses a traditional **pass-based pipeline**:

```
Source Code
    ↓
┌─────────┐
│  Lexer  │  → Tokens
└─────────┘
    ↓
┌─────────┐
│ Parser  │  → AST (Abstract Syntax Tree)
└─────────┘
    ↓
┌──────────┐
│ Compiler │  → Bytecode
└──────────┘
    ↓
┌─────────┐
│   VM    │  → Execution
└─────────┘
```

### File Structure

```
src/
├── frontend/
│   ├── lexer.rs         # Tokenization
│   ├── parser.rs        # Parsing → AST
│   ├── ast/
│   │   ├── expression.rs
│   │   ├── statement.rs
│   │   └── ...
│   └── token_type.rs
├── bytecode/
│   ├── compiler.rs      # AST → Bytecode (main file, ~1700 lines)
│   ├── const_eval.rs    # Compile-time constant evaluation
│   ├── op_code.rs       # Bytecode opcodes
│   ├── symbol_table.rs  # Variable/function tracking
│   └── ...
├── runtime/
│   ├── vm.rs            # Bytecode execution
│   ├── object.rs        # Runtime values
│   └── ...
└── lib.rs
```

---

## Architecture Patterns

### 1. Pass-Based Architecture

**Used by:** Simple interpreters, educational compilers

```
Source → Lexer → Parser → Compiler → VM
```

**Pros:**
- Simple to understand and implement
- Each phase is independent
- Easy to test each phase

**Cons:**
- Hard to share information between phases
- Can require multiple traversals
- Monolithic phases can grow large

**Flux status:** Currently using this pattern.

---

### 2. Multi-IR / Lowering Architecture

**Used by:** GHC (Haskell), Rust, Swift, LLVM

```
Source → AST → HIR → MIR → LIR → Target
          ↓      ↓      ↓      ↓
       (parse) (type) (opt) (codegen)
```

Each IR (Intermediate Representation) is suited for specific tasks:

| IR | Purpose | Example Transformations |
|----|---------|------------------------|
| **HIR** (High-level) | Close to source syntax | Macro expansion, desugaring |
| **MIR** (Mid-level) | Control flow explicit | Optimization, borrow checking |
| **LIR** (Low-level) | Close to target | Register allocation |

**GHC's Pipeline:**
```
Haskell → AST → Renamed → Typed → Core → STG → Cmm → Assembly
                   ↓         ↓       ↓      ↓      ↓
              (resolve)  (infer) (optimize) (eval) (codegen)
```

**Core** is GHC's key IR - a tiny typed lambda calculus (~10 constructors).

**Pros:**
- Each IR optimized for its purpose
- Clean separation of concerns
- Powerful optimization opportunities

**Cons:**
- More complex implementation
- More code to maintain
- Overhead of IR conversions

---

### 3. Query-Based / Demand-Driven Architecture

**Used by:** Rust (rustc), rust-analyzer, Salsa framework

Instead of linear passes, computation is organized as memoized queries:

```rust
// Pseudocode
fn type_of(db: &Database, expr: ExprId) -> Type {
    match db.lookup_expr(expr) {
        Expr::Call { func, args } => {
            let func_type = db.type_of(func);  // Recursive query
            // ...
        }
    }
}
```

**Pros:**
- Excellent for incremental compilation
- Perfect for IDE integration
- Only computes what's needed

**Cons:**
- Complex to implement correctly
- Requires careful cycle handling
- Higher initial investment

---

### 4. Nanopass Architecture

**Used by:** Chez Scheme, academic compilers

Many tiny passes, each doing one small transformation:

```
Pass 1: Remove syntax sugar A
Pass 2: Remove syntax sugar B
Pass 3: Convert pattern X
Pass 4: Convert pattern Y
...
Pass 50: Generate code
```

**Pros:**
- Each pass is trivial to understand
- Very modular and testable
- Easy to add new passes

**Cons:**
- Many IR definitions
- Performance overhead from conversions
- Can be harder to see the big picture

---

### 5. Visitor Pattern

**Used by:** Many compilers for AST traversal

```rust
trait Visitor {
    fn visit_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Binary { left, right, .. } => {
                self.visit_expr(left);
                self.visit_expr(right);
            }
            // ...
        }
    }
}

struct TypeChecker;
impl Visitor for TypeChecker { /* ... */ }

struct CodeGenerator;
impl Visitor for CodeGenerator { /* ... */ }
```

**Pros:**
- Separates traversal from logic
- Easy to add new passes
- Reusable traversal code

**Cons:**
- Can be verbose in Rust (better in OO languages)
- Hard to accumulate state across nodes

---

## BEAM/Elixir Architecture

Elixir compiles to Erlang's BEAM VM:

```
Elixir Source
    ↓
Elixir AST (quoted expressions)
    ↓
Expanded AST (macros applied)
    ↓
Erlang Abstract Format
    ↓
Core Erlang (simple functional IR)
    ↓
BEAM Bytecode
```

**Key insight:** Elixir delegates to Erlang's mature compiler after macro expansion.

**Core Erlang** is similar to GHC's Core:
- Simple functional language
- All pattern matching compiled to case expressions
- Easy to analyze and optimize

---

## Flux Roadmap

### Phase 1: Module Split (v0.0.2 - v0.0.3)

Split `compiler.rs` by concern without changing semantics:

```
src/bytecode/
├── compiler.rs          # Main driver, Compiler struct, helpers (~400 lines)
├── expr_compiler.rs     # compile_expression() (~500 lines)
├── stmt_compiler.rs     # compile_statement() (~300 lines)
├── module_compiler.rs   # compile_module_statement() (~200 lines)
├── const_eval.rs        # Constant evaluation (done ✓)
└── pattern_compiler.rs  # Pattern matching compilation (~200 lines)
```

**Benefits:**
- Smaller, focused files
- Easier to navigate
- Clear ownership of functionality

### Phase 2: Desugaring Phase (v0.0.4+)

Add explicit desugaring before compilation:

```
AST → Desugared AST → Bytecode
         ↓
   List comprehensions → map/filter
   Pipe operator → function calls (already done in parser)
   Lambda shorthand → full functions (already done in parser)
```

**New file:** `src/frontend/desugar.rs`

**Benefits:**
- List comprehensions become trivial
- Future syntax sugar is easy to add
- Compiler sees simpler AST

### Phase 3: Core IR (v0.1.0+)

Add a minimal intermediate representation:

```
AST → Core Flux → Bytecode
          ↓
    Simple constructs only
```

**Core Flux constructs (~10):**
```rust
enum Core {
    Literal(Value),
    Var(Name),
    Let { name: Name, value: Box<Core>, body: Box<Core> },
    Lambda { params: Vec<Name>, body: Box<Core> },
    Apply { func: Box<Core>, args: Vec<Core> },
    Case { scrutinee: Box<Core>, branches: Vec<Branch> },
    Array(Vec<Core>),
    Hash(Vec<(Core, Core)>),
    Builtin { name: Name, args: Vec<Core> },
    If { cond: Box<Core>, then: Box<Core>, else_: Box<Core> },
}
```

**Benefits:**
- All complex features reduce to simple Core
- Optimization passes work on Core
- Easier to reason about semantics

---

## Recommended Books

### Compiler Construction

1. **"Crafting Interpreters" by Robert Nystrom**
   - Free online: https://craftinginterpreters.com
   - Practical, builds two complete interpreters
   - Great for bytecode VMs (very relevant to Flux)

2. **"Engineering a Compiler" by Cooper & Torczon**
   - Comprehensive academic text
   - Excellent on IRs and optimization
   - Good reference for production compilers

3. **"Modern Compiler Implementation in ML" by Andrew Appel**
   - Also available in Java/C versions
   - Strong on functional language compilation
   - Covers SSA, register allocation

4. **"Compilers: Principles, Techniques, and Tools" (Dragon Book)**
   - Classic reference by Aho, Lam, Sethi, Ullman
   - Comprehensive but dense
   - Good for parsing theory

### Functional Language Implementation

5. **"The Implementation of Functional Programming Languages" by Simon Peyton Jones**
   - Free online: https://www.microsoft.com/en-us/research/publication/the-implementation-of-functional-programming-languages/
   - How GHC works (earlier version)
   - Pattern matching compilation, graph reduction

6. **"Implementing Functional Languages: a tutorial" by SPJ & Lester**
   - Free online: https://www.microsoft.com/en-us/research/publication/implementing-functional-languages-a-tutorial/
   - Step-by-step implementation
   - Great for understanding Core

7. **"Types and Programming Languages" by Benjamin Pierce**
   - Definitive text on type systems
   - Essential if adding types to Flux

### Specific Topics

8. **"A Nanopass Framework for Compiler Education" (Paper)**
   - https://www.cs.indiana.edu/~dyb/pubs/nano-jfp.pdf
   - Explains nanopass architecture

9. **"Rust Compiler Development Guide"**
   - https://rustc-dev-guide.rust-lang.org/
   - Query-based architecture in practice
   - Good for modern compiler techniques

10. **"The BEAM Book"**
    - https://blog.stenmans.org/theBeamBook/
    - How Erlang/Elixir's VM works
    - Relevant for bytecode VM design

---

## Summary

| Phase | Change | Complexity | Benefit |
|-------|--------|------------|---------|
| **Current** | Single compiler.rs | Low | Simple |
| **Phase 1** | Module split | Low | Maintainability |
| **Phase 2** | Desugaring | Medium | Extensibility |
| **Phase 3** | Core IR | High | Optimization |

Start with Phase 1 (module split) - it's low risk and provides immediate benefits. Phase 2 becomes valuable when adding list comprehensions. Phase 3 is only needed for serious optimization work.
