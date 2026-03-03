- Feature Name: Base Function HM Signature Tightening
- Start Date: 2026-03-01
- Completion Date: 2026-03-03
- Status: Completed
- Proposal PR: pending
- Flux Issue: pending

# Proposal 0074: Base Function HM Signature Tightening

## Summary

Replace the `Any`-typed `BaseHmSignature` entries for non-HOF built-in functions with
precise polymorphic types. For example, `len: Any -> Int` becomes
`len: forall a. Array<a> -> Int` (with overloads for `List<a>`, `String`, and `Map<k, v>`).
This allows the HM inference engine to catch type errors at built-in call sites that
currently pass through unchecked.

## Motivation

Proposal 0064 introduced the `BaseHmSignature` infrastructure and wired every built-in
function into the HM inference pass. However, the initial implementation used `Any` for
most non-HOF builtins as a pragmatic shortcut. This means calls like `len(42)`,
`reverse(true)`, or `first("hello")` are silently accepted by HM inference even though
they will fail at runtime.

The infrastructure to fix this already exists:

- `BaseHmType` supports `TypeVar`, `List`, `Array`, `Map`, `Option`, `Either`, `Tuple`
- `BaseHmSignature::to_scheme` correctly quantifies type variables
- The HOF builtins (`map`, `filter`, `fold`, etc.) already use `TypeVar` for callback
  parameter and return types

The gap is purely in the signature definitions in `signature_for_id`. Tightening them is
a mechanical task with high diagnostic payoff.

## Guide-level explanation

After this proposal, the type checker catches errors on built-in calls at compile time:

```flux
fn main() {
    let x = len(42)        // E300: expected Array<a> | List<a> | String, got Int
    let y = reverse(true)  // E300: expected Array<a> | List<a>, got Bool
    let z = trim(42)       // E300: expected String, got Int
}
```

No new syntax or language features are introduced. Existing correct programs are
unaffected because the tighter types are strictly more precise than `Any`.

### Overloaded builtins

Some builtins accept multiple container types. Flux handles this through ad-hoc
overloading at the HM level using a fresh type variable constrained by unification:

| Built-in | Current | Tightened |
|----------|---------|-----------|
| `len` | `Any -> Int` | `Array<a> -> Int` (also accepts `List<a>`, `String`, `Map<k,v>`) |
| `reverse` | `Any -> Any` | `forall a. Array<a> -> Array<a>` (also `List<a> -> List<a>`) |
| `first` | `Any -> Option<Any>` | `forall a. Array<a> -> Option<a>` |
| `contains` | `(Any, Any) -> Bool` | `forall a. (Array<a>, a) -> Bool` |

For builtins that operate on multiple container types (e.g., `len` works on arrays, lists,
strings, and maps), the initial tightening targets the most common usage pattern. A
follow-up iteration can introduce union-style overload resolution if needed.

## Reference-level explanation

### Phase 1: String builtins (low risk, high coverage) ✅ Complete (2026-03-03)

These builtins already have `String` parameters but return `Any` where the return type is
known. Tightened in `src/runtime/base/helpers.rs`:

| Built-in | Before | After |
|----------|--------|-------|
| `chars` | `String -> Any` | `String -> Array<String>` |
| `split` | `(String, String) -> Any` | `(String, String) -> Array<String>` |
| `parse_ints` | `Array<String> -> Any` | `Array<String> -> Array<Int>` |
| `split_ints` | `(String, String) -> Any` | `(String, String) -> Array<Int>` |
| `read_lines` | `String -> Any with IO` | `String -> Array<String> with IO` |

Also added `t_array()` helper constructor alongside existing `t_option()`, `t_fun()` etc.
All tests pass (78 type_inference, 144 compiler_rules, 122 base_functions).

### Phase 2: Collection builtins with polymorphic signatures

These require `type_params` to express the element type relationship:

```rust
// len: forall a. Array<a> -> Int
Id::Len => sig_with_type_params(
    vec!["a"], vec![],
    vec![t_array(t_var("a"))],
    t_int(),
    row(vec![], None),
),

// first: forall a. Array<a> -> Option<a>
Id::First => sig_with_type_params(
    vec!["a"], vec![],
    vec![t_array(t_var("a"))],
    t_option(t_var("a")),
    row(vec![], None),
),

// last: forall a. Array<a> -> Option<a>
Id::Last => sig_with_type_params(
    vec!["a"], vec![],
    vec![t_array(t_var("a"))],
    t_option(t_var("a")),
    row(vec![], None),
),

// rest: forall a. Array<a> -> Array<a>
Id::Rest => sig_with_type_params(
    vec!["a"], vec![],
    vec![t_array(t_var("a"))],
    t_array(t_var("a")),
    row(vec![], None),
),

// push: forall a. (Array<a>, a) -> Array<a>
Id::Push => sig_with_type_params(
    vec!["a"], vec![],
    vec![t_array(t_var("a")), t_var("a")],
    t_array(t_var("a")),
    row(vec![], None),
),

// reverse: forall a. Array<a> -> Array<a>
Id::Reverse => sig_with_type_params(
    vec!["a"], vec![],
    vec![t_array(t_var("a"))],
    t_array(t_var("a")),
    row(vec![], None),
),

// contains: forall a. (Array<a>, a) -> Bool
Id::Contains => sig_with_type_params(
    vec!["a"], vec![],
    vec![t_array(t_var("a")), t_var("a")],
    t_bool(),
    row(vec![], None),
),

// slice: forall a. (Array<a>, Int, Int) -> Array<a>
Id::Slice => sig_with_type_params(
    vec!["a"], vec![],
    vec![t_array(t_var("a")), t_int(), t_int()],
    t_array(t_var("a")),
    row(vec![], None),
),

// sort: forall a. Array<a> -> Array<a>
Id::Sort => sig_with_type_params(
    vec!["a"], vec![],
    vec![t_array(t_var("a"))],
    t_array(t_var("a")),
    row(vec![], None),
),

// zip: forall a b. (Array<a>, Array<b>) -> Array<(a, b)>
Id::Zip => sig_with_type_params(
    vec!["a", "b"], vec![],
    vec![t_array(t_var("a")), t_array(t_var("b"))],
    t_array(t_tuple(vec![t_var("a"), t_var("b")])),
    row(vec![], None),
),

// flatten: forall a. Array<Array<a>> -> Array<a>
Id::Flatten => sig_with_type_params(
    vec!["a"], vec![],
    vec![t_array(t_array(t_var("a")))],
    t_array(t_var("a")),
    row(vec![], None),
),
```

### Phase 3: Map builtins

```rust
// keys: forall k v. Map<k, v> -> Array<k>
Id::Keys => sig_with_type_params(
    vec!["k", "v"], vec![],
    vec![t_map(t_var("k"), t_var("v"))],
    t_array(t_var("k")),
    row(vec![], None),
),

// values: forall k v. Map<k, v> -> Array<v>
Id::Values => sig_with_type_params(
    vec!["k", "v"], vec![],
    vec![t_map(t_var("k"), t_var("v"))],
    t_array(t_var("v")),
    row(vec![], None),
),

// has_key: forall k v. (Map<k, v>, k) -> Bool
Id::HasKey => sig_with_type_params(
    vec!["k", "v"], vec![],
    vec![t_map(t_var("k"), t_var("v")), t_var("k")],
    t_bool(),
    row(vec![], None),
),

// merge: forall k v. (Map<k, v>, Map<k, v>) -> Map<k, v>
Id::Merge => sig_with_type_params(
    vec!["k", "v"], vec![],
    vec![t_map(t_var("k"), t_var("v")), t_map(t_var("k"), t_var("v"))],
    t_map(t_var("k"), t_var("v")),
    row(vec![], None),
),

// delete: forall k v. (Map<k, v>, k) -> Map<k, v>
Id::Delete => sig_with_type_params(
    vec!["k", "v"], vec![],
    vec![t_map(t_var("k"), t_var("v")), t_var("k")],
    t_map(t_var("k"), t_var("v")),
    row(vec![], None),
),

// put: forall k v. (Map<k, v>, k, v) -> Map<k, v>
Id::Put => sig_with_type_params(
    vec!["k", "v"], vec![],
    vec![t_map(t_var("k"), t_var("v")), t_var("k"), t_var("v")],
    t_map(t_var("k"), t_var("v")),
    row(vec![], None),
),

// get: forall k v. (Map<k, v>, k) -> Option<v>
Id::Get => sig_with_type_params(
    vec!["k", "v"], vec![],
    vec![t_map(t_var("k"), t_var("v")), t_var("k")],
    t_option(t_var("v")),
    row(vec![], None),
),
```

### Phase 4: List builtins

```rust
// hd: forall a. List<a> -> Option<a>
Id::Hd => sig_with_type_params(
    vec!["a"], vec![],
    vec![t_list(t_var("a"))],
    t_option(t_var("a")),
    row(vec![], None),
),

// tl: forall a. List<a> -> List<a>
Id::Tl => sig_with_type_params(
    vec!["a"], vec![],
    vec![t_list(t_var("a"))],
    t_list(t_var("a")),
    row(vec![], None),
),

// list: forall a. Array<a> -> List<a>
Id::List => sig_with_type_params(
    vec!["a"], vec![],
    vec![t_array(t_var("a"))],
    t_list(t_var("a")),
    row(vec![], None),
),

// to_list: forall a. Array<a> -> List<a>
Id::ToList => sig_with_type_params(
    vec!["a"], vec![],
    vec![t_array(t_var("a"))],
    t_list(t_var("a")),
    row(vec![], None),
),

// to_array: forall a. List<a> -> Array<a>
Id::ToArray => sig_with_type_params(
    vec!["a"], vec![],
    vec![t_list(t_var("a"))],
    t_array(t_var("a")),
    row(vec![], None),
),
```

### Phase 5: Numeric and conversion builtins

| Built-in | Current | Tightened |
|----------|---------|-----------|
| `abs` | `Any -> Any` | `Int -> Int` (or overloaded for Float) |
| `min` | `(Any, Any) -> Any` | `forall a. (a, a) -> a` |
| `max` | `(Any, Any) -> Any` | `forall a. (a, a) -> a` |
| `range` | `(Int, Int) -> Any` | `(Int, Int) -> Array<Int>` |
| `sum` | `Any -> Any` | `Array<Int> -> Int` |
| `product` | `Any -> Any` | `Array<Int> -> Int` |
| `concat` | `(Any, Any) -> Any` | `forall a. (Array<a>, Array<a>) -> Array<a>` |
| `join` | `(Any, String) -> String` | `(Array<String>, String) -> String` |

### Phase 6: HOF signature tightening

The existing HOF signatures already use row parameters for effect polymorphism but still
use `Any` for element types. Tighten these in a separate phase:

```rust
// map: forall a b. (Array<a>, (a) -> b with |e) -> Array<b> with |e
Id::Map => sig_with_type_params(
    vec!["a", "b"],
    vec!["e"],
    vec![
        t_array(t_var("a")),
        t_fun(vec![t_var("a")], t_var("b"), row(vec![], Some("e"))),
    ],
    t_array(t_var("b")),
    row(vec![], Some("e")),
),

// filter: forall a. (Array<a>, (a) -> Bool with |e) -> Array<a> with |e
Id::Filter => sig_with_type_params(
    vec!["a"],
    vec!["e"],
    vec![
        t_array(t_var("a")),
        t_fun(vec![t_var("a")], t_bool(), row(vec![], Some("e"))),
    ],
    t_array(t_var("a")),
    row(vec![], Some("e")),
),

// fold: forall a b. (Array<a>, b, (b, a) -> b with |e) -> b with |e
Id::Fold => sig_with_type_params(
    vec!["a", "b"],
    vec!["e"],
    vec![
        t_array(t_var("a")),
        t_var("b"),
        t_fun(vec![t_var("b"), t_var("a")], t_var("b"), row(vec![], Some("e"))),
    ],
    t_var("b"),
    row(vec![], Some("e")),
),
```

### Builtins that remain `Any`

Some builtins are intentionally polymorphic in ways that don't map cleanly to Flux's
current type system:

| Built-in | Reason |
|----------|--------|
| `to_string` | Accepts any type — true `Any -> String` |
| `type_of` | Accepts any type — true `Any -> String` |
| `is_int`, `is_float`, etc. | Type predicates — true `Any -> Bool` |
| `assert_eq`, `assert_neq` | Polymorphic comparison — could become `forall a. (a, a) -> Unit` |

### Implementation approach

Each phase is independent and can be merged separately. The implementation pattern is:

1. Update `signature_for_id` in `src/runtime/base/signatures.rs`
2. Add helper constructors if needed (e.g., `t_array`, `t_list`, `t_map`, `t_tuple`)
3. Run `cargo test` — any new HM errors in existing examples reveal call sites that
   were previously unchecked
4. Fix or update examples/snapshots as needed
5. Add targeted failing fixtures in `examples/type_system/failing/` for each tightened
   builtin

