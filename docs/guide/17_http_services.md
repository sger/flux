# Chapter 17 — HTTP Services

Phase 3 adds a source-level `Flow.Http` API for HTTP/1.1 services and plain
HTTP clients. The implementation runs through `Flow.Async`, so examples use
`run_async`, short sleeps, and explicit shutdown to keep programs bounded.

## Server Basics

Import `Flow.Http` and write a handler from `Request` to `Response`:

```flux
import Flow.Async exposing (..)
import Flow.Http exposing (..)

fn handler(req) with Async {
    ok("hello from " + req.path)
}
```

Start a server with `serve` for defaults or `serve_config` for explicit
limits. Both return a `ServerHandle`; call `shutdown(h)` for graceful drain or
`shutdown_now(h)` to close active work.

```flux
let config = server_config(100, 65536, 8388608, 30000)
let h = serve_config("127.0.0.1", 8080, config, handler)
shutdown(h)
```

`ServerConfig` controls connection count, maximum header/body size, and
`request_timeout_ms`. A timed-out fixed-response handler returns `504 Gateway
Timeout`.

## Client Helpers

Plain HTTP clients use `get`, `post`, or `request`:

```flux
let resp = get("http://127.0.0.1:8080/hello")
print(resp.body)
```

Only `http://` is supported in Phase 3. HTTPS/TLS is Phase 4 work.

## Streaming Responses

Use `serve_stream` for chunked HTTP/1.1 responses:

```flux
import Flow.Stream as Stream

fn handler(req) with Async {
    stream_response(200, {}, Stream.from_array([|"one", "two"|]))
}
```

Streaming responses write `Transfer-Encoding: chunked` and close the
connection when the stream ends. Existing fixed-body `serve` behavior is
unchanged.

Worked examples:

```bash
cargo run -- --no-cache examples/http/hello_http_service.flx
cargo run -- --no-cache examples/http/parallel_http_fetch.flx
```
