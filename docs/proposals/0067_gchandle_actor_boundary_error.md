- Feature Name: GcHandle Cross-Actor Boundary Error
- Start Date: 2026-03-01
- Status: Not Implemented
- Proposal PR: pending
- Flux Issue: pending

# Proposal 0067: GcHandle Cross-Actor Boundary Error

## Summary
[summary]: #summary

Emit a clean, actionable runtime error (`E1005`) when a `Value::Gc` (cons list or HAMT
map) is passed to `send()`, with a diagnostic that tells the user exactly how to fix it.
This is a deliberate Phase 1 limitation — GC-managed values cannot safely cross actor
thread boundaries — and the user must convert them first.

## Motivation
[motivation]: #motivation

`Value::Gc(GcHandle)` wraps a `u32` index into a global `GcHeap`. The heap itself is
`!Send` — it lives on one thread and is not protected by any lock. Allowing a raw
`GcHandle` to cross actor boundaries would:

1. Cause undefined behavior if the receiving actor dereferences the handle on a different
   thread where the heap is not accessible.
2. Silently corrupt data if the sending actor's GC runs and moves objects after the
   handle is sent.

A clear error is strictly better than a silent memory safety violation. The error message
must explain *what* failed and *how* to fix it.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### What the user sees

```flux
import Flow.Actor

fn main() with Actor, IO {
    let xs = list(1, 2, 3)       -- creates a Value::Gc (cons list)
    let w  = spawn(\() with Actor -> do {
        let _ = recv()
        unit
    })
    send(w, xs)                  -- runtime error: xs is a GC-managed value
}
```

```
error[E1005]: cannot send GC-managed value across actor boundary
  --> examples/actors/bad_send.flx:7:5
   |
 7 |     send(w, xs)
   |     ^^^^^^^^^^^
   |
   = note: `xs` is a cons list (GC-managed). Cons lists cannot be sent between actors
           because they reference the sending actor's private GC heap.
   = hint: convert to an array before sending: `send(w, to_array(xs))`
   = hint: or rebuild on the receiving side: `send(w, [1, 2, 3])`

error: actor send failed — execution halted
```

### How to fix it

```flux
-- Option 1: Convert to array (sendable)
send(w, to_array(xs))

-- Option 2: Convert to array inline
send(w, [|1, 2, 3|])

-- Option 3: Send individual elements
map(xs, \x -> send(w, x))
```

HAMT maps:

```flux
let m = put(put({}, "a", 1), "b", 2)   -- Value::Gc (HAMT map)
send(w, m)                              -- E1005

-- Fix: no direct conversion in Phase 1.
-- Send a tuple of key-value pairs instead:
send(w, [|("a", 1), ("b", 2)|])
```

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### New error code: E1005

Register in `src/diagnostics/runtime_errors.rs`:

```rust
// src/diagnostics/runtime_errors.rs

/// E1005: GC-managed value sent across actor boundary.
pub const E1005_GC_VALUE_CROSS_BOUNDARY: ErrorCode = ErrorCode::new(1005);

pub fn gc_value_actor_boundary(
    value_description: &str,    // "a cons list" or "a HAMT map"
    fix_hint: &str,             // "convert with to_array()" or "send as a tuple"
    span: Option<Span>,
) -> Diagnostic {
    let mut d = diag_enhanced(E1005_GC_VALUE_CROSS_BOUNDARY)
        .with_title("cannot send GC-managed value across actor boundary")
        .with_message(format!(
            "the value is {} which references the sending actor's private GC heap",
            value_description
        ))
        .with_hint(fix_hint);

    if let Some(s) = span {
        d = d.with_span(s);
    }
    d
}
```

Register in `src/diagnostics/registry.rs`:

```rust
registry.register(
    E1005_GC_VALUE_CROSS_BOUNDARY,
    "GC-managed value cannot cross actor boundary",
    "Convert cons lists with `to_array()` before sending. \
     HAMT maps have no direct conversion in Phase 1 — represent as an array of tuples.",
);
```

### Error emission in `SendableValue::from_value`

Extend the existing `SendError` → `E1005` path in the send PrimOp:

```rust
// src/primop/mod.rs — in execute_actor_primop, ActorSend arm:

PrimOp::ActorSend => {
    let target_id = /* ... */;
    let msg       = /* ... */;

    let sendable = SendableValue::from_value(&msg)
        .map_err(|e| match e {
            SendError::GcValueCrossBoundary => {
                let (desc, hint) = describe_gc_value(&msg);
                gc_value_actor_boundary(&desc, &hint, None).render_to_string()
            }
            SendError::FunctionCrossBoundary =>
                "cannot send function or closure to another actor".to_string(),
            SendError::AdtFieldNotSendable { constructor, field_index } =>
                format!("ADT `{}` field {} contains a non-sendable value", constructor, field_index),
        })?;

    runtime.registry.send(target_id, current_actor_id(), sendable);
    Ok(Value::None)
}

/// Returns a human-readable description of why a Gc value cannot be sent.
fn describe_gc_value(v: &Value) -> (String, String) {
    match v {
        Value::Gc(_) => (
            "a cons list or HAMT map".to_string(),
            "convert cons lists with `to_array(xs)` before sending. \
             For maps, send as an array of tuples.".to_string(),
        ),
        _ => ("an unsendable value".to_string(), "".to_string()),
    }
}
```

