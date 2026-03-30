- Feature Name: Benchmark Framework — GHC + Koka Hybrid Approach
- Start Date: 2026-03-26
- Proposal PR:
- Flux Issue:
- Depends on: Proposal 0125 (Flow.List and Flow.Array)

## Summary

Add a Flux-level benchmarking library (`Flow.Bench`) and list/array benchmark programs that combine GHC's deterministic allocation counting with Koka's compiler-variant comparison approach. Currently all Flux benchmarking is external (hyperfine) or Rust-level (criterion) — there's no way for a Flux program to time its own operations.

## Motivation

### Current state

Flux has benchmark infrastructure at three levels, but none at the Flux language level:

| Level | Tool | What it measures | Limitation |
|-------|------|-----------------|------------|
| **Script** | `scripts/bench/bench.sh` + hyperfine | Whole-program wall-clock | Can't measure individual operations |
| **Rust** | 11 criterion benches in `benches/*.rs` | Compiler internals (lexer, parser, VM) | Can't benchmark Flux library functions |
| **Flux programs** | 11 files in `benchmarks/flux/*.flx` | Externally timed | No self-timing, no per-operation breakdown |

### What's missing

1. **No `clock_ms()` or timer in Flux** — Koka has `std/time/timer.kk` with `ticks()` and `elapsed()`. GHC has `getCPUTime`. Flux has nothing.

2. **No list/array operation benchmarks** — Flow.List has 30+ functions, Flow.Array has 30+ functions, but no benchmarks measuring their performance. We can't answer: "Is `Flow.List.map` on 100K elements fast enough?"

3. **No allocation regression tests** — GHC asserts `bytes allocated` in CI. Flux has `alloc_stats` counters but no programs that exercise them.

4. **No borrow/reuse impact measurement** — Koka benchmarks the same program with/without TRMC, with/without borrowing. Flux can't measure the impact of Aether optimizations on real workloads.

### Prior art

**GHC** (`testsuite/tests/perf/should_run/`):
- 79 runtime perf tests measuring `bytes allocated` (deterministic)
- Tolerance-based regression detection: `collect_stats('bytes allocated', 5)` — 5% tolerance
- Zero timing noise — allocation counts are reproducible
- Tests validate fusion: `product [1..50]` should allocate near-zero bytes

**Koka** (`test/bench/koka/`):
- `borrowlist.kk` — 1M element list, 100 iterations of linear scan
- `cfold.kk` — recursive fold on expression trees
- Cross-compiler comparison: same program compiled with `kk`, `kkx` (no TRMC), `kkold` (no borrowing)
- Uses `std/time/timer.kk` for in-program timing: `elapsed(\() -> action)`
- Measures median runtime + peak RSS via `/usr/bin/time`

---

## Reference-level explanation

### Phase 1 — `clock_ms()` primop

Add a monotonic clock primop that returns milliseconds as a Float. This is the foundation for all in-Flux timing.

**New CorePrimOp variant:**

```rust
// src/core/mod.rs
ClockMs,  // Returns monotonic time in milliseconds as Float
```

**Primop promotion:**

```rust
// src/core/passes/primop_promote.rs
("clock_ms", 0, CorePrimOp::ClockMs),
```

**VM implementation:**

```rust
// src/primop/mod.rs
PrimOp::ClockMs => {
    use std::time::Instant;
    // Use a static Instant for monotonic baseline
    use std::sync::OnceLock;
    static START: OnceLock<Instant> = OnceLock::new();
    let start = START.get_or_init(Instant::now);
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    Ok(Value::Float(ms))
}
```

Using `Instant` (monotonic) rather than `SystemTime` (wall-clock) — matches Koka's `ticks()` which guarantees monotonically increasing values.

**GHC reference**: `getCPUTime :: IO Integer` returns picoseconds. `getMonotonicTime :: IO Double` returns seconds. Both are RTS primops.

**Koka reference**: `pub fun ticks() : ndet duration` in `lib/std/time/timer.kk` — monotonic, at least millisecond resolution.

**Files**: `src/core/mod.rs`, `src/core/passes/primop_promote.rs`, `src/core/display.rs`, `src/core/passes/helpers.rs`, `src/core/to_ir/primop.rs`, `src/primop/mod.rs`

