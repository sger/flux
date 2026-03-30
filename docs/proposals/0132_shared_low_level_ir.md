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

## Implementation phases

Incremental — LIR is additive until the final phase.  Each phase produces a working compiler.  Existing backends continue operating through the old paths until Phase 7 removes them.

### Phase 1: LIR data structures ✅

**Goal:** Define the IR types.  No lowering, no backends — just the data model.

**Scope:**
- `src/lir/mod.rs` — `LirVar`, `LirConst`, `CmpOp`, `LirInstr`, `LirTerminator`, `LirBlock`, `LirFunction`, `LirProgram`
- Register `pub mod lir` in `lib.rs`
- Display impls for debugging

**Verification:** `cargo build` compiles.  No functional changes.

---

### Phase 2: Core → LIR lowering — scalars and let bindings

**Goal:** Lower the simplest Core expressions to LIR: literals, variables, let bindings, arithmetic, comparisons, and function definitions (no closures yet — only top-level non-capturing functions).

**Scope:**
- `src/lir/lower.rs` — `lower_program(&CoreProgram) -> LirProgram`
- Handle: `CoreExpr::Lit`, `CoreExpr::Var`, `CoreExpr::Let`, `CoreExpr::LetRec`
- Handle: `CoreExpr::PrimOp` for typed arithmetic (`IAdd`, `ISub`, …) → `LirInstr::IAdd` etc.
- Handle: `CoreExpr::PrimOp` for promoted primops → `LirInstr::PrimCall`
- Handle: `CoreExpr::PrimOp` for generic arithmetic (`Add`, `Sub`, …) → `LirInstr::PrimCall` (runtime dispatch)
- Handle: `CoreExpr::Lam` for top-level functions (no free variables) → `LirFunction`
- NaN-box tag/untag insertion for typed arithmetic paths
- `src/lir/display.rs` — human-readable LIR dump (for `--dump-lir` flag)

**Verification:** Dump LIR for `examples/basics/fibonacci.flx` and verify structure manually.  Existing backends still used for execution.

---

### Phase 3: Pattern matching and ADTs

**Goal:** Lower `CoreExpr::Case` and `CoreExpr::Con` to LIR blocks with switches and memory operations.

**Scope:**
- `Case` on literals → `LirTerminator::Switch` or `LirTerminator::Branch`
- `Case` on ADT constructors → `GetTag` + `Switch` + `Load` for field extraction
- `Case` on cons lists → `GetTag` for `None`/`EmptyList`/`Cons` discrimination
- `Con` (ADT construction) → `Alloc` + `Store` fields + `TagPtr`
- `MakeList`, `MakeArray`, `MakeTuple`, `MakeHash` → `PrimCall` or inline alloc sequences
- `CoreExpr::MemberAccess`, `CoreExpr::TupleField` → `Load` with known offsets
- Guards in case alternatives

**Verification:** `examples/basics/pattern_matching.flx` lowers to correct LIR.

---

### Phase 4: Closures and function calls

**Goal:** Lower lambda expressions with free variables, function application, and tail calls.

**Scope:**
- `CoreExpr::Lam` with captures → `Alloc` closure struct + store `fn_ptr` + captured vars
- `CoreExpr::App` → `LirTerminator::Call` (extract fn_ptr from closure, pass captures + args)
- Tail call detection → `LirTerminator::TailCall`
- `CoreExpr::AetherCall` → same as `App` but with borrow mode metadata
- Self-recursive tail calls → `LirTerminator::Jump` back to entry block

**Verification:** `examples/basics/higher_order.flx` lowers correctly.

---

### Phase 5: Aether dup/drop/reuse

**Goal:** Lower Aether annotations in Core IR to explicit LIR RC instructions.

**Scope:**
- `CoreExpr::Dup` → `LirInstr::Dup`
- `CoreExpr::Drop` → `LirInstr::Drop`
- `CoreExpr::DropSpecialized` → `LirInstr::IsUnique` + `LirTerminator::Branch` (unique vs shared paths)
- `CoreExpr::Reuse` → `LirInstr::DropReuse` + conditional `Alloc` or reuse

**Verification:** `--dump-lir` on Aether-annotated programs shows dup/drop/reuse instructions.  Compare against `--dump-core` Aether stats.

---

### Phase 6: Bytecode emitter (LIR → VM)

**Goal:** Emit bytecode from LIR instead of from CFG.  Run the compiler with `FLUX_USE_LIR=1` to opt in.

