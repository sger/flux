- Feature Name: Typed LLVM Code Generation — Unboxed Values via HM Type Information
- Start Date: 2026-03-24
- Status: Implemented
- Proposal PR:
- Flux Issue:

## Summary

Carry Hindley-Milner type information through the Core IR into the `core_to_llvm` backend, enabling **type-directed code generation** that eliminates NaN-boxing overhead for values with statically known types. When the compiler knows a value is `Int`, it emits raw `i64` arithmetic — no tagging, no untagging, no branch on type tag. This follows GHC's three-tier representation system (`PrimRep` → `CmmType` → `LlvmType`) adapted for Flux's NaN-box architecture.

## Motivation

### The NaN-boxing tax

Today, every Flux value in `core_to_llvm` is an `i64` NaN-box. Even when the compiler has proven through HM inference that a value is `Int`, the generated code still tags and untags:

```llvm
; Flux: let x = 1 + 2
; Current codegen (untyped):
%a = call fastcc i64 @flux_untag_int(i64 9222246136947933185)  ; untag 1
%b = call fastcc i64 @flux_untag_int(i64 9222246136947933186)  ; untag 2
%sum = add i64 %a, %b                                          ; actual work
%result = call fastcc i64 @flux_tag_int(i64 %sum)              ; retag
```

With type information, the same code becomes:

```llvm
; Typed codegen (knows both operands are Int):
%result = add i64 1, 2    ; that's it
```

Four instructions → one. The tag/untag functions are `alwaysinline` so LLVM can partially optimize this, but it cannot eliminate the masking and shifting entirely because it doesn't know the input is always a tagged integer.

### Where the type information exists

Flux already has **complete type information** from Hindley-Milner inference:

```
Source:     fn add(x, y) { x + y }
HM output:  add : Int -> Int -> Int
Core IR:    λx. λy. IAdd(x, y)     ← types are KNOWN but not carried
LLVM IR:    i64 @add(i64 %x, i64 %y)  ← everything is i64, types are lost
```

The types are computed in `ast/type_infer/` but discarded before Core lowering. The Core IR has no type annotations on binders or expressions. This is the gap.

### What GHC does

GHC maintains type/representation information through the entire pipeline via three levels of abstraction:

```
Core Type (e.g., Int, Int#, [Int], Maybe Int)
    ↓  typePrimRep
PrimRep (e.g., IntRep, BoxedRep Lifted, DoubleRep)
    ↓  STG → Cmm lowering
CmmType (e.g., BitsCat W64, GcPtrCat W64, FloatCat W64)
    ↓  cmmToLlvmType
LlvmType (e.g., i64, ptr, double)
```

**PrimRep** answers: "What kind of register does this value live in?"
- `IntRep` → machine word, no GC tracing needed
- `BoxedRep Lifted` → heap pointer, GC must trace it
- `DoubleRep` → float register

**CmmType** answers: "What's the category and width?"
- `BitsCat W64` → raw 64-bit integer
- `GcPtrCat W64` → traced heap pointer
- `FloatCat W64` → IEEE 754 double

The LLVM backend then pattern-matches on `CmmType` to choose representations:
- `GcPtrCat` → may need `inttoptr` casts, GC root registration
- `BitsCat` → direct arithmetic, no indirection
- `FloatCat` → float registers, `fadd`/`fmul` instructions

### Performance impact

For numeric-heavy code (AoC puzzles, benchmarks, tight loops):

| Operation | Current (NaN-boxed) | Typed (unboxed) | Speedup |
|-----------|-------------------|-----------------|---------|
| `a + b` (Int) | untag + add + retag (4 ops) | `add i64` (1 op) | ~4x |
| `a * b + c` (Int) | 3x untag + 2 ops + 2x retag (9 ops) | `mul` + `add` (2 ops) | ~4x |
| `x > 0` (Int) | untag + cmp + tag bool (5 ops) | `icmp sgt` (1 op) | ~5x |
| `f(x)` where f: Int→Int | NaN-box arg + call + NaN-box result | raw i64 call | ~2x |
| Loop counter | tag/untag every iteration | raw register | ~4x |

