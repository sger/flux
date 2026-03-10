- Feature Name: Cranelift JIT Improvements & Flux IR Layer
- Start Date: 2026-03-10
- Completion Date: pending
- Status: Draft
- Proposal PR: pending
- Flux Issue: pending
- Depends on: 0031 (cranelift jit backend), 0077 (type-informed optimization), 0086 (backend-neutral core ir)

# Proposal 0098: Cranelift JIT Improvements & Flux IR Layer

## Summary

Seven targeted improvements to the Flux Cranelift JIT backend (`src/jit/`) ranging from
low-effort micro-optimizations (skip redundant C calls, reuse stack slots, intern unit ADTs)
to a medium-effort type-directed arithmetic inlining pass, up to an architectural Flux IR
layer that decouples the AST from Cranelift and enables a proper optimization pipeline.

## Motivation

The Flux JIT backend (`src/jit/compiler.rs`, 5 896 lines) compiles the AST directly to
Cranelift IR in a single pass. This works correctly but leaves significant performance on
the table:

- Every `if` condition calls `rt_is_truthy` via an `extern "C"` round-trip, even when HM
  inference has already proven the expression is `Bool`.
- Every arithmetic and comparison operation calls a runtime helper even when both operands
  are statically `Int` or `Float`.
- Every Base function call allocates a fresh `stack_slot_create` for the arguments array,
  even when the same slot could be reused across calls in the same function.
- Nullary ADT constructors (`None`, unit variants) heap-allocate a new `HeapObject::Adt`
  on every evaluation, despite being immutable and structurally identical each time.
- The flat AST → Cranelift pass has no intermediate representation, making it impossible
  to run optimization passes, test IR independently, or retarget to another backend.

HM inference output (`ExprTypeMap`) is already threaded into the JIT compiler — it is used
in the VM strict-mode path via `hm_expr_typer.rs` — but the JIT expression lowering does
not query it. This means type information that already exists at compile time is discarded
before codegen.

Benchmarks in `reports/CFOLD_REPORT.md` and `reports/DERIV_REPORT.md` show the JIT
performing 17–18× slower than Rust; arithmetic-heavy programs are the primary gap.

## Guide-level explanation

From a compiler contributor's perspective, the improvements fall into three tiers:

**Tier 1 — Local patches (Proposals 1–4):** Isolated changes inside `FunctionCompiler`
that require no new data structures. Each can be reviewed and merged independently.

**Tier 2 — Type-directed codegen (Proposals 1, 5):** Query `ExprTypeMap` in expression
lowering to emit typed fast paths. Requires plumbing an `ExprTypeMap` reference into
`FunctionCompiler`. Once plumbed, adding new typed fast paths is additive.

**Tier 3 — Flux IR layer (Proposal 7):** A new `src/ir/` crate providing a typed SSA-lite
three-address IR. The AST is lowered to Flux IR first; optimization passes run on Flux IR;
Cranelift lowering reads Flux IR. Proposals 1–5 become IR-level passes rather than ad-hoc
patches to `compiler.rs`.

From a Flux user's perspective, programs compile to faster native code with no syntax
changes. The improvements are most visible in arithmetic-heavy code (constant folding,
derivative computation) and in programs that construct option/result variants in loops.

## Reference-level explanation

### Proposal 1 — Skip `rt_is_truthy` for known-Bool conditions

**Problem:** Every `if` condition emits a call to `rt_is_truthy` (an `extern "C"` helper)
regardless of the condition's inferred type.

**Solution:** In `compile_if_expr` (compiler.rs ~line 3860), consult `ExprTypeMap` for the
condition expression. When the inferred type is `Bool`, the `Value::Bool` payload is already
an `i64` in the arena slot. Emit a direct Cranelift `icmp` against zero:

```
%payload = load.i64 cond_slot+8   // skip Value tag word
%branch  = icmp ne %payload, 0
brnz %branch, then_block
jump else_block
```

**Affected file:** `src/jit/compiler.rs` — `compile_if_expr`

---

### Proposal 2 — Inline integer arithmetic using ExprTypeMap

**Problem:** Every arithmetic and comparison PrimOp calls a runtime helper even when both
operands are statically `Int`:

```
// current — always emitted
call rt_add(ctx, lhs_ptr, rhs_ptr) → *mut Value
```

**Solution:** For binary PrimOps where both operands have HM type `Int`, emit Cranelift
integer instructions directly and write a tagged `Value::Int` inline — no helper call, no
arena allocation:

```
%lhs_payload = load.i64 lhs_slot+8
%rhs_payload = load.i64 rhs_slot+8
%result      = iadd %lhs_payload, %rhs_payload
store.i64 TAG_INT  → result_slot+0
store.i64 %result  → result_slot+8
```

For comparisons (`<`, `>`, `==`) where operands are `Int`, emit `icmp` and write
`Value::Bool`. Apply the same pattern for `Float` using `fadd`/`fsub`/`fmul`/`fdiv`.

**Affected files:**
- `src/jit/compiler.rs` — `compile_primop_call` / binary op dispatch
- `src/jit/context.rs` — expose `ExprTypeMap` reference to compiler

**Expected impact:** 20–40% speedup on numeric benchmarks (`cfold`, `deriv`).

---

### Proposal 3 — Reuse stack slots across calls

**Problem:** Every Base function call and PrimOp call allocates a fresh `stack_slot_create`
for the arguments array. In a function with ten Base calls, ten independent slots are
created even when they are never live simultaneously.

**Solution:** At function entry, scan the function body for the maximum Base/PrimOp arity
used. Allocate one args stack slot sized to that arity and reuse it for every call:

```rust
struct FunctionCompiler<'a> {
    // existing fields …
    args_slot: Option<StackSlot>,    // reusable args array slot
    args_slot_capacity: usize,
}
```

**Affected file:** `src/jit/compiler.rs` — `FunctionCompiler` struct,
`compile_base_call`, `compile_primop_call`

---

### Proposal 4 — Intern unit ADT variants as constants

**Problem:** `rt_make_adt0` (nullary ADT constructor) always heap-allocates a new
`HeapObject::Adt` even though the value is immutable and structurally identical every time.

**Solution:** During compilation, for each nullary constructor encountered, call
`rt_make_adt0` once at module init time and store the result as a JIT constant pointer.
At use sites, emit an `iconst` of the pre-allocated pointer:

```rust
// at compile time (once per constructor tag)
let const_val: *mut Value = context.intern_unit_adt(tag_id);
// at use site — pure load, no allocation
let ptr_val = builder.ins().iconst(types::I64, const_val as i64);
```

This is safe because ADT values are immutable and the arena is never compacted.

**Affected files:**
- `src/jit/context.rs` — `intern_unit_adt` map
- `src/jit/compiler.rs` — ADT construction lowering

---

### Proposal 5 — Unbox Base function arguments on typed call sites

**Problem:** Before every Base function call, all arguments are boxed into a stack-allocated
`Value` array regardless of their static types. When argument types are statically known
(e.g. `print(42 : Int)`), this boxing is wasteful.

**Solution:** Introduce a typed fast path for Base functions whose parameter types are fully
known. Pass `i64` payloads in registers up to arity 4 (matching the existing
`JitCallAbi::Reg1–Reg4` scheme) and let the Base shim unpack them directly. Start with
`print_int` and `print_float` as proof-of-concept since they appear in every benchmark.

**Affected files:**
- `src/jit/runtime_helpers.rs` — typed shim variants
- `src/jit/compiler.rs` — call-site routing
- `src/runtime/base/` — corresponding unboxed entry points

---

### Proposal 6 — Mutual tail-call optimization via trampoline

**Problem:** Self-recursion is compiled to a `jump` back to the loop block (no call frame).
Mutual recursion (e.g. `isEven`/`isOdd`, CPS-transformed state machines) still generates
full Cranelift `call` instructions and grows the stack.

**Solution:** Add a trampoline executor at the top level. Mutually tail-recursive calls
return a `Thunk { function_index, args }` value instead of making a call. The trampoline
loop in `context.rs::invoke_value` drives execution:

```
loop:
  result = invoke(thunk.fn, thunk.args)
  if result is Thunk → thunk = result; goto loop
  else → return result
```

