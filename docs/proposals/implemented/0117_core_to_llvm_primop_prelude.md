- Feature Name: CoreToLlvm Primop Expansion and Flux Prelude
- Start Date: 2026-03-24
- Status: Superseded by Proposal 0120 (Unified Base Library)
- Proposal PR:
- Flux Issue:

## Summary

Expand the `core_to_llvm` backend to full parity with the VM by adopting GHC's two-tier architecture: a small set of **primitive operations** backed by the C runtime (for things that cannot be written in Flux), and a **Flux prelude** (standard library written in Flux itself) for everything else. This eliminates the dependency on the 83 Rust base functions for native compilation and enables the LLVM backend to compile all existing Flux programs.

## Motivation

### The base function problem

The `core_to_llvm` backend (Proposal 0116) generates self-contained LLVM IR that links against a minimal C runtime. It currently compiles programs that use arithmetic, closures, ADTs, pattern matching, strings, and `print`/`println`. However, it cannot compile any program that uses the 83 Rust base functions (`map`, `filter`, `fold`, `len`, `range`, `sort`, `substring`, `trim`, `parse_int`, `read_lines`, etc.) because these are implemented in Rust and called via a VM-specific dispatch mechanism.

This blocks compilation of real-world programs. For example, the AoC 2024 Day 6 solution uses `len`, `substring`, `trim`, `parse_int`, `sum`, `product`, `range`, and `read_lines` — none of which compile through `core_to_llvm` today.

### The GHC lesson

GHC solves this with a clean separation:

1. **Primops** (~590 operations): Built into the compiler. These are things that *cannot* be written in Haskell — unboxed arithmetic, array mutation, I/O syscalls, GC interaction, foreign calls. They generate Cmm/machine code directly.

2. **Base library** (thousands of functions): Written in Haskell itself. `map`, `filter`, `foldl`, `length`, `show`, `read`, etc. are just Haskell functions compiled through the same pipeline as user code. The LLVM backend compiles them to native code like any other function.

The key insight: **the compiler only needs primitives for operations that touch hardware, OS, or memory layout**. Everything else is library code in the source language.

### Specific benefits

1. **Full program compilation**: Every Flux program that runs on the VM can compile through `core_to_llvm`.

2. **LLVM can optimize across boundaries**: When `map` is Flux code compiled to LLVM IR, the optimizer can inline it, unroll loops, and eliminate closures. When it's an opaque C call, none of this is possible.

3. **No Rust dependency for native binaries**: The generated binary links only against `libflux_rt.a` (C) and the Flux prelude (compiled from `.flx`). No Rust runtime, no Cargo.

4. **Single source of truth**: The prelude defines the semantics of `map`, `filter`, etc. in Flux. No need to keep Rust and C implementations in sync.

5. **User-extensible**: Users can read and understand the prelude. They can override functions or add new ones using the same mechanism.

---

## Guide-level explanation

### For Flux users

Nothing changes in how you write Flux code. The standard library functions (`map`, `filter`, `len`, `range`, etc.) work exactly as before. The difference is under the hood:

- **VM/JIT backends**: Standard library is implemented in Rust (existing behavior).
- **`--core-to-llvm` backend**: Standard library is implemented in Flux, compiled alongside your code to native LLVM IR.

```bash
# This works today (VM):
cargo run -- examples/aoc/2025/aoc_day6_part1.flx

# This will work after this proposal (native LLVM):
cargo run --features core_to_llvm -- examples/aoc/2025/aoc_day6_part1.flx --core-to-llvm --emit-binary -o aoc_day6
./aoc_day6    # Runs natively, no Flux installation needed
```

### For compiler contributors

The `core_to_llvm` backend uses two tiers of operations:

**Tier 1 — Primops (C runtime)**

Primitive operations that cannot be written in Flux. These appear as `CoreExpr::PrimOp` nodes in Core IR and lower to `declare ccc ... @flux_*` calls in LLVM IR:

```
CorePrimOp::Println  →  declare ccc void @flux_println(i64)
CorePrimOp::StringLen →  declare ccc i64  @flux_string_length(i64)
CorePrimOp::MapGet   →  declare ccc i64  @flux_hamt_get(i64, i64)
```

The C runtime (`runtime/c/`) provides the implementations. These are operations that need:
- OS interaction (I/O, time, file system)
- Memory layout knowledge (string/array/HAMT internals)
- NaN-box tag inspection (type checking)
- Allocation (GC interaction)