### Phase 2 — `Flow.Bench` module

A pure Flux benchmarking library. NOT auto-preluded — users import explicitly with `import Flow.Bench as Bench`.

```flux
module Flow.Bench {

    // Core: time a zero-arg function, return (result, elapsed_ms)
    public fn timed(f) with IO {
        let start = clock_ms()
        let result = f()
        let end = clock_ms()
        (result, end - start)
    }

    // Convenience: time and print
    public fn bench(label, f) with IO {
        let pair = timed(f)
        print("[bench] " + label + ": " + to_string(pair.1) + " ms")
        pair.0
    }

    // Run f n times, collect timings into array, report median
    public fn bench_n(label, n, f) with IO {
        fn collect_go(i, times) {
            if i >= n { times }
            else {
                let pair = timed(f)
                collect_go(i + 1, push(times, pair.1))
            }
        }
        let times = sort(collect_go(0, [||]))
        let median = match times[n / 2] {
            Some(v) -> v,
            _ -> 0.0
        }
        let min_v = match times[0] { Some(v) -> v, _ -> 0.0 }
        let max_v = match times[n - 1] { Some(v) -> v, _ -> 0.0 }
        print("[bench] " + label + ": " + to_string(median) + " ms"
            + " (min=" + to_string(min_v) + ", max=" + to_string(max_v)
            + ", n=" + to_string(n) + ")")
        median
    }

    // Compare two implementations
    public fn compare(label_a, f_a, label_b, f_b, n) with IO {
        let a = bench_n(label_a, n, f_a)
        let b = bench_n(label_b, n, f_b)
        let ratio = if b > 0.0 { a / b } else { 0.0 }
        print("[compare] " + label_a + " / " + label_b + " = " + to_string(ratio) + "x")
        (a, b)
    }
}
```

