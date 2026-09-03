# Flux Type Class Syntax — Reference v1

## Purpose

The complete surface syntax of Flux type classes: every form the parser accepts,
every form it rejects, and the gap between the two.

This is a *reference*, not a narrative. [Proposal
0179](../proposals/implemented/0179_typeclass_soundness_dictionary_passing_and_associated_types.md)
specified the semantics across nine stages and carries ten `Example syntax:`
blocks, but they are scattered through a change history and describe what each
stage altered rather than what is legal today. [`type_classes.md`](type_classes.md)
documents the dictionary calling convention and the compiler's data flow.
[Guide chapter 21](../guide/21_type_classes.md) teaches the feature. None of
them answers "what can I write?", which is what this file is for.

**Every example below was run.** The supported forms are collected into one
program, [`examples/type_classes/syntax_tour.flx`](../../examples/type_classes/syntax_tour.flx),
which is this document's executable half — if a form stops working, the tour
stops printing. The rejected forms were each confirmed to fail, and the error
code shown is the one actually produced.

Authority for the grammar is `parse_class_statement`, `parse_instance_statement`,
`parse_function_type_params_angle_bracket` and `parse_where_constraints` in
[src/syntax/parser/statement.rs](../../src/syntax/parser/statement.rs).

## Status legend

| | |
|---|---|
| **Works** | Accepted, and verified running on the VM. |
| **Works (VM only)** | Accepted, but the native backend disagrees. Bug recorded. |
| **Rejected** | The parser or checker refuses it. Whether it *should* is a separate question. |

---

## 1. Declaring a class

```
class [Superclass<a> =>] Name<params> { members }
```

### One type parameter — **Works**

```flux
class Sizeable<a> { fn size(x: a) -> Int }
```

### Several type parameters — **Works**

```flux
class Convert<a, b> { fn convert(x: a) -> b }
```

A parameter that appears only in a method's *return* type is resolved from the
type the call site demands. This is what makes `Convert` usable at all, and it
is why `let text: String = convert(42)` picks a different instance from
`let flag: Bool = convert(42)`.

### A superclass — **Works, exactly one**

```flux
class Named<a> => Ranked<a> { fn rank(x: a) -> Int }
```

A `Ranked` dictionary leads with an evidence slot for `Named`, so a function
constrained by `Ranked` alone can still call `name_of`.

Longer hierarchies are built by stacking single links, and the chain is
traversed transitively — this works, and `describe` below reaches `Named`
through two hops:

```flux
class Eq2<a> { fn eq2(x: a, y: a) -> Bool }
class Eq2<a> => Named<a> { fn name_of(x: a) -> String }
class Named<a> => Ranked<a> { fn rank(x: a) -> Int }

fn describe<a: Ranked>(x: a) -> String { name_of(x) }
```