**Tier 2 — Flux Prelude**

Higher-level functions written in Flux source code. These are compiled through the standard pipeline (parse → type infer → Core IR → Aether → LLVM IR) and linked with user code:

```flux
// prelude/list.flx
fn map(f, list) {
    match list {
        [] -> [],
        [x, ...xs] -> [f(x), ...map(f, xs)]
    }
}
```

When `--core-to-llvm` is active, the compiler automatically includes the prelude modules before user code. The prelude functions become top-level Core definitions that LLVM compiles and optimizes alongside user code.

---

## Reference-level explanation

### Tier 1: Expanded CorePrimOp

The `CorePrimOp` enum gains new variants for operations that require C runtime support. These map 1:1 to C runtime functions.

#### New primop variants

```rust
pub enum CorePrimOp {
    // ── Existing (unchanged) ──────────────────────────
    Add, Sub, Mul, Div, Mod,
    IAdd, ISub, IMul, IDiv, IMod,
    FAdd, FSub, FMul, FDiv,
    Neg, Not, Eq, NEq, Lt, Le, Gt, Ge, And, Or,
    MakeList, MakeArray, MakeTuple, MakeHash,
    Index, MemberAccess(Identifier), TupleField(usize),
    Concat, Interpolate,

    // ── New: I/O primitives ───────────────────────────
    Print,              // flux_print(i64) -> void
    Println,            // flux_println(i64) -> void
    ReadFile,           // flux_read_file(i64) -> i64
    WriteFile,          // flux_write_file(i64, i64) -> i64
    ReadLines,          // flux_read_lines(i64) -> i64
    ReadStdin,          // flux_read_line() -> i64

    // ── New: String primitives ────────────────────────
    StringLen,          // flux_string_length(i64) -> i64
    StringConcat,       // flux_string_concat(i64, i64) -> i64
    StringSlice,        // flux_string_slice(i64, i64, i64) -> i64
    StringEq,           // flux_string_eq(i64, i64) -> i1
    IntToString,        // flux_int_to_string(i64) -> i64
    FloatToString,      // flux_float_to_string(i64) -> i64
    StringToInt,        // flux_string_to_int(i64) -> i64

    // ── New: Array primitives ─────────────────────────
    ArrayNew,           // flux_array_new(i32) -> i64
    ArrayLen,           // flux_array_len(i64) -> i64
    ArrayGet,           // flux_array_get(i64, i64) -> i64
    ArraySet,           // flux_array_set(i64, i64, i64) -> i64
    ArrayPush,          // flux_array_push(i64, i64) -> i64
    ArrayConcat,        // flux_array_concat(i64, i64) -> i64

    // ── New: HAMT (map) primitives ────────────────────
    MapNew,             // flux_hamt_empty() -> i64
    MapGet,             // flux_hamt_get(i64, i64) -> i64
    MapSet,             // flux_hamt_set(i64, i64, i64) -> i64
    MapDelete,          // flux_hamt_delete(i64, i64) -> i64
    MapHas,             // flux_hamt_contains(i64, i64) -> i64
    MapSize,            // flux_hamt_size(i64) -> i64
    MapKeys,            // flux_hamt_keys(i64) -> i64
    MapValues,          // flux_hamt_values(i64) -> i64

    // ── New: Type inspection ──────────────────────────
    TypeOf,             // inspect NaN-box tag -> string
    IsInt,              // tag == INTEGER
    IsFloat,            // not a NaN-box (raw double)
    IsString,           // tag == BOXED_VALUE && string layout
    IsBool,             // tag == BOOLEAN
    IsNone,             // tag == NONE
    IsList,             // tag == EMPTY_LIST || tag == BOXED(Cons)

    // ── New: Numeric ──────────────────────────────────
    Abs,                // inline: select on sign
    Min,                // inline: icmp + select
    Max,                // inline: icmp + select

    // ── New: Control ──────────────────────────────────
    Panic,              // flux_panic(i64) -> noreturn
    ClockNow,           // flux_clock_now() -> i64
}
```

#### C runtime additions

The C runtime (`runtime/c/`) gains:

| File | New functions |
|------|--------------|
| `flux_rt.c` | `flux_read_lines`, `flux_panic`, `flux_clock_now` |
| `string.c` | Already complete |
| `hamt.c` | `flux_hamt_keys`, `flux_hamt_values` |
| `array.c` (new) | `flux_array_new`, `flux_array_len`, `flux_array_get`, `flux_array_set`, `flux_array_push`, `flux_array_concat` |

#### Codegen mapping

Each new primop maps to a simple pattern in `lower_primop`:

```rust
CorePrimOp::Println => self.lower_c_call("flux_println", args, false),
CorePrimOp::StringLen => self.lower_c_call("flux_string_length", args, true),
CorePrimOp::MapGet => self.lower_c_call("flux_hamt_get", args, true),
CorePrimOp::IsInt => self.lower_tag_check(args, NanTag::Integer),
CorePrimOp::Abs => self.lower_inline_abs(args),
CorePrimOp::Min => self.lower_inline_min_max(args, LlvmCmpOp::Slt),
```

The `lower_c_call` helper:
1. Lowers all arguments
2. Ensures the C function is declared in the module
3. Emits `call ccc ... @flux_*(args)`
4. Returns the result (or unit for void functions)

### Tier 2: Flux Prelude

#### Prelude structure

```
prelude/
├── list.flx          List operations: map, filter, fold, reverse, zip, ...
├── numeric.flx       sum, product, abs, min, max (list versions)
├── string.flx        trim, upper, lower, split, join, starts_with, ...
├── option.flx        unwrap, map_option, flat_map_option, ...
├── either.flx        map_left, map_right, from_left, from_right, ...
├── assert.flx        assert_eq, assert_true, assert_false, ...
└── io.flx            read_lines (wrapper around ReadFile + split)
```

#### Example: `prelude/list.flx`

```flux
fn map(f, list) {
    match list {
        [] -> [],
        [x, ...xs] -> [f(x), ...map(f, xs)]
    }
}

fn filter(f, list) {
    match list {
        [] -> [],
        [x, ...xs] -> if f(x) { [x, ...filter(f, xs)] } else { filter(f, xs) }
    }
}

fn fold(f, acc, list) {
    match list {
        [] -> acc,
        [x, ...xs] -> fold(f, f(acc, x), xs)
    }
}

fn len(list) {
    fn go(l, acc) {
        match l {
            [] -> acc,
            [_, ...xs] -> go(xs, acc + 1)
        }
    }
    go(list, 0)
}

fn range(start, stop) {
    if start >= stop { [] }
    else { [start, ...range(start + 1, stop)] }
}

fn reverse(list) {
    fold(fn(acc, x) { [x, ...acc] }, [], list)
}

fn concat_lists(a, b) {
    match a {
        [] -> b,
        [x, ...xs] -> [x, ...concat_lists(xs, b)]
    }
}

fn flat_map(f, list) {
    match list {
        [] -> [],
        [x, ...xs] -> concat_lists(f(x), flat_map(f, xs))
    }
}

fn any(f, list) {
    match list {
        [] -> false,
        [x, ...xs] -> if f(x) { true } else { any(f, xs) }
    }
}

fn all(f, list) {
    match list {
        [] -> true,
        [x, ...xs] -> if f(x) { all(f, xs) } else { false }
    }
}

fn find(f, list) {
    match list {
        [] -> None,
        [x, ...xs] -> if f(x) { Some(x) } else { find(f, xs) }
    }
}

fn zip(a, b) {
    match a {
        [] -> [],
        [x, ...xs] -> match b {
            [] -> [],
            [y, ...ys] -> [(x, y), ...zip(xs, ys)]
        }
    }
}

fn flatten(list) {
    match list {
        [] -> [],
        [x, ...xs] -> concat_lists(x, flatten(xs))
    }
}

fn take(n, list) {
    if n <= 0 { [] }
    else {
        match list {
            [] -> [],
            [x, ...xs] -> [x, ...take(n - 1, xs)]
        }
    }
}

fn drop_list(n, list) {
    if n <= 0 { list }
    else {
        match list {
            [] -> [],
            [_, ...xs] -> drop_list(n - 1, xs)
        }
    }
}

fn sort(list) {
    match list {
        [] -> [],
        [pivot, ...rest] -> {
            let lo = filter(fn(x) { x < pivot }, rest);
            let hi = filter(fn(x) { x >= pivot }, rest);
            concat_lists(sort(lo), [pivot, ...sort(hi)])
        }
    }
}

fn contains(list, x) {
    any(fn(item) { item == x }, list)
}

fn first(list) {
    match list { [x, ..._] -> Some(x), _ -> None }
}

fn last(list) {
    match list {
        [] -> None,
        [x] -> Some(x),
        [_, ...xs] -> last(xs)
    }
}

fn rest(list) {
    match list { [_, ...xs] -> xs, _ -> [] }
}

fn count(f, list) {
    fold(fn(acc, x) { if f(x) { acc + 1 } else { acc } }, 0, list)
}
```

