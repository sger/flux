# Flux Compiler Architecture

This document describes the current architecture of the Flux compiler.

---

## Canonical Type/Effects Spec

For current type-system + effects semantics and diagnostics contracts, see:
- `docs/internals/type_system_effects.md`

This architecture document focuses on component layout; semantic truth for type/effects lives in that dedicated spec.

---

## Pipeline

Flux compiles source code through a series of intermediate representations. Both execution
backends (bytecode VM and Cranelift JIT) share the same front-end and lowering pipeline:

```
Source (.flx)
    │
    ▼
  Lexer                    token stream
    │
    ▼
  Parser                   AST  (string-interned identifiers, parse-time desugaring)
    │
    ▼
  AST Passes               constant folding · desugaring · free var collection · tail call detection
    │
    ▼
  HM Type Inference        Algorithm W with effect rows — produces typed AST + hm_expr_types map
    │
    ├──────────────────────────────────────────────────┐
    ▼                                                  ▼
  Structured IR Lowering  (cfg/lower.rs)         Core IR Lowering  (nary/lower_ast.rs)
  AST → IrTopLevelItem + IrFunction              AST → CoreProgram
  (handles modules, data types, effects)         (handles top-level functions)
    │                                                  │
    │                                                  ▼
    │                                            Core IR Passes  (nary/passes.rs)
    │                                            beta reduction · COKC · inline trivial lets
    │                                                  │
    │                                                  ▼
    │                                            Core → CFG IR  (nary/to_ir.rs)
    │                                            CoreExpr → IrFunction + IrBlock
    │                                                  │
    ├──────────────── merge ◄──────────────────────────┘
    │  (Core IR functions replace structured-IR functions;
    │   module-internal functions kept from structured-IR)
    ▼
  IrProgram                unified CFG IR with IrFunction, IrBlock, IrInstr, IrTerminator
    │
    ├─────────────────────────────────────────┐
    ▼                                         ▼
  Bytecode Compiler                       Cranelift JIT  (--features jit)
  .fxc cache                              native machine code
    │                                         │
    ▼                                         ▼
  Stack VM                              Native Execution
    │                                         │
    └─────────────────────────────────────────┘
                        │
                        ▼
                  GC Heap  (cons lists · HAMT maps)
```

### Dual Lowering Pipeline

The current pipeline runs **two parallel lowering passes** that are merged before code generation:

1. **Structured IR lowering** (`cfg/lower.rs`): Lowers the full AST including modules, data
   types, effect declarations, imports, and function bodies. Produces `IrTopLevelItem` metadata
   and `IrFunction` entries with CFG basic blocks. This is the "old" path.

2. **Core IR lowering** (`nary/lower_ast.rs` → `nary/passes.rs` → `nary/to_ir.rs`): Lowers
   top-level functions through the N-ary Core IR, runs optimization passes, then converts to
   CFG IR. This is the "new" path that will eventually replace the structured-IR lowering.

After both passes run, the Core IR functions replace the structured-IR functions for any
top-level function that has a Core IR representation. Module-internal functions (which the
Core IR pipeline does not yet cover) are preserved from the structured-IR pass.

### Why Two Passes?

The N-ary Core IR pipeline currently only processes top-level function definitions. It skips:
- `Statement::Module` bodies (module-internal functions)
- Data type declarations
- Effect declarations
- Import statements

These constructs are handled by the structured-IR lowering path. The merge step ensures
both paths contribute to the final `IrProgram`. The goal is to eventually extend the Core IR
pipeline to handle all constructs, eliminating the need for the structured-IR path.

### Parse-Time Desugaring

Several constructs are fully desugared during parsing — no dedicated AST node or compiler support needed:

| Syntax | Desugars to |
|--------|------------|
| `a \|> f(b)` | `f(a, b)` (`Expression::Call`) |
| `[x * 2 \| x <- xs, guard]` | `map` / `filter` / `flat_map` calls |
| `} else if c {` | nested `else { if c { ... } }` block |
| `expr where x = val` | `let` binding injected before `expr` |

---

## Source Layout (`src/`)

