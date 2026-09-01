- Feature Name: Typeclass Soundness and Extension Roadmap
- Start Date: 2026-08-29
- Status: Draft
- Proposal PR:
- Flux Issue:
- Depends on: [0145](implemented/0145_type_classes.md), [0146](implemented/0146_type_class_hardening.md), [0147](implemented/0147_constrained_type_params_and_instance_contexts.md), [0150](implemented/0150_hkt_instance_resolution.md), [0151](implemented/0151_module_scoped_type_classes.md), [0168](implemented/0168_hkt_polymorphic_dispatch_completion.md)

# Proposal 0179: Typeclass Soundness and Extension Roadmap

## Summary

Flux already supports classes, instances, default methods, multi-parameter
classes, HKT matching, module-scoped classes, and dictionary elaboration. The
next feature is to make that foundation sound and complete enough for a useful
standard hierarchy.

Haskell and GHC are inspiration for concepts such as dictionary evidence,
superclass entailment, kind checking, associated types, and deriving. They are
not Flux’s architecture or compatibility target. This proposal defines a
Flux-native plan using the existing frontend, HM inference, Core IR, VM, and
LLVM pipeline.

## Motivation and current gaps

The current implementation has feature surface ahead of its semantic and
runtime guarantees:

- Some polymorphic dictionary calls can reach the runtime with the wrong
  number of arguments.
- Generalized class constraints lose structured type arguments, so obligations
  can be silently skipped.
- ~~Superclass checking is not yet transitive, order-independent evidence
  passing.~~ Fixed in Stage 5.
- Method resolution is primarily directed by the first value argument, which
  cannot support methods determined by a result type.
- `Kind` exists but is not used to validate constructors, HKT applications, or
  instance heads.
- Associated types are not implemented.
- Unsupported deriving can create an instance without usable methods.
- Structural container checks can answer “yes” without producing a dictionary.

These gaps should be closed before adding `Monoid`, `Applicative`, or `Monad`
to the standard library.

## Haskell-inspired comparison

| Area | Haskell/GHC inspiration | Flux decision |
|---|---|---|
| Class metadata | Parameters, superclasses, methods, defaults, and associated type declarations. | Keep metadata in `ClassEnv`; add structural predicates, kinds, superclass evidence, and associated-type metadata. |
| Constraints | Explicit evidence-bearing obligations rather than names only. | Preserve complete predicates through inference, schemes, interfaces, and Core. |
| Instance solving | Resolution can succeed, fail, or remain ambiguous while producing evidence. | Use coherent, complete-predicate resolution; reject overlap and ambiguity. |
| Dictionaries | Selectors and superclass evidence are part of dictionary values. | Use Flux Core tuples with a stable superclass prefix and method slots. |
| Type-level information | Kinds validate applications; associated types reduce or remain stuck. | Add only the kind checking and associated-type fragment required by Flux. |
| Deriving | Contexts are validated and derived instances contain real methods. | Unsupported deriving is an error; supported deriving emits methods and evidence. |

Flux deliberately does not promise full GHC parity. Overlapping/incoherent
instances, functional dependencies, quantified or higher-rank constraints,
deriving strategies, standalone deriving, and full type families are deferred.

## Current Flux architecture

Typeclass processing currently crosses several compiler representations:

```text
Flux source
  → syntax parser / AST
  → class + instance collection (ClassEnv)
  → HM inference (wanted class constraints and schemes)
  → AST-to-Core lowering
      ├─ direct __tc_* dispatch for resolvable calls
      └─ Core dictionary elaboration for polymorphic calls
  → Aether / CFG / bytecode
      ├─ VM
      └─ LLVM native backend
```

The architecture is sound in direction—typeclass decisions belong before
backend lowering and both backends should consume equivalent Core—but the
current implementation has multiple sources of truth. Direct dispatch,
dictionary elaboration, AST fallback code, VM arity handling, and solver-only
structural checks can make different decisions for the same class obligation.

### What is not working and what must change

