- Feature Name: Shared Low-Level IR — Unified Backend Representation
- Start Date: 2026-03-27
- Proposal PR:
- Flux Issue:
- Depends on: Proposal 0131 (Unified C Runtime Primops)

## Summary

Replace Flux's divergent backend lowering (Core → CFG for VM, Core → LLVM IR for native) with a shared low-level intermediate representation (LIR) that both backends consume. After this proposal, control flow lowering, ADT layout, closure construction, pattern matching, and Aether dup/drop all happen **once** in Core → LIR, and the two backends are thin code emitters from the same representation.

This follows GHC's Cmm architecture: STG lowers to Cmm once, then NCG, LLVM, and Via-C all consume the same Cmm.

## Motivation

### The problem beyond primops

Proposal 0131 unifies primop implementations by having both backends call the same C runtime. But primops are only half the story. The backends also diverge on **control flow lowering**:

| Concern | VM path (Core → CFG) | LLVM path (Core → LLVM IR) | Same? |
|---------|----------------------|---------------------------|-------|
| ADT construction | `IrExpr::MakeAdt` in CFG | `emit_make_adt` in LLVM codegen | No — different field layout code |
| Pattern matching | `IrTerminator::Switch` on ADT tags | `emit_case_expr` with tag extraction | No — different tag check logic |
| Closure creation | `IrExpr::MakeClosure` in CFG | `emit_make_closure` with capture array | No — different capture handling |
| Closure calls | `IrCallTarget::Dynamic` in CFG | `emit_closure_call` with arity dispatch | No — different call conventions |
| Aether dup/drop | Applied during Core → CFG lowering | Applied during Core → LLVM lowering | No — applied at different points |
| MemberAccess | `IrExpr::MemberAccess` with module_name | `MemberAccess` primop with module_members lookup | No — different resolution strategy |
| Tail calls | `IrCallTarget::SelfTail` in CFG | `emit_tail_call` with frame reuse in LLVM | No — different TCO strategies |
| Effect handlers | `IrTerminator::Handle/Perform` | Not supported in LLVM | Not comparable |

Each of these is a potential parity bug. MemberAccess resolution already caused a non-deterministic bug (HashMap iteration order) that took significant debugging to find.

### What GHC's Cmm solves

In GHC, **all** of these concerns are lowered ONCE in `StgToCmm`:

```
STG → StgToCmm → Cmm:
  - ADT construction → CmmStore(hp + offset, field)
  - Pattern matching → CmmSwitch(tag, [label1, label2, ...])
  - Closure creation → CmmStore(hp, info_table_ptr); CmmStore(hp+8, free_var_0)
  - Closure calls   → CmmCall(ENTRY_CODE(info_ptr), args_in_registers)
  - Tail calls      → CmmCall with no return frame
  - Thunk update    → CmmStore(thunk, indirection_info)
```

Then NCG, LLVM, and Via-C backends are just **instruction selection** from Cmm — they don't need to understand ADTs, closures, or pattern matching.

### What this proposal achieves

```
Before (two lowering paths):
  Core → Aether → CFG → Bytecode → VM           (CFG knows about ADTs, closures, effects)
  Core → Aether → LLVM IR → native               (LLVM codegen re-lowers ADTs, closures)

After (one lowering path):
  Core → Aether → LIR → { Bytecode → VM, LLVM IR → native }
                   ↑
              everything lowered to:
              - NaN-box tag/untag
              - memory load/store
              - C runtime calls
              - conditional branches
              - closure protocol (alloc + fill + call)
```

---

## Reference-level explanation

### LIR design

The LIR is a typed, NaN-box-aware CFG with explicit memory operations. It sits between Core IR (functional, high-level) and machine code (registers, instructions).

#### LIR types

```rust
// src/lir/types.rs

/// LIR operates on NaN-boxed i64 values exclusively.
/// No Value enum, no Rc, no heap types at this level.
pub type LirValue = i64;  // NaN-boxed

/// Machine-level types for LIR expressions.
pub enum LirType {
    I64,        // NaN-boxed value (default)
    I32,        // integer (tags, indices)
    I1,         // boolean
    Ptr,        // raw pointer (for C runtime calls)
    Void,       // no return value
}
```

