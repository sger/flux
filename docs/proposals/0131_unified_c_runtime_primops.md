- Feature Name: Unified C Runtime Primops — Single Implementation, Two Backends
- Start Date: 2026-03-27
- Proposal PR:
- Flux Issue:
- Depends on: None (incremental, can be done per-primop)

## Summary

Replace the dual primop implementation pattern (Rust for VM, C for native backend) with a single C runtime implementation called by both backends. Today, every primop that touches values is implemented twice — once in Rust (`src/primop/mod.rs`, `src/runtime/hamt.rs`) and once in C (`runtime/c/flux_rt.c`, `runtime/c/hamt.c`). This causes parity bugs that are difficult to test exhaustively. After this proposal, both backends call the same C functions, eliminating this entire class of bugs.

This follows GHC's proven model: the interpreter (GHCi) does not reimplement primops — it calls the same RTS functions as the compiled backends via libffi/CCALL.

## Motivation

### The dual implementation problem

Every primop that operates on Flux values has two implementations:

| Primop | VM (Rust) | Native (C) | Known divergences |
|--------|-----------|------------|-------------------|
| `push` | `Vec::push` in primop/mod.rs | `flux_array_push` in flux_rt.c | None known |
| `sort` | `Vec::sort` (timsort, stable) | C `qsort` (unstable) | **Different element order for equal keys** |
| `hamt_put` | `hamt.rs` (Rust, 280 lines) | `hamt.c` (C, 350 lines) | **Different iteration order** |
| `hamt_keys` | Rust HashMap iteration | C HAMT traversal | **Non-deterministic order difference** |
| `to_string` | Rust `Display` for `Value` | `flux_to_string` in C | **Cons list quoting, HAMT display** |
| `cmp_eq` | Rust `PartialEq` for `Value` | `flux_rt_eq` in C | Edge cases in nested comparison |
| `type_of` | Rust `type_name()` | `flux_type_of` in C | **HAMT vs ADT detection heuristic** |
| `cons` | `ConsCell::cons` in Rust | `flux_make_cons` in C | None known |
| `slice` | `Vec` slice in Rust | `flux_array_slice` in C | None known |
| `concat` | `Vec::extend` in Rust | `flux_array_concat` in C | None known |
| `len` | Rust match on Value | `flux_len` in C | **Cons list walk removed in VM, exists in C** |

We've already encountered parity bugs in this conversation:
- Cons list rendering differed between VM and native
- HAMT display showed `Some()` in native but correct output in VM
- `type_of` HAMT detection used a heuristic in C that collided with ADT tags
- `MemberAccess` resolution was non-deterministic in the LLVM backend

### How GHC solves this

GHC has **zero duplicated primop implementations**:

- **58 out-of-line primops** (allocation, I/O, concurrency) are written ONCE in `rts/PrimOps.cmm`. The NCG, LLVM, and interpreter ALL call these same functions.
- **Inline primops** (arithmetic, memory loads) are generated as Cmm by a single code generator (`StgToCmm/Prim.hs`). All compiled backends consume the same Cmm.
- **The interpreter** uses libffi/CCALL to call the compiled Cmm stubs. It does NOT have its own Haskell implementations of primops.

```
GHC:
  NCG backend     → generates call to stg_newArrayzh ─┐
  LLVM backend    → generates call to stg_newArrayzh ─┼→ ONE implementation in rts/PrimOps.cmm
  Interpreter     → CCALL/libffi to stg_newArrayzh  ─┘
```

### What this proposal achieves

```
Flux after this proposal:
  VM (bytecode)   → FFI call to flux_array_push ─┐
  Native (LLVM)   → LLVM call to flux_array_push ─┼→ ONE implementation in runtime/c/flux_rt.c
                                                   │
                                        Zero parity bugs for primops
```

---

## Reference-level explanation

### Architecture change

**Before:**
```
VM dispatch loop:
  OpPrimOp(Push, 2) → match args {
      Value::Array(arr) => {
          let mut v = (*arr).clone();  // Rust Vec clone
          v.push(elem);                // Rust Vec::push
          Ok(Value::Array(Rc::new(v))) // Rust Rc allocation
      }
  }

Native LLVM codegen:
  CorePrimOp::ArrayPush → emit call @flux_array_push(arr_i64, elem_i64)
    → flux_rt.c: flux_array_push(uint64_t arr, uint64_t elem) { ... C implementation ... }
```

**After:**
```
VM dispatch loop:
  OpPrimOp(Push, 2) → {
      let result = unsafe { flux_array_push(args[0].to_nan_boxed(), args[1].to_nan_boxed()) };
      Ok(Value::from_nan_boxed(result))
  }

Native LLVM codegen:
  CorePrimOp::ArrayPush → emit call @flux_array_push(arr_i64, elem_i64)
    → SAME flux_rt.c function
```

### Phase 1 — Link C runtime into the VM binary

Currently the C runtime (`runtime/c/flux_rt.c`, `runtime/c/hamt.c`) is only compiled for the native backend. It needs to be linked into the main `flux` binary so the VM can call it.

**Build system change** (`build.rs` or `Cargo.toml`):

```rust
// build.rs
fn main() {
    // Compile C runtime for VM use
    cc::Build::new()
        .file("runtime/c/flux_rt.c")
        .file("runtime/c/hamt.c")
        .include("runtime/c")
        .opt_level(2)
        .compile("flux_rt");
}
```

**Cargo.toml:**
```toml
[build-dependencies]
cc = "1.0"
```

This compiles `flux_rt.c` and `hamt.c` as a static library and links it into the Flux binary. The VM can then call C functions directly via Rust's `extern "C"` FFI — no libffi needed.

### Phase 2 — NaN-box bridge layer

The VM uses `Value` enum (16 bytes, Rust). The C runtime uses NaN-boxed `uint64_t` (8 bytes). A bridge converts between them.

```rust
// src/runtime/nan_bridge.rs (new file)

/// Convert a Rust Value to a NaN-boxed i64 for C runtime calls.
pub fn value_to_nan(value: &Value) -> i64 {
    match value {
        Value::Integer(n) => flux_tag_int(*n),
        Value::Float(f) => f.to_bits() as i64,
        Value::Boolean(true) => NAN_TRUE,
        Value::Boolean(false) => NAN_FALSE,
        Value::None => NAN_NONE,
        Value::String(s) => flux_tag_string(s),
        Value::Array(arr) => flux_tag_array(arr),
        Value::Cons(cell) => flux_tag_cons(cell),
        Value::HashMap(node) => flux_tag_hamt(node),
        Value::Adt(adt) => flux_tag_adt(adt),
        // ... other variants
    }
}

/// Convert a NaN-boxed i64 from C runtime back to a Rust Value.
pub fn nan_to_value(nan: i64) -> Value {
    let tag = nan_get_tag(nan);
    match tag {
        TAG_INT => Value::Integer(flux_untag_int(nan)),
        TAG_FLOAT => Value::Float(f64::from_bits(nan as u64)),
        TAG_STRING => Value::String(flux_untag_string(nan)),
        TAG_ARRAY => Value::Array(flux_untag_array(nan)),
        TAG_CONS => Value::Cons(flux_untag_cons(nan)),
        // ... other tags
    }
}
```

**Alternative (simpler):** If the VM switches to NaN-boxed values internally (the `Value` enum already has a NaN-boxing path), no bridge is needed — the VM passes `i64` directly to C functions.

### Phase 3 — Migrate primops one category at a time

Migrate primops from Rust to C FFI calls incrementally. Each category can be a separate PR.

**Order of migration** (easiest/safest first):

#### 3a. String operations (~10 primops)

```rust
// Before (Rust, primop/mod.rs):
PrimOp::Upper => {
    let Value::String(s) = &args[0] else { ... };
    Ok(Value::String(Rc::new(s.to_uppercase())))
}

// After (C FFI call):
extern "C" { fn flux_upper(s: i64) -> i64; }
PrimOp::Upper => {
    let result = unsafe { flux_upper(value_to_nan(&args[0])) };
    Ok(nan_to_value(result))
}
```

Primops: `Upper`, `Lower`, `Trim`, `Split`, `Replace`, `Substring`, `StartsWith`, `EndsWith`, `StrContains`, `Chars`, `Join`

#### 3b. Array operations (~8 primops)

Primops: `ArrayPush`, `ArrayConcat`, `ArraySlice`, `ArrayGet`, `ArraySet`, `ArrayLen`, `ArraySort`, `MakeArray`

#### 3c. HAMT operations (~6 primops)

This is the highest-value migration — `hamt.rs` (Rust, 280 lines) and `hamt.c` (C, 350 lines) are completely separate implementations of the same data structure.

Primops: `HamtGet`, `HamtPut`, `HamtDelete`, `HamtKeys`, `HamtValues`, `HamtMerge`

After migration, delete `src/runtime/hamt.rs` entirely.

#### 3d. Comparison and conversion (~8 primops)

Primops: `CmpEq`, `CmpNe`, `ToString`, `TypeOf`, `ParseInt`, `ParseFloat`, `ToList`, `ToArray`

#### 3e. I/O operations (~5 primops)

Primops: `Print`, `Println`, `ReadLine`, `ReadLines`, `Panic`

#### 3f. Arithmetic operations (~12 primops)

These are the simplest — just integer/float operations. Can stay in Rust for performance (no bridge overhead) or move to C for consistency. Recommend: **keep in Rust** — arithmetic on `i64`/`f64` is identical in both languages, no parity risk.

### Phase 4 — Delete Rust primop implementations

After all primops are migrated, delete:
- `src/runtime/hamt.rs` (~280 lines) — replaced by `runtime/c/hamt.c`
- Most of `src/primop/mod.rs` (~600 lines) — replaced by FFI calls
- Rust `cons_list_len`, `format_value`, etc. — replaced by C equivalents

### Phase 5 — Shared value representation (optional, future)

