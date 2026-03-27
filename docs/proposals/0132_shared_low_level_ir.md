- Feature Name: Shared Low-Level IR — Deprecate CFG
- Start Date: 2026-03-27
- Proposal PR:
- Flux Issue:
- Depends on: Proposal 0131 (Aether RC), Proposal 0133 (Unified CorePrimOp)

## Summary

Replace Flux's divergent backend lowering (Core → CFG for VM, Core → LLVM IR for native) with a shared low-level IR (LIR) consumed by both backends. After this proposal, ADT layout, closure construction, pattern matching, and Aether dup/drop are lowered **once** in Core → LIR. The CFG module (`src/cfg/`) is deleted.

This follows GHC's Cmm architecture: STG lowers to Cmm once, then all backends consume the same Cmm.

## Motivation

### Why this is possible now

After Proposals 0131 and 0133, both backends already share:

| Shared | How |
|--------|-----|
| Memory layout | Aether RC — `flux_gc_alloc`, refcount at `ptr - 8` (Proposal 0131) |
| Primop enum | `CorePrimOp` — single type, both backends (Proposal 0133) |
| Primop implementation | C runtime — `flux_upper`, `flux_hamt_get`, etc. |
| Value representation | NaN-boxed `i64` |

The **only remaining difference** is control flow representation:

```
VM path:     Core → CFG IR (src/cfg/) → Bytecode → VM
LLVM path:   Core → LLVM IR (src/core_to_llvm/) → Native binary
```

Two separate lowering passes that each handle ADTs, closures, pattern matching, and Aether annotations independently. Each is a source of parity bugs.

### What diverges today

| Concern | VM path (CFG) | LLVM path | Same? |
|---------|--------------|-----------|-------|
| ADT construction | `IrExpr::MakeAdt` | `emit_make_adt` | No |
| Pattern matching | `IrTerminator::Switch` | `emit_case_expr` | No |
| Closure creation | `IrExpr::MakeClosure` | `emit_make_closure` | No |
| Closure calls | `IrCallTarget::Dynamic` | `emit_closure_call` | No |
| Aether dup/drop | Applied in Core → CFG | Applied in Core → LLVM | No |
| Tail calls | `IrCallTarget::SelfTail` | `emit_tail_call` | No |
| MemberAccess | `IrExpr::MemberAccess` | module_members lookup | No |

### What GHC's Cmm solves

All concerns lowered **once** in `StgToCmm → Cmm`. Then NCG, LLVM, Via-C are thin instruction selectors from the same Cmm.

---

## Design

### LIR overview

The LIR is a flat, NaN-box-aware CFG with explicit memory operations. It sits between Core IR (functional, high-level) and machine code.

```
Core IR (functional)
  │
  └── Core → LIR lowering (single pass)
        │
        ├── LIR → Bytecode emitter (VM)
        └── LIR → LLVM IR emitter (native)
```

### LIR instructions

```rust
pub enum LirInstr {
    // ── Memory ──
    Load { dst: LirVar, ptr: LirVar, offset: i32 },
    Store { ptr: LirVar, offset: i32, val: LirVar },
    Alloc { dst: LirVar, size: u32 },

    // ── NaN-box ──
    TagInt { dst: LirVar, raw: LirVar },
    UntagInt { dst: LirVar, val: LirVar },
    GetTag { dst: LirVar, val: LirVar },
    TagPtr { dst: LirVar, ptr: LirVar },
    UntagPtr { dst: LirVar, val: LirVar },

    // ── Inline arithmetic (no C call) ──
    IAdd { dst: LirVar, a: LirVar, b: LirVar },
    ISub { dst: LirVar, a: LirVar, b: LirVar },
    IMul { dst: LirVar, a: LirVar, b: LirVar },
    ICmp { dst: LirVar, op: CmpOp, a: LirVar, b: LirVar },

    // ── C runtime calls (CorePrimOp) ──
    PrimCall { dst: Option<LirVar>, op: CorePrimOp, args: Vec<LirVar> },

    // ── Aether RC ──
    Dup { val: LirVar },
    Drop { val: LirVar },
    IsUnique { dst: LirVar, val: LirVar },

    // ── Variables ──
    Copy { dst: LirVar, src: LirVar },
    Const { dst: LirVar, value: LirConst },
}

pub enum LirTerminator {
    Return(LirVar),
    Jump(BlockId),
    Branch { cond: LirVar, then_block: BlockId, else_block: BlockId },
    Switch { scrutinee: LirVar, cases: Vec<(i32, BlockId)>, default: BlockId },
    TailCall { func: LirVar, args: Vec<LirVar> },
    Call { dst: LirVar, func: LirVar, args: Vec<LirVar>, cont: BlockId },
    Unreachable,
}
```

Key: `PrimCall` uses `CorePrimOp` directly — the unified primop enum from Proposal 0133. Both bytecode and LLVM emitters resolve it to the same C function.

### How Core constructs lower to LIR

