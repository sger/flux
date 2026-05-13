# Async examples

Each file is a runnable Flux program that demonstrates one slice of the
`Flow.Async` / `Flow.Task` surface introduced by proposal 0174.

Run any of them with the CLI:

```sh
cargo run -- examples/async/01_run_async.flx                 # VM
cargo run --features llvm -- examples/async/01_run_async.flx # native
```

| File | Concept |
|---|---|
| [01_run_async.flx](01_run_async.flx) | `run_async` — entering the async world |
| [02_sleep_yield.flx](02_sleep_yield.flx) | `sleep`, `yield_now` |
| [03_both.flx](03_both.flx) | `both` — concurrent pair, returns `(a, b)` |
| [04_race.flx](04_race.flx) | `race` — first finisher wins, loser cancelled |
| [05_first_of.flx](05_first_of.flx) | `first_of` / `first` — N-way race |
| [06_timeout.flx](06_timeout.flx) | `timeout`, `timeout_result` |
| [07_try_fail.flx](07_try_fail.flx) | `try`, `fail`, `AsyncError` variants |
| [08_finally_bracket.flx](08_finally_bracket.flx) | `finally`, `bracket` cleanup arms |
| [09_scope_fork_cancel.flx](09_scope_fork_cancel.flx) | `scope`, `fork`, `cancel` |
| [10_check_cancelled.flx](10_check_cancelled.flx) | `check_cancelled`, `bail_if_cancelled` |
| [11_runtime_config.flx](11_runtime_config.flx) | `RuntimeConfig`, `run_async_with`, `run_async_with_workers` |
| [12_task_spawn_join.flx](12_task_spawn_join.flx) | `Task.spawn`, `blocking_join`, `Sendable` |
| [13_task_await.flx](13_task_await.flx) | `Task.await` from inside a fiber |
| [14_task_cancel.flx](14_task_cancel.flx) | `Task.cancel` and cancel-then-await |
| [15_concurrent_jobs.flx](15_concurrent_jobs.flx) | Fiber concurrency + task parallelism in one program |
| [16_current_worker_count.flx](16_current_worker_count.flx) | `Async.current_worker_count` introspection |
| [17_worker_diagnostic.flx](17_worker_diagnostic.flx) | worker-count diagnostics |
| [18_task_closure_captures.flx](18_task_closure_captures.flx) | `Task.spawn` with immutable closure captures |
| [19_task_spawn_scoped.flx](19_task_spawn_scoped.flx) | `Task.spawn_scoped` — structured concurrency for tasks |
| [22_select_channel_timer.flx](22_select_channel_timer.flx) | `select` over channel receive and timer |
| [23_select_send_recv.flx](23_select_send_recv.flx) | `select` send arm readiness |
| [24_event_composition.flx](24_event_composition.flx) | `Event.choose`, `wrap`, `guard`, `with_nack` |

## Effect surface cheat sheet

The `Async` effect alias expands to `<Suspend | Fork | GetContext | AsyncFail>`.
You only see the alias name in user code:

```flux
fn worker() -> Int with Async {
    Async.sleep(10)
    42
}

fn main() with IO {
    print(Async.run_async(worker))
}
```

## Parser limitation to remember

When a function takes multiple callback parameters, only the **last** one may
carry a `with Async` annotation in its type. Earlier callbacks must be
unannotated; the effect is inferred from named functions you pass in.

```flux
// ✅ allowed
fn both<a, b>(f: (() -> a), g: (() -> b with Async)) -> (a, b) with Async

// ❌ rejected by the parser
fn both<a, b>(f: (() -> a with Async), g: (() -> b with Async)) -> (a, b)
```
