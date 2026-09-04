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
keeping them in agreement. Every "elaboration picked the wrong dictionary" bug —
KI-052 (a second dictionary shadowing the first), KI-077 (superclass evidence
built from the wrong dictionary), "stop picking the last dictionary when a call
reveals nothing" — is a symptom of that split, not an unrelated series of
mistakes. `Evidence::Unrecorded`'s own doc comment anticipates closing this
("so dictionary elaboration can stop re-resolving"); it has not been closed.

This is the highest-value structural fix available: it converts a class of bugs
into an impossibility rather than fixing them one at a time.

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

### F5 — Class-method calls dispatch on the first argument

Flux resolves a class-method call through
`resolve_method_call_instance_from_first_arg`, ahead of and independently of the
predicate. GHC has no such path: the call emits a wanted, and the instance is
whatever the solver selects.

The consequence was KI-081 — the predicate emitted from scheme instantiation was
never tied to the call's arguments, because dispatch had already been decided by
other means, leaving an unsolvable obligation over a variable nothing binds.
Dispatching on one argument is also why a class whose variable appears only in
the return position needs its own machinery (KI-015).

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

1. **F2** — make elaboration consume the solver's `Evidence`. Removes a bug
   class rather than a bug, and needs no language change.
2. **F1** — generalize on arity, not annotation. Blocked today by KI-083.
3. **F3** — continue converting deferred-unification constructs to predicates;
   0184 is the template, tuple projection the next candidate.
4. **F6** — the ambiguity check, once F1 lands.
5. **F4/F5** — only worth it if Flux wants improvement, fundeps, or
   return-position dispatch as first-class features. Both are large.
