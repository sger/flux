- Feature Name: `hash_class_for_map_keys`
- Start Date: 2026-11-02
- Proposal PR:
- Flux Issue:
- Status: Draft
- Depends on: [0145](implemented/0145_type_classes.md) (type classes)
- Relates to: KI-157 (map keys and ordering operands that stop at run time)

# Proposal 0194: A `Hash` class for map keys

## Summary
[summary]: #summary

Only an `Int`, a `Bool` or a `String` can key a map, on both backends, but
the types accept any key: `Map.set({}, (1, 2), "a")` type-checks and then
stops at run time with E1004. This proposal adds a class, `Hash`, that names
the types that can key a map, and puts `k: Hash` on `Flow.Map`'s functions,
on map literals and on `m[key]`, so such a program is rejected when it is
compiled. Tuples of hashable types are hashable, user types can derive it,
and `Float` is left out on purpose.

## Motivation
[motivation]: #motivation

`Map.set<k, v>` (`lib/Flow/Map.flx`) puts no constraint on `k`, and a map
literal is typed with none either
(`infer_hash_literal_expression`, `src/ast/type_infer/expression/collections.rs`).
Each backend keys its map by a small set of kinds:

- The VM's `HashKey` (`src/runtime/hash_key.rs`) is an `Int`, a `Bool` or a
  `String`; any other key stops the run with E1004 (`primop put expects
  hashable key (String, Int, Bool), got Tuple`) or, in a literal or an index,
  E1009 (`unusable as hash key`).
- The native map (`runtime/c/hamt.c`) hashed any other boxed key by its
  address and compared it as a string, so `(1, 3)` found the entry for
  `(1, 2)` and `Green` replaced `Red` (KI-157). Since 2026-11-02 it refuses
  such a key with the VM's errors, so both backends now agree, but only at
  run time.

So a program the types accept fails the first time it runs a line, and
nothing in a signature tells the reader which types may key a map. No
`.flx` file in the repository uses another kind of key, so a constraint
costs no existing program anything.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

A map's keys must be hashable:

```flux
import Flow.Map as Map

fn main() with IO {
    let ages = Map.set({}, "ada", 36)        // String: hashable
    let seen = Map.set({}, (3, 4), true)     // a tuple of Ints: hashable
    print(Map.get(ages, "ada"))
    print(Map.get(seen, (3, 4)))
}
```

`Int`, `Bool` and `String` are hashable, and so is a tuple whose parts are.
A type of your own can derive it:

```flux
data Color { Red, Green, Blue } deriving (Eq, Hash)

let names = {Red: "red", Green: "green"}
```

`Float` is not hashable. A Float computed two ways rarely compares equal,
`0.0 == -0.0` holds for two different bit patterns, and `NaN` equals
nothing, so a Float key is a map entry that may never be found again:

```text
error[E0xx]: `Float` cannot key a map
  ages.flx:4:24
  |
4 |     let m = Map.set({}, half(3.0), "a")
  |                         ^^^^^^^^^ `Float` has no `Hash` instance
  = help: key the map by a rounded `Int`, or by the number's text
```

A generic function that builds a map says so in its signature:

```flux
fn count<a: Hash>(xs: List<a>) -> Map<a, Int> { ... }
```

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### The class

`Hash` is declared in Flux, in a new `lib/Flow/Hash.flx`, beside `Eq`
(`lib/Flow/Eq.flx`), with `Eq` as its superclass, since a map compares the
keys that hash alike:

```flux
public class Eq<a> => Hash<a> { }
public instance Eq<Int> => Hash<Int> { }
public instance Eq<Bool> => Hash<Bool> { }
public instance Eq<String> => Hash<String> { }
```

It has **no method** in this proposal: it says which types can key a map,
and both runtimes keep hashing those keys themselves. That is what makes it
cheap, and it avoids a hazard found while writing this: `Ord` on a tuple
type-checks through the solver's structural rule but its dictionary cannot be
built, so `fn bigger<a: Ord>(x: a, y: a) -> Bool` called with two tuples
stops the compiler with E000 on both backends (to be filed as a known issue).
A class without methods needs no dictionary at run time, as `Sendable` shows.

### Tuples, `Option`, `List`

The solver's built-in structural rule (`structural_builtin_evidence`,
`src/types/class_solver.rs`) decomposes `Eq`, `Ord` and `Sendable` over
tuples, `Option`, `List`, `Array` and `Either`. `Hash` joins that list for
**tuples only**: the runtimes can key a map by an `Int`, a `Bool`, a
`String`, and, after this proposal, a tuple of those. `Option`, `List` and
user types become keys only through `deriving`, below, once the runtimes
hash them.

### The runtimes

Both runtimes learn to hash and compare a tuple key by its parts:

- VM: a `HashKey::Tuple(Vec<HashKey>)` variant, built by `to_hash_key` when
  every part converts.
- Native: `hamt_hash` mixes the hashes of a tuple's fields, and `keys_equal`
  uses the existing structural `flux_rt_eq` (`runtime/c/flux_rt.c`) for two
  tuples. `flux_hamt_key_ok` accepts a tuple whose fields it accepts.

The run-time refusals stay as a backstop for code the types cannot see, such
as a key that crossed an untyped boundary.

### `deriving (Hash)`

A `data` type with `deriving (Hash)` gets the instance when each field's
type is hashable, as `deriving (Eq)` already requires of `Eq`. The runtimes
key such a value by its constructor and fields: a `HashKey::Adt` variant in
the VM, and the constructor tag mixed with the fields' hashes natively.
Nullary constructors, now static objects natively, hash by their tag.

### Where the constraint is checked

- `Flow.Map`'s functions become `get<k: Hash, v>`, `set<k: Hash, v>` and so
  on. These are `intrinsic fn`s, and none has had a constraint before, so the
  lowering of an intrinsic must drop the (empty) evidence argument rather than
  pass it to the primop.
- A map literal adds `Hash<k>` for its key type in
  `infer_hash_literal_expression`.
- `m[key]` on a map adds `Hash<k>` where indexing is typed.

### The error

A new compile-time error, raised where an unsolved `Hash<Float>` (or any type
without the instance) meets a map, names the key's type and the map
operation, and suggests a key of another type.

## Drawbacks
[drawbacks]: #drawbacks

- A new class, a new derivable class, a new error, and changes in the solver,
  two runtimes and three type-checking sites, for programs nobody has written
  yet.
- A class without methods cannot let a user decide how a type is hashed; a
  type that wants a key other than its structure must be mapped to one first.
- `Float` keys, which Elm, F#, Swift and OCaml allow, are refused.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- **Hash by structure in C, accept any key (OCaml's `Hashtbl`).** Keeps the
  types as they are. But the VM would need a key kind for every value kind,
  a hand-written `Eq` would be silently ignored by the map, and an unusable
  key would still be found only at run time.
- **A class with a `hash` method that the map calls.** Lets users choose the
  hash. It needs a dictionary passed into every map primop, or a map written
  in Flux, and it meets the tuple dictionary crash above. It can be added
  later without breaking this design (see Future possibilities).
- **A built-in constraint like F#'s `comparison` or Elm's `comparable`.**
  The same check, but not extensible by `deriving` and not visible as a class
  in `lib/`.
- **Do nothing.** Both backends now agree (since the KI-157 refusal), but
  only when the program runs.

## Prior art
[prior-art]: #prior-art

| Language | A map's key must be | Tuples, user types | Float keys |
|---|---|---|---|
| Haskell | `Ord k` (`Data.Map`), or `Eq k, Hashable k` (`Data.HashMap`) | instances from the parts; `deriving` | allowed, but GHC warns against it |
| Rust | `Eq + Hash` (`HashMap`), or `Ord` (`BTreeMap`) | `#[derive(Hash, Eq)]` | refused: `f64` is neither `Eq` nor `Hash` |
| Swift | `Hashable` | synthesized for structs and enums; tuples cannot | allowed |
| Lean 4 | `BEq` and `Hashable` | `deriving BEq, Hashable` | allowed |
| F# | the built-in `comparison` constraint | tuples, records and unions, structurally | allowed |
| Elm | the built-in `comparable` constraint | tuples and lists of comparables; custom types cannot | allowed |
| OCaml | anything (`Hashtbl`), or a `compare` (`Map.Make`) | any value, by its representation | allowed |
| Erlang, Elixir | any term | any term | allowed |

GHC's own documentation of `Ord Double`
(`libraries/ghc-internal/src/GHC/Internal/Classes.hs`) says: "one should
avoid ordered containers with keys represented by 'Double', because data
loss and corruption may happen." Rust makes the same choice in its types.
Flux follows Rust: the VM already refuses Float keys, so no program changes.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- Can a class declared in Flux have no methods (`{ }`), or must `Hash` be
  built into the compiler, as `Sendable` is (`src/types/class_env.rs`)?
- Should `<` on tuples become lexicographic through `Ord`, or stay refused?
  Today both backends stop with E1009, and `Ord` on tuples crashes the
  compiler.
- How does a constraint on an `intrinsic fn` interact with KI-076 (operators
  on constrained types inside a `module` block)?
- The new error's code and wording.

## Future possibilities
[future-possibilities]: #future-possibilities

- A `hash` method, once dictionaries reach the map primops, so a type can
  choose its own key.
- `Option` and `List` keys, by the structural rule, once the runtimes hash
  them.
- An ordered map keyed by `Ord`, alongside the hash map.
