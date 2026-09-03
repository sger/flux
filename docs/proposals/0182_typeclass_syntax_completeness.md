- Feature Name: Type class syntax completeness
- Start Date: 2026-09-03
- Proposal PR:
- Flux Issue:

# Proposal 0182: Type Class Syntax Completeness

## Summary
[summary]: #summary

Allow more than one constraint before `=>` in a class or instance head, allow a
superclass constraint over a non-variable type, and settle whether `where`
becomes the single spelling for class contexts. Every gap named here is a
*parser* limitation over machinery that already exists: the solver, the
dictionary layout, and the interface format all carry lists of predicates
already, and the head is the one place that accepts exactly one.

## Motivation
[motivation]: #motivation

[Proposal 0179](implemented/0179_typeclass_soundness_dictionary_passing_and_associated_types.md)
made type classes sound across nine stages, then closed leaving the surface
syntax undocumented and unaudited. Writing
[`docs/internals/type_class_syntax.md`](../internals/type_class_syntax.md)
audited it for the first time — every form run, accepted or rejected — and the
rejections cluster into one cause.

`parse_class_statement` and `parse_instance_statement`
([src/syntax/parser/statement.rs](../../src/syntax/parser/statement.rs)) each
parse a single `Name<args>`, check for `=>`, and construct a one-element
`vec![constraint]`. There is no loop and no parenthesised list. Six of the seven
rejected forms are that one decision seen from different angles:

| Form | Today |
|---|---|
| `class (Eq<a>, Show<a>) => Ord<a>` | E034 |
| `class Eq<a> => Show<a> => Ord<a>` | E034 |
| `class Eq<List<a>> => Ord<a>` | E034 |
| `instance (Eq<a>, Show<a>) => C<Pair<a>>` | E034 |
| `instance C<T> where Eq<a>, Show<a>` | E034 |
| `class Box<a: Eq> { ... }` | E034 |

For superclasses there is a partial workaround: a transitive chain of single
links is traversed correctly, so `Eq => Named => Ranked` gives a function
constrained by `Ranked` access to `eq`. That works only when the classes
genuinely nest. Two *independent* superclasses have no workaround, and neither
does an instance needing two contexts — the shape a `Pair<a, b>` instance wants
the moment it needs both `Eq<a>` and `Eq<b>`.

The cost is already visible in the standard library. `Flow.Ord` names its
superclass evidence explicitly on every instance (`Eq<Int> => Ord<Int>`) because
one constraint is all a head can hold; anything needing two must be restructured
or abandoned.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

A class or instance head takes a *list* of constraints, parenthesised when there
is more than one:

```flux
class (Eq<a>, Show<a>) => Ord<a> { fn compare(x: a, y: a) -> Int }

instance (Eq<a>, Eq<b>) => Eq<Pair<a, b>> {
    fn eq(p, q) { ... }
}
```

A single constraint keeps its current unparenthesised spelling, so no existing
program changes:

```flux
class Eq<a> => Ord<a> { ... }
instance Eq<a> => Eq<List<a>> { ... }
```

A superclass constraint may name a structured type, not only a bare parameter:

```flux
class Eq<List<a>> => Ord<a> { ... }
```

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

Four changes, in dependency order.

**1. Parse a constraint list in both heads.** Replace the single-constraint
branch in `parse_class_statement` and `parse_instance_statement` with a shared
helper that accepts either one bare constraint or a parenthesised
comma-separated list, and returns `Vec<ClassConstraint>`. Both `Statement::Class`
and `Statement::Instance` already hold `Vec`, so the AST is unchanged.

**2. Parse structured superclass arguments.** `parse_class_statement` currently
builds superclass `type_args` by mapping the bare identifiers returned by
`parse_type_params_angle_bracket` into `TypeExpr::Named { args: vec![] }`. Use
`parse_instance_type_args`, which already parses full type expressions, as the
instance path does.

**3. Extend superclass evidence to a list.** A dictionary leads with one slot per
directly declared superclass. The layout is already positional and already
plural; what needs auditing is every site that assumes at most one leading slot
— dictionary construction, superclass projection, and the `.flxi` interface
round-trip. `superclass_across_modules.flx` and `SuperclassMetadata.flx` are the
existing contracts for that boundary.