#### Example: `prelude/numeric.flx`

```flux
fn sum(list) {
    fold(fn(a, b) { a + b }, 0, list)
}

fn product(list) {
    fold(fn(a, b) { a * b }, 1, list)
}

fn max_list(list) {
    match list {
        [x] -> x,
        [x, ...xs] -> {
            let m = max_list(xs);
            if x > m { x } else { m }
        },
        _ -> 0
    }
}

fn min_list(list) {
    match list {
        [x] -> x,
        [x, ...xs] -> {
            let m = min_list(xs);
            if x < m { x } else { m }
        },
        _ -> 0
    }
}
```

#### Example: `prelude/string.flx`

These wrap string primops that need the C runtime:

```flux
fn trim(s) {
    // Implemented as a primop call since it needs character-level access
    __primop_trim(s)
}

fn split(s, delim) {
    __primop_split(s, delim)
}

fn join(list, sep) {
    match list {
        [] -> "",
        [x] -> to_string(x),
        [x, ...xs] -> to_string(x) ++ sep ++ join(xs, sep)
    }
}

fn starts_with(s, prefix) {
    substring(s, 0, len(prefix)) == prefix
}

fn ends_with(s, suffix) {
    let slen = string_length(s);
    let plen = string_length(suffix);
    if plen > slen { false }
    else { substring(s, slen - plen, slen) == suffix }
}

fn upper(s) { __primop_upper(s) }
fn lower(s) { __primop_lower(s) }

fn chars(s) {
    fn go(i) {
        if i >= string_length(s) { [] }
        else { [substring(s, i, i + 1), ...go(i + 1)] }
    }
    go(0)
}
```

#### Example: `prelude/assert.flx`

```flux
fn assert_eq(a, b) with IO {
    if a == b { () }
    else { panic("assert_eq failed") }
}

fn assert_true(x) with IO {
    if x { () }
    else { panic("assert_true failed") }
}

fn assert_false(x) with IO {
    if !x { () }
    else { panic("assert_false failed") }
}
```

#### Auto-inclusion mechanism

When `--core-to-llvm` is active, the compiler prepends the prelude modules to the module graph before lowering:

```rust
// In main.rs, core_to_llvm dispatch block:
if use_core_to_llvm {
    // Load prelude modules
    let prelude_dir = locate_prelude_dir();
    let prelude_files = ["list.flx", "numeric.flx", "string.flx", "assert.flx"];
    for file in prelude_files {
        let path = prelude_dir.join(file);
        if path.exists() {
            // Parse and add to module graph before user code
            graph.add_prelude_module(&path)?;
        }
    }
    // ... then lower to Core IR and compile as normal
}
```

The prelude functions become top-level Core definitions. If user code defines a function with the same name, the user's version shadows the prelude.

### AST-to-Core lowering changes

Currently, base functions like `len`, `map`, `filter` appear in Core IR as:

```
aether_call[borrowed] len(list)     ← CoreExpr::AetherCall { func: Var("len"), ... }
```

Where `Var("len")` has `binder: None` (unresolved external reference). The AST-to-Core lowering must be updated to:

1. **For operations that become primops** (I/O, string, array, map primitives): Lower them to `CoreExpr::PrimOp` nodes instead of `AetherCall`. This requires recognizing known function names during AST-to-Core lowering and emitting the appropriate `CorePrimOp` variant.

2. **For operations that become prelude functions** (`map`, `filter`, `fold`, etc.): These resolve naturally once the prelude is included in the module graph — they become top-level Core definitions with binders, so `AetherCall { func: Var("map") }` resolves to `AetherCall { func: Var("map", binder: Some(prelude_map_id)) }`.

### Migration path for base functions

