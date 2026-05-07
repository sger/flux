# Chapter 18 — JSON Codecs

`Flow.Json` provides a tagged `Json` value type, parser/stringifier primops,
manual `Encode` / `Decode` instances, and derived codecs for ADTs.

## Json Values

Parse JSON into `Json` with `parse_result` or `parse`:

```flux
import Flow.Json as Json

let value = Json.parse("{\"name\":\"Ada\",\"active\":true}")
print(Json.encode_json(value))
```

`parse_result` returns `Json.JsonOk(value)` or `Json.JsonErr(error)`.
`parse` is a convenience wrapper that panics on malformed input.

## Manual Values

Construct values directly when that is clearer:

```flux
let doc = Json.object(Json.set_field(Json.empty_fields(), "message", Json.string("ok")))
print(Json.encode_json(doc))
```

Primitive and container codec instances are available for strings, booleans,
numbers, options, arrays, lists, and maps.

## Derived Codecs

Attach `deriving (Json.Encode, Json.Decode)` to an ADT declaration:

```flux
data User {
    User { name: String, age: Int }
} deriving (Json.Encode, Json.Decode)

let encoded = encode(User { name: "Ada", age: 42 })
let wire = Json.encode_json(encoded)
let decoded = decode(Json.parse(wire))
```

Derived ADTs use the stable tagged-object shape:

```json
{"tag":"User","fields":{"age":42,"name":"Ada"}}
```

Decode failures return `JsonErr(JsonError { path, message })`.

Worked example:

```bash
cargo run -- --no-cache examples/http/json_echo_service.flx
```
