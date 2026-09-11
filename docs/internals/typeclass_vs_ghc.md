# Type classes and generics: Flux against GHC

A structural comparison of Flux's class and generics machinery with GHC's.
Every claim below cites the line it was read from. Where a claim is an
inference rather than something read, it says so.

**Re-verified 2026-09-11** against GHC at `c673ecf057` and Flux at
`feat/0-0-7-generics-remainder`. The body was written on 2026-09-04 against GHC
`2ca87972f6`; the GHC citations were re-read at the newer revision and
`Bind.hs:804-811`, quoted below, is byte-identical. What moved is Flux:
**§1's headline divergence is closed and §4's remaining hole is filled**, both
by 0.0.7. The rows are rewritten in place and listed under *Corrections* at the
end.

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

**Flux** generalized a function only if the author wrote type parameters. That
was true when this section was written and is no longer: **G5 shipped in 0.0.7**
and arity now decides, as in GHC — with one extra condition GHC does not have:

```rust
fn should_generalize_function(&self, type_params, param_tys, fn_ty, constraint_start) -> bool {
    if !type_params.is_empty() {
        return true;
    }
    let unconstrained = self
        .class_constraints
        .captured_since(constraint_start)
        .is_empty()
        && !self.shares_var_with_pending_field_predicate(fn_ty);
    !param_tys.is_empty() && unconstrained
}
```
— `src/ast/type_infer/function.rs:573–589`.

`!param_tys.is_empty()` **is** GHC's `matchGroupVisArity mg == 0`, negated: a
binding with arguments is unrestricted, a nullary one is restricted. On that
test the two compilers now agree, and `fn identity(x) { x }` is usable at `Int`
and `String` with no signature.

The divergence that remains is the `unconstrained` conjunct. GHC generalizes a
binding with arguments *whatever* it raises — `f x = x * x` becomes
`Num a => a -> a`, with the dictionary passed at each call. Flux withholds
generalization from a definition that raises a class constraint, so
`fn double(x) { x + x }` is still monomorphic. That is not a different rule
about quantification; it is the absence of the machinery to pass the dictionary
a quantified constraint would require. Roadmap B2, behind the evidence
translation (Track C), on top of specialisation (B1).

So the shape of the gap has changed. It was *"Flux does not generalize"*. It is
now *"Flux generalizes exactly what needs no evidence"* — which is the larger
half of the unannotated helpers people write, and which is why the remaining
half is a dictionary-plumbing problem rather than an inference one.

The second conjunct, `shares_var_with_pending_field_predicate`, has no GHC
analogue at all, and §4 explains why.

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

