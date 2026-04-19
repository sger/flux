- Feature Name: Static Purity Completion
- Start Date: 2026-03-11
- Status: **Part 2 complete, Parts 1 and 3 open**
- Last Updated: 2026-04-15
- Proposal PR:
- Flux Issue:
- Depends on: 0086 (backend-neutral core IR), 0098 (Flux IR / JIT work), 0145 (Type Classes), 0156 (Static Typing Completion Roadmap)

# Proposal 0099: Static Purity Completion

## Summary
[summary]: #summary

Track three changes that together complete Flux as a statically pure functional
language:

1. **IO as a first-class algebraic effect**
2. **`Any` elimination from user-facing code** — complete and retained here only as historical context
3. **Monomorphization at the IR layer**

This proposal is no longer the active static-typing roadmap. That role moved to
`0156`. This proposal should now be read mainly as:

- an IO/purity follow-on proposal
- a monomorphization/specialization umbrella
- a historical record that the old `Any`-elimination track is closed

## Status
[status]: #status

| Part | Status | Notes |
|---|---|---|
| Part 1: IO as algebraic effect | Open | still not implemented |
| Part 2: `Any` elimination | Complete | closed by `0145`, `0146`, `0149`, and `0156`; downstream semantic-`Dynamic` cleanup moved to `0157`/`0158` |
| Part 3: Monomorphization | Open | now unblocked by the static-typing closure |

## Motivation
[motivation]: #motivation

Flux has the language features needed for a statically pure FP language, but two
open areas still matter here:

### Gap 1 — IO is still privileged

`IO` remains a privileged runtime path instead of being modeled as a normal
algebraic effect with a standard handler boundary.

This prevents:

- effect handling symmetry between built-in and user-defined effects
- pure interception/mocking of IO-heavy code
- a cleaner purity story at the language level

### Gap 2 — Monomorphization and specialization are still missing

The type system is now strong enough to support deeper specialization work, but
the runtime still depends heavily on generic tagged values for many paths.

This prevents:

- erasing generic value traffic on more typed code paths
- specializing generic functions per concrete type
- fully exploiting the typed Core / representation split already established in
  later proposals

### Historical note on `Any`

The old static-typing problem that used to live here is no longer open:

- source annotations do not accept `Any`
- maintained HM/runtime paths no longer rely on `Any`
- the maintained static-typing closure is recorded in `0156`
- downstream semantic-vs-representation cleanup is tracked by `0157` and `0158`

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Part 1 — IO as an algebraic effect

After this change, built-in IO should behave like other effects:

- operations are represented through the effect system
- the runtime supplies the default real-world handler
- tests and tooling can install alternate handlers

The main language payoff is purity that is structurally enforced and testable.

### Part 2 — Historical `Any` elimination

This part is complete.

Current corpus rule:

- use `0156` for the static-typing closure
- use `0157` and `0158` for downstream representation cleanup
- do not use `0099` as the active static-typing roadmap

### Part 3 — Monomorphization

Once generic functions can be specialized at the IR layer, the compiler should
be able to clone and specialize them per concrete type use where profitable.

That work is now better grounded than when this proposal was first written,
because:

- maintained static typing is closed
- explicit semantic residue now survives in Core
- runtime representation is better separated from semantic type

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Part 1 — IO effect work

Required work:

- represent built-in IO through the effect model
- define the default runtime IO handler boundary
- migrate privileged base IO functions to perform through that boundary
- add tests that intercept or replace IO handlers in pure harnesses

### Part 2 — Closed historical work

This proposal does not own any further active `Any`-elimination tasks.

The closed work should be considered to have moved through:

- constrained polymorphism and class infrastructure
- operator desugaring and hardening
- maintained static-typing closure in `0156`

### Part 3 — Monomorphization work

Required work:

- collect concrete generic instantiations at the IR layer
- clone/specialize eligible generic functions
- preserve a fallback path for unspecialized generic execution where needed
- add performance and correctness coverage for specialized vs unspecialized paths

This part should build on the existing typed semantic pipeline, not on any
revived gradual typing machinery.

## Exit criteria
[exit-criteria]: #exit-criteria

This proposal should be considered complete when:

- built-in IO is modeled through the effect system instead of privileged ad hoc runtime calls
- monomorphization exists as a real maintained optimization path
- the proposal corpus consistently treats Part 2 as closed historical work
