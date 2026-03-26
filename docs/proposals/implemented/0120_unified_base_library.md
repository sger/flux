- Feature Name: Unified Base Library — Single Source in Flux
- Start Date: 2026-03-24
- Status: Implemented
- Proposal PR:
- Flux Issue:

## Summary

Replace the 83 Rust base functions in `src/runtime/base/` with a single Flux standard library at `lib/Base/`. Both the VM bytecode backend and the `core_to_llvm` native backend compile the same Flux source code through their respective pipelines. This eliminates the duplication between Rust and Flux/C implementations, establishes `lib/Base/` as the single source of truth for standard library semantics, and reduces the codebase by ~3,000 lines of Rust.

## Motivation

### The duplication problem

Today, standard library functions exist in three places:

| Function | VM (Rust) | core_to_llvm (C runtime) | core_to_llvm (Flux) |
|----------|-----------|--------------------------|---------------------|
| `map` | `src/runtime/base/higher_order_ops.rs` | — | `lib/Base/List.flx` |
| `filter` | `src/runtime/base/higher_order_ops.rs` | — | `lib/Base/List.flx` |
| `len` | `src/runtime/base/collection_ops.rs` | `runtime/c/array.c` | `lib/Base/List.flx` |
| `trim` | `src/runtime/base/string_ops.rs` | `runtime/c/flux_rt.c` | `lib/Base/String.flx` |
| `sort` | `src/runtime/base/collection_ops.rs` | — | `lib/Base/List.flx` |
| `abs` | `src/runtime/base/numeric_ops.rs` | `runtime/c/flux_rt.c` | — |

Three implementations of the same semantics. When behavior changes, all three must be updated. When they diverge, bugs appear only in specific backends.

### The GHC model

GHC has exactly **one** base library — written in Haskell, compiled by the same compiler:

```
libraries/base/Data/List.hs  →  GHCi: compiled to bytecode
                              →  GHC:  compiled to native code (NCG or LLVM)
```

One source, two compilation targets. No duplication. The only non-Haskell code is the RTS (runtime system) — ~50K lines of C providing GC, I/O primitives, and the scheduler. Everything algorithmic (`map`, `filter`, `foldl`, `sort`, `show`) is Haskell.

### What this proposal achieves

1. **Single source of truth**: `lib/Base/*.flx` defines standard library semantics. Period.
2. **Zero Rust base functions**: Remove `src/runtime/base/` (~3,000 lines of Rust, 12 files).
3. **Both backends use the same code**: VM compiles `lib/Base/` to bytecode; `core_to_llvm` compiles it to LLVM IR.
4. **Semantic equivalence by construction**: Both backends produce the same results because they compile the same source.
5. **Easier contribution**: Contributors write Flux, not Rust, to add standard library functions.

---

## Guide-level explanation

### For Flux users

No visible changes. Standard library functions work exactly as before:

```flux
fn main() with IO {
    let nums = range(1, 10);
    let evens = filter(nums, \x -> x % 2 == 0);
    println(sum(evens))    // 20
}
```

The only difference is internal: `map`, `filter`, `range`, `sum` are now Flux functions compiled by the same compiler, not Rust functions called via FFI.

### For compiler contributors

The standard library lives at `lib/Base/`:

```
lib/Base/
├── List.flx       map, filter, fold, len, range, reverse, sort, zip, ...
├── Numeric.flx    sum, product, max_list, min_list
├── String.flx     starts_with, ends_with, chars, join, replace, ...
├── Option.flx     unwrap, unwrap_or, map_option, is_some
├── Assert.flx     assert_eq, assert_true, assert_false, ...
└── IO.flx         print_all, println_all
```

Both backends load these files automatically:

```
VM path:          parse lib/Base/*.flx → type infer → bytecode → prepend to VM
core_to_llvm:     parse lib/Base/*.flx → type infer → Core IR → LLVM IR → link
```

**What stays in Rust/C (true primitives):**

Only operations that cannot be expressed in Flux remain as primitives:

| Operation | Why it's a primitive |
|-----------|---------------------|
| `print`, `println` | OS syscall (`write(2)`) |
| `read_file`, `write_file` | OS syscall (`open`/`read`/`write`) |
| `string_concat`, `string_slice`, `string_length` | Needs memory layout knowledge (FluxString struct) |
| `array_new`, `array_get`, `array_set`, `array_len` | Needs memory layout knowledge (FluxArray struct) |
| `hamt_get`, `hamt_set`, `hamt_delete` | Complex persistent data structure internals |
| `panic` | Process abort |
| `clock_now` | OS syscall (`clock_gettime`) |
| Integer/float arithmetic | CPU instructions |