#### LIR instructions

```rust
// src/lir/mod.rs

pub enum LirInstr {
    // ── Memory ──
    /// Load NaN-boxed value from pointer + offset
    Load { dst: LirVar, ptr: LirVar, offset: i32 },
    /// Store NaN-boxed value to pointer + offset
    Store { ptr: LirVar, offset: i32, val: LirVar },
    /// Allocate n bytes on the heap, return pointer
    Alloc { dst: LirVar, size: LirVar },

    // ── NaN-box operations ──
    /// Tag an integer as NaN-boxed value
    TagInt { dst: LirVar, raw: LirVar },
    /// Untag a NaN-boxed value to raw integer
    UntagInt { dst: LirVar, val: LirVar },
    /// Extract the tag bits from a NaN-boxed value
    GetTag { dst: LirVar, val: LirVar },
    /// Tag a heap pointer as NaN-boxed value
    TagPtr { dst: LirVar, ptr: LirVar, tag: u8 },
    /// Untag a NaN-boxed value to raw heap pointer
    UntagPtr { dst: LirVar, val: LirVar },

    // ── Arithmetic (inline, no C call) ──
    IAdd { dst: LirVar, a: LirVar, b: LirVar },
    ISub { dst: LirVar, a: LirVar, b: LirVar },
    IMul { dst: LirVar, a: LirVar, b: LirVar },
    ICmp { dst: LirVar, op: CmpOp, a: LirVar, b: LirVar },

    // ── C runtime calls ──
    /// Call a C runtime function with NaN-boxed args
    CCall { dst: Option<LirVar>, func: &'static str, args: Vec<LirVar> },

    // ── Aether RC ──
    /// Increment reference count
    Dup { val: LirVar },
    /// Decrement reference count (may free)
    Drop { val: LirVar },
    /// Check if value is uniquely owned (Rc::strong_count == 1)
    IsUnique { dst: LirVar, val: LirVar },

    // ── Variables ──
    /// Copy value
    Copy { dst: LirVar, src: LirVar },
    /// Load constant from constant pool
    Const { dst: LirVar, value: LirConst },
}

pub enum LirTerminator {
    /// Return value to caller
    Return(LirVar),
    /// Unconditional jump
    Jump(LirBlockId),
    /// Conditional branch
    Branch { cond: LirVar, then_block: LirBlockId, else_block: LirBlockId },
    /// Multi-way switch (for ADT tag dispatch)
    Switch { scrutinee: LirVar, cases: Vec<(i32, LirBlockId)>, default: LirBlockId },
    /// Tail call (reuse current frame)
    TailCall { func: LirVar, args: Vec<LirVar> },
    /// Regular call with continuation
    Call { dst: LirVar, func: LirVar, args: Vec<LirVar>, cont: LirBlockId },
    /// Unreachable (after panic, etc.)
    Unreachable,
}
```

### How Core constructs lower to LIR

#### ADT construction

```
Core:   MakeAdt("Some", [value])

LIR:    %ptr = Alloc(24)                          // 3 words: tag + ctor_name + field
        Store(%ptr, 0, TAG_ADT)                    // info word
        Store(%ptr, 8, %ctor_name_ptr)             // constructor name
        Store(%ptr, 16, %value)                    // field 0
        %result = TagPtr(%ptr, TAG_BOXED_VALUE)    // NaN-box the pointer
```

#### Pattern matching

```
Core:   case x of
          Some(v) -> body1
          None    -> body2

LIR:    %tag = GetTag(%x)
        Switch(%tag) {
            TAG_ADT -> check_ctor:
            TAG_NONE -> none_case:
        }
        check_ctor:
            %ptr = UntagPtr(%x)
            %ctor_tag = Load(%ptr, 0)
            Branch(%ctor_tag == SOME_TAG, some_case, none_case)
        some_case:
            %v = Load(%ptr, 16)                    // extract field 0
            ... body1 ...
        none_case:
            ... body2 ...
```

