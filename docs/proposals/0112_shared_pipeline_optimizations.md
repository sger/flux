- Feature Name: Shared Pipeline Optimizations
- Start Date: 2026-03-19
- Status: Partially Implemented — Phase 1 (iterative Core simplifier, `MAX_SIMPLIFIER_ROUNDS = 3` in `src/core/passes/mod.rs`) shipped via Proposal 0155 (2026-04-18). Phase 2 (SSA in CFG) and Phase 3 (Core IR cache) open.
- Proposal PR:
- Flux Issue:

# Proposal 0112: Shared Pipeline Optimizations

## Summary

Optimizations to the shared compilation pipeline (Core IR, CFG IR, frontend) that benefit all three backends (VM, Cranelift JIT, LLVM). These operate above the backend split point and multiply their value across every execution path.

## Motivation

The shared pipeline processes every Flux program regardless of backend:

```
Source → Lexer → Parser → AST → HM Type Inference → Core IR → CFG IR → [backends]
```

Optimizations here improve compilation speed and code quality for VM, JIT, and LLVM equally. The research survey of GHC, OCaml, Koka, Rust, Swift, Go, and Erlang identified several high-value techniques applicable to this shared layer.

## Reference Languages

| Language | Shared IR | Key Technique |
|----------|----------|---------------|
| GHC | Core (System FC) | Iterative simplifier (run Core passes 2-3 times) |
| Koka | Core (System F) | Perceus RC insertion at Core level |
| OCaml | Lambda IR | Fast compilation through minimal passes |
| Rust | MIR (CFG, SSA) | SSA-based analysis and optimization |
| Go | SSA IR | Single IR with generic→machine lowering |
| Zig | ZIR/AIR | Flat-array IR for cache-friendly traversal |
| Erlang | Core Erlang | Stable IR as multi-frontend target |

## Phases

### Phase 1: Iterative Core Simplifier

Currently Core passes run once in sequence. GHC's key insight: running the simplifier **iteratively** (2-3 rounds) discovers optimization opportunities that only appear after previous passes transform the code.

**Example:** After beta reduction exposes a known constructor, case-of-known-constructor can fire. After COKC eliminates a branch, dead let elimination can remove now-unused bindings. A second round of inlining may expose more COKC opportunities.

**Implementation:**
1. Wrap `run_core_passes()` in a fixed-point loop (max 3 iterations)
2. Track whether any pass made a change (return a `changed: bool` flag)
3. Stop early if no pass reports changes
4. Guard with a flag (`-O2` or similar) to avoid extra cost in debug builds

**Inspired by:** GHC Core simplifier (runs iteratively until fixed point), Rust MIR (multiple optimization rounds).

**Expected impact:** 5-15% runtime improvement on programs with nested pattern matching, closures, and effect handlers — where multi-pass optimization unlocks cascading simplifications.

**Complexity:** Low. The passes already exist; this is orchestration.

### Phase 2: SSA Form for CFG IR

Convert the CFG IR (`cfg/mod.rs`) to SSA (Static Single Assignment) form. Currently `IrVar` variables can be reassigned across blocks without phi nodes, limiting the optimizations that CFG passes can perform.

**SSA enables:**
- **Constant propagation** with sparse conditional constant propagation (SCCP)
- **Global value numbering** (GVN) — eliminate redundant computations across blocks
- **Dead store elimination** — identify writes that are never read
- **Better register allocation** — SSA directly maps to register interference graphs
- **Loop-invariant code motion** (LICM) — hoist computations out of loops

**Implementation:**
1. Add phi nodes (`IrInstr::Phi`) to `IrBlock`
2. Convert existing CFG construction (in `core/to_ir/`) to emit SSA form
3. Update `cfg/passes.rs` to operate on SSA
4. Add SSA-specific passes: SCCP, GVN, LICM
5. Add SSA destruction pass before bytecode lowering (bytecode is stack-based, not SSA)

**Inspired by:** Rust MIR (SSA), Go SSA, Swift SIL (SSA), V8 TurboFan (sea-of-nodes).

**Complexity:** High. Fundamental IR redesign. Phase-gated: start with SSA construction, then add passes incrementally.

### Phase 3: Core IR Caching

Cache the typed Core IR (after type inference and Core passes) to disk, not just the final bytecode. When source hasn't changed, skip parsing + type inference + Core passes entirely — load cached Core IR and lower to the target backend.

**Current:** `.fxc` caches only final bytecode. Changing `--jit` vs VM requires full recompilation.

