# Type classes: how GHC does what 0.0.8 has to do

**Written 2026-09-23** against GHC commit `bf17f289eb` (2026-09-07, upstream
`gitlab.haskell.org/ghc/ghc`) and Flux `main` at `3f123c02`. GHC paths are
relative to `compiler/GHC/`.

**Purpose.** [0_0_8_plan.md](../roadmaps/0_0_8_plan.md) rebuilds how Flux
passes class evidence. This document reads GHC's implementation of the same
four problems and says, phase by phase, what Flux should copy, what it should
deliberately do differently, and what the plan should change as a result.

The existing [typeclass_vs_ghc.md](typeclass_vs_ghc.md) is a broader comparison
(solving, coherence, field access). This one is narrower and deeper: it follows
the evidence from the solver to the call site.

---

## Summary

| # | Finding | Plan impact |
|---|---|---|
| 1 | GHC attaches evidence to the **occurrence** of an overloaded name, not to the call. `f x` and a bare `f` get the same wrapper. | Phase 1: emit dictionaries where the *identifier* is lowered, so calls, values and operators share one emitter. |
| 2 | Operators are desugared to ordinary applications of the method variable before instantiation. | KI-076 is the operator path not sharing the identifier path. Phase 1 should route it through the same emitter. |
| 3 | A dictionary parameter is a *given*; the body's wanteds are solved from it. A method call inside a constrained body is just more evidence, with a superclass path. | Phase 1d: resolve method calls in a constrained body from evidence (`FromGiven` + path). This deletes `choose_candidate`. |
| 4 | A default method is an **ordinary exported function** `$dmop` taking the class dictionary. An instance that omits the method gets `op = $dmop @T`. | Phase 3d (KI-086): compile defaults once as exported functions. Do not carry `default_body` through `.flxi`. |
| 5 | Instance-method binders are looked up **among the class's methods**, never in scope. | Phase 3c (KI-071) is confirmed: lowering must not re-resolve names. |
| 6 | Quantification is decided **once per binding group**. Each member's type is then cut down to what its own type reaches. | Phase 2c follows GHC's shape directly. |
| 7 | Type-class defaulting happens **only at the top level**, never during inference. | Flux defaults in two places: lowering (the `Int` default, deleted in 1f) and `decide_quantification` (`build_numeric_default_subst`). The second is new; see §6. |
| 8 | Ambiguity is reported with the constraint's **origin**, and only when the variable is still unknown and some instance could match. | Phase 4a gets a concrete message format and conditions. |
| 9 | GHC specialises **only on dictionaries**, redirects callers with rewrite rules, and has no numeric cap. | Phase 2a: key clones on dictionaries *and* representation-relevant types, rewrite call sites directly, and borrow SpecConstr's cap of 3. |

---

## 1. Evidence reaches the call site without being re-derived — Phase 1

### How GHC does it

When the typechecker instantiates an overloaded name, it does two things at once
(`Tc/Utils/Instantiate.hs:357-380`, `instCall` / `instCallConstraints`):

1. It emits a *wanted* constraint `C τ` whose evidence destination is a fresh
   variable `d` (`Tc/Utils/TcMType.hs:256-260`, `emitWanted`).
2. It wraps the occurrence in `WpEvApp (Var d) <.> WpTyApp τ`. The result is
   stored on the tree as `XExpr (WrapExpr wrap (HsVar f))`
   (`Hs/Expr.hs:681-683`, built by `mkHsWrap`, `Hs/Utils.hs:803-805`).

The same variable `d` appears in both, and that is the entire link. The solver
later writes exactly one binding `d = <term>` into a mutable binding store that
is already on the tree (`EvBindsVar`, `Tc/Types/Evidence.hs:734-744`; written by
`setWantedDict`, `Tc/Solver/Monad.hs:2022-2034`).

- Instance evidence is a Core expression: the instance function applied to
  sub-dictionaries, `$fOrdList @Int $fOrdInt` (`evDFunApp`, `Evidence.hs:946-952`).
- Superclass evidence is a selector application: `$p1Ord @a d`
  (`Tc/Solver/Dict.hs:1795`).

The desugarer never searches for an instance. `dsEvTerm (EvExpr e) = return e`
(`HsToCore/Binds.hs:1751-1759`) is the identity, and `ds_hs_wrapper`
(`Binds.hs:1622-1650`) maps each wrapper form to one Core form:

| wrapper | becomes |
|---|---|
| `WpTyApp` | type application |
| `WpEvApp` | an application to the dictionary |
| `WpEvLam` | a lambda over the dictionary |
| `WpLet` | `let` |

At a call site, `splitHsWrapperArgs` (`HsToCore/Expr.hs:938-950`) flattens the
wrapper's arguments into the call, so `f @T $dC x` is one application.

### What Flux has

Flux's solver records the same answers, but in a side table:
`EvidenceMap { by_site: HashMap<EvidenceSite{expr, index}, Evidence> }`
(`src/types/evidence.rs:32-45`). A table keyed by `ExprId` is equivalent to
GHC's wrapper **provided the reader looks it up at the node that raised it**. The
survey found two facts about which node that is:

- A constrained function's predicates are raised by
  `infer_identifier_expression` (`expression/mod.rs:24`), so they are keyed at
  the **callee identifier**, exactly like GHC's wrapper around `HsVar`.
- Operator predicates are raised under the operator expression's id, with origin
  `InferredOperator` (`expression/operators.rs:143`).

Lowering does not read the table. It re-derives dictionaries on three separate
`Call` paths (`lower_ast/expression.rs:173-242`), and `dict_elaborate` then
re-derives them again.

### Copy

- **Read evidence where the identifier is lowered, not where the call is
  lowered.** The Phase 1 emitter belongs in the lowering of an identifier
  occurrence: it produces `f` applied to its dictionaries. The `Call` lowering
  then flattens that into a single call, `f(d1, …, x1, …)`, the same move as
  `splitHsWrapperArgs`.
  - With one emitter, three cases that are separate paths today become the same
    case: a call, a bare reference passed as a value (KI-090), and an operator
    (KI-076).
- **Evidence is already a target term.** `DictArg`
  (`src/types/translate.rs:32`) is Flux's `EvTerm`. Rendering it to Core
  (`Global` → a `__dict_*` reference, `Applied` → an application, `Param` → a
  variable plus `TupleField` projections) must be a pure function with no
  lookups. That is the Flux equivalent of `dsEvTerm`.
- **One variable, one binding.** GHC gets "no short argument list" by
  construction: every hole is a variable, and the solver binds each one exactly
  once. Flux's equivalent is Phase 0c. `args_for` must know how many predicates
  were raised, so that a missing answer is a hole rather than a shorter list.

### Do differently

- **Keep the side table.** Moving evidence onto AST nodes would change the
  parser's output types for no gain. `ExprId` is stable, and a table keyed by it
  carries the same information. What must be copied is the discipline: read at
  the raising node, never recompute.
- **No partial application.** GHC's `dbl @Int $dNumInt` is a partially applied
  Core term. Flux's backends do not support partial application of a known
  function (proposal 0052 is *Not Implemented*). A bare constrained reference
  therefore still has to become `λx. dbl(d, x)`. The difference from today is
  that it is built in one place, the identifier emitter, rather than as a
  special case. That construction is where KI-090's closure-conversion defect
  lives (§4).

---

## 2. Dictionary parameters and method calls inside a constrained body — Phase 1d

### How GHC does it

For an inferred binding, `simplifyInfer` (`Tc/Solver.hs:932-1040`) decides the
context `θ`. It creates **fresh given variables** for `θ` (`:1013`), then solves
the body's remaining wanteds *from those givens*. That produces bindings of the
form `d_wanted = d_param`, or `d_wanted = $p1Ord d_param` for a superclass. The
givens become the binding's dictionary lambdas through `AbsBinds.abs_ev_vars`,
and desugaring emits
`Λtvs. λdicts. let <ev_binds> in body` (`HsToCore/Binds.hs:292-295`).

A method call in the body is **not special**. `eq x y` inside `f :: Eq a => …`
instantiates the selector `eq`, which raises `[W] Eq a`, which is solved from the
given, and the call becomes `eq @a d x y`. The selector projects the method out
of the dictionary. Superclass access is the same mechanism with a selector
chain.

### Flux today

`rewrite_constrained_functions` (`dict_elaborate.rs:505`) adds dictionary
parameters in a Core pass, then resolves each method call by *searching*
candidates (`reachable_methods`, `choose_candidate`, `select_dictionary`). That
search is the second re-derivation. `choose_candidate`'s first-candidate
fallback (Phase 0d) exists because the search can fail.

### Copy

- Parameters come from the scheme's givens, in scheme order, created at lowering
  (Phase 1d as planned).
- A method call in a constrained body is resolved from its **own evidence**:
  `Evidence::FromGiven`, which becomes `DictArg::Param { index, path }`. `path`
  is the superclass selector chain, matching GHC's `$p1Ord d`. The method slot is
  then projected from the dictionary that path reaches.
  - **Check first** (add to 1b): that `harvest_evidence` records a `FromGiven`
    answer, with its path, for **method-call** predicates raised inside a
    constrained body, and not only for calls to constrained functions. If it
    does not, that is a gap in the recording, and 1d cannot delete
    `choose_candidate` until it is closed.

### Not now

GHC routes recursive calls inside an `AbsBinds` through the **monomorphic** id,
so dictionaries are passed once rather than on every iteration (Note
[Polymorphic recursion], `Tc/Gen/Bind.hs:123-181`). Flux passes them on every
iteration. It is a performance point, not a correctness one, and out of scope.

---

## 3. Default methods across modules — Phase 3d (KI-086)

### How GHC does it

A class's default method is compiled **once, in the class's module**, as an
ordinary exported function `$dmop :: forall a. C a => <method type>`. It takes
the class dictionary as "self" (`Tc/TyCl/Class.hs:190-319`: `this_dict` at
`:218`, exported with `abs_ev_vars = [this_dict]`). Because it receives the
whole dictionary, a default body that calls a sibling method just projects that
sibling from `self`.

An instance that omits `op` gets a **generated source-level binding**
`op = $dmop @T` (`mkDefMethBind`, `Tc/TyCl/Instance.hs:2256-2339`), which is
type-checked like user code. Instantiating `$dmop` raises `[W] C T`, and the
solver answers it with the instance's own dictionary. Note [How instance
declarations are translated] (`Instance.hs:114`): "Default methods get the
'self' dictionary as argument so they can call other methods at the same type."

Across modules, the interface records only *whether* a method has a default
(`IfaceClassOp … (Maybe (DefMethSpec IfaceType))`, `Iface/Syntax.hs:320-326`).
The `$dm` name is derived from the method name, and `$dm` itself is an ordinary
exported id with its own interface entry. Note [default method Name]
(`Iface/Recomp.hs:1845`). **An importer never needs the default's source.**

### Flux today

The default body travels as AST. A class rebuilt from a cached `.flxi` gets
`default_body: None` (`src/compiler/mod.rs:315`), and
`generate_dispatch_functions` then skips the method
(`types/class_dispatch.rs:1306-1312`). Inside a module, a default body cannot
see its sibling methods at all (the `E004` half).

### Copy — and a change to the plan

Phase 3d as written would carry `default_body` through `.flxi`. GHC's design
fixes **both** halves of KI-086 instead:

1. Compile each default as an exported function `__dm_<Class>_<method>` that
   takes the class dictionary as its first parameter. A sibling call inside it
   is ordinary evidence, `FromGiven` on the self dictionary, which fixes the
   `E004` half with no new scoping rule.
2. An instance that omits the method fills its slot with
   `__dm_<Class>_<method>(__dict_<Class>_<T>)`, in the same place the instance
   dictionary is built.
3. `.flxi` records only a "has default" flag per method. The function's name is
   derived from it, and the function is a normal public symbol of the class's
   module. This fixes the cross-module half.

The interface changes (from a body to a flag), so this still takes an epoch
bump. But the importer compiles no foreign AST, and the `--no-cache`
difference in behaviour disappears by construction.

---

## 4. A constrained function used as a value — Phase 3a (KI-090)

GHC needs nothing special. The bare `dbl` in `twice dbl 2` is instantiated like
any occurrence: Rule IALL "applies even if args = []" (`Tc/Gen/App.hs:723-755`).
It becomes `dbl @Int $dNumInt`, an ordinary partial application. Core arity
counts dictionary arguments as value arguments (`HsToCore/Binds.hs:476-498`,
`findSatArity`).

