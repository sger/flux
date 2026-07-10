# Chapter 21 — Actors

> Proposal: [`docs/proposals/0177_concurrency_reliability_and_actor_mvp.md`](../proposals/0177_concurrency_reliability_and_actor_mvp.md) (M4, delivering [0143](../proposals/0143_actor_concurrency_roadmap.md) Phase A)
>
> Worked examples: [`examples/actors/`](../../examples/actors/) — `counter`, `ping_pong`, `fan_out`, each running with identical output on the VM and the native backend (pinned by `tests/integration/actor_mvp.rs` and its native twin).

## Learning Goals

- Understand what an actor is and what problem it solves that bare fibers don't.
- Spawn an actor with `spawn`, send it messages with `tell`, and block on its
  mailbox with `receive`.
- Keep actor state in tail-recursion parameters and express protocols as
  message ADTs.
- Do request/reply by carrying a reply channel *inside* a message.
- Shut an actor down gracefully (a `Stop` message) or forcefully (`stop`).

This chapter assumes [Chapter 20 (Async and Fibers)](20_async_and_fibers.md).
Actors are not a new runtime — they are a *pattern* over the fibers and
channels you already know.

---

## 1. Why Actors

Chapter 20 gives you fibers and channels: cheap concurrent tasks and typed
queues between them. That's enough to wire any concurrency shape by hand — and
for long-lived *stateful* services, the hand-wiring is always the same: a loop
fiber, an inbox channel, a message type, careful shutdown. An **actor** is that
shape packaged: a fiber that owns a **mailbox** and processes `Sendable`
messages one at a time.

The payoff is reasoning: *inside* an actor there is no concurrency. State
transitions happen one message at a time, so the body reads like a plain
sequential loop. All concurrency lives *between* actors, and the only way to
affect an actor's state is to send it a message. No shared mutable state, no
locks — the type system even enforces (via `Sendable`) that you can't smuggle
a closure or continuation across the boundary.

## 2. The Surface

Everything lives in `Flow.Actor`:

```flux
import Flow.Actor exposing (..)
```

| Function | Type (informally) | Meaning |
|---|---|---|
| `spawn(body)` | `((Mailbox<m>) -> Unit with Async) -> ActorRef<m>` | Fork `body` on a fresh fiber under a private scope; hand it a new mailbox. |
| `spawn_sized(cap, body)` | as above, with explicit mailbox capacity | Default capacity via `spawn` is 64. |
| `tell(ref, msg)` | `(ActorRef<m>, m) -> Unit with Async` | Enqueue a message. Suspends the *sender* if the mailbox is full (back-pressure). |
| `receive(mb)` | `(Mailbox<m>) -> m with Async` | Park the actor fiber until a message arrives. A cancellation checkpoint. |
| `try_receive(mb)` | `(Mailbox<m>) -> Option<m>` | Non-blocking probe. |
| `stop(ref)` | `(ActorRef<m>) -> Unit with Async` | Cancel the actor's scope, curtailing even a parked `receive`. |

Two design points worth noticing:

- **The mailbox is the capability.** Only `spawn` ever constructs a
  `Mailbox<m>`, and `receive` demands one — so "only an actor can read its own
  mailbox" is unforgeable by construction, with no runtime check. (A
  first-class `with Actor` effect label is future work; today the capability
  is carried as a value.)
- **Messages must be `Sendable`.** Primitives, strings, ADTs of sendable
  things, and channels all qualify; closures and continuations don't. The
  compiler rejects unsendable payloads at the `tell` site.

## 3. A Stateful Actor: the Counter

State lives in the loop parameters; the protocol is an ADT. From
[`examples/actors/counter.flx`](../../examples/actors/counter.flx):

```flux
data Msg { Inc, Get(Channel<Int>) }

fn counter(mb: Mailbox<Msg>, state: Int) -> Unit with Async {
    match receive(mb) {
        Inc      -> counter(mb, state + 1),
        Get(rep) -> reply(mb, rep, state)
    }
}

fn reply(mb: Mailbox<Msg>, rep: Channel<Int>, state: Int) -> Unit with Async {
    let _ = Channel.send(rep, state)
    counter(mb, state)
}
```

Each `receive` returns exactly one message; the tail call carries the updated
state to the next iteration. Tail-call elimination makes the loop free.

Driving it:

```flux
fn driver() -> String with Async {
    let c = spawn(fn(mb) { counter(mb, 0) })
    tell(c, Inc)
    tell(c, Inc)
    let rep = Channel.make(1)
    tell(c, Get(rep))
    match Channel.recv(rep) {
        Some(v) -> "count = " + to_string(v),   // "count = 2"
        None    -> "mailbox closed"
    }
}
```

**Request/reply is a channel in the message.** The requester makes a channel,
sends it along inside `Get`, and blocks on `Channel.recv` until the actor
answers. There is no built-in `ask` — this composes one out of parts you
already have, and it typechecks end-to-end: a `Get` carrying a
`Channel<String>` into a `Mailbox<Msg>` where `Msg` declares `Channel<Int>` is
a compile-time error.

## 4. Shutdown: Graceful and Forceful

**Graceful** — make "stop" part of the protocol. The actor returns from its
loop instead of recursing, its fiber completes, and its scope is reaped. From
[`examples/actors/ping_pong.flx`](../../examples/actors/ping_pong.flx):

```flux
data Msg { Ping(Channel<String>), Stop }

fn pong(mb: Mailbox<Msg>) -> Unit with Async {
    match receive(mb) {
        Ping(rep) -> answer(mb, rep),   // reply, then recurse
        Stop      -> done()             // return: the actor ends
    }
}
```

**Forceful** — `stop(ref)` cancels the actor's scope. A `receive` parked on an
empty mailbox is a cancellation checkpoint, so the actor is torn down at once;
pending messages are dropped. Use it when the actor's remaining work is
irrelevant — a graceful `Stop` message is the right default when it isn't.

**Or neither** — an actor still parked in `receive` when `run_async` returns
is reaped by teardown. One-shot workers
([`examples/actors/fan_out.flx`](../../examples/actors/fan_out.flx)) simply
handle their one message and return; no shutdown protocol at all:

```flux
fn worker(mb: Mailbox<Int>, results: Channel<Int>) -> Unit with Async {
    let job = receive(mb)
    Channel.send(results, job * job)
}
```

## 5. What Happens Underneath

- `spawn` forks the body with the same primop as `Flow.Async`'s structured
  concurrency, under a private `Scope` — an actor *is* a fiber, and `stop` *is*
  a scope cancel.
- The mailbox is a bounded `Flow.Channel`. `tell` is a channel send: a full
  mailbox suspends the sender, which is back-pressure for free. `receive` is a
  channel recv: an empty mailbox parks the fiber at zero cost until a message
  is published.
- Determinism: under the test scheduler
  (`run_async_with(with_deterministic_scheduler(seed), body)`), actor
  spawn-wake order is seed-selected and replays byte-identically; mailbox
  wake-ups resume in publish order. See `tests/integration/actor_mvp.rs`.

## 6. Not Yet (Deliberately)

The MVP is the 0143 Phase A slice. Explicitly deferred: **supervision /
restart strategies**, **typed per-message protocols** beyond a shared message
ADT, **distribution**, and the first-class **`Actor` effect label** (blocked
on effect-system work; the value-carried `Mailbox` capability covers the
safety in the meantime). If you need those today, compose them from scopes,
`try`/`fail`, and messages — the substrate is all public.
