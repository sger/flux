- Feature Name: Concurrency Model — Async/Await + Actors
- Start Date: 2026-02-12
- Status: Not Implemented
- Proposal PR: 
- Flux Issue: 

# Proposal 0026: Concurrency Model — Async/Await + Actors

## Summary
[summary]: #summary

Flux adopts a two-layer concurrency model with an **actor-first rollout** for current compiler/runtime maturity: Flux adopts a two-layer concurrency model with an **actor-first rollout** for current compiler/runtime maturity:

## Motivation
[motivation]: #motivation

Flux currently has no concurrency story. The VM runs a single instruction stream to completion. Real programs need: Flux currently has no concurrency story. The VM runs a single instruction stream to completion. Real programs need:

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Syntax (proposed)

```flux
actor Counter(initial: Int) {
  state count = initial

  receive Increment {
    count = count + 1
  }

  receive Get {
    reply(count)
  }
}

fn main() with IO, Async {
  let c = spawn Counter(0)
  send(c, Increment)
  let n = await ask(c, Get)
  print("count=" + to_string(n))
}
```

### Syntax

```flux
// Mark a function as async
fn fetch_user(id) with IO, Async {
  let response = await http_get("/users/#{id}")
  parse_json(response.body)
}

// Concurrent execution
fn load_dashboard(user_id) with IO, Async {
  // spawn concurrent tasks
  let profile = async fetch_profile(user_id)
  let posts = async fetch_posts(user_id)
  let notifications = async fetch_notifications(user_id)

  // await results
  {
    profile: await profile,
    posts: await posts,
    notifications: await notifications,
  }
}

// Sequential async (just use await inline)
fn process_in_order(urls) with IO, Async {
  urls |> map(\url -> await http_get(url))
}
```

### A. VM Runtime Path (authoritative semantics)

Concurrency semantics are defined by VM runtime behavior first. JIT must match these semantics.

### D. Failure Semantics

1. Actor panic/crash:
   - does not crash unrelated actors by default.
   - requester receives failure reply (or timeout failure).
2. Unknown message tag:
   - deterministic runtime error (or compile-time rejection where statically known).
3. Reply timeout (if configured):
   - deterministic timeout error signature.

### Syntax (proposed)

### Syntax

### A. VM Runtime Path (authoritative semantics)

### D. Failure Semantics

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **`Rc` Is Not Thread-Safe:** Every heap-allocated value uses `Rc` ([value.rs:1](src/runtime/value.rs#L1)): - **Single-Threaded VM:** The VM has one stack, one frame list, one...
- **`Rc` Is Not Thread-Safe:** Every heap-allocated value uses `Rc` ([value.rs:1](src/runtime/value.rs#L1)): ```rust pub enum Value { String(Rc<str>), Some(Rc<Value>), Array(Rc<Vec<Value>>), Closure(Rc<Closur...
- **Single-Threaded VM:** The VM has one stack, one frame list, one globals array: ```rust pub struct VM { constants: Vec<Value>, stack: Vec<Value>, // single stack globals: Vec<Value>, // shared mutable...
- **Mutable Globals:** `OpSetGlobal` writes to `globals` — a data race in any concurrent model. In a pure language, globals become constants set once at initialization, eliminating this problem.
- **Layer 1: Actors (MVP, Multi-Threaded Isolation):** Each actor is an **isolated VM instance** running on its own OS thread. Actors communicate exclusively through message passing. Values are **deep-copied** when sent between acto...
- **Message Operations:** | Operation | Syntax | Behavior | |-----------|--------|----------| | **Spawn** | `spawn ActorName(args)` | Create actor, return `ActorRef<T>` | | **Send** | `send(actor, Msg)`...

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

1. Restructuring legacy material into a strict template can reduce local narrative flow.
2. Consolidation may temporarily increase document length due to historical preservation.
3. Additional review effort is required to keep synthesized sections aligned with implementation changes.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Phase 1 Repo File Targets (Decision-Complete)

Parser and syntax:
- `src/syntax/token_type.rs`
- `src/syntax/lexer/mod.rs`
- `src/syntax/parser/statement.rs`
- `src/syntax/parser/expression.rs`
- `src/syntax/parser/parser_test.rs`

AST and compiler front-end:
- `src/ast/statement.rs`
- `src/ast/expression.rs`
- `src/bytecode/compiler/statement.rs`
- `src/bytecode/compiler/expression.rs`
- `src/bytecode/compiler/mod.rs`
- `src/diagnostics/compiler_errors.rs`

Runtime VM:
- `src/runtime/value.rs`
- `src/runtime/vm/mod.rs`
- `src/runtime/vm/dispatch.rs`
- `src/runtime/vm/function_call.rs`
- `src/runtime/base/mod.rs`
- new actor runtime module(s): `src/runtime/actor/*`

Runtime JIT:
- `src/jit/compiler.rs`
- `src/jit/runtime_helpers.rs`
- `src/jit/context.rs`
- `src/jit/mod.rs`

CLI and integration wiring:
- `src/main.rs` (if new flags or runtime setup needed)

### Phase 1 Repo File Targets (Decision-Complete)

## Prior art
[prior-art]: #prior-art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### References

- Erlang/OTP actor model — supervision, mailboxes, "let it crash"
- Elixir `Task` and `GenServer` — friendly actor API
- Kotlin coroutines — structured concurrency with `async`/`await`
- Rust `tokio` — poll-based async runtime architecture
- Gleam — pure FP on BEAM with actors (closest spiritual match)

### References

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions were explicitly listed in the legacy text.
- Follow-up questions should be tracked in Proposal PR and Flux Issue fields when created.

## Future possibilities
[future-possibilities]: #future-possibilities

### Supervision (Future Phase)

Basic fault tolerance — an actor can monitor its children:

```flux
actor Supervisor {
  receive Start {
    let worker = spawn Worker()

    // Monitor — get notified if worker dies
    monitor(worker)
  }

  receive ActorDown(ref, reason) {
    print("Worker died: #{reason}")
    // Restart
    let new_worker = spawn Worker()
    monitor(new_worker)
  }
}
```

Full supervision trees (Erlang OTP-style) are a future extension.

### Supervision (Future Phase)

Basic fault tolerance — an actor can monitor its children:
