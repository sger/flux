- Feature Name: Polymorphic Effect Operations
- Start Date: 2026-04-23
- Status: Draft
- Proposal PR:
- Flux Issue:
- Depends on: [0145](0145_type_classes.md) (Type Classes — introduced `Scheme` / generalization), [0161](0161_effect_system_decomposition_and_capabilities.md) (Effect System Decomposition)
- Related: [0165](0165_io_primop_migration_to_effect_handlers.md) (IO Primop Migration — blocked on this)

# Proposal 0170: Polymorphic Effect Operations

## Summary
[summary]: #summary

Generalize effect-operation signatures over their free type variables
at collection time, and instantiate them fresh at each `perform` site.
Effect operations stop being monomorphic. A signature like
`effect Console { println : a -> () }` becomes "forall a. a -> ()",
usable against any caller type, the same way ordinary function schemes
work today.

Scope is the effect-op signature layer: `effect_op_signatures`
storage changes from `TypeExpr` to `Scheme`, and the `perform` type
checker instantiates before unification. No new surface syntax, no
handler-runtime changes, no effect-row changes.

## Motivation
[motivation]: #motivation

### The current design is silently monomorphic

Today, effect operations are stored as bare `TypeExpr`:

```rust
pub struct Compiler {
    pub effect_op_signatures: HashMap<(Symbol, Symbol), TypeExpr>,
    …
}
```

When `perform Console.println(x)` is type-checked, the compiler looks
up the signature and unifies against the arg. Any type variable in the
signature (`a` in `println: a -> ()`) is treated as a **rigid
skolem**: the same named variable across every call site, not a fresh
per-call instantiation.

The consequences are invisible until a caller tries to use a generic
operation at two concrete types, or calls a polymorphic op from
inside another polymorphic function:

```flux
effect Log { emit : a -> () }

fn greet<a>(x: a) with Log {
    perform Log.emit(x)   // ← rigid `a` from Log.emit meets
                          //   rigid `a` from greet — different vars,
                          //   same name: "cannot unify `a` with `a`".
}
```

### Why this blocked 0165

Proposal 0165 (IO primop → effect handler migration) was prototyped
end-to-end in 2026-04. Its core move is routing
`println(x)` through `perform Console.println(x)`. Flux's surface
`println` is polymorphic; an effect-op signature `println: String -> ()`
rejects `println(42)` with a type mismatch, and `println: a -> ()`
rejects `assert_eq<a>(a, b)` calling `println(a)` with the "rigid
vs rigid" error above. The spike section of 0165 documents both
paths in detail.

Neither workaround (pre-stringifying via `to_string`, narrowing to
monomorphic strings) is viable without cascading observable changes
or dropping the proposal's "no user-visible change" guarantee. 0165
becomes mechanical the moment effect ops generalize.

### The broader story