These are `CorePrimOp` variants backed by inline LLVM IR or C runtime calls. They are **not** in `lib/Base/` — they're compiler built-ins, like GHC's `+#`, `readArray#`, `catch#`.

---

## Reference-level explanation

### Architecture after this proposal

```
lib/Base/*.flx                          (Flux source — single source of truth)
    │
    ├── VM bytecode backend:
    │   parse → HM type infer → compile to bytecode
    │   bytecode prepended to user program before execution
    │   Base functions become normal bytecode functions
    │   called via standard function call mechanism (no OpGetBase)
    │
    └── core_to_llvm backend:
        parse → HM type infer → Core IR → Aether → LLVM IR
        LLVM IR linked with user code
        Base functions become native code
        LLVM can inline across Base ↔ user code boundaries

src/primop/mod.rs                       (compiler built-ins — cannot be Flux)
    │
    ├── VM:          inline bytecode instructions
    └── core_to_llvm: inline LLVM IR or C runtime declare calls

runtime/c/                              (C runtime — only for core_to_llvm)
    flux_rt.c    I/O, init/shutdown
    string.c     string memory operations
    array.c      array memory operations
    hamt.c       persistent hash map
    gc.c         allocator
```

### Files removed

| Path | Lines | Purpose |
|------|-------|---------|
| `src/runtime/base/array_ops.rs` | ~200 | Array operations → `lib/Base/List.flx` + primops |
| `src/runtime/base/collection_ops.rs` | ~300 | List/collection ops → `lib/Base/List.flx` |
| `src/runtime/base/hash_ops.rs` | ~200 | Hash map ops → primops (MapGet/MapSet) |
| `src/runtime/base/higher_order_ops.rs` | ~400 | map/filter/fold → `lib/Base/List.flx` |
| `src/runtime/base/io_ops.rs` | ~100 | I/O → primops (Print/ReadFile) |
| `src/runtime/base/list_ops.rs` | ~200 | Cons list helpers → `lib/Base/List.flx` |
| `src/runtime/base/numeric_ops.rs` | ~100 | abs/min/max → primops or `lib/Base/Numeric.flx` |
| `src/runtime/base/string_ops.rs` | ~300 | String ops → primops + `lib/Base/String.flx` |
| `src/runtime/base/type_ops.rs` | ~150 | type_of/is_* → primops |
| `src/runtime/base/assert_ops.rs` | ~300 | Assertions → `lib/Base/Assert.flx` |
| `src/runtime/base/registry.rs` | ~150 | Base function registry → removed |
| `src/runtime/base/mod.rs` | ~100 | Module root → removed |
| **Total** | **~2,500** | |

### VM changes

The VM currently dispatches base functions via:

```rust
OpCode::OpGetBase => {
    let idx = read_u8!();
    let base_fn = get_base_function_by_index(idx);
    // Push base_fn closure onto stack
}
```

After this proposal, `OpGetBase` is removed. Base functions are normal bytecode functions loaded from `lib/Base/`. The VM's startup sequence becomes:

```rust
fn run_program(user_source: &str) {
    // 1. Load and compile Base library
    let base_bytecode = compile_base_library();

    // 2. Compile user program
    let user_bytecode = compile_user_program(user_source);

    // 3. Prepend base functions to user program
    let full_bytecode = merge(base_bytecode, user_bytecode);

    // 4. Execute
    vm.execute(full_bytecode);
}
```

Base functions are resolved by name during compilation — when the user writes `map(list, f)`, the compiler finds `map` in the Base library's symbol table, just like any other imported function.

### Separate compilation of Base library

To avoid recompiling the Base library for every user program, the compiled form is cached:

**For the VM:**
```
lib/Base/*.flx → parse → type infer → bytecode → cache as lib/Base/.cache/base.fxc
```

The cached bytecode is loaded on subsequent runs without re-parsing or re-inferring. Cache invalidation: hash of `lib/Base/*.flx` source files.

**For core_to_llvm:**
```
lib/Base/*.flx → parse → type infer → Core IR → LLVM IR → cache as lib/Base/.cache/base.ll
```

The cached `.ll` is linked with user code via `llvm-link` or concatenation. Alternatively, compile to `.bc` (bitcode) for faster loading.

### HM type inference for Base library

The Base library contains polymorphic recursive functions (`map`, `filter`, `fold`). These require Hindley-Milner inference to resolve type schemes. The inference must run separately from user code to avoid:

1. **Performance**: Re-inferring 30+ polymorphic functions for every user program is wasteful.
2. **Constraint explosion**: Combining polymorphic library definitions with user code creates unnecessarily large constraint systems.