**Verdict.** Was the largest divergence; now half closed. The arity test
matches GHC. What is left is constrained generalization, which is blocked on
evidence passing rather than on anything about the MR.
[KI-083](../known_issues.md#ki-083), named here as the blocker when this was
written, is fixed.

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
| `pair.0`, receiver unknown | was: constrain to a tuple shape, so call-site unification discharges it | fixed — 0185 Stage 5 emits `__tuple<R, T>` with the index on the origin |
| `match` arms of different families | each arm bound against a fresh variable | fixed — KI-080 |
| `+` at `String` | hard-coded case at emission, no predicate | fixed — `Flow.Add` |

A fallback variable is excluded from every scheme's `forall` by
`resolve_binding_schemes` (`src/ast/type_infer/mod.rs:786–810`), so it can be
filled only by unifying the enclosing definition with a call site — which is
why §1 and §4 are locked together.

**Verdict.** All four are converted, tuple projection as of 0185 Stage 5 in
0.0.7. Out-of-range and undetermined receivers are now reported (`E492`,
`E491`) instead of being widened into whatever the call site happened to
supply.

### Where the field predicate is solved, and what that costs

Converting the construct is not the whole comparison. GHC and Flux both make
field access a predicate and both *determine* the field type from the receiver;
they differ in **where** that happens, and Flux pays for the difference in §1.

GHC declares the dependency on the class:

```haskell
class HasField x r a | x r -> a where
  getField :: r -> a
```
— Note [HasField instances], `GHC/Tc/Instance/Class.hs:1161–1190`. `matchHasField`
solves a wanted by instantiating the selector and emitting an *equality*,
`[W] co : (T alpha -> [alpha]) ~# (T rty -> fty)`, which the solver's unifier
discharges. Because it is an ordinary predicate solved in the ordinary
fixpoint, it can be quantified by `pickQuantifiablePreds` like any other, and a
function that merely forwards a record to a field-accessing callee generalizes
with a `HasField` in its context.

Flux has no functional dependencies and no equality constraints in its solver,
so it cannot do that. It gets the same determination from a **post-unification
pass**:

> Runs after all unification, which is the point of doing it here rather than
> in the class solver: a receiver that any call site determines is determined
> by now … Each predicate is resolved against the receiver as it finally
> stands and unified with the field-type argument, which is what makes the
> field type propagate — GHC's functional dependency `x r -> a` on
> `HasField x r a`.

— `discharge_field_predicates`, `src/ast/type_infer/mod.rs:850–865`.

The consequence is that a field predicate **cannot be quantified**: it has no
dictionary, the solver has no rule for it, and it is discharged only once
everything is known. A definition whose type shares a variable with an
undischarged one therefore has to stay monomorphic, or the receiver inside the
callee ends up determined by nothing — which is exactly what
`shares_var_with_pending_field_predicate` withholds generalization for, and
what cost four `examples/aoc/2024/day06*` programs when G5's rule was first
written without it.

So the two designs reach the same answer for direct access and diverge on
*forwarders*. GHC quantifies the constraint and passes it along; Flux pins the
receiver. Giving Flux either fundeps or solver-level equalities would remove
the pin — which is a larger change than it looks, and is why the table below
now lists fundeps as *load-bearing elsewhere* rather than simply absent.

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
but no report is produced. This is 0183's R6d, gated on §1 because the residue
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
| when to generalize | every binding; MR on nullary | **same arity test since G5**; plus a no-constraint side condition | converged on arity; constrained half is B2 |
| which predicates | mentions qtv + minimal by SC | same | match |
| grow quantified set | `growThetaTyVars` | none | gap, minor while §4 open |
| solver | fixpoint, unifies, budgeted | single pass, verifies | design difference; adequate today |
| superclass use | givens eager 1 layer; wanteds lazy, fuel | givens only, by path | adequate; no fundeps to feed |
| instance resolution | once, in solver, evidence flows | 4 phases, 3 derivations, evidence unused | **structural hazard** |
| field access | `HasField`, built-in, solved in the fixpoint | predicate since 0184, discharged in a post-pass | converted; **not quantifiable**, see §4 |
| tuple projection | ordinary typing | predicate since 0185 stage 5 | converted |
| signature ambiguity | `checkAmbiguity` | `AMBIGUOUS_TYPE_VARIABLE` | match |
| inferred ambiguity | reported with origin | stuck, then panics at run time | gap (R6d/E2); pinned by `examples/generics/failing/runtime/e2_*` |
| overlap | specificity + pragmas | prohibited | sound simplification |
| fundeps | yes; `x r -> a` carries `HasField` | no | absent — but load-bearing in GHC's §4, which is why Flux pins instead |
| dictionaries | data con, SC fields first | tuple, SC slots first | match |
| specialisation | `Specialise` | B1: single concrete instantiation | partial; KI-098 is the gap |

## 9. Priority, by confidence in the finding

Re-ordered 2026-09-11. Items 1 and 2 of the previous list are **done**:
§1's arity rule shipped as G5 and §4's tuple projection as 0185 stage 5, both
in 0.0.7.

1. **§1's remaining half — constrained generalization.** `fn double(x) { x + x }`
   at two types. Not an inference problem: the rule is already written, and what
   it waits on is a dictionary to pass. Roadmap B2, behind Track C's evidence
   translation, on top of B1.
2. **§5 inferred ambiguity.** Still the gap it was, and now pinned — the
   program reaches run time and panics with a message naming the wrong cause
   ("No instance of Def.zero", when the problem is that two matched). Roadmap
   E2, blocked on B2 for the reason 0183 gives: until generalize-by-arity is
   complete the residue is stranded obligations rather than ambiguity, so the
   report would be wrong.
3. **§3 unify instance resolution.** Unchanged in substance; 0186 stage 5
   answered its sizing question (`ExprId` plus the predicate's index at the
   site identifies a call site; a span cannot).
4. **§4's forwarder pin.** Not a defect — `shares_var_with_pending_field_predicate`
   is correct for a compiler without fundeps. It is listed because removing it
   is the only thing that would close the last structural difference in field
   access, and because the cost of *not* removing it is a class of definition
   that cannot generalize for a reason unrelated to its own constraints.
5. **§2 / §6** — only if Flux wants improvement or fundeps.

## Corrections from the 2026-09-11 re-verification

- **§1 said Flux generalizes only with written type parameters.** False since
  G5. Arity now decides, matching GHC's `matchGroupVisArity mg == 0`; what is
  withheld is the *constrained* case, and for a different reason (no evidence
  to pass) than the one this section originally gave.
- **§1 said the fix was blocked by KI-083.** That was true when written;
  KI-083 was fixed on 2026-09-05.
- **§4 listed tuple projection as the one remaining hole.** Filled by 0185
  stage 5.
- **The table called fundeps "absent, not a bug".** Accurate as far as it went,
  but it missed that GHC's `HasField` *depends* on one, which is the reason
  Flux's field predicate cannot be quantified and the reason §1's rule needs a
  side condition GHC has no use for. The two rows were treated as independent
  and are not.
- **The table called specialisation "none / performance only".** B1 landed in
  0.0.7. It is also not purely performance: G5 introduced a despecialisation
  with no dictionary to clone on ([KI-098](../known_issues.md#ki-098)).

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