Polymorphic effect operations are the default in Koka, Effekt, and
Unison. Flux already has the generalization machinery (`Scheme`,
`generalize`, `instantiate` — [src/types/scheme.rs:35](../../src/types/scheme.rs#L35))
for ordinary functions and type-class methods. Extending it to
effect ops is a localized change that lifts a capability Flux's
effect system already looks like it has but quietly doesn't.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### The mental model

An effect operation declaration is treated exactly like a function
declaration at the top level, for typing purposes:

```flux
effect Log {
    emit : a -> ()        // generalized to ∀a. a -> ()
    scope<a> : a -> a     // explicit type parameter — same shape
}
```

Every `perform Log.emit(x)` instantiates fresh: the `a` at each call
site is a new unification variable, not the signature's `a`.

### What users see

Today:

```flux
effect Log { emit : a -> () }

fn greet<a>(x: a) with Log {
    perform Log.emit(x)   // error: "Rigid type variable `a` cannot
                          //         be unified with `a`"
}
```

After this proposal:

```flux
effect Log { emit : a -> () }

fn greet<a>(x: a) with Log {
    perform Log.emit(x)   // ok — Log.emit's `a` instantiates fresh,
                          //      unifies with greet's `a`
}

fn use_log() with Log {
    perform Log.emit(42)    // ok — instantiates at Int
    perform Log.emit("hi")  // ok — instantiates at String
}
```

No new syntax. The observable change is that programs that used to
error now type-check.

### What changes for handlers

Nothing. Handler arm bodies already receive the op arg at its
instantiated type (the type the `perform` site presented). A handler
that wants to print any value still writes `emit(resume, v) -> { …
use v … }` — same as today. What changes is that the *check at the
perform site* no longer over-constrains the caller.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Storage change

```rust
// Before
pub effect_op_signatures: HashMap<(Symbol, Symbol), TypeExpr>,

// After
pub effect_op_signatures: HashMap<(Symbol, Symbol), Scheme>,
```

The map keys are unchanged: `(effect_name, op_name)`. The value
changes to `Scheme { forall, constraints, infer_type }`. For an op
with no free type variables (e.g. `read_stdin: () -> String`), the
scheme is trivially monomorphic (`forall = []`).

### Generalization (happens once, at collection time)

During `collect_effect_declarations_from_stmt`
([src/compiler/mod.rs:2169](../../src/compiler/mod.rs#L2169)), when
storing an op signature:

1. Lower the `TypeExpr` to `InferType` using the existing conversion
   (same path the function-signature collection uses).
2. Collect the free `TypeVarId`s in the resulting `InferType` that
   are *not* bound by any enclosing scope. For a top-level effect
   decl, that's all of them.
3. Call `generalize(infer_type, &env_free_vars)` from
   [src/types/scheme.rs:179](../../src/types/scheme.rs#L179) with an
   empty `env_free_vars` set, producing a `Scheme`.
4. Store the scheme.

The existing `TypeExpr` conversion already allocates fresh `Var` IDs
for lowercase identifiers in type expressions; generalization simply
lifts those vars into the scheme's `forall`.

### Instantiation at `perform` sites

`compile_perform` ([src/compiler/expression.rs:3198](../../src/compiler/expression.rs#L3198))
looks up the op signature and validates arg types. Two edits:

1. `effect_operation_function_parts` currently returns
   `(&[TypeExpr], &TypeExpr)` ([line 447](../../src/compiler/expression.rs#L447)).
   It becomes `instantiate(scheme) -> InferType::Fun(params, ret,
   row)`, returning owned `InferType` values.
2. The caller unifies each arg against the instantiated param type
   using `ensure_unify` (the same path function calls use).

`instantiate` already exists in `scheme.rs` and produces fresh
`TypeVarId`s for every `forall` var. No new machinery is needed.

### Handler-arm parameter types

Handler arms today derive param types from the op signature
([src/compiler/expression.rs:3466](../../src/compiler/expression.rs#L3466)).
Once the signature is a `Scheme`, the arm body sees the types at
their **declared** form (the scheme's quantified variables are
rigid inside the arm body — same discipline as a polymorphic
function's param types are rigid inside its body). This is the
standard HM discipline and needs no special case.

### Class constraints on op signatures

`Scheme` already carries `constraints: Vec<SchemeConstraint>`
(introduced by 0145 for type-class dictionary passing). This
proposal uses the same mechanism for effect ops that declare class
bounds:

```flux
effect Compare { lt : a -> a -> Bool where a: Ord }
```

At each `perform Compare.lt(x, y)` site, the row solver requests an
`Ord<a>` dictionary exactly as it does for a polymorphic function
call. No new machinery.

### What does *not* change

- `effect_ops_registry: HashMap<Symbol, HashSet<Symbol>>` — still the
  same shape; it tracks which ops an effect declares, not their
  types.
- `effect_row_aliases` — unchanged.
- Handler-runtime (`OpPerform`, evidence passing, continuation
  capture) — unchanged. Polymorphism is a type-system property; at
  runtime every value is NaN-boxed and the dispatch is untyped.
- Surface syntax. Operations with no free type vars still parse and
  store as monomorphic schemes.

## Drawbacks
[drawbacks]: #drawbacks

- Behavioral change for programs that relied (knowingly or not) on
  the rigid-skolem semantics. In practice this is narrow: the spike
  found no fixture that benefits from rigid op signatures, and
  generalization is strictly more permissive (everything that
  type-checks today continues to type-check).
- One extra step at effect-decl collection (lower → generalize). The
  cost is negligible — effect decls are rare and small.
- Slightly more work at each `perform` site (instantiate scheme).
  Identical in shape and cost to every function call that
  instantiates a `Scheme` today.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Why generalize, rather than add explicit per-op type parameters?

Koka and Effekt both generalize implicitly. Requiring explicit
`op<a>:` syntax would force every effect author to repeat
boilerplate that HM can infer. Flux already uses implicit lowercase
= type variable for function signatures; extending the rule to effect
ops is the least-surprise choice.

If later proposals want explicit type parameters on ops (for
readability or documentation), the parser can accept `op<a, b>: ...`
as sugar for the same generalized form. That is out of scope here.

### Why not make the rigid-skolem behavior an error?

That would require a new diagnostic and still leave the underlying
feature (polymorphic ops) missing. Generalizing is the feature; an
error would be a consolation prize.

### Why not defer until 0162 Phase 2?

0162 Phase 2 adds a separate runtime specialization (State/Reader
continuation elimination) that is orthogonal to type-level
polymorphism. Conflating them grows both proposals. Polymorphic op
signatures are small and self-contained; they ship independently.

### Alternatives considered

- **Per-op monomorphization.** Duplicate the op declaration per call
  site after inference. Explodes the effect surface. Rejected.
- **Default all ops to monomorphic, allow `forall` opt-in.** Puts
  the burden on users to remember syntax, while every realistic
  effect wants generalization. Rejected.

## Prior art
[prior-art]: #prior-art

- **Koka**: effect operation signatures are generalized by default.
  `effect log { fun emit(msg: a) : () }` is ∀a. See
  `koka/lib/std/core/exn.kk` for real examples with quantified op
  signatures.
- **Effekt**: effect operations are elaborated to capability
  parameters whose types are generalized alongside the declaring
  scope. Polymorphic ops are the common case.
- **Unison**: abilities (Unison's term for effects) are declared
  with generic type parameters and each ability constructor is
  treated as a polymorphic function by the inferencer.
- **Flux today**: partial. Function `Scheme`s generalize; effect
  ops do not. This proposal closes the gap.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- **Row polymorphism inside op return types.** An op that returns a
  value with its own effect row (e.g., `call : a -> b with e`) raises
  the question of whether `e` is generalized. Recommend: yes, unless
  `e` is the enclosing effect's own label, in which case it's fixed.
  Revisit with fixture coverage after the basic case lands.
- **Interaction with `handle` return-type inference.** Handlers
  today infer their return type from the wrapped expression.
  Polymorphic ops don't affect handler return types, but the fixture
  suite should include a case where different `perform` sites
  instantiate the same op at incompatible types, just to pin the
  behavior.
- **Diagnostic quality.** When unification at a `perform` site
  fails, the message should name the instantiated form of the op,
  not the scheme's quantified form. Existing HM diagnostics
  substitute after unification, so this should be free, but the
  fixture suite should include a deliberate mismatch to confirm.

## Future possibilities
[future-possibilities]: #future-possibilities

- **0165 becomes mechanical.** The Console-only slice that was
  spiked and reverted can land as a short PR: route `println`/`print`
  through `Perform` and synthesize a default handler. The signature
  becomes `println: a -> ()` — generalized — and existing
  polymorphic call sites continue to work unchanged.
- **Effect operations with class constraints.** With `Scheme`'s
  existing constraint support, an effect can declare
  `compare : a -> a -> Bool where a: Ord`. The dictionary-passing
  infrastructure from 0145 handles this with no additional work.
- **Effect aliases of parameterized ops.** Once ops generalize,
  0161's alias system can compose effects whose ops share type
  parameters. Tracked separately if it becomes relevant.
