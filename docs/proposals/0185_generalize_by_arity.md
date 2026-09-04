- Feature Name: `generalize_by_arity`
- Start Date: 2026-09-04
- Proposal PR:
- Flux Issue:

## Summary
[summary]: #summary

Bring Flux's generics and type classes into line with GHC where the
[comparison](../internals/typeclass_vs_ghc.md) found them divergent and the
divergence is a defect, in the order the evidence supports. The centrepiece is
one rule change: **generalize a definition by its arity, not by whether the
author wrote type parameters** — the Haskell monomorphism restriction, which
Flux already implements for `let`, applied the way GHC applies it. Around that
sit the two bugs that block it, the ambiguity report it unblocks, the last
construct still typed by a hole, and a sized decision on unifying Flux's four
instance-resolution phases.

This proposal supersedes the remaining items of
[0183](0183_constraint_solver_terminal_states.md) (R6, R7, and its
documentation) and is where they are tracked from now on. 0183's shipped
stages stand.

## Motivation
[motivation]: #motivation

The comparison established, from source, that Flux's class *core* is sound —
module-qualified `ClassId`, THIH-faithful dispositions, GHC-identical predicate
selection, the same dictionary model — and that its defects sit at two seams:
where inference decides what to generalize, and where the compiler decides
which instance a call uses.

**The generalization rule is inverted relative to GHC.** GHC restricts a
binding only when it has *no arguments* (`checkMonomorphismRestriction`,
`GHC/Tc/Gen/Bind.hs:804–811`); everything with parameters is generalized.
Flux generalizes only when type parameters were *written*
(`finalize_and_bind_function_scheme`, `src/ast/type_infer/function.rs:522–533`).
So every unannotated helper in `lib/Flow/*` is monomorphic, its class
obligations are never consumed by a scheme, and the program typechecks only when
a call site happens to pin the shared variable. Measured on a `print(1)`
program, whose residue is entirely stdlib: 9 obligations survive to
whole-program scope today; with the rule fixed, **0**.

