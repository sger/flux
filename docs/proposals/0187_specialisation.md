# 0187 — Specialisation, and generalize-by-arity on top of it

**Status:** stage 1 partly shipped (2026-09-10), stages 2–3 open ·
**Supersedes:** [0185](0185_generalize_by_arity.md) stage 3 and
[0186](0186_generics_foundations.md) stage 6 · **Depends on:** 0186 stages 1–5

> **Amended 2026-09-11.** Stage 1 landed as `4f45fb70` and **did not do what
> this proposal described**. The difference is the finding, not an
> implementation detail, and both the stage and the answered open question
> below are rewritten to match. Anyone building stage 1's remainder from the
> original text would put the pass in a place it provably cannot work.
>
> 0185 stage 3, which this proposal supersedes, has also since half shipped:
> the **unconstrained** case is G5, in 0.0.7. What stage 2 below still owes is
> the constrained case.

## Why this is a separate proposal

Generalize-by-arity — a function with parameters gets a scheme, whether or not
its author wrote `<a>` — was implemented on the 0186 branch and reverted on
2026-09-07. **It works.** `fn identity(x) { x }` types at three types,
`fn double(x) { x + x }` at `Int` and `Float`, and the `E490` the 0185 attempt
lost is prevented structurally by field-predicate pinning, which shipped in
0186 stage 4.

What blocks it is not correctness. An unannotated helper becomes *constrained*,
and a constrained function cannot be lowered to specialised code:

```
- let %t331:Int = IAdd(acc#621, h#622)      // epoch 45
+ let %t331     = Add(acc#621, h#622)       // dictionary-dispatched
```

Aether loses in-place reuse along with the concrete type. `my_filter` in
`tests/aether/fixtures/verify_aether.flx`:

```
- Dups: 0  Drops: 1  Reuses: 1  DropSpecs: 0  FBIP: fip      FreshAllocs: 0
+ Dups: 1  Drops: 0  Reuses: 1  DropSpecs: 1  FBIP: fbip(1)  FreshAllocs: 1
```

Cost of landing it as things stand, **re-measured 2026-09-09**: of the 13
failures, **4** are the lost optimisation and **9** are missing or miscounted
dictionaries, which specialisation does not touch.

The original figure here — "16 tests that assert an optimisation still fires",
covering superinstruction fusion (`compile_function_fuses_add_locals`,
`superinstruction_tests`), `DropSpecialized` elimination for `IntRep` binders
(`aether_core_regressions`), `ir_pipeline_tests` and `tail_call_tests` — was an
overcount. It attributed every failure to despecialisation without separating
them, so it made specialisation look like the whole answer.

It is not: **specialisation is necessary but not sufficient**. The 9 dictionary
failures are the evidence-plumbing gap that 0186 stage 5 closes, so Track C has
to land before stage 1 here, not merely alongside it. The 4 that are genuinely
about lost optimisation are the ones stage 1 must fix, and they are what its
exit condition should name. Those 4 are guarantees, not noise.

This is the trade GHC makes, and the answer it reaches: generalize first,
recover the performance with a specialisation pass, rather than refuse to
generalize. Flux should do the same — in that order.

## Stages

