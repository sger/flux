# Async & Fibers — beginner walkthrough

These twelve examples accompany [Chapter 20 of the Flux guide](../../docs/guide/20_async_and_fibers.md). Each one is small, self-contained, and focuses on a single concept. Run them in order; later examples assume you've seen the earlier ones.

| # | File | Concept |
|---|------|---------|
| 01 | [`01_first_async.flx`](01_first_async.flx) | The smallest possible `with Async` program: `run_async` boundary, function annotation. |
| 02 | [`02_sleep_and_yield.flx`](02_sleep_and_yield.flx) | What suspension looks like: `sleep` and `yield_now`. |
| 03 | [`03_sequential_vs_concurrent.flx`](03_sequential_vs_concurrent.flx) | Wall-clock proof that concurrent execution beats sequential when work is mostly waiting. |
| 04 | [`04_both_concurrent_pair.flx`](04_both_concurrent_pair.flx) | `both(f, g)` — run two fibers, get both results, in source order. |
| 05 | [`05_race_first_wins.flx`](05_race_first_wins.flx) | `race(f, g)` — first to finish wins; loser is cancelled. |
| 06 | [`06_first_of_mirrors.flx`](06_first_of_mirrors.flx) | `first_of(fs)` — race over a list of candidates. |
| 07 | [`07_timeout_deadline.flx`](07_timeout_deadline.flx) | `timeout(ms, f)` — bound wall-clock time. |
| 08 | [`08_try_catch_failure.flx`](08_try_catch_failure.flx) | `try` / `fail` — errors as values; `result_is_ok` and `result_or` helpers. |
| 09 | [`09_scope_fork_workers.flx`](09_scope_fork_workers.flx) | Structured concurrency with `scope` / `fork` / `cancel`. |
| 10 | [`10_channel_producer_consumer.flx`](10_channel_producer_consumer.flx) | `Flow.Channel` for passing values between fibers. |
| 11 | [`11_task_spawn_parallel.flx`](11_task_spawn_parallel.flx) | `Task.spawn` for real OS-thread parallelism on CPU-bound work. |
| 12 | [`12_real_world_fanout.flx`](12_real_world_fanout.flx) | Putting it together: `try` + `first_of` + `timeout` for a robust mirrored-fetch pattern. |

## Running

Each file is a standalone program; run with the Flux CLI:

```sh
cargo run -- examples/guide_async/01_first_async.flx
cargo run -- examples/guide_async/03_sequential_vs_concurrent.flx
# ...etc
```

Every example carries a `parity-expected-stdout-begin/end` block, so you can verify VM and LLVM agree:

```sh
cargo run --features llvm -- parity-check examples/guide_async --ways vm,llvm
```

## Reading order

The chapter walks through these examples in lockstep. If you'd rather just hack on the code:

1. Start with **01** and **02** — these establish the boundary (`run_async`) and what suspension means.
2. Work through **03–07** in order — each adds one concurrent combinator on top of the previous.
3. **08** introduces failure handling; **09** generalises that into structured cancellation.
4. **10** is communication; **11** is real parallelism. Read these when you have a real use case in mind.
5. **12** stitches several primitives together — read it last.

## See also

- [`docs/guide/20_async_and_fibers.md`](../../docs/guide/20_async_and_fibers.md) — the conceptual chapter.
- [`examples/async/`](../async/) — the original (more technical) async demos. Each primop has its own example with deeper commentary.
- [`docs/internals/async_syntax.md`](../../docs/internals/async_syntax.md) — runtime, scheduler, and primop layer for compiler hackers.
