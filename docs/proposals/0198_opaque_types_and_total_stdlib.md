- Feature Name: `opaque_types_and_total_stdlib`
- Start Date: 2026-11-03
- Proposal PR:
- Flux Issue:
- Status: Draft
- Umbrella: [0195](0195_checked_guarantees.md) (track T3)
- Depends on: [0151](implemented/0151_module_scoped_type_classes.md) (`data` visibility), [0197](0197_partiality_as_effects.md) (`Exn`, for the stdlib audit)
- Relates to: [0135](0135_total_functions_and_safe_arithmetic.md) (Phase 2 `NonZero`, Phase 4 refinement types)

# Proposal 0198: Opaque types and a total standard library

## Summary
[summary]: #summary

An invariant such as "this integer is not zero" or "this list is not empty" can
be carried by a type whose constructor only its module can use: a smart
constructor checks the invariant once, and every function that takes the type
can rely on it. Flux has the pieces, but the boundary leaks and the type cannot
be named outside its module. This proposal:

1. Closes the leak: `exposing (..)` brings in a private constructor.
2. Adds an **opaque** export: the type is public, its constructors are not.
3. Adds `Flow.NonEmpty`, and audits the stdlib so that no function crashes or
   returns a made-up value on input its type accepts.

No refinement types and no solver: 0135 Phase 4 stays future work.

## Motivation
[motivation]: #motivation

### `NonZero` can be forged

`lib/Flow/NonZero.flx` declares `data NonZero { NonZero(Int) }` without `public`.
0151 says such a type's constructors "are invisible outside the module".
Checked 2026-11-03 at b21d7f89:

| Program | Result |
|---|---|
| `import Flow.NonZero` then `NonZero.NonZero(0)` | E004, undefined |
| `import Flow.NonZero exposing (NonZero)` | E420, private member in exposing |
| `import Flow.NonZero exposing (..)` then `div_nz(10, NonZero(0))` | **compiles**, then E1008 inside `lib/Flow/NonZero.flx:28` |

The glob form skips the visibility check that the explicit form applies. The
file's header says `div_nz` "cannot fail because `b` is proven non-zero at the
type level", and `tests/flux/non_zero.flx` itself imports with `exposing (..)`.

### An opaque type cannot be named

0151 makes `public data` atomic: the type and all its constructors, or neither
(`implemented/0151_module_scoped_type_classes.md:479`). Its advice for an opaque
type is to keep the whole `data` private. Then an importer cannot write
`NonZero` in an annotation, so `fn ratio(a: Int, b: NonZero) -> Int` is not
expressible outside the module, and `NonZero.flx`'s own public functions leave
their parameters unannotated. Nothing checks that a `public fn` mentions a
private type. Only classes have that check (E451, E455).

### The stdlib has partial functions and a wrong answer

| Function | On bad input |
|---|---|
| `Array.maximum`, `Array.minimum` (`lib/Flow/Array.flx:220,240`) | return `0` for `[]`: a wrong value, no error |
| `List.first`, `List.last` (`lib/Flow/List.flx:234,242`) | untyped; return the element on a non-empty list, `None` on an empty one |
| `List.fold1`, `List.init` (`List.flx:59,272`) | `panic` on `[]` |
| `Option.unwrap`, `unwrap_result`, `unwrap_err_result` | `panic` |
| `Json.parse` (`lib/Flow/Json.flx:48`) | `panic` (`parse_result` returns a result) |
| `Async.first_of`, `Event.choose` | `panic` on `[]` |
| `parse_int` (builtin) | runtime error on bad input |

`List.maximum` and `List.minimum` already return `Option`, so the List and
Array modules disagree on the same name.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

```flux
module Flow.NonZero {
    public opaque data NonZero { NonZero(Int) }

    public fn non_zero(x: Int) -> Option<NonZero> {
        if x == 0 { None } else { Some(NonZero(x)) }
    }

    public fn div_nz(a: Int, b: NonZero) -> Int {
        match b { NonZero(d) -> a / d }        // total: d != 0 by construction
    }
}
```

Outside the module, `NonZero` is a type you can name, store and pass, but not
build or take apart:

```flux
import Flow.NonZero exposing (..)

fn ratio(a: Int, b: NonZero) -> Int { div_nz(a, b) }   // ok

fn forge() { NonZero(0) }   // error: `NonZero` is an opaque type of Flow.NonZero
```

`Flow.NonEmpty` uses the same mechanism:

```flux
import Flow.NonEmpty as NE

fn biggest(xs) {
    match NE.from_list(xs) {
        Some(ne) -> NE.maximum(ne),   // total: never empty
        None     -> 0,
    }
}
```

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Step 1: close the glob leak (a bug fix, first)

