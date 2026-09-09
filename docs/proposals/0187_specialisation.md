# 0187 — Specialisation, and generalize-by-arity on top of it

**Status:** design · **Supersedes:** [0185](0185_generalize_by_arity.md) stage 3 and
[0186](0186_generics_foundations.md) stage 6 · **Depends on:** 0186 stages 1–5

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
| 1 | A specialisation pass over `core/`: clone a constrained function at each concrete instantiation and rewrite that call site to the clone | a constrained helper called only at `Int` lowers to `IAdd` again, and the **4** optimisation tests pass with the *current* generalization rule unchanged |
| 2 | Land generalize-by-arity (`monomorphism_restriction` by arity in `finalize_and_bind_function_scheme`) | `fn identity(x) { x }` types at two types; the same 4 still pass, **without being weakened**; suite + parity green |
| 3 | Bump `CACHE_EPOCH` | a stale artifact cannot hold the monomorphic lowering |

Stage 2 is a two-line change — it was written and reverted, and the revert
commit says where. Stage 1 is the work.

## Open questions

- ~~**Where specialisation runs.**~~ **Answered 2026-09-09: after
  `dict_elaborate`.** `elaborate_dictionaries` already runs whole-program at
  Stage 0.5 of `run_core_passes_with_class_env`, before the per-def
  simplification loop and before `promote_builtins`. The pass slots in at Stage
  0.6 — clone on a known-global dictionary argument, rewrite the call — and
  `promote_builtins` then sees a monomorphic body and can emit `IAdd` again.

  It must be whole-program: the simplification loop is `for def in
  &mut program.defs`, and specialisation rewrites call sites in *other* defs. It
  must also satisfy `verify_aether_contract_stage` and `core_lint_stage`. Note
  that `src/core/passes/specialize.rs` already exists and is unrelated — it
  inlines single-use wrappers — so the new pass needs a different name.

  Scope it narrowly first: specialise only a function whose call sites all use
  **one** instance. That is the common case in `lib/Flow/` and a fraction of
  general monomorphisation.
- **Code size.** Cloning per instantiation is exponential in the worst case.
  GHC bounds it with `SPECIALISE` pragmas and a size threshold; Flux has
  neither yet.
- **What stays polymorphic.** A function passed as a value has no single
  instantiation to specialise on — see
  [KI-090](../known_issues.md#ki-090) — so the dictionary-passing path has to
  remain as the fallback, not be replaced.