Flux has no partial application of known functions (see §1), so the eta
expansion `λx. dbl(d, x)` stays. What changes under Phase 1 is *where* it is
built: in the identifier emitter, as the "occurrence not in call position" case
of the one emitter, not as a separate rewrite. KI-090's remaining defect is
below Core, in closure conversion, and GHC offers no guidance there because it
never builds this lambda. The fix stays as planned: fill `param_types` and
`result_ty` from `hm_expr_types`, since they drive `FluxRep` selection.

---

## 5. Operators and name resolution — Phase 3b, 3c (KI-076, KI-071)

**Operators.** GHC's renamer leaves `OpApp` alone, and the typechecker expands
`a <= b` to `(<=) a b` (`Tc/Gen/Expand.hs:106-111`; Note [Desugar OpApp in the
typechecker], `Tc/Gen/Head.hs:334`). The head `(<=)` is then instantiated like
any variable, so there is **no separate operator dispatch**.

KI-076's symptom is the arity mismatch `want=3, got=2`. It means Flux's
operator lowering reaches the dictionary-passing method without asking for the
dictionary, which is exactly what a separate path would produce. In Phase 1, the
operator's evidence (keyed at the operator's id) must go through the same
emitter as an identifier's.

**Name resolution.** An instance-method binder in GHC is resolved with
`lookupInstDeclBndr` (`Rename/Env.hs:409-440`) **among the class's children**,
never through scope. It then binds the class selector's name, and the
typechecker gives it a fresh internal name (`$cop`). A same-named top-level
function is either a duplicate-declaration error (same module) or an ordinary
ambiguity (imported class). The instance binder can never capture it. KI-071 is
the opposite: Flux's *lowering*, after inference, rebinds `compare` to the class
dispatch stub. Plan 3c stands: lowering consumes inference's resolution and
never resolves a name again.

---

## 6. Generalization — Phase 2b, 2c

### How GHC does it

1. **Two-stage SCC.** The renamer computes SCCs (`Rename/Bind.hs:647-667`).
   Inside a recursive SCC, the typechecker runs SCC again after **removing edges
   to binders that have a complete signature** (`Tc/Gen/Bind.hs:429-448`).
   Note [Polymorphic recursion] (`Hs/Binds.hs:313-348`): "we can use the type
   signature for g to break the recursion."
2. **Collect, then decide once.** `tcPolyInfer` (`Bind.hs:716-776`) checks every
   member against monomorphic types for the others and captures all their
   wanteds together. It then makes **one** `simplifyInfer` call, which returns
   one `(qtvs, θ)` for the group.
3. **Deciding quantification** (Note [Deciding quantification],
   `Solver.hs:1192-1248`):
   - The mono variables are those free in the environment, plus those in
     constraints the monomorphism restriction blocks, closed under functional
     dependencies. They are *promoted* so they cannot be quantified.
   - The quantified variables are the free variables of **all** members' types,
     grown through the constraints (`growThetaTyVars`), minus the mono set.
   - `θ` is the candidates that mention a quantified variable, minimised by
     superclasses.
4. **Per-member export.** Each member keeps only the quantified variables and
   predicates reachable from **its own** type (`chooseInferredQuantifiers`,
   `Bind.hs:998-1007`). Its type must pass the ambiguity check: "if the user
   simply adds the inferred type to the program source, it'll compile fine"
   (Note [Validity of inferred types], `Bind.hs:1205-1217`).
5. **The monomorphism restriction is per group, and all-or-nothing.** If any
   member is a pattern binding, or has no arguments and no signature, the whole
   group is restricted (`checkMonomorphismRestriction`, `Bind.hs:778-815`).
6. **Defaulting happens only at the top level**, after solving, in a loop
   (Note [Top-level Defaulting Plan], `Tc/Solver/Default.hs:78-117`). Note
   [Default while Inferring] (`Solver.hs:2227-2259`): "defaulting only happens
   at simplifyTop and not simplifyInfer."

### Flux today

