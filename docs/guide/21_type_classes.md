# Chapter 21 — Type Classes

> Worked examples: [`examples/type_classes/`](../../examples/type_classes/) — every fixture below is compiled and run by the test suite, and most are parity-checked across VM and LLVM.

## Learning Goals

- Write a `class` and an `instance`, and know what each one contributes.
- Constrain a function with `<a: Ord>` and understand what that costs at run time.
- Read the standard hierarchy as ordinary Flux source in `lib/Flow/`.
- Know which classes arrive automatically and which you import.
- Use a superclass so one constraint gives you two classes' methods.
- Recognise the three shapes of dispatch, including the one with no argument to dispatch on.

---

## Overview

A type class names a capability that several types can have, and lets a
function require it without naming those types. `Ord` is the capability of
being ordered; `Int` and `String` both have it; a sorting function needs only
`Ord`, not a list of every type it might sort.

Flux has had classes since well before this chapter. What is new is that the
standard ones are no longer built into the compiler — they are Flux source you
can open and read.

## Declaring a class

A `class` declares the methods a type must supply. It is a signature, not an
implementation:

```flux
class Container<a> {
    fn empty_container(x: a) -> Bool
    fn size_of(x: a) -> Int
}
```

An `instance` supplies them for one type:

```flux
instance Container<String> {
    fn empty_container(x) { x == "" }
    fn size_of(x) { Flow.String.string_len(x) }
}
```

Instance methods take no type annotations — the class already fixed them.

## Constraining a function

Write the constraint on the type parameter:

```flux
fn describe<a: Container>(x: a) -> String {
    if empty_container(x) { "empty" } else { to_string(size_of(x)) }
}
```

`describe` works for every type with a `Container` instance, and for no other —
that is the difference between a class constraint and `Any`. The compiler
rejects the call rather than discovering the problem at run time.

**What it costs.** A constrained function receives a hidden extra argument: a
*dictionary*, the tuple of methods for whichever instance the caller used.
`describe("")` passes `Container<String>`'s. This is why the constraint is on
the signature rather than inferred silently — it is part of the calling
convention.

## The standard hierarchy

These live in `lib/Flow/` as Flux source. Reading them is the fastest way to
see what a real instance looks like.

| Class | Methods | Instances |
|---|---|---|
| `Eq` | `eq` `neq` | `Int` `Float` `String` `Bool`, `List<a>`, `Option<a>` |
| `Ord` | `compare` `lt` `lte` `gt` `gte` | `Int` `Float` `String` |
| `Num` | `add` `sub` `mul` `div` | `Int` `Float` |
| `Show` | `show` | `Int` `Float` `String` `Bool` |
| `Semigroup` | `append` | `String`, `List<a>`, `Array<a>`, `Option<a>` |
| `Monoid` | `mempty` | `String`, `List<a>`, `Array<a>`, `Option<a>` |
| `Functor` | `fmap` | `List` `Option` `Array` |
| `Applicative` | `pure` `ap` | `List` `Option` `Array` |
| `Monad` | `bind` | `List` `Option` `Array` |

### Operators are class methods

`==` is `eq`, `<` is `lt`, `+` is `add`. Writing `x == y` in a function
constrained by nothing in particular is what puts an `Eq` obligation on it:

```flux
fn same<a: Eq>(x: a, y: a) -> Bool { x == y }
```

On a concrete type the compiler skips the dictionary entirely and emits the
machine comparison, so `1 == 2` costs nothing extra.

`append` has no operator — `++` is not a Flux operator — so `Semigroup` is
always reached by calling `append(x, y)`.

### Which ones you get for free

`Eq`, `Ord`, `Num`, `Show` and `Semigroup` are in the **prelude**: available in
every module without an import, because the operators desugar to them.

`Monoid`, `Functor`, `Applicative` and `Monad` are **explicit import**:

```flux
import Flow.Monad
```

No operator desugars to them, so a program that does not use them does not pay
to compile them.

## Superclasses

A class may require another. `Ord` requires `Eq`, so every ordered type is also
an equatable one:

```flux
class Eq<a> => Ord<a> {
    fn compare(x: a, y: a) -> Int
    // ...
}
```

The payoff is at the use site. A function constrained by `Ord` **alone** can
still call `eq`:

```flux
fn both<a: Ord>(x: a, y: a) -> String {
    if eq(x, y) { "same" } else { if lt(x, y) { "less" } else { "more" } }
}
```

`eq` is not an `Ord` method. It is reached through the `Eq` evidence carried in
the `Ord` dictionary — one dictionary, two classes' methods. See
[`eq_ord.flx`](../../examples/type_classes/eq_ord.flx).

Superclasses chain. `Monad` requires `Applicative`, which requires `Functor`,
so a `Monad`-constrained function reaches all three:
[`functor_applicative_monad.flx`](../../examples/type_classes/functor_applicative_monad.flx).

An instance names its superclass evidence:

```flux
instance Eq<Int> => Ord<Int> { /* ... */ }
```

If the evidence does not exist, the compiler says so — **E445**, "Missing
Superclass Instance".

## Instances that depend on other instances

Two lists are equal when their elements are. That is a *contextual* instance —
the part before `=>` is what it needs, not a superclass:

```flux
instance Eq<a> => Eq<List<a>> {
    fn eq(xs, ys) { /* ... */ }
}
```

`eq([1, 2], [1, 2])` works because `Eq<Int>` exists. `eq` over a list of some
type with no `Eq` instance is rejected, naming the element type.

## Three ways an instance gets chosen

**From an argument.** The usual case: `show(42)` picks `Show<Int>`.

**From the result type.** `mempty` takes no arguments at all, so nothing at the
call site says which instance to use. The type the result is required to have
decides:

```flux
let xs: List<Int> = mempty()   // []
let s: String = mempty()       // ""
```

`pure` is the same shape with an argument that does not help — `pure(1)` is a
`List<Int>` or an `Option<Int>` depending on context. See
[`mempty_result_dispatch.flx`](../../examples/type_classes/mempty_result_dispatch.flx)
and [`return_directed_pure.flx`](../../examples/type_classes/return_directed_pure.flx).

**Through a dictionary.** Inside a constrained function the type is not known
yet, so the method is fetched from the dictionary the caller passed.

## Deriving

For a data declaration, `deriving` writes the instance for you:

```flux
data Colour { Red, Green, Blue } deriving (Eq, Show)
```

Only classes whose methods can actually be generated are accepted — `Eq`,
`Show`, `Json.Encode` and `Json.Decode`. `deriving (Ord)` is **rejected** with
**E486** rather than producing a method that compiles and then fails at run
time. Write the instance by hand instead.

## Effects work through classes

A class method may be effect-polymorphic, so `fmap` can map a function that
performs effects:

```flux
import Flow.Functor

fn shout(x: Int) -> Int with IO {
    print(x)
    x * 10
}

fn main() with IO {
    print(fmap([1, 2], shout))
}
```

The `IO` propagates to the caller — it is not swallowed, and `main` must
declare it. See
[`effectful_fmap.flx`](../../examples/type_classes/effectful_fmap.flx).

## Failure patterns

| Symptom | Cause | Fix |
|---|---|---|
| `E444 No instance for Eq<Foo>` | no instance for that type | write one, or add `deriving (Eq)` |
| `E445 Missing Superclass Instance` | an `Ord` instance with no `Eq` instance | add the superclass instance |
| `E486` on a `deriving` clause | that class cannot be generated | write the instance by hand |
| `E440 Duplicate Type Class` | re-declaring a class in one module | rename yours |
| `E443 Duplicate Instance` | an instance the prelude already provides | delete yours |
| A function of yours is called instead of a class method | a name bound to a function never dispatches as a class method | rename your function |

That last row is worth remembering: if you define `fn add(...)`, calls to `add`
reach yours, not `Num`'s — silently.

## Current limits

- One superclass per class, and one context constraint per instance.
- `Either` has no `Functor`/`Applicative`/`Monad` instance
  ([KI-064](../known_issues.md#ki-064)). `Eq` and `Ord` over
  `Either` work.
- A constrained function declared **inside a `module` block** does not receive
  its dictionary and fails at run time
  ([KI-061](../known_issues.md#ki-061)). Declare it at the top level.
- `deriving` on a parameterized type is not supported
  ([KI-059](../known_issues.md#ki-059)).

---

## Next

Return to the [Guide index](README.md).
