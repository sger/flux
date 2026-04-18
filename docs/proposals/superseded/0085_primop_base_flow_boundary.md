- Feature Name: PrimOp / Base / Flow Boundary and Promotion Policy
- Start Date: 2026-03-08
- Status: Superseded (2026-04-18)
- Proposal PR:
- Flux Issue:

## Why superseded

The three-layer architecture this proposal formalised (PrimOp / Base / Flow)
no longer exists. `lib/Base/` has been removed; the standard library is now a
single `lib/Flow/` tree sitting directly on top of PrimOps. With the Base
layer gone, the promotion question collapses to a binary choice — something
is either a PrimOp (runtime-privileged) or a Flow module (written in Flux) —
and does not need a dedicated policy proposal. See `lib/Flow/*.flx` for the
current standard library; see proposal 0034 for the PrimOp surface rules.

# Proposal 0085: PrimOp / Base / Flow Boundary and Promotion Policy

## Summary
[summary]: #summary

Define the canonical architectural boundary between **PrimOps**, **Base**, and **Flow** in
Flux, and establish a promotion policy for deciding when an operation should move from
Base to PrimOp.

The policy is:

1. **PrimOps** are the privileged runtime substrate.
2. **Base** is the stable, user-facing prelude vocabulary built on top of PrimOps and
   runtime-backed helpers.
3. **Flow** is the explicit standard library, written in Flux where practical and built
   on Base.

This proposal does **not** recommend converting Base wholesale into PrimOps. Instead, it
locks a selective promotion rule:

> Promote an operation to PrimOp only when it requires runtime privilege,
> representation-aware behavior, backend parity control, or Aether-aware optimization
> hooks that cannot be expressed cleanly at the Base or Flow layer.

This proposal extends the architecture established in [0028](implemented/0028_base.md),
[0029](0029_base_and_flow.md),
[0030](0030_flow.md), and
[0034](implemented/0034_builtin_primops.md).

## Motivation
[motivation]: #motivation

Flux now has three layers that can all host functionality:

- PrimOps
- Base
- Flow

Without a clear policy, growth becomes unstable:

1. **Too many PrimOps** turn the runtime substrate into a second standard library and
   make evolution harder.
2. **Too many Base functions** turn the prelude into a dumping ground for every useful
   helper.
3. **Too much in Flow too early** can hide runtime-critical behavior behind abstractions
   that need backend-sensitive optimization or semantic privilege.

This problem becomes sharper as Flux moves toward:

- Aether ([0084](implemented/0084_aether_memory_model.md))
- actor concurrency ([0065](0065_actor_effect_stdlib.md),
  [0066](0066_thread_per_actor_handler.md))
- persistent collection migration ([0070](implemented/0070_perceus_gc_heap_replacement.md))
- reuse optimizations ([0068](superseded/0068_perceus_uniqueness_analysis.md),
  [0069](superseded/0069_rcget_mut_fast_path.md))

Those features need a small, disciplined runtime substrate, not an ever-growing set of
special cases.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### The three layers

#### PrimOps

PrimOps are internal, privileged operations owned by the runtime and compiler.

Use PrimOps for:

- value-representation-sensitive logic
- VM/JIT semantic single-source-of-truth operations
- effect runtime dispatch
- actor runtime bridge points
- Aether reuse hooks
- low-level boundary checks

PrimOps are not the primary language surface.

#### Base

Base is Flux's user-facing prelude. It should contain the core vocabulary programmers use
every day:

- `len`
- `print`
- `map`
- `filter`
- `fold`
- `get`
- `put`

Base may be implemented in Rust and may call PrimOps, but it is still the stable user
API layer, not the low-level substrate.

#### Flow

Flow is the explicit standard library namespace:

- `Flow.List`
- `Flow.Option`
- `Flow.Either`
- `Flow.Dict`
- `Flow.String`
- `Flow.Actor`

Flow should host combinators, domain helpers, and convenience APIs that do not need to be
implicitly available everywhere.

### Promotion rule

Do **not** promote a function from Base to PrimOp just because it is:

- useful
- common
- performance-sensitive in the abstract

Promote it only if at least one of these is true:

1. It requires **runtime privilege** and cannot be expressed correctly as an ordinary
   Base helper.
2. It depends on **internal value representation** and therefore must be owned by the
   runtime.
3. It requires one **canonical VM/JIT implementation** for semantic parity.
4. It needs **Aether-aware reuse or uniqueness hooks** that should not leak into user
   APIs.
5. It forms part of the **effect/actor runtime bridge** and must be controlled centrally.

If none apply, keep it in Base or Flow.

### Examples

#### Good PrimOp candidate

```text
array_push_unique_internal
```

Reason:

- representation-aware
- Aether-sensitive
- backend-critical

#### Good Base function

```text
push(xs, x)
```

Reason:

- user-facing vocabulary
- may call a PrimOp internally
- should remain stable even if implementation changes

#### Good Flow function

```text
Flow.List.take(xs, n)
```

Reason:

- convenience/combinator
- not needed in the prelude
- can be written in Flux over Base

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Canonical boundary

#### PrimOp layer

PrimOps should be:

- small in count
- semantically sharp
- representation-aware
- runtime-owned
- shared by VM and JIT through one semantic implementation where practical

Canonical responsibilities:

1. Numeric and boolean primitive operations
2. Core comparisons and low-level equality support
3. Low-level string runtime operations
4. Array/list/map internal constructors and access paths
5. Typed boundary/runtime contract checks
6. Effect runtime dispatch hooks
7. Actor runtime bridge hooks
8. Aether reuse and uniqueness-sensitive helpers

