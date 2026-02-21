# Flux Compiler Architecture

This document describes the current architecture of the Flux compiler (v0.2.0).

---

## Pipeline

Flux has two execution backends that share the same front-end:

```
Source (.flx)
    │
    ▼
  Lexer              token stream
    │
    ▼
  Parser             AST  (string-interned identifiers, parse-time desugaring)
    │
    ▼
  AST Passes         constant folding · desugaring · free var collection · tail call detection
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
│   └── visitor.rs           Visitor + Folder traits for AST traversal
│
├── bytecode/                Bytecode compiler
│   ├── compiler/            AST → stack-based bytecode
│   │   └── mod.rs           Compiler struct, symbol table setup, builtin registration
│   ├── opcode.rs            ~45 opcodes (OpGetLocal, OpCall, OpMatch, ...)
│   ├── symbol_table.rs      Variable/function/builtin tracking per scope
│   └── cache.rs             .fxc bytecode cache (SHA-2 content hashing)
│
├── runtime/                 Bytecode VM and supporting runtime
│   ├── vm/                  Stack-based VM, instruction dispatch, call frames
│   │   └── test_runner.rs   --test flag: collect_test_functions, run_test_fns, reporting
│   ├── value.rs             Value enum (Integer, Float, String, Array, Gc, Closure, ...)
│   ├── builtins/            75 builtin functions, registered via BUILTINS array
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
│   ├── compiler.rs          AST → Cranelift IR
│   ├── context.rs           JIT execution context, shares GC heap with VM
│   ├── runtime_helpers.rs   Native callbacks: rt_call_builtin, GC allocation
│   └── value_arena.rs       Pointer-stable allocation for JIT values
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
    Function(Rc<CompiledFunction>), Closure(Rc<Closure>), Builtin(u8),
    JitClosure(Rc<JitClosure>),     // feature-gated
    // Internal
    ReturnValue(Rc<Value>),  // sentinel for return from blocks
    Uninit,                  // VM internal sentinel
}
```

`Value::Gc` wraps heap objects. `Display` for `Value::Gc` shows `<gc@N>` — use `list_ops::format_value()` with a `RuntimeContext` for proper rendering.

### Builtin Registration

Builtins must be registered in three places with matching indices:

1. **Implementation** in `runtime/builtins/<module>.rs`
2. **`BUILTINS` array** in `runtime/builtins/mod.rs` — the array index is the builtin's ID
3. **Symbol table** in `bytecode/compiler/mod.rs` — `symbol_table.define_builtin(INDEX, ...)`

`OpGetBuiltin` emits the index at compile time; `get_builtin_by_index()` resolves it at runtime. The JIT uses the same `BUILTINS` array via `rt_call_builtin()`, so new builtins work in both backends automatically.

### Diagnostics Builder

To use `with_*` methods on `Diagnostic`, the `DiagnosticBuilder` trait must be in scope:

```rust
use crate::diagnostics::{Diagnostic, DiagnosticBuilder};
```

Error codes are defined in `compiler_errors.rs` / `runtime_errors.rs` and registered in `registry.rs`. Use `diag_enhanced(ERROR_CODE)` to create structured diagnostics.

### GC Heap

The mark-and-sweep GC (`runtime/gc/`) manages two kinds of heap objects:

- **Cons cells** — `HeapObject::Cons { head: Value, tail: Value }` — immutable linked lists, O(1) prepend. Empty list is `Value::None`.
- **HAMT maps** — `HeapObject::HamtNode` / `HeapObject::HamtCollision` — Hash Array Mapped Trie with 5 bits per level, structural sharing on insert/delete.

GC runs when `allocation_count >= gc_threshold` (default 10,000). Stop-the-world mark-and-sweep.

### Bytecode Cache

Compiled bytecode is cached as `.fxc` files under `target/flux/`. Cache keys are SHA-2 hashes of the source content + dependency graph. The `--no-cache` flag disables this.

### JIT Backend

The JIT (`src/jit/`) compiles the AST directly to native machine code via [Cranelift](https://cranelift.dev/), bypassing the bytecode compiler and VM entirely. It shares:
- The same `RuntimeContext` trait
- The same `BUILTINS` array
- The same GC heap

Enable with `cargo build --features jit` and run with `--jit`.

---

## Parser Structure

The parser (`syntax/parser/`) uses a **hybrid** approach:
- **Recursive descent** for top-level declarations, statements, and blocks
- **Pratt / TDOP precedence climbing** for expressions

Three-token lookahead (`current_token`, `peek_token`, `peek2_token`). Error recovery via `recover_expression_list_to_delimiter()` and `sync_to_*` functions to prevent cascading errors.

Tokens are defined via the `define_tokens!` macro in `token_type.rs`. Keywords get automatic `lookup_ident()` dispatch; symbols need explicit branches in the lexer's one-byte or two-byte dispatch tables.
