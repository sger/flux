# Bytecode

> Source: `src/bytecode/`

The Flux bytecode compiler translates the AST to a compact stack-based instruction set. Compiled programs can be cached as `.fxc` files.

## Instruction Set (62 opcodes)

### Constants and Literals

| Opcode | Operand | Description |
|--------|---------|-------------|
| `OpConstant` | u16 index | Push constant from pool |
| `OpConstantLong` | u32 index | Push constant (large pool) |
| `OpTrue` | — | Push `true` |
| `OpFalse` | — | Push `false` |
| `OpNone` | — | Push `None` |

### Arithmetic

| Opcode | Description |
|--------|-------------|
| `OpAdd` | Pop two, push sum (Int+Int, Float+Float, or String+String) |
| `OpSub` | Subtract |
| `OpMul` | Multiply |
| `OpDiv` | Divide |
| `OpMod` | Modulo |
| `OpMinus` | Unary negation |

### Comparison

| Opcode | Description |
|--------|-------------|
| `OpEqual` | `==` |
| `OpNotEqual` | `!=` |
| `OpGreaterThan` | `>` |
| `OpLessThanOrEqual` | `<=` |
| `OpGreaterThanOrEqual` | `>=` |
| `OpBang` | Unary `!` (boolean negation) |

### Control Flow

| Opcode | Operand | Description |
|--------|---------|-------------|
| `OpJump` | u16 offset | Unconditional jump |
| `OpJumpNotTruthy` | u16 offset | Jump if top of stack is falsy |
| `OpJumpTruthy` | u16 offset | Jump if top of stack is truthy |
| `OpPop` | — | Discard top of stack |
| `OpReturn` | — | Return `None` from current frame |
| `OpReturnValue` | — | Return top-of-stack value |
| `OpReturnLocal` | u8 index | Return a local directly (optimized) |

### Variables

| Opcode | Operand | Description |
|--------|---------|-------------|
| `OpGetGlobal` | u16 index | Push global variable |
| `OpSetGlobal` | u16 index | Pop and store into global |
| `OpGetLocal` | u8 index | Push local variable |
| `OpSetLocal` | u8 index | Pop and store into local |
| `OpGetLocal0` | — | Push local[0] (optimized fast path) |
| `OpGetLocal1` | — | Push local[1] (optimized fast path) |
| `OpConsumeLocal` | u8 index | Move local (avoids Rc clone) |
| `OpGetBuiltin` | u8 index | Push builtin function by BUILTINS index |

### Functions and Closures

| Opcode | Operand | Description |
|--------|---------|-------------|
| `OpCall` | u8 arity | Call top-of-stack with N args |
| `OpTailCall` | u8 arity | Tail-call optimization variant |
| `OpClosure` | u16 fn\_index + captures | Create closure, capturing free vars |
| `OpClosureLong` | u32 fn\_index + captures | Closure with large function pool |
| `OpGetFree` | u8 index | Push captured free variable |
| `OpCurrentClosure` | — | Push current closure (for recursion) |

### Collections

| Opcode | Operand | Description |
|--------|---------|-------------|
| `OpArray` | u16 count | Pop N values, push `Array` |
| `OpArrayLong` | u32 count | Array (large count) |
| `OpHash` | u16 pairs | Pop N key-value pairs, push hash map |
| `OpHashLong` | u32 pairs | Hash map (large pair count) |
| `OpIndex` | — | Pop index and collection, push `Option` |
| `OpTuple` | u8 count | Pop N values, push `Tuple` |
| `OpTupleLong` | u16 count | Tuple (large count) |
| `OpTupleIndex` | u8 index | Direct tuple field access |
| `OpIsTuple` | — | Push `true` if top is a `Tuple` |

### Cons Lists

| Opcode | Description |
|--------|-------------|
| `OpCons` | Pop head and tail, push cons cell (GC-allocated) |
| `OpIsCons` | Push `true` if top is a cons cell |
| `OpIsEmptyList` | Push `true` if top is `None` / empty list |
| `OpConsHead` | Push head of cons cell |
| `OpConsTail` | Push tail of cons cell |

### Option / Either

| Opcode | Description |
|--------|-------------|
| `OpSome` | Wrap top in `Some(...)` |
| `OpIsSome` | Push `true` if top is `Some` |
| `OpUnwrapSome` | Unwrap `Some(x)` to `x` (error on `None`) |
| `OpLeft` | Wrap top in `Left(...)` |
| `OpRight` | Wrap top in `Right(...)` |
| `OpIsLeft` | Push `true` if top is `Left` |
| `OpIsRight` | Push `true` if top is `Right` |
| `OpUnwrapLeft` | Unwrap `Left(x)` to `x` |
| `OpUnwrapRight` | Unwrap `Right(x)` to `x` |

### Strings

| Opcode | Description |
|--------|-------------|
| `OpToString` | Pop value, push its string representation (for interpolation) |

---

## Stack Frame Layout

Each call frame holds:
- **Instruction pointer** — current position in the function's bytecode
- **Base pointer** — index into the stack where this frame's locals start
- **Local variables** — accessed via `OpGetLocal(index)` / `OpSetLocal(index)`
- **Closure reference** — for `OpGetFree` and `OpCurrentClosure`

Globals are shared across all frames and addressed by `OpGetGlobal(index)`.

---

## Constant Pool

Each `CompiledFunction` has its own constant pool (`Vec<Value>`). Constants include:
- Integer and float literals
- String literals
- Compiled functions (for closures and named functions)

`OpConstant(i)` and `OpConstantLong(i)` index into this pool.

---

## Bytecode Cache (`.fxc` files)

Compiled bytecode is cached under `target/flux/` to avoid recompilation on unchanged files.

**Cache key:** SHA-2 hash of the source content and its transitive dependency graph.

**Invalidation:** Any change to the source file or any of its imports invalidates the cache.

**Inspection:**
```bash
cargo run -- cache-info <file.flx>        # Show cache status for a source file
cargo run -- cache-info-file <file.fxc>   # Inspect a .fxc cache file directly
```

**Disable:** Pass `--no-cache` to skip reading and writing the cache.

---

## Disassembly

```bash
cargo run -- bytecode examples/basics/fibonacci.flx
```

Prints human-readable bytecode with operands decoded. Useful for verifying compiler output or debugging code generation.