| Current area | Failure in the current architecture | Required change |
|---|---|---|
| Class environment | Resolved: `ClassEnv` stores classes and instances by `ClassId`; short names are used only at source-resolution and diagnostic boundaries, with ambiguity reported instead of first-match lookup. | Keep all new semantic consumers on `ClassId`; never reintroduce short-name fallback for identity. |
| Constraint model | HM constraints contain full type arguments, but generalized schemes retain only class names and variable IDs. Structured obligations can disappear. | Introduce one structural predicate type used by wanted constraints, schemes, solver, interfaces, and Core elaboration. |
| Instance solver | The solver primarily answers whether a concrete instance exists; unresolved, ambiguous, and contextual cases are not represented uniformly. Structural built-ins may report success without evidence. | Return a typed resolution result with evidence or a precise unresolved/ambiguous/error disposition. Every successful result must produce usable evidence. |
| Method dispatch | AST-to-Core lowering resolves many calls from the first value argument and has special cases for methods such as `Decode`. | Resolve from the complete class predicate, all value arguments, and expected result type through one shared resolver. |
| Dictionary elaboration | The Core pass can add dictionary parameters and arguments, while other paths still tolerate missing or alternate arities. This can surface as a runtime wrong-arity error. | Make elaborated Core the single calling convention for constrained functions; validate exact callee/call arity before either backend runs. |
| Backend selection | A constrained function can still be eligible for an AST/bytecode fallback even after Core dictionary rewriting. | Mark constrained definitions as Core-only until all dictionary behavior is represented in Core; remove runtime `__tc_*` workarounds. |
| Superclasses | **Done (Stage 5).** Obligations are checked once the whole environment is collected and matched structurally, cycles are rejected (E477), and dictionaries lead with one evidence slot per declared superclass. | — |
| Kinds | `Kind` and constructor kinds exist, but there is no checking pass; parameterized ADTs and invalid HKT heads can be accepted. | Add a kind environment and validate type applications, class parameters, instance heads, predicates, and associated types before solving. |
| Associated types | **Done (Stage 6).** Declarations and equations are parsed, collected, and validated; applications reduce through `normalize_associated_types` or stay stuck; both cross the module interface. | — |
| Deriving | Unsupported deriving may register an instance without generated methods or a dictionary. | Validate the supported deriving set and make every supported derivation produce ordinary callable methods and evidence. |
| Interfaces and cache | New class metadata must be present on both cold and warm compilation paths; dictionary layout changes can make cached artifacts stale. | Version and fingerprint predicate, kind, superclass, associated-type, and dictionary-layout metadata; add cold/warm tests. |
| Tests | Existing coverage often checks compilation or Core text, not execution of polymorphic calls. | Require a Flux example and Rust test for every implementation item, plus VM/LLVM parity for supported runtime behavior. |

The target architecture is therefore not “copy GHC.” It is one Flux semantic
pipeline with one predicate model, one evidence resolver, one dictionary
calling convention, and two backend consumers. The pre-phase must confirm
each row against the current code before implementation stages modify it.

## Add, improve, delete, and keep

| Action | Scope |
|---|---|
| Add | Structured predicates, kind checking, complete evidence resolution, superclass dictionary evidence, associated types, safe deriving diagnostics, and the standard class hierarchy. |
| Improve | `ClassId` lookup, contextual dictionary construction, call-arity validation, superclass validation, interface serialization, cache invalidation, diagnostics, and VM/LLVM parity coverage. |
| Delete | Silent constraint drops, solver-only “successful” structural instances without evidence, first-argument/name special cases, constrained-function AST fallback, and runtime dictionary-arity workarounds. |
| Keep | Flux syntax where it already works, HM inference, Core as the semantic IR, tuple dictionaries, monomorphic `__tc_*` fast paths where sound, module visibility/orphan rules, and coherent no-overlap instance policy. |

## Goals

1. A well-typed program never reaches either backend with an incomplete or
   mismatched dictionary call.
