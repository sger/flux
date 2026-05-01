# Flux services

Runnable HTTP services written in Flux, exercising the Phase 2 stdlib
(`Flow.Tcp`, `Flow.Async`, `Flow.Json`, `Flow.JsonCodec`, `Flow.Map`).

## todo_api.flx

A minimal in-memory todo API.

```sh
cargo run -- examples/services/todo_api.flx
```

Then in another terminal:

```sh
curl -s http://127.0.0.1:8080/todos
# {"todos":[{"text":"buy milk","id":1},{"text":"write Flux services","id":2}]}

curl -s -X POST -d '{"text":"hello flux"}' http://127.0.0.1:8080/todos
# {"text":"hello flux","id":3}

curl -s http://127.0.0.1:8080/todos
# {"todos":[{"text":"buy milk","id":1},{"text":"write Flux services","id":2},{"text":"hello flux","id":3}]}

curl -s -X POST -d 'garbage' http://127.0.0.1:8080/todos
# {"error":"invalid JSON"}

curl -s http://127.0.0.1:8080/unknown
# {"error":"not found"}
```

State (the todo list) is threaded through a custom accept loop —
Flux is purely functional, so each request returns a new list and the
loop recurses with the updated state.

The service uses one connection per request (closes after responding).
For keep-alive, see `Http.serve_keep_alive` in `lib/Flow/Http.flx`.

The example uses `Flow.Http.Request` / `Flow.Http.Response` directly
via named-field syntax (e.g. `req.method`, `Http.with_header(...)`).
If you need to handcraft request reading or response serialization,
`Http.read_request(conn)` and `Http.build_response_text(resp)` are
public.

## async_demo.flx

A service that exercises async/await primitives — `Async.both`,
`Async.race`, `Async.timeout`. Each request handler does real
concurrent work; you can verify the parallelism with `time`.

```sh
cargo run -- examples/services/async_demo.flx
```

```sh
$ time curl -s http://127.0.0.1:8081/aggregate
{"users_payload":{"users":["alice","bob"]},"orders_payload":{"orders":[101,102,103]}}
real    0m0.211s    # two 200ms tasks ran in parallel via Async.both

$ time curl -s http://127.0.0.1:8081/race
{"winner":"fast"}
real    0m0.107s    # Async.race returned at the 100ms boundary

$ time curl -s http://127.0.0.1:8081/timeout
{"error":"deadline exceeded"}
real    0m0.154s    # Async.timeout(150) tripped, inner task cancelled
```

The accept loop itself is sequential — `Tcp` handles can't currently
cross fiber boundaries, so one connection at a time. Real concurrency
happens **inside** each request handler via `Async.both` / `race` /
`timeout`, which run on the multi-threaded work-stealing scheduler in
[src/runtime/async/](../../src/runtime/async/).