**4. Decide the `where` question.** 0179 left it open. Three spellings exist for
one concept: `<a: C>`, `where C<a>`, and `=>` in a head. This proposal takes no
position beyond requiring that one be taken; see
[unresolved questions](#unresolved-questions).

`class Box<a: Eq>` — a bound on a class parameter — is deliberately **out of
scope**. Its meaning is not obvious (a constraint on every instance? on every
use?), it has no demand behind it, and unlike the others it is not the same
one-line parser decision.

## Drawbacks
[drawbacks]: #drawbacks

Cycle detection, superclass ordering, and coherence checking all get harder with
a constraint list; E477 currently reports one cycle anchored at its
first-declared class, and a lattice rather than a chain makes "the cycle" less
well defined.

More seriously, this widens a surface that already has open soundness bugs.
[KI-071](../known_issues.md#ki-071) (an instance method capturing a module-level
function, with a native SIGSEGV) and
[KI-076](../known_issues.md#ki-076) (operators not dispatching inside a module)
are both live. Adding syntax over them makes the existing defects reachable from
more programs.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

**Do nothing.** Defensible for superclasses, where the transitive chain covers
the common case. Not defensible for instance contexts: a two-parameter container
needing evidence for both parameters is ordinary, and has no workaround.

**Allow repeated `=>` instead of a parenthesised list** (`Eq<a> => Show<a> => C<a>`).
Fewer parser changes, but it reads as nesting rather than conjunction, and it
was already tried and rejected during the audit as the less clear of the two.

**Infer superclass evidence rather than requiring it.** `Flow.Ord` names its
`Eq` evidence explicitly because leaving it to be solved sends the solver into a
loop that overflows the compiler's stack (0179 Stage 8). Fixing that is a
separate and larger change, and it would reduce the demand for this one without
removing it.

## Prior art
[prior-art]: #prior-art

### Haskell

Haskell 2010 has had the parenthesised constraint list since the beginning, in
both positions this proposal wants it:

```haskell
class (Eq a, Show a) => Ord a where
    compare :: a -> a -> Ordering

instance (Eq a, Eq b) => Eq (a, b) where
    (x1, y1) == (x2, y2) = x1 == x2 && y1 == y2
```

A single constraint is written without parentheses (`class Eq a => Ord a`),
which is the compatibility rule this proposal adopts.

Three of Flux's other limits map onto Haskell restrictions that were also
present at first and later relaxed by extension, which is worth knowing because
it says the restrictions are a real design point rather than an oversight:

| Flux today | Haskell 2010 | Relaxed by |
|---|---|---|
| Superclass args must be bare variables (`class Eq<List<a>> => …` rejected) | Same restriction: a superclass context must be `C a` for a class variable | `FlexibleContexts` |
| Instance heads over structured types | Restricted to `C (T a1 … an)`, distinct variables | `FlexibleInstances` |
| Multi-parameter classes | Not in the standard at all | `MultiParamTypeClasses` |

So Flux already ships something Haskell needed an extension for
(multi-parameter classes, which `Convert<a, b>` uses), while lacking something
Haskell has had since 1990 (a two-element context). That inversion is the
clearest evidence the single-constraint head is an implementation artifact.

Two further points bear on the design rather than the grammar:

- **Default method bodies** are standard Haskell and are how the real hierarchy
  stays usable — `Eq` defines each of `==` and `/=` in terms of the other, so an
  instance supplies one. Flux supports default bodies (verified; see the syntax
  reference) but `Flow.Eq` does not use them, defining both `eq` and `neq` in
  every instance. That is redundancy the feature already permits removing.
- **Functional dependencies** (`class Convert a b | a -> b`) are how Haskell
  makes a multi-parameter class unambiguous. Flux instead resolves the second
  parameter from the type the result is required to have, which 0179 Stage 4
  built. It reaches the same place for the common case without new syntax, which
  is why 0179 recorded fundeps as future work rather than a prerequisite.

### Rust

Rust is the closer precedent for the `where` question, because it has the same
duality Flux does:

```rust
fn twice<T: Sized>(x: T) -> usize { ... }
fn twice<T>(x: T) -> usize where T: Sized { ... }
```

These are exactly equivalent, both spellings are idiomatic, and the convention
that emerged is stylistic: inline bounds for short signatures, `where` when they
grow. Supertraits take a `+`-separated list (`trait Ord: Eq + PartialOrd`),
which is a third spelling for what Haskell writes as a context.

**This settles the open question in a useful direction.** The precedent does not
say "pick one" — Rust kept both for thirty years of its existence without
trouble. What it requires is that the two spellings be *equivalent*. Flux's are
not: `<a: C>` always means `C<a>` and so cannot reach a multi-parameter class,
making `where` the only spelling for `Convert<a, b>`. The defect is the
asymmetry, not the plurality.


## Unresolved questions
[unresolved-questions]: #unresolved-questions

- Inherited from 0179: should `where` become the single spelling for class
  contexts? The Rust precedent above argues the plurality is not itself the
  problem — the asymmetry is. That reframes the question to: **should `<a: C>`
  be extended to reach multi-parameter classes, or should it be documented as
  deliberately limited to one?** Either answer closes 0179's question; leaving
  the two spellings silently unequal does not.
- Should a lowercase class name be rejected at its declaration, or should
  class-ness stop being inferred from capitalisation?
  ([KI-074](../known_issues.md#ki-074) — the two must not both stand.)
- Does superclass cycle detection stay one diagnostic per cycle when the
  hierarchy becomes a lattice?

## Future possibilities
[future-possibilities]: #future-possibilities

Functional dependencies and richer `deriving` were both recorded as future work
by 0179 and are unaffected by this proposal. Overlapping instances, quantified
and higher-rank constraints, and full type families remain explicit non-goals.