2. Every class obligation is solved, generalized, represented as a documented
   stuck predicate, or reported as an error—never silently discarded.
3. Class identity and instance lookup are `ClassId`-aware and deterministic.
4. Superclasses are checked independently of declaration order and passed as
   evidence.
5. Resolution can use all relevant argument types and the expected result.
6. Kinds, associated types, and deriving have explicit validation rules.
7. The standard library can add `Eq → Ord`, `Semigroup → Monoid`, and
   `Functor → Applicative → Monad` without backend-specific behavior.

## Non-goals

- Runtime type inspection as a dispatch mechanism.
- Overlapping or incoherent instances.
- Functional dependencies or quantified constraints in this roadmap.
- Full GHC type families, kind polymorphism, or higher-rank constraints.
- Deriving strategies, standalone deriving, and `via`.
- Changes to effect semantics beyond preserving effect rows on class methods.

## Pre-phase — current-code obstacle analysis

Before feature implementation, complete an analysis-only pass over the current
Flux code. The purpose is to turn assumptions into reproducible facts and to
identify blockers early.

| Area | Current code to inspect | Required result |
|---|---|---|
| Frontend pipeline | `src/compiler/pipeline.rs`, `src/compiler/statement.rs`, `src/core/lower_ast/`, `src/core/passes/dict_elaborate.rs` | A short data-flow diagram from class collection through HM inference, Core, CFG, VM, and LLVM. |
| Dictionary convention | `src/core/passes/dict_elaborate.rs`, `src/compiler/expression.rs`, `src/vm/function_call.rs` | A minimal Flux reproduction for each arity mismatch and a list of every dictionary insertion, removal, and arity check. |
| Predicate preservation | `src/ast/type_infer/constraint.rs`, `src/types/scheme.rs`, `src/types/class_solver.rs`, `src/types/class_defaulting.rs` | A producer/consumer table marking each obligation solved, generalized, skipped, or diagnosed. |
| Class identity | `src/types/class_id.rs`, `src/types/class_env.rs`, module-interface loading | A list of bare-name lookup callers and a same-named-class module example. |
| Kinds | `src/types/kind.rs`, `src/types/type_constructor.rs`, `src/types/infer_type.rs` | A constructor-arity inventory plus valid and invalid Flux kind examples. |
| Associated types | `src/types/assoc_type.rs`, `ClassEnv::validate_associated_types`, `InferType::Assoc` | Reducing, stuck, malformed, and cross-module examples — all covered by Stage 6 fixtures. |
| Superclasses | `ClassEnv::validate_superclass_obligations`, `ClassEnv::dictionary_layout`, dictionary construction, method rewriting | Declaration order, missing superclass, cycles, and generic superclass-method examples — all covered by Stage 5 fixtures. |
| Structural/contextual instances | builtin registration, `has_structural_builtin_instance`, dictionary emission | For every solver-only answer, whether usable runtime evidence exists, with a Flux example. |
| Deriving | deriving collection and generated methods | A supported/unsupported/sealed deriving matrix showing whether methods and evidence exist. |
| Interfaces and caches | `src/types/module_interface.rs`, cache paths, cold/warm helpers | A cold/warm comparison for class, instance, constraint, and dictionary metadata. |
| Backend parity | `tests/support/primop_parity.rs`, `tests/parity/`, native tests | A VM-only/LLVM-supported/parity-failing matrix for current typeclass examples. |

The pre-phase produces:

1. `docs/internals/type_classes.md` describing the current Flux data-flow and
   calling convention.
2. A Flux `.flx` example for every discovered obstacle under
   `examples/type_classes/`, with expected output or diagnostic.
3. A short blocker list in the Stage 0 pull request, with the smallest
   independently testable fix for each blocker.

No semantic feature work is merged in the pre-phase. Its merge gate is a
reviewed analysis, all examples wired into Rust tests or parity, and passing
existing repository gates.

Example syntax recorded by the pre-phase:

```flux
class Eq<a> {
    fn eq(x: a, y: a) -> Bool
}

instance Eq<Int> {
    fn eq(x, y) { x == y }
}

fn same<a: Eq>(x: a, y: a) -> Bool {
    eq(x, y)
}
```

## Staged implementation plan

Every stage is a separate pull request. Stages merge to `main` in order; the
next stage starts from the latest green `main`.

### Stage 0 — Baseline and test contract

- Record dictionary arity, dropped-constraint, first-argument dispatch,
  invalid-kind, and unsupported-deriving behavior. Examples:
  `dictionary_call_arity.flx`, `generalized_constraint_obligation.flx`,
  `result_directed_method_lookup.flx`, `invalid_higher_kind.flx`, and
  `unsupported_deriving_diagnostic.flx`.
- Add the Flux-native typeclass test matrix and parity smoke case:
  `typeclass_backend_parity.flx`.
- Finalize diagnostic and `.flxi` requirements with
  `TypeclassMetadata.flx`.

Example syntax:

```flux
class Eq<a> { fn eq(x: a, y: a) -> Bool }
instance Eq<Int> { fn eq(x, y) { x == y } }
fn same<a: Eq>(x: a, y: a) -> Bool { eq(x, y) }
fn main() with IO { print(same(20, 20)) }
```

### Stage 1 — Lossless predicates and kinds

- Replace lossy scheme constraints with complete structured predicates. A
  predicate keeps its full type arguments through inference, generalization,
  instantiation, dictionary resolution, Core lowering, and interface metadata.
  The executable contract is `examples/type_classes/structured_predicate.flx`.
- Add contextual kind checking for constructors, class parameters, instance
  heads, and constraint arguments. Valid higher-kinded use is covered by
  `examples/type_classes/kind_valid.flx` and
  `examples/type_classes/hkt_instance_positive.flx`.
- Keep invalid applications and mismatched predicates as stable diagnostics:
  `examples/compiler_errors/kind_invalid_ctor_arity_e472.flx`,
  `kind_invalid_underapplied_e472.flx`,
  `kind_invalid_instance_head_e473.flx`,
  `kind_invalid_constraint_e474.flx`, and
  `kind_invalid_class_param_conflict_e475.flx`.
- Preserve public class and instance metadata across interface serialization
  and reload, covered by `examples/type_classes/interface_roundtrip.flx`.
- Share structural predicate matching between AST and Core dictionary
  resolution so both lowering paths bind the same type variables.

Example syntax:

```flux
class Functor<f> {
    fn fmap<a, b>(xs: f<a>, g: (a) -> b) -> f<b>
}

instance Functor<Int> { }
// The instance above is expected to fail kind checking.
```

### Stage 2 — Dictionary runtime correctness

- Extend the shared resolver to cover complete call-site evidence and strict
  runtime contracts.
- Require exactly the elaborated dictionary arity:
  `dictionary_call_arity.flx`.
- Support contextual and function-valued dictionaries:
  `contextual_dictionary.flx`.
- Remove runtime arity workarounds and reject partial resolution:
  `no_partial_resolution.flx`.

Example syntax:

```flux
class Sizeable<a> { fn size(x: a) -> Int }
instance Sizeable<Int> { fn size(x) { x } }
fn twice<a: Sizeable>(x: a) -> Int { size(x) + size(x) }
fn main() with IO { print(twice(21)) }
```

### Stage 3 — Complete constraint solving

- Give every wanted predicate a disposition: `solved_constraint.flx`,
  `generalized_constraint.flx`, `stuck_constraint.flx`, and
  `diagnosed_constraint.flx`. A disposition is `Solved`, `Generalized`,
  `Stuck` with a documented reason, or `Diagnosed`; the type admits no
  fourth outcome, so an obligation cannot be discarded.
- Preserve structured constraints through generalization:
  `generalized_structured_constraint.flx`. Generalization partitions
  obligations with a port of the THIH `split`, which by construction retains
  every predicate it does not defer.
