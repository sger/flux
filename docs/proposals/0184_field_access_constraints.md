- Feature Name: `field_access_constraints`
- Start Date: 2026-09-04
- Proposal PR:
- Flux Issue:

## Summary
[summary]: #summary

Make `record.field` emit a constraint instead of a hole. Today, dot access on a
receiver whose type is not yet a known named-field ADT allocates a *fallback
variable* — a type variable deliberately marked as un-generalizable — and hopes
a later unification fills it in. That works only while the enclosing definition
is bound monomorphically, which is why Flux cannot generalize an unannotated
definition, and why an obligation over such a variable has no terminal state.
This proposal replaces the hole with a `HasField` predicate the solver
discharges, following GHC's `HasField` (`GHC.Internal.Records`).

## Motivation
[motivation]: #motivation

`infer_member_access_expression` (`src/ast/type_infer/expression/access.rs`)
ends like this:

```rust
let object_ty = self.infer_expression(object);
if let Some(field_ty) = self.resolve_named_field_access(&object_ty, member, expr.span()) {
    return field_ty;
}
self.alloc_fallback_var()
```

`resolve_named_field_access` needs `adt_name_of(&resolved)` to succeed, so a
receiver that is still a variable produces no field type and no record of what
was wanted. The fallback variable that stands in its place is not an ordinary
unification variable: `resolve_binding_schemes` builds every scheme with
`forall = free_vars(resolved) - fallback_vars`, so a fallback variable can
never be quantified. It is a hole that must be filled by unifying the
definition's type with a call site, or it becomes `E430`.

The adjacent tuple path states the intent outright — it constrains the receiver
to a tuple shape so that "later call-site unification [can] discharge local
helper projections like `pair.0` instead of poisoning the expression with a
fallback hole".

Three consequences, all measured while implementing Proposal 0183:

1. **Unannotated definitions cannot generalize.** Generalizing a definition
   decouples it from its call sites, which is precisely the channel the hole
   depends on. Making every definition generalize dropped the stdlib's stuck
   predicates from 15 to 6, and broke `mini_database.flx`
   (`E430` on `record.id`), `function_arg_mismatch.flx` (an unresolved variable
   reaching the backend) and `multi_file_lib.flx` (`E998`, a duplicate binder
   in a `Lam`). The patch is kept at
   `scratchpad/r6-generalize-unannotated.patch`.
2. **0183's central claim stays false.** "Every class constraint has a terminal
   outcome" cannot hold while a predicate can rest on a variable that is
   neither solvable nor quantifiable.
3. **The error lands far from the cause.** `record.id` reports at a call site,
   or at the backend as "definition `9` still has unresolved type variable
   #10784", rather than where the field was read.

This is the same defect Proposal 0183 fixed for `+`: a construct typed by
deferred unification rather than by a constraint. `+` was resolved by giving it
a class (`Flow.Add`); field access needs the same treatment.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

Nothing changes for code that already works. `record.id` on a value whose type
is known resolves exactly as it does now.

What changes is code where the receiver's type is not yet known:

```flux
fn label(r) { r.name }
```

Today this compiles, and `r.name` has no type at all until some call site
happens to pin `r`. Under this proposal `label` records what it needs — `r` has
a field `name` — and the requirement is discharged at each call:

```flux
label(Person("Ada"))     // fine: Person has `name`
label(42)                // error: Int has no field `name`, reported at the call
```

If nothing ever determines `r`, that is now an error at the definition, naming
the field:

```
error[E4xx]: Unknown Field
Cannot tell which type `r` is, so the field `name` cannot be resolved.
```

Stage 1 of this proposal reports that case rather than generalizing over it, so
`fn label(r) { r.name }` used at two different record types is still an error —
the same as today, but reported at the definition and in terms of the field.
Stage 2 lifts that restriction.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### The predicate

Add a solver-internal predicate `HasField`, mirroring GHC's

```haskell
class HasField x r a | x r -> a where
  getField :: r -> a
```

with the functional dependency `x r -> a`: the field name and the record type
together determine the field type.

Flux has no type-level strings, so the field name is carried in the predicate's
*identity* rather than in a type argument: a `ClassId` in a reserved module
(`__field`) whose name is the field identifier. `r.name` emits
`__field.name<R, T>` — two type arguments, the receiver and the field type. This
needs no new type-level machinery: `SchemeConstraint` already carries
`class_id` plus `Vec<InferType>`, and the reserved module keeps these out of
the surface language and off the orphan rules.

### Solving

`classify_constraint` gains one rule, placed with the other structural rules:

- If the receiver is still a variable, the predicate is undecided — the ordinary
  deferred state, not a hole.
- If the receiver resolves to a named-field ADT, run the existing
  `resolve_named_field_access` logic, unify its result with the field-type
  argument, and discharge with `Evidence::Structural`. The diagnostics it
  already emits (`NAMED_FIELD_NOT_ON_TYPE`, `NAMED_FIELD_TYPE_DIVERGES`) move
  here unchanged, so they fire at the same places with better spans.
- If the receiver resolves to anything else, report: that type has no fields.

The functional dependency is what makes this useful — discharging
`__field.name<Person, t>` *determines* `t`, so the field type propagates the way
the hole never could.

### Two stages

**Stage 1 — emit and report.** *(Shipped.)* Field access emits the predicate;
the fallback variable at that site is deleted. `collect_scheme_constraints` does *not* retain
`HasField`, so nothing generalizes over it, and an undischarged one at
whole-program scope is reported. This alone removes the hole and gives 0183 the
terminal state it needs, with no change to lowering: every predicate that
survives is either discharged against a known ADT or reported.

