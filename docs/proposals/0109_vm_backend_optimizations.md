- Feature Name: VM Backend Optimizations
- Start Date: 2026-03-19
- Status: Partially Implemented — Phase 4 (NaN-boxing, `src/runtime/nanbox.rs`) shipped; partial Phase 2 (8 fused superinstructions in `src/bytecode/compiler/mod.rs`). Phases 1 (computed goto) and 3 (lazy compilation) open.
- Proposal PR:
- Flux Issue:

# Proposal 0109: VM Backend Optimizations

## Summary

Performance optimizations specific to the bytecode VM backend (`bytecode/vm/`), informed by techniques used in Lua/LuaJIT, BEAM (Erlang), V8 Sparkplug, Ruby YJIT, and OCaml's ZINC machine.

## Motivation

The VM is Flux's default execution backend — fast startup, zero compilation latency, ideal for development. These optimizations target runtime execution speed without affecting the shared pipeline (Core IR, CFG IR) or other backends.

## Reference Languages

| Language | VM Type | Key Technique |
|----------|---------|---------------|
| Lua/LuaJIT | Register-based | Computed goto dispatch, NaN-boxing, trace JIT |
| BEAM (Erlang) | Register-based | Threaded dispatch, ~170 specialized opcodes |
| V8 Sparkplug | Stack→native | Linear bytecode walk, frame-compatible codegen |
| Ruby YJIT | Stack-based | Lazy basic block versioning |
| OCaml ZINC | Stack-based | GRAB for partial application, flat closures |

## Phases

### Phase 1: Computed Goto Dispatch

Replace the `match` dispatch loop in `bytecode/vm/dispatch.rs` with computed goto (indirect threading) via Rust's `unsafe` block pattern.

**Current:**
```rust
loop {
    match opcode {
        OpCode::OpConstant => { ... }
        OpCode::OpAdd => { ... }
        // ~85 arms
    }
}
```

**Proposed:** Use a jump table indexed by opcode discriminant:
```rust
static DISPATCH_TABLE: [fn(&mut VM); 85] = [...];
// or: computed goto via inline assembly / unsafe pointer dispatch
```

**Expected impact:** 10-30% dispatch overhead reduction. BEAM, LuaJIT, and CPython all use threaded dispatch. The branch predictor benefits from each opcode having its own indirect branch site rather than a single switch.

**Complexity:** Low-medium. Requires `unsafe` but the pattern is well-established. Benchmark before/after with `criterion`.

### Phase 2: Specialized Superinstructions

Identify common opcode sequences and fuse them into single superinstructions:

| Sequence | Superinstruction | Benefit |
|----------|-----------------|---------|
| `OpGetLocal` + `OpCall` | `OpCallLocal` | Skip stack push for local calls |
| `OpConstant(int)` + `OpAdd` | `OpAddImmediate` | Avoid constant pool lookup |
| `OpGetLocal` + `OpGetLocal` + `OpAdd` | `OpAddLocals` | 3 dispatches → 1 |
| `OpJumpNotTrue` + `OpConstant(true)` | `OpJumpIfFalsy` | Common branch pattern |

**Inspired by:** BEAM's ~170 specialized opcodes, V8's bytecode combining.

**Implementation:** Add a peephole pass after bytecode compilation that scans for patterns and replaces with fused opcodes. The VM dispatch table grows but each superinstruction does more work per dispatch.

**Complexity:** Medium. Requires new opcodes, compiler pass, and dispatch handlers.

### Phase 3: Lazy Function Compilation

Defer bytecode compilation of function bodies until first call:

1. During initial compilation, emit a stub for each function that triggers compilation on first call
2. On first call, compile the function body, patch the stub with the real bytecode offset
3. Subsequent calls go directly to compiled code

**Inspired by:** Dart VM (lazy compilation), V8 (lazy parsing + compilation).

**Expected impact:** Faster program startup for large codebases where not all functions are called. Particularly valuable for the `--test` flag where only `test_*` functions execute.

**Complexity:** Medium. Requires stub mechanism and function patching in the VM.

### Phase 4: NaN-Boxing Value Representation

Replace the `Value` enum (25 variants, multi-word) with NaN-boxed 64-bit representation in the VM:

- Doubles: use full IEEE 754 encoding
- Integers: encoded in NaN payload (47 bits for small ints)
- Booleans: two specific NaN bit patterns
- Pointers: heap pointers in NaN payload (47 bits, sufficient for x86-64)

**Inspired by:** LuaJIT, SpiderMonkey, Wren.

**Existing proposal:** 0041 (NaN Boxing Runtime Optimization) — this phase implements that proposal for the VM backend specifically. The `runtime/nanbox.rs` module already exists as a foundation.

**Expected impact:** Halve stack memory usage, eliminate enum dispatch for type checks, improve cache locality.

**Complexity:** High. Touches all of `dispatch.rs`, `function_call.rs`, and every opcode handler.

### Phase 5: Inline Caching for Base Function Dispatch

Add monomorphic inline caches at `OpCallBase` sites:

1. First call: resolve base function, cache the function pointer at the call site
2. Subsequent calls: direct call through cached pointer (no table lookup)

**Inspired by:** V8 inline caches, Wren's monomorphic dispatch, SpiderMonkey CacheIR.

**Complexity:** Medium. Requires mutable bytecode or side-table for IC state.

## Dependencies

- Phase 1 (computed goto): standalone, no dependencies
- Phase 2 (superinstructions): standalone, but benefits from Phase 1
- Phase 3 (lazy compilation): requires changes to `bytecode/compiler/` pipeline
- Phase 4 (NaN-boxing): depends on proposal 0041, may conflict with shared `Value` enum
- Phase 5 (inline caching): standalone

## Drawbacks

- Computed goto requires `unsafe` code
- Superinstructions increase opcode count and maintenance surface
- NaN-boxing reduces `Value` to 64 bits but loses the ability to store large payloads inline
- Lazy compilation adds complexity to the compilation model

## Verification

Each phase should be benchmarked independently:
```bash
cargo bench --bench closure_capture_bench   # Before/after each phase
cargo bench --bench map_filter_fold_bench
scripts/bench_benchmark_flamewatch.sh binarytrees
cargo test --all --all-features             # Correctness preserved
```