### Detection at the `AdtValue` level

ADT values may contain `Gc` in their fields. The recursive `from_value` call in
`SendableValue::from_value` handles this:

```rust
Value::Adt(adt) => {
    let fields: Result<Vec<_>, _> = adt.fields.iter()
        .enumerate()
        .map(|(i, f)| {
            SendableValue::from_value(f).map_err(|_| SendError::AdtFieldNotSendable {
                constructor: adt.constructor.as_ref().to_string(),
                field_index: i,
            })
        })
        .collect();
    // ...
}
```

The error message for ADT fields:

```
error[E1005]: cannot send ADT value across actor boundary
  = note: constructor `Node` field 0 contains a cons list (GC-managed)
  = hint: restructure `Node` to store arrays instead of cons lists,
          or convert each Gc field with `to_array()` before construction.
```

### Compile-time warning (optional enhancement)

If the static type of the `send` argument is known to be a cons list or HAMT map at
compile time (via HM inference), the compiler can emit an `E1005` warning at compile time
rather than waiting for runtime:

```rust
// In hm_expr_typer.rs — when validating a send() call:
if let HmExprTypeResult::Known(InferType::Con(TypeConstructor::List, _)) = arg_type {
    self.push_warning(
        diag_enhanced(E1005_GC_VALUE_CROSS_BOUNDARY)
            .with_span(send_span)
            .with_message("argument is a cons list; this will fail at runtime")
            .with_hint("convert with `to_array()` before calling `send()`")
    );
}
```

This is a best-effort enhancement, not required for Phase 1.

### Test fixture

```flux
-- tests/testdata/actors/gc_send_error.flx
-- Expected: E1005 at runtime

import Flow.Actor

fn main() with Actor, IO {
    let xs = list(1, 2, 3)
    let w = spawn(\() with Actor -> do {
        let _ = recv()
        unit
    })
    send(w, xs)
}
```

```bash
# Should fail with E1005
cargo run -- --no-cache --root lib/ tests/testdata/actors/gc_send_error.flx
echo "Exit code: $?"  # expect non-zero
```

```flux
-- tests/testdata/actors/gc_send_fixed.flx
-- Expected: success after conversion

import Flow.Actor

fn main() with Actor, IO {
    let xs = list(1, 2, 3)
    let w = spawn(\() with Actor -> do {
        let arr = recv()
        print(arr)
    })
    send(w, to_array(xs))    -- OK: Array is sendable
}
```

## Drawbacks
[drawbacks]: #drawbacks

- Runtime error rather than compile-time error for the most common case. Avoidable with
  the optional compile-time warning enhancement.
- `to_array()` produces an `Array` (Rc-based, sendable) from a cons list (Gc-based).
  This is O(n) in list length — not free. The hint should mention this.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

**Why not allow it unsafely?** The heap index `u32` in `GcHandle` is only valid in the
context of the specific `GcHeap` instance. On a different thread, it indexes into
unrelated memory. This is undefined behavior, not a performance tradeoff.

**Why not automatically convert?** Silent conversion hides O(n) work. Explicit conversion
(user calls `to_array()`) makes the cost visible at the call site.

**Why E1005 not E1004?** E1004 is reserved for runtime boundary type violations
(`Any → T` cast failure). E1005 is a new class: actor isolation violation.

## Prior art
[prior-art]: #prior-art

- **Erlang/BEAM**: solves this by copying all terms on send. Flux chose explicit conversion
  instead.
- **Go**: channel sends of non-`Send` types are a compile-time error. Flux's `Value` type
  does not carry sendability at the type level, hence the runtime error.
- **Rust's `std::thread::spawn`**: requires `Send + 'static` bounds — the same guarantee
  we enforce manually via `SendableValue`.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

1. Should the error terminate the program or only the sending actor? Decision: terminate
   the program in Phase 1. Recoverable actor errors are part of the failure model (future).
2. Should HAMT maps get a `to_array_of_tuples()` stdlib function? Decision: yes, add as
   a base function alongside this error. Makes the fix accessible.

## Future possibilities
[future-possibilities]: #future-possibilities

- **Proposal 0070** (Perceus + GcHandle replacement): makes `Value::Gc` unnecessary.
  Cons lists and HAMT maps become Perceus-managed with `Rc`, making them sendable via
  the `SendableValue` deep-copy path like all other values.
- **Compile-time sendability check**: extend the type system to track whether a value
  type is actor-sendable, turning E1005 from a runtime error into a compile-time error.
