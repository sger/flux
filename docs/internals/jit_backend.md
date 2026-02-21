# JIT Backend

> Source: `src/jit/` — requires `--features jit` to build

The JIT backend compiles the Flux AST directly to native machine code using [Cranelift](https://cranelift.dev/), bypassing the bytecode compiler and VM entirely.

## Building and Running

```bash
cargo build --features jit
cargo run --features jit -- examples/basics/fibonacci.flx --jit
cargo run --features jit -- --test examples/tests/array_test.flx --jit
```

## Architecture

```
AST
 │
 ▼
JIT Compiler (src/jit/compiler.rs)
 │   Cranelift IR (CLIF)
 ▼
Cranelift Codegen
 │   native machine code
 ▼
JIT Context (src/jit/context.rs)
 │   executes via function pointer
 ▼
Value Arena (src/jit/value_arena.rs)
 │   pointer-stable allocations
 ▼
Runtime Helpers (src/jit/runtime_helpers.rs)
    extern "C" callbacks for GC, builtins, closures
```

## JIT Compiler (`compiler.rs`)

Translates each Flux AST function to Cranelift IR. Key points:

- All `Value` pointers are `i64` in Cranelift IR (`PTR_TYPE = types::I64`).
- Functions are compiled independently and linked via `FuncId`.
- Literal deduplication: function literals at the same source span (tracked by `LiteralKey`) are compiled once.
- Closures capture free variables as extra pointer arguments injected at call sites.

The compiler emits calls to runtime helper functions for anything that requires heap allocation or builtin dispatch.

## Runtime Helpers (`runtime_helpers.rs`)

A set of `extern "C"` functions callable from JIT-compiled code. Convention:

- First parameter: `*mut JitContext` — for allocation, error reporting, and GC access.
- Return: `*mut Value` (or null on error, with message stored in `ctx.error`).

Key helpers:

| Helper | Purpose |
|--------|---------|
| `rt_make_integer(ctx, i64)` | Allocate `Value::Integer` in the value arena |
| `rt_make_float(ctx, i64)` | Allocate `Value::Float` (bits passed as i64) |
| `rt_make_bool(ctx, i64)` | Allocate `Value::Boolean` |
| `rt_make_none(ctx)` | Allocate `Value::None` |
| `rt_call_builtin(ctx, index, args, nargs)` | Call `BUILTINS[index]` with argument array |
| `rt_make_closure(ctx, func_ptr, captures, ncap)` | Allocate a `JitClosure` |
| `rt_cons(ctx, head, tail)` | Allocate a cons cell on the GC heap |
| `rt_hamt_put(ctx, map, key, val)` | HAMT insert, returns new map handle |
| `rt_hamt_get(ctx, map, key)` | HAMT lookup, returns `Value::Some` or `Value::None` |

## Value Arena (`value_arena.rs`)

JIT-compiled code works with raw `*mut Value` pointers. The value arena provides pointer-stable allocation — addresses do not move after allocation, which is required for Cranelift's `i64` pointer model.

The arena is owned by `JitContext` and lives for the duration of program execution.

## JIT Context (`context.rs`)

`JitContext` is the JIT equivalent of the VM's runtime state. It holds:

- The value arena
- The GC heap (same `GcHeap` type as the VM uses)
- The string interner (shared with the compiler)
- Error state (`Option<String>`) for propagating runtime errors from helpers back to Rust

## Shared Infrastructure

The JIT reuses the same components as the VM:

| Component | Shared via |
|-----------|-----------|
| Builtin functions | `BUILTINS` array in `runtime/builtins/mod.rs` |
| GC heap | `runtime/gc/gc_heap.rs` — `JitContext` owns a `GcHeap` |
| `RuntimeContext` trait | Both VM and JIT context implement it |
| Error codes | `diagnostics/` — same error codes and messages |

This means adding a new builtin automatically makes it available in JIT mode — no JIT-specific code needed.

## Limitations

- `--trace` is not supported in JIT mode (no bytecode to trace).
- Bytecode cache (`.fxc` files) is not used in JIT mode — each run recompiles.
- Some language features may have partial JIT support — check `src/jit/compiler.rs` for unimplemented arms.