- Add missing-instance and overlapping-instance diagnostics:
  `missing_instance_e444.flx`, and `E454` for a predicate matched by more
  than one instance, which previously failed at runtime as `E1009`.
- Use one representation for `where` constraints: `FunctionTypeParam` carries
  `ClassConstraint`, so `fn f<a: Eq>(..)` and `fn f<a>(..) where Eq<a>` lower
  to the same predicate with its own span.

Enforcing operator-derived obligations closes a soundness hole: `fn double<a>(x: a) -> a { x + x }`
applied to a `String` used to compile and concatenate, and is now rejected.

`ambiguous_constraint.flx` moves to Stage 4. Ambiguity in the Haskell Report
sense needs a predicate whose variable is fixed only by the expected result
type, and Flux emits the obligation on the argument type until Stage 4 makes
resolution result-directed. A Stage 3 fixture would therefore assert the
wrong diagnostic rather than the ambiguity check.

Example syntax:

```flux
fn all_equal<a>(x: a, y: a) -> Bool where Eq<a> {
    eq(x, y)
}
```

### Stage 4 — Deterministic evidence resolution

- Resolve using the complete predicate, all arguments, and expected result:
  `multi_parameter_resolution.flx` and `result_directed_resolution.flx`. A
  class-method call now derives its predicate from the positions the *class
  declaration* puts its parameters in, so a parameter is read from whichever
  argument — or from the result — actually carries it.
- Reject candidates a call cannot choose between: `ambiguous_instance_e459.flx`
  reports `E459` when a call leaves a class parameter open and more than one
  instance stays compatible. `E454` continues to report a fully known predicate
  matched by several instances.
- Report a bound whose variable the signature never mentions:
  `ambiguous_constraint_e476.flx` (`E476`, Haskell Report §4.3.4) — the Stage 3
  deferral. Reporting it needed `where Convert<a, b>` to keep both arguments;
  bounds previously emitted only the parameter they were attached to, so the
  second argument was absent rather than undetermined
  (`where_constraint_multi_param.flx`).
- Record the matched instance as evidence, so dictionary elaboration consumes
  the solver's choice instead of resolving a second time and possibly
  disagreeing.

Two things the earlier draft of this stage assumed turned out to be wrong, and
are recorded here so they are not re-derived:

- **Specificity pruning is not applicable.** GHC prunes an overlapped instance
  only when one of the pair opts in with an `OVERLAPPING`/`OVERLAPPABLE`
  pragma (`Note [Rules for instance lookup]`, IL3). Flux has no such pragma, so
  reporting overlap — the existing `E454` behaviour — is already the
  GHC-consistent answer, and `Sizeable<List<a>>` against `Sizeable<List<Int>>`
  must keep erroring.
- **Inline `expr: Type` ascription does not exist.** The example this section
  used to carry did not parse. Expected types reach a call through `let`
  annotations and return-type position, which is what result-directed dispatch
  uses; adding ascription is a separate language change.

`qualified_class_id.flx` is fixed. Semantic class and instance identity is
`ClassId = (owning module, class name)`, following GHC's use of globally unique
class names rather than short textual names. Two modules may therefore declare
same-named classes with the same head; they produce distinct solver evidence,
`__tc_*` methods, and `__dict_*` globals. Short names remain a source-resolution
and diagnostic concern only, and an ambiguous unqualified class or method must
be qualified. The interface symbol table preserves both ClassId components
across `.flxi` boundaries, while VM aliases and native module ownership remain
separate from semantic identity.

Example syntax:

```flux
class Convert<a, b> { fn convert(x: a) -> b }
instance Convert<Int, String> { fn convert(x) { to_string(x) } }
instance Convert<Int, Bool> { fn convert(x) { x > 0 } }

fn main() with IO {
    let text: String = convert(42)   // selects Convert<Int, String>
    let flag: Bool = convert(42)     // selects Convert<Int, Bool>
    print(text)
    print(flag)
}
```

### Stage 5 — Superclass evidence — **done**

