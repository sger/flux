# Async examples (proposal 0174 Phase 1)

End-to-end examples for the `Flow.Async` and `Flow.Tcp` surface. Each
example runs identically on the VM and the LLVM native backend.

| Example | Demonstrates |
| --- | --- |
| `tcp_echo_server.flx` | `Flow.Tcp` listen / accept / read / write / close + `Flow.Async` effect row. |
| `parallel_both.flx` | `Async.both(f, g)` reading from two TCP connections concurrently. |
| `timeout_connect.flx` | `Async.timeout` and `Async.timeout_result` racing a body against a timer. |
| `cancellation_propagation.flx` | `Async.scope` + `Async.fork`, `Async.bracket` cleanup, `Async.try_` catching `AsyncError`. |

## Run

```bash
# VM
cargo run -- examples/async/tcp_echo_server.flx

# LLVM native backend
cargo run --features llvm -- examples/async/tcp_echo_server.flx --native
```

## Phase 1 status

The runtime ships full surface and runtime semantics for `both`, `race`,
`timeout`, `scope`, `fork`, `bracket`, `finally`, `try_`, `fail`,
`yield_now`, `sleep`, plus `Task.spawn` / `Task.await` /
`Task.blocking_join` and `Sendable<T>` enforcement. The `Flow.Tcp`
surface is connected through to the `mio` reactor on both backends.

TCP handles created in the parent fiber are usable inside `Async.both`
/ `Async.race` / `Async.fork` branches: worker VMs share the parent's
`mio` reactor and the reactor routes per-request completions back to
the originating worker via a `RequestId → CompletionSink` map.