**Stage 2 — generalize.** Allow a scheme to carry `HasField`, making
`fn label(r) { r.name }` genuinely polymorphic over records with a `name` field.
This needs evidence with runtime content — an accessor — where Stage 1's
evidence is purely static, so it is where the dictionary work lives. It is
separable and should not block Stage 1.

### What this unblocks

Proposal 0183's R6 needs unannotated definitions to generalize, which needs
Stage 1 (so field access no longer depends on call-site unification) and,
for functions that are genuinely record-polymorphic, Stage 2.

### Stage 1 as shipped

Discharge runs as a pass at the end of inference (`discharge_field_predicates`
in `src/ast/type_infer/mod.rs`) rather than inside `classify_constraint`. Two
reasons: the tables saying which variants carry which field live on `InferCtx`,
not on the `ClassEnv` the solver holds; and by the end of inference every
receiver any call site determines *is* determined, which is exactly when the
question can be answered. The predicates are removed from the wanted set either
way — they carry no dictionary and the class solver has no rule for them.

The predicate is raised only for a receiver whose type is still a variable, and
only when the receiver is bound as a value. `a.b` is also the syntax for
reaching into a module, and an unresolved module member falls through to the
same code path — a missing, private or misspelled import, already reported as
`E011`/`E012`/`E013`. Measured: without those two guards, 30 of 1,305 programs
gained a spurious `E490`, `lib/Flume/*` among them; with them, **zero programs
change**. The cases that forced each guard are `Lock.lock(..)`, where `Lock`
names both a module alias and a constructor, and `Parse.here(..)`, a module
referred to from inside its own body, which has neither a binding nor a type.

`E490` is reported at the access, naming the field and the receiver:

```
error[E490]: Unresolved Field Receiver
Cannot tell which type this is, so the field `name` cannot be resolved. Inferred receiver: `_`.
  fld2.flx:1:15
1 | fn label(r) { r.name }
  |               ^^^^^^
```

Stage 2 — letting a scheme carry the predicate, so `fn label(r) { r.name }` is
genuinely record-polymorphic — is not implemented. Its evidence has runtime
content (an accessor), where Stage 1's is purely static.

## Drawbacks
[drawbacks]: #drawbacks

The predicate is solver-internal, so users cannot write `HasField` bounds by
hand, and a diagnostic that mentions one is exposing an internal name. The
reserved `__field` module keeps the collision surface small but does not make
the concept invisible.

Stage 1 reports some programs that compile today — those where a field is read
from a receiver no call site ever determines. Every such program is one whose
field access is currently untyped, so this is a real tightening, and it needs
the same fallout sweep 0183's R7 describes.

`is_concrete`-style checks elsewhere in the compiler assume a type either is or
is not resolved. A predicate that determines a type argument on discharge adds
a third state — "determined by solving" — and any pass that samples types
mid-solve may need to zonk first.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

**Structural record types with row polymorphism.** Add
`InferType::Record(row)` with a tail variable, unified like `InferEffectRow`
(`{ concrete: HashSet<Identifier>, tail: Option<TypeVarId> }`), as PureScript
and Elm do. More expressive: it gives anonymous records and record extension for
free. Rejected as the primary design because Flux's records are *named-field
variants of nominal ADTs*, not structural values — `resolve_named_field_access`
merges a field's type across every variant of the ADT and lifts to `Option<T>`
when only some variants carry it. A structural row cannot express that, so this
would be a second, parallel record system rather than a fix to the existing one.
The row machinery remains the right answer if Flux ever adds anonymous records.

**Keep the hole, and make generalization avoid it.** Do not generalize a
definition whose type mentions a fallback variable. This was implemented and
measured while investigating 0183: restricting quantification to the variables
a class constraint mentions fixed one of five broken programs and left four,
because the hole is reached through the *field type*, which the constraint does
not mention. It also leaves field access untyped, so it fixes nothing else.

**Require annotations.** Reject `fn label(r) { r.name }` outright. Simple, and
consistent with the current `E430`, but it makes an ordinary HM program illegal
for an implementation reason and does not give 0183 its terminal state — an
unannotated helper would still have to be rejected rather than solved.

## Prior art
[prior-art]: #prior-art

- **GHC**, `GHC.Internal.Records`: `class HasField x r a | x r -> a`, solved
  automatically by the constraint solver with manual instances permitted. The
  functional dependency and the automatic solving are what this proposal copies.
  `OverloadedRecordDot` desugars `r.x` to `getField @"x" r`.
- **PureScript / Elm**: structural row polymorphism, `{ name :: String | r }`.
  The alternative considered above.
- **OCaml** object types and **Ur/Web** records: row typing with a different
  surface, same underlying idea.
- **Flux itself**: effect rows are already row-polymorphic
  (`InferEffectRow`), and Proposal 0183 gave `+` a class rather than a built-in
  unification rule. This is the same move for a third construct.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- Should `HasField` be user-writable as a bound? Doing so needs a surface syntax
  for the field name and turns an internal predicate into a language feature.
- What evidence does Stage 2 pass — a field offset, or a closure? Offsets are
  cheaper but assume a uniform layout across the variants the ADT merges.
- Does the `Option<T>` lift for a field present on only some variants belong in
  the predicate's discharge, or should the predicate carry the lifted type
  directly? The first keeps `resolve_named_field_access` as the single source of
  that rule.
- Tuple projection (`pair.0`) has the same shape and the same comment about
  call-site unification. Should it become a predicate too, or is constraining
  the receiver to a tuple shape sufficient once definitions generalize?

## Future possibilities
[future-possibilities]: #future-possibilities

Stage 2's accessor evidence is most of what record update (`{ ...base, field: v }`)
on an unknown receiver would need, and `setField` is GHC's other half of the
same class. Anonymous records, if Flux ever wants them, would build on the row
machinery this proposal deliberately does not add.