**Proposed:**
1. After Core passes, serialize Core IR to `.fxcore` files (content-addressed by source hash)
2. On subsequent runs, if source hash matches, load `.fxcore` and lower directly to backend
3. This benefits all backends — VM, JIT, and LLVM all start from cached Core IR
4. Backend-specific bytecode/native caches remain as a second layer

**Inspired by:** Dart (Kernel binary format), Scala (TASTy), Erlang (Core Erlang as stable target), Unison (content-addressed code).

**Expected impact:** 50-80% compilation time reduction for unchanged modules (skipping parse + type inference + Core passes, which are the most expensive phases).

**Complexity:** Medium. Requires Core IR serialization/deserialization. Content-addressed hashing already exists for `.fxc`.

### Phase 4: Flat-Array IR Representation

Replace tree-based `CoreExpr` (heap-allocated, pointer-chasing) with a flat-array representation where expressions are stored contiguously in a `Vec<CoreNode>` indexed by `NodeId(u32)`.

**Current:**
```rust
enum CoreExpr {
    App { func: Box<CoreExpr>, args: Vec<CoreExpr>, .. },
    Let { binder: .., rhs: Box<CoreExpr>, body: Box<CoreExpr>, .. },
    // ... 12 variants, each with Box/Vec children
}
```

**Proposed:**
```rust
struct CoreArena {
    nodes: Vec<CoreNode>,  // contiguous, cache-friendly
}

enum CoreNode {
    App { func: NodeId, args: SmallVec<NodeId>, .. },
    Let { binder: .., rhs: NodeId, body: NodeId, .. },
    // ... children are indices, not pointers
}
```

**Benefits:**
- Cache-friendly traversal (linear memory, no pointer chasing)
- Cheap cloning (copy indices, not trees)
- Natural serialization (for Phase 3 caching)
- Faster allocation (arena bump-allocate, no per-node Box allocation)

**Inspired by:** Zig ZIR/AIR (flat-array encoding), ECS architectures, Rust's `la-arena` crate pattern.

**Complexity:** High. Requires rewriting Core IR construction and all passes to use arena indices instead of Box/recursive types. Consider doing this for CFG IR first (smaller surface area) as a proving ground.

### Phase 5: Demand Analysis (Usage-Based Optimization)

Analyze which function results and ADT fields are actually demanded (used) by callers:

1. **Dead result elimination:** If a function's return value is always ignored, eliminate the computation
2. **Field demand:** If only one field of an ADT is ever accessed, avoid constructing the full ADT
3. **Absence analysis:** If a closure captures a variable but never uses it, eliminate the capture

**Inspired by:** GHC (demand analysis / strictness analysis), Idris 2 (QTT erasure).

**Implementation:** Backward analysis on Core IR, propagating demand information from use sites to definitions. Since Flux is strict, this is simpler than GHC's version (no thunks to worry about).

**Complexity:** Medium. Backward dataflow analysis on Core IR.

## Relationship to Existing Proposals

| This Phase | Related Proposal | Relationship |
|------------|-----------------|-------------|
| Phase 1 (iterative simplifier) | 0102 (Core IR Optimization Roadmap) | Extends 0102 with iterative execution |
| Phase 2 (SSA) | 0086 (Backend-Neutral Core IR) | Evolves the CFG IR introduced by 0086 |
| Phase 3 (Core caching) | 0033 (JIT Cache Compatibility) | Extends caching above bytecode level |
| Phase 4 (flat-array) | — | New infrastructure |
| Phase 5 (demand analysis) | 0068 (Perceus Uniqueness) | Complements: demand finds unused; Perceus finds unique |

## Dependencies

- Phase 1: standalone (orchestration over existing passes)
- Phase 2: standalone but large scope
- Phase 3: benefits from Phase 4 (flat-array makes serialization natural)
- Phase 4: standalone but large scope
- Phase 5: benefits from Phase 1 (iterative passes expose more demand info)

## Drawbacks

- Iterative passes increase compilation time (mitigated by `-O` flag gating)
- SSA conversion is a major refactor touching all CFG passes
- Core IR caching adds a serialization format that must be maintained across versions
- Flat-array IR is a fundamental data structure change

## Verification

```bash
cargo test --all --all-features
cargo clippy --all-targets --all-features -- -D warnings
# Verify all 3 backends produce same results:
cargo run -- parity-check examples/basics
cargo run -- parity-check examples/advanced
# Benchmark compilation speed:
cargo run -- --stats examples/perf/binarytrees.flx    # Before/after
```
