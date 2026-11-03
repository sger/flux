- Feature Name: `checked_guarantees`
- Start Date: 2026-11-03
- Proposal PR:
- Flux Issue:
- Status: Draft (umbrella)
- Tracks: [0196](0196_checked_fip.md) (T1), [0197](0197_partiality_as_effects.md) (T2), [0198](0198_opaque_types_and_total_stdlib.md) (T3), [0199](0199_handler_linearity_and_scoped_concurrency.md) (T4)
- Relates to: [0135](0135_total_functions_and_safe_arithmetic.md), [0174](0174_async_effect_concurrency.md), [0189](0189_aether_owned_moves.md), [0193](0193_parameter_ownership_annotations.md)

# Proposal 0195: Checked guarantees

## Summary
[summary]: #summary

Rust checks memory safety and freedom from data races at compile time. Flux
already gets memory safety from purity and Aether, so a borrow checker would
make users prove what the compiler already knows. This umbrella proposal picks
four properties that Flux programs can still get wrong today, and makes each one
a **checked guarantee**: a property the compiler infers where it can, and that
an annotation turns into a promise the build rejects when it is broken.

| Track | Guarantee | Annotation | Proposal |
|---|---|---|---|
| T1 | This function updates in place and allocates nothing | `@fip` / `@fbip` (exist) | [0196](0196_checked_fip.md) |
| T2 | This function cannot crash, or cannot loop forever | `total`, effects `Exn` / `Diverge` | [0197](0197_partiality_as_effects.md) |
| T3 | This value satisfies its invariant (non-zero, non-empty) | opaque types | [0198](0198_opaque_types_and_total_stdlib.md) |
| T4 | A continuation is resumed as often as its effect allows; no fiber outlives its scope | one-shot operations, scoped handles | [0199](0199_handler_linearity_and_scoped_concurrency.md) |

The tracks run in this order, T1 to T4. Each is useful on its own.

## Motivation
[motivation]: #motivation

Each of the four properties is partly built, and in each case a doc, a comment
or a dump claims more than the build checks. Checked 2026-11-03 on
`perf/native-runtime-roadmap` (b21d7f89):

- **T1.** `@fip` and `@fbip` parse, reach Aether and have a fixpoint analysis.
  But the build checks the Core *before* Aether (`src/aether/mod.rs:992`,
  `check_fbip(&semantic_core, ..)`), where no `Reuse` node exists yet. In
  `tests/snapshots/aether/aether__aether__fbip_success_cases__dump_aether.snap`
  the dump header says `set_black … FBIP: fip`, while the build diagnostic for the
  same function says `NotProvable`. A failed `@fip` is only a warning.
  `docs/internals/aether.md:282` says annotated functions are checked after
  Aether.
- **T2.** `Div`, `Panic` and `Exn` labels exist (`src/syntax/builtin_effects.rs`).
  But `/` and `%` carry no effect, `Panic` is filtered out of eight effect checks,
  and `panic`'s fallback scheme is pure (`src/compiler/mod.rs:4665`). So a function
  that can stop the process type-checks as pure.
- **T3.** `Flow.NonZero` promises a non-zero integer through a private
  constructor. `import Flow.NonZero exposing (..)` followed by `NonZero(0)` compiles
  with no diagnostic, and `div_nz(10, NonZero(0))` stops with E1008 inside
  `lib/Flow/NonZero.flx:28`. `Array.maximum([])` returns `0`.
- **T4.** `docs/guide/20_async_and_fibers.md:326` says "There are no orphan
  fibers", and `examples/guide_async/09_scope_fork_workers.flx:15` says "When the
  scope's body returns, every forked child is awaited". `scope`
  (`lib/Flow/Async.flx:237`) only allocates a scope id and calls the body. A child
  forked under it prints after `scope` has returned (VM, 2026-11-03). `resume`
  is an ordinary function with no use tracking.

A guarantee the user cannot rely on is worse than no guarantee: it moves the
failure from the type checker to production. Each track first makes the build
agree with what the docs already say, then adds the annotation that lets a user
state the property.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

A Flux programmer should think of each guarantee in three layers:

1. **Inferred.** The compiler works the property out for every function and
   shows it: in hover, in `--dump-aether`, and in the inferred effect row.
2. **Promised.** An annotation (`@fip`, `total`, an opaque type, a one-shot
   operation) states the property. If the body breaks it, the build fails with
   a registered error code that says which line broke it and why.
3. **Escaped.** Each guarantee has a visible way out: `@fbip(n)` for bounded
   allocation, handling `Exn` explicitly, a smart constructor that returns
   `Option`, a multi-shot operation. The escape is in the type or in the source,
   never silent.