The solution: **compile the Base library as a separate compilation unit** with its own inference pass. Export the inferred type schemes (like GHC's `.hi` interface files) so user code can reference them without re-inferring.

```
lib/Base/List.flx
    → HM inference: map : (List a, (a -> b)) -> List b
    → Export type scheme to interface
    → Compile to Core IR / bytecode

user/program.flx
    → Import Base.List type schemes
    → HM inference for user code (instantiates map's scheme)
    → Compile to Core IR / bytecode
    → Link with Base.List compiled output
```

### Primop promotion

Functions currently in `src/runtime/base/` that need hardware/memory access become `CorePrimOp` variants:

```rust
// New CorePrimOp variants (added to existing enum):
pub enum CorePrimOp {
    // ... existing ...

    // I/O (OS syscalls)
    Print,
    Println,
    ReadFile,
    WriteFile,
    ReadStdin,

    // String memory operations
    StringLen,
    StringConcat,
    StringSlice,
    StringEq,
    IntToString,
    FloatToString,
    StringToInt,

    // Array memory operations
    ArrayNew,
    ArrayLen,
    ArrayGet,
    ArraySet,
    ArrayPush,

    // HAMT memory operations
    MapNew,
    MapGet,
    MapSet,
    MapDelete,
    MapHas,
    MapSize,

    // Type tag inspection
    TypeOf,
    IsInt,
    IsFloat,
    IsString,
    IsBool,
    IsNone,

    // Control
    Panic,
    ClockNow,
}
```

AST-to-Core lowering recognizes these names and emits `CorePrimOp` nodes instead of function calls. Both backends handle them:

- **VM**: `CorePrimOp::Println` → `OpCode::OpPrimOp(PrimOp::Println)` → inline bytecode
- **core_to_llvm**: `CorePrimOp::Println` → `call ccc void @flux_println(i64 %val)`

### Migration path for `src/runtime/base/`

| Phase | Action | Files affected |
|-------|--------|---------------|
| **1** | Promote I/O, string, array, HAMT ops to `CorePrimOp` | `src/core/mod.rs`, `src/primop/mod.rs` |
| **2** | Make VM load `lib/Base/*.flx` as bytecode prelude | `src/main.rs`, `src/bytecode/compiler/` |
| **3** | Implement separate compilation + caching for Base | `src/bytecode/compiler/`, `src/main.rs` |
| **4** | Remove `src/runtime/base/*.rs` one module at a time | Test each removal against all examples |
| **5** | Remove `OpGetBase` bytecode instruction | `src/bytecode/`, `src/bytecode/vm/` |
| **6** | Update all tests that reference base function indices | `tests/` |

### Implementation phases

**Phase 1 — Primop promotion** (~1 week)
- Add new `CorePrimOp` variants for I/O, string, array, HAMT, type inspection
- Update AST→Core lowering to recognize builtin names and emit primops
- Update VM bytecode compiler to handle new primops
- Update `core_to_llvm` `lower_primop` to emit C runtime calls
- Test: all current examples still work on VM

**Phase 2 — VM prelude loading** (~1 week)
- Implement separate compilation of `lib/Base/*.flx` to bytecode
- Cache compiled bytecode in `lib/Base/.cache/base.fxc`
- Prepend Base bytecode to user program before VM execution
- Test: Base library functions work as bytecode functions
- Remove corresponding Rust implementations one by one

**Phase 3 — core_to_llvm prelude loading** (~3 days)
- Fix HM inference hang with polymorphic prelude functions
- Implement separate compilation of `lib/Base/*.flx` to Core IR
- Cache compiled Core IR or LLVM IR
- Link Base functions with user code in core_to_llvm pipeline
- Test: all examples compile and run natively

**Phase 4 — Cleanup** (~3 days)
- Delete `src/runtime/base/` entirely
- Remove `OpGetBase` from bytecode format
- Remove base function registry
- Update CLAUDE.md and documentation
- Remove `BASE_FUNCTIONS` static array

---

## Drawbacks

- **Startup cost for VM**: Loading and compiling `lib/Base/*.flx` to bytecode adds startup time (~50-100ms). Mitigated by caching compiled bytecode. GHCi has the same tradeoff and solves it with pre-compiled `.o` files.

- **Base library must be available on disk**: The VM needs `lib/Base/*.flx` files (or their cached bytecode) at runtime. If files are missing, standard library functions are unavailable. Mitigated by: embedding the source in the binary as `include_str!()` at compile time, or shipping pre-compiled bytecode.

- **Separate compilation complexity**: Implementing interface files (type scheme export) and cached compilation adds compiler complexity. This is the hardest part of the proposal.

- **Debugging**: Stack traces through Base library functions show `lib/Base/List.flx:3` instead of `<builtin:map>`. This is arguably better (users can read the source) but may be confusing initially.

- **Performance of interpreted Base functions**: VM-interpreted `map`/`filter` in Flux bytecode may be slower than the current Rust implementations (which use Rust iterators). Mitigated by: the VM is for development (not production), and the difference is small for typical use.

---

## Rationale and alternatives

### Why not keep both implementations?

Duplication is a maintenance burden and a source of semantic divergence bugs. When `map` behaves differently in Rust vs Flux, the bug is subtle and backend-specific. A single source eliminates this class of bugs entirely.

### Why not generate Flux from Rust (or vice versa)?

Code generation between languages is fragile and hard to maintain. The generated code is unreadable and not directly editable. Writing the standard library in the language it serves is the natural approach — it's what GHC, Lean, Koka, OCaml, and Zig all do.

### Why not embed Base functions in the compiler binary?

We could use `include_str!("lib/Base/List.flx")` to embed the source in the Rust binary, eliminating the disk dependency. This is a valid optimization (and should be done for release builds) but doesn't change the architecture — the source is still Flux, compiled through the standard pipeline.

### Why not implement everything as primops?

Adding 83 functions as `CorePrimOp` variants bloats the compiler with algorithmic code (quicksort, zip, flatten) that belongs in a library. Primops should be minimal — only operations that truly need compiler/hardware support.

---

## Prior art

### GHC (Haskell)

GHC's `libraries/base/` is Haskell source compiled by both GHCi (to bytecode) and GHC (to native code). Interface files (`.hi`) cache type information for separate compilation. The RTS provides only primitives (GC, I/O, scheduler). This architecture has been stable for 20+ years.

### Lean 4

Lean's standard library (`Init/`) is written in Lean. It compiles to C and links with a small C runtime. The compiler caches compiled `.olean` files for incremental compilation.

### Koka

Koka's standard library (`lib/std/`) is written in Koka. It compiles to C alongside user code. Perceus reference counting is applied uniformly to both library and user code.

### Rust

Rust's `core` and `std` libraries are written in Rust. They're pre-compiled and shipped as `.rlib` files. The compiler links them with user code. Only platform-specific primitives use `extern` (libc, OS APIs).

### OCaml

OCaml's standard library (`stdlib/`) is written in OCaml. Both the bytecode interpreter (`ocamlrun`) and the native compiler (`ocamlopt`) compile the same source. External C primitives provide I/O and OS interaction.

---

## Unresolved questions

- **Embedding vs. file-based**: Should the Base library source be embedded in the Flux binary via `include_str!()` for zero-dependency deployment, or loaded from `lib/Base/` on disk? Embedding is simpler for users; file-based is simpler for development. Both can coexist (embedded as fallback when files are missing).

- **Cache format**: Should cached compilation use Flux's existing `.fxc` bytecode format for the VM, or a new format? Using `.fxc` reuses existing infrastructure; a new format could include type scheme information for separate compilation.

- **Polymorphic base function type inference**: The current HM inference engine hangs when processing all Base library functions in one pass. The fix likely requires separate compilation with exported type schemes. How much of this infrastructure already exists?

- **Argument order**: Flux uses `map(collection, fn)` (collection-first) while Haskell uses `map fn collection` (function-first). The Flux convention is established and should be preserved in `lib/Base/`, but it affects currying patterns.

- **`len` polymorphism**: `len` works on lists, strings, and arrays. Should it be one polymorphic function in `lib/Base/` that dispatches at runtime, or three separate functions (`list_len`, `string_len`, `array_len`)? Runtime dispatch matches current behavior; separate functions are more explicit and optimize better.

---

## Future possibilities

- **`flux init`**: A project scaffolding command that sets up `lib/Base/` as a dependency, similar to `cargo init` setting up `Cargo.toml`.

- **Package manager**: `lib/Base/` is the first "package." A future package manager extends this to external libraries: `lib/Base/`, `lib/Http/`, `lib/Json/`, etc.

- **User-overridable stdlib**: Users can shadow Base functions by defining their own `map`, `filter`, etc. This is already natural with the prelude approach.

- **Base library documentation**: Since `lib/Base/*.flx` is readable Flux source, documentation can be generated directly from the source with doc comments.

- **Benchmarking Base implementations**: With both VM and native backends compiling the same source, performance comparisons become meaningful — any difference is due to the backend, not the implementation.

- **Property-based testing**: The Base library functions have well-defined algebraic properties (`map id == id`, `filter p . filter q == filter (\x -> p x && q x)`). These can be tested with QuickCheck-style property testing once a testing framework exists.
