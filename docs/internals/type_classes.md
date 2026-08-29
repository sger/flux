# Flux Typeclass Architecture

## Purpose

This document records the Stage 0 architecture baseline for Flux typeclasses.
It is an implementation map for the current compiler, not a language proposal
and not a promise that every future typeclass feature already exists.

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

| Subsystem | Producer | Consumer | Current representation | Status and Stage 0 finding |
|---|---|---|---|---|
| Syntax | Lexer/parser | Compiler and inference | `Statement::Class`, `Statement::Instance`, `ClassConstraint`, `TypeExpr` | Supported class, instance, superclass-context, module, and deriving syntax. |
| Class collection | `ClassEnv::collect_from_statements` | Solver, dispatch, interfaces | `ClassDef`, `InstanceDef`, `ClassId`, method metadata | Supported validation for duplicates, methods, arity, visibility, orphan rules, and direct superclass instances. Legacy short-name compatibility callers remain. |
| Class identity | `ClassId` and `ModulePath` | Lookup, dispatch, interface loading | `(module path, class name)` | Storage is ClassId-aware, but some semantic callers still use short-name compatibility lookup. Full migration belongs to Stage 4. |
| HM constraints | Expression/type inference | Solving and scheme construction | `WantedClassConstraint` with class name and full `Vec<InferType>` | Inferred obligations preserve type arguments. Explicit scheme constraints currently retain class name and quantified type variables, which is sufficient for the current single-variable dictionary path but lossy for future structured predicates. Stage 1 owns the replacement. |
| Constraint solving | `class_solver` and class environment | Compiler diagnostics and dispatch | Concrete instance matching plus structural builtin checks | Supported concrete, HKT, contextual, and structural cases have coverage. The solver can skip unresolved variables because they are generalized; complete obligation dispositions belong to Stage 3. |
| Direct dispatch | AST-to-Core lowerer and `class_dispatch` | Core/backend lowering | Mangled `__tc_*` functions | Supported for monomorphic calls. It still has compatibility paths that identify methods by short name and use call-shape heuristics. Stage 4 owns complete-predicate resolution. |
| Dictionary elaboration | `core::passes::dict_elaborate` | Core, Aether, VM, LLVM | `__dict_*` tuples, implicit dictionary parameters, `TupleField` calls | Supported for current constrained functions and HKT forwarding. Exact calling-convention tests are part of Stage 0; removal of all fallback/workaround paths belongs to Stage 2. |
| Ownership/backend lowering | Aether, CFG/LIR, bytecode, LLVM | VM and native execution | Ordinary calls, tuples, globals, and indirect calls | Both backends consume the lowered representation. Stage 0 adds a deterministic typeclass parity smoke test and records unrelated baseline parity failures separately. |
| Superclasses | Parser, class collection, instance validation | Class environment and dispatch | `ClassDef.superclasses`, `InstanceDef.context` | Parsing and direct instance-presence checks are supported. Transitive declaration-order-independent evidence and dictionary superclass slots belong to Stage 5. |
| Kinds | `types::kind`, constructors, inference types | Future type validation | `Kind::Type` and `Kind::Arrow` | Kind values and arity helpers exist, but there is no complete checking pass. Stage 0 records the accepted/rejected baseline; Stage 1 adds validation. |
| Associated types | None in current AST/inference/Core | Future class system | No representation | Not implemented. Stage 6 owns declarations, equations, reduction, stuck applications, and interface metadata. |
| Deriving | ADT/class collection and dispatch generation | Runtime method calls | Structural built-in deriving for current supported classes | Existing deriving cases compile and run. Unsupported deriving behavior is recorded in Stage 0; broader safe deriving and generated method guarantees belong to Stage 7. |
| Interfaces | `compiler::module_interface` | Importing compiler and cache validation | Public class/instance entries plus fingerprints | Public class and instance metadata is serialized and fingerprinted. Stage 0 verifies round-trip and cold/warm behavior; new predicate/kind/associated-type fields belong to later stages. |

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
lookup followed by an indirect call. The dictionary tuple method order is the
class declaration order. Contextual instances are represented as dictionary
constructors with recursively supplied context dictionaries.

The Stage 0 runtime contract is that the callee's implicit dictionary
parameters and the caller's inserted dictionary arguments have exactly the
same count and order. A valid program must not rely on VM-side arity recovery.

