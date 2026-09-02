# Flux Typeclass Architecture

## Purpose

This document records the current Flux typeclass architecture and the Stage 1
acceptance baseline. It is an implementation map for the compiler, not a
language proposal and not a promise that every future typeclass feature exists.

Haskell-style typeclasses are useful prior art for the concepts of class
constraints, dictionaries, superclass evidence, and derived instances. Flux
keeps its own syntax, representations, module rules, and backend pipeline.

## End-to-end data flow

```text
Flux source
  |
  v
Lexer and parser
  |
  v
AST: class, instance, function, data, and expression declarations
  |
  +--> ClassEnv collection and validation
  |      - ClassDef and InstanceDef
  |      - ClassId ownership and module visibility
  |      - method signatures, defaults, contexts, and effect floors
  |
  +--> HM type inference
  |      - inferred types and schemes
  |      - WantedClassConstraint obligations
  |      - explicit type-parameter bounds
  |
  v
AST-to-Core lowering
  |
  +--> direct __tc_* call for a concrete, known instance
  |
  +--> dictionary arguments for constrained function calls
  |
  v
Core dictionary elaboration
  |
  +--> __dict_* tuple definitions
  +--> dictionary parameters on constrained functions
  +--> TupleField method extraction for polymorphic calls
  |
  v
Aether ownership and CFG/LIR lowering
  |
  +--> bytecode and VM
  +--> LLVM native backend
```

Module compilation additionally builds and consumes `.flxi` interfaces and
`.fxc` bytecode caches. Imported public classes and instances are reconstructed
into the consuming compiler's `ClassEnv` before inference and lowering.

## Subsystem responsibilities

