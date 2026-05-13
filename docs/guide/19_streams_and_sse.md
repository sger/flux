# Chapter 19 — Streams and SSE

`Flow.Stream` is a pure pull-stream library. A stream pull returns the next
value and the next stream state, so callers explicitly continue with the
returned rest stream.

## Pull Model

```flux
import Flow.Stream as Stream

let s = Stream.from_array([|1, 2, 3|])
match Stream.next(s) {
    Some(pair) -> print(pair.0),
    _ -> print("empty")
}
```

The original stream is not mutated. Use `pair.1` for the next pull.

## Adapters and Consumers

Streams include constructors, consumers, adapters, and composition helpers:

```flux
let values = Stream.from_array([|1, 2, 3, 4, 5, 6|])
let shaped = Stream.map(
    Stream.filter(values, fn(value) { value % 2 == 0 }),
    fn(value) { value * 10 }
)
print(Stream.to_array(shaped))
```

`chunk(size)` groups values and emits a final short chunk. `merge(left, right)`
alternates deterministically and drains the remaining side when one stream
ends.

## SSE Helpers

`Flow.Http` builds SSE frames as stream chunks:

```flux
import Flow.Http exposing (..)
import Flow.Stream as Stream

let events = Stream.from_array([|
    sse_event("ready"),
    sse_named_event("tick", "done"),
|])
sse_response(events)
```

`sse_response` sets `Content-Type: text/event-stream` and
`Cache-Control: no-cache`. The response is chunked and closes when the finite
stream ends.

Worked examples:

```bash
cargo run -- --no-cache examples/http/stream_pipeline.flx
cargo run -- --no-cache examples/http/sse_broadcaster.flx
```
