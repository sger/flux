# Type classes and generics: Flux against GHC

A structural comparison of Flux's class and generics machinery with GHC's. GHC
is read from the checkout at `2ca87972f6`; Flux from branch
`fix/phase1-promote-e442` at `98302de0`. Every claim below cites the line it
was read from. Where a claim is an inference rather than something read, it
says so.

This is a second, from-scratch pass. The first version of this document made
three errors, corrected here and listed at the end so they are not
reintroduced.

Scope: inference and generalization, the constraint solver, instance
resolution, evidence and dictionaries, ambiguity, coherence. Not covered:
effects, Aether, the native backend.

## 1. Generalization

**GHC** generalizes every binding group unless the monomorphism restriction
applies, and the MR is decided by *arity*:

```haskell
restricted (FunBind { fun_id = v, fun_matches = m }) = restricted_match m
                                                       && mr_needed_for (unLoc v)
restricted_match mg = matchGroupVisArity mg == 0
    -- No args => like a pattern binding
    -- Some args => a function binding
```
— `checkMonomorphismRestriction`, `GHC/Tc/Gen/Bind.hs:804–811`. Note [When the
MR applies] (`Bind.hs:831`) states it: a binding is restricted if it is a
pattern binding, or a `FunBind` **with no arguments** whose binder lacks a
signature that says it is overloaded. `MonomorphismRestriction` is in the
default extension sets for Haskell98, Haskell2010 and GHC2021
(`GHC/Driver/DynFlags.hs:1401, 1423, 1441`). So `f x = x * x` is generalized
to `Num a => a -> a` with no signature.

Which predicates are kept is decided by `pickQuantifiablePreds`
(`GHC/Tc/Solver.hs:1948–1975`): a class predicate is quantified iff it
*mentions* a quantified variable, then `mkMinimalBySCs` drops any predicate
another one implies through superclasses. Before that, `growThetaTyVars`
(`Solver.hs:1977`) *extends* the quantified set through the constraints, so a
variable reachable from a quantified one via a predicate is quantified too.

**Flux** generalizes a function only if the author wrote type parameters:

```rust
let scheme = if !type_params.is_empty() {
    self.finalize_binding_scheme(BindingSchemeSpec { .. mode: Definition .. })
} else {
    Scheme::mono(fn_ty)
};
```
— `finalize_and_bind_function_scheme`, `src/ast/type_infer/function.rs:522–533`.
Arity plays no part. An unannotated `fn pick(a, b) { a > b }` is monomorphic,
its `Ord` obligation is never consumed by a scheme, and its type variable is
shared across every call site.

Flux *does* have the monomorphism restriction, and has it correctly: a `let`
is generalized in `GeneralizationMode::NestedBinding`
(`src/ast/type_infer/statement.rs:179`), which keeps operator obligations out of
the scheme so a nested binding is never given a dictionary parameter no caller
would pass (`class_defaulting.rs:318`). That is precisely GHC's `ApplyMR`
("never quantifying over any constraints", `Solver.hs:919`). What Flux lacks is
the *other* branch: generalizing function bindings that have arguments.

Predicate selection matches GHC. `collect_scheme_constraints`
(`class_defaulting.rs:305–330`) keeps a predicate that mentions a quantified
variable, and `retain_minimal_by_superclasses` reduces by superclasses. Flux has
no analogue of `growThetaTyVars`; the branch's experimental
`generalize_constrained_vars` does the opposite (shrinks to constrained
variables), which is a deliberate narrowing while the holes in §4 exist.

Lambdas are not generalized in either compiler.