**ADT construction:**
```
Core:  MakeAdt("Some", [value])
LIR:   %ptr = Alloc(16)           // ctor_tag + field
       Store(%ptr, 0, SOME_TAG)   // i32 constructor tag
       Store(%ptr, 4, 1)          // i32 field count
       Store(%ptr, 8, %value)     // field 0
       %result = TagPtr(%ptr)
```

**Pattern matching:**
```
Core:  case x of { Some(v) -> e1; None -> e2 }
LIR:   %tag = GetTag(%x)
       Switch(%tag) { BOXED -> check_ctor:, NONE -> none_block: }
       check_ctor:
         %ptr = UntagPtr(%x)
         %ctor = Load(%ptr, 0)
         Branch(%ctor == SOME_TAG, some_block, none_block)
       some_block:
         %v = Load(%ptr, 8)
         ...e1...
       none_block:
         ...e2...
```

**Closure creation:**
```
Core:  \x -> body  (captures a, b)
LIR:   %closure = PrimCall(MakeClosure, [%fn_ptr, %a, %b])
```

**Primop call:**
```
Core:  PrimOp(Upper, [s])
LIR:   %result = PrimCall(Upper, [%s])
```

### How backends consume LIR

**Bytecode emitter:**

| LIR | Bytecode |
|-----|----------|
| `PrimCall(Upper, [%s])` | `OpPrimOp(CorePrimOp::Upper, 1)` |
| `Branch(%c, then, else)` | `OpJumpNotTruthy(else_addr)` |
| `Switch(%tag, cases)` | Series of compare-and-jump |
| `Dup(%x)` | Clone slot (Rc increment or flux_dup) |
| `Drop(%x)` | `OpAetherDropLocal` |
| `Return(%v)` | `OpReturnValue` |

**LLVM IR emitter:**

| LIR | LLVM IR |
|-----|---------|
| `PrimCall(Upper, [%s])` | `call i64 @flux_upper(i64 %s)` |
| `Branch(%c, then, else)` | `br i1 %c, label %then, label %else` |
| `Switch(%tag, cases)` | `switch i32 %tag, label %default [...]` |
| `Dup(%x)` | `call void @flux_dup(i64 %x)` |
| `Drop(%x)` | `call void @flux_drop(i64 %x)` |
| `Return(%v)` | `ret i64 %v` |

---

## Migration strategy

Incremental — LIR is additive until the final step:

1. **Define LIR data structures** (`src/lir/mod.rs`)
2. **Core → LIR lowering** for arithmetic, let bindings, function definitions
3. **Add ADT construction and pattern matching** to LIR
4. **Add closures and calls** to LIR
5. **Add Aether dup/drop** to LIR
6. **Add effect handlers** (VM-only, since native backend doesn't support them)
7. **Delete CFG** (`src/cfg/`), delete LLVM codegen's Core lowering. Both backends consume LIR exclusively.

At each step, existing backends continue working.

---

## What gets deleted

| Module | Lines | Replaced by |
|--------|-------|-------------|
| `src/cfg/` | ~2,000 | LIR + bytecode emitter |
| `src/core_to_llvm/codegen/expr.rs` (Core lowering) | ~1,500 | LIR + LLVM emitter |
| `src/core/to_ir/` (Core → CFG) | ~800 | Core → LIR |

**Total deleted:** ~4,300 lines
**Total added:** ~2,000 lines (LIR definition + Core → LIR + two thin emitters)

**Net reduction:** ~2,300 lines

---

## Architecture after all three proposals

```
Source → Lexer → Parser → AST → Type Inference → Core IR
                                                    │
                                                    ├── CorePrimOp (unified, Proposal 0133)
                                                    ├── Aether dup/drop annotations
                                                    │
                                                Core → LIR (single lowering pass)
                                                    │
                                          ┌─────────┴─────────┐
                                          │                    │
                                   LIR → Bytecode        LIR → LLVM IR
                                          │                    │
                                        VM                Native binary
                                          │                    │
                                          └─────────┬─────────┘
                                                    │
                                            C Runtime (single)
                                            Aether RC (Proposal 0131)
```

**Everything shared:** primops, values, memory layout, IR, C runtime.
**Only the emitters differ:** bytecode instructions vs LLVM instructions.

---

## Drawbacks

- **Large refactoring** — Core → LIR lowering rewrites both Core → CFG and Core → LLVM as a single pass. Biggest compiler change since the LLVM backend.
- **Effect handlers** — VM-only feature. LIR needs effect instructions that only the bytecode emitter handles, or effects must be CPS-transformed before LIR.
- **Extra IR level** — adds a compilation stage. Mitigated by: LIR is simpler than CFG (fewer node types).

## Prior art

- **GHC Cmm**: Direct inspiration. Shared by NCG, LLVM, Via-C, JS backends.
- **OCaml Lambda**: Shared IR consumed by bytecode and native compilers.
- **Erlang BEAM**: Shared instruction set consumed by interpreter and JIT.
