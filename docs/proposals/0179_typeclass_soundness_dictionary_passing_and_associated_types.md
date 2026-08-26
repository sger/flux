- Feature Name: Typeclass Soundness, Dictionary Passing, Syntax, Associated Types, and Standard Hierarchy
- Start Date: 2026-08-26
- Status: Draft
- Proposal PR:
- Flux Issue:
- Depends on: [0145](implemented/0145_type_classes.md), [0146](implemented/0146_type_class_hardening.md), [0147](implemented/0147_constrained_type_params_and_instance_contexts.md), [0150](implemented/0150_hkt_instance_resolution.md), [0151](implemented/0151_module_scoped_type_classes.md), [0168](implemented/0168_hkt_polymorphic_dispatch_completion.md)
- Relates to: [known_issues.md#KI-015](../known_issues.md) (first-argument dispatch), 0139 (`.flxi` interfaces), 0123 (static typing)

# Proposal 0179: Typeclass Soundness, Dictionary Passing, Syntax, Associated Types, and Standard Hierarchy

This is the complete design for Flux's typeclass direction — surface syntax,
constraint solving, dictionary passing, superclass entailment, kind checking,
associated types, deriving, module behavior, the standard class hierarchy,
and a phased implementation roadmap. It is not a Phase 0 syntax proposal; it
is the reference another engineer can implement phase by phase without
rediscovering the language design.

All source citations were verified against this repository at commit
`34318536` (v0.0.7).

## 1. Summary
[summary]: #summary

Flux has a substantial typeclass implementation — single- and multi-parameter
classes, default methods, HKT instances, contextual instances, module-scoped
classes with a relaxed orphan rule, effect rows on methods, static
monomorphic dispatch via generated `__tc_*` functions, and tuple-based
dictionaries for polymorphic dispatch. But the system is unsound at three
levels:

1. **Runtime**: polymorphic dictionary passing crashes with
   `E1000: wrong number of arguments`. A Core dump shows the correct
   elaborated call (`twice(__dict_Sizeable_Int, 21)`), yet the VM receives a
   call with only the visible source arguments. The committed example
   `examples/type_inference/numeric_defaulting_explicit_bound.flx` fails this
   way today (`want=3, got=2`) while its header comment claims it "still
   elaborates through dictionary passing".
2. **Solver**: class obligations are silently discarded on at least four
   paths in `src/types/class_solver.rs` and two more in
   `src/types/class_defaulting.rs`; `Scheme` cannot even represent a
   structured predicate such as `Eq<List<a>>`.
3. **Semantics**: superclass checking is string-rendered and
   declaration-order-sensitive; dispatch is first-argument-directed with a
   name-matched `Decode` special case (KI-015); `src/types/kind.rs` is dead
   code, so `instance Functor<Int>` is not rejected by kind checking;
   unsupported `deriving` produces a method-less instance that satisfies the
   solver while generating no code.

This proposal fixes all of the above and extends the design with the missing
target features: multi-constraint contexts, structured predicates, real kind
checking, principled evidence resolution (including return-type-directed
dispatch, subsuming the `Decode` hack), superclass entailment with dictionary
superclass slots, **associated types** (a required part of the target design,
not an optional extra), safe deriving, and the standard hierarchy
`Eq → Ord`, `Semigroup → Monoid`, `Functor → Applicative → Monad`.

## 2. Motivation
[motivation]: #motivation

Three forces drive this proposal.

**Soundness debt is now user-visible.** A five-line program using an explicit
class bound crashes at runtime (§4.1). The failure is not exotic: any
constrained function whose body reaches the AST bytecode fallback emits calls
without dictionary arguments. Because the compile-time arity check accepts
*either* arity (`src/compiler/expression.rs:1600-1603`), the mismatch
surfaces only at the `OpCall` in the VM. Well-typed programs must never reach
the VM with a dictionary arity mismatch; today they routinely do.

**The feature surface has outrun the semantic foundation.** HKT genuinely
works — `instance Functor<List>` compiles and `fmap` executes — but it works
through unification and structural head-matching, not kind checking.
Proposal 0150 already flagged this: *"A malformed instance like
`instance Functor<Int>` … would match incorrectly. This should be caught by
kind checking during instance validation"*
(`docs/proposals/implemented/0150_hkt_instance_resolution.md:153`). Nothing
picked that up. Similarly, multi-parameter classes parse and mangle
correctly, but the solver drops any predicate mentioning a type variable, so
soundness rests on which programs users happen to write.

**The stdlib needs the hierarchy, and the hierarchy needs the fixes.**
`Semigroup`/`Monoid`/`Functor`/`Applicative`/`Monad` cannot ship until
(a) dictionaries execute correctly, (b) superclass evidence exists (a `Monoid`
dictionary must contain its `Semigroup` evidence), and (c) return-position
methods (`pure`, `mempty`) dispatch — which requires the general
result-directed resolution that KI-015 asks for. Associated types are
required for the container abstractions the stdlib wants
(`class Collection<c> { type Element<c> … }`) and were repeatedly deferred
(0145 §alternatives, 0146:383, 0147:330); this proposal makes them part of
the core roadmap.

## 3. Current Flux capabilities
[current-flux-capabilities]: #current-flux-capabilities

An honest inventory, in five buckets. "Works" means *verified to compile and
execute correctly*, not merely to compile.

### 3.1 Genuinely works today

- **Single-parameter classes with monomorphic dispatch.** `class Eq<a>`,
  `instance Eq<Int>`, concrete call sites. Dispatch resolves the first
  argument's HM type and rewrites the call to the mangled
  `__tc_{Class}_{Type}_{method}` function
  (`src/core/lower_ast/mod.rs`, `try_resolve_class_call`, line 441;
  mirrored in `src/compiler/expression.rs:4552-4620`).
- **Default methods.** `ClassMethod.default_body`
  (`src/syntax/type_class.rs:29-43`); instance methods fall back to the
  default in dispatch generation (`src/types/class_dispatch.rs:1051`).
- **Per-method type parameters.** `fn fmap<a, b>(…)` inside a class body
  (`ClassMethod.type_params`).
- **HKT classes and instances through unification.**
  `class Functor<f> { fn fmap<a, b>(fa: f<a>, g: (a) -> b) -> f<b> }` with
  `instance Functor<List>` compiles and executes
  (`examples/strict_types/type_class_functor.flx` prints `[2, 4, 6]`).
  Matching handles `InferType::HktApp` heads
  (`src/types/class_env.rs:1978-1990`); 0168 wired the constraint-matching
  case (`src/core/lower_ast/mod.rs`, `match_constraint_type_var`).
- **Module-scoped classes and instances** (0151): `ClassId =
  (ModulePath, ClassName)` (`src/types/class_id.rs`), `public class` /
  `public instance` visibility (E450/E451/E455), qualified class-method
  calls (`src/compiler/expression.rs:916`), and `.flxi` serialization of
  public classes/instances (`src/types/module_interface.rs:55-159`).
- **Effect rows on methods** (`ClassMethod.effects`), including
  per-instance effect rows (`docs/internals/modules.md:331`).
- **A relaxed orphan rule.** `instance C<T>` in module `M` is legal iff `M`
  defines `C` or `M` owns the head constructor of the first type argument
  (`src/types/class_env.rs:614-662`, E449).
- **Singleton instance contexts, monomorphically.**
  `instance Eq<a> => Eq<List<a>>` parses, and contextual dictionaries are
  built for concrete uses
  (tests: `tests/vm_runtime/contextual_instance_runtime_tests.rs`).
- **Deriving for built-in shapes.** `deriving (Eq, Ord, Show, Encode,
  Decode)` synthesizes bodies (`src/types/class_dispatch.rs:154-249`,
  `derived_json_method_body:306`).

### 3.2 Compiles but is not runtime-safe

- **Polymorphic dictionary passing.** Any call to a constrained function
  from a context where the callee is not monomorphically resolvable — or
  where the *caller's own function body* is compiled via the AST bytecode
  fallback — crashes with E1000 (§4.1). The Core dump looks correct;
  execution fails.
- **`fn f<a: Num>` with explicit bounds.** The committed example
  `examples/type_inference/numeric_defaulting_explicit_bound.flx` fails at
  runtime with `E1000: wrong number of arguments: want=3, got=2` (verified
  2026-08-26 with `--no-cache`).
- **Unsupported deriving.** `deriving (SomeClass)` for a class with no
  built-in derivation registers a method-less `InstanceDef`
  (`src/types/class_env.rs:1188-1198`) which the solver counts as satisfied,
  while `generate_builtin_instance_functions` silently emits nothing
  (`src/types/class_dispatch.rs:185-201`). Calls then hit the polymorphic
  panic stub or an undefined symbol.
- **Contextual dictionaries in the VM's global emitter.**
  `emit_dict_globals` (`src/compiler/passes/codegen.rs:83-153`) requires a
  dictionary body to be `MakeTuple` of `Var`s; a contextual
  (function-valued) dictionary fails the check and is silently dropped.

### 3.3 Partially implemented

- **Constraint solving.** `solve_class_constraints`
  (`src/types/class_solver.rs:30`) checks only fully concrete predicates;
  everything mentioning a type variable is skipped (§4.2).
- **Scheme constraints.** `SchemeConstraint { class_name, type_vars }`
  (`src/ast/type_infer/constraint.rs:97-103`) can represent `Eq<a>` but not
  `Eq<List<a>>` or `Convert<a, Int>`; `collect_scheme_constraints` drops
  anything it cannot represent (`src/types/class_defaulting.rs:166-168`).
- **Superclasses.** Parsed (singleton only), stored, checked by rendering
  both sides to strings and comparing against the instances accumulated *so
  far* (declaration-order-sensitive; E445). No superclass evidence exists
  in dictionaries (§4.3).
- **Multi-parameter classes.** Parse, mangle (args joined with `_`,
  `src/types/class_dispatch.rs:1030`), and resolve — but only when the
  first argument disambiguates, because dispatch is first-argument-directed
  (§4.4).
- **Class identity.** `ClassId` exists and keys `ClassEnv.classes`, but the
  solver, defaulting, and lowering paths all use the bare-short-name
  compatibility shims (`lookup_class`, `resolve_instance_with_subst`,
  `method_to_class` — `src/types/class_env.rs:1236-1292, 1392`), which are
  module-blind and first-match.
- **`.flxi` class metadata.** `PublicClassEntry.superclasses` stores short
  names only; the source comment says *"full ClassId resolution for
  superclasses lands in a later phase"*
  (`src/types/module_interface.rs:29-31`).

### 3.4 Missing

- Multi-constraint superclass lists and instance contexts (parser accepts
  at most one constraint before `=>`; §4.5).
- Kind checking of any form (§4.8). `src/types/kind.rs` has one producer
  (`TypeConstructor::kind`, `src/types/type_constructor.rs:47`) and zero
  consumers.
- Structured predicates in bounds: `fn f<a: Convert<Int>>` is not
  expressible; bounds are bare identifiers
  (`src/syntax/statement.rs:20`, `FunctionTypeParam`).
- Associated types, in any form.
- Superclass dictionary slots / selectors; result-directed dispatch
  (general form); ambiguity checking; residual-constraint reporting.
- The standard hierarchy above `Eq`/`Ord`/`Show`/`Num`:
  no `Semigroup`, `Monoid`, `Functor`-as-stdlib-class, `Applicative`,
  `Monad`.

### 3.5 Deliberately omitted (target design)

Flux keeps these choices unless future requirements prove otherwise:

- **No overlapping instances** and **no incoherent instances** — one
  matching instance per predicate, coherently, or a compile error.
- **No instance chains.**
- **No monomorphism restriction.**
- **Simpler defaulting initially** — `Num`-to-`Int` style, not a
  pluggable multi-strategy defaulting pipeline.
- **Static evidence passing rather than runtime type inspection** —
  dictionaries are ordinary values chosen at compile time; the VM never
  inspects a runtime type to pick an instance.
- Deferred (not rejected): functional dependencies, quantified
  constraints, deriving strategies, `MINIMAL`-style declarations,
  size-based termination analysis, associated-type extensions
  beyond §8. Associated types themselves are **not** deferred.

## 4. Confirmed defects
[confirmed-defects]: #confirmed-defects

Each defect below was re-verified against the working tree on 2026-08-26.

### 4.1 Defect 1 — Dictionary arity mismatch at runtime

**Reproduction** (fails today, VM backend, `--no-cache`):

```flux
class Sizeable<a> {
    fn size(x: a) -> Int
}

instance Sizeable<Int> {
    fn size(x) { x }
}

fn twice<a: Sizeable>(x: a) -> Int {
    size(x) + size(x)
}

fn main() with IO {
    print(twice(21))
}
```

Expected `42`; actual
`error[E1000]: wrong number of arguments: want=2, got=1` at `twice(21)`.
The committed example
`examples/type_inference/numeric_defaulting_explicit_bound.flx` fails
identically (`want=3, got=2`) while its comment claims dictionary
elaboration works.

**Root cause — duplicated lowering paths with one-sided elaboration.**
Flux has one VM pipeline but **two per-function codegen paths**, selected
function-by-function at bytecode emission:

- The Core/Aether path always runs for the whole program:
  `src/cfg/mod.rs:70` (`lower_program_to_ir_typed`) → AST→Core lowering →
  **dictionary elaboration** (`src/cfg/mod.rs:110-119`) → Aether → IR.
  Dictionary *elaboration* (hidden `__dict_*` parameters, method-to-
  projection rewrites, dict forwarding) happens only here, in
  `src/core/passes/dict_elaborate.rs`. Concrete dictionary *arguments* at
  call sites are inserted only during AST→Core lowering
  (`src/core/lower_ast/expression.rs:188-204`, calling
  `resolve_dict_args_for_call` / `resolve_direct_class_call_dict_args`
  from `src/core/lower_ast/mod.rs:532-664`).
- Bytecode emission then chooses per function between the CFG-derived body
  and an **AST-direct fallback** (`src/compiler/statement.rs:1949-1976`):
  `requires_ir_only` forces dict-bearing functions onto CFG when
  `ir_function.params.len() != parameters.len()` or the body contains
  constrained calls, *but* the AST fallback still triggers when
  `ir_function.is_none()`, when the body contains CFG-incompatible
  statements, or when CFG compilation errors and rolls back
  (`src/compiler/statement.rs:1994-1999`). The AST path has **no**
  dictionary insertion for constrained ordinary functions (its dict logic
  covers only direct class-method calls,
  `src/compiler/expression.rs:4621-4650`), so it emits the visible-arity
  call against a callee compiled with hidden dict parameters → E1000.

Three aggravating factors:

1. `resolve_dict_arg` in `src/core/passes/dict_elaborate.rs:1001-1022` —
   the Core-pass call-site resolver — implements only polymorphic
   forwarding. Its concrete-dictionary branch is a bare `None` with the
   comment *"TODO: When type info is available (e.g., from hm_expr_types),
   resolve to `Var(__dict_{Class}_{Type})`"*; `_class_env` is
   underscore-unused. Because `insert_dict_args_at_call_sites` prepends
   dict args only when `!dict_args.is_empty()`
   (`dict_elaborate.rs:648`), a failed resolution silently produces a
   visible-arity call instead of an error.
2. The compile-time arity check accepts *either* arity:
   `src/compiler/expression.rs:1600-1603` allows `raw_expected`,
   `visible_expected`, or `raw_expected + hidden_dicts` — static tolerance
   that converts a compiler bug into a runtime crash.
3. The VM already carries a band-aid at the wrong layer:
   `src/vm/function_call.rs:221-239` silently *discards* extra leading
   arguments for callees whose debug-info name starts with `__tc_`. It
   keys on a debug-string prefix, handles only the too-many-args
   direction, and does not apply to user constrained functions.

**A Core dump is not sufficient evidence of runtime correctness.** For the
reproduction above, `--dump-core` shows both the elaborated definition
(`letrec twice = λSizeable, x. …`) and the elaborated call
(`twice(__dict_Sizeable_Int, 21)`) — and execution still fails, because the
executed bytecode for the calling function came from the AST fallback.

**Requirement.** This proposal chooses **one authoritative evidence-passing
pipeline**: for any function whose scheme has constraints, or whose body
contains a constrained call, the CFG (Core-derived) body is the only legal
backend; the AST fallback must either be removed for such functions or
taught full dictionary insertion with the same shared resolver (§12). In
either case, a compiler invariant must check, at bytecode finalization,
that every emitted call's argument count matches the callee's compiled
parameter count for statically-known callees — a mismatch is an ICE, not a
deferred E1000. The native path (`src/lir/`, `src/llvm/`) is uniformly
Core-derived and already structurally consistent
(`src/lir/lower.rs:5353`, `is_dict_def` propagation;
`src/lir/emit_llvm.rs:95-96`); it must be covered by the same execution
tests (§22) rather than assumed correct.

### 4.2 Defect 2 — Class obligations disappear

`solve_class_constraints` (`src/types/class_solver.rs:30`) silently
discards obligations on four paths in its main loop (lines 37-65):

1. `origin == InferredOperator && !originated_from_concrete_type` — dropped
   (lines 41-44).
2. Any constraint whose type args are not all concrete —
   `is_solvable_type_arg` (line 295) — dropped (lines 51-53). This is the
   main hole: `Foo<a>` is never an error and never a residual obligation.
3. Any constraint with `span == Span::default()` — i.e. everything
   compiler-generated — unchecked (lines 57-59).
4. Unknown class — skipped, deferring to E441 (lines 63-65).

`collect_scheme_constraints` (`src/types/class_defaulting.rs:141`) then
drops more: every `InferredOperator` constraint outright (line 155), and
any constraint whose args are not all bare `InferType::Var`s in the type's
free variables (lines 166-168) — so `Eq<List<a>>` and `Convert<a, Int>`
vanish. The representational cause is `SchemeConstraint { class_name,
type_vars: Vec<TypeVarId> }` (`src/ast/type_infer/constraint.rs:97-103`):
a scheme predicate cannot mention a structured type. `Scheme::generalize`
(`src/types/scheme.rs:192`) unconditionally produces
`constraints: Vec::new()`; only `generalize_with_constraints` (line 213)
preserves anything, and it filters to fully-quantified bare-var predicates.

**Requirement.** Full predicate preservation. The checker must classify
every wanted predicate as exactly one of: **solved** (evidence found),
**generalized** (moved into the enclosing scheme, any shape — `Eq<a>`,
`Eq<List<a>>`, `Convert<a, String>`), **equality**
(`Collection.Element<c> ~ a`, §15), **stuck** (an associated-type
application blocked on an abstract type, §15), **ambiguous** (a variable
constrained but not determined by the type — an error at generalization),
or **missing-instance** (E444). No path may `continue` past a predicate
without assigning it a bucket. Valid generalized declarations such as
`fn f<a: Eq>(x: a) -> Bool` must not be rejected, and generic operator use
(`a + b` in a generic function) must yield a residual `Num<a>` on the
scheme instead of being discarded.

### 4.3 Defect 3 — Superclasses are not entailment

Current state: superclass constraints parse only as a single pre-`=>`
predicate whose type args are degraded to bare identifiers
(`src/syntax/parser/statement.rs:1660-1698`); satisfaction is checked by
**rendering both sides to strings and comparing** against the instances
accumulated so far (`src/types/class_env.rs:1027-1037`, E445) — so the
check is declaration-order-sensitive and matches on text, not structure.
There is no superclass evidence: an `Ord<Int>` dictionary does not contain
or reach an `Eq<Int>` dictionary, so a generic
`fn sort<a: Ord>` body cannot call `eq`.

**Requirement** (design in §14): structural, `ClassId`-keyed superclass
matching; declaration-order independence (check after the whole
`ClassEnv` is built); immediate *and* transitive entailment
(`Ord<T>` given ⊢ `Eq<T>`); dictionary layout =
`(superclass dicts…, methods…)` with compiler-generated superclass
selectors; and entailment aware of associated-type equalities in
superclass positions (deferred to Phase 6+ where they interact).

### 4.4 Defect 4 — First-argument-directed dispatch

Dispatch resolves the instance from the first value argument's inferred
type (`src/core/lower_ast/mod.rs:483-504`,
`resolve_method_call_instance_from_first_arg` at
`src/types/class_env.rs:1426`), with `method_to_class` returning the
*first* class declaring a method name (`class_env.rs:1285-1292`). A class
whose variable appears only in the return position cannot dispatch — this
is **KI-015** (`docs/known_issues.md`). `Flow.Json`'s `Decode` works only
via a string-matched special case in four places
(`src/core/lower_ast/mod.rs:453-478` and `:552-562`;
`src/compiler/expression.rs:4566` and `:4673`) keyed on the method name
`decode` and class short name `Decode`.

**Requirement** (design in §12): full predicate solving over all class
parameters, all argument types, the expected result type, associated-type
equalities, contextual evidence, and module-qualified class identity.
Remove the `Decode` hack once general resolution exists. Runtime type
inspection is explicitly **not** the solution — resolution stays static.

### 4.5 Defect 5 — Incomplete context parser

`parse_class_statement` (`src/syntax/parser/statement.rs:1637-1733`) and
`parse_instance_statement` (lines 1854-1903) each accept **at most one**
constraint before `=>`; there is no parenthesized form and no chaining.
Superclass type args are rebuilt from bare identifiers (lines 1665-1672),
so a superclass can never be `Eq<List<a>>`. Function bounds
(`parse_function_type_params_angle_bracket`, lines 2079-2136) accept
`a: Eq + Show` but only bare class names — no type arguments. Notably, the
AST already holds `Vec<ClassConstraint>` everywhere
(`src/syntax/statement.rs:161-191`), and `deriving` synthesis already
pushes multi-element contexts into it (`src/types/class_env.rs:1180-1187`)
— the parser is the only bottleneck. The parser and AST must preserve the
complete constraint list; a multi-constraint context must not be
representable only as a parser-level singleton vector.

### 4.6 Defect 6 — Non-structural deriving

`collect_deriving` (`src/types/class_env.rs:1115-1214`) registers a
method-less `InstanceDef` (`method_names: vec![]`,
`span: Span::default()`) for every derived class; body synthesis
(`src/types/class_dispatch.rs:154-249`) then `continue`s for any class it
cannot derive (lines 185-201) — no diagnostic, no methods. The instance
still satisfies the solver and the E445 superclass check. Unsupported
deriving must be a compile-time error (§16).

### 4.7 Defect 7 — Structural instance solver hacks

`has_structural_builtin_instance` (`src/types/class_solver.rs:164`)
hardcodes `"Eq" | "Ord" | "Sendable"` by **resolved string name** (so a
user's own module-scoped `Eq` inherits the structural rules), covers
tuples, `Option`/`List`/`Array` (first arg only), `Map`
(Sendable only), and `Either` — and produces **no evidence**: it answers
the solver's yes/no question but there is no corresponding dictionary, so
a polymorphic consumer of, say, `Eq<(Int, Int)>` has nothing to receive.
Every structural rule must correspond to a real evidence-producing
dictionary or a clearly defined compiler-owned evidence mechanism (§16.3),
for `Eq`, `Ord`, `Sendable`, tuples, `Option`, `List`, `Array`, `Either`,
and future associated-type-aware containers.

### 4.8 Defect 8 — No kind checking

`src/types/kind.rs` defines `Kind { Type, Arrow }` (Proposal 0123 Phase 5)
with exactly one producer — `TypeConstructor::kind()`
(`src/types/type_constructor.rs:47`), which itself has zero call sites,
and which claims `Adt(_) => Kind::Type` for every user ADT regardless of
arity. There is no kind inference, no kind checking of class parameters,
instance heads, HKT applications, associated types, derived instances, or
multi-parameter class arguments. HKT correctness rests entirely on
structural matching in `match_instance_type_expr`
(`src/types/class_env.rs:1945`), where "is a type variable" means "first
character is ASCII lowercase" (line 2018). Genuine kind checking is
specified in §10.

## 5. Goals
[goals]: #goals

1. Well-typed programs never reach the VM (or native runtime) with a
   dictionary arity mismatch — enforced by a compiler invariant, tested by
   execution.
2. One authoritative evidence-passing pipeline for the VM; the native path
   specified and tested against the same fixtures.
3. No silent dropping of class obligations; every predicate is solved,
   generalized, residual-stuck, or an error.
4. Structural, order-independent superclass entailment with superclass
   evidence in dictionaries.
5. Principled, complete-predicate instance resolution including
   result-directed dispatch; the `Decode` special case removed.
6. Multi-constraint contexts and superclass lists in the surface syntax.
7. Real kind checking for constructors, class parameters, instance heads,
   HKT applications, associated types, and derived instances.
8. Associated types: coherent, open, class-attached type families with one
   equation per instance, reduced at compile time, never present in
   runtime dictionaries.
9. Safe deriving: unsupported deriving is a compile error; supported
   deriving is structural and produces real instances.
10. The standard hierarchy `Eq → Ord`, `Semigroup → Monoid`,
    `Functor → Applicative → Monad` in `lib/Flow`.
11. Module/interface behavior (`.flxi`) and cache invalidation specified
    for every new piece of metadata.

## 6. Non-goals
[non-goals]: #non-goals

- Overlapping or incoherent instances; instance chains (§3.5).
- Functional dependencies (associated types are Flux's chosen mechanism;
  fundeps may be evaluated in Phase 9).
- Quantified constraints, higher-rank constraint contexts.
- Deriving strategies (`stock`/`newtype`/`anyclass`/`via`),
  standalone deriving, `MINIMAL` declarations — Phase 9 candidates.
- Size-based instance termination analysis. Flux initially restricts
  instance contexts to structurally smaller predicates by construction
  (§12.5) rather than implementing a size metric.
- Runtime type inspection as a dispatch mechanism.
- A monomorphism restriction.
- Changes to the effect system beyond keeping effect rows working on class
  methods.

## 7. Flux typeclass syntax
[flux-typeclass-syntax]: #flux-typeclass-syntax

Syntax is a required feature of this proposal, not an implementation
detail. Everything in this section is target surface syntax; §9 gives the
grammar and AST changes.

### 7.1 Basic class declarations (unchanged)

```flux
class Eq<a> {
    fn eq(x: a, y: a) -> Bool
    fn neq(x: a, y: a) -> Bool {
        not(eq(x, y))
    }
}
```

Default methods, per-method type parameters, and effect rows on methods
keep their current syntax.

### 7.2 Superclasses — single and multiple

The existing shorthand is preserved:

```flux
class Eq<a> => Ord<a> {
    fn compare(x: a, y: a) -> Int
}
```

Multiple superclasses use a parenthesized context (recommended canonical
form for more than one):

```flux
class (Eq<a>, Show<a>) => Ord<a> {
    fn compare(x: a, y: a) -> Int
}
```

Rules:

- `class C1<…> => C2<…>` ≡ `class (C1<…>) => C2<…>`; the unparenthesized
  form is sugar for a one-element context.
- Chained `=>` (`class A<a> => B<a> => C<a>`) is **not** accepted; the
  parenthesized list is the only multi-constraint form. E-code: reuse the
  existing parser diagnostics with a hint pointing at the list form.
- Superclass constraints carry full `TypeExpr` arguments. `class
  (Eq<List<a>>) => Foo<a>` is grammatically legal; whether it is
  *semantically* accepted is a solver question (§14.5), not a parser one.
  The current parser's degradation of superclass args to bare identifiers
  (`src/syntax/parser/statement.rs:1665-1672`) is removed.

### 7.3 Function typeclass bounds

```flux
fn twice<a: Sizeable>(x: a) -> Int {
    size(x) + size(x)
}

fn describe<a: Eq + Show>(x: a) -> String {
    ...
}
```

**`+` is the canonical function-bound syntax.** Parenthesized predicate
lists are *not* accepted in bound position — bounds stay per-parameter,
Rust-style, because that is what Flux users already write and what the
existing parser accepts (`parse_function_type_params_angle_bracket`,
`src/syntax/parser/statement.rs:2079-2136`).

Bounds are extended to accept type arguments, for multi-parameter classes
where the bound variable is the *first* parameter:

```flux
fn stringify<a: Convert<String>>(x: a) -> String {
    convert(x)
}
// a: Convert<String>  desugars to the predicate  Convert<a, String>
```

The bound `a: C<T1, …, Tn>` desugars to the predicate `C<a, T1, …, Tn>`.
Predicates that do not fit this shape (the constrained variable not in the
first position, or predicates over non-parameters such as `Eq<List<a>>`)
are written in a `where` clause (§7.8) or arise through inference and are
carried on the scheme (§11).

### 7.4 Contextual instances — single and multiple

```flux
instance Eq<a> => Eq<List<a>> {
    fn eq(xs, ys) { ... }
}

instance (Eq<a>, Show<a>) => Eq<List<a>> {
    fn eq(xs, ys) { ... }
}
```

Same sugar rule as classes: bare single constraint ≡ one-element
parenthesized list. Context constraints keep full `TypeExpr` arguments
(the current parser already preserves these for the singleton case,
`src/syntax/parser/statement.rs:1869`). The AST's existing
`context: Vec<ClassConstraint>` field is filled with the complete list —
never a parser-level singleton.

### 7.5 Multi-parameter classes (unchanged syntax)

```flux
class Convert<a, b> {
    fn convert(x: a) -> b
}

instance Convert<Int, String> {
    fn convert(x) { int_to_string(x) }
}
```

What changes is resolution (§12), not syntax.

### 7.6 Higher-kinded classes

```flux
class Functor<f> {
    fn fmap<a, b>(fa: f<a>, g: (a) -> b) -> f<b>
}

instance Functor<List> {
    fn fmap(xs, g) { ... }
}
```

Kinds are **inferred**, not annotated. The class parameter `f` gets its
kind from its uses in the method signatures: `f<a>` forces
`f : Type -> Type`. Explicit kind annotations are deliberately omitted
from the surface language until inference proves insufficient (§10.2;
open question in §24). An instance head must match the inferred parameter
kind: `instance Functor<Int>` becomes a kind error (new E-code, §20),
closing the gap 0150 documented.

### 7.7 Qualified and module-scoped classes

Existing 0151 semantics are kept and completed:

- **Class identity** is `ClassId = (ModulePath, ClassName)`
  (`src/types/class_id.rs`) — and after this proposal, resolution paths
  actually *use* it (§12.2) instead of the short-name shims.
- **Qualified class references** in bounds and heads:
  `fn f<a: Json.Encode>(…)`, `instance Json.Encode<MyType> { … }`.
- **Qualified instances**: an instance may name an imported class with its
  module qualifier; the orphan rule (§17.2) applies to the resolved
  `ClassId`, not the spelling.
- **Visibility**: `public class` / `public instance` keep E450/E451/E455.
- **Same short name in two modules**: legal. An unqualified reference
  resolves to (1) a class defined in the current module, else (2) a
  uniquely imported class of that name; if two imports collide, the
  reference is ambiguous (new diagnostic, §20) and must be qualified.
  Method-name dispatch across same-named methods of *different* classes
  follows the same rule applied to the candidate class set (§12.3),
  replacing today's first-match `method_to_class`
  (`src/types/class_env.rs:1285-1292`).
- The qualified-class-method rule stays: a qualified call `M.method(x)`
  dispatches as a class method only when the qualifier names the class
  (existing behavior, `src/ast/type_infer/expression/calls.rs:502`).

### 7.8 `where` clauses on functions

Bounds (§7.3) cover the common shape `C<a, …>` on a single parameter.
For every other predicate — a constrained non-first parameter, a
structured argument, or several parameters at once — a function may carry
a `where` clause between its signature and its body, after the return
type and any effect row:

```flux
fn dedup<a>(xs: List<a>) -> List<a> where Eq<List<a>> {
    ...
}

fn transcode<a, b>(x: a) -> b with IO where Convert<a, b> {
    ...
}
```

Rules:

- `where` is followed by one or more comma-separated full predicates
  (class name, optionally qualified, with complete `TypeExpr` arguments).
- Bounds and `where` predicates are unioned into the function's scheme
  constraints; `a: Eq + Show` is exactly sugar for
  `where Eq<a>, Show<a>`. Duplicates are deduplicated silently.
- Every type variable mentioned in a `where` predicate must be one of the
  function's type parameters; anything else is an error.
- Equality predicates (`Element<c> ~ a`) are **not** writable in `where`
  in the initial design (Phase 9, §21).

**Relation to the existing body-level `where`.** Flux already uses
`where` for value bindings *inside* a function body
(`where x = val …` after the body expression;
`parse_where_clauses`, `src/syntax/parser/helpers.rs:1272-1326`, with
`parse_block_with_context` treating `where` as a block terminator). The
two uses occupy disjoint grammatical positions — the constraint clause
appears strictly between the signature and the opening `{`, the binding
form strictly inside a block — so the parser never has to choose. The
follow token also differs (`Ident <`/`Ident ,`/`Ident {` for a
predicate vs `Ident =` for a binding), which the E-code hints should use
when a user puts one where the other belongs.

## 8. Associated type syntax
[associated-type-syntax]: #associated-type-syntax

Associated types are **mandatory** for the target design. They are
compile-time type-level members of classes — never runtime dictionary
fields.

### 8.1 Declaration

Flux-style type application syntax, inside the class body:

```flux
class Collection<c> {
    type Element<c>

    fn empty() -> c
    fn insert(x: Element<c>, xs: c) -> c
}
```

`type Element<c>` declares an open type family `Element` attached to class
`Collection`, with the class parameter list as its argument list. The
declared parameters must be exactly the class parameters, in order, in the
initial design (§8.4 relaxes this in Phase 9).

### 8.2 Equations in instances

```flux
instance Collection<List<a>> {
    type Element = a

    fn empty() { [] }
    fn insert(x, xs) { [x | xs] }
}
```

The equation form is `type <name> = <rhs>`. The equation's left-hand
arguments are **implicit**: they are always exactly the instance head's
type arguments, so the head is not restated. (An earlier draft used
`type Element<List<a>> = a`; since the arguments are required to match
the head verbatim, restating them added only an error opportunity, and
the short form is canonical. The declaration in the *class* keeps its
explicit parameters, `type Element<c>`, because there they carry
information — they are what appears at use sites.) The right-hand side
may mention only variables bound by the instance head. Exactly one
equation per declared associated type per instance; a missing equation is
an error unless the class declares a default (Phase 9); a duplicate is an
error.

### 8.3 Use sites

Associated types may appear in method signatures (as above) and in
ordinary function signatures:

```flux
fn first<c: Collection>(xs: c) -> Option<Element<c>> {
    ...
}
```

**Reference outside class scope.** A bare `Element<c>` resolves like any
type name: (1) an associated type of a class defined or imported in the
current module, if unique; otherwise it is ambiguous and must be
qualified with the *class* name:

```flux
Collection.Element<List<Int>>
```

The qualified form `Class.Assoc<args>` (optionally
`Module.Class.Assoc<args>`) is always available and is the canonical
rendering in diagnostics. Ambiguity between an associated type and an
ordinary type of the same name is resolved in favor of the ordinary type,
with a warning-level hint suggesting qualification.

### 8.4 Semantics (normative list)

- **Identity**: an associated type is identified by
  `(module, class, member)` — the owning class's `ClassId` plus the member
  name. Two classes may both declare `Element` without conflict.
- **Kinds**: the associated type's kind is inferred from its uses in the
  class body, exactly like class parameters (§10.2); its arity is the
  class's parameter count in the initial design.
- **Additional parameters** beyond the class parameters: **Phase 9**.
- **May mention all class parameters**: yes — the declaration lists all of
  them; an equation instantiates all of them.
- **In superclass contexts / instance contexts / function bounds**: not in
  the initial design. Predicates in those positions are class predicates
  only; associated types appear in *types*, and equalities over them
  (`Element<c> ~ a`) arise from unification, not from user syntax.
  User-visible equality constraints are Phase 9 (§21).
- **Reduction**: `Element<List<Int>> → Int` by matching the unique
  instance equation (§15.1).
- **Stuck applications**: `Element<c>` with `c` abstract does not reduce;
  it is a first-class type form that unifies only with itself (same
  family, unifiable arguments) or via a known equality (§15.2).
- **Substitution**: substitution applies inside associated-type
  applications like any type application, and re-attempts reduction
  afterward (§15.3).
- **Overlap**: overlapping associated-type equations are rejected exactly
  as overlapping instances are — one equation per family per instance
  head, and instance heads themselves may not overlap (§12.4).
- **Orphans**: an equation lives inside its instance; the instance's
  orphan rule (§17.2) is the only orphan rule needed.
- **Defaults, conflicting-equation strategies beyond rejection**: Phase 9.
- **Serialization / cache**: §17.3–17.4. **Diagnostics**: §20.
- **Runtime representation**: none. Dictionaries contain runtime method
  evidence only. If an associated-type application is still unreduced when
  a value representation is needed, the value is represented uniformly
  (boxed, `FluxRep::BoxedRep`) exactly as a type-variable-typed value is
  today — an unresolved associated type is *representationally* a type
  variable.

### 8.5 Implementation boundaries

Associated types are not "another kind of method". The stages, each
separately testable, and their phase assignments (§21):

1. Declarations and parsing (Phase 0).
2. Kind checking (Phase 1/6).
3. Instance equation collection (Phase 6).
4. Structural instance matching for equations (Phase 6).
5. Type-level reduction (Phase 6).
6. Inference integration — unification with stuck applications (Phase 6).
7. Generalization of stuck applications into schemes (Phase 6).
8. Module/interface serialization (Phase 6).
9. Runtime representation resolution (Phase 6, trivial given §8.4's
   "boxed like a type variable" rule).
10. Defaults and advanced features (Phase 9).

## 9. Grammar and AST representation
[grammar-and-ast]: #grammar-and-ast

### 9.1 Grammar deltas (EBNF-ish)

```
class_decl    ::= "public"? "class" context? class_head "{" class_item* "}"
instance_decl ::= "public"? "instance" context? instance_head "{" instance_item* "}"

context       ::= constraint "=>"                     -- singleton sugar
                | "(" constraint ("," constraint)* ")" "=>"

constraint    ::= qualified_ident type_args?          -- full TypeExpr args
class_head    ::= ident "<" ident ("," ident)* ">"
instance_head ::= qualified_ident "<" type_expr ("," type_expr)* ">"

class_item    ::= class_method | assoc_type_decl
assoc_type_decl ::= "type" ident "<" ident ("," ident)* ">"

instance_item ::= instance_method | assoc_type_eqn
assoc_type_eqn  ::= "type" ident "=" type_expr
                   -- LHS args are implicit: always the instance head's

bound         ::= ident ":" bound_pred ("+" bound_pred)*
bound_pred    ::= qualified_ident type_args?          -- a: C  or  a: C<T,...>

fn_where      ::= "where" constraint ("," constraint)*
                   -- after return type and effect row, before the body `{`
```

Associated-type *references* (`Element<c>`,
`Collection.Element<List<Int>>`) have **no distinct production**: they
are token-for-token ordinary (possibly qualified) type applications, so
the parser produces a plain named `TypeExpr` and **name resolution**
reclassifies it as an associated-type application when the name resolves
to a class member (§9.2). The same applies to the dotted qualifier —
`Collection.Element` vs `Module.Type` is a lookup distinction, not a
grammatical one.

### 9.2 AST changes

Existing nodes already shaped correctly (no change except that the parser
fills them fully):

- `Statement::Class { superclasses: Vec<ClassConstraint>, … }` and
  `Statement::Instance { context: Vec<ClassConstraint>, … }`
  (`src/syntax/statement.rs:161-191`) — the vectors become genuinely
  multi-element; superclass constraints get real `TypeExpr` args.
- `ClassConstraint { class_name, type_args: Vec<TypeExpr>, span }`
  (`src/syntax/type_class.rs:12-16`) — unchanged; it already derives
  `Serialize`/`Deserialize` for `.flxi`.

New nodes:

- `FunctionTypeParam.constraints` changes from `Vec<Identifier>` to
  `Vec<BoundPred>` where
  `BoundPred { class_name: Identifier, type_args: Vec<TypeExpr>, span }`
  (`src/syntax/statement.rs:20`). A bare `a: Eq` is
  `BoundPred { class_name: Eq, type_args: [] }`.
- `ClassAssocType { name: Identifier, params: Vec<Identifier>, span }`,
  added to `Statement::Class` as `assoc_types: Vec<ClassAssocType>`.
- `InstanceAssocTypeEqn { name: Identifier, rhs: TypeExpr, span }`,
  added to `Statement::Instance` as
  `assoc_type_eqns: Vec<InstanceAssocTypeEqn>`. No LHS field: the
  equation's arguments are the instance's `type_args` by definition.
- `Statement::Function` gains `where_preds: Vec<ClassConstraint>` for the
  §7.8 clause (reusing `ClassConstraint`, which already carries full
  `TypeExpr` args and serializes).
- `TypeExpr::AssocApp { class: Option<QualifiedName>, name: Identifier,
  args: Vec<TypeExpr>, span }` for `Element<c>` /
  `Collection.Element<List<Int>>`. This node is produced by **name
  resolution**, never by the parser (§9.1): a parsed named type
  application whose name resolves to a class's associated type is
  rewritten to `AssocApp` with a concrete `(ClassId, member)` identity;
  an ambiguous reference errors (§8.3).

Parser work: rewrite the context-parsing prologue of
`parse_class_statement` / `parse_instance_statement` to accept the
parenthesized list (lookahead: `(` followed by an identifier and
eventually `) =>` distinguishes a context from nothing — classes and
instances cannot otherwise start with `(`); parse `type` items inside
class and instance bodies; extend bound parsing with optional type args;
parse the signature-level `where` clause in `parse_function` between the
return type / effect row and the body `{` (disjoint from the body-level
`where`-bindings position, §7.8).

### 9.3 Inference-side representation

- `InferType` gains
  `Assoc(AssocTypeId, Vec<InferType>)` where
  `AssocTypeId = (ClassId, Identifier)` (`src/types/infer_type.rs`).
- `SchemeConstraint` is replaced by a full predicate (§11.1).
- `WantedClassConstraint` (`src/ast/type_infer/constraint.rs:74-86`)
  keeps its origin machinery; `type_args` already holds `Vec<InferType>`
  and needs no change for structured predicates.

## 10. Kind system
[kind-system]: #kind-system

### 10.1 Kinds

`src/types/kind.rs` stays as-is structurally —
`Kind ::= Type | Arrow(Kind, Kind)` — and finally gains consumers. No kind
polymorphism, no `TypeInType` (matching its Proposal 0123 header). Fix
`TypeConstructor::kind()` (`src/types/type_constructor.rs:47`): `Adt(id)`
must return the constructor's real arity-derived kind
(`Type -> … -> Type`), looked up from the ADT declaration, not `Type`.

### 10.2 Kind inference for classes

For each class, a kind-inference pass runs over the class body before HM
inference:

1. Assign each class parameter (and each associated type) a fresh kind
   metavariable.
2. Walk every method signature (and associated-type use). Each
   application `f<t1, …, tn>` generates the equation
   `kind(f) = kind(t1) -> … -> kind(tn) -> Type`; each use of a parameter
   as a plain type generates `kind(p) = Type`.
3. Solve by first-order unification. Unconstrained metavariables default
   to `Type`.
4. Record the solved kinds on `ClassDef` (new field
   `param_kinds: Vec<Kind>`, plus `assoc_kinds`).

`class Functor<f>` with `fn fmap<a,b>(fa: f<a>, g: (a) -> b) -> f<b>`
infers `f : Type -> Type` with no annotation.

### 10.3 Kind checking

With class-parameter kinds known, check:

- **Instance heads**: each head type argument's kind must equal the
  corresponding class parameter kind. `instance Functor<List>` checks
  `List : Type -> Type` ✓; `instance Functor<Int>` fails (`Int : Type`).
  New diagnostic E456 (§20). This closes the 0150 gap.
- **Ordinary type applications**: `List<Int, Int>` and using a
  higher-kinded constructor as a plain type become kind errors wherever
  `TypeExpr` is converted to `InferType` (today over-applied constructors
  are caught ad hoc or not at all).
- **Superclass and context predicates**: each constraint's argument kinds
  must match the constrained class's parameter kinds.
- **Associated-type declarations and equations**: declaration parameters
  inherit the class parameter kinds; an equation's RHS must have the
  family's result kind (`Type` in the initial design).
- **Derived instances**: the synthesized head is kind-checked like a
  written one.
- **Multi-parameter class arguments**: positionally, same rule as
  single-parameter.

Kind checking runs at class/instance collection time (in
`ClassEnv::from_statements`, `src/types/class_env.rs:236` area), before
any solving, so the solver may assume kind-correct predicates.

## 11. Predicate and constraint representation
[predicate-representation]: #predicate-representation

### 11.1 The predicate type

One canonical predicate form used by the solver, schemes, elaboration, and
interfaces:

```rust
enum Pred {
    /// C<t1, …, tn> — class_id is the resolved (module, name) identity.
    Class { class_id: ClassId, args: Vec<InferType> },
    /// F<t1, …, tn> ~ t — associated-type equality (Phase 6).
    AssocEq { assoc: AssocTypeId, args: Vec<InferType>, rhs: InferType },
}
```

`SchemeConstraint { class_name, type_vars }`
(`src/ast/type_infer/constraint.rs:97-103`) is replaced by `Pred` on
`Scheme.constraints`. This is the representational fix for Defect 2:
`Eq<List<a>>`, `Convert<a, String>`, and `Element<c> ~ a` all become
expressible. `Scheme::generalize` (`src/types/scheme.rs:192`) is removed
in favor of `generalize_with_constraints`, whose filter changes from
"all bare quantified vars" to "every free variable of the predicate is
quantified"; predicates with free variables that are *not* quantified are
an ICE (they belong to an outer scope and must have been solved or
propagated there, never dropped).

### 11.2 Wanted classification

`solve_class_constraints` is rewritten around an explicit disposition:

```rust
enum Disposition {
    Solved(Evidence),          // instance or given found
    Generalized(Pred),         // goes onto the enclosing scheme
    Stuck(Pred),               // assoc-type application on abstract type
    Ambiguous(Pred),           // var not determined by the type — error
    Missing(Pred),             // no instance — E444
}
```

Every wanted gets exactly one disposition; the four `continue` paths of
`src/types/class_solver.rs:37-65` and the drops in
`collect_scheme_constraints` (`src/types/class_defaulting.rs:155,
166-168`) are deleted. `InferredOperator`-origin constraints on type
variables become `Generalized(Num<a>)` instead of vanishing. Compiler-
generated constraints (`span == Span::default()`) are checked like any
other; synthesized code must carry real spans or the def's span.

**Ambiguity check.** At generalization, a predicate variable that does not
appear in the generalized type (and is not determined via an
associated-type equality) is ambiguous — reported at the definition, not
at use sites. Defaulting (§11.3)
runs first, so `fn f() -> Int { 1 + 2 }` never sees an ambiguity error.

### 11.3 Defaulting

Kept deliberately simple: the existing `Num`-to-`Int` rule
(`build_numeric_default_subst`, `src/types/class_defaulting.rs:91`) is
retained, with two changes: the class is identified by `ClassId` (the
prelude's `Num`), not by interned string `"Num"` (line 102); and the
"blocked by any other constraint" rule (lines 127-129) is relaxed to
"blocked by any non-defaultable constraint on the same variable" so
`Num<a> + Show<a>` can still default once `Show<Int>` exists.
Extended or user-configurable defaulting is out of scope.

## 12. Instance resolution
[instance-resolution]: #instance-resolution

### 12.1 The resolution question

Given a wanted `Pred::Class { class_id, args }` and a set of givens (the
enclosing function's bound predicates plus their superclass closure), find
evidence. Resolution inputs are the **whole predicate** — all class
parameters, which are derived from all argument types *and* the expected
result type of the call via ordinary HM unification — plus contextual
givens and module-qualified class identity. Evidence keys are complete
predicates, never class names alone.

### 12.2 Algorithm

1. **Normalize** the predicate: apply the current substitution; reduce
   associated-type applications (§15).
2. **Givens**: if a given (or any predicate in a given's transitive
   superclass closure, §14) matches by structural equality up to the
   substitution, the evidence is that given's dictionary (projected
   through superclass selectors as needed).
3. **Instances**: look up `instances_for_id(class_id)`
   (`src/types/class_env.rs:1321` — the existing ClassId-keyed variant,
   which becomes the *only* variant; the short-name shims
   `lookup_class` / `resolve_instance_with_subst` / `method_to_class` at
   `class_env.rs:1236-1292, 1392` are deleted). Match the instance head
   against the predicate args by one-way structural matching
   (instance-head variables bind; wanted-side variables do not).
   - **Zero matches** with all args concrete → `Missing` (E444).
   - **Zero matches** with a variable in a relevant position →
     `Generalized` or `Stuck` per §11.2.
   - **One match** → instantiate; recursively resolve the instance's
     context predicates (this is where contextual instances chain);
     evidence = the instance dictionary applied to context evidence.
   - **More than one match** → coherence violation, compile error at the
     *instance declarations* (detected eagerly at collection time, §12.4;
     the resolution-time case is an ICE).
4. **Structural built-ins**: expressed as ordinary compiler-owned
   instances (§16.3), so this step disappears as a special case.

Matching is structural over `InferType`, kind-checked, and
ClassId-correct; the "first char is lowercase means variable" test
(`class_env.rs:2018`) and string comparisons of constructor names
(`type_constructor_matches`, `class_env.rs:2026`) are replaced by
comparisons on resolved constructor identities.

### 12.3 Method-call dispatch (replacing first-argument direction)

A call `method(a1, …, an)` (or `expected_ty = method(…)`) elaborates as:

1. Resolve the candidate class set for `method` per §7.7 (module-aware,
   ambiguity is an error — replacing first-match `method_to_class`).
2. Instantiate the method's scheme; unify parameter types with argument
   types **and** the method result type with the call's expected type
   (already available in `hm_expr_types` keyed by call id — the same
   mechanism the `Decode` hack uses for its one case,
   `src/core/lower_ast/mod.rs:453-478`).
3. The class parameters are now as-determined-as-they-can-be. Resolve the
   predicate per §12.2.

This is the general result-directed rule KI-015 asks for. `pure`/`return`/
`mempty`-style return-position methods dispatch when the expected type
determines the class parameter; when nothing determines it, the predicate
is ambiguous and reported as such (never a runtime "No instance" panic).
The `Decode` special case (four sites: `src/core/lower_ast/mod.rs:453-478,
552-562`; `src/compiler/expression.rs:4566, 4673`) is deleted once this
lands, along with `is_json_codec_class`'s dispatch role.

### 12.4 Coherence and overlap

At `ClassEnv` build time, for every pair of instances of the same
`ClassId`: if their heads unify (two-way), report an overlap error at the
second declaration. Associated-type equations inherit this check for free
(one equation per instance, non-overlapping heads ⇒ non-overlapping
equations). No overlap pragmas exist. Cross-module duplicates surface at
import merge (`merge_imported_public_instances`,
`src/compiler/mod.rs:642`) with the same diagnostic.

### 12.5 Termination

Instead of Paterson-style size analysis, the initial rule is structural:
every predicate in an instance context must mention only type variables
bound by the instance head, and must be strictly structurally smaller than
the head (each context arg is a subterm of a head arg, the common
`instance (Eq<a>) => Eq<List<a>>` shape). This is restrictive but
decidable by construction and covers the stdlib. Revisit in Phase 9 if a
real program needs more.

### 12.6 The panic stub becomes unreachable

`generate_polymorphic_stub` (`src/types/class_dispatch.rs:1788-1867`)
currently emits arity-`n` stubs whose body panics `"No instance of …"`.
After Phases 2–4 the stub is retained only as an ICE trap: reaching it
means the compiler failed to elaborate, and its message changes to an ICE
diagnostic asking for a bug report. No well-typed program may reach a
runtime "No instance" path.

## 13. Dictionary representation
[dictionary-representation]: #dictionary-representation

### 13.1 Layout

A dictionary for `instance C<T…>` is a tuple:

```
( sc_dict_1, …, sc_dict_k,   // superclass evidence, class-declaration order
  method_1, …, method_m )    // methods, class-declaration order
```

Superclass slots come **first**, so method indices are stable relative to
the end of the superclass prefix. Today's layout is methods-only
(`build_instance_dictionaries`, `src/core/passes/dict_elaborate.rs:213`,
`MakeTuple` of `__tc_*` refs at lines 262-266); Phase 5 adds the
superclass prefix, and every projection index in
`rewrite_expr` (`dict_elaborate.rs:1224-1243`) and
`try_build_dict_class_method_call`
(`src/compiler/expression.rs:4621-4650`) shifts by `k`.

Naming stays: globals `__dict_{Class}_{TypeKey}`, hidden parameters
`__dict_{Class}` — extended to `__dict_{Class}_{n}` (constraint index)
to fix the collision when one function has two constraints on the same
class (`rewrite_constrained_functions`,
`dict_elaborate.rs:437-441`). Contextual instances stay dictionary
*functions* (lambdas over context dictionaries,
`build_contextual_dictionary_expr`, `dict_elaborate.rs:293`), now also
receiving superclass evidence.

### 13.2 Calling convention

Dictionary arguments are prepended, one per scheme constraint, in
constraint order, before all visible arguments — unchanged
(`dict_elaborate.rs:650-651`, `prepend_lam_params:1026`). What changes is
*completeness*:

- `resolve_dict_arg` (`dict_elaborate.rs:1001-1022`) gains its missing
  concrete branch — but as a **shared resolver**: one function, fed by
  `hm_expr_types` and the `ClassEnv`, used by AST→Core lowering, the Core
  pass, and (if retained, §13.3) the AST bytecode path. Partial
  resolution becomes an ICE: if a callee has `n` constraints, exactly `n`
  dictionary arguments are produced or compilation fails — the
  "prepend only if non-empty" silent path (`dict_elaborate.rs:648`) and
  the all-or-nothing empty-vec return
  (`src/core/lower_ast/mod.rs:663-664`) are both errors now.
- The compile-time arity check (`src/compiler/expression.rs:1600-1603`)
  stops accepting multiple arities: after elaboration there is exactly one
  correct arity per call.
- The `__tc_` argument-dropping band-aid in the VM
  (`src/vm/function_call.rs:221-239`) is deleted; E1000 at any
  statically-elaborated call site is an ICE symptom, and the new
  finalization invariant (§4.1) catches it before the VM does.

### 13.3 One authoritative pipeline

Decision: **the VM executes Core-derived code for every function that
declares or uses constraints.** Concretely, `use_ast_path`
(`src/compiler/statement.rs:1969-1976`) may not select the AST fallback
when `requires_ir_only` holds; the current escape hatches become hard
errors surfaced at compile time (CFG-incompatible statements inside a
constrained function is a compiler bug to fix, not a fallback trigger;
today's known triggers are module/import statements, which cannot appear
in function bodies anyway). The AST path's partial dictionary machinery
(`try_build_constrained_user_fn_call_ast` from 0168,
`try_build_dict_class_method_call`) is kept only until Phase 2 completes,
then removed with the fallback eligibility. `emit_dict_globals`
(`src/compiler/passes/codegen.rs:83-153`) is extended to handle
contextual (function-valued) dictionaries instead of silently dropping
them (lines 102-119).

The native path keeps consuming elaborated Core
(`src/compiler/mod.rs:1487` `lower_core_from_program` with
`elaborate_dictionaries = true`) and is specified to have identical
observable behavior, verified by parity fixtures (§22) — the two-backend
contract from `parity-check` applies to every typeclass fixture.

## 14. Superclass entailment
[superclass-entailment]: #superclass-entailment

### 14.1 Representation

`ClassDef` gains `superclasses: Vec<Pred>` (structural, ClassId-resolved,
kind-checked — replacing the short-name `Vec<ClassConstraint>` semantics).
`.flxi`'s `PublicClassEntry.superclasses` is upgraded accordingly (§17.3).

### 14.2 Instance obligation

When collecting `instance C<T…>`, for each superclass predicate `S<σ(u…)>`
(σ = the head substitution): the obligation is discharged through the
solver (§12.2) — *not* by rendering strings
(`class_env.rs:1027-1037` is deleted) — and is checked **after the entire
ClassEnv is built**, making it declaration-order-independent. Failure is
E445 with the full predicate chain in the diagnostic.

### 14.3 Evidence

The discharged superclass evidence is stored in the instance's dictionary
prefix (§13.1). Compiler-generated **superclass selectors** are just
`TupleField` projections at known indices; no named selector functions are
needed at the Core level, because Flux's Core has first-class tuple
projection, so index-based selection suffices.

### 14.4 Entailment in solving

Givens are closed over superclasses: a bound `a: Ord` contributes givens
`Ord<a>` *and* `Eq<a>` (transitively), each with an evidence path
(dictionary, then a chain of superclass projections). Closure is computed
per class at ClassEnv build time with cycle detection
(`class C1 => C2; class C2 => C1` is an error — Flux rejects superclass
cycles outright rather than bounding recursive expansion with a fuel
mechanism). Both immediate and transitive entailment therefore fall out
of the closure.

### 14.5 Interaction with associated types

Superclass predicates may mention the class's own parameters only (initial
design); associated-type applications in superclass positions
(`class (Eq<Element<c>>) => Collection<c>`) are Phase 9 — they require
stuck-predicate entailment and are explicitly deferred, with a parse-time
"not yet supported" diagnostic rather than silent misbehavior.

## 15. Associated-type reduction
[associated-type-reduction]: #associated-type-reduction

### 15.1 Reduction rule

The equation environment maps `(AssocTypeId, instance head)` → RHS,
collected per instance (Phase 6). Reduction of `F<t…>`:

1. Normalize the arguments (apply substitution, reduce inner
   applications).
2. Find the unique instance of the owning class whose head matches `t…`
   (one-way match, same matcher as §12.2). Coherence (§12.4) guarantees
   at most one.
3. If found: apply the head substitution to the RHS and continue
   normalizing. `Element<List<Int>>` → match `instance
   Collection<List<a>>` with `a := Int` → `Int`.
4. If the match is blocked on a variable (`Element<c>`, `c` abstract):
   the application is **stuck** — returned as-is.

Reduction is terminating: RHSs may mention only head-bound variables, and
the §12.5 structural-descent rule applies to equations as well (an RHS
may not mention the family being defined applied to non-smaller
arguments; the initial design simply forbids the family's own name in any
RHS).

### 15.2 Stuck applications and unification

`InferType::Assoc(F, args)` unifies:

- with `Assoc(F, args')` (same family): by unifying `args` with `args'`
  pointwise. Note this is sound but incomplete (injectivity is *not*
  assumed; two different arg lists may reduce to equal types — but since
  unification only ever *adds* equalities this incompleteness produces
  type errors, never unsoundness; document as a limitation).
- with any other type `t`: if reduction makes progress, retry; otherwise
  record the equality `F<args> ~ t` as a wanted `Pred::AssocEq`. At
  generalization, a residual `AssocEq` whose variables are all quantified
  is carried on the scheme; instantiation re-installs it as a wanted,
  which reduces once the instantiating types are concrete.

This is a deliberately small fragment of full type-family rewriting.
Flux needs no coercion evidence because it erases types: reduction
equalities have no runtime content, so "reduce and substitute" suffices.

### 15.3 Where reduction runs

- During unification (both directions, before failing a constructor
  clash).
- Before instance matching in the solver (§12.2 step 1).
- At scheme instantiation and at `.flxi` serialization (interfaces store
  fully-reduced types where possible; stuck applications serialize
  structurally, §17.3).
- In diagnostics rendering: messages show the reduced form, with the
  unreduced form as a secondary note when they differ.

## 16. Deriving and structural instances
[deriving-and-structural-instances]: #deriving-and-structural-instances

### 16.1 Supported deriving set

`deriving (…)` on a `data` declaration supports exactly: `Eq`, `Ord`,
`Show`, `Encode`, `Decode` — the classes with real synthesis today
(`builtin_method_body`, `src/types/class_dispatch.rs:1453`;
`derived_json_method_body:306`). Any other class name in a deriving list
is a **compile-time error** (new E-code, §20) at the deriving clause —
the current silent method-less-instance path
(`class_env.rs:1188-1198` + `class_dispatch.rs:185-201`) is removed:
`collect_deriving` consults the supported set (extensible later by a
registry, not by string match) and rejects up front. `Sendable` remains
sealed (existing behavior).

### 16.2 Structural derivation

Derived `Eq`/`Ord`/`Show` are structural over constructors and fields,
with auto-generated contexts constraining each type parameter (existing
mechanism, `class_env.rs:1180-1187` — which already produces
multi-element contexts; after §4.5 the parser can produce them too, so
derived and written instances share one code path). Derived instances are
kind-checked and coherence-checked like written ones. Derived
associated-type equations: none of the derivable classes declare
associated types, so this is vacuous until Phase 9; the design rule is
that a deriving-capable class with associated types must specify its
equation synthesis alongside its method synthesis, or deriving it is an
error.

### 16.3 Structural instances become real instances

The solver-side `has_structural_builtin_instance` hack
(`src/types/class_solver.rs:164-198`) is replaced by compiler-owned
instances registered in the ClassEnv at bootstrap: `Eq`/`Ord` for tuples
up to a fixed arity (contexts `(Eq<a>, Eq<b>, …)`), `Eq`/`Ord` for
`Option<a>`/`List<a>`/`Array<a>`/`Either<a, b>` with element contexts,
`Sendable` for the same plus `Map<k, v>` and function types where sound.
Each is an ordinary `InstanceDef` with synthesized method bodies (the
`generate_builtin_instance_functions` machinery already exists), so each
**produces a real dictionary** and participates in contexts, superclass
discharge, and polymorphic forwarding — closing Defect 7. They are keyed
to the *prelude's* ClassIds, so a user-defined `Eq` in another module no
longer inherits them.

### 16.4 Future deriving strategies

`newtype`-style and `via`-style deriving, standalone deriving, and a
`MINIMAL`-like completeness declaration are Phase 9 evaluations (§20).

## 17. Module and interface behavior
[module-and-interface-behavior]: #module-and-interface-behavior

### 17.1 Identity and visibility

- Class identity: `ClassId = (ModulePath, ClassName)` everywhere; all
  short-name shims deleted (§12.2). Two modules defining `Eq` coexist;
  references resolve per §7.7.
- Associated-type identity: `(ClassId, member)` (§8.4).
- Visibility: unchanged E450/E451/E455 semantics; an associated type is
  visible exactly where its class is.

### 17.2 Orphan rule

Kept relaxed but tightened in statement: `instance C<T1, …, Tn>` in
module `M` is legal iff `M` defines `C`, or `M` defines the head
constructor of **some** `Ti` (today: only `T1`,
`head_type_owning_module`, `src/types/class_env.rs:671-679` — the
extension to any position is required for multi-parameter classes like
`Convert<Int, MyType>`). The two grandfather exemptions (empty
`instance_module` = prelude, `class_env.rs:622`; derived placeholders,
line 625) are retained for the prelude and removed for deriving once
§16.1 makes derived instances real. Associated-type equations need no
separate orphan rule (§8.4).

### 17.3 `.flxi` serialization

`src/types/module_interface.rs` changes:

- `PublicClassEntry`: `superclasses` becomes serialized `Pred`s
  (ClassId-resolved — completing the deferred "full ClassId resolution
  for superclasses" note at lines 29-31); add `param_kinds: Vec<Kind>`
  and `assoc_types: Vec<PublicAssocTypeEntry { name, param_kinds,
  result_kind }>`.
- `PublicInstanceEntry`: `class_module`/`class_name` collapse into a
  serialized `ClassId`; add `assoc_type_eqns: Vec<PublicAssocEqnEntry {
  name, rhs: TypeExprRepr }>` (the LHS is the entry's own instance head);
  `context` becomes `Pred`s.
- `Scheme` serialization carries the new `Pred` constraints (structured
  predicates and `AssocEq` residuals) — `collect_symbols`/`remap_symbols`
  (`src/types/scheme.rs:56-64`) extend to the new payloads for the
  portable symbol table.

**Population parity warning**: KI-014's post-mortem showed an interface
field populated only on the cached path. Every new field must be
populated on both the cold (fresh compile) and warm (`.flxi` preload)
paths, with a test asserting cold/warm equivalence (§22, test 19).

### 17.4 Cache invalidation

`CACHE_EPOCH` (`src/shared/cache_paths.rs:58`, currently `27`) **must be
bumped** in every phase that changes serialized metadata: Phase 1
(kinds + `Pred` on schemes), Phase 5 (superclass entailment metadata if
layout-affecting), Phase 6 (associated types). Dictionary *layout*
changes (Phase 5's superclass prefix) alter compiled bytecode and
therefore also require a bump even though `.flxi` shape may not change —
the epoch invalidates all four cache kinds at once, which is exactly
what's needed. Interface fingerprints must incorporate class/instance/
associated-type metadata so a downstream module recompiles when an
upstream instance or equation changes.

## 18. Standard library hierarchy
[standard-library-hierarchy]: #standard-library-hierarchy

Target Flux classes in `lib/Flow` (arrow direction: **the left-hand class
is a superclass/prerequisite of the right-hand class** — `Eq → Ord` means
`class Eq<a> => Ord<a>`):

```
Eq → Ord
Semigroup → Monoid
Functor → Applicative → Monad
```

```flux
class Eq<a> {
    fn eq(x: a, y: a) -> Bool
    fn neq(x: a, y: a) -> Bool { not(eq(x, y)) }
}

class Eq<a> => Ord<a> {
    fn compare(x: a, y: a) -> Int
    // lt/lte/gt/gte/max/min as default methods over compare
}

class Semigroup<a> {
    fn append(x: a, y: a) -> a
}

class Semigroup<a> => Monoid<a> {
    fn mempty() -> a
}

class Functor<f> {
    fn fmap<a, b>(fa: f<a>, g: (a) -> b) -> f<b>
}

class Functor<f> => Applicative<f> {
    fn pure<a>(x: a) -> f<a>
    fn ap<a, b>(ff: f<(a) -> b>, fa: f<a>) -> f<b>
}

class Applicative<f> => Monad<f> {
    fn bind<a, b>(ma: f<a>, k: (a) -> f<b>) -> f<b>
}
```

Sequencing constraints:

- `Eq`/`Ord` land in Phase 8 step 1 (they exist informally today; the
  step is making the stdlib declarations canonical and the structural
  instances real, §16.3).
- `Semigroup`/`Monoid` require superclass evidence (Phase 5) and
  result-directed dispatch for `mempty` (Phase 4).
- **`Monad` (and `pure`) must land only after Phase 4**: `pure` and
  `mempty` are return-position-directed methods; without §12.3 they
  cannot dispatch at all (KI-015).
- Instances ship for `Option`, `List`, `Array`, `Either<e>` (the
  partially-applied instance `instance Functor<Either<e>>` exercises HKT
  head matching), and `String`/`List` for `Semigroup`/`Monoid`.
- Method effect rows: `fmap`'s callback parameter follows the existing
  row-polymorphic class-method rules (`docs/internals/modules.md:331`);
  nothing new is required, but the Phase 8 tests must include an
  effectful `fmap` to keep the interaction covered.
- Explicitly omitted: a separate `return` method on `Monad` (redundant
  with `pure`), a separate `mappend` on `Monoid` (redundant with
  `append`), and `fail`. Naming stays Flux-flavored (`append`, `bind`,
  `ap`) with operators as a separate later decision.

## 19. Compatibility and migration
[compatibility-and-migration]: #compatibility-and-migration

- **Source compatibility**: all currently-working programs keep working.
  New syntax (parenthesized contexts, `type` members, bound type args) is
  purely additive. Two behavior changes are breaking on purpose:
  unsupported `deriving` becomes an error (previously silently useless),
  and kind-invalid instances become errors (previously latent wrong
  matches). Both are "programs that could never have worked correctly".
- **Silent-semantics changes**: programs that today *run wrong or crash*
  (E1000 cases, structural-Eq-satisfied-but-no-dictionary cases) start
  working; programs relying on first-match instance selection across
  same-named classes get ambiguity errors with a qualification hint.
- **Cache**: `CACHE_EPOCH` bumps per §17.4; no user action beyond
  automatic recompiles.
- **Examples**: `examples/type_inference/numeric_defaulting_explicit_bound.flx`
  starts printing `4`; its stale comment is already accurate after
  Phase 2. The parity corpus under `examples/type_system` gains typeclass
  fixtures and must stay green under
  `cargo run -- parity-check examples/type_system --ways vm,llvm`.
- **LSP**: `crates/flux-lsp` reuses the frontend; new AST nodes and
  diagnostics surface automatically, but hover/completion for associated
  types should be checked in Phase 6.

## 20. Diagnostics
[diagnostics]: #diagnostics

New and changed error codes (declared in
`src/diagnostics/compiler_errors.rs`, registered in `registry.rs`,
constructed via `Diagnostic::make_error(&CODE, …)` with the
`DiagnosticBuilder` trait in scope):

| Code | Meaning |
|---|---|
| E444 (existing) | No instance — now reported with the full normalized predicate, the tried instance heads, and the reduction trail if associated types were involved. |
| E445 (existing) | Superclass obligation unsatisfied — now structural, order-independent, with the predicate chain. |
| E449 (existing) | Orphan instance — extended to any-argument-position ownership (§17.2). |
| E456 (new) | Kind mismatch in instance head / type application / predicate. Shows expected vs actual kind and the inference origin ("`f` is applied to one argument in `fmap`, so `f : Type -> Type`"). |
| E457 (new) | Ambiguous class predicate at generalization (with the undetermined variable and the signature span). |
| E458 (new) | Ambiguous class or method reference across modules (with qualification hint). |
| E459 (new) | Overlapping instances (both declaration spans). |
| E460 (new) | Unsupported deriving class (names the supported set). |
| E461 (new) | Associated-type equation errors: missing, duplicate, RHS mentions a variable not bound by the instance head, self-recursive RHS. |
| E462 (new) | Unresolvable/conflicting associated-type equality (`Element<List<Int>> ~ String` where reduction gives `Int`). |
| ICE | Dictionary arity invariant violation at bytecode finalization; partial dictionary resolution; panic-stub reachability. |

All spans must be real: the `span == Span::default()` skip (§4.2) forced
this proposal to require synthesized code to carry the originating def's
span.

A new KI entry must be filed immediately (before any implementation): the
polymorphic-dictionary runtime failure has **no** `docs/known_issues.md`
entry today (no hits for "E1000" or "dictionary"). File it as the next
free number (KI-050 at time of writing), Severity High ("silent wrong
answers or workaround required" — a five-line valid program crashes),
Area "Type classes / dispatch", with the §4.1 reproduction, and
cross-reference KI-015 and this proposal. KI-015 itself gets
"Tracked by: 0179 (Phase 4)". Proposal 0135's claim that wrong arity is
"already caught by HM" (`docs/proposals/0135_total_functions_and_safe_arithmetic.md:43`)
is contradicted by Defect 1 and should be annotated when that file is
next touched.

## 21. Implementation phases
[implementation-phases]: #implementation-phases

Dependency-aware roadmap. Each phase is independently landable, gated by
the tests in §22, and ends with clippy + full test suite per CI. Phases 2
and 3 are the soundness core; nothing new (Phases 4–8) lands before
Phase 2's execution tests exist and pass.

### Phase 0 — Surface language and semantic specification

Parser + AST only; no semantics change.

- Parenthesized multi-constraint contexts for classes and instances
  (§7.2, §7.4); full `TypeExpr` args on superclass constraints.
- Bound type arguments (`a: Convert<String>`, §7.3) via `BoundPred`.
- Signature-level `where` clauses (§7.8) parsed into
  `Statement::Function.where_preds`; rejected downstream with a "not yet
  supported" diagnostic until Phase 3 gives them semantics.
- Associated-type declarations and equations parsed into the new AST
  nodes (§9.2), with "not yet implemented" diagnostics downstream until
  Phase 6.
- Qualified class names in every head/bound/context position.
- Deriving syntax unchanged. Associated-type references parse as
  ordinary named type applications; the `AssocApp` reclassification
  happens in name resolution (§9.1) and lands with Phase 6.
- Diagnostics: precise spans on every new node; parser tests under
  `tests/parser/`.
- Docs: grammar additions to `docs/proposals/0027`-lineage syntax docs.

### Phase 1 — Type and kind foundation

- Kind inference for class parameters and ADT constructors; kind checking
  of instance heads, applications, predicates (§10). E456.
- Fix `TypeConstructor::kind()` for `Adt` arity.
- `Pred` replaces `SchemeConstraint`; `InferType::Assoc` added (inert
  until Phase 6); structural type equality and substitution extended.
- `.flxi`: kinds + `Pred` schemes serialized; cold/warm parity test;
  **bump `CACHE_EPOCH`**.

### Phase 2 — Runtime dictionary correctness

The E1000 fix. Scope is exactly Defect 1:

- Implement `resolve_dict_arg`'s concrete branch as the shared resolver
  (§13.2); make partial resolution an ICE.
- Remove AST-fallback eligibility for constrained functions (§13.3);
  extend `emit_dict_globals` to contextual dictionaries.
- Delete the VM `__tc_` band-aid; tighten the compile-time arity check to
  the single post-elaboration arity.
- Add the bytecode-finalization arity invariant (emitted calls vs emitted
  callees agree for statically-known callees).
- Execution-based regression tests (§22 tests 1–8, 21) on VM **and**
  native, covering concrete insertion, polymorphic forwarding, contextual
  dictionaries, HKT dictionaries, multi-parameter dictionaries, and
  higher-order calls — **before** any new standard classes.

### Phase 3 — Sound constraint solving

- Disposition-based solver (§11.2); delete all silent-drop paths.
- Generalization preserves arbitrary predicates; ambiguity check (E457);
  missing-instance diagnostics with full predicates.
- `where`-clause predicates (§7.8) become semantically active: unioned
  with desugared bounds into scheme constraints, validated (unknown
  class, out-of-scope variables), and discharged like any predicate.
- Equality-constraint plumbing (`Pred::AssocEq` carried, though nothing
  produces it until Phase 6); stuck-predicate handling defined.
- No runtime "No instance" path: panic stub becomes ICE trap (§12.6).

### Phase 4 — Principled evidence resolution

- Complete-predicate, ClassId-keyed matching (§12.2); delete short-name
  shims; module-aware method candidate sets (E458).
- Result-directed resolution (§12.3); **remove the `Decode` special
  casing** (all four sites); close KI-015.
- Eager pairwise overlap rejection (E459).
- Evidence keys = complete predicates.

### Phase 5 — Superclass entailment

- Structural, order-independent superclass discharge after ClassEnv
  build (§14.2); superclass cycle rejection.
- Dictionary layout gains superclass slots; projections re-indexed;
  transitive given-closure in the solver (§14.4).
- **Bump `CACHE_EPOCH`** (bytecode layout change).

### Phase 6 — Associated types

- Equation collection + validation (duplicates, unbound RHS variables,
  self-recursion — E461); kind checking of declarations/equations.
- Reduction engine (§15.1); unification integration + stuck handling
  (§15.2); generalization of stuck applications; `AssocEq` residuals
  (E462).
- Method-signature and ordinary-signature integration; boxed
  representation rule for unresolved applications.
- `.flxi` serialization of declarations and equations; fingerprint
  coverage; **bump `CACHE_EPOCH`**; diagnostics; coherence enforcement.

### Phase 7 — Deriving and structural instances

- E460 for unsupported deriving; derived instances become real (methods
  + dictionary) at collection time.
- Structural `Eq`/`Ord` container instances as compiler-owned real
  instances (§16.3); delete `has_structural_builtin_instance`.
- Associated-type-aware deriving rule (§16.2, vacuous but specified).

### Phase 8 — Standard library hierarchy

- `Eq → Ord`, `Semigroup → Monoid`, `Functor → Applicative → Monad` in
  `lib/Flow` (§18), with instances for `Option`/`List`/`Array`/
  `Either<e>`/`String`.
- `Monad` lands only after Phase 4 (return-position dispatch).
- Parity fixtures for every class; effectful-`fmap` coverage.

### Phase 9 — Optional advanced features (evaluate, don't promise)

Associated types with extra parameters; associated-type defaults
(declared in the class body, renamed onto the class parameters when
stored); user-visible equality constraints;
functional dependencies; quantified constraints; standalone deriving and
strategies; `MINIMAL`-style declarations; richer defaulting; superclass
predicates over associated types (§14.5); unary-class dictionary
unboxing.

## 22. Runtime and compile-time tests
[tests]: #tests

**The central testing gap**: every current dictionary test asserts on the
Core dump or on compilation success
(`tests/type_inference/constrained_type_params_integration.rs:40-55` —
`dump_core_with_opts` + substring asserts; its own
`fn same<A: Eq>` test is precisely the case that crashes at runtime).
Defect 1 lives in the untested step. All new semantic tests below are
**execution** tests through `tests/support/flux_runner.rs` (`run_flux`
spawns the real binary with `--no-cache`, i.e. the default VM backend),
plus parity fixtures for native. Per `tests/README.md`, they live under
`tests/type_inference/` (type inference & classes), `tests/vm_runtime/`,
`tests/native_llvm/`, and `tests/parity/`, each with an explicit
`[[test]]` target in `Cargo.toml` (names must not drift from targets —
look them up, don't guess).

Required tests (VM execution unless stated; ✱ = also a native/parity
fixture where the feature is native-supported):

1. Concrete call to a constrained function (`twice(21)` → `42`). ✱
2. Generic function using a class method (`fn same<A: Eq>` executed, not
   dumped).
3. Generic function forwarding its dictionary to another constrained
   function. ✱
4. Two instances of the same class selected correctly at two call sites.
5. Contextual instance executing (`Eq<List<Int>>` via
   `instance (Eq<a>) => Eq<List<a>>`). ✱
6. HKT: `Functor<List>` `fmap` through a constrained generic
   (`fn double_all<f: Functor>`). ✱
7. Multi-parameter class (`Convert<Int, String>`) resolved and executed.
8. Multiple bounds (`a: Eq + Show`) — two dictionaries threaded.
9. Return-directed method (`Parse<Int>` from KI-015's reproduction;
   later `pure`/`mempty`).
10. Superclass entailment: `fn f<a: Ord>` body calling `eq` (superclass
    projection); transitive case with a three-level chain.
11. Structured constraint `Eq<List<a>>` preserved on a scheme and
    discharged at a concrete use — written both via inference and via an
    explicit `where Eq<List<a>>` clause (§7.8).
12. Associated-type reduction: `Element<List<Int>>` → `Int` (a program
    whose result type proves the reduction).
13. Associated types in method result types (`fn first<c: Collection>`).
14. Associated types in ordinary function signatures.
15. Stuck associated type generalized and later resolved at a concrete
    call.
16. Conflicting associated-type equations → E461 (compile-fail).
17. Missing associated-type equation → E461 (compile-fail).
18. Kind-mismatched associated type / `instance Functor<Int>` → E456
    (compile-fail).
19. Cross-module: class, instance, and associated type defined in one
    module, used from another — **both cold and warm `.flxi` paths**
    (the KI-014 lesson).
20. Unsupported deriving → E460 (compile-fail).
21. VM/native parity: every ✱ fixture under
    `cargo run -- parity-check` discipline (`tests/parity/`).
22. Arity invariant: a unit test driving the finalization check, plus a
    regression asserting the §4.1 program no longer emits mismatched
    arities.

Fixture policy: **do not execute every example snapshot
indiscriminately** — many examples contain effects, external
dependencies, or intentional non-execution behavior. Add an opt-in
runtime fixture suite (a dedicated directory of typeclass examples with
expected-output headers, driven by
`tests/support/examples_snapshot.rs` helpers) rather than flipping the
existing snapshot corpus to execution.

## 23. Risks
[risks]: #risks

- **Removing the AST fallback for constrained functions** (Phase 2) may
  expose functions the CFG path cannot yet compile. Mitigation: the
  fallback triggers are enumerable (`ir_function.is_none()`,
  CFG-incompatible statements, CFG rollback at
  `src/compiler/statement.rs:1994-1999`); Phase 2 starts by logging every
  trigger across the example corpus and fixing the CFG gaps first. The
  risk is bounded because the native path already lives Core-only.
- **Deleting the short-name shims** touches every dispatch site; the
  0145/0151 history (memory: qualified `__tc_*` lookup at every dispatch
  site) shows these sites are numerous. Mitigation: type-driven — change
  the `ClassEnv` method signatures and fix every compile error, the same
  E0004-style sweep used for primops.
- **Dictionary layout change** (Phase 5) silently corrupts stale caches
  if the epoch bump is forgotten. The phase checklist makes the bump a
  review item, and test 19 (cold/warm) catches divergence.
- **Kind inference false positives** on existing code (e.g. an ADT used
  at two kinds by accident that currently "works"). Mitigation: run the
  checker across `examples/` and `lib/Flow` before enabling the error;
  anything that breaks is a latent bug to triage, with the option of a
  one-release warning period.
- **Result-directed dispatch changes inference observability**: call
  sites that today resolve (wrongly) via the first argument may become
  ambiguous. The E458/E457 diagnostics must be good enough to make the
  fix obvious; the parity corpus guards against behavior drift.
- **Associated-type unification incompleteness** (§15.2, no injectivity)
  can reject programs a user expects to check. Documented limitation;
  errors, never unsoundness.
- **Scope**: nine phases is a large program. The phase gating (each
  independently landable, soundness first) is the mitigation; Phases 0–3
  alone already remove every silent failure even if later phases slip.

## 24. Open design questions
[open-design-questions]: #open-design-questions

1. **Extending `where` to class and instance declarations** as an
   alternative spelling of the `=>` context (`class Ord<a> where Eq<a>,
   Show<a> { … }`)? §7.8 gives functions a `where` clause; declarations
   keep the `=>` context for now, leaving the language with two context
   spellings. Decide whether to converge, and in which direction, once
   `where` has usage experience.
2. **Explicit kind annotations** on class parameters (`class C<f: Type ->
   Type>`) if inference proves insufficient — syntax reserved, not
   designed.
3. **Operator surface** for the hierarchy (`<>`, `>>=`-analogues) versus
   named methods only — deferred to a stdlib-ergonomics proposal.
4. **Dictionary-passing performance**: monomorphization vs dictionary
   calls for hot paths; the VM/JIT could specialize `__tc_*`-direct calls.
   Measure after Phase 8 with the 0130 benchmark framework.
5. **Superclass predicates over associated types** (§14.5) — Phase 9, but
   the answer shapes whether `Collection` can require `Eq<Element<c>>`.
6. **How much of the orphan grandfather clause survives** once the
   prelude's instances are compiler-owned real instances (§16.3) — can
   the empty-`instance_module` exemption (`class_env.rs:622`) be retired?
7. **Whether bound sugar should stay minimal.** `where` (§7.8) can
   express every predicate, including the non-first-parameter cases
   bounds cannot (`where Convert<a, b>`); should bounds stay limited to
   the `C<a, …>` shape (recommended), or grow toward parity with
   `where`?

Per repo convention, questions that outlive this proposal migrate to
`docs/known_issues.md` with stable KI anchors when the proposal moves to
`implemented/` — they must not die in this section.

## 25. Acceptance criteria
[acceptance-criteria]: #acceptance-criteria

| # | Criterion |
|---|---|
| 1 | The §4.1 reproduction and `examples/type_inference/numeric_defaulting_explicit_bound.flx` print `42` / `4` on VM and native. |
| 2 | No well-typed program can produce E1000 from dictionary arity: the finalization invariant is on, the VM `__tc_` band-aid is deleted, and tests 1–8/22 pass. |
| 3 | `grep` finds no silent-drop `continue` in `solve_class_constraints` / `collect_scheme_constraints`; every wanted has a disposition; test 11 passes. |
| 4 | Superclass checking is string-free and order-independent; `fn f<a: Ord>` can call `eq`; test 10 passes. |
| 5 | KI-015's reproduction compiles and runs; the `Decode` string special-case is absent from the tree. |
| 6 | `class (Eq<a>, Show<a>) => Ord<a>`, `instance (Eq<a>, Show<a>) => Eq<List<a>>`, and a function with a `where` clause all parse, check, and execute. |
| 7 | `instance Functor<Int>` fails with E456. |
| 8 | Unsupported deriving fails with E460. |
| 9 | Structural container instances produce real dictionaries usable polymorphically. |
| 10 | Tests 12–17 (associated types) pass; `Element<List<Int>>` reduces to `Int`; dictionaries contain no type-level members. |
| 11 | The Phase 8 hierarchy compiles, executes, and passes parity; `pure` dispatches from the return position. |
| 12 | Cold and warm `.flxi` paths agree on all class/instance/associated-type metadata (test 19); `CACHE_EPOCH` was bumped in each metadata-changing phase. |
| 13 | `cargo fmt --check`, `clippy -D warnings`, and the full suite stay green at every phase boundary. |

## 26. Documentation and cache impact
[documentation-and-cache-impact]: #documentation-and-cache-impact

- **`docs/known_issues.md`**: file the new polymorphic-dictionary runtime
  failure entry (next free number; KI-050 at time of writing, §20) *now*,
  independent of implementation; mark KI-015 "Tracked by: 0179"; on
  Phase 2 and Phase 4 completion move both to the resolved section with
  FIXED dates.
- **Changelog fragments** under `changes/` (CI-enforced,
  `scripts/changelog/check_changelog_fragment.sh`): one per landing PR —
  this proposal's filing PR carries a `### Docs` fragment; each phase PR
  carries its own `### Added`/`### Fixed` fragment. Fragments are per-PR,
  not per-commit.
- **New internals doc**: `docs/internals/type_classes.md` — currently no
  dedicated typeclass/dictionary internals doc exists (coverage is
  scattered across `modules.md`, `language_design.md`,
  `hm_inference_compiler.md`). It should document: the predicate
  representation, the resolution algorithm, dictionary layout and calling
  convention, superclass evidence, associated-type reduction, and the
  single-pipeline invariant. Written incrementally per phase.
- **Syntax/reference docs**: grammar additions (contexts, bounds with
  args, `type` members) into the syntax specification lineage (0027) and
  `docs/internals/language_design.md`.
- **Module-interface documentation**: `docs/internals/modules.md` gains
  the upgraded `.flxi` class/instance/associated-type entries (§17.3);
  the 0139 format-version precedent is followed.
- **Error-code registry**: `docs/internals/error_codes.md` rows for
  E456–E462; registration in `src/diagnostics/registry.rs`.
- **Cache**: `CACHE_EPOCH` (`src/shared/cache_paths.rs:58`, currently 27)
  is bumped in Phases 1, 5, and 6 (§17.4). Class, instance, superclass,
  and associated-type metadata **are** serialized into `.flxi`
  (`src/types/module_interface.rs`), so every shape change there requires
  the bump; the dictionary-layout change in Phase 5 requires it even
  without an `.flxi` shape change because compiled bytecode changes.
- **Proposal index**: add the 0179 row to the Backlog table in
  `docs/proposals/0000_index.md`; on completion, move this file to
  `implemented/` and migrate surviving §24 questions to
  `docs/known_issues.md`.


