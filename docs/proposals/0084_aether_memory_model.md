- Feature Name: Aether Memory Model
- Start Date: 2026-03-08
- Status: Draft
- Proposal PR:
- Flux Issue:

# Proposal 0084: Aether Memory Model

## Summary
[summary]: #summary

Define **Aether** as Flux's custom reuse-oriented reference-counted memory model. Aether is
inspired by Perceus, but it is not a direct port of Perceus into Flux. Instead, Aether is
the Flux-native semantic and runtime contract for memory management across the VM and JIT:

1. Values are **semantically immutable** at the language level.
2. The runtime uses **reference counting as the baseline ownership mechanism**.
3. Storage may be **reused internally** when uniqueness and isolation make that
   observationally safe.
4. Actor boundaries define explicit **send/copy or transfer/rebuild rules**.
5. VM and JIT must preserve the **same observable memory semantics** even if they use
   different low-level representations or optimizations.

This proposal establishes the canonical memory model for Flux and positions proposals
0068, 0069, and 0070 as implementation steps within Aether rather than standalone
optimizations.

## Motivation
[motivation]: #motivation

Flux is converging on a design with:

- a pure functional surface language
- algebraic effects and handlers
- persistent collections
- actor-style concurrency
- two execution backends (VM and JIT)

That combination makes a generic "GC choice" insufficient. Flux needs a memory model that
matches its language semantics and runtime architecture.

Perceus is strong prior art for this direction:

- precise reference counting
- compiler-guided reuse
- functional-but-in-place optimization

But Flux has requirements that Perceus alone does not settle:

1. **Dual backend parity**: the VM and JIT must behave identically from the language's
   perspective.
2. **Persistent runtime values**: cons lists, arrays, HAMT maps, tuples, closures, and
   ADTs must all participate in one coherent model.
3. **Actor isolation**: values crossing actor boundaries cannot simply share `Rc`
   internals across threads.
4. **Effect-aware runtime structure**: handlers, continuations, and effect boundaries
   create reuse barriers and evaluation boundaries that matter to correctness.
5. **No ownership syntax leakage**: Flux should benefit from reuse without exposing
   Rust-like ownership mechanics in the language surface.

The current runtime history reflects this lack of a single model:

- proposal 0045 introduced tracing GC to reclaim heap objects
- proposal 0068 adds Perceus-inspired uniqueness analysis
- proposal 0069 adds `Rc::get_mut` fast paths
- proposal 0070 removes `GcHandle` in favor of persistent `Rc` structures

These should not remain disconnected features. Aether unifies them under one design.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### What Aether means for Flux programmers

Flux programmers write pure code over immutable values:

```flux
fn increment_all(xs: Array<Int>) -> Array<Int> {
    map(xs, \x -> x + 1)
}
```

The programmer sees ordinary immutable semantics:

- `xs` is never mutated in user-visible behavior
- old values remain valid if referenced elsewhere
- sharing is safe
- actor boundaries preserve isolation

Aether allows the compiler and runtime to make this efficient:

- if `xs` is uniquely owned, the runtime may reuse its storage in-place
- if `xs` is shared, the runtime allocates a fresh result
- both cases produce the same observable result

The principle is:

> Flux values are semantically immutable, but Aether may reuse storage internally when
> uniqueness, effect boundaries, and actor isolation make that observationally safe.

### What Aether is not

Aether is not:

- a user-visible ownership system
- a direct copy of Perceus
- "just the GC"
- permission for arbitrary mutation of shared values
- a license for VM and JIT to diverge in behavior

### Core user-facing guarantees

Programs can rely on these guarantees:

1. **Purity is preserved**. Memory reuse cannot change language-visible behavior.
2. **No aliasing surprises**. Reuse occurs only when a value is not observably shared.
3. **Actor isolation is preserved**. Cross-actor communication never exposes shared
   mutable runtime state.
4. **Backend parity holds**. VM and JIT produce identical results and failure behavior.
5. **Fallback safety exists**. If uniqueness cannot be proven, Aether must fall back to
   allocation rather than guess.

### Example: safe internal reuse

```flux
fn append_one(xs: Array<Int>) -> Array<Int> {
    push(xs, 1)
}
```

Under Aether:

- if `xs` is unique, the runtime may reuse the underlying array buffer
- if `xs` is shared, the runtime allocates a new buffer

Either way, the semantics stay:

```flux
let xs = [|1, 2|]
let ys = append_one(xs)
```

`ys` becomes `[|1, 2, 1|]`. If `xs` is still referenced elsewhere, it remains `[|1, 2|]`.

### Example: actor boundary

```flux
fn main() with Actor, IO {
    let xs = [|1, 2, 3|]
    let pid = spawn(\() -> worker())
    send(pid, xs)
}
```

At the actor boundary, Aether must not expose actor-local `Rc` internals to another
thread/actor. The send path must perform one of the Aether-approved boundary actions:

- deep-copy into a sendable representation
- rebuild into a receiver-local representation
- in a future transfer mode, move ownership in a way that preserves isolation

Phase 1 assumes copy/rebuild semantics unless a stronger transfer proof exists.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Aether semantic model

Aether defines three layers:

#### 1. Semantic immutability

Every Flux value is immutable in the language semantics. No optimization may violate:

- referential transparency
- pattern-match consistency
- equality/display behavior
- actor isolation
- effect handler semantics

This is the non-negotiable top-level rule.

#### 2. Runtime ownership model

At runtime, heap-backed values participate in Aether ownership states:

```text
Fresh      - newly allocated, uniquely owned
Unique     - exactly one live owning reference is known or proven
Shared     - multiple live owners may exist
Escaped    - ownership crosses an analysis/runtime boundary and must be treated conservatively
Transferred - ownership moved across an actor/runtime boundary under a future transfer rule
```

Not all of these states must be explicit in the implementation; they define the model.

Minimum implementation requirement:

- `Fresh`, `Unique`, and `Shared` must be representable either statically, dynamically,
  or both
- `Escaped` must act as a reuse barrier
- `Transferred` is optional in Phase 1 and may be treated as copy/rebuild

#### 3. Reuse policy

Reuse is permitted only when all of the following are true:

1. The value is uniquely owned or proven fresh.
2. The operation preserves the same observable result as allocation would.
3. No actor boundary is crossed illegally.
4. No effect/handler rule requires the old structure to remain materialized.
5. The specific representation supports safe reuse.

If any condition is not met, the runtime must allocate a fresh value.

### Aether-managed value classes

The Aether model applies to all heap-backed runtime values, but not all values need the
same policy.

#### Arrays

- Baseline: `Rc<Vec<Value>>` or backend-equivalent
- Reuse target: strong candidate
- Operations: `map`, `push`, `slice`, rebuild, match reconstruction

Arrays are the first-class reuse target because they are flat, common, and easy to
optimize with `Rc::get_mut`-style checks or equivalent uniqueness tests.

#### Persistent cons lists

- Baseline: `Rc<ConsList>` or backend-equivalent
- Reuse target: selective
- Operations: prepend, tail-focused transformations, conversion boundaries

Lists are semantically persistent. Reuse should focus on reconstruction paths where the
tail is unique and the transformed shape permits safe cell reuse.

#### HAMT maps

- Baseline: persistent `Rc`-backed trie nodes
- Reuse target: path-copy elimination under uniqueness
- Operations: `put`, `remove`, merge-like transforms

HAMTs should preserve structural sharing by default. Under uniqueness, Aether may reuse
intermediate nodes instead of always path-copying.

#### Tuples and ADTs

- Baseline: `Rc<Vec<Value>>` or custom fixed-arity structures
- Reuse target: limited but valid
- Operations: reconstruction after pattern match, field update style transforms

These values are often short-lived and are good candidates for localized reuse.

#### Closures, handlers, continuations

- Baseline: runtime-managed heap values
- Reuse target: highly restricted

These represent execution state, not only data. Aether must treat them conservatively.
They are likely reuse barriers until the effect runtime is fully stabilized.

### Aether reuse barriers

Aether defines situations where reuse must stop unless explicitly proven safe:

1. **Actor boundary**: values sent between actors
2. **Handler boundary**: values captured by effect handlers or resumptions
3. **Unknown aliasing**: closures, globals, or polymorphic calls that may duplicate refs
4. **Foreign/runtime boundary**: host callbacks, JIT helper calls, or debug tooling that
   obscures ownership
5. **Escaping values**: values stored where later aliasing cannot be modeled precisely

This list is intentionally conservative. Incorrect reuse is a semantic bug; missed reuse
is only a performance bug.

### VM and JIT contract

The VM is the authoritative semantics for Aether. The JIT must preserve those semantics.

This means:

1. **Same observable values**: results, errors, display, equality, and effect behavior
   must match.
2. **Same reuse legality**: the JIT may optimize more aggressively only if it does not
   expand the set of programs with behavior differences.
3. **Shared runtime helpers where practical**: sendability conversion, actor boundary
   checks, and persistent structure operations should be centralized.
4. **No backend-specific user semantics**: there must be no program that is valid and
   pure under one backend but impure or unsound under the other.

Implementation guidance:

- model Aether legality once
- expose it to both backends
- keep representation-specific optimizations behind a shared semantic contract

### Relationship to proposals 0068, 0069, and 0070

Aether is the umbrella design. Existing proposals become subordinate parts:

- **0068**: compiler uniqueness analysis
- **0069**: localized in-place reuse fast paths
- **0070**: removal of `GcHandle` and migration to persistent `Rc` structures

Those proposals remain useful, but their rationale should now be read as:

```text
0068 provides Aether's static ownership evidence
0069 provides Aether's first runtime reuse path
0070 provides Aether's persistent representation baseline
```

### Relationship to proposal 0045

Proposal 0045 introduced tracing GC as a practical initial heap strategy. Aether changes
the long-term direction:

- tracing GC is no longer the conceptual center of Flux memory management
- reference counting and reuse become the primary model
- tracing may remain temporarily for legacy representations or cycles if necessary during
  migration, but it is not the target architecture

If Aether is accepted, proposal 0045 should be treated as an implementation-era step, not
the final memory model for Flux.

### Actor boundary rules

Phase 1 Aether actor semantics:

1. Actor-local heap values are not shared directly across actor boundaries.
2. Sending a value performs conversion to a sendable form and reconstruction on the
   receiving side.
3. Non-sendable runtime state must be rejected with a deterministic error.
4. Send/copy behavior must be identical in VM and JIT.

Future Aether extensions may introduce transfer semantics for uniquely owned values, but
only if they preserve:

- isolation
- determinism
- backend parity
- straightforward diagnostics

### Effect and handler interaction

Because Flux uses algebraic effects, memory semantics cannot ignore control flow.

Aether therefore assumes:

1. A continuation capture may turn a previously unique value into an escaped value.
2. Values referenced by active handlers or resumptions are conservatively treated as
   non-reusable unless proven otherwise.
3. Compiler reuse analysis must integrate with effect lowering and handler evidence.

This proposal does not specify the exact algorithm for handler-aware uniqueness, but it
does establish that handler boundaries are part of the Aether model rather than an
afterthought.

### Required invariants

Any Aether implementation must preserve these invariants:

1. **Observational immutability**: no reused structure may be observed as mutated by a
   still-live alias.
2. **Deterministic destruction semantics**: dropping local references must not create
   backend-specific behavior.
3. **Boundary safety**: actor sends never leak non-sendable local runtime pointers.
4. **Fallback correctness**: failed uniqueness proofs always degrade to safe allocation.
5. **Representation transparency**: user code cannot distinguish reused values from
   freshly allocated values except through performance.

### Phased rollout

#### Phase 1: Aether baseline

- adopt Aether as the canonical memory model
- keep VM authoritative
- implement actor-safe sendability boundaries
- continue migrating data structures away from `GcHandle`

#### Phase 2: Aether evidence

- land compiler uniqueness analysis (0068)
- expose ownership evidence to VM/JIT codegen
- add diagnostics/debug support for ownership and reuse decisions

#### Phase 3: Aether reuse

- implement array fast paths first
- extend to tuples, lists, and HAMT nodes where justified
- verify parity across VM and JIT

#### Phase 4: Aether transfer

- consider transfer semantics for uniquely owned actor messages
- only after copy/rebuild semantics are fully correct and observable

## Drawbacks
[drawbacks]: #drawbacks

1. Aether is a larger conceptual system than "use RC everywhere", which increases design
   complexity.
2. Dual-backend parity constrains low-level optimization freedom.
3. Effects, handlers, and actor boundaries make ownership analysis harder than in a
   single-runtime pure data language.
4. Conservative fallback behavior may leave performance on the table until later phases.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Why not keep tracing GC as the main model?

Tracing GC does not align as well with Flux's current direction:

- Flux already uses `Rc` broadly
- actor boundaries dislike shared non-sendable heap handles
- Perceus-style reuse is a better fit for persistent FP optimization
- a dual "sometimes tracing GC, sometimes RC" architecture is harder to reason about

### Why not adopt Perceus verbatim?

Because Flux is not Koka:

- Flux has a VM and a JIT
- Flux has a distinct runtime value representation
- Flux is adopting actors and sendability constraints
- Flux has its own effect/runtime pipeline

Perceus is prior art and influence, not a drop-in implementation plan.

### Why not expose ownership in the syntax?

That would fight the language goals. Flux should stay:

- pure by default
- approachable
- effect-explicit where it matters semantically
- free of low-level ownership burden in ordinary code

Aether should be mostly invisible above the compiler/runtime layer.

## Prior art
[prior-art]: #prior-art

- Perceus: Garbage Free Reference Counting with Reuse (Reinking, Xie, Leijen)
- Reference Counting with Frame Limited Reuse
- Koka runtime and effect system design
- persistent functional data structure implementation techniques
- actor runtimes that separate local representation from sendable representation

## Unresolved questions
[unresolved-questions]: #unresolved-questions

1. Which runtime values, if any, still require tracing support after the Aether
   migration?
2. Should uniquely owned actor messages eventually support zero-copy transfer?
3. What is the right internal representation for tuples and ADTs in the JIT path under
   Aether?
4. How should handler-aware uniqueness evidence be represented in the compiler IR?
5. Should Aether diagnostics be user-visible only through debug tooling, or should some
   ownership facts surface in optimizer reports?

## Future possibilities
[future-possibilities]: #future-possibilities

- transfer semantics for uniquely owned cross-actor values
- region-like short-lived allocation strategies layered under Aether
- adaptive heuristics for choosing reuse vs rebuild for small values
- ownership-aware optimization reports in `flux analyze`
- specialized Aether paths for persistent collection combinators
