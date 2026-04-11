- Feature Name: Unified PrimOp — VM uses CorePrimOp
- Start Date: 2026-03-27
- Status: Implemented
- Proposal PR:
- Flux Issue:
- Depends on: Proposal 0131 (Aether RC in C runtime)

## Summary

Replace the VM's `PrimOp` enum (81 variants, Rust dispatch) with `CorePrimOp` so both the VM and LLVM native backend use the same primop type and the same C runtime functions. Delete `src/primop/mod.rs` (~1,400 lines) and all translation layers between the two enums.

## Motivation

### Current state: two enums

```
VM:     PrimOp (81 variants) → execute_primop() → Rust match arms
Native: CorePrimOp (51 variants) → builtins.rs → C function calls
```

Same logical operations, two separate types, four translation layers:

```
CorePrimOp → promoted_primop_name() → string name
           → resolve_primop_call() → PrimOp
           → OpPrimOp(PrimOp id) → execute_primop()
```

Adding a new primop requires updating 4 separate tables. Known bugs from the split: `PrimOp::MapHas` vs `CorePrimOp::HamtContains` (different names, same operation), missing variants in one enum but not the other.

### What GHC does

One `PrimOp` type in `GHC.Builtin.PrimOps`. Interpreter and all compiled backends consume it directly. No translation.

### After this proposal

```
VM:     CorePrimOp → C function call
Native: CorePrimOp → C function call
```

One enum. One dispatch. Both backends.

---

## Design

### Step 1: Add missing variants to CorePrimOp

CorePrimOp is missing ~10 variants that only exist in PrimOp:

| Missing in CorePrimOp | PrimOp equivalent |
|----------------------|-------------------|
| `Abs` | `PrimOp::Abs` |
| `Min` | `PrimOp::Min` |
| `Max` | `PrimOp::Max` |
| `Time` | `PrimOp::Time` |
| `ParseInts` | `PrimOp::ParseInts` |
| `SplitInts` | `PrimOp::SplitInts` |
| `ReadLines` | `PrimOp::ReadLines` |
| Typed comparisons (`ICmpEq`..`FCmpGe`) | `PrimOp::ICmpEq`..`PrimOp::FCmpGe` |

Add these to `CorePrimOp` in `src/core/mod.rs`. Add corresponding C implementations in `runtime/c/flux_rt.c` if missing.

### Step 2: Move data-carrying variants out of CorePrimOp

Two variants carry data and prevent `#[repr(u8)]`:

- `MemberAccess(Identifier)` — resolved at compile time, never reaches bytecode. Move to a `CoreExpr` variant.
- `TupleField(usize)` — becomes an opcode operand (like `OpAdtField` already uses a u8 operand).

After this, CorePrimOp is flat and can be `#[repr(u8)]` for bytecode encoding.

### Step 3: Add `#[repr(u8)]` discriminants to CorePrimOp

Assign stable discriminants for bytecode cache compatibility:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CorePrimOp {
    IAdd = 0,
    ISub = 1,
    IMul = 2,
    // ...
}
```

Bump the `.fxc` cache version to invalidate old caches.

### Step 4: Change OpPrimOp to carry CorePrimOp id

The bytecode instruction `OpPrimOp(id, arity)` currently uses `PrimOp` discriminants. Change to use `CorePrimOp` discriminants.

**File: `src/bytecode/compiler/cfg_bytecode.rs`**

```rust
// Before: resolve string name → PrimOp → emit PrimOp id
let primop = resolve_primop_call(name, arity);
self.emit(OpCode::OpPrimOp, &[primop.id(), arity]);

// After: emit CorePrimOp id directly
let primop = CorePrimOp::from_name(name, arity);
self.emit(OpCode::OpPrimOp, &[primop as u8 as usize, arity]);
```

### Step 5: VM dispatch on CorePrimOp → C functions

**File: `src/bytecode/vm/primop.rs`**

```rust
// Before:
let op = PrimOp::from_id(primop_id);
let result = execute_primop(self, op, args)?;  // 600 lines of Rust match

// After:
let op = CorePrimOp::from_id(primop_id);
let result = execute_core_primop(op, &raw_args)?;  // C FFI calls

fn execute_core_primop(op: CorePrimOp, args: &[i64]) -> Result<i64, String> {
    match op {
        CorePrimOp::Println => { unsafe { flux_println(args[0]); } Ok(flux_make_none()) }
        CorePrimOp::Upper   => Ok(unsafe { flux_upper(args[0]) }),
        CorePrimOp::HamtGet => Ok(unsafe { flux_hamt_get(args[0], args[1]) }),
        CorePrimOp::IAdd    => Ok(flux_tag_int(flux_untag_int(args[0]) + flux_untag_int(args[1]))),
        // ...
    }
}
```

### Step 6: Delete PrimOp and translation layers

Delete entirely:

| File | Lines | What |
|------|-------|------|
| `src/primop/mod.rs` | ~1,400 | `PrimOp` enum, `execute_primop()`, `resolve_primop_call()`, `PRIMOP_CALL_MAPPINGS` |
| `src/core/to_ir/primop.rs` | ~280 | `promoted_primop_name()` (reverse translation) |
| `src/runtime/hamt.rs` | ~280 | Rust HAMT (C HAMT is the single implementation) |

**Total deleted:** ~1,960 lines

Add:

| File | Lines | What |
|------|-------|------|
| `src/core/mod.rs` | ~20 | `CorePrimOp::from_name()`, `CorePrimOp::from_id()` |
| `src/core_to_llvm/codegen/builtins.rs` | ~10 | `CorePrimOp::c_function_name()` (re-keyed by enum) |
| `src/bytecode/vm/primop.rs` | ~100 | `execute_core_primop()` dispatch to C |

**Total added:** ~130 lines

**Net reduction:** ~1,830 lines

---

## Files modified

| File | Change |
|------|--------|
| `src/core/mod.rs` | Add missing variants, remove MemberAccess/TupleField, add `#[repr(u8)]` |
| `src/bytecode/vm/primop.rs` | Dispatch CorePrimOp → C FFI calls |
| `src/bytecode/compiler/cfg_bytecode.rs` | Emit CorePrimOp id in OpPrimOp |
| `src/bytecode/compiler/expression.rs` | Use CorePrimOp::from_name() |
| `src/core_to_llvm/codegen/builtins.rs` | Re-key by CorePrimOp enum |
| `src/primop/mod.rs` | **Delete** |
| `src/core/to_ir/primop.rs` | Delete `promoted_primop_name()` |
| `src/runtime/hamt.rs` | **Delete** |
| `src/lib.rs` | Remove `pub mod primop` |

---

## Architecture after

```
Source → Core IR (CorePrimOp)
              │
    ┌─────────┴─────────┐
    │                    │
CFG → Bytecode        LLVM IR
    │                    │
VM dispatch           Native binary
    │                    │
    └─────────┬─────────┘
              │
      C Runtime (single)
      flux_upper, flux_hamt_get, ...
```

Both backends: same enum, same C functions, same values.

---

## Verification

1. `cargo build` — compiles
2. `cargo build --features core_to_llvm` — LLVM backend compiles
3. `cargo test --all` — all tests pass
4. `cargo clippy --all-targets --all-features -- -D warnings` — clean
5. Parity test:
   ```bash
   cargo run -- examples/basics/fibonacci.flx
   cargo run --features core_to_llvm -- examples/basics/fibonacci.flx --native --no-cache
   # Both must produce identical output
   ```

## Impact on Value enum

Deleting `PrimOp` and `execute_primop()` removes the **biggest consumer** of the `Value` enum in the runtime. Today, `execute_primop()` (~600 lines) takes `Vec<Value>` and returns `Result<Value, String>` — every primop call constructs and destructs `Value` variants.

After this proposal, primop dispatch uses raw `i64`:

```rust
// Before (PrimOp + Value):
PrimOp::Upper => {
    let Value::String(s) = &args[0] else { ... };   // Value in
    Ok(Value::String(s.to_uppercase().into()))        // Value out
}

// After (CorePrimOp + i64):
CorePrimOp::Upper => Ok(unsafe { flux_upper(args[0]) })  // i64 in, i64 out
```

`CorePrimOp` itself is just an enum of operation tags — it never touches `Value`.

**Value usage after this proposal:**

| Still uses Value | Why | Hot/Cold |
|-----------------|-----|----------|
| Bytecode compiler | Builds constants (`Value::String`, `Value::Function`) | Cold (compile time) |
| Closures | `Rc<CompiledFunction>` is Rust-only | Cold (creation only) |
| Effect handlers | Continuations capture Rust frame stacks | Cold (rare) |
| Error messages | `format_value(&Value)` for display | Cold (error path) |
| VM stack (non-CSlot) | Default `NanBox` path wraps `Rc<Value>` | Hot but migrating (CSlot replaces) |

`Value` becomes a **compiler and cold-path type**. The VM hot loop operates on `i64`.

## Drawbacks

- **Bytecode cache invalidation** — new discriminants break `.fxc` files. Mitigated by version bump.
- **MemberAccess refactor** — moving to CoreExpr touches Core IR and all passes. Most complex single change.
- **C compiler required** — VM build now needs C compiler for linked runtime. Already required for native backend.

## Prior art

- **GHC**: Single `PrimOp` in `GHC.Builtin.PrimOps`, consumed by interpreter and all backends
- **Koka**: Single `Extern` declarations consumed by all backends (C, JS, C#)
- **OCaml**: Single `Primitive` type consumed by bytecode interpreter and native compiler