```
src/
├── syntax/                  Front-end
│   ├── lexer/               Tokenization, one/two-byte dispatch tables
│   ├── parser/              Hybrid recursive descent + Pratt expression parser
│   │   ├── mod.rs           Entry point, token navigation (3-token lookahead)
│   │   ├── expression.rs    Expression parsing, array/hash/cons/comprehension literals
│   │   ├── statement.rs     fn / module / import / let declarations
│   │   ├── literal.rs       Number, string, interpolation parsing
│   │   └── helpers.rs       Error recovery, LIST_ERROR_LIMIT
│   ├── token_type.rs        Token definitions via define_tokens! macro
│   ├── interner.rs          String interning — all identifiers are Symbol (u32 index)
│   ├── linter.rs            Lint passes over AST
│   ├── formatter.rs         Source formatter
│   └── module_graph.rs      Import resolution, cycle detection, topological sort
│
├── ast/                     AST transforms and analysis (optional passes)
│   ├── fold/                Constant folding
│   ├── desugar/             Additional desugaring (after parse)
│   ├── free_vars/           Free variable collection for closure compilation
│   ├── tail_calls/          Tail call detection / annotation
│   ├── type_infer/          Hindley-Milner inference (Algorithm W) with effect rows
│   └── visitor.rs           Visitor + Folder traits for AST traversal
│
├── nary/                    Core IR — functional intermediate representation
│   ├── mod.rs               CoreExpr, CoreDef, CoreProgram types (~12 expression variants)
│   ├── lower_ast.rs         AST → Core IR lowering (post-HM, type-directed)
│   ├── passes.rs            Core IR optimization passes (beta reduction, COKC, etc.)
│   ├── to_ir.rs             Core IR → CFG IR lowering (CoreExpr → IrFunction/IrBlock)
│   └── display.rs           Pretty-printing for Core IR
│
├── cfg/                     CFG IR — control flow graph representation
│   ├── mod.rs               IrProgram, IrFunction, IrBlock, IrInstr, IrTerminator types
│   ├── lower.rs             AST → CFG IR lowering (structured path, handles modules)
│   ├── passes.rs            CFG IR optimization passes
│   └── validate.rs          CFG IR validation
│
├── bytecode/                Bytecode compiler
│   ├── compiler/            CFG IR → stack-based bytecode
│   │   ├── mod.rs           Compiler struct, symbol table setup, Base function registration
│   │   ├── statement.rs     CFG IR compilation (IrInstr, IrTerminator → opcodes)
│   │   └── expression.rs    AST expression compilation (fallback for module functions)
│   ├── opcode.rs            ~45 opcodes (OpGetLocal, OpCall, OpMatch, ...)
│   ├── symbol_table.rs      Variable/function/Base-function tracking per scope
│   └── cache.rs             .fxc bytecode cache (SHA-2 content hashing)
│
├── runtime/                 Bytecode VM and supporting runtime
│   ├── vm/                  Stack-based VM, instruction dispatch, call frames
│   │   └── test_runner.rs   --test flag: collect_test_functions, run_test_fns, reporting
│   ├── value.rs             Value enum (Integer, Float, String, Array, Gc, Closure, ...)
│   ├── base/                77 Base functions, registered via BASE_FUNCTIONS array
│   │   ├── array_ops.rs
│   │   ├── string_ops.rs
│   │   ├── hash_ops.rs
│   │   ├── list_ops.rs
│   │   ├── numeric_ops.rs
│   │   ├── io_ops.rs
│   │   ├── type_check.rs
│   │   └── assert_ops.rs
│   └── gc/                  Mark-and-sweep GC heap
│       ├── heap.rs          Allocation, mark, sweep
│       ├── cons.rs          HeapObject::Cons — immutable linked lists
│       └── hamt.rs          HeapObject::HamtNode/HamtCollision — persistent maps
│
├── jit/                     Cranelift JIT backend (--features jit)
│   ├── compiler.rs          CFG IR → Cranelift IR
│   ├── context.rs           JIT execution context, shares GC heap with VM
│   ├── runtime_helpers.rs   Native callbacks: rt_call_base_function, GC allocation
│   └── value_arena.rs       Pointer-stable allocation for JIT values
│
├── primop/                  41 primitive operations with frozen discriminants
│
└── diagnostics/             Structured error reporting
    ├── diagnostic.rs        Core Diagnostic struct (builder pattern via trait)
    ├── builders/            DiagnosticBuilder trait — 24 with_* methods
    ├── types/               ErrorCode, Severity, Hint, Label, Suggestion, Related
    ├── rendering/           ANSI rendering, source snippets, formatter
    ├── compiler_errors.rs   Compile-time error constructors (67 error codes)
    ├── runtime_errors.rs    Runtime error constructors
    ├── aggregator.rs        Multi-diagnostic deduplication, grouping, sorting
    └── registry.rs          Error code registry (E1xx, E2xx–E9xx, E10xx, W2xx)
```

---

## Intermediate Representations

Flux has three IRs between the surface AST and executable code. See
[`ir_pipeline.md`](ir_pipeline.md) for a deep dive into each one.