**Scope:**
- `src/lir/emit_bytecode.rs` — walk `LirProgram`, emit opcodes
- `LirInstr::PrimCall` → `OpPrimOp`
- `LirTerminator::Branch` → `OpJumpNotTruthy` / `OpJumpIfFalse`
- `LirTerminator::Switch` → series of compare-and-jump
- `LirTerminator::Call` → `OpCall` / `OpTailCall`
- `LirInstr::Dup` → clone slot, `LirInstr::Drop` → `OpAetherDropLocal`
- `LirTerminator::Return` → `OpReturnValue`
- Feature flag `FLUX_USE_LIR` in bytecode compiler pipeline to switch paths

**Verification:** `scripts/run_examples.sh --all` produces identical output with `FLUX_USE_LIR=1`.  Parity with existing CFG path.

---

### Phase 7: LLVM emitter (LIR → LLVM IR)

**Goal:** Emit LLVM IR from LIR instead of from Core directly.

**Scope:**
- `src/lir/emit_llvm.rs` — walk `LirProgram`, emit LLVM IR text
- `LirInstr::PrimCall` → `call i64 @flux_<name>(i64 ...)`
- `LirTerminator::Branch` → `br i1`
- `LirTerminator::Switch` → `switch i32`
- `LirInstr::Dup`/`Drop` → `call void @flux_dup`/`@flux_drop`
- Memory instructions → direct LLVM `load`/`store`/`call @malloc`

**Verification:** `scripts/check_core_to_llvm_parity.sh` passes with LIR-based LLVM emitter.

---

### Phase 8: Delete old paths

**Goal:** Remove the old Core-to-LLVM direct lowering and eventually the CFG IR so both backends consume LIR exclusively.

**Phase 8a — Old Core→LLVM codegen (DONE):**
- ✅ Deleted `src/core_to_llvm/codegen/expr.rs` (3,571 lines)
- ✅ Deleted `src/core_to_llvm/codegen/function.rs` (1,804 lines)
- ✅ Deleted `src/core_to_llvm/codegen/aether.rs` (324 lines)
- ✅ Deleted old test files (`core_to_llvm_adt.rs`, `core_to_llvm_closures.rs`, `core_to_llvm_codegen.rs`)
- ✅ Moved `CoreToLlvmError` + `display_ident` to `codegen/mod.rs`
- ✅ Cleaned dead code in `closure.rs` and `adt.rs` (~150 lines)
- ✅ Updated snapshot test to use LIR→LLVM path
- **Total removed:** ~5,700 lines of old codegen + ~150 lines dead code

**Phase 8b — CFG IR removal (TODO):**

The CFG path (`src/cfg/`, `src/core/to_ir/`, `src/bytecode/compiler/cfg_bytecode.rs`) is still used by the default bytecode pipeline for prelude/module compilation. The LIR path depends on a globals table populated by CFG-compiled prelude modules.

To eliminate CFG, adopt GHC's approach — separate compilation with deferred symbol resolution:

1. **Module interface metadata:** Each compiled module produces export metadata (function names, arities, indices) — like GHC's `.hi` files. Downstream modules read this instead of requiring a pre-populated globals table.
2. **Symbolic references in LIR:** Cross-module calls emit `SymbolRef("Flow.List.map")` instead of resolved `GetGlobal(idx)`. Resolution happens at link time.
3. **Two-pass pipeline:**
   - Pass 1: Compile all modules (prelude + user) through LIR → bytecode with symbolic refs
   - Pass 2: Link — resolve symbolic refs to actual global indices
4. **Remove `FLUX_USE_LIR` / `--run-lir` flag** — LIR becomes the only path

**Scope (Phase 8b):**
- Add module interface / export metadata to LIR compilation output
- Add symbolic reference support to LIR lowerer (replace `globals_map` dependency)
- Add link phase for bytecode (resolve symbolic refs → global indices)
- Delete `src/cfg/` (~2,000 lines)
- Delete `src/core/to_ir/` (~800 lines)
- Delete `src/bytecode/compiler/cfg_bytecode.rs`
- Remove `--run-lir` flag and `FLUX_USE_LIR` env var
- Update CLAUDE.md architecture diagram

**Verification:** Full test suite passes. No references to `cfg::`, `IrExpr`, or `IrProgram` remain in production code.

---

### Phase 9: Effect handlers in LIR

**Goal:** Lower effect handlers to LIR (VM-only initially, native backend can be added later via CPS or setjmp/longjmp).

**Scope:**
- `CoreExpr::Perform` → LIR perform instruction (new `LirInstr` variant)
- `CoreExpr::Handle` → LIR handler scope setup/teardown
- VM bytecode emitter handles these; LLVM emitter returns `unsupported` error

**Verification:** Effect handler examples (`examples/effects/`) work on VM.

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