- Obligations are validated after the complete environment is collected, and
  matched structurally rather than by rendered type name:
  `superclass_order_independent.flx`, `missing_superclass_e445.flx`.
- Cycles are rejected with E477, one diagnostic per cycle anchored at its
  first-declared class: `superclass_cycle_e477.flx`.
- Dictionaries lead with one slot per directly declared superclass, and
  inherited methods are reached by projecting through it:
  `superclass_method_call.flx`, `transitive_superclass.flx`.
- The layout crosses module interfaces, so an inherited method dispatches the
  same cold and warm: `superclass_across_modules.flx`,
  `SuperclassMetadata.flx`.

Example syntax:

```flux
class Eq<a> { fn eq(x: a, y: a) -> Bool }
class Eq<a> => Ord<a> { fn compare(x: a, y: a) -> Int }
fn same_order<a: Ord>(x: a, y: a) -> Bool { eq(x, y) }
```

Two notes for later stages.

**E445 was previously demoted to a warning.** `collect_class_declarations`
partitions class diagnostics and treated only E453 and the kind codes as hard
errors, so `missing_superclass` compiled and exited 0. E445 and E477 are now
promoted. E440/E441/E442 remain demoted; they are blocked on built-in
shadowing, not on this stage.

**One inherited limitation, filed as KI-057.** A method reachable from two
dictionaries for the same class, over different type variables, always
dispatches through the first — the map from method to dictionary is keyed on the
method name and never consults the call's argument types. Superclass evidence
inherits this (two constraints whose closures reach one superclass take that
evidence from whichever dictionary is found first) but does not cause it: the
reproduction uses no superclasses and fails identically before Stage 5. It
belongs with the dictionary-selection work, not here.

**Entailment needed no separate solver change.** A wanted predicate discharged
by a superclass of a constraint already in scope is handled by the projection
paths, because a caller forwards the dictionary it holds — `fn outer<a:
Measurable>` calling `fn doubled<a: Sizeable>` works without a `bySuper` rule in
`classify_constraint`. If a case is found that the projection paths cannot
reach, that rule is where it belongs.

### Stage 6 — Associated types — **done**

- Declarations and equations are parsed and reach the class environment:
  `associated_type_declaration.flx`.
- Duplicate, missing, unbound, ill-kinded, recursive, and undeclared equations
  are rejected with their own codes (E479–E484): `duplicate_equation_e479.flx`,
  `missing_equation_e480.flx`, `unbound_equation_e481.flx`,
  `bad_kind_equation_e482.flx`, `recursive_equation_e483.flx`, and
  `unknown_equation_e484.flx`.
- Applications whose arguments select an instance reduce to that equation's
  body, and applications over a rigid variable are preserved:
  `associated_type_reduction.flx` and `stuck_associated_type.flx`.
- Declarations and equations cross the module interface, so an importing module
  reduces exactly as the defining module did, cold and warm:
  `associated_type_interface_roundtrip.flx`.

Example syntax:

```flux
class Collection<c> {
    type Element<c>
    fn first_of(xs: c) -> Element<c>
}

instance Collection<List<Int>> {
    type Element<List<Int>> = Int
    fn first_of(xs) { 7 }
}
```

Four notes for later stages.

**A new `InferType` variant, not a new `TypeConstructor`.** `Assoc` reaches
around 32 sites against 448 for a constructor variant, but the deciding reason
is that `TypeConstructor` flows into Core, LIR, and LLVM, where a runtime type
is expected. Keeping stuck applications inside the type checker lets every
backend boundary reject one outright, which makes "a stuck type escaped
inference" loud rather than a miscompile.

**Recognition, not a threaded lookup.** `Element<c>` already converts to
`App(Adt(Element), [Var])`, so the normalizer recognizes it after the fact
instead of `convert_type_expr_rec` producing it directly. That kept 40 call
sites untouched.

