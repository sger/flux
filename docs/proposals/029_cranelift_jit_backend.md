# Cranelift JIT Backend for Flux

## Context

Flux currently compiles AST to custom bytecode and interprets it in a stack-based VM. Adding a Cranelift JIT backend will eliminate the dispatch loop overhead, producing native machine code while reusing the existing runtime (Value type, GC, builtins). The JIT sits alongside the VM behind a `--jit` CLI flag, gated by a `jit` Cargo feature.

## Architecture

```
Source → Lexer → Parser → AST → [Transforms] → ┬→ Bytecode Compiler → VM  (default)
                                                 └→ JIT Compiler → Native  (--jit)
```

The JIT compiles AST directly to Cranelift IR. Each Flux function becomes a native function. Values flow as `*mut Value` pointers (i64 in Cranelift). All type-checked operations delegate to `extern "C"` runtime helpers that operate on the existing `Value` enum.

### Value Passing Convention

```
JIT code works with i64 values that are pointers to heap-allocated Values.

  Runtime helper signature:  extern "C" fn(ctx: *mut JitContext, ...) -> *mut Value
  JIT function signature:    extern "C" fn(ctx: *mut JitContext, args: *const *mut Value, nargs: i64) -> *mut Value
  Error signaling:           NULL return = error (message stored in JitContext)
```

Values are allocated via a bump arena in `JitContext` (reset between top-level calls) to avoid per-value Box overhead.

### Module Structure

```
src/jit/
├── mod.rs              # Public API: JitEngine, feature gate
├── context.rs          # JitContext: arena, globals, GC heap, error state
├── compiler.rs         # AST → Cranelift IR (expressions, statements)
├── functions.rs        # Function/closure compilation, call dispatch
├── control_flow.rs     # If/else, match, pattern compilation
├── runtime_helpers.rs  # extern "C" bridge functions for Value operations
└── value_arena.rs      # Bump allocator for JIT-allocated Values
```

## Files to Create/Modify

### New Files

| File | Purpose |
|------|---------|
| `src/jit/mod.rs` | JitEngine public API |
| `src/jit/context.rs` | JitContext struct (arena + globals + gc_heap + error) |
| `src/jit/compiler.rs` | AST → Cranelift IR translation |
| `src/jit/functions.rs` | Function/closure compilation |
| `src/jit/control_flow.rs` | If/match/pattern compilation |
| `src/jit/runtime_helpers.rs` | `extern "C"` runtime bridge functions |
| `src/jit/value_arena.rs` | Bump arena for Value allocation |
| `tests/jit_tests.rs` | JIT integration tests |

### Modified Files

| File | Change |
|------|--------|
| `Cargo.toml` | Add cranelift dependencies under `[features] jit` |
| `src/lib.rs` | Add `#[cfg(feature = "jit")] pub mod jit;` |
| `src/main.rs` | Add `--jit` flag, branch to JIT execution path |

## Implementation Phases

### Phase 1: Infrastructure & Hello World

**Goal**: `cargo run --features jit -- examples/basics/print.flx --jit` prints output.

#### 1.1 Cargo.toml — Feature-gated dependencies

```toml
[features]
jit = [
    "cranelift-codegen",
    "cranelift-frontend",
    "cranelift-module",
    "cranelift-jit",
    "cranelift-native",
    "target-lexicon",
]

[dependencies]
cranelift-codegen = { version = "0.116", optional = true }
cranelift-frontend = { version = "0.116", optional = true }
cranelift-module = { version = "0.116", optional = true }
cranelift-jit = { version = "0.116", optional = true }
cranelift-native = { version = "0.116", optional = true }
target-lexicon = { version = "0.12", optional = true }
```

#### 1.2 JitContext (`context.rs`)

```rust
#[repr(C)]
pub struct JitContext {
    pub arena: ValueArena,
    pub globals: Vec<Value>,
    pub constants: Vec<Value>,
    pub gc_heap: GcHeap,
    pub error: Option<String>,
}
```

Implements `RuntimeContext` trait so builtins work unchanged.

#### 1.3 ValueArena (`value_arena.rs`)

- Pre-allocates chunks of Values
- `alloc(Value) -> *mut Value` — bump-allocates, returns stable pointer
- `reset()` — resets allocation pointer (keeps memory)
- Uses `Vec<Box<[Value]>>` chunks so pointers stay stable across allocations

#### 1.4 Runtime Helpers (`runtime_helpers.rs`)

All helpers follow the pattern: `extern "C" fn(ctx: *mut JitContext, ...) -> *mut Value`

**Value constructors:**
- `rt_make_integer(ctx, i64) -> *mut Value`
- `rt_make_float(ctx, f64) -> *mut Value`
- `rt_make_bool(ctx, bool) -> *mut Value`
- `rt_make_none(ctx) -> *mut Value`
- `rt_make_string(ctx, *const u8, len) -> *mut Value`

**Arithmetic & comparison:**
- `rt_add`, `rt_sub`, `rt_mul`, `rt_div`, `rt_mod`
- `rt_equal`, `rt_not_equal`, `rt_greater_than`, `rt_less_than_or_equal`, `rt_greater_than_or_equal`
- `rt_negate`, `rt_not`

**Builtins & calls:**
- `rt_call_builtin(ctx, builtin_index, *const *mut Value, nargs) -> *mut Value`
- `rt_print(ctx, *const *mut Value, nargs) -> *mut Value`