- `exposing (..)` must expose exactly the public members that an explicit list
  could name. Find where the glob is expanded in module resolution and apply
  the same visibility filter that produces E420.
- Regression fixture with a `_ki` suffix (file a known issue first):
  `exposing (..)` then a private constructor gives E004 or E420.
- Audit the stdlib's own tests for code that relied on the leak.

### Step 2: `public opaque data`

- Parser: `opaque` after `public` in a `data` declaration
  (`src/syntax/parser/statement.rs:122`, `Statement::Data.is_public` at
  `src/syntax/statement.rs:141` becomes a three-state visibility).
- Module interface (`src/compiler/module_interface.rs:187-215`): export the type
  name and its parameters, no constructors. The type stays nominal, so two
  opaque types over `Int` are distinct.
- Outside the module, constructing, matching and naming a constructor of an
  opaque type is an error. A new code, since E004 ("undefined variable") would be
  misleading: the type is visible. `deriving` instances declared in the module
  (Eq, Show) are exported.
- Inside the module, an opaque type behaves exactly like a private one.

### Step 3: private types in public signatures

- A `public fn` whose inferred or declared signature mentions a module-private
  type is an error, like E451 for classes. The fix the error suggests is
  `public opaque data`.
- `NonZero.flx` and `Async.flx`'s handles (0199) are the first users.

### Step 4: `Flow.NonEmpty`

- `public opaque data NonEmpty<a> { NonEmpty(a, List<a>) }`, with `from_list`,
  `singleton`, `cons`, `head`, `last`, `to_list`, `map`, `fold1`, `maximum` and
  `minimum` (with `a: Ord`). Each is total.
- Array variant, or a `NonEmptyArray` later: an open question.

### Step 5: stdlib audit

For each function in the table above, pick one of:

- **Return `Option`** (or a `Result`) where absence is ordinary:
  `Array.maximum`/`minimum` (matching List), `List.first`/`last`, given a type.
- **Take a `NonEmpty`** where a non-empty input is the precondition:
  `fold1`, `first_of`, `choose`. The old name stays as a deprecated wrapper.
- **Keep the crash and say so with `Exn`** (0197) where a crash is the point:
  `unwrap`, `Json.parse`.

The Array changes alter results for existing programs. Each gets a line in
`CHANGELOG.md`.

### Tests

- Compiler-error fixtures (`//~`): the glob leak, construction and matching of
  an opaque constructor, a private type in a public signature.
- `tests/flux/non_empty.flx` with `flux --test`, and a parity fixture.
- `tests/type_inference/non_zero_type_tests.rs` today only runs a parity fixture on
  two backends. Add a typing test: `NonZero` named in an importer's annotation
  type-checks, and `NonZero(0)` from an importer is rejected.

## Drawbacks
[drawbacks]: #drawbacks

- 0151 refused partial exports to avoid the "can I match on what I can see?"
  question. `opaque` answers it (no) at the cost of one more visibility form.
- Changing `Array.maximum` to `Option` breaks callers that relied on `0`.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- **Refinement types** (`{Int | b != 0}`, 0135 Phase 4). Deferred: they need a
  decision procedure and new syntax. An opaque type plus a smart constructor
  checks the invariant once, at a run-time cost of one comparison.
- **Haskell-style `T(..)` / `T` export lists.** Rejected: Flux has no export lists,
  and 0151 chose per-declaration visibility.
- **Newtype erasure.** Out of scope: `NonZero` stays a boxed constructor until
  the specialisation work (0187) can unbox single-field types.

## Prior art
[prior-art]: #prior-art

- OCaml abstract types in signatures; Elm's opaque types (module exports the type
  without `(..)`); Rust's private fields; Haskell `Data.List.NonEmpty` and
  smart-constructor modules.
- Koka's `abstract` types export the type and hide its constructors, the same
  split as `public opaque data`.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- Keyword: `opaque`, or `public type T` with no constructors listed?
- Does a `NonEmptyArray` justify its own module, or does `NonEmpty` take a
  container parameter?
- Where should `Exn`-raising wrappers live after the audit, the same module or a
  `Flow.Partial` module that has to be imported on purpose?

## Future possibilities
[future-possibilities]: #future-possibilities

- `match` guards that refine (`n if n != 0 -> div_nz(a, unsafe_non_zero(n))`)
  without the `Option` round trip, checked by the compiler.
- 0135 Phase 4 refinement types, built on top of opaque types as the escape
  hatch for what the solver cannot prove.
