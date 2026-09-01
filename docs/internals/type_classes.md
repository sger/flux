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
| Superclasses | Parser, class collection, instance validation | Class environment and dispatch | `ClassDef.superclasses`, `InstanceDef.context` | Parsing and direct instance-presence checks are supported. Transitive declaration-order-independent evidence and dictionary superclass slots belong to Stage 5. |
| Kinds | `types::kind`, `types::kind_check`, constructors, inference types | Compiler diagnostics and interface consumers | `Kind::Type` and `Kind::Arrow` | Stage 1 validates known constructors, contextual applications, instance heads, constraints, and class-parameter conflicts. Unknown imported constructors remain open at the interface boundary. |
| Associated types | None in current AST/inference/Core | Future class system | No representation | Not implemented. Stage 6 owns declarations, equations, reduction, stuck applications, and interface metadata. |
| Deriving | ADT/class collection and dispatch generation | Runtime method calls | Structural built-in deriving for current supported classes | Existing deriving cases compile and run. Unsupported deriving behavior is recorded; broader safe deriving and generated method guarantees belong to Stage 7. |
| Interfaces | `compiler::module_interface` | Importing compiler and cache validation | Public class/instance entries, structured predicates, kind metadata, and fingerprints | `parameter_kinds` and `head_kinds` are serialized with defaults for old interfaces and included in fingerprints. Associated-type metadata remains future work. |

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

The runtime contract is that the callee's implicit dictionary
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
to classify. The baseline measures each route and records whether a case
is supported, intentionally deferred, or a reproducible regression.

The most important current limitations are:

- scheme constraints do not yet preserve arbitrary structured predicates;
- unresolved constraints are intentionally deferred to generalization, but the
  compiler does not expose one complete disposition model for every obligation;
- short-name lookup remains only at source-resolution and diagnostic boundaries;
- direct dispatch is not yet selected from a complete predicate and expected
  result type;
- superclass metadata is not yet represented as transitive dictionary
  evidence;
- kind values are not enforced by a complete type-application checker;
- associated types have no frontend, solver, or interface representation;
- deriving coverage is limited to the classes and shapes currently supported.

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
| `unsupported_deriving_diagnostic.flx` | Unsupported deriving request | Diagnostic behavior is stable and explicit; broader deriving remains Stage 7. |
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
- Stage 5: superclass closure, evidence slots, and transitive entailment.
- Stage 6: associated type declarations, equations, reduction, and interfaces.
- Stage 7: safe deriving and usable structural dictionaries.
- Stage 8: standard library hierarchy and instances.

No later stage may depend on an undocumented fallback. Every change to the
calling convention, interface shape, or class metadata must add a Rust test and
a Flux example in the same pull request.
