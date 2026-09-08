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

Measured cost of landing it as things stand: **16 tests that assert an
optimisation still fires** — superinstruction fusion
(`compile_function_fuses_add_locals`, `superinstruction_tests`),
`DropSpecialized` elimination for `IntRep` binders (`aether_core_regressions`),
`ir_pipeline_tests`, `tail_call_tests` — plus 25 snapshots. Those tests are
guarantees, not noise; landing the rule now means dismantling them.

This is the trade GHC makes, and the answer it reaches: generalize first,
recover the performance with a specialisation pass, rather than refuse to
generalize. Flux should do the same — in that order.

## Stages

| stage | change | exit |
|---|---|---|
| 1 | A specialisation pass over `core/`: clone a constrained function at each concrete instantiation and rewrite that call site to the clone | a constrained helper called only at `Int` lowers to `IAdd` again; the 16 optimisation tests pass with the *current* generalization rule unchanged |
| 2 | Land generalize-by-arity (`monomorphism_restriction` by arity in `finalize_and_bind_function_scheme`) | `fn identity(x) { x }` types at two types; the same 16 tests still pass, **without being weakened**; suite + parity green |
| 3 | Bump `CACHE_EPOCH` | a stale artifact cannot hold the monomorphic lowering |

Stage 2 is a two-line change — it was written and reverted, and the revert
commit says where. Stage 1 is the work.

## Open questions

- **Where specialisation runs.** After `dict_elaborate` (specialise a
  dictionary-passing call whose dictionary is a known global) or before it
  (specialise on the solver's evidence, which 0186 stage 5 makes available)?
  The second is more direct and depends on 0186 landing first.
- **Code size.** Cloning per instantiation is exponential in the worst case.
  GHC bounds it with `SPECIALISE` pragmas and a size threshold; Flux has
  neither yet.
- **What stays polymorphic.** A function passed as a value has no single
  instantiation to specialise on — see
  [KI-090](../known_issues.md#ki-090) — so the dictionary-passing path has to
  remain as the fallback, not be replaced.
