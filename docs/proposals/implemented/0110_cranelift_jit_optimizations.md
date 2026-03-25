- Feature Name: Cranelift JIT Backend Optimizations
- Start Date: 2026-03-19
- Status: Obsolete (Cranelift JIT backend removed)
- Proposal PR:
- Flux Issue:

# Proposal 0110: Cranelift JIT Backend Optimizations

## Summary

Performance optimizations specific to the Cranelift JIT backend (`jit/`), informed by techniques from LuaJIT, V8 TurboFan, Dart VM's optimizing tier, and Swift's SIL ARC optimization.

## Motivation

The JIT backend produces native code for faster execution but has higher compilation latency than the VM. These optimizations target both code quality (faster generated code) and compilation speed (faster JIT compilation), without affecting the shared pipeline.

## Reference Languages

| Language | JIT Type | Key Technique |
|----------|----------|---------------|
| LuaJIT | Trace-based | Allocation sinking, snapshot deoptimization |
| V8 TurboFan | Method-based | Sea-of-nodes IR, type feedback, escape analysis |
| Dart VM | Tiered | Unboxing via type feedback, lazy compilation |
| Swift SIL | AOT | ARC optimization (retain/release elimination) |
| YJIT | Lazy BBV | Lazy basic block versioning, context-driven specialization |

## Phases

### Phase 1: Rc Retain/Release Elimination

Analyze Cranelift IR to identify redundant `Rc::clone()` / `drop()` pairs where a value's lifetime is statically known.

**Current state:** Every value crossing a function boundary or stored in an ADT gets `Rc::clone()`. Many of these are immediately dropped.

**Proposed:** At the JIT IR level, track value ownership flow. When a value is consumed (last use) immediately after being cloned, eliminate the clone/drop pair. This is the Cranelift equivalent of Swift's SIL ARC optimization.

**Inspired by:** Swift SIL (retain/release pairing), Koka Perceus (drop specialization).

**Expected impact:** Reduce Rc reference counting overhead by 20-40% in typical functional code (map/filter/fold chains).

**Complexity:** Medium. Requires lifetime analysis on the JIT's value flow graph.

### Phase 2: Unboxed Arithmetic Widening

Extend `JitValueKind::Int` / `Float` / `Bool` unboxed paths to cover more operations:

1. **Binary operations on known-int pairs:** Currently some paths box before arithmetic. Ensure all `Int op Int → Int` paths stay unboxed end-to-end.
2. **Comparison chains:** `a < b && b < c` should keep values unboxed across the `&&`.
3. **Loop counters:** Recursive functions with integer accumulators should keep the accumulator in a register across tail calls.

**Inspired by:** V8 TurboFan (speculative unboxing), Dart (type-driven unboxing), LuaJIT (trace-based unboxing).

**Complexity:** Medium. Extend the existing `JitValueKind` tracking in `jit/compiler.rs`.

### Phase 3: Escape Analysis and Allocation Sinking

Analyze ADT allocations in the JIT to determine if they escape the current function:

1. **Non-escaping ADTs:** If an ADT is constructed, pattern-matched, and never returned or stored, skip the heap allocation entirely — keep fields in registers.
2. **Allocation sinking:** If an ADT only escapes on one branch of a conditional, defer allocation to that branch.

**Inspired by:** LuaJIT (allocation sinking on traces), V8 TurboFan (escape analysis), GHC (worker/wrapper).

**Example:**
```flux
fn magnitude(point) {
    match point {
        Point(x, y) -> sqrt(x * x + y * y)
    }
}
// If Point is constructed just before this call, the allocation can be eliminated
```

**Expected impact:** Major win for code that constructs temporary ADTs (common in functional style).

**Complexity:** High. Requires interprocedural analysis or inlining + local escape analysis.

### Phase 4: Background JIT Compilation

Move JIT compilation to a background thread:

1. Start execution in the VM (bytecode)
2. Trigger JIT compilation of hot functions on a background thread
3. When JIT compilation completes, patch call sites to use native code
4. Subsequent calls use JIT-compiled code

**Inspired by:** V8 (concurrent TurboFan/Maglev), SpiderMonkey (concurrent IonMonkey), Dart (background optimization).

**Implementation:** Requires a tiered execution model — VM runs first, JIT replaces hot paths. The existing `--jit` flag would become the default rather than an either/or choice.

**Complexity:** High. Requires thread-safe compilation, code patching, and potentially on-stack replacement (OSR).

### Phase 5: Speculative Optimization with Deoptimization

Use runtime type feedback to speculate about value types in JIT-compiled code:

1. Add counters/type profiling in the VM (phase 4 prerequisite)
2. JIT compiles code assuming observed types (e.g., "this argument is always Int")
3. Insert guards that check the assumption
4. On guard failure, deoptimize back to VM execution

**Inspired by:** V8 (type feedback → speculative optimization → deoptimization), YJIT (lazy BBV with type contexts), HotSpot (uncommon traps).

**Complexity:** Very high. Requires VM profiling infrastructure, guard insertion, and deoptimization support.

## Dependencies

- Phase 1 (Rc elimination): standalone — operates on existing JIT value flow
- Phase 2 (unboxed widening): standalone — extends existing JitValueKind
- Phase 3 (escape analysis): benefits from inlining support in JIT
- Phase 4 (background JIT): requires VM + JIT interop infrastructure
- Phase 5 (speculative opt): requires Phase 4 (tiered execution)
- Proposals 0068-0070 (Perceus): Phase 1 is the JIT counterpart of Perceus

## Drawbacks

- Escape analysis adds compilation latency (acceptable if JIT runs in background)
- Background JIT adds significant complexity (threading, patching, OSR)
- Speculative optimization requires deoptimization support — increases runtime complexity

## Verification

```bash
cargo test --all --all-features
scripts/release/check_parity.sh examples/basics examples/advanced  # VM/JIT parity
cargo bench --bench closure_capture_bench
cargo bench --bench map_filter_fold_bench
```