| Current base function | New location | Mechanism |
|-----------------------|-------------|-----------|
| `print`, `println` | CorePrimOp::Print/Println | C runtime call |
| `read_file`, `read_lines`, `read_stdin` | CorePrimOp::ReadFile/ReadLines/ReadStdin | C runtime call |
| `to_string` | CorePrimOp::IntToString | C runtime call |
| `substring` | CorePrimOp::StringSlice | C runtime call |
| `parse_int` | CorePrimOp::StringToInt | C runtime call |
| `trim`, `upper`, `lower`, `split` | CorePrimOp + C runtime | C runtime call |
| `len` (list) | prelude/list.flx | Flux code |
| `len` (string) | CorePrimOp::StringLen | C runtime call |
| `len` (array) | CorePrimOp::ArrayLen | C runtime call |
| `map`, `filter`, `fold` | prelude/list.flx | Flux code |
| `range`, `reverse`, `sort` | prelude/list.flx | Flux code |
| `sum`, `product` | prelude/numeric.flx | Flux code |
| `contains`, `any`, `all`, `find` | prelude/list.flx | Flux code |
| `zip`, `flatten`, `flat_map` | prelude/list.flx | Flux code |
| `assert_eq`, `assert_true`, ... | prelude/assert.flx | Flux code |
| `put`, `get`, `has_key`, `keys`, `values` | CorePrimOp::Map* | C runtime call |
| `type_of`, `is_int`, `is_string`, ... | CorePrimOp::TypeOf/Is* | Inline LLVM IR |
| `abs`, `min`, `max` | CorePrimOp::Abs/Min/Max | Inline LLVM IR |
| `now_ms` | CorePrimOp::ClockNow | C runtime call |
| `panic` | CorePrimOp::Panic | C runtime call |

### Polymorphic `len` dispatch

The `len` function is polymorphic — it works on lists, strings, and arrays. In the prelude, this is handled by making `len` a primop that dispatches at runtime:

```rust
CorePrimOp::Len => {
    // Check NaN-box tag to determine type, then call appropriate C function
    // - List: walk cons cells and count (or call flux_list_len)
    // - String: call flux_string_length
    // - Array: call flux_array_len
    self.lower_polymorphic_len(args)
}
```

Alternatively, `len` can be a C runtime function that inspects the tag and dispatches internally. This is simpler and matches how the VM does it.

### Implementation phases

**Phase 1 — Primop expansion** (~3 days)
- Add new `CorePrimOp` variants
- Add `lower_primop` cases → C runtime `declare` calls
- Add missing C runtime functions (`array.c`, `flux_read_lines`, `flux_panic`, etc.)
- Test: `print`, `println`, `read_file`, string ops, HAMT ops work end-to-end

**Phase 2 — Flux prelude: list operations** (~3 days)
- Write `prelude/list.flx` with `map`, `filter`, `fold`, `len`, `range`, `reverse`, `sort`, `zip`, `flatten`, `any`, `all`, `find`, `take`, `drop`, `contains`
- Auto-include mechanism in `--core-to-llvm` path
- Test: list-heavy examples compile and produce correct output

**Phase 3 — Flux prelude: remaining modules** (~2 days)
- `prelude/numeric.flx` — `sum`, `product`, `min_list`, `max_list`
- `prelude/string.flx` — `trim`, `split`, `join`, `starts_with`, `ends_with`, `chars`
- `prelude/assert.flx` — test framework functions
- `prelude/io.flx` — `read_lines` (wrapper around ReadFile + split)

**Phase 4 — Parity testing** (~3 days)
- Run all `examples/` through both VM and `--core-to-llvm`
- Parity script: compare output of every example across backends
- Fix remaining gaps
- Benchmark: measure speedup on compute-heavy examples (fibonacci, AoC puzzles)

---

## Drawbacks

- **Two implementations of standard library**: The VM uses Rust base functions; `core_to_llvm` uses Flux prelude. Semantics must stay in sync. Mitigated by parity testing.

- **Prelude compilation overhead**: The prelude adds ~200-500 lines of Flux code that gets compiled alongside every program. LLVM's dead code elimination removes unused functions, but parsing and Core lowering still happen. Mitigated by caching compiled prelude modules.

- **Polymorphic dispatch at runtime**: Operations like `len` need to check the NaN-box tag at runtime to determine the type. This adds a branch per call. LLVM can sometimes eliminate this via type specialization, but not always.

- **Prelude functions are not as optimized as Rust**: The Rust `map`/`filter`/`fold` implementations may use iterator adaptors and other optimizations not available in Flux. However, LLVM's optimizer compensates — it can inline, unroll, and vectorize the Flux implementations.

---

## Rationale and alternatives

### Why follow GHC's architecture?

GHC has maintained this exact architecture for 15+ years across hundreds of thousands of Haskell programs. The separation of "primops for hardware" and "library for algorithms" is proven to scale.

### Alternative: Implement everything in C

