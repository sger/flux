- Feature Name: Static Purity Completion
- Start Date: 2026-03-11
- Status: **Part 2 complete, Parts 1 and 3 open**
- Last Updated: 2026-04-20
- Proposal PR:
- Flux Issue:
- Depends on: 0086 (backend-neutral core IR), 0098 (Flux IR / JIT work), 0145 (Type Classes), 0156 (Static Typing Completion Roadmap), 0161 (Effect System Decomposition), 0162 (Unified Effect Handler Runtime), 0165 (IO Primop Migration to Effect Handlers — executes Part 1)

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
| Part 1: IO as algebraic effect | Delegated to 0161 + 0162 Phase 1 + 0165 | execution lives in 0165; this row closes when 0165 is Implemented |
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

#### Sequencing — this part is now gated on 0161 and 0162

Part 1 should **not** be started in isolation. It composes with two other
proposals that landed after 0099 was first written:

1. **0161 (Effect System Decomposition)** must land first. Otherwise
   IO primops would be migrated to `perform` against the monolithic `IO`
   label that 0161 is actively deleting, requiring a second migration
   once `Console` / `FileSystem` / `Stdin` / `Clock` exist as real labels.
2. **0162 Phase 1 (evidence-passing for tail-resumptive handlers)** should
   land next. Without it, routing every `println` through `Perform`
   allocates a continuation per call — a visible regression on hello-world-
   shaped programs. Phase 1 makes the migration performance-neutral.
3. **Then Part 1 of this proposal** performs the mechanical rewiring:
   stop promoting `println` / `print` / `read_file` / `write_file` /
   `read_stdin` to `CorePrimOp` in
   [src/core/passes/primop_promote.rs](../../src/core/passes/primop_promote.rs);
   lower them instead to `Perform("Console", "print", args)` and siblings;
   install a default handler at program init that dispatches each
   operation to the existing C runtime entry point (`flux_println`, etc.).
   The primops do not disappear — they become the handler body rather
   than the direct call target.

#### Prior art — languages that model IO as an algebraic effect

The pattern below is not novel. It has been shipped in production and
research languages. Flux is converging on the same answer.

| Language | IO representation | Default handler | User-interceptable? |
|---|---|---|---|
| **Koka** | `io` row alias over `console`, `fsys`, `net`, `ui`, `exn`, `ndet`, …; ops like `console/print`, `fsys/read-file` | Runtime installs primitive handlers implicitly at `main`; `main` is typed `io` | Yes — `with handler { fun print(s) { … } }` is idiomatic for mocking |
| **Eff** (Bauer & Pretnar) | Built-in effects `Stdout`, `Stdin`, `RandomInt`, …; operations `#print`, `#read` | Interpreter carries a built-in handler that delegates to OCaml stdlib | Yes — the language's central selling point |
| **Effekt** | Capability-based: `console: Console`, `io: IO` capabilities passed as blocks; `do println(s)`, `do readFile(p)` | Runtime entry installs built-in capabilities (JS/Chez/LLVM) before `main` | Yes — `try { … } with console { def println(s) = … }` |
| **Unison** | `IO` ability with ops `IO.putText`, `IO.fileRead`, `IO.getLine`; functions typed `'{IO} a` | Native `IO` handler provided by the runtime at program entry | Partial — a pure `IO` handler can be written for tests, but `ucm run` installs the native one |
| **Frank** | All effects are abilities on the ambient ability; e.g. `Console` with `inch : Char`, `ouch : Char -> Unit` | Compiler/interpreter wires `Console` to host IO at the outermost shell | Yes — routine in Frank's examples |
| **Links** | Effect rows with ops like `Print : String -> ()`, `Read : String` | Top-level `run` handles ambient effects against runtime primitives | Yes — `handle { … }` |

Contrast cases (effect-capable languages that still keep IO imperative):

- **OCaml 5** — effect handlers (2022) power concurrency libs (Eio,
  Domainslib), but `Stdlib.print_endline` and `In_channel` remain direct
  C calls. Demonstrates that having handlers is not the same as moving
  IO onto them.
- **Idris 2** — `IO a` is a primitive monad backed by the runtime; the
  older `Effects` library is not the stock stdlib surface.

Koka is the closest fit for Flux's direction: row-polymorphic effects,
multi-label `IO` alias, default-handlers-at-`main`, and handlers that
the user can replace for testing. 0161 explicitly cites Koka as the
model for label decomposition; Part 1 extends the same alignment from
labels to the actual primop rewiring.

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

Required work (to be done *after* 0161 lands and 0162 Phase 1 lands):

- represent built-in IO through the effect model, using the labels
  declared by 0161 in `Flow.Effects` (`Console`, `FileSystem`, `Stdin`,
  `Clock`, …) rather than a single monolithic `IO`
- define the default runtime IO handler boundary — one handler per
  decomposed label, each dispatching to the existing C runtime entry
  point (`flux_println`, `flux_read_file`, …)
- install the default handler stack at program init so plain `println`
  at the top level continues to work without explicit `handle`
- migrate privileged base IO functions in
  [lib/Flow/IO.flx](../../lib/Flow/IO.flx) and the primop promotion
  table in
  [src/core/passes/primop_promote.rs](../../src/core/passes/primop_promote.rs)
  to `Perform` against the new labels
- add tests that intercept or replace IO handlers in pure harnesses
  (capture, mock, redirect) — this is the user-visible proof that IO
  is no longer privileged

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