All return NULL on error, with the error message stored in `ctx.error`.

#### 1.5 JIT Compiler (`compiler.rs`) — Initial subset

- Integer/Float/Boolean/None/String literals
- Prefix and infix expressions (arithmetic, comparisons)
- Let bindings → Cranelift variables
- Identifier lookup → load variable / load global / load builtin
- Expression statements (compile + discard result)
- Top-level builtin calls (e.g. `print`)

#### 1.6 JitEngine (`mod.rs`)

```rust
pub struct JitEngine {
    module: JITModule,
    // ...
}

impl JitEngine {
    pub fn new() -> Self;
    pub fn compile_and_run(program: &Program, interner: &Interner) -> Result<Value, String>;
}
```

#### 1.7 main.rs Integration

- Parse `--jit` flag
- After parsing + transforms, branch: if `--jit` → `JitEngine::compile_and_run()`, else → bytecode + VM

---

### Phase 2: Control Flow

**Goal**: If/else, match expressions, logical operators work.

#### Control Flow Translation

| Flux Construct | Cranelift IR |
|---------------|-------------|
| `if/else` | Blocks with conditional `brif` branches |
| `match` | Sequential pattern checks with fallthrough to next arm |
| `&&` / `\|\|` | Short-circuit via Cranelift blocks |

#### Pattern Matching Compilation

- **Pattern check**: Call runtime helpers (`rt_is_some`, `rt_is_cons`, `rt_is_empty_list`, etc.)
- **Pattern bind**: Extract and bind to Cranelift variables (`rt_unwrap_some`, `rt_cons_head`, `rt_cons_tail`)
- **Guard expressions**: Compile guard, branch on result

#### New Runtime Helpers

- `rt_is_truthy(ctx, *mut Value) -> bool`
- `rt_is_some`, `rt_is_none`, `rt_is_cons`, `rt_is_empty_list`
- `rt_unwrap_some`, `rt_unwrap_left`, `rt_unwrap_right`
- `rt_cons_head`, `rt_cons_tail`
- `rt_is_left`, `rt_is_right`
- `rt_values_equal` (for literal pattern matching)

---

### Phase 3: Functions & Closures

**Goal**: Function definitions, calls, closures with captures, tail calls.

#### Function Compilation

Each `Function` expression → separate Cranelift function:
- Signature: `(ctx: i64, args: i64, nargs: i64) -> i64`
- Parameters read from args array
- Body compiled normally, result returned

#### Function Calls

`rt_call_value(ctx, callee, args, nargs) -> *mut Value` dispatches to:
- JIT-compiled function pointer (direct call)
- Builtin function
- Closure

#### Closures

Represented as `(function_ptr, *mut Vec<Value>)` pair:
- Free variables captured into a `Vec<Value>` at closure creation
- `rt_make_closure(ctx, fn_ptr, captures) -> *mut Value`
- `rt_get_free(closure, index) -> *mut Value`

#### Tail Calls

- Self-recursive tail calls → loop (Cranelift block jump back to entry)
- General tail calls → trampoline pattern or regular calls initially

---

### Phase 4: Collections & Full Language

**Goal**: Arrays, cons lists, HAMT maps, modules, string interpolation.

| Feature | Runtime Helper |
|---------|---------------|
| Arrays | `rt_make_array(ctx, *const *mut Value, len)` |
| Cons lists | `rt_cons(ctx, head, tail)` — allocates on GC heap |
| List literals | Desugar to cons chain or call `list` builtin |
| HAMT maps | `rt_make_hash(ctx, keys_values, npairs)` |
| Index | `rt_index(ctx, collection, key)` |
| Some/Left/Right | `rt_make_some`, `rt_make_left`, `rt_make_right` |
| String interpolation | Compile parts, call `rt_concat_strings` |
| Module system | Compile each module as namespace, resolve member access |
| Import | Load module globals into current scope |

---

## Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| **AST → Cranelift IR** (not bytecode → IR) | Preserves structured control flow, avoids decompiling bytecode back to blocks |
| **All Values as i64 pointers** | Simple, uniform representation in Cranelift. One type for everything. |
| **Arena allocation** | Avoids per-value Box allocation. Arena reset between top-level expressions. |
| **Runtime helpers for everything** | v1 prioritizes correctness over speed. Inline integer fast-paths later. |
| **Feature-gated** | `--features jit` keeps binary size small by default. Cranelift adds ~5MB. |

---

## Future Optimizations (post-v1)

- **Inline integer arithmetic** — check tag, operate directly, skip helper call
- **NaN boxing** — encode primitives in 64 bits without allocation
- **Type specialization** — monomorphize hot functions for known argument types
- **Direct calls** — bypass `call_value` dispatch when callee is known at compile time

---

## Verification

```bash
# Build with JIT feature
cargo build --features jit

# Run a simple program
cargo run --features jit -- examples/basics/print.flx --jit

# Run tests
cargo test --features jit
cargo test --features jit --test jit_tests

# Compare output: VM vs JIT should match
cargo run -- examples/basics/print.flx > /tmp/vm.out
cargo run --features jit -- examples/basics/print.flx --jit > /tmp/jit.out
diff /tmp/vm.out /tmp/jit.out

# Ensure non-JIT builds still work
cargo test
cargo build
```