## Current architecture problems

The current implementation has more than one route to a class method:

1. Direct AST-to-Core `__tc_*` dispatch.
2. Core dictionary elaboration.
3. AST fallback dictionary insertion for selected paths.
4. Runtime function-call arity validation.
5. Solver-only structural checks for some container types.

These routes are useful compatibility layers, but they can make failures hard
to classify. Stage 0 therefore measures each route and records whether a case
is supported, intentionally deferred, or a reproducible regression.

The most important current limitations are:

- scheme constraints do not yet preserve arbitrary structured predicates;
- unresolved constraints are intentionally deferred to generalization, but the
  compiler does not expose one complete disposition model for every obligation;
- short-name lookup remains in compatibility callers even though `ClassId` is
  available;
- direct dispatch is not yet selected from a complete predicate and expected
  result type;
- superclass metadata is not yet represented as transitive dictionary
  evidence;
- kind values are not enforced by a complete type-application checker;
- associated types have no frontend, solver, or interface representation;
- deriving coverage is limited to the classes and shapes currently supported.

The staged roadmap assigns those limitations to later stages. Stage 0 does not
silently fix them by changing their expected baseline; it adds explicit
fixtures and tests so later stages can change one contract at a time.

## Stage 0 test contract

Baseline fixtures live in `examples/type_classes/` and use
descriptive behavior names. They are loaded by the dedicated Rust contract
tests and the typeclass parity fixture rather than by the broad example
snapshot suite.

| Fixture | Contract measured | Expected Stage 0 result |
|---|---|---|
| `dictionary_call_arity.flx` | Concrete and polymorphic dictionary call shape | Supported behavior executes without an arity error. |
| `generalized_constraint_obligation.flx` | Constraint attached to a generic function | Current supported constraint is preserved through compilation and execution; structured-loss limitations are recorded. |
| `result_directed_method_lookup.flx` | Method whose result could influence selection | Baseline behavior is recorded; result-directed selection remains Stage 4. |
| `invalid_higher_kind.flx` | Invalid type-constructor application | Current baseline is recorded; complete kind rejection remains Stage 1. |
| `unsupported_deriving_diagnostic.flx` | Unsupported deriving request | Diagnostic behavior is stable and explicit; broader deriving remains Stage 7. |
| `TypeclassMetadata.flx` | Public class/instance interface serialization | Metadata survives interface construction and reload. |
| `typeclass_backend_parity.flx` | Supported class dispatch on VM and LLVM | Deterministic output is identical across supported backends. |
| `multiple_class_obligations.flx` | Two independent generalized obligations | Both dictionaries are inserted in declaration order and both methods execute. |
| `superclass_instance_validation.flx` | Existing superclass and contextual-instance validation | Direct superclass evidence and the contextual instance continue to compile and run. |

The Rust tests must reference every fixture. The parity fixture must also be
registered under `tests/parity/` with the standard parity metadata header.

## Baseline blocker list

- Hidden dictionary arity has historically been derived from all constraints,
  including marker-only classes. The shared filtered counter now covers the
  statement and expression paths; a marker-class regression test remains the
  smallest follow-up validation.
- Runtime contract validation must account for injected dictionaries before
  checking source parameters. Mixed-signature dictionary calls now cover this
  independently in the baseline fixture.
- LLVM TCP listen/accept parity still has backend-specific timeout mismatches.
  This is unrelated to typeclass lowering and remains a separate backend
  blocker until its native handle scheduling path is fixed.

## Stage ownership after Stage 0

- Stage 1: one lossless predicate model and real kind checking.
- Stage 2: one dictionary resolver and strict call-shape validation.
- Stage 3: complete solved/generalized/stuck/diagnosed constraint states.
- Stage 4: complete `ClassId`-aware and result-directed resolution.
- Stage 5: superclass closure, evidence slots, and transitive entailment.
- Stage 6: associated type declarations, equations, reduction, and interfaces.
- Stage 7: safe deriving and usable structural dictionaries.
- Stage 8: standard library hierarchy and instances.

No later stage may depend on an undocumented fallback. Every change to the
calling convention, interface shape, or class metadata must add a Rust test and
a Flux example in the same pull request.