#### Closure creation

```
Core:   Lam([x], body) with free vars [a, b]

LIR:    %closure = CCall("flux_make_closure",
                         [%fn_ptr, 1, %captures_ptr, 2, %applied_ptr, 0])
        // captures_ptr points to [a, b]
        // fn_ptr points to the compiled function
```

#### Closure call

```
Core:   App(f, [x, y])

LIR:    %result = CCall("flux_call_closure", [%f, %args_ptr, 2])
```

#### Aether dup/drop

```
Core:   Dup(x); ... use x twice ...

LIR:    Dup(%x)                                    // calls flux_dup
        ... use %x in branch A ...
        ... use %x in branch B ...
        Drop(%x)                                   // calls flux_drop
```

### How backends consume LIR

#### Bytecode emitter (VM)

Each LIR instruction maps to one or more bytecode opcodes:

| LIR instruction | Bytecode |
|-----------------|----------|
| `Load(dst, ptr, off)` | `OpGetLocal(ptr); OpConstant(off); OpIndex` |
| `Store(ptr, off, val)` | Internal VM operation |
| `TagInt(dst, raw)` | Implicit (VM values are already tagged) |
| `CCall("flux_array_push", [arr, elem])` | `OpPrimOp(Push, 2)` |
| `Branch(cond, then, else)` | `OpJumpNotTruthy(else_addr)` |
| `Switch(tag, cases)` | Series of `OpCmpEqJumpNotTruthy` |
| `Dup(val)` | `OpGetLocal(val)` (Rc::clone implicit) |
| `Drop(val)` | `OpAetherDropLocal(val)` |
| `Return(val)` | `OpReturnValue` |

#### LLVM IR emitter (native)

Each LIR instruction maps to LLVM instructions:

| LIR instruction | LLVM IR |
|-----------------|---------|
| `Load(dst, ptr, off)` | `%dst = load i64, ptr getelementptr(%ptr, %off)` |
| `Store(ptr, off, val)` | `store i64 %val, ptr getelementptr(%ptr, %off)` |
| `TagInt(dst, raw)` | `%dst = call @flux_tag_int(%raw)` |
| `CCall("flux_array_push", [arr, elem])` | `%r = call @flux_array_push(%arr, %elem)` |
| `Branch(cond, then, else)` | `br i1 %cond, label %then, label %else` |
| `Switch(tag, cases)` | `switch i32 %tag, label %default [i32 0, label %c0 ...]` |
| `Dup(val)` | `call @flux_dup(%val)` |
| `Drop(val)` | `call @flux_drop(%val)` |
| `Return(val)` | `ret i64 %val` |

### Migration strategy

The LIR doesn't need to replace CFG in one step. It can be introduced incrementally:

**Step 1**: Define the LIR data structures (`src/lir/mod.rs`)

**Step 2**: Write `Core → LIR` lowering that handles the simplest cases (arithmetic, let bindings, function definitions). The LLVM backend switches to consuming LIR for these cases.

**Step 3**: Add ADT construction and pattern matching to LIR lowering. Both backends use LIR for these.

**Step 4**: Add closure creation and calls. Both backends use LIR.

**Step 5**: Add Aether dup/drop to LIR. The Aether pass runs on LIR instead of Core.