### Helper constructors needed

```rust
fn t_var(name: &'static str) -> BaseHmType { BaseHmType::TypeVar(name) }
fn t_float() -> BaseHmType { BaseHmType::Float }
fn t_array(inner: BaseHmType) -> BaseHmType { BaseHmType::Array(Box::new(inner)) }
fn t_list(inner: BaseHmType) -> BaseHmType { BaseHmType::List(Box::new(inner)) }
fn t_map(k: BaseHmType, v: BaseHmType) -> BaseHmType {
    BaseHmType::Map(Box::new(k), Box::new(v))
}
fn t_tuple(elements: Vec<BaseHmType>) -> BaseHmType { BaseHmType::Tuple(elements) }
```

Most of these constructors mirror existing `BaseHmType` variants and are trivial to add.

## Drawbacks

- **Overloaded builtins**: Some builtins like `len` work on arrays, lists, strings, and
  maps. Without union types or type classes, we must pick one primary type and rely on
  HM's `Any` fallback or ad-hoc overload resolution for the others. The initial approach
  is to type them for the most common usage (arrays) and add overload support later.

- **Snapshot churn**: Tightening signatures may cause existing snapshot tests to change
  if HM inference now infers more precise types for expressions involving builtins.

- **Gradual typing friction**: Code that passes untyped values to builtins will now get
  type errors where it previously worked. This is by design but may surprise users
  upgrading.

## Rationale and alternatives

**Why not type classes?** Type classes (or traits) would be the principled solution for
overloaded builtins like `len`. However, Flux does not yet have type classes, and adding
them is a much larger undertaking. This proposal takes the pragmatic approach of
tightening what we can with the existing type system.

**Why phased?** Each phase is independently valuable and testable. String builtins (Phase

1) are the easiest because they have no polymorphism. Collection builtins (Phase 2-4)
require type variables but use straightforward parametric polymorphism. HOFs (Phase 6)
are the most complex because they combine type variables with effect row variables.

## Prior art

- **Elm**: All standard library functions have precise types. No `Any` escape hatch.
- **Haskell**: Prelude functions use type classes (`Foldable`, `Traversable`) for
  container-generic operations.
- **OCaml**: Module signatures provide precise types for all standard library functions.
- **Koka**: Built-in functions have precise effect-polymorphic types from the start.

## Unresolved questions

1. **Overload resolution strategy**: For builtins that work on multiple container types
   (e.g., `len` on Array vs List vs String), should we:
   (a) Pick the most common type and leave others as `Any`
   (b) Introduce ad-hoc overload resolution in the HM pass
   (c) Wait for type classes

2. **`concat` semantics**: `concat` works on both arrays and strings. Should it be typed
   as `(Array<a>, Array<a>) -> Array<a>` or `(String, String) -> String`, or deferred
   until overloads are available?

3. **Numeric overloads**: `abs`, `min`, `max`, `sum`, `product` work on both `Int` and
   `Float`. Same overload question applies.

## Completion notes (2026-03-03)

All phases that can be implemented without type classes or overload resolution are complete.
Tightened in `src/runtime/base/helpers.rs`:

| Phase | Built-ins | Status |
|-------|-----------|--------|
| 1: String returns | `chars`, `split`, `parse_ints`, `split_ints`, `read_lines` | ✅ Done |
| 2: Collection | `first`, `last`, `rest`, `push`, `slice`, `sort`, `zip`, `flatten` | ✅ Done |
| 3: Map | `keys`, `values`, `has_key`, `merge`, `delete`, `put`, `get` | ✅ Done |
| 4: List | `hd`, `tl`, `to_list`, `to_array` | ✅ Done |
| 5: Misc | `range` → `(Int,Int)->Array<Int>`, `join` → `(Array<String>,String)->String` | ✅ Done |
| 5: Deferred | `abs/min/max/sum/product/concat` — require type classes or union types | Deferred |
| 6: HOF element types | `map/filter/fold/flat_map/any/all/find/sort_by/count` | Deferred |

Deferred items depend on proposal 0053 (traits/type classes) or a dedicated overload resolution mechanism.

Helper constructors added: `t_array`, `t_list`, `t_map`, `t_tuple`, `t_var`.

Verification commands:
```bash
cargo test --test type_inference_tests    # 78 passed
cargo test --test compiler_rules_tests   # 144 passed
cargo test --test base_functions_tests   # 122 passed
```