- **Groups.** `infer_binding_group` (`statement.rs:80-110`) predeclares a group
  monomorphically, as GHC does. It then calls `decide_quantification` **once per
  member**, each with its own constraint window. That is B4.
- **The monomorphism restriction.** Function definitions always generalize;
  `let` bindings are always `Restricted` (`statement.rs:226`). This matches
  GHC's rule for arity 0, except that Flux's rule is per binding, not per group.
- **Defaulting.** `decide_quantification` calls `build_numeric_default_subst`
  **before** it computes the quantified set (`src/types/quantify.rs:124`). That
  is defaulting during inference, which GHC deliberately avoids.

### Copy

- **2c follows GHC's shape.** Collect the constraints of every member of the
  group, call `decide_quantification` once on the union of the members' types,
  then trim each member's scheme to what its own type reaches. Test with a pair
  where only one member uses the constrained variable.
- **2b.** Once constrained definitions generalize, pick the numeric-defaulting
  behaviour before landing, because the two now interact.
  `fn double(x) { x + x }` must come out as `forall a. Num<a> => (a) -> a`, not
  defaulted to `Int`. Add a Phase 2 step:
  - measure what `build_numeric_default_subst` does to an unannotated
    constrained definition once B2 lands;
  - if it defaults `a` there, restrict it to variables that are *not*
    quantified. That is GHC's rule: default only what cannot be generalized.
- **Signatures still break recursion.** `binding_groups.rs` should drop edges to
  members with a complete signature before computing SCCs. Check whether it
  already does. If it does not, a signature-bearing member is quantified
  together with its group, which is weaker than GHC but not wrong. Note it, and
  don't block on it.

### Keep

Flux *pins* a field-access receiver so it is never quantified. GHC's `HasField`
works the other way: `getX p = p.x` generalizes to
`forall r a. HasField "x" r a => r -> a`. Flux's choice is deliberate, because
record-polymorphic access is 0184 stage 2, which is out of scope. The GHC
analogue of pinning is *promotion* of mono variables closed under functional
dependencies (Note [decideAndPromoteTyVars], `Solver.hs:1621-1714`), and it
confirms the rule E1 landed on: pin only while the receiver is still an unknown
variable.

---

## 7. Reporting ambiguity — Phase 4a

GHC reports a residual constraint in `reportUnsolved` (`Tc/Errors.hs:156-184`)
and builds the message in `pprTcSolverReportMsg (CannotResolveInstance …)`
(`Tc/Errors/Ppr.hs:4275-4359`). The error leads with ambiguity **only when all
three** hold (`:4309-4312`):

- the predicate still contains an unknown type variable;
- at least one instance *could* match;
- there are no usable givens.

Otherwise the error is "No instance for". Every wanted carries its origin
(`CtOrigin`, e.g. `OccurrenceOf name`, `Types/Origin.hs:384`), printed as
"arising from a use of ‘f’".

For `class Zero a where zero :: a`, a top-level `x = zero`, and
`instance Zero Int` in scope:

```
Ambiguous type variable ‘a0’ arising from a use of ‘zero’
prevents the constraint ‘(Zero a0)’ from being solved.
Probable fix: use a type annotation to specify what ‘a0’ should be.
Potentially matching instance: instance Zero Int
```

**Copy for 4a.**
- Use the same three conditions and the same three-part message: the variable
  and origin, the constraint, and a probable fix that lists the candidate
  instances.
- Flux's wanted constraints already carry an origin
  (`WantedClassConstraintOrigin`) and a span. The message should name the
  identifier whose use raised the constraint, not only its span.

---

## 8. Specialisation — Phase 2a (KI-098)

### How GHC does it

`Core/Opt/Specialise.hs` (overview at `:77-538`):

- **Collecting calls.** Calls are collected bottom-up as a call pattern. Each
  argument is classified as one of `SpecType`, `UnspecType`, `SpecDict` or
  `UnspecArg` (`:2462-2477`). The pattern is trimmed after the last
  `SpecDict`, and **a call with no interesting dictionary is not recorded at
  all** (`:3029`, `:3043`).
- **Interesting dictionaries.** A known global instance, or an instance
  function applied to interesting dictionaries, counts. A dictionary that is a
  lambda parameter does not: "We simply get junk specialisations" (Note
  [Interesting dictionary arguments], `:3164-3279`).