| IR | Module | Purpose | Key types |
|----|--------|---------|-----------|
| **Core IR** (N-ary) | `nary/` | Functional IR: ~12 expression variants, eliminates all sugar | `CoreExpr`, `CoreDef`, `CoreProgram` |
| **CFG IR** | `cfg/` | Control flow graph: basic blocks, SSA-like variables | `IrFunction`, `IrBlock`, `IrInstr`, `IrTerminator` |
| **Bytecode** | `bytecode/` | Stack-based instructions for the VM | `OpCode`, `CompiledFunction` |

---

## Key Design Decisions

### Interned Identifiers

All identifiers go through `syntax::interner`. The `Identifier` type is `symbol::Symbol` (a `u32` index), not `String`. This makes identifier comparison O(1) and eliminates string allocation in the AST.

### Rc-Based Values

Runtime values use `Rc` for sharing. The **no-cycle invariant** is critical — values must form DAGs, never cyclic graphs. Strings are `Rc<str>`, arrays are `Rc<Vec<Value>>`. The GC heap handles the only collections that can share structure (cons lists, HAMT maps).

### Value Enum

```rust
enum Value {
    // Primitives
    Integer(i64), Float(f64), Boolean(bool), String(Rc<str>), None, EmptyList,
    // Wrappers
    Some(Rc<Value>), Left(Rc<Value>), Right(Rc<Value>),
    // Collections
    Array(Rc<Vec<Value>>),  // Rc-backed, not GC-managed
    Gc(GcHandle),           // GC-managed: cons cells and HAMT nodes
    // Functions
    Function(Rc<CompiledFunction>), Closure(Rc<Closure>), BaseFunction(u8),
    JitClosure(Rc<JitClosure>),     // feature-gated
    // Internal
    ReturnValue(Rc<Value>),  // sentinel for return from blocks
    Uninit,                  // VM internal sentinel
}
```

`Value::Gc` wraps heap objects. `Display` for `Value::Gc` shows `<gc@N>` — use `list_ops::format_value()` with a `RuntimeContext` for proper rendering.

### Base Function Registration

Base functions must be registered in three places with matching indices:

1. **Implementation** in `runtime/base/<module>.rs`
2. **`BASE_FUNCTIONS` array** in `runtime/base/mod.rs` — the array index is the Base function ID
3. **Symbol table** in `bytecode/compiler/mod.rs` — `symbol_table.define_base_function(INDEX, ...)`

`OpGetBase` emits the index at compile time; `get_base_function_by_index()` resolves it at runtime. The JIT uses the same `BASE_FUNCTIONS` array via `rt_call_base_function()`, so new Base functions work in both backends automatically.

### Diagnostics Builder

To use `with_*` methods on `Diagnostic`, the `DiagnosticBuilder` trait must be in scope:

```rust
use crate::diagnostics::{Diagnostic, DiagnosticBuilder};
```

Error codes are defined in `compiler_errors.rs` / `runtime_errors.rs` and registered in `registry.rs`. Use `diagnostic_for(ERROR_CODE)` to create structured diagnostics.

### GC Heap

The mark-and-sweep GC (`runtime/gc/`) manages two kinds of heap objects:

- **Cons cells** — `HeapObject::Cons { head: Value, tail: Value }` — immutable linked lists, O(1) prepend. Empty list is `Value::None`.
- **HAMT maps** — `HeapObject::HamtNode` / `HeapObject::HamtCollision` — Hash Array Mapped Trie with 5 bits per level, structural sharing on insert/delete.

GC runs when `allocation_count >= gc_threshold` (default 10,000). Stop-the-world mark-and-sweep.

### Bytecode Cache

Compiled bytecode is cached as `.fxc` files under `target/flux/`. Cache keys are SHA-2 hashes of the source content + dependency graph. The `--no-cache` flag disables this.

### JIT Backend

The JIT (`src/jit/`) compiles CFG IR to native machine code via [Cranelift](https://cranelift.dev/), bypassing the bytecode compiler and VM entirely. It shares:
- The same `RuntimeContext` trait
- The same `BASE_FUNCTIONS` array
- The same GC heap

Enable with `cargo build --features jit` and run with `--jit`.

---

## Parser Structure

The parser (`syntax/parser/`) uses a **hybrid** approach:
- **Recursive descent** for top-level declarations, statements, and blocks
- **Pratt / TDOP precedence climbing** for expressions

Three-token lookahead (`current_token`, `peek_token`, `peek2_token`). Error recovery via `recover_expression_list_to_delimiter()` and `sync_to_*` functions to prevent cascading errors.

Tokens are defined via the `define_tokens!` macro in `token_type.rs`. Keywords get automatic `lookup_ident()` dispatch; symbols need explicit branches in the lexer's one-byte or two-byte dispatch tables.