**Step 6**: Add effect handlers to LIR (VM-only initially, since the native backend doesn't support them yet).

**Step 7**: Delete CFG IR. Delete LLVM codegen's Core lowering. Both backends consume LIR exclusively.

At each step, the existing backends continue working — the LIR is additive until step 7.

---

## Comparison: Flux LIR vs GHC Cmm

| Aspect | GHC Cmm | Flux LIR |
|--------|---------|----------|
| **Type system** | CmmType (width + category: GcPtr/Bits/Float) | LirType (I64/I32/I1/Ptr/Void) |
| **Values** | Untyped words, GC-traced pointers | NaN-boxed i64 exclusively |
| **Registers** | R1-R6 (virtual), Sp, Hp (STG machine) | LirVar (SSA variables) |
| **Memory** | CmmLoad/CmmStore with explicit addresses | Load/Store with pointer + offset |
| **Allocation** | Bump pointer (Hp += size) | CCall to `malloc` or `flux_alloc` |
| **GC** | Heap check + GC entry points | None (Aether RC: Dup/Drop instructions) |
| **Calls** | CmmCall with register conventions | CCall to C runtime functions |
| **Stack** | Explicit Sp manipulation | Implicit (VM manages stack, LLVM uses SSA) |
| **Closures** | Info table + payload in heap | CCall to `flux_make_closure` |
| **Backends** | NCG, LLVM, Via-C, JS | Bytecode (VM), LLVM IR |

**Key difference:** GHC's Cmm manages the stack explicitly (Sp register) because both NCG and LLVM need stack layout control. Flux's LIR can leave stack management to the backends — the VM has its own stack, and LLVM uses SSA phi nodes.

---

## Drawbacks

- **Large refactoring effort** — Core → LIR lowering is essentially rewriting the Core → CFG lowering AND the Core → LLVM lowering as a single pass. This is the biggest single change to the compiler since the LLVM backend was added.

- **Effect handlers are VM-only** — The native backend doesn't support algebraic effects. LIR needs either (a) effect handler instructions that only the VM emitter handles, or (b) effects must be lowered before LIR (in Core passes). Option (b) is cleaner but requires CPS transformation of effect handlers in Core.

- **Performance risk during transition** — The LIR adds an extra IR level (Core → LIR → Bytecode vs Core → CFG → Bytecode). This could slow compilation. Mitigated by: LIR is simpler than CFG (fewer node types), so lowering should be faster.

- **Loss of backend-specific optimizations** — CFG currently has VM-specific optimizations (superinstructions, peephole). LIR is backend-neutral, so these optimizations move to the bytecode emitter. Not a problem — they belong there anyway.

## Rationale and alternatives

### Why not just improve parity testing?

Parity testing is reactive — it catches bugs after they exist, only for tested cases. A shared LIR makes parity bugs impossible by construction for control flow, ADT layout, and closure handling. Combined with Proposal 0131 (unified C primops), the entire lowering from Core to execution is single-path.

### Why not make CFG the shared IR?

CFG is too high-level — it has `MakeAdt`, `MakeClosure`, `PatternMatch` nodes that each backend must interpret differently. Sharing CFG would still leave parity bugs in how ADTs are laid out in memory. LIR forces these decisions to be made once.

### Why not use LLVM IR as the shared IR?

LLVM IR is too low-level for the VM — the bytecode emitter can't efficiently consume SSA phi nodes and LLVM-specific instructions. LIR is the right middle ground: lower than CFG (explicit memory ops), higher than LLVM IR (no SSA, no platform-specific details).

## Prior art

- **GHC Cmm**: The direct inspiration. Shared by NCG, LLVM, Via-C, JS backends. ~50 instruction types. Handles stack, heap, closures, continuations.
- **OCaml Lambda/Clambda**: OCaml's bytecode and native compilers share a "Lambda" IR that's lower than the typed AST but higher than machine code.
- **Erlang BEAM**: The BEAM instruction set serves as a shared IR — both the interpreter and JIT (since OTP 25) consume the same BEAM instructions.
- **V8 Maglev**: V8's mid-tier compiler uses a shared graph IR consumed by both the interpreter (Ignition) and optimizing compiler (TurboFan/Maglev).

## Future possibilities

- **WebAssembly backend**: A `LIR → Wasm` emitter would get control flow parity for free — same lowering as VM and LLVM.
- **JavaScript backend**: A `LIR → JS` emitter — closures lowered to JS objects, CCall to JS FFI.
- **Interpreter mode**: Instead of compiling LIR to bytecode, interpret LIR blocks directly (like GHCi interpreting STG). Useful for debugging.
- **LIR optimization passes**: Common subexpression elimination, dead store elimination, constant propagation — applied once, benefiting both backends.
- **Typed LIR**: If Proposal 0123 (type classes) lands, LIR can carry type information for unboxing optimizations.