| stage | change | exit |
|---|---|---|
| 0 | **0186 stage 5 — one evidence-passing translation** (tracked as Track C). Not part of this proposal, but its predecessor: 9 of the 13 failures are dictionaries, not lost optimisation, and no amount of specialisation reaches them | the forwarding reproduction in 0186 compiles; the six re-resolution sites are gone |
| 1 | **Partly shipped 2026-09-10 (`4f45fb70`), and not as a `core/` clone — see below.** A definition whose call sites all agree on **one** fully concrete instantiation has its body lowered under that substitution, **in place**: no clone, no call-site rewriting, no arity change. Remaining: the *two or more* instantiations case, which is where cloning is actually owed ([KI-098](../known_issues.md#ki-098)) | shipped half: the **4** optimisation tests pass with the generalization rule unchanged. Remainder: a helper used at two types keeps its representation |
| 2 | Land generalize-by-arity (`monomorphism_restriction` by arity in `finalize_and_bind_function_scheme`) — **the constrained case only**; the unconstrained one shipped as G5 in 0.0.7 | `fn double(x) { x + x }` types at `Int` and `Float`; the same 4 still pass, **without being weakened**; suite + parity green |
| 3 | Bump `CACHE_EPOCH` | a stale artifact cannot hold the monomorphic lowering |

### Stage 1 as shipped, and why the original design could not work

The table above used to say *"a specialisation pass over `core/`: clone a
constrained function at each concrete instantiation and rewrite that call site
to the clone"*, and the open question below placed it at Stage 0.6, after
`dict_elaborate`. Implementing it established two things that invalidate both.

**It cannot live in `core/` at all.** A `core/` pass cannot see a call site's
instantiation: `CoreExpr` carries no `ExprId` — zero references in
`src/core/mod.rs` — and a `CoreBinder` carries only a `FluxRep`. The type lives
in `hm_expr_types`, which is live during AST → Core lowering and gone
afterwards. That is the same constraint that puts Track C's emitter at lowering
rather than inside `dict_elaborate`, and it is structural rather than
incidental.

**And the despecialisation it was written for is not the only kind.** G5
introduced a *second* one this proposal did not anticipate. The four
optimisation tests are **class-free** — `fn copy_head(xs) { match xs { [h | t] ->
[h | [h | t]], _ -> [] } }` has no dictionary to clone on. Generalizing it
replaces a concrete parameter type with a variable, a variable carries no
`FluxRep`, `IntRep` is lost, and Aether must emit a `DropSpecialized` it had
proved unnecessary. A pass keyed on "a known-global dictionary argument" reaches
none of them.

Three things stage 1 had to get right, recorded because each was a live failure:

- **Self-calls are not instantiations.** A recursive occurrence is at the
  definition's own type by construction; counting it excludes every recursive
  function, which is half the failing tests.
- **No `TypeEnv` dependency.** Reading the function's scheme works in the
  compiler and does nothing under `lower_program_ast` (`type_env: None`) — the
  optimisation would silently not happen on any path without a scheme table.
  The generic side is recovered from parameter occurrences in the body instead.
- **Constrained functions are excluded.** Specialising one rewrites body types
  that several sites read to decide which instance a method call means, while
  its dictionaries stay positional and fixed by its signature. Verified failure:
  `result_directed_two_dictionaries.flx` printed `7` for a `String`.

Stage 2 is a two-line change — it was written and reverted, and the revert
commit says where. Stage 1 is the work.

## Open questions

- ~~**Where specialisation runs.**~~ **Answered twice. The first answer was
  wrong; the second is load-bearing.**

  *Answered 2026-09-09 — superseded:* after `dict_elaborate`, at Stage 0.6 of
  `run_core_passes_with_class_env`, cloning on a known-global dictionary
  argument. This was recorded one day before the implementation disproved it.

  **Answered 2026-09-10 — during AST → Core lowering.** A `core/` pass cannot
  see a call site's instantiation (`CoreExpr` carries no `ExprId`), and the
  despecialisation that actually broke the four tests is class-free, so there is
  no dictionary argument to key on. Stage 0.6 is not a placement this pass can
  have. See *Stage 1 as shipped* above.

  Two notes from the original answer still hold. Anything that rewrites call
  sites in *other* defs must be whole-program, because the simplification loop
  is `for def in &mut program.defs`. And `src/core/passes/specialize.rs` already
  exists and is unrelated — it inlines single-use wrappers — so a cloning pass
  still needs a different name.

  The narrow scoping was right and is what shipped: only a definition whose call
  sites all agree on one instantiation. The general case is stage 1's remainder.
- **Code size.** Cloning per instantiation is exponential in the worst case.
  GHC bounds it with `SPECIALISE` pragmas and a size threshold; Flux has
  neither yet.
- **What stays polymorphic.** A function passed as a value has no single
  instantiation to specialise on — see
  [KI-090](../known_issues.md#ki-090) — so the dictionary-passing path has to
  remain as the fallback, not be replaced.