We could implement all 83 base functions in C (`runtime/c/`) and call them from LLVM IR. This avoids writing a Flux prelude but has serious downsides:
- LLVM cannot optimize across C call boundaries (opaque calls block inlining)
- Higher-order functions (`map`, `filter`, `fold`) require calling Flux closures from C, which needs a complex calling convention bridge
- Duplicates logic between Rust (VM) and C (native) — three implementations total

### Alternative: Implement everything as primops

We could add all 83 functions as `CorePrimOp` variants. This works but:
- Bloats the compiler with operations that are purely algorithmic (quicksort, zip, flatten)
- Every new standard library function requires compiler changes
- Misses LLVM optimization opportunities (inlining, specialization)

### Alternative: Compile Rust base functions to LLVM

We could use Rust's LLVM output to link Flux programs against the existing Rust base functions. This is technically possible but:
- Pulls in Rust's `libstd`, allocation, and panic infrastructure
- The ABI between NaN-boxed Flux values and Rust `Value` enum is complex
- Defeats the goal of self-contained binaries with minimal dependencies

### Why the chosen design is best

The two-tier approach minimizes compiler complexity (primops are simple C calls), maximizes LLVM optimization (prelude is visible IR), and follows a proven architecture (GHC).

---

## Prior art

### GHC (Haskell)

GHC's LLVM backend has used this exact architecture since 2010. Primops (~590) are defined in `primops.txt.pp` and generate Cmm directly. The base library (`Data.List`, `Data.Map`, `Prelude`, etc.) is Haskell compiled through the same pipeline. The RTS (~50K lines C) provides GC, I/O, and concurrency primitives.

### Lean 4

Lean 4 compiles to C, with a small C runtime for GC, I/O, and object allocation (~3000 lines). Standard library functions (`List.map`, `Array.push`, etc.) are written in Lean and compiled to C. Lean demonstrates that a dependently-typed functional language can use this self-hosting approach effectively.

### Koka

Koka compiles to C with Perceus reference counting (the inspiration for Flux's Aether). Standard library functions are written in Koka. The C backend sees the full program including library code, enabling whole-program optimization. This is the same principle as our Flux prelude + LLVM optimization.

### OCaml

OCaml's native compiler has a fixed set of primitives (`caml_alloc`, `caml_apply`, `caml_compare`, I/O functions) provided by the runtime. Standard library modules (`List`, `Array`, `String`, `Map`) are written in OCaml and compiled to native code.

---

## Unresolved questions

- **Polymorphic `len`**: Should `len` be a single primop with runtime dispatch, or should the type inferencer specialize `len(string)` vs `len(list)` at compile time? Runtime dispatch is simpler; compile-time specialization is faster.

- **String operations that need character iteration**: Functions like `trim`, `upper`, `lower`, `split` need character-level string access. Should these be C primops or Flux functions that use `substring` and `string_length` primops? C is faster; Flux is more transparent.

- **Prelude versioning**: If the prelude changes between Flux versions, should compiled binaries be re-linked? Since the prelude is compiled from source each time, this is automatically handled — but cached prelude IR would need invalidation.

- **Name shadowing**: If a user defines `fn map(...)` in their code, it should shadow the prelude's `map`. The current module system handles this naturally, but we need to verify priority ordering.

- **`Index` primop polymorphism**: `list[0]` vs `array[0]` vs `map["key"]` — should `Index` be a single primop with runtime dispatch, or three separate primops?

---

## Future possibilities

- **Prelude optimization passes**: A dedicated Core pass could specialize prelude calls based on known types. For example, `len(range(0, n))` could be constant-folded to `n` without executing the list construction.

- **Self-hosting the prelude**: Once the prelude is stable, it can be pre-compiled to LLVM bitcode and linked directly, eliminating recompilation overhead. This mirrors GHC's approach with pre-compiled base library `.o` files.

- **Whole-program optimization**: With both user code and prelude as LLVM IR, `llvm-link` can merge all modules and `opt -O2` can perform whole-program optimization (cross-module inlining, dead code elimination).

- **Cross-compilation**: Since the prelude is Flux source compiled to LLVM IR, cross-compilation works automatically: `llc --target=aarch64-unknown-linux-gnu` produces ARM binaries from the same IR.

- **Package system**: The prelude mechanism generalizes to a package system — external Flux libraries compile through the same pipeline and link at the LLVM IR level.

- **Incremental compilation**: Prelude modules that haven't changed can be cached as `.bc` (LLVM bitcode) files, similar to GHC's `.o` caching for library modules.