- **Making the clone.** The clone substitutes the dictionaries, then is
  **re-specialised immediately** (`:1710`), so calls inside it are found in the
  same pass.
- **Redirecting callers.** Callers are not rewritten. A rule
  `f @T $dT = $sf` is attached to `f`, and the simplifier fires it later
  (`:433-438`).
- **Recursive groups.** A recursive group gets **two sweeps, not a fixpoint**,
  because polymorphic recursion would otherwise specialise forever (Note
  [Specialising a recursive group], `:2284-2312`).
- **No numeric cap.** Blow-up is controlled structurally:
  - unconstrained type arguments stay polymorphic;
  - duplicate clones are removed;
  - instance functions are not cloned by default (Note [Do not specialise
    imported DFuns], `:995-1030`);
  - a function is only specialised if its leading parameters cover the
    dictionaries (Note [Specialisation Must Preserve Sharing], `:1873-1897`).
  
  The numeric caps belong to SpecConstr: `-fspec-constr-count=3` and a size
  threshold of 2000 (`Driver/DynFlags.hs:595-597`).
- **Unboxing comes later.** GHC gets unboxed `Int#` *after* specialisation:
  selecting the method on a known dictionary, then inlining, then
  demand analysis and worker/wrapper (`Core/Opt/Pipeline.hs:174-175, 261`).
  There is **no type-only specialisation for representation**, because GHC's
  polymorphic values are uniformly boxed (Note [Representation polymorphism
  invariants], `Core.hs:807-825`).

### Where Flux must differ

KI-098 is a definition used at two types **with no class constraint**
(`copy_head`), which loses `IntRep`. GHC would not specialise it at all. So:

- **Key clones on dictionaries *and* on representation-relevant type
  arguments.** Two instantiations that differ only in boxed types (`String` vs
  `List<Int>`) should share one clone. Two that differ in `FluxRep`
  (`Int` vs a boxed type) should not.
- **Rewrite call sites directly.** Flux has no rewrite rules and no simplifier,
  so keep a `(function, key) → clone` map and use it for every caller,
  including self-calls inside clones. This removes GHC's whole rule layer
  (activation, subsumption, `fireRewriteRules`).
- **Resolve method calls in the clone during cloning.** Once a clone's
  dictionaries are known globals, its method calls become direct `__tc_*`
  calls. GHC leaves that to the simplifier; Flux has no simplifier to leave it
  to.
- **Compute representations in the clone directly** from the concrete types,
  instead of waiting for a later unboxing pass.

### Copy as-is

- Re-specialise each clone immediately, and deduplicate by key.
- Two sweeps over a recursive group, and a stack of functions currently being
  specialised that refuses re-entry.
- Only known global dictionaries (or instance applications to them) count;
  parameters never do.
- Do not clone instance dictionary constructors.
- Deterministic clone names derived from the original name and the key (GHC's
  `$s` prefix; Flux's `f$<type_key>`), and deterministic emission order, because
  clones reach cached artifacts.
- **Cap: 3 clones per function**, borrowed from SpecConstr. Beyond the cap,
  callers use the generic body. Log it under `FLUX_DBG_SPEC` as a missed
  specialisation.

---

## Changes this makes to the plan

These are the edits to [0_0_8_plan.md](../roadmaps/0_0_8_plan.md) that follow
from the sections above.

1. **1b**: also verify that method-call predicates inside a constrained body are
   harvested as `FromGiven` with a path (§2), and that operator predicates are
   keyed at the operator's id (§5).
2. **1c**: the emitter lives in *identifier* lowering and is flattened into the
   call. One emitter covers calls, values and operators (§1).
3. **2**: new step. Before B2, decide what numeric defaulting does to a
   generalized constrained definition, and default only variables that are not
   quantified (§6).
4. **2a**: key clones on dictionaries plus representation; rewrite directly;
   two sweeps; cap of 3; never clone dictionary constructors (§8).
5. **2c**: collect, decide once, then trim per member (§6).
6. **3d**: replace "carry `default_body` through `.flxi`" with default methods
   as exported `__dm_*` functions taking the self dictionary (§3).
7. **4a**: GHC's three conditions and message shape (§7).