#### Base layer

Base should contain:

- the prelude vocabulary users expect without imports
- operations that define Flux's core programming ergonomics
- wrappers over PrimOps where runtime privilege exists but should remain hidden
- higher-order operations that form the language's everyday style

Base is allowed to remain Rust-backed where needed. "Base" does not mean "written in Flux";
it means "language-facing and stable".

#### Flow layer

Flow should contain:

- explicit modules
- domain-grouped helper families
- combinators and convenience APIs
- functionality that can evolve without changing the prelude contract

### Classification matrix

#### Definitely PrimOp

These belong in PrimOp unless later architecture removes the need:

- integer and float arithmetic
- primitive comparisons
- low-level value classification and runtime type predicates
- string concat/length/index/slice internals
- array construction, indexing, slicing, and internal rebuild helpers
- persistent list constructors/accessors used by the runtime substrate
- HAMT/map internal insert/lookup/remove primitives
- typed boundary enforcement helpers
- effect perform/handler dispatch/resume runtime hooks
- actor spawn/send/recv bridge hooks
- Aether uniqueness and reuse helpers

#### Base by default

These should stay user-facing in Base even if backed by PrimOps:

- `len`
- `print`
- `panic` / failure helpers
- `hd`, `tl`, `fst`, `snd`
- `get`, `put`, `has_key`
- `map`, `filter`, `fold`
- assertions
- debug helpers such as `spy`
- common conversions and formatting helpers

#### Flow by default

These should live in explicit modules:

- collection combinators such as `take`, `drop`, `zip`, `chunk`
- option/either helper families
- dictionary helper families beyond the minimal Base surface
- function composition helpers
- actor convenience helpers such as `ask`, `broadcast`, retry wrappers
- test support modules

### "Maybe PrimOp later" category

A function may start in Base and later be promoted if evidence appears.

Likely examples:

- `map`
- `filter`
- `fold`
- `append`
- `concat`
- `keys`
- `values`
- `entries`

Promotion requires evidence, not intuition. Acceptable evidence includes:

1. measured hot-path cost in representative workloads
2. inability to preserve VM/JIT parity cleanly through existing abstractions
3. need for Aether-sensitive fast paths that cannot remain encapsulated below Base

### Aether interaction

This proposal is especially important for Aether.

Aether-specific machinery should generally be:

- implemented in PrimOps or lower runtime helpers
- hidden behind Base APIs
- surfaced to users only through performance and tooling, not direct ownership syntax

Example:

```text
PrimOp: reuse_array_if_unique
Base:   map / push / append
Flow:   higher-level collection combinators
```

This preserves Flux's pure user model while still enabling aggressive runtime reuse.

### Actor interaction

Actor concurrency follows the same rule:

- sendability conversion and mailbox bridge logic belong below Base
- user-facing operations such as `send` and `recv` should stay at the Base or `Flow.Actor`
  surface depending on the final concurrency API

The runtime substrate should expose the mechanism; the language surface should expose the
capability.

### Migration policy

When evaluating an existing Base function:

1. Ask whether the **public name** should stay stable for users.
2. Ask whether the **implementation** needs PrimOp privilege.
3. If yes, keep the public name in Base and move only the implementation substrate down.
4. Only remove the Base surface if there is a compelling architectural reason.

This means most migrations should be:

```text
Base function remains
implementation changes to use PrimOp
Flow helpers remain unchanged
```

not:

```text
delete Base API
force users onto raw PrimOps
```

### Non-goals

This proposal does not:

1. require a rewrite of all higher-order Base functions as PrimOps
2. remove Base as Flux's prelude layer
3. prevent promotion of hot paths when evidence exists
4. freeze the exact contents of Flow forever

## Drawbacks
[drawbacks]: #drawbacks

1. A three-layer model requires discipline and documentation to keep boundaries clear.
2. Some functions will live in Base even when their implementation is PrimOp-heavy, which
   can feel indirect to contributors.
3. Performance work may occasionally require revisiting a classification after evidence
   arrives.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Why not move everything performance-sensitive into PrimOps?

Because that would turn PrimOps into a second standard library and weaken Flux's ability
to evolve user-facing APIs independently of runtime internals.

### Why not keep almost everything in Flow?

Because some operations require runtime privilege, exact backend parity, or representation
knowledge that should not be reimplemented in ordinary library code.

### Why not expose PrimOps directly to users?

Because Flux's design goal is a clean pure FP language surface. Users should think in
terms of values, effects, handlers, Base, and Flow, not low-level runtime intrinsics.

## Prior art
[prior-art]: #prior-art

- language runtimes that distinguish low-level intrinsics from prelude functions
- Koka's split between language/runtime substrate and library surface
- Elm/Haskell/Gleam style separation between prelude/core vocabulary and explicit modules

## Unresolved questions
[unresolved-questions]: #unresolved-questions

1. Which existing Base functions currently exceed the intended prelude boundary and should
   move to Flow in a later cleanup?
2. Should actor operations ultimately live in Base or exclusively in `Flow.Actor` with
   Base re-exports?
3. Should some higher-order array operations get hidden PrimOp-assisted fast paths without
   changing their public Base classification?
4. How should contributor docs present "implementation in PrimOp, API in Base" to reduce
   confusion?

## Future possibilities
[future-possibilities]: #future-possibilities

- a contributor-facing inventory mapping every Base function to its implementation layer
- an optimizer report showing which Base calls were lowered to PrimOps
- a future cleanup proposal to shrink Base after Flow matures