For allocation-heavy code (ADTs, closures, lists), the improvement is smaller because heap-allocated values must remain boxed regardless. But even there, fields with known types can be stored unboxed.

---

## Guide-level explanation

### For Flux users

No syntax changes. The compiler automatically uses unboxed representations when types are statically known:

```flux
fn fibonacci(n) {
    if n <= 1 { n }
    else { fibonacci(n - 1) + fibonacci(n - 2) }
}
```

HM infers `fibonacci : Int -> Int`. The `core_to_llvm` backend emits:

```llvm
define fastcc i64 @fibonacci(i64 %n) {
  ; No NaN-boxing — n is a raw i64, comparisons and arithmetic are native
  %cmp = icmp sle i64 %n, 1
  br i1 %cmp, label %base, label %recurse
base:
  ret i64 %n
recurse:
  %n1 = sub i64 %n, 1
  %fib1 = call fastcc i64 @fibonacci(i64 %n1)
  %n2 = sub i64 %n, 2
  %fib2 = call fastcc i64 @fibonacci(i64 %n2)
  %result = add i64 %fib1, %fib2
  ret i64 %result
}
```

Zero tag/untag overhead. LLVM can now apply tail-call optimization, loop unrolling, and register allocation without NaN-box interference.

### For compiler contributors

