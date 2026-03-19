- Feature Name: LLVM Backend Optimizations
- Start Date: 2026-03-19
- Status: Not Implemented
- Proposal PR:
- Flux Issue:

# Proposal 0111: LLVM Backend Optimizations

## Summary

Optimization strategies specific to the LLVM backend (`llvm/`), informed by techniques from Rust (MIR→LLVM), Swift (SIL→LLVM), Zig (AIR→LLVM), Koka (Core→C), and Julia (typed IR→LLVM). Focuses on generating better LLVM IR so LLVM's optimization passes produce faster native code.

## Motivation

The LLVM backend targets release builds where execution speed matters most. LLVM provides powerful optimizations (inlining, vectorization, loop optimization) but only if the input IR is well-structured. Poorly generated LLVM IR leaves performance on the table. These optimizations ensure Flux feeds LLVM IR that it can optimize effectively.

## Reference Languages

| Language | LLVM Usage | Key Technique |
|----------|-----------|---------------|
| Rust | MIR → LLVM IR | Monomorphization before LLVM, MIR-level optimizations |
| Swift | SIL → LLVM IR | ARC optimization on SIL, generic specialization before LLVM |
| Zig | AIR → LLVM IR | Fast debug backend skipping LLVM, LLVM only for release |
| Koka | Core → C | Perceus RC in generated C, TRMC optimization |
| Julia | Typed IR → LLVM | Aggressive type specialization before LLVM |

## Phases

### Phase 1: Tagged Value Optimization

The current `{i64, i64}` tagged value representation (tag + payload) prevents LLVM from reasoning about value types. Improve this:

1. **Type-annotated LLVM IR:** When the CFG IR carries type information (`IrType::Int`, `IrType::Float`, `IrType::Bool`), emit LLVM IR with native types (`i64`, `double`, `i1`) instead of tagged values. Fall back to tagged values only for polymorphic/unknown types.
2. **Tag check elimination:** When consecutive operations on the same value all expect the same type, emit one tag check and use the untagged value for subsequent operations.
3. **SROA-friendly structs:** Ensure tagged value structs are emitted in a way that LLVM's Scalar Replacement of Aggregates (SROA) can decompose them into individual registers.

**Inspired by:** Julia (type-specialized LLVM IR), Swift SIL (type-driven lowering).

**Expected impact:** LLVM can apply arithmetic optimizations, loop vectorization, and constant folding when it sees native types instead of opaque `{i64, i64}` structs.

**Complexity:** Medium. Requires threading IrType information through `llvm/compiler/expressions.rs`.

### Phase 2: Function Specialization for Hot Types

Generate type-specialized clones of polymorphic functions for commonly used type combinations:

1. Analyze call sites where concrete types are known from CFG IR type information
2. Clone the function with specialized types (e.g., `map<Int>`, `map<Float>`)
3. In specialized versions, eliminate tag checks and use native LLVM types
4. Keep the generic version as fallback

**Inspired by:** Rust (monomorphization), Julia (aggressive specialization), Swift (generic specialization on SIL).

**Complexity:** Medium-high. Requires specialization pass on CFG IR before LLVM lowering.

### Phase 3: Rc/Reference Counting LLVM Intrinsics

Help LLVM optimize reference counting by using LLVM's optimization-friendly patterns:

1. **Mark Rc operations as `nounwind`** — Rc::clone and drop never panic, allowing LLVM to optimize around them
2. **Use `llvm.objc.retain` / `llvm.objc.release` semantics** — LLVM has built-in understanding of retain/release patterns (from Objective-C ARC) and can eliminate redundant pairs
3. **Annotate pure functions as `readnone`/`readonly`** — Allow LLVM to move/eliminate Rc operations around pure code

**Inspired by:** Swift (LLVM ARC optimization via intrinsics), Koka Perceus (compile-time RC optimization).

**Complexity:** Low-medium. Mostly annotation work on existing LLVM IR emission.

### Phase 4: Effect Handler Lowering

Optimize algebraic effect handler compilation for LLVM:

1. **Tail-resumptive handlers as direct calls:** When evidence-passing has already eliminated continuation capture (from Core `evidence_pass`), ensure the LLVM IR emits a direct call rather than going through the handler stack.
2. **Handler stack allocation:** For handlers with known lifetime (not captured as continuations), allocate handler frames on the LLVM stack rather than the heap.
3. **Continuation as setjmp/longjmp:** For multi-shot continuations, use LLVM's `@llvm.eh.sjlj` intrinsics or split-stack approach rather than copying frames.

**Inspired by:** Koka (evidence-passing to C), GHC (continuation optimization in Cmm).

**Complexity:** High. Effect handler lowering is the most complex part of any backend.

### Phase 5: AOT Compilation Pipeline

Complete the ahead-of-time compilation pipeline for standalone binary output:

1. **Whole-program optimization:** With full program visibility, LLVM can perform interprocedural optimization (IPO), link-time optimization (LTO), and dead function elimination
2. **Static linking of runtime:** Bundle `runtime/base/` functions and `rt_*` helpers as LLVM IR modules, enabling cross-module inlining
3. **Object file emission:** `llvm_emit_object` already exists — extend with proper linking and executable generation

**Inspired by:** Zig (LLVM for release, custom for debug), Rust (LTO), Go (static binaries).

**Complexity:** High. Requires linker integration and runtime bundling.

## Dependencies

- Phase 1 (tagged value): requires IrType propagation through CFG IR (partially exists)
- Phase 2 (specialization): requires Phase 1 + specialization pass on CFG IR
- Phase 3 (Rc intrinsics): standalone — annotation work on existing emission
- Phase 4 (effect handlers): benefits from Core evidence_pass (already implemented)
- Phase 5 (AOT): requires Phase 1-4 for optimal output
- Proposal 0105 (LLVM Native Backend): this proposal extends 0105 with optimization strategies

## Drawbacks

- Type specialization increases code size (mitigated by only specializing hot functions)
- LLVM intrinsic reliance may break across LLVM versions
- AOT compilation removes the flexibility of dynamic loading

## Verification

```bash
cargo test --all --all-features
scripts/release/check_parity.sh examples/basics examples/advanced  # VM/LLVM parity
# Benchmark specific programs with --llvm vs --jit vs VM:
cargo run --features llvm -- --llvm --stats examples/perf/binarytrees.flx
cargo run --features jit -- --jit --stats examples/perf/binarytrees.flx
cargo run -- --stats examples/perf/binarytrees.flx
```