If the VM adopts NaN-boxed `i64` values internally (instead of the `Value` enum), the bridge layer disappears entirely. The VM would pass raw `i64` values to C functions with zero conversion overhead. This aligns with the LLVM backend which already uses NaN-boxed values.

---

## Primop migration table

| Category | Primops | Rust lines deleted | Risk | Priority |
|----------|---------|-------------------|------|----------|
| HAMT | get, put, delete, keys, values, merge, has_key | ~280 (entire hamt.rs) | Medium (complex data structure) | **High** (most parity bugs) |
| Comparison | cmp_eq, cmp_ne, to_string, type_of | ~150 | Medium (edge cases in nested values) | **High** (known bugs) |
| Array | push, concat, slice, get, set, len, sort | ~100 | Low (simple operations) | Medium |
| String | upper, lower, trim, split, replace, etc. | ~120 | Low | Medium |
| I/O | print, println, read_line, read_lines | ~60 | Low | Low |
| Arithmetic | add, sub, mul, div, mod, neg, etc. | ~80 | **Keep in Rust** — no parity risk | Skip |

**Total Rust code deleted:** ~710 lines
**Total C code added:** ~0 lines (already exists in `flux_rt.c` and `hamt.c`)

---

## FFI overhead analysis

**Concern:** Calling C from Rust via FFI has overhead (~2-5ns per call). For hot primops like `add` and array indexing, this could slow down the VM.

**Analysis:**

| Primop | Call frequency | FFI overhead per call | Impact |
|--------|---------------|----------------------|--------|
| `IntAdd` | Very hot (every `+`) | ~3ns | **Significant** — keep in Rust |
| `ArrayGet` | Hot (every `arr[i]`) | ~3ns | Measurable — benchmark before migrating |
| `HamtPut` | Medium (map operations) | ~3ns | Negligible vs 200ns+ HAMT traversal |
| `ToString` | Cold (display only) | ~3ns | Negligible |
| `Print` | Cold (I/O) | ~3ns | Negligible vs I/O cost |

**Recommendation:** Keep arithmetic and comparison operators in Rust (fast path). Migrate collection operations (HAMT, array mutating ops), string operations, I/O, and conversion operations to C.

---

## Drawbacks

- **FFI bridge overhead** for hot primops. Mitigated by: keeping arithmetic in Rust, benchmarking before migrating array indexing.

- **C runtime becomes a hard dependency** for the VM binary. Currently the VM is pure Rust. After this change, building Flux requires a C compiler. Mitigated by: `cc` crate handles this automatically, and the C runtime is already required for the native backend.

- **Debugging is harder** — stack traces cross the Rust/C boundary. Mitigated by: the C runtime is small (~2K lines) and well-tested.

- **NaN-box bridge adds complexity** if the VM keeps using `Value` enum internally. Mitigated by: the bridge is a thin layer (~100 lines), or eliminated entirely if the VM adopts NaN-boxed values.

## Rationale and alternatives

### Why not keep dual implementations with better testing?

Testing catches specific bugs but can't prove absence of bugs. Every new primop would need test coverage for both backends. Every edge case (empty arrays, deeply nested ADTs, hash collisions) needs explicit testing. With a single implementation, these bugs are impossible by construction.

### Why not rewrite the C runtime in Rust?

This would also achieve single implementation but in the opposite direction — delete C, keep Rust. The problem: the LLVM native backend links against C functions. Rewriting in Rust would require either (a) compiling Rust to a C-compatible static library (complex cross-compilation) or (b) rewriting the LLVM codegen to call Rust functions (invasive change). Keeping C is simpler because LLVM naturally calls C functions.

### Why not use libffi like GHC's interpreter?

GHC's interpreter uses libffi for dynamic C calls because it needs to call arbitrary functions discovered at link time. Flux's primops are statically known — we can use Rust's `extern "C"` FFI which is zero-overhead (just a regular function call with C ABI).

## Prior art

- **GHC**: Interpreter calls `stg_newArrayzh` etc. from `rts/PrimOps.cmm` via CCALL. Zero duplicated primop implementations across 5 backends.
- **Lua/LuaJIT**: C API functions (`lua_pushstring`, `lua_gettable`) are the single implementation. Both the interpreter and JIT call them.
- **CPython**: Built-in types (`list.append`, `dict.__getitem__`) are implemented once in C. The bytecode interpreter calls these C functions directly.
- **Erlang BEAM**: Built-in functions (BIFs) are implemented once in C. The interpreter and JIT both call them.

## Future possibilities

- **Shared LIR (Proposal 0132)**: A shared low-level IR that both backends consume, eliminating control flow divergence in addition to primop divergence. This proposal (0131) is a prerequisite — unified primops make the LIR transition easier because primops are already single-implementation C calls.
- **NaN-boxed VM values**: If the VM adopts NaN-boxed `i64` internally, the bridge layer disappears and the VM becomes a direct consumer of C runtime functions with zero conversion overhead.
- **WebAssembly backend**: A future Wasm backend would also call the same C runtime (compiled to Wasm via wasi-sdk), getting primop parity for free.
