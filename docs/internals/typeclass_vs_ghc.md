# Type classes and generics: Flux against GHC

A structural comparison of Flux's class/generics machinery with GHC's, read from
the GHC source at `2ca87972f6`. The question it answers is not "is Flux like
Haskell" but "where does Flux's architecture make correctness accidental rather
than structural".

Scope: HM inference and generalization, the constraint solver, instance lookup,
evidence and dictionary elaboration. It does **not** cover effects, Aether, or
the native backend.

## The pipeline, side by side

| stage | GHC | Flux |
|---|---|---|
| collect | `tcMonoBinds` under `pushLevelAndCaptureConstraints` | `InferCtx` walks the AST, pushing `WantedClassConstraint`s |
| generalize | `simplifyInfer` → `decideQuantification` | `finalize_binding_scheme` → `collect_scheme_constraints` |
| solve | `solveWanteds` → `simplify_loop` to a fixpoint | `solve_wanted_tree`, one pass |
| default | `tryDefaulting` → `disambigProposalSequences` | `build_numeric_default_subst` |
| evidence | solver emits `EvBind`; desugarer uses it | solver emits `Evidence`; **elaboration ignores it** |
| elaborate | desugar `EvBind` to Core dictionaries | `dict_elaborate` re-resolves instances from scratch |

## Findings

### F1 — Generalization is keyed on annotation, not on arity

GHC generalizes *every* binding unless the monomorphism restriction applies, and
the MR is keyed on **arity**: `restricted_match mg = matchGroupVisArity mg == 0`
— "No args => like a pattern binding; Some args => a function binding"
(`checkMonomorphismRestriction`, `GHC/Tc/Gen/Bind.hs`). Note [When the MR
applies] states it directly: a binding is restricted if it is a pattern binding,
or a `FunBind` **with no arguments**. So `f x = x * x` is generalized to
`Num a => a -> a` with no signature written.

Flux inverts the rule. `finalize_and_bind_function_scheme` generalizes only when
the author wrote type parameters; everything else is `Scheme::mono`, whatever its
arity. A generic helper is therefore monomorphic unless annotated, its class
obligations are never consumed by a scheme, and its type variable is shared
across every call site — so the program typechecks only when some call site
happens to pin that variable to a concrete type.

This is the root of Proposal 0183's residue. It is also *not* the monomorphism
restriction: Flux has a separate, correct MR analogue in
`GeneralizationMode::NestedBinding`, which stops a nested `let` from acquiring a
dictionary parameter it could never be passed.

The GHC-shaped rule would be: generalize every definition with parameters; apply
the restriction to nullary bindings only.

### F2 — Instances are resolved twice, by two independent implementations

In GHC the solver *produces* the evidence: `matchGlobalInst` returns a
`ClsInstResult`, the solver records an `EvBind`, and the desugarer emits the
dictionary from that binding. There is exactly one resolution, and the term the
program runs is the one the solver justified.

In Flux the solver produces `Evidence` (`FromInstance`, `Structural`,
`FromGiven`, `Marker`) — and `dict_elaborate` never reads it. Searching the
elaboration pass for `Evidence` returns nothing; it calls
`resolve_dictionary_ref_by_id` and walks `class_env.instances` itself.

So instance selection happens twice, in two code paths, with no mechanism
keeping them in agreement. The solver does produce real evidence —
`Evidence::FromInstance { instance, subst, context }`, with `Unrecorded` only
for a cycle in the instance-context graph — so there is something substantive to
consume. `Evidence::Unrecorded`'s own doc comment anticipates consuming it ("so
dictionary elaboration can stop re-resolving"); it has not been done.

**Two caveats, both material to whether this is worth doing first.**

*Causation is inferred, not established.* It is tempting to read KI-052 (a
second dictionary shadowing the first), KI-077 (superclass evidence from the
wrong dictionary) and "stop picking the last dictionary when a call reveals
nothing" as symptoms of the split. They are certainly bugs in elaboration's own
selection logic. Whether the solver would have chosen correctly in each case —
and so whether consuming its evidence would have prevented them — has not been
checked.

*The plumbing does not exist.* `elaborate_dictionaries` receives
`(CoreProgram, ClassEnv, TypeEnv, Interner, next_id)`. `SolveOutcome` and its
dispositions never reach `src/core/` at all. Evidence is attached to wanted
constraints carrying AST spans; elaboration works on Core binder ids, after
lowering. Connecting them needs a correspondence between the two that is not
currently kept anywhere, so this is a design task, not a swap.

### F3 — Typing is completed by unification where it should be completed by a constraint

GHC types every overloaded construct with a predicate. Even record field access
is a class: `hasFieldClassKey → matchHasField` in `matchGlobalInst`, solving
`HasField x r a` with the functional dependency `x r -> a`.

Flux repeatedly does the opposite — allocate a hole, and let a later unification
fill it:

- `record.field` on an unknown receiver allocated a fallback variable (fixed by
  Proposal 0184 Stage 1);
- `pair.0` constrains the receiver to a tuple shape so that "later call-site
  unification [can] discharge local helper projections";
- a `match` whose arms disagree binds each arm against a fresh variable "that
  unifies with anything" (fixed, KI-080);
- `+` carried a hard-coded `String` case at emission with no predicate (fixed by
  `Flow.Add`).

Each works only while the enclosing definition stays monomorphic, because
call-site unification is the delivery mechanism. That is why F1 and F3 are
locked together: definitions cannot generalize while the holes need call-site
unification, and the holes survive because nothing generalizes.

### F4 — The solver verifies; it does not solve

`solveWanteds` runs `simplify_loop` to a fixpoint, re-running while unification
happened or superclasses were expanded, with `check_limit` raising
`TcRnSimplifierTooManyIterations`. Solving *unifies*: that is what the inert set
and kick-out are for, and it is what makes improvement, functional dependencies
and equality constraints possible.

Flux's `solve_wanted_tree` is a single traversal returning `Vec<Disposition>`.
Its signature takes `&ClassEnv` and `&Interner` and returns no substitution: it
cannot unify, so solving one predicate can never inform another. A predicate
over an unresolved variable can only be deferred, never *resolved* by solving.

The one place Flux does solve-then-unify is 0184's `discharge_field_predicates`,
which unifies the field-type argument on discharge. That is the shape the rest
of the solver would need in order to grow.

### F5 — A second, first-argument instance lookup runs during inference

*(Corrected. An earlier version of this section claimed class-method calls
**dispatch** on the first argument. That is wrong: the obligation is emitted
from the argument types by `class_method_predicate_args`, which is tried first,
and the dictionary is selected in elaboration.)*

What `resolve_method_call_instance_from_first_arg` actually feeds is
`propagate_resolved_class_call_effects` — it resolves the instance in order to
read its *effect row* and to record a dispatch target for LSP navigation. It is
a third instance lookup, alongside the solver's and elaboration's, and it is
first-argument-only.

That is still worth recording, because it is where KI-081 came from: the lookup
instantiated the instance method's scheme for its effect row, and emitting that
scheme's constraints put the instance context on variables nothing binds. But it
is a narrower fault than "dispatch is wrong", and the ranking below is adjusted
accordingly.

### F6 — No ambiguity check

GHC rejects an ambiguous type at the definition. Flux has no equivalent, which
is Example A of Proposal 0183: `let d = zero()` with two `Default` instances
reaches run time and fails with `E1009` at a line-0 span. This is R6b's target
and depends on F1 being fixed first — escalating a residue produced by F1 would
report correct programs.

## What Flux already gets right

Worth stating, because it decides what the fixes should preserve.

- **Class identity.** `ClassId = (ModulePath, Identifier)` is module-qualified,
  so two same-named classes in different modules are genuinely distinct through
  mangling, dictionaries and interfaces. GHC uses `Unique`s for the same purpose.
- **Dispositions.** `Disposition` has no "dropped" variant, and `SolveOutcome`
  guarantees one disposition per wanted. That invariant is what made 0183
  measurable at all.
- **THIH vocabulary, faithfully applied.** `Generalized`/`Stuck` are the two
  halves of `split`; `Evidence::FromInstance` keeps the instance context as
  subgoals rather than collapsing to a boolean; `FromGiven` carries the
  superclass path.
- **Quantified-predicate selection matches GHC.** `collect_scheme_constraints`
  keeps a predicate that mentions a quantified variable, then reduces by
  superclasses — precisely `pickQuantifiablePreds` + `mkMinimalBySCs`.
- **Superclass evidence** occupies a leading dictionary slot, projected by path,
  as in GHC's dictionary representation.
- **Coherence by prohibition.** Flux rejects duplicate instances (E443) and has
  no overlapping-instance mechanism, so it avoids the whole
  `Note [Rules for instance lookup]` specificity apparatus. A defensible
  simplification, not a gap.

## Priority

Ordered by *confidence in the finding*, which is not the same as ordering by
value:

1. **F1** — generalize on arity, not annotation. The best-evidenced finding
   here: read straight from `checkMonomorphismRestriction` and Note [When the MR
   applies]. Blocked today by KI-083.
2. **F3** — continue converting deferred-unification constructs to predicates.
   Four instances observed, two already fixed; 0184 is the template and tuple
   projection is the next candidate.
3. **F6** — the ambiguity check, once F1 lands.
4. **F2** — unify the two instance resolutions. Real and worth doing, but the
   evidence→elaboration correspondence has to be designed first, so the cost is
   unknown and could exceed F1's. Do not schedule it ahead of F1 on the strength
   of this document alone.
5. **F4/F5** — only worth it if Flux wants improvement, fundeps, or a single
   instance lookup shared with the effect-row machinery. Both are large.