Detection: in tail position, if the callee is a known JIT function (not a closure), emit a
`Value::Thunk` return instead of a `call`. This is opt-in per call site and does not affect
non-tail calls.

**Affected files:**
- `src/jit/context.rs` — trampoline loop in `invoke_value`
- `src/jit/compiler.rs` — tail-call detection, thunk emission
- `src/jit/value_arena.rs` — `Value::Thunk` variant

---

### Proposal 7 — Introduce a Flux IR layer

**Problem:** `src/jit/compiler.rs` is 5 896 lines performing AST → Cranelift IR in one
pass with no intermediate representation:

- No place to run peephole optimizations (constant folding, dead block elimination, CSE)
  before Cranelift sees the IR.
- Pattern-matching lowering and phi-node merging are deeply coupled to Cranelift API calls,
  making them untestable in isolation.
- The VM bytecode compiler (`src/bytecode/compiler/`) duplicates similar logic with no
  sharing.
- Impossible to retarget to another backend without rewriting from scratch.

**Solution:** Introduce Flux IR — a typed, SSA-lite three-address IR between the AST and
any backend.

```rust
// src/ir/mod.rs

pub struct IrFunction {
    pub name: Identifier,
    pub params: Vec<(Identifier, FluxIRType)>,
    pub ret_type: FluxIRType,
    pub blocks: Vec<IrBlock>,
    pub entry: BlockId,
}

pub struct IrBlock {
    pub id: BlockId,
    pub params: Vec<(IrVar, FluxIRType)>,   // phi params
    pub instrs: Vec<IrInstr>,
    pub terminator: IrTerminator,
}

pub enum IrInstr {
    Assign(IrVar, IrExpr),
    Call(IrVar, IrCallTarget, Vec<IrVar>),
}

pub enum IrExpr {
    Const(IrConst),
    Var(IrVar),
    IAdd(IrVar, IrVar),        // typed — no helper needed
    ISub(IrVar, IrVar),
    IMul(IrVar, IrVar),
    FAdd(IrVar, IrVar),
    ICmp(CmpOp, IrVar, IrVar),
    MakeCons(IrVar, IrVar),
    MakeAdt(u32, Vec<IrVar>),
}

pub enum IrTerminator {
    Jump(BlockId, Vec<IrVar>),
    Branch(IrVar, BlockId, BlockId),
    Return(IrVar),
    TailCall(IrVar, Vec<IrVar>),
}
```

Optimization passes on Flux IR (before Cranelift lowering):

| Pass | Description |
|------|-------------|
| Constant folding | `IAdd(Const(2), Const(3))` → `Const(5)` |
| Dead block elimination | Remove blocks with no predecessors |
| Unit ADT interning | Replace `MakeAdt(tag, [])` with pre-allocated constant ref |
| Type-directed unboxing | Downgrade `Any` ops to typed ops where HM type is known |
| CSE | Deduplicate identical pure expressions within a block |

Proposals 1–5 become IR-level passes rather than ad-hoc patches to `compiler.rs`.

**New files:**
- `src/ir/mod.rs` — IR type definitions
- `src/ir/lower.rs` — AST → Flux IR lowering
- `src/ir/passes/` — optimization pass modules

**Modified files:**
- `src/jit/compiler.rs` — consumes Flux IR instead of AST directly
- `src/jit/mod.rs` — pipeline: AST → IR → optimize → Cranelift

## Drawbacks

**Proposal 2 (arithmetic inlining):** Requires overflow behaviour parity between
`rt_add` and Cranelift `iadd`. The current helpers return a `Value::Int` wrapping Rust
`i64::wrapping_add`; Cranelift `iadd` is also wrapping, so parity holds. Division by zero
still needs a guard before emitting `idiv`.

**Proposal 7 (Flux IR):** Adding an IR layer increases total lines of code and the number
of compilation stages. The lowering pass (AST → Flux IR) must be kept in sync with any
future AST changes. Migration risk: `compiler.rs` is large and has subtle control-flow
invariants that must be preserved during the rewrite.

**Proposal 6 (mutual TCO):** Returning `Value::Thunk` changes the observable calling
convention for JIT functions. Any external caller (e.g. the test harness) must handle
thunk values, adding complexity at the execution boundary.