The four tracks never ask the user to write a lifetime, a region or a proof
term. The inference does the work, and a user who writes no annotation sees no
new errors, except where T2 makes an existing crash visible as `Exn`.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Order and dependencies

```
T1 checked fip ──────────────► (independent; builds on 0189 / 0193)
T2 partiality as effects ─────► T3 total stdlib uses `with Exn`
T3 opaque types ──────────────► T4 Scope / Channel / Task become opaque
```

- **T1 first.** The plumbing exists, the fix is mostly moving one call site and
  correcting the analysis, and it lands next to the Aether work in flight
  (0189 F3-3 and F3-5, KI-164).
- **T2 second.** It is the largest change: two effect checkers, both must change
  together (the HM rows in `src/ast/type_infer/` and the set-based inference in
  `src/compiler/mod.rs`).
- **T3 third.** The stdlib audit needs T2's `Exn` to give partial functions an
  honest type where `Option` would be awkward.
- **T4 last.** Its static half needs T3's opaque types so that `Scope(n)` cannot
  be forged. Its runtime half (making `scope` join or cancel) does not depend on
  anything and can land at any time.

### Overclaims to fix by hand

These texts describe a guarantee the build does not check. They are guide text,
example comments or diagnostic messages, so the maintainer should correct
them by hand when the matching track lands, or earlier:

| Text | Claim | Track |
|---|---|---|
| `docs/guide/20_async_and_fibers.md:326` | "no orphan fibers" | T4 |
| `docs/guide/20_async_and_fibers.md` §9 (around line 452) | scoped tasks are cancelled when the scope exits | T4 |
| `examples/guide_async/09_scope_fork_workers.flx:15` | children are awaited when the body returns | T4 |
| `runtime/c/effects.c:654` (E1201 text) | the VM enforces one-shot continuations | T4 |
| `docs/proposals/0171_effect_system_polish_and_hardening.md` (lines 490-598) | the VM enforces one-shot with E1009 | T4 |
| `docs/internals/aether.md:282` | annotated functions are checked after Aether | T1 |
| `lib/Flow/NonZero.flx` header | non-zero "at the type level" | T3 |
| `docs/proposals/0135_total_functions_and_safe_arithmetic.md:3` | Phase 2 open | T2 (updated by this work) |

### Shared rules for every track

- Each new check gets a registered error code (`src/diagnostics/compiler_errors.rs`
  plus `src/diagnostics/registry.rs`, see `docs/internals/error_codes.md`), never
  `make_error_dynamic("E999", ..)`.
- Each check has a parity fixture that passes on both backends, and a
  compiler-error fixture with `//~` annotations (0191).
- Each track that touches Aether ownership or effect checking is a soundness
  area: `cargo xtask verify` runs before merging.

## Drawbacks
[drawbacks]: #drawbacks

- T2 changes the inferred type of every function that divides. Code that
  annotates `with IO` and divides will need `Exn` in its row or a handler. That
  migration is the main cost of the whole program.
- Four tracks at once spread effort. The order above exists so that each one
  finishes before the next starts.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- **A borrow checker.** Rejected: Aether already decides ownership, and Flux has
  no mutable references to check.
- **Refinement types with an SMT solver** (0135 Phase 4, Liquid Haskell).
  Deferred: opaque types plus smart constructors cover `NonZero` and `NonEmpty`
  with no solver and no new syntax.
- **Full termination checking** (Agda, Idris). Rejected: it refuses too many
  ordinary programs. T2 tracks possible divergence as an effect instead, as Koka
  does.
- **Doing nothing but fixing the docs.** This would remove the overclaims, but
  leave users with no way to state these properties.

## Prior art
[prior-art]: #prior-art

- Koka: `fip`/`fbip` (Lorenzen, Leijen, Swierstra, "FP²: Fully in-Place
  Functional Programming", ICFP 2023); the `exn` and `div` effects and `total`.
- Rust: `Send`/`Sync`, the model for guarantees inferred structurally and
  checked at the boundary.
- Haskell: `ST`'s rank-2 region variable, the model for T4's scoped handles;
  `Data.List.NonEmpty`.
- OCaml 5: one-shot continuations by default, with a runtime error on a second
  resume.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- Does `total` appear in hover output for every function that has it, or only
  when it differs from what the user wrote?
- Should a guarantee break across a module boundary (an imported function
  loses `@fip` in a new version) be a compile error in the importer, or a
  warning there and an error in the exporter?

## Future possibilities
[future-possibilities]: #future-possibilities

- A `flux guarantees` report listing, per module, which functions are fip,
  total and so on, for library authors.
- Guarantees recorded in module interfaces (`.flxi`), so a dependency cannot
  silently drop one.