A new `FluxRep` enum (analogous to GHC's `PrimRep`) is attached to Core IR binders and expressions. The `core_to_llvm` backend reads it to choose between boxed and unboxed code generation.

---

## Reference-level explanation

### Flux representation types

Analogous to GHC's `PrimRep`, Flux introduces `FluxRep`:

```rust
/// Runtime representation of a Flux value.
/// Determined by HM type inference and carried through Core IR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FluxRep {
    /// Raw signed 64-bit integer. No NaN-boxing.
    /// Fits in a general-purpose register.
    IntRep,

    /// Raw IEEE 754 double. No NaN-boxing.
    /// Fits in a floating-point register.
    FloatRep,

    /// Raw boolean (i1). No NaN-boxing.
    BoolRep,

    /// Heap-allocated boxed value (NaN-boxed pointer).
    /// Used for: String, Array, Closure, ADT, Cons, HashMap.
    /// Must be traced by GC / reference counted by Aether.
    BoxedRep,

    /// NaN-boxed value with unknown or polymorphic type.
    /// Fallback when the type is not statically known (e.g., `Any`, type variables).
    /// Uses the current NaN-box encoding with runtime tag dispatch.
    TaggedRep,

    /// Unit / void. No runtime representation.
    VoidRep,
}
```

The mapping from Flux types to `FluxRep`:

| Flux Type | FluxRep | LLVM Type | Register |
|-----------|---------|-----------|----------|
| `Int` | `IntRep` | `i64` | GPR |
| `Float` | `FloatRep` | `double` | FPR |
| `Bool` | `BoolRep` | `i1` | GPR |
| `String` | `BoxedRep` | `i64` (NaN-boxed ptr) | GPR |
| `[a]` (list) | `BoxedRep` | `i64` (NaN-boxed ptr) | GPR |
| `{...}` (ADT) | `BoxedRep` | `i64` (NaN-boxed ptr) | GPR |
| `a -> b` (closure) | `BoxedRep` | `i64` (NaN-boxed ptr) | GPR |
| `HashMap k v` | `BoxedRep` | `i64` (NaN-boxed ptr) | GPR |
| `()` (unit) | `VoidRep` | void | none |
| `a` (type variable) | `TaggedRep` | `i64` (NaN-boxed) | GPR |
| `Option Int` | `BoxedRep` | `i64` (NaN-boxed ptr) | GPR |

### Carrying types through Core IR

Core IR binders gain a `rep` field:

```rust
pub struct CoreBinder {
    pub id: CoreBinderId,
    pub name: Identifier,
    pub rep: FluxRep,          // NEW: runtime representation
}
```

Core expressions gain type annotations where needed:

```rust
pub enum CoreExpr {
    Var { var: CoreVarRef, rep: FluxRep, .. },
    Lit(CoreLit, Span),  // rep is implied by literal type
    Lam { params: Vec<CoreBinder>, body: Box<CoreExpr>, ret_rep: FluxRep, .. },
    App { func: Box<CoreExpr>, args: Vec<CoreExpr>, result_rep: FluxRep, .. },
    PrimOp { op: CorePrimOp, args: Vec<CoreExpr>, result_rep: FluxRep, .. },
    // ... etc
}
```

### HM type → FluxRep conversion

A new function converts inferred types to representations:

```rust
fn type_to_rep(ty: &InferType) -> FluxRep {
    match ty {
        InferType::Int => FluxRep::IntRep,
        InferType::Float => FluxRep::FloatRep,
        InferType::Bool => FluxRep::BoolRep,
        InferType::Unit => FluxRep::VoidRep,
        InferType::String => FluxRep::BoxedRep,
        InferType::List(_) => FluxRep::BoxedRep,
        InferType::Function(_, _) => FluxRep::BoxedRep,
        InferType::Adt(_) => FluxRep::BoxedRep,
        InferType::HashMap(_, _) => FluxRep::BoxedRep,
        InferType::Var(_) => FluxRep::TaggedRep,  // polymorphic — must NaN-box
        _ => FluxRep::TaggedRep,
    }
}
```

This runs during AST → Core lowering, after HM inference has resolved all types.

### Code generation changes

The `core_to_llvm` backend's `FunctionLowering` uses `FluxRep` to decide how to emit code:

#### Arithmetic

```rust
fn lower_primop_add(&mut self, args: &[CoreExpr], result_rep: FluxRep) {
    let lhs = self.lower_expr(&args[0])?;
    let rhs = self.lower_expr(&args[1])?;
    match result_rep {
        FluxRep::IntRep => {
            // Both args are IntRep (guaranteed by type checker).
            // Emit raw add — no tag/untag.
            emit(Binary { op: Add, ty: i64, lhs, rhs })
        }
        FluxRep::FloatRep => {
            emit(Binary { op: FAdd, ty: double, lhs, rhs })
        }
        FluxRep::TaggedRep => {
            // Polymorphic — use existing NaN-box path with runtime dispatch.
            self.lower_helper_call("flux_iadd", args)
        }
    }
}
```

#### Function calls

```rust
fn lower_direct_call(&mut self, callee: GlobalId, args: Vec<LlvmOperand>, arg_reps: &[FluxRep]) {
    // Emit call with native types matching FluxRep.
    let llvm_args: Vec<(LlvmType, LlvmOperand)> = args.into_iter()
        .zip(arg_reps)
        .map(|(val, rep)| (rep_to_llvm_type(*rep), val))
        .collect();
    emit(Call { callee, args: llvm_args, ret_ty: rep_to_llvm_type(ret_rep) })
}

fn rep_to_llvm_type(rep: FluxRep) -> LlvmType {
    match rep {
        FluxRep::IntRep => LlvmType::i64(),
        FluxRep::FloatRep => LlvmType::Double,
        FluxRep::BoolRep => LlvmType::i1(),
        FluxRep::BoxedRep | FluxRep::TaggedRep => LlvmType::i64(),  // NaN-boxed
        FluxRep::VoidRep => LlvmType::Void,
    }
}
```

#### Boxing at boundaries

When an unboxed value must become boxed (stored in an ADT field, returned from a polymorphic function, captured by a closure), the compiler inserts a **box** operation:

```rust
fn box_value(&mut self, val: LlvmOperand, from_rep: FluxRep) -> LlvmOperand {
    match from_rep {
        FluxRep::IntRep => {
            // Tag as NaN-boxed integer
            emit(Call { callee: "flux_tag_int", args: [val] })
        }
        FluxRep::FloatRep => {
            // Float is stored as raw bits — already a valid NaN-box
            emit(Cast { op: Bitcast, from: double, to: i64, val })
        }
        FluxRep::BoolRep => {
            // Tag as NaN-boxed boolean
            emit(Call { callee: "flux_tag_bool", args: [val] })
        }
        FluxRep::BoxedRep | FluxRep::TaggedRep => val, // already boxed
        FluxRep::VoidRep => const_i64(tagged_none_bits()),
    }
}
```

And the reverse **unbox** when a boxed value flows into a typed context:

```rust
fn unbox_value(&mut self, val: LlvmOperand, to_rep: FluxRep) -> LlvmOperand {
    match to_rep {
        FluxRep::IntRep => emit(Call { callee: "flux_untag_int", args: [val] }),
        FluxRep::FloatRep => emit(Cast { op: Bitcast, from: i64, to: double, val }),
        FluxRep::BoolRep => {
            // Extract payload bit
            emit(Binary { op: And, val, const(PAYLOAD_MASK) });
            emit(Icmp { op: Ne, lhs: result, rhs: 0 })
        }
        _ => val,
    }
}
```

### Boxing discipline: when to box/unbox

The rules follow GHC's "let-no-escape" principle:

| Context | Rep | Boxing needed? |
|---------|-----|---------------|
| Local `let x = 1 + 2` where x: Int | IntRep | No — stays unboxed |
| Function param where f: Int→Int | IntRep | No — passed as raw i64 |
| Function return where f: Int→Int | IntRep | No — returned as raw i64 |
| Stored in ADT field: `Some(x)` | BoxedRep | Yes — must box x |
| Captured by closure | BoxedRep | Yes — must box captured value |
| Passed to polymorphic function | TaggedRep | Yes — must box to NaN-box |
| Pattern match extraction from `Some(x)` where x: Int | IntRep | Unbox after extraction |
| List cons `[x, ...]` | BoxedRep | Yes — list elements are boxed |
| Returned from `main` to C runtime | TaggedRep | Yes — C runtime expects NaN-box |

### Interaction with Aether (RC)

Aether's `Dup`/`Drop`/`Reuse` only apply to `BoxedRep` and `TaggedRep` values. Unboxed values (`IntRep`, `FloatRep`, `BoolRep`) have no heap allocation and need no reference counting:

```rust
fn lower_dup(&mut self, var: CoreVarRef, body: &CoreExpr) {
    if var.rep == FluxRep::IntRep || var.rep == FluxRep::FloatRep || var.rep == FluxRep::BoolRep {
        // No-op: unboxed values don't need RC.
        return self.lower_expr(body);
    }
    // Boxed: emit flux_dup as before.
    let val = self.load_var_value(var)?;
    self.emit_dup_call(val);
    self.lower_expr(body)
}
```

This eliminates unnecessary `flux_dup`/`flux_drop` calls on primitive values — a measurable win in numeric code.

### Polymorphism and TaggedRep

When a function is polymorphic (type variable not resolved), values use `TaggedRep` — the existing NaN-box encoding with runtime tag dispatch:

```flux
fn identity(x) { x }   // identity : a -> a
```

HM infers `a` (unresolved type variable) → `TaggedRep`. The codegen emits the same NaN-boxed code as today. No performance regression for polymorphic code.

When a polymorphic function is called with a known type, **specialization** at the call site can insert box/unbox:

```flux
let y = identity(42)    // identity called with Int
```

The compiler knows `42` is `IntRep` and `identity` expects `TaggedRep`, so it inserts a box before the call and an unbox after. Future work: monomorphization (generating `identity_Int` that works entirely in `IntRep`).

### Implementation phases

**Phase 1 — FluxRep infrastructure** (~1 week) ✅ **DONE**
- `FluxRep` enum defined in `src/core/mod.rs` with variants: `IntRep`, `FloatRep`, `BoolRep`, `BoxedRep`, `TaggedRep`, `UnitRep`
- `rep` field added to `CoreBinder` (defaults to `TaggedRep`)
- `CoreBinder::with_rep()` constructor for typed binders
- `type_to_rep` conversion populates `rep` during AST → Core lowering in `src/core/lower_ast/mod.rs`
- Core passes (`anf`, `evidence`) propagate `rep` through transformations

**Phase 2 — Typed arithmetic** (~1 week) ✅ **DONE**
- `core_to_llvm` codegen checks `FluxRep` on binders to emit raw arithmetic
- `IntRep` operands use raw `add`/`sub`/`mul`/`div`/`icmp` without tag/untag
- `FloatRep` operands use `fadd`/`fsub`/`fmul`/`fdiv`/`fcmp`
- `TaggedRep` falls back to NaN-box path with runtime dispatch

**Phase 3 — Typed function signatures (worker/wrapper)** (~1 week) ✅ **DONE**
- `qualifies_for_int_worker_wrapper()` in `src/core_to_llvm/codegen/function.rs` detects all-`IntRep` functions
- Worker function emits raw `i64` parameters and return — no NaN-boxing
- Wrapper function untags args, calls worker, retags result at the boundary
- Self-recursive calls in the worker target the worker directly (raw args)

**Phase 4 — Box/unbox insertion** (~1 week) ✅ **DONE**
- Automatic box/unbox at boundaries where reps disagree
- Worker/wrapper handles the primary box/unbox case for function calls
- ADT fields and closure captures remain NaN-boxed (`BoxedRep`)

**Phase 5 — Aether optimization** (~3 days) ✅ **DONE**
- `var_is_unboxed()` check in `src/core_to_llvm/codegen/aether.rs`
- `lower_dup` and `lower_drop` skip `flux_dup`/`flux_drop` for `IntRep`/`FloatRep`/`BoolRep`
- Eliminates unnecessary RC overhead on immediate scalar values

**Phase 6 — Benchmarks and validation** (~3 days) ✅ **DONE**
- All examples run through typed codegen via `--native` flag
- Output parity verified with VM backend

---

## Drawbacks

- **Complexity**: Adding a representation layer to Core IR touches many files. Every pass that creates or transforms `CoreBinder` must propagate `rep` correctly.

- **Box/unbox insertion correctness**: Getting the boxing discipline right is subtle. A missing box causes type confusion at runtime (e.g., raw `42` interpreted as a NaN-boxed pointer → segfault). A missing unbox wastes performance but is semantically correct. The safe default is `TaggedRep` everywhere, with unboxing as an optimization.

- **Polymorphism boundary overhead**: When unboxed values cross into polymorphic contexts, boxing adds overhead that doesn't exist in the current all-NaN-boxed world. However, this overhead is localized to boundaries and is far less than the current overhead of boxing everywhere.

- **Two calling conventions**: Functions with known types use unboxed parameters; polymorphic functions use NaN-boxed parameters. Calling a typed function from a polymorphic context requires wrapper generation. GHC handles this with "worker/wrapper" transformation.

---

## Rationale and alternatives

### Why follow GHC's PrimRep model?

GHC's three-tier system (PrimRep → CmmType → LlvmType) has been refined over 20+ years. It cleanly separates "what the source language means" from "how values are stored" from "what LLVM instructions to emit." Flux's `FluxRep` is a simplified version of `PrimRep` — fewer variants because Flux has fewer types than Haskell.

### Alternative: Monomorphization

Instead of carrying types and boxing/unboxing, generate specialized versions of every function for each concrete type it's called with. `map_Int_Int`, `map_String_Bool`, etc. This eliminates all boxing but causes code bloat (exponential in the number of type parameters). Rust and C++ do this; GHC does not (it uses boxing + occasional specialization). For Flux, monomorphization is future work — the FluxRep approach gives 80% of the benefit with 20% of the complexity.

### Alternative: Keep all-NaN-boxed

Do nothing. The current approach works and is simple. But it leaves significant performance on the table — GHC without unboxing would be 3-5x slower on numeric code. For a language that aims to compete with native performance, this is not acceptable long-term.

### Alternative: Separate boxed and unboxed types in the surface language

Like Haskell's `Int` vs `Int#`, require users to explicitly choose boxed or unboxed types. This leaks implementation details into the surface language and makes Flux harder to learn. The compiler should make this decision automatically based on inferred types.

---

## Prior art

### GHC (Haskell)

GHC's `PrimRep` system is the direct inspiration. Key papers:
- "Unboxed values as first-class citizens" (Peyton Jones & Launchbury, 1991) — the foundational paper
- "Worker/Wrapper transformation" — generates unboxed worker functions from boxed wrappers
- GHC's strictness analyzer identifies values that can be unboxed

GHC achieves 3-10x speedup from unboxing on numeric benchmarks.

### OCaml

OCaml uses a single-bit tag on every value (lowest bit: 0 = pointer, 1 = immediate integer). Integers are 63-bit (on 64-bit platforms). Floats are always boxed unless stored in a float array. This is simpler than GHC's approach but wastes one bit for every integer and boxes all floats.

### Lean 4

Lean 4 compiles to C with unboxed scalars. The compiler tracks which values are "scalar" (unboxed) vs "object" (heap-allocated) and generates different C code for each. Lean's approach is closest to what this proposal describes — automatic unboxing based on type information.

### Koka

Koka compiles to C with Perceus reference counting. Values are either "raw" (unboxed integers, booleans) or "boxed" (heap pointers). The compiler's type information determines which representation to use. This is exactly the `FluxRep` model.

---

## Unresolved questions

- **Should `FluxRep` live on every `CoreExpr` node or just on binders?** GHC puts types on every STG expression via `Id`. For Flux, annotating just binders and let-bound expressions may be sufficient — the type of any sub-expression can be computed from the types of its components.

- **How to handle `Option Int`?** The `Some` constructor wraps an `Int`. Should the field be stored as unboxed `i64` in the ADT, or as a NaN-boxed `i64`? Unboxed fields require knowing the ADT layout at compile time; NaN-boxed fields are uniform. GHC uses unboxed fields via the "unpack" pragma — Flux could do this automatically when the field type is known.

- **Worker/wrapper transformation?** GHC generates an unboxed "worker" and a boxed "wrapper" for exported functions. Should Flux do the same, or is call-site boxing sufficient?

- **Impact on Aether**: If `IntRep` values skip `Dup`/`Drop`, the Aether pass must be type-aware. Should Aether run before or after type-rep assignment? Before is simpler (treat everything as boxed, then optimize away unnecessary RC in codegen); after is more precise (never insert Dup/Drop for unboxed values).

- **TaggedRep as default**: During the transition, all existing code should continue to work by defaulting to `TaggedRep` when type information is missing. The optimization is opt-in per expression, not all-or-nothing.

---

## Future possibilities

- **Monomorphization**: Generate specialized versions of frequently-called polymorphic functions. `map : (a→b) → [a] → [b]` called with `Int→Int` generates `map_Int_Int` that passes raw `i64` values. Combined with LLVM inlining, this eliminates closure call overhead for known function arguments.

- **Unboxed ADT fields**: Store `Int` and `Float` fields directly in ADT allocations without NaN-boxing. `Some(42)` becomes a 16-byte allocation (tag + raw i64) instead of 24 bytes (tag + NaN-boxed i64). Requires compile-time knowledge of field types.

- **Unboxed arrays**: `[Int]` as a contiguous array of raw `i64` values instead of a cons-list of NaN-boxed values. Enables SIMD vectorization by LLVM.

- **Strict/unboxed annotations**: Optional programmer hints (`!Int` or `Int#`) for cases where the compiler's analysis is conservative. Like GHC's `BangPatterns` and `MagicHash`.

- **Profile-guided unboxing**: Use runtime profiling data to identify hot paths where unboxing would help most, then recompile with targeted unboxing.

- **Register allocation hints**: `IntRep` → GPR, `FloatRep` → FPR. Pass this information to LLVM via parameter attributes for better register allocation on architectures with separate register files (ARM, RISC-V).