Two *independent* superclasses cannot be written. See [§7](#7-rejected-forms).

### A default method body — **Works**

A class method may carry a body. Instances that do not override it inherit it.

```flux
class Greet<a> {
    fn label(x: a) -> String
    fn greet(x: a) -> String { "hello " + label(x) }
}

instance Greet<Int> { fn label(x) { to_string(x) } }
```

`greet(5)` yields `"hello 5"` with no `greet` in the instance. This form is
undocumented outside this file, and is easy to miss because the class body
otherwise looks like a list of signatures.

### Method-level type parameters and higher-kinded parameters — **Works**

A class parameter may stand for a type constructor, and a method may introduce
type parameters of its own:

```flux
class Mappable<f> { fn fmap<a, b>(xs: f<a>, g: (a) -> b) -> f<b> }
```

Kind correctness is checked: `instance Mappable<Int>` is rejected, because `Int`
takes no argument.

### An associated type — **Works**

A type the *instance* chooses, rather than one the caller supplies:

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

Inside a function generic over `c`, `Element<c>` is *stuck* — a type waiting for
the call site that fixes `c`. That is not an error. Two stuck applications unify
only when they name the same declaration and their arguments unify; associated
types are not injective, so `Element<a> = Element<b>` never implies `a = b`.

---

## 2. Declaring an instance

```
instance [Context<a> =>] Class<types> { members }
```

### A plain instance — **Works**

```flux
instance Sizeable<Int> { fn size(x) { x } }
```

Method parameters are unannotated. The class declaration supplies the types.

### A contextual instance — **Works, exactly one context**

```flux
instance Render<a> => Render<List<a>> {
    fn render(xs) {
        match xs {
            [h | _] -> "[" + render(h) + "]",
            _ -> "[]",
        }
    }
}
```

The context is a dictionary the instance receives at construction, so the
recursive `render(h)` reaches the *element's* instance rather than looping.
A contextual instance lowers to a dictionary constructor — a function over its
context dictionaries — where a plain instance lowers to a tuple.

Two independent contexts cannot be written. See [§7](#7-rejected-forms).

### An associated type equation — **Works**

```flux
type Element<List<Int>> = Int
```

The left side repeats the instance head; it is an equation, not a binding.

---

## 3. Constraining a function

Four spellings. All lower to the same predicate, so the choice is stylistic
except where noted.

### `<a: C>` — **Works**

```flux
fn twice<a: Sizeable>(x: a) -> Int { size(x) + size(x) }
```

The sugar constrains *the parameter itself*: `<a: C>` always means `C<a>`. It
therefore reaches only classes of one parameter.

### `<a: C + D>` — **Works**

```flux
fn describe<a: Sizeable + Render>(x: a) -> String {
    render(x) + ":" + to_string(size(x))
}
```

### `where C<a>` — **Works**

```flux
fn thrice<a>(x: a) -> Int where Sizeable<a> { size(x) + size(x) + size(x) }
```

Identical in meaning to `<a: Sizeable>`. Both produce one `ClassConstraint` with
its own span, which is why a constraint diagnostic can underline `Sizeable`
rather than the whole signature.

### `where C<a, b>` — **Works (VM only)**

```flux
fn via<a, b>(x: a) -> b where Convert<a, b> { convert(x) }
```

**The only spelling that reaches a multi-parameter class**, because the `<a: C>`
sugar cannot supply a second argument. The VM resolves this correctly; the
native backend prints `<value>` — see [KI-073](../known_issues.md#ki-073).

### `where C<a>, D<a>` — **Works**

```flux
fn describe<a>(x: a) -> String where Sizeable<a>, Render<a> { ... }
```

Constraints are comma-separated, and the `where` clause may be repeated.

### How a `where` constraint finds its parameter

`parse_where_constraints` attaches each constraint to the first declared type
parameter its arguments mention. **A constraint naming no declared parameter is
silently attached to the first one**, where the solver reports it against the
real class. That fallback is deliberate but undocumented; it means a typo in a
constraint's argument produces a diagnostic about a parameter you did not write.

### Where `where` is *not* accepted

Only in a function signature. `instance C<T> where ...` and `class C<a> where ...`
are parse errors — an instance context is written with `=>` before the head.

---

## 4. `deriving`

```flux
data Point { Point { x: Int, y: Int } } deriving (Eq)
```

**Works.** Multiple classes are comma-separated; a trailing comma is a
diagnostic. Class names may be qualified — `deriving (Flow.Eq.Eq)` works.

Which classes can be derived is a semantic question answered in
[`type_classes.md`](type_classes.md#what-deriving-accepts), not a syntactic one.

---

## 5. Visibility and modules

`public` precedes `class` and `instance` exactly as it precedes `fn` and `data`,
and both are legal inside a `module` block:

```flux
module Shapes {
    public class Area<a> { fn area(x: a) -> Int }
    public instance Area<Sq> { fn area(x) { ... } }
}
```

A module-owned class is mangled with its module's name, so two modules may each
declare a class of the same name without collision.

**Two traps live here**, both found by converting real code:

- An instance method's name **captures unqualified calls to a module-level
  function of the same name** ([KI-071](../known_issues.md#ki-071)). Declaring
  `instance Ord<Version>` inside a module that already has `fn compare` silently
  rebinds every bare `compare(...)` in that module to the class stub.
- An operator on a class-constrained rigid type parameter does not dispatch
  through the dictionary **inside a `module` block**, though it does at top
  level ([KI-076](../known_issues.md#ki-076)).

---

## 6. Lexical rules

Class names are conventionally capitalised, and one place *requires* it.
`class sz<a> { ... }` declares fine, but `where sz<a>` cannot be written: the
parser decides whether a signature-position `where` begins a constraint or a
local binding by testing whether the next identifier starts with an uppercase
letter (`peek_starts_class_constraint`). A lowercase class is therefore
declarable but unusable in the `where` spelling, and the resulting error blames
a missing function body ([KI-074](../known_issues.md#ki-074)).

---

## 7. Rejected forms

Each was run; the error code is the one actually produced.

| Form | Result |
|---|---|
| `class (Eq<a>, Show<a>) => Ord<a>` — two superclasses | **E034**, parse error |
| `class Eq<a> => Show<a> => Ord<a>` — chained spelling of the same | **E034**, parse error |
| `class Eq<List<a>> => Ord<a>` — superclass over a non-variable | **E034**, parse error |
| `instance (Eq<a>, Show<a>) => C<Pair<a>>` — two contexts | **E034**, parse error |
| `instance C<T> where Eq<a>, Show<a>` — contexts via `where` | **E034**, parse error |
| `class Box<a: Eq> { ... }` — a bound on a class parameter | **E034**, parse error |
| `fn via<a: Convert>(...)` — bound sugar on a 2-parameter class | **E444**, misleading hint |

The first six are the same limitation seen from six directions: **the parser
accepts exactly one constraint before `=>`**, and builds its arguments from bare
identifiers only. `parse_class_statement` and `parse_instance_statement` both
construct a one-element `vec![constraint]`; there is no loop and no parenthesised
list.

The transitive chain in [§1](#a-superclass--works-exactly-one) is a partial
workaround for superclasses, and only when the classes genuinely nest. Two
*independent* superclasses, and any instance needing two contexts, have no
workaround at all.

The seventh is a diagnostic bug rather than a grammar limit: the hint proposes
`instance Convert<Int> { ... }`, a one-argument instance of a two-parameter
class, which cannot be written either ([KI-075](../known_issues.md#ki-075)).

**These gaps are the subject of [Proposal
0182](../proposals/0182_typeclass_syntax_completeness.md).**

---

## 8. Open question inherited from 0179

0179 closed leaving one syntax question undecided: **whether `where` should
become the single spelling for class contexts.** Today `<a: C>` and
`where C<a>` are equivalent for one-parameter classes, while multi-parameter
classes are reachable only through `where`, and instance contexts only through
`=>`. Three spellings for one concept is the status quo, not a decision.

Converting real code makes the choice concrete rather than theoretical, so the
question belongs with 0182 rather than ahead of it.