**Verdict.** This is the largest divergence and the best-evidenced. The
GHC-shaped rule is mechanical: generalize a definition with parameters; apply
the restriction to nullary bindings. It is blocked in Flux by
[KI-083](../known_issues.md#ki-083), a run-time bug that generalization makes
universal but did not cause.

## 2. Solving

**GHC**'s `solveWanteds` runs `simplify_loop` to a fixpoint, re-running while
unification happened or superclasses were expanded, and raises
`TcRnSimplifierTooManyIterations` when the budget is exhausted
(`GHC/Tc/Solver/Solve.hs:182, 1184`) — an error, deliberately, so an unsolved
constraint cannot suppress it. Solving *unifies*: that is what the inert set is
for, and what makes improvement and functional dependencies possible.
Superclasses are expanded eagerly for givens by one layer and lazily for
wanteds, bounded by `ExpansionFuel` (Note [The superclass story],
`GHC/Tc/Solver/Dict.hs:1438`).

**Flux**'s `solve_wanted_tree` (`src/types/class_solver.rs:89–140`) is one
traversal: a `for` over `wanted.simple` calling `classify_constraint`, then
recursion into each implication with its givens appended. Its signature takes
`&ClassEnv` and `&Interner` and returns `Vec<Disposition>` — no substitution.
It cannot unify, so solving one predicate can never inform another, and a
predicate over an unresolved variable can only be deferred.

Superclasses are used for *givens* only, through `superclass_path`
(`class_solver.rs:278`): a wanted `Eq<a>` is discharged by a given `Ord<a>`. A
wanted's own superclasses are never expanded, which GHC does for the
functional-dependency reason it explains in the note — a reason that does not
apply to Flux, which has no functional dependencies (§6).

One place in Flux does solve-then-unify: `discharge_field_predicates`
(`src/ast/type_infer/mod.rs`, added by Proposal 0184), which unifies the
field-type argument on discharge. That is the shape the solver would need if it
were ever to do improvement.

**Verdict.** Flux's solver is a *verifier*, not a solver. That is a coherent
design given that it runs after inference has already unified everything, and
it is adequate for what Flux's classes need today. It becomes a limit only if
Flux wants fundeps, equality constraints, or return-type-driven improvement
without special cases.

## 3. Instance resolution: how many times, and where

**GHC** resolves an instance once, in the solver. `matchGlobalInst`
(`GHC/Tc/Instance/Class.hs:134–154`) returns a `ClsInstResult`; the solver
records an `EvBind`; everything downstream uses that binding. `reportUnsolved`
returns the `Bag EvBind` (`GHC/Tc/Errors.hs:156`), and the desugarer builds
dictionaries from it. There is one selection and one result.

**Flux** resolves instances in **four phases**, with **three separate
implementations of the dispatch-argument derivation**:

| phase | file | sites | what it derives |
|---|---|---|---|
| inference | `src/types/class_solver.rs` | 3 | `Evidence` for each wanted |
| inference | `src/types/class_defaulting.rs` | 2 | verified default candidates |
| inference | `src/ast/type_infer/expression/calls.rs:688` | 1 | instance for effect row + LSP target |
| Core lowering | `src/core/lower_ast/mod.rs:537, 654, 705, 917` | 5 | mangled `__tc_*` callee, own `class_call_type_args` (`:568`) |
| Core elaboration | `src/core/passes/dict_elaborate.rs:1216, 316` | 2 | dictionary refs, contextual dictionaries by its own recursion |
| AST bytecode | `src/compiler/expression.rs:4763, 4799, 5153, 5376` | 4 | mangled callee, own `class_call_type_args` (`:5299`) |

The codebase knows. `lower_ast/mod.rs:528` says its derivation "mirrors the
derivation type inference uses … so lowering and the solver cannot disagree",
and `compiler/expression.rs:4755` says it "mirrors `LowerCtx::try_resolve_class_call`
exactly — the two must stay in lockstep or the VM and native backends dispatch
differently". Agreement is maintained by discipline, not by construction.

The AST bytecode path is live. `compiler/statement.rs:2199` tries the CFG
compiler first and, on a compile error, rolls back "and fall[s] through to AST
for proper diagnostics"; on `None` it also falls through, behind a
`debug_assert`. So functions can be compiled by either, and each has its own
class dispatch.

The solver's `Evidence` has no consumer. `grep -rl "Evidence::" src/` finds
only `class_solver.rs`, `class_disposition.rs` and `class_defaulting.rs`; the
solver does produce `Evidence::FromInstance { instance, subst, context }`
(`class_solver.rs:656`) with `Unrecorded` reserved for a context cycle
(`:582`), so there is real information being discarded.

**Verdict.** This is the finding I most understated before. Not two resolvers
but four phases and three derivations, maintained in lockstep by comment.
Unifying them is the right structural fix — but the correspondence between a
wanted constraint (keyed by AST span) and a Core call site (keyed by binder id)
does not exist today, so it is a design task whose cost is unknown. It should
not be scheduled ahead of §1 on the strength of this document.

## 4. What is typed by a constraint, and what by a hole

**GHC** types every overloaded construct with a predicate, including record
field access: `hasFieldClassKey → matchHasField` in `matchGlobalInst`
(`Class.hs:150`) solves `HasField x r a` with the dependency `x r -> a`.

**Flux** has, in four places, allocated a variable and relied on a later
unification to fill it:

| construct | mechanism | status |
|---|---|---|
| `record.field`, receiver unknown | `alloc_fallback_var()` | fixed — Proposal 0184 Stage 1 emits `__field.name<R, T>` |
| `pair.0`, receiver unknown | constrain to a tuple shape "so later call-site unification [can] discharge local helper projections" (`access.rs:146`) | open |
| `match` arms of different families | each arm bound against a fresh variable | fixed — KI-080 |
| `+` at `String` | hard-coded case at emission, no predicate | fixed — `Flow.Add` |

A fallback variable is excluded from every scheme's `forall` by
`resolve_binding_schemes` (`src/ast/type_infer/mod.rs:786–810`), so it can be
filled only by unifying the enclosing definition with a call site — which is
why §1 and §4 are locked together.

**Verdict.** Three of four are converted. Tuple projection is the remaining
one, and the same shape as 0184.

## 5. Ambiguity

**GHC** defines ambiguity operationally (Note [The ambiguity check for type
signatures], `GHC/Tc/Validity.hs:96–140`): a signature `f :: ty` is ambiguous
iff `g :: ty; g = f` would fail — instantiate the type and try to solve the
instantiated constraints from the originals. `checkAmbiguity`
(`Validity.hs:233`) applies it to user signatures; an *inferred* ambiguity
surfaces at the use site as an unsolved wanted, reported by `reportUnsolved`
with its `CtOrigin` ("arising from a use of …"). GHC has 53 `CtOrigin`
constructors (`GHC/Tc/Types/Origin.hs`).

**Flux** has the signature half. `class_defaulting.rs:600–625` reports
`AMBIGUOUS_TYPE_VARIABLE` for an `ExplicitBound` predicate whose type argument
is neither quantified nor free in the environment — "not determined by this
signature, so no call can select an instance for it". That is `f :: C a => Int`.
It also reports `E485` when a constrained function holds two dictionaries for a
class and the call reveals nothing to choose between them
(`DictSelection::Ambiguous`, `class_env.rs`).

What Flux lacks is the *inferred* half: `let d = zero()` with two `Default`
instances reaches run time (Proposal 0183, Example A). The predicate sits in
`Disposition::Stuck` and nothing reports it. Flux records five constraint
origins (`constraint.rs:68–74`), enough to say "arising from a use of `zero`",
but no report is produced. This is 0183's R6b, gated on §1 because the residue
it would report is today dominated by §1's stranded obligations.

**Verdict.** Narrower than "no ambiguity check": the signature check exists and
is GHC-shaped; the call-site report does not.

## 6. Coherence and instance selection

**GHC** permits overlapping instances and resolves them by specificity and
pragma (IL1–IL5, Note [Rules for instance lookup], `GHC/Core/InstEnv.hs:592–668`),
and lets a local given override an instance unless `IncoherentInstances` (IL0).
It has functional dependencies, associated types, and multi-parameter classes.

**Flux** rejects a duplicate instance outright (`DUPLICATE_INSTANCE`,
`class_env.rs:1858`) and has no overlap mode; `candidate_instances_by_id`
(`class_env.rs:2500`) collects matches with no specificity ordering. It has an
orphan rule (Proposal 0151, `class_env.rs:341`), multi-parameter classes,
associated types (`InferType::Assoc`, reduced by `normalize_associated_types`),
and **no functional dependencies** (the only mentions in `src/` are 0184's
comments citing GHC's). Givens are consulted before instances in
`classify_constraint`, which is IL0's ordering.

**Verdict.** Coherence-by-prohibition is a sound simplification, not a gap: it
avoids the whole IL3 apparatus and makes "at most one instance per head" a
usable invariant (which `DictSelection` relies on).

## 7. Dictionaries

**GHC**: a class is a data type (`classTyCon`, `GHC/Core/Class.hs:61`) whose
constructor's fields are the superclass dictionaries followed by the methods,
with selector `Id`s for both (`Class.hs:147–152, 297–303`).

**Flux**: a tuple whose leading slots are the *directly declared* superclasses
and whose remaining slots are the methods, in declaration order
(`dictionary_layout`, `class_env.rs:395–417`). A transitive superclass is
reached by projecting twice, which keeps a class's layout independent of the
hierarchy above it. Same model.

Flux has no dictionary specialisation. `src/core/passes/specialize.rs` is
"specialize trivial known wrappers" — inlining single-use pure `let`s — not
GHC's `Specialise`, which clones an overloaded function at a known dictionary.
A cost, not a correctness issue.

## 8. Summary table

| area | GHC | Flux | verdict |
|---|---|---|---|
| when to generalize | every binding; MR on nullary | only with written type params | **divergent, fix well-defined** |
| which predicates | mentions qtv + minimal by SC | same | match |
| grow quantified set | `growThetaTyVars` | none | gap, minor while §4 open |
| solver | fixpoint, unifies, budgeted | single pass, verifies | design difference; adequate today |
| superclass use | givens eager 1 layer; wanteds lazy, fuel | givens only, by path | adequate; no fundeps to feed |
| instance resolution | once, in solver, evidence flows | 4 phases, 3 derivations, evidence unused | **structural hazard** |
| field access | `HasField`, built-in | predicate since 0184 | converted |
| tuple projection | ordinary typing | tuple-shape hole | open |
| signature ambiguity | `checkAmbiguity` | `AMBIGUOUS_TYPE_VARIABLE` | match |
| inferred ambiguity | reported with origin | stuck, silent | gap (R6b) |
| overlap | specificity + pragmas | prohibited | sound simplification |
| fundeps | yes | no | absent, not a bug |
| dictionaries | data con, SC fields first | tuple, SC slots first | match |
| specialisation | `Specialise` | none | performance only |

## 9. Priority, by confidence in the finding

1. **§1 generalization** — read directly from GHC's MR code and Flux's
   `finalize_and_bind_function_scheme`. Highest confidence, largest effect.
   Blocked by KI-083.
2. **§4 tuple projection** — one construct left, the 0184 template applies.
3. **§5 inferred ambiguity** — after §1.
4. **§3 unify instance resolution** — real, but its cost depends on a
   span↔binder correspondence that has to be designed. Size it before
   sequencing it.
5. **§2 / §6** — only if Flux wants improvement or fundeps.

## Corrections to the first version of this document

- It said class-method calls *dispatch* on the first argument. They do not:
  the obligation is emitted from the argument types
  (`class_method_predicate_args`, tried first), and dictionaries are selected in
  lowering and elaboration. The first-argument lookup in `calls.rs:688` feeds
  the effect row and the LSP dispatch target.
- It said instances are resolved "twice". Four phases, three derivations.
- It said Flux has "no ambiguity check". It has the signature check
  (`AMBIGUOUS_TYPE_VARIABLE`) and `E485`; it lacks the inferred/call-site
  report.
- It asserted that KI-052 and KI-077 were consequences of the split in §3. That
  was inference; it is not established that the solver would have chosen
  correctly in those cases, and the claim is withdrawn.