## Rationale and alternatives

**Inline only in strict-mode:** Proposals 1 and 2 could be limited to functions where all
parameters are typed (strict mode). This reduces the code path count but leaves most user
code unoptimized since strict mode is opt-in.

**Single-pass typed codegen without IR:** Apply Proposals 1–5 as direct patches to
`compiler.rs` without introducing Flux IR. This is lower risk short-term but perpetuates
the single-pass architecture and does not enable further optimization passes. Proposal 7
is the right long-term answer; Proposals 1–5 inform what the IR needs to express.

**NaN-boxing value representation (Proposal 0041):** A tagged pointer / NaN-boxing scheme
would eliminate the `slot+8` payload reads entirely and make unboxed arithmetic even
cheaper. That is a separate, larger change; Proposals 1–2 provide a stepping-stone in the
same direction within the current `Value` layout.

## Prior art

- **LuaJIT** inlines arithmetic for typed integer/float traces using a similar type-check
  + inline IR approach. The tracing model differs but the "skip the C helper when the type
  is known" pattern is the same.
- **GHC's Cmm IR** is a typed, low-level IR between STG and native codegen that serves
  the same structural role as the proposed Flux IR.
- **Cranelift's own `filetests`** test IR in isolation from the host language — the Flux IR
  layer would enable the same isolation for Flux JIT testing.

## Unresolved questions

1. Should Flux IR be typed at the `FluxIRType` level or re-use `InferType` from `src/types/`?
   Using `InferType` avoids a second type representation but couples IR to the HM layer.

2. What is the right granularity for the IR lowering unit — per function or per module?
   Per-module enables cross-function analysis (inlining, escape analysis) but requires
   holding the full module in memory during lowering.

3. For Proposal 2, should the typed fast path be guarded by a compile-time check
   (`debug_assert!(both_operands_are_int)`) or fully trusted based on HM inference output?

4. Does Proposal 6 (mutual TCO trampoline) need to be visible to the VM as well, or is
   JIT-only sufficient?

## Implementation sequence

| Step | Proposal | File(s) | Description |
|------|----------|---------|-------------|
| S1 | 3 | `src/jit/compiler.rs` | Add `args_slot` to `FunctionCompiler`, reuse across calls |
| S2 | 4 | `src/jit/context.rs`, `compiler.rs` | Add `intern_unit_adt` map, emit `iconst` at use sites |
| S3 | 1 | `src/jit/compiler.rs` | Skip `rt_is_truthy` for Bool conditions using `ExprTypeMap` |
| S4 | 2 | `src/jit/compiler.rs`, `context.rs` | Inline `Int` arithmetic; plumb `ExprTypeMap` reference |
| S5 | 5 | `src/jit/runtime_helpers.rs`, `compiler.rs` | Typed Base shims for `print_int`, `print_float` |
| S6 | 6 | `src/jit/context.rs`, `compiler.rs` | Mutual TCO trampoline |
| S7 | 7 | `src/ir/mod.rs`, `src/ir/lower.rs` | Scaffold Flux IR types + AST lowering (no optimization yet) |
| S8 | 7 | `src/ir/passes/` | Constant folding + dead block elimination passes |
| S9 | 7 | `src/jit/compiler.rs` | Switch JIT to consume Flux IR; remove direct AST walking |

S1–S2 can be merged independently before any `ExprTypeMap` plumbing. S3–S4 share the same
plumbing step and should be developed together. S7 can start in parallel with S5–S6 as a
separate branch.

## Future possibilities

- **WASM backend:** Once Flux IR exists (Proposal 7), a WASM lowerer can be added under
  `src/wasm/` without touching the JIT or VM.
- **Profile-guided optimization:** Flux IR pass could specialize hot call sites based on
  runtime type frequency data.
- **Escape analysis:** An IR-level escape analysis pass could promote short-lived cons
  cells and ADTs to the stack, reducing GC pressure.
- **Shared IR between VM and JIT:** The bytecode compiler (`src/bytecode/compiler/`) could
  lower from Flux IR instead of the AST, unifying the two codegen backends and eliminating
  duplicated lowering logic.