| Subsystem | Producer | Consumer | Current representation | Status and baseline finding |
|---|---|---|---|---|
| Syntax | Lexer/parser | Compiler and inference | `Statement::Class`, `Statement::Instance`, `ClassConstraint`, `TypeExpr` | Supported class, instance, superclass-context, module, and deriving syntax. |
| Class collection | `ClassEnv::collect_from_statements` | Solver, dispatch, interfaces | `ClassDef`, `InstanceDef`, `ClassId`, method metadata | Supported validation for duplicates, methods, arity, visibility, orphan rules, and direct superclass instances. Legacy short-name compatibility callers remain. |
| Class identity | `ClassId` and `ModulePath` | Lookup, dispatch, interface loading | `(module path, class name)` | Semantic lookup, solver evidence, dispatch, dictionaries, and interface schemes use ClassId; short-name lookup is restricted to source resolution and diagnostics. |
| HM constraints | Expression/type inference | Solving and scheme construction | `WantedClassConstraint` and `SchemeConstraint` both carry full `Vec<InferType>` predicates | Structured arguments survive collection, remapping, instantiation, generalization, and dictionary lookup. Built-in concrete helper constraints retain compatibility handling; richer obligation states remain later work. |
| Constraint solving | `class_solver` and class environment | Compiler diagnostics and dispatch | Concrete instance matching plus structural builtin checks | Supported concrete, HKT, contextual, and structural cases have coverage. The solver can skip unresolved variables because they are generalized; complete obligation dispositions belong to Stage 3. |
| Direct dispatch | AST-to-Core lowerer and `class_dispatch` | Core/backend lowering | Mangled `__tc_*` functions | Supported for monomorphic calls. It still has compatibility paths that identify methods by short name and use call-shape heuristics. Stage 4 owns complete-predicate resolution. |
| Dictionary elaboration | `core::passes::dict_elaborate` | Core, Aether, VM, LLVM | `__dict_*` tuples, implicit dictionary parameters, `TupleField` calls | Supported for current constrained functions and HKT forwarding. Exact calling-convention tests are covered; removal of fallback/workaround paths belongs to Stage 2. |
| Ownership/backend lowering | Aether, CFG/LIR, bytecode, LLVM | VM and native execution | Ordinary calls, tuples, globals, and indirect calls | Both backends consume the lowered representation. The typeclass parity smoke test is deterministic; unrelated TCP timeout behavior remains explicitly skipped. |
| Superclasses | Parser, class collection, `ClassEnv::validate_superclass_obligations` | Class environment, dictionary layout, dispatch | `ClassDef.superclasses`, `ClassDef.superclass_class_ids`, `DictSlot::Superclass` | Stage 5: obligations are checked against the whole program transitively (E445), cycles are rejected (E477), and dictionaries carry evidence in leading slots so inherited methods dispatch on both backends. |
| Kinds | `types::kind`, `types::kind_check`, constructors, inference types | Compiler diagnostics and interface consumers | `Kind::Type` and `Kind::Arrow` | Stage 1 validates known constructors, contextual applications, instance heads, constraints, and class-parameter conflicts. Unknown imported constructors remain open at the interface boundary. |
| Associated types | Parser, class collection, `types::assoc_type` | Inference and interfaces | `AssociatedTypeDecl`, `AssociatedTypeEquation`, `InferType::Assoc` | Stage 6: declarations and equations are collected and validated (E479–E484), applications reduce through `normalize_associated_types` or stay stuck, and both cross the module interface. |
| Deriving | ADT/class collection and dispatch generation | Runtime method calls | Structural built-in deriving for current supported classes | Stage 7: a clause is accepted only when every method of the class can be given a *usable* body, and rejected with E486 otherwise. A supported clause on a monomorphic head yields methods callable by name and a dictionary a constrained function can project them out of. `Eq` over `List` and `Option` resolves to a real contextual instance rather than a solver-only answer. A parameterized derived head still fails ([KI-059](../known_issues.md#ki-059)). |
| Interfaces | `compiler::module_interface` | Importing compiler and cache validation | Public class/instance entries, structured predicates, kind metadata, associated types, and fingerprints | `parameter_kinds`, `head_kinds`, and associated-type metadata are serialized with defaults for old interfaces and included in fingerprints. |

## Dictionary calling convention

There are two supported forms.

### Concrete call

When inference and the class environment identify an instance, AST-to-Core
can lower a class method directly to its generated function:

```text
eq(1, 1)
  -> __tc_Eq_Int_eq(1, 1)
```

The direct path avoids dictionary overhead for a monomorphic call.

### Polymorphic call

A constrained function receives its dictionary before its source value
parameters. A class method in its body extracts the method slot from that
dictionary:

```text
fn same<a: Eq>(x: a, y: a) -> Bool { eq(x, y) }

same(1, 1)
  -> same(__dict_Eq_Int, 1, 1)
```

Inside `same`, the `eq` operation is represented as a dictionary tuple-field
lookup followed by an indirect call. Contextual instances are represented as
dictionary constructors with recursively supplied context dictionaries.

### Dictionary layout

A dictionary tuple leads with one slot per **directly declared superclass**,
holding that superclass's dictionary, and the class's own method slots follow.
Both groups are in declaration order:

```text
class Sizeable<a> => Measurable<a> { fn weight(x: a) -> Int }

__dict_Measurable_Int = ( __dict_Sizeable_Int,          // slot 0 — evidence
                          __tc_Measurable_Int_weight )  // slot 1 — method
```

Evidence leads so that a slot's offset does not depend on how many methods the
class declares. Only *direct* superclasses get a slot, which keeps a class's
layout independent of the hierarchy above it; a transitive superclass is
reached by projecting again (`__dict_Top.0.0`). That is what lets a function
constrained on a subclass call an inherited method:

```text
fn describe<a: Measurable>(x: a) -> Int { size(x) + weight(x) }

  weight(x) -> __dict_Measurable.1        // own method
  size(x)   -> __dict_Measurable.0.0      // through Sizeable evidence
```

`ClassEnv::dictionary_layout` is the single definition of this layout. Four
places build a dictionary and two read a slot out of one, and all six derive
their offsets from it rather than restating the convention — a half-migrated
layout reads every slot after the divergence at the wrong index, which is a
silent miscompile rather than a missing method.

Superclass evidence for a contextual instance comes from that instance's own
context before its global. `instance Middle<Int> => Top<Int>` is handed the
`Middle<Int>` dictionary its superclass slot needs; reaching for the global
would apply a dictionary *constructor* to an already-built dictionary.

The count of superclasses therefore decides the layout, so it crosses module
interfaces alongside the constraints themselves (`superclass_class_modules`,
parallel to `superclasses`). A consumer that cannot rebuild every superclass
identity refuses the class rather than laying it out one slot short.

### Dictionary selection

Layout says where a method sits *inside* a dictionary. Selection says *which*
dictionary a call means, and the two are separate problems: a function
constrained twice on one class holds two dictionaries that have identical
layouts.

A call chooses by predicate, not by method name.
`ClassEnv::dispatch_positions` reports, for each of a class's type parameters,
which value argument of the method is declared as exactly that parameter;
`select_dictionary` matches the types found there against each candidate
constraint. Both are defined once and used by the elaborator and by the
ambiguity check, so a call one accepts is one the other can resolve.

```text
fn both<a: Root, b: Root>(x: a, y: b) -> Int { root(x) + root(y) }

  root(x) -> __dict_Root.0     // x : a  matches constraint 0
  root(y) -> __dict_Root_1.0   // y : b  matches constraint 1
```

Two rules keep this correct rather than merely different:

- **Equal predicates are interchangeable.** There is at most one instance per
  type, so two constraints over the same type reach the same implementation
  whichever is chosen. This is what lets a function constrained on both
  `Sizeable<a>` and `Measurable<a>` call `size`: one candidate reaches it
  directly and the other through superclass evidence, and they arrive at the
  same method. Without it, every superclass would make its own methods look
  ambiguous.
- **A call that cannot decide is reported.** A class parameter mentioned
  nowhere in its method's signature can never be named by any call site. That is
  E485, not a first-match — selecting the first is precisely what made
  `both(5, "hi")` return `14` instead of `12` (KI-057).

Selection reads argument types only. A method dispatched on its result type,
like `Flow.Json`'s `decode`, keeps the behavior it had; closing that needs the
call's expected result type, which Core does not carry (KI-058).

The runtime contract is that the callee's implicit dictionary
parameters and the caller's inserted dictionary arguments have exactly the
same count and order. A valid program must not rely on VM-side arity recovery.

## Associated types

A class may declare a type alongside its methods, and each instance says what
that type is:

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

A use of `Element<c>` in a signature is an application of that declaration, not
a reference to an ordinary type constructor. It is carried by
`InferType::Assoc(ClassId, name, args)` and eliminated by
`types::assoc_type::normalize_associated_types`, which matches the arguments
against each instance's equation head and substitutes the body.

Reduction lives there rather than in `apply_type_subst` or `unify_core`:
selecting an equation needs the class environment and neither of those has one.
Inference calls it at the boundaries where it does hold one — chiefly
`infer_type_from_annotation` — so unification only ever sees an application
that is genuinely stuck.

An application over a rigid variable cannot reduce, and that is not an error:
it is a type waiting for the call site that fixes the variable. Two stuck
applications unify when they name the same declaration and their arguments
unify. That rule is deliberately syntactic — an associated type is not
injective, so `Element<a>` equal to `Element<b>` must never be taken to mean
`a` equal to `b`.

Every backend boundary rejects `Assoc`: `CoreType::try_from_infer` returns
`None` and `try_to_runtime` errors. A stuck type must have been reduced or
reported before lowering, and accepting one there would be a miscompile rather
than a missing type.

Reduction terminates because `normalize` carries fuel and because `E483`
rejects an equation whose body reaches itself before anything relies on it —
the same ordering E477's cycle check uses for the superclass closure.

Declarations and equations cross the module interface on `PublicClassEntry` and
`PublicInstanceEntry`, so an importing module reduces an application exactly as
the defining module did. They travel separately, and a short list does not fail
loudly on its own: the application simply stays stuck and resurfaces as an
unrelated error about the type it never became. The two counts are therefore
compared when an imported instance is merged, and a mismatch is reported as a
stale interface (E478).

## The standard classes are Flux source

`Eq`, `Ord`, `Num`, `Show` and `Semigroup` are declared in `lib/Flow/*.flx`,
not registered from Rust. `ClassEnv::register_builtins` now registers only
`Sendable` — a sealed marker class with no methods, which no Flux module could
declare.

| Module | Declares | Instances |
|---|---|---|
| `Flow.Eq` | `Eq` | `Int Float String Bool`; `Eq<a> => Eq<List<a>>`, `Eq<a> => Eq<Option<a>>` |
| `Flow.Ord` | `Eq<a> => Ord<a>` | `Int Float String` |
| `Flow.Num` | `Num` | `Int Float` |
| `Flow.Show` | `Show` | `Int Float String Bool` |
| `Flow.Semigroup` | `Semigroup` | `String`, `List<a>`, `Array<a>`, `Semigroup<a> => Semigroup<Option<a>>` |
| `Flow.Monoid` | `Semigroup<a> => Monoid<a>` | `String List<a> Array<a> Option<a>` |
| `Flow.Functor` | `Functor` | `List Option Array` |
| `Flow.Applicative` | `Functor<f> => Applicative<f>` | `List Option Array` |
| `Flow.Monad` | `Applicative<f> => Monad<f>` | `List Option Array` |

The first five are the **class prelude** (`src/shared/class_prelude.rs`): they
are injected into every module in the graph, not only the entry file. That is
not a convenience. `==` emits an `Eq` obligation only when a class named `Eq`
is in the environment and emits *nothing* otherwise, so a module compiled
without them would type its operators unconstrained and silently. **E487**
(`OPERATOR_CLASS_NOT_IN_SCOPE`) now reports that case rather than dropping it.

`Flow.Monoid`, `Flow.Functor`, `Flow.Applicative` and `Flow.Monad` are
explicit-import: no operator desugars to them, so a program that does not use
them does not pay to compile them.

Registration is centralised in `collect_class_declarations_diagnostics`, which
calls `ClassEnv::register_prelude_classes` for every module that is not itself
a class-prelude module. It is idempotent per class, so a module that already
holds some of them keeps those and gains the rest rather than declaring one
twice (E440 / E443).

### `is_builtin` means "declared by the stdlib"

`ClassDef::is_builtin` no longer means "registered from Rust". It is set at
collection time for any class whose owning module path starts with `Flow.`
(`ClassEnv::is_stdlib_module`), and it must also be set where an imported class
is reconstructed from a module interface — the class arrives by that path, not
through `collect_classes`, and setting it in only one of the two places has no
effect. It gates `structural_builtin_evidence`, so a Flux-declared stdlib class
keeps the structural answers the Rust registrations used to have.

### Consequences worth knowing

- **A user `class Eq` no longer collides with the prelude's.** They are
  different classes in different modules, so both register. The user's shadows
  for operators, and `1 == 2` then fails with a deterministic
  `E444 No instance for Eq<Int>`. Loud, not silent — but a behaviour change.
- **A stdlib function may not share a name with a class method.** A name bound
  to a function definition never dispatches as a class method
  (`reserved_names` in `types::class_dispatch`), so the function silently wins.
  `Flow.Numeric.div` became `floor_div` and `Flow.List`'s private `append`
  became `concat_lists` for exactly this reason; the second was caught only by
  the native backend, which had no symbol for the class method the calls had
  been rewritten to.
- **A superclass edge is checked after imported instances are merged.**
  `Ord`'s `Eq` evidence lives in `Flow.Eq`, so checking it at the end of
  `collect_from_statements` reported every edge as unsatisfied. The check is
  deferrable (`SuperclassCheck`), and the compiler runs it once merging is
  done — judging only the instances the module declares, since an imported one
  was already checked where its evidence was in scope.
- **Each stdlib instance names its superclass evidence explicitly**
  (`instance Eq<Int> => Ord<Int>`). Leaving it to be solved from `Flow.Eq`
  sends the solver into a loop that overflows the compiler's stack.
- **Transitive superclass evidence is not transitive through imports.**
  `Flow.Monad` imports `Flow.Functor` directly even though `Flow.Applicative`
  already does, or its instances cannot discharge the transitive obligation.

### Higher-kinded classes

`Functor`, `Applicative` and `Monad` take a type *constructor*, and their kinds
are inferred from method signatures rather than written down. A superclass edge
between two such classes was rejected until Stage 8: `validate_constraint`
checked the constraint's argument with no local binders, so the `f` in
`class Functor<f> => Applicative<f>` was unknown and defaulted to kind `Type`
while `Functor` wanted `Type -> Type` (**E474**). It now binds the owner class's
own parameters, which is what the caller already had to hand.

`fmap` carries an effect row — `fn fmap<a, b>(x: f<a>, g: ((a) -> b with |e))
-> f<b> with |e` — so a class method is not restricted to pure functions.
`ap` cannot delegate to `Flow.List.map` for the same reason: `map` is
effect-row polymorphic and `ap` is not, leaving the row nothing to resolve
against (E419), so both go through `flat_map`.

`Either` has **no** `Functor`/`Applicative`/`Monad` instance. Its head would be
partially applied (`Either<l>`), which does not survive a module boundary — see
[KI-064](../known_issues.md#ki-064). `Eq` and `Ord` over `Either` are
unaffected; that evidence is structural.

## What `deriving` accepts

A `deriving` clause is accepted when **every** method of the class can be given
a body that actually runs on the derived type. The rule is deliberately not a
curated list of class names: the question "is this clause supported?" is the
same question as "can the generator emit something callable?", and one predicate
answering both cannot drift out of step with the generator.

`can_derive_method` in `types::class_dispatch` is that predicate. It is asked
only about the head of a `deriving` clause — a user `data` declaration by
construction — so the built-in `Eq<Int>`-style phantom instances never reach it.
`has_builtin_method_body` beside it mirrors the match inside
`builtin_method_body`, and `builtin_bodies_match_the_derivable_set` cross-checks
the two over the whole class x type x method grid.

| Class | Derivable | Why |
|---|---|---|
| `Eq` | yes | `==` and `!=` compare ADTs structurally on both backends |
| `Show` | yes | renders the constructor name |
| `Json.Encode` / `Json.Decode` | yes | bodies are built structurally from the data declaration |
| `Ord` | **no** | a body is generated — `x < y` — but the comparison primops reject an ADT, so the method compiles and traps with `E1009` |
| `Num` | **no** | same shape: `x + y` over constructors does not run |
| `Semigroup` | **no** | only `String` has a body |
| anything user-declared | **no** | nothing can generate its methods |

Rejection is **E486** at the clause, not at the generator. The generator's
missing-body site runs only when the program independently needs built-in
dispatch support, so a diagnostic there would be conditional on unrelated code
being present. Reporting from `collect_deriving` also keeps the phantom
instances out of range without a marker field on `InstanceDef`.

`Ord` and `Num` become derivable again once their generated bodies compare
constructors structurally rather than delegating to a primop;
`examples/compiler_errors/underivable_ord_e486.flx` records that, so the
restriction is not undone by someone reading only the table above.

### Structural predicates and their evidence

`Eq`, `Ord` and `Sendable` are answered for some heads by a structural rule in
`class_solver::structural_builtin_evidence` — `Eq<(Int, String)>` holds because
each component does — rather than by an `InstanceDef`. Evidence with no instance
behind it names no dictionary, so a constrained function declaring that
constraint took a dictionary parameter nobody supplied and was called one
argument short (`E1000 want=3, got=2`). Calling the method directly worked,
which is what kept the gap out of sight.

`Eq` over `List` and `Option` therefore has real contextual instances — written
in `lib/Flow/Eq.flx` since Stage 8, previously registered from Rust — and
`solve_instance_evidence` tries instance resolution **before** the structural
rule so they are not shadowed by it. The structural rule stays as the fallback for heads with no
instance: tuples, `Either`, `Array`, and every `Sendable` case — `Sendable` is a
marker class with no methods, so it has no dictionary to build in the first
place.

## Current architecture problems

The current implementation has more than one route to a class method:

1. Direct AST-to-Core `__tc_*` dispatch.
2. Core dictionary elaboration.
3. AST fallback dictionary insertion for selected paths.
4. Runtime function-call arity validation.
5. Solver-only structural checks for some container types.

These routes are useful compatibility layers, but they can make failures hard
to classify. The baseline measures each route and records whether a case
is supported, intentionally deferred, or a reproducible regression.

The most important current limitations are:

- scheme constraints do not yet preserve arbitrary structured predicates;
- unresolved constraints are intentionally deferred to generalization, but the
  compiler does not expose one complete disposition model for every obligation;
- short-name lookup remains only at source-resolution and diagnostic boundaries;
- direct dispatch is not yet selected from a complete predicate and expected
  result type;
- kind values are not enforced by a complete type-application checker;
- deriving coverage is limited to the classes and shapes currently supported
  (see **What `deriving` accepts** below).

The staged roadmap assigns those limitations to later stages. Stage 1 does not
silently fix them by changing their expected baseline; it adds explicit
fixtures and tests so later stages can change one contract at a time.

## Stage 1 test contract

Baseline fixtures live in `examples/type_classes/` and use
descriptive behavior names. They are loaded by the dedicated Rust contract
tests and the typeclass parity fixture rather than by the broad example
snapshot suite.

| Fixture | Contract measured | Expected result |
|---|---|---|
| `dictionary_call_arity.flx` | Concrete and polymorphic dictionary call shape | Supported behavior executes without an arity error. |
| `generalized_constraint_obligation.flx` | Constraint attached to a generic function | The supported constraint is preserved through compilation and execution. |
| `result_directed_method_lookup.flx` | Method whose result could influence selection | Baseline behavior is recorded; result-directed selection remains Stage 4. |
| `invalid_higher_kind.flx` | Invalid type-constructor application | Invalid known applications receive contextual kind diagnostics. |
| `unsupported_deriving_diagnostic.flx` | Unsupported deriving request | Stage 7: the clause is rejected with E486 rather than registering a method-less instance. |
| `TypeclassMetadata.flx` | Public class/instance interface serialization | Metadata survives interface construction and reload. |
| `typeclass_backend_parity.flx` | Supported class dispatch on VM and LLVM | Deterministic output is identical across supported backends. |
| `multiple_class_obligations.flx` | Two independent generalized obligations | Both dictionaries are inserted in declaration order and both methods execute. |
| `superclass_instance_validation.flx` | Existing superclass and contextual-instance validation | Direct superclass evidence and the contextual instance continue to compile and run. |

The Rust tests reference every fixture. The parity fixture is also registered
under `tests/parity/` with the standard parity metadata header. Structured
predicate tests inspect both runtime behavior and retained scheme metadata.

Stage 1 acceptance requires lossless predicate arguments, contextual kind
diagnostics, interface kind metadata round-trips, and one shared structural
matcher for AST and Core dictionary resolution. The final two concerns are
implemented as shared compiler utilities; full superclass entailment,
associated types, and deriving expansion remain later roadmap work.

## Baseline blocker list

- Hidden dictionary arity has historically been derived from all constraints,
  including marker-only classes. The shared filtered counter now covers the
  statement and expression paths, but `src/core/passes/dict_elaborate.rs`
  still derives Core parameters from raw constraints (the dictionary insertion
  and lambda-parameter paths around lines 408–640). Stage 2 must align Core
  elaboration with the filtered definition before a marker-class regression
  test can become a passing contract.
- Runtime contract validation must account for injected dictionaries before
  checking source parameters. Mixed-signature dictionary calls now cover this
  independently in the baseline fixture.
- LLVM TCP listen/accept parity still has backend-specific timeout mismatches.
  This is unrelated to typeclass lowering and remains a separate backend
  blocker until its native handle scheduling path is fixed.

## Stage ownership after Stage 1

- Stage 1: lossless predicates, contextual kind checking, kind metadata, and a shared structural matcher.
- Stage 2: one dictionary resolver and strict call-shape validation.
- Stage 3: complete solved/generalized/stuck/diagnosed constraint states.
- Stage 4: complete `ClassId`-aware and result-directed resolution.
- Stage 5 (done): superclass closure, evidence slots, and transitive entailment.
- Stage 6 (done): associated type declarations, equations, reduction, and interfaces.
- Stage 7 (done): safe deriving diagnostics, generated evidence, and dictionaries for structural container predicates.
- Stage 8: standard library hierarchy and instances.

No later stage may depend on an undocumented fallback. Every change to the
calling convention, interface shape, or class metadata must add a Rust test and
a Flux example in the same pull request.