**Reduction cannot live in substitution or unification.** Neither
`apply_type_subst` nor `unify_core` holds a `ClassEnv`, and selecting an
equation needs one. Inference normalizes at the boundaries where it does hold
one; unification treats what is left structurally.

**A stuck type nearly unified with a concrete one.** `fn wrong<c: Collection>(xs:
c) -> String` compiled and returned the selected instance's value. Unification
correctly reported the mismatch; the E300 guard suppressed it, because it only
reports when both sides are `is_concrete()` and a stuck application is not. The
guard now also counts an application settled over this signature's own rigid
variables. Relaxing it further — to any type whose free variables are all rigid
— regresses `lib/Flow/Stream.flx`, which is the false positive the guard exists
to avoid.

### Stage 7 — Safe deriving and structural dictionaries

- Reject unsupported deriving: `unsupported_deriving.flx`.
- Generate real methods and evidence for supported deriving:
  `derived_eq.flx`, `derived_ord.flx`,
  `derived_show.flx`, `derived_encode.flx`, and
  `derived_decode.flx`.
- Replace solver-only structural answers with usable evidence:
  `structural_container_dictionary.flx`.

Example syntax:

```flux
data Pair<a, b> = Pair(a, b) deriving (Eq, Show)
fn equal<a: Eq>(x: a, y: a) -> Bool { eq(x, y) }
fn main() with IO { print(equal(Pair(1, 2), Pair(1, 2))) }
```

### Stage 8 — Standard hierarchy

- Add `Eq → Ord`, `Semigroup → Monoid`, and
  `Functor → Applicative → Monad`: `eq_ord.flx`,
  `semigroup_monoid.flx`, and `functor_applicative_monad.flx`.
- Add supported scalar and collection instances:
  `option_instances.flx`, `list_instances.flx`,
  `array_instances.flx`, and `either_instances.flx`.
- Cover result-directed `pure`/`mempty` and effectful `fmap`:
  `return_directed_pure.flx`, `mempty_result_dispatch.flx`,
  and `effectful_fmap.flx`.

Example syntax:

```flux
class Functor<f> { fn fmap<a, b>(xs: f<a>, g: (a) -> b) -> f<b> }
class Functor<f> => Applicative<f> { fn pure<a>(x: a) -> f<a> }
fn main() with IO { print(fmap([1, 2], \x -> x + 1)) }
```

## Testing and merge policy

When compiler or runtime code changes, the same pull request must add or
update Rust tests and Flux tests. Core dumps alone do not prove runtime
correctness.

Every implementation item above must add at least one Flux `.flx` example.
Positive examples execute with expected results; negative examples assert
diagnostic codes; backend-supported examples run through VM/LLVM parity; and
cache-sensitive examples exercise cold and warm compilation paths. Examples
are part of the feature contract and must be referenced by a Rust test or the
parity runner so they cannot silently become stale.

The default location is `examples/type_classes/`, using descriptive filenames.
A fixture may be mirrored into `tests/parity/` or `tests/fixtures/` when an
existing harness requires that location.

Each stage must pass:

```text
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all --all-features
cargo run -- parity-check tests/parity --ways vm,llvm,vm_cached,vm_strict,llvm_strict
```

No stage is merged with a known silent fallback, skipped obligation, or
backend-specific behavior without an explicit parity decision.

## Acceptance criteria

- Existing Flux unit, integration, and parity tests remain passing.
- The known polymorphic dictionary example executes correctly on VM and
  native, without a wrong-arity runtime error.
- No class obligation is silently dropped during solving or generalization.
- Superclass evidence, kind errors, associated-type errors, ambiguity, and
  unsupported deriving have deterministic diagnostics.
- Derived and structural instances produce usable evidence.
- The Stage 8 hierarchy executes identically on VM and LLVM.
- Every stage can merge independently while `main` remains green.

## Open decisions

- Whether `where` should become the single spelling for all class contexts.
- Whether associated-type equality should be user-visible in a later proposal.
- Whether functional dependencies or richer deriving are justified by real
  Flux use cases after Stage 8.