**Design choices:**
- `timed` returns `(result, elapsed_ms)` — the result is preserved so benchmarks can verify correctness
- `bench_n` uses sorted array + median — robust against outliers (same approach as Koka's harness)
- `compare` runs two implementations with the same iteration count and prints the ratio
- All functions require `IO` effect — timing is inherently effectful
- No global state — each call is independent

**Koka reference**: `pub fun elapsed(action) : (duration, a)` in `std/time/timer.kk` — same `(result, time)` return pattern.

**File**: `lib/Flow/Bench.flx` (new file, ~60 lines)

### Phase 3 — List/array operation benchmarks

Two new benchmark programs that exercise Flow.List and Flow.Array:

#### `benchmarks/flux/list_ops.flx`

```flux
import Flow.Bench as Bench
import Flow.Array as Array

fn main() with IO {
    let n = 100000
    let xs = range(1, n)
    let arr = to_array(xs)

    print("=== List operations (n=" + to_string(n) + ") ===")
    Bench.bench("List.map", \() -> map(xs, \x -> x + 1))
    Bench.bench("List.filter", \() -> filter(xs, \x -> x % 2 == 0))
    Bench.bench("List.fold", \() -> fold(xs, 0, \(a, x) -> a + x))
    Bench.bench("List.length", \() -> length(xs))
    Bench.bench("List.reverse", \() -> reverse(xs))
    Bench.bench("List.sort", \() -> sort(range(1, 10000)))

    print("")
    print("=== Array operations (n=" + to_string(n) + ") ===")
    Bench.bench("Array.map", \() -> Array.map(arr, \x -> x + 1))
    Bench.bench("Array.filter", \() -> Array.filter(arr, \x -> x % 2 == 0))
    Bench.bench("Array.fold", \() -> Array.fold(arr, 0, \(a, x) -> a + x))
    Bench.bench("Array.reverse", \() -> Array.reverse(arr))
    Bench.bench("Array.sort", \() -> Array.sort(to_array(range(1, 10000))))

    print("")
    print("=== List vs Array comparison ===")
    Bench.compare(
        "List.map 100K", \() -> map(xs, \x -> x + 1),
        "Array.map 100K", \() -> Array.map(arr, \x -> x + 1),
        5
    )
}
```

#### `benchmarks/flux/borrow_bench.flx`

Koka-style benchmark measuring the impact of Aether borrow optimization:

```flux
import Flow.Bench as Bench

fn lookup(xs, target) {
    match xs {
        [h | t] -> if h == target { true } else { lookup(t, target) },
        _ -> false
    }
}

fn main() with IO {
    let n = 1000000
    let xs = range(1, n)

    print("=== Borrow benchmark (n=" + to_string(n) + ") ===")
    Bench.bench_n("lookup worst-case", 10, \() -> lookup(xs, n))
    Bench.bench_n("lookup best-case", 10, \() -> lookup(xs, 1))
    Bench.bench_n("lookup mid-case", 10, \() -> lookup(xs, n / 2))
}
```

Run with `--dump-aether` to inspect borrow annotations, then compare timing with/without Aether optimizations.

**Koka reference**: `test/bench/koka/borrowlist.kk` — same pattern: create 1M list, linear scan with different search positions, repeat N times, report median.

### Phase 4 — Allocation regression benchmarks (GHC-style) — NOT YET IMPLEMENTABLE

**Prerequisite**: Allocation counting (`alloc_stats` feature) does not exist in the codebase yet. This phase requires adding per-type `AtomicUsize` counters to `src/runtime/leak_detector.rs`, gated behind a `alloc_stats` Cargo feature flag, with instrumentation at all `Rc::new` call sites (cons cells, ADTs, closures, HAMT nodes, arrays, etc.). See the design notes below for the planned approach.

Programs that exercise allocation-heavy operations and report counts via `--stats` + `alloc_stats` feature:

#### `benchmarks/flux/alloc_regression.flx`

```flux
fn main() with IO {
    // Pure computation — should allocate zero heap objects
    let sum = fold(range(1, 10000), 0, \(a, x) -> a + x)
    print("sum: " + to_string(sum))

    // Map — allocates n cons cells
    let mapped = map(range(1, 1000), \x -> x * 2)
    print("mapped length: " + to_string(length(mapped)))

    // HAMT — allocates hamt nodes
    let m = fold(range(1, 100), {}, \(acc, x) -> put(acc, x, x * x))
    print("map size: " + to_string(len(keys(m))))
}
```

Run with: `cargo run --features alloc_stats -- benchmarks/flux/alloc_regression.flx --stats --no-cache`

Expected output includes allocation counts that should stay stable across compiler changes. A CI script can compare these counts against a baseline file:

```bash
#!/bin/bash
# scripts/bench/check_alloc_regression.sh
EXPECTED="benchmarks/baselines/alloc_regression.expected"
ACTUAL=$(cargo run --features alloc_stats -- benchmarks/flux/alloc_regression.flx --stats --no-cache 2>&1 | grep "total allocs")
EXPECTED_COUNT=$(cat "$EXPECTED")
ACTUAL_COUNT=$(echo "$ACTUAL" | grep -oE '[0-9]+')

if [ "$ACTUAL_COUNT" -gt "$((EXPECTED_COUNT * 110 / 100))" ]; then
    echo "REGRESSION: allocations increased from $EXPECTED_COUNT to $ACTUAL_COUNT (>10%)"
    exit 1
fi
echo "OK: $ACTUAL_COUNT allocations (baseline: $EXPECTED_COUNT)"
```

**GHC reference**: `testsuite/tests/perf/should_run/all.T` — each test specifies `collect_stats('bytes allocated', 5)` with a 5% tolerance. The test runner compares against a baseline stored in git notes.

### Phase 5 — Benchmark runner script

Add to `scripts/bench/bench_flow.sh`:

```bash
#!/bin/bash
# Run all Flow library benchmarks
set -e

echo "=== Flow Library Benchmarks ==="
echo ""

cargo build --release

echo "--- List/Array Operations ---"
cargo run --release -- benchmarks/flux/list_ops.flx --no-cache

echo ""
echo "--- Borrow Impact ---"
cargo run --release -- benchmarks/flux/borrow_bench.flx --no-cache

echo ""
echo "--- Allocation Counts ---"
cargo run --release --features alloc_stats -- benchmarks/flux/alloc_regression.flx --stats --no-cache 2>&1 | grep -E "Allocation|closures|cons|arrays|tuples|adts|hamt|total"

echo ""
echo "--- VM vs Native ---"
if cargo build --release --features core_to_llvm 2>/dev/null; then
    echo "VM:"
    cargo run --release -- benchmarks/flux/list_ops.flx --no-cache 2>&1 | grep "\[bench\]"
    echo ""
    echo "Native:"
    cargo run --release --features core_to_llvm -- benchmarks/flux/list_ops.flx --native --no-cache 2>&1 | grep "\[bench\]"
fi
```

---

## Summary of deliverables

| Deliverable | Type | Lines | Status |
|-------------|------|-------|--------|
| `clock_ms()` primop | Compiler change | ~30 lines across 6 files | Phase 1 — ready to implement |
| `lib/Flow/Bench.flx` | New Flux module | ~60 lines | Phase 2 — ready to implement |
| `benchmarks/flux/list_ops.flx` | New benchmark | ~40 lines | Phase 3 — ready to implement |
| `benchmarks/flux/borrow_bench.flx` | New benchmark | ~25 lines | Phase 3 — ready to implement |
| `benchmarks/flux/alloc_regression.flx` | New benchmark | ~20 lines | Phase 4 — blocked on `alloc_stats` feature |
| `alloc_stats` feature + leak_detector extension | Compiler change | ~150 lines | Phase 4 — not yet implemented |
| `scripts/bench/bench_flow.sh` | New script | ~30 lines | Phase 5 — ready to implement |

## Drawbacks

- **`clock_ms()` is impure** — it reads the system clock, which is a side effect. Must require `IO` effect. Not a problem since benchmarking is inherently effectful.

- **Timing granularity** — `Instant::elapsed` on macOS has ~42ns resolution. For sub-millisecond operations, `bench_n` with high iteration count is needed.

- **Allocation counts are feature-gated** — `alloc_stats` must be explicitly enabled. This means CI must build with `--features alloc_stats` for regression checks. Acceptable tradeoff for zero overhead in normal builds.

## Rationale and alternatives

### Why a Flux-level library rather than just external tools?

External tools (hyperfine) can't measure individual operations within a program. A Flux library enables: per-function benchmarks within a single program, A/B comparisons of two implementations, and composable benchmark suites.

### Why not use Rust's criterion for everything?

Criterion measures Rust code, not Flux programs. It can't benchmark Flow.List.map — it can only benchmark the VM executing Flow.List.map, which includes dispatch overhead. A Flux-level timer measures what users care about: how long their Flux code takes.

### Why median rather than mean?

Koka uses median, GHC uses deterministic counts (no timing). Median is robust against GC pauses, OS scheduling jitter, and cold-cache outliers. Mean is sensitive to outliers. All serious benchmark frameworks use median or trimmed mean.

## Prior art

- **GHC**: `bytes allocated` counter (always-on, zero overhead via bump pointer). 79 perf tests with tolerance-based regression detection. No in-language timing library (uses `criterion` Haskell package).
- **Koka**: `std/time/timer.kk` with `ticks()`, `elapsed()`, `print-elapsed()`. `test/bench/bench.kk` harness using `/usr/bin/time` + median. `borrowlist.kk` for RC impact measurement.
- **Lean 4**: `Lean.Profiler` module with `trace` and `heartbeats` (deterministic work metric, similar to GHC's allocation counting).
- **OCaml**: `Benchmark` library with `latency` and `throughput` functions. `Sys.time()` for wall-clock.

## Future possibilities

- **`bench_allocs(label, f)`** — Run function and report allocation count (requires `alloc_stats`). Enables GHC-style in-program allocation assertions.
- **Flamegraph integration** — `Bench.profile(label, f)` that writes a flamegraph-compatible trace.
- **JSON output** — `Bench.bench_json(label, n, f)` for machine-readable results. Enables CI dashboards.
- **Baseline files** — Store expected benchmark results in `benchmarks/baselines/`. CI compares against baselines with tolerance.
- **Cross-backend comparison** — `bench_flow.sh` already scaffolds VM vs Native. Future: add WASM, JS backends.