**The fix is blocked by a run-time bug that predates it.** A top-level `let`
calling any constrained function fails with `E1001` on the current compiler
([KI-083](../known_issues.md#ki-083)) — six lines of annotated Flux reproduce
it. Generalizing by arity does not cause this; it makes nearly every top-level
helper constrained, so it makes the bug universal. It has to go first.

**Instance resolution is duplicated four times.** The solver produces
`Evidence` that nothing reads; `lower_ast`, `dict_elaborate` and the AST
bytecode compiler each re-derive the instance, two of them from their own copy
of `class_call_type_args`, kept in agreement by comments saying they "must stay
in lockstep". This is a structural hazard. It is *not* in this proposal's
critical path, because closing it needs a correspondence between wanted
constraints (keyed by span) and Core call sites (keyed by binder) that does not
exist and whose cost is unknown. The proposal schedules sizing it, not doing it.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

For a Flux programmer, one thing changes and one thing starts being reported.

**Unannotated functions become polymorphic.**

```flux
fn pick(a, b) { if a > b { a } else { b } }

print(pick(1, 2))          // 2
print(pick("a", "b"))      // "b"  — today this compiles only by accident
```

`pick` now has the type it always should have had, `Ord<a> => (a, a) -> a`,
and takes an `Ord` dictionary at each call. Calling it at a type with no `Ord`
instance is a compile error at the call, naming the instance — not, as today, a
type error somewhere else or a run-time panic. Nullary bindings keep their
current behaviour; that is the monomorphism restriction, and Flux already had
it right for `let`.

**Ambiguity is reported at compile time.**

```flux
class Default<a> { fn zero() -> a }
instance Default<Int>  { fn zero() { 0 } }
instance Default<Bool> { fn zero() { false } }

fn main() with IO {
    let d = zero()          // error: cannot infer which `Default` instance
                            //        `zero` refers to here
}
```

Today this reaches run time and panics with a line-0 span. It becomes an error
at the use, in GHC's words: "ambiguous type variable, arising from a use of
`zero`".

Nothing else in the surface language changes. There is no new syntax and no
flag.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

Seven stages, in dependency order. Each has an exit criterion measured the same
way: the all-codes sweep over every `.flx` program in the repository (1,305 at
time of writing, `--no-cache`, recording every diagnostic code with its count),
plus the stdlib residue on a `print(1)` program under `FLUX_STUCK_TRACE=1`. A
stage is not done until its sweep diff is exactly the set of programs it
intended to change.

### Stage 0 — land the current branch

`fix/phase1-promote-e442` carries 0183's shipped stages, 0184 Stage 1, and four
fixes, 44 commits and all green. Merge it before starting, with a merge commit
rather than a squash, so later branches rebase across real ancestry. Delete the
stale `feat/0183-terminal-constraint-states` pointer.

### Stage 1 — fix KI-083

*A top-level value def cannot call a function dictionary elaboration
synthesized.* The failing callee is the dictionary constructor
(`__dict_..._Num_Int`, one argument), whose global slot is defined but never
assigned. Established so far, and recorded in the KI so it is not re-derived:
the VM path lowers through `lower_aether_program` (not `lower_program`); Core
is correct; `bind_function_id_in_items` finds an item for the constructor;
binding a `MakeClosure` into the entry function does not fix it and
`IrProgram.global_bindings` is not read by the VM backend. The open question is
what assigns a *declared* function's global slot in the CFG path — every
`OpSetGlobal` emission is in the AST-based `statement.rs`/`expression.rs`, and
`ir_lowering.rs` special-cases `__dict_*` names to *define* symbols without
values.

Exit: the six-line reproduction prints `9`;
`tests/parity/toplevel_pure_expression.flx` runs on the VM; sweep neutral.

### Stage 2 — fix KI-082

*A generalized function masks an arity error.* `add(1, 2, 3)` reports `E430`
where `E056` belongs. Diagnostic quality only, but
`examples/diagnostics/hint_demos/function_arg_mismatch.flx` exists to
demonstrate `E056`, so Stage 3 cannot be neutral without it.

Exit: that fixture reports `E056`; sweep neutral.

### Stage 3 — generalize by arity

Replace the `type_params.is_empty()` test in
`finalize_and_bind_function_scheme` with GHC's rule: a definition with
parameters is generalized in `GeneralizationMode::Definition`; a nullary one is
not. The patch exists (`scratchpad/r6-generalize-unannotated.patch`, 174
lines) and applies cleanly; it quantifies the variables a class constraint
mentions (`generalize_constrained_vars`) rather than all free variables, a
deliberate narrowing while tuple projection (Stage 5) is still a hole.

Measured before Stages 1–2: stdlib residue 9 → 0, and exactly two programs
change — the two those stages fix.

Exit: stdlib residue **0**; sweep diff is empty; `CACHE_EPOCH` bumped, since
every unannotated constrained helper changes arity.

### Stage 4 — report inferred ambiguity

0183's R6b. With Stage 3 landed the whole-program residue is the set of
predicates over variables *inference* never resolved — ambiguity, not stranded
obligations. `Disposition` loses `Stuck`; the terminal set becomes Solved /
Generalized / Defaulted / Reported. A predicate reaching whole-program scope
over an unresolved variable is reported with its origin, which Flux already
records (`WantedClassConstraintOrigin`, five variants) — "cannot infer which
instance … arising from a use of `zero`". New code in `compiler_errors.rs`,
registered, with a docs row.

Exit: Example A is a compile error; sweep diff is exactly the programs that
were ambiguous, each moved to `examples/compiler_errors/` with a snapshot.

### Stage 5 — tuple projection as a constraint

The last construct typed by a hole. `infer_tuple_field_access_expression`
(`src/ast/type_infer/expression/access.rs:146`) constrains an unknown receiver
to a tuple shape so that "later call-site unification [can] discharge local
helper projections". Convert it on 0184's template: a solver-internal predicate
in the reserved module, discharged after inference, reported if the receiver
is never determined. Once done, `generalize_constrained_vars` can be retired
in favour of ordinary full generalization plus `growThetaTyVars`.

Exit: sweep neutral; the narrowing in Stage 3 removed.

### Stage 6 — size the instance-resolution unification

A design spike, not an implementation. Answer three questions in a short
document: what identifies a call site in Core such that the solver's
`Evidence` for it can be found (span? a new `ExprId` carried through
lowering?); which of the three derivations becomes canonical; and whether the
AST bytecode fallback (`compiler/statement.rs:2199`) can be retired so there
are two consumers rather than three. Produce an estimate and a recommendation.
Do not start the work on the strength of this proposal.

### Stage 7 — close 0183

Rewrite 0183 around the GHC study and mark it implemented: record Stage 1 and
R1–R5 as shipped, R6 as delivered by Stages 3–4 here, and close its open
questions as decided (defaulting is `Num ⇒ [Int, Float]`, verified and inert;
there is no defer flag). Move it to `docs/proposals/implemented/`.

### Measurement discipline

Three traps this work has already fallen into, recorded so they are not
repeated:

- The VM path runs `lower_aether_program`, which has its own copy of the
  def-seeding loop. Instrumenting `lower_program` traces nothing.
- The AST bytecode compiler is live as a fallback; a function may be compiled
  by either path, and each has its own class dispatch.
- `FLUX_STUCK_TRACE` counts are per compiled module. Summing them across a
  corpus counts the stdlib once per program; only the `print(1)` figure is a
  population.

## Drawbacks
[drawbacks]: #drawbacks

Stage 3 is a language change: programs that compiled by accident — a helper
used at one type that happened to pin its variable — keep compiling, but a
helper used at a type with no instance now fails at the call instead of
somewhere downstream. That is the intended tightening, and it needs the same
fallout sweep as everything else; there is no flag to keep the old behaviour.

Every unannotated constrained function gains dictionary parameters, so more of
the program runs through elaboration and lowering. Stage 6 exists because that
is the machinery with three uncoordinated copies of the same derivation;
Stages 1–2 are the two failures that surfaced when this was tried.

The proposal deliberately does not fix the resolver split. If Stage 6 finds it
cheap, it should be done before Stage 3 rather than after; the ordering here
reflects current knowledge, not a claim that the split is unimportant.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

**Unify instance resolution first.** Argued for in an earlier draft on the
grounds that Stage 3 multiplies traffic through the duplicated machinery.
Rejected for sequencing because its cost is unknown — the span↔binder
correspondence it needs does not exist — while Stage 3's cost is known (a patch
that applies cleanly and breaks two programs). Stage 6 keeps the option open.

**Require annotations instead of generalizing.** Reject unannotated
polymorphic helpers outright. Consistent with today's `E430`, but it makes an
ordinary Hindley–Milner program illegal for an implementation reason, and
`lib/Flow/*` is full of such helpers.

**Keep `Scheme::mono` and report the residue anyway.** Rejected by
measurement: the residue is the standard library, and every predicate in it is
correct code.

**Adopt a fixpoint solver.** The comparison found Flux's single-pass verifier
adequate for what Flux's classes need. A fixpoint becomes necessary for
functional dependencies or equality constraints, neither of which this proposal
introduces.

## Prior art
[prior-art]: #prior-art

- GHC's monomorphism restriction, `checkMonomorphismRestriction` and
  Note [When the MR applies] (`GHC/Tc/Gen/Bind.hs`); `decideQuantification`
  and Note [Deciding quantification] (`GHC/Tc/Solver.hs`).
- GHC's ambiguity definition, Note [The ambiguity check for type signatures]
  (`GHC/Tc/Validity.hs`): a type is ambiguous iff `g :: ty; g = f` fails.
- GHC's single-resolution evidence flow: `matchGlobalInst` → `EvBind` →
  desugarer.
- Proposal 0184, the template for converting a hole into a predicate.
- Proposal 0183, whose measurement of the residue is what showed the
  generalization rule to be the cause.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- Stage 6's answer. In particular whether `ExprId`, which HM already keys
  `hm_expr_types` by and which Core lowering already consults, is the
  correspondence the unification needs — if so the cost may be modest.
- Whether Stage 3 should apply the restriction to nullary *function*
  declarations (`fn x() { … }`) as GHC does to `f = …`, or only to `let`.
  GHC's rule is arity, so a nullary `fn` is restricted; Flux may prefer to
  treat `fn` as always-generalize. Decide by measurement on the corpus.
- Whether `growThetaTyVars` is wanted once Stage 5 lands, or whether
  quantifying every non-environment free variable is sufficient for Flux.

## Future possibilities
[future-possibilities]: #future-possibilities

If Stage 6 recommends unifying resolution, that work would also retire
`Evidence::Unrecorded` and let the solver's context evidence drive
`build_contextual_dictionary_expr`. Dictionary specialisation
(GHC's `Specialise`) becomes attractive once every helper is overloaded, and is
the natural next performance item. 0184 Stage 2 — generalizing over `HasField`
— becomes possible once Stage 3 has made generalization the norm.
