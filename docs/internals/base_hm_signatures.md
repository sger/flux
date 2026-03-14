# Base Function HM Signatures (Internal Reference)

> **Proposal:** [0074](../proposals/implemented/0074_base_signature_tightening.md)
> **Source:** `src/runtime/base/helpers.rs` — `signature_for_id`

This document is the canonical reference for the `BaseHmSignature` registry — the declarative type/effect signatures used by the HM inference engine for all 77 built-in base functions.

---

## 1. Purpose

Before HM inference runs (`infer_program`), every base function is installed in the `TypeEnv` via `scheme_for_signature_id`. This gives the HM engine precise type information at builtin call sites, enabling:

- Type errors at builtin call sites (e.g., passing `String` to `first(arr)` → E300).
- Effect row propagation through HOF callbacks (`map`, `filter`, `fold`, etc.).
- Polymorphic type inference for container builtins (`first`, `zip`, `get`, etc.).

Without these signatures, all builtins would infer as `Any → Any`, silently accepting any argument types.

---

## 2. Core Types

File: `src/runtime/base/base_hm_type.rs`

```rust
pub enum BaseHmType {
    Any,
    Int,
    Float,
    Bool,
    String,
    Unit,
    TypeVar(&'static str),          // polymorphic variable: "a", "b", "k", "v"
    Option(Box<BaseHmType>),
    List(Box<BaseHmType>),
    Array(Box<BaseHmType>),
    Map(Box<BaseHmType>, Box<BaseHmType>),
    Either(Box<BaseHmType>, Box<BaseHmType>),
    Tuple(Vec<BaseHmType>),
    Fun { params: Vec<BaseHmType>, ret: Box<BaseHmType>, effects: BaseHmEffectRow },
}
```

File: `src/runtime/base/base_hm_effect_row.rs`

```rust
pub struct BaseHmEffectRow {
    pub concrete: Vec<&'static str>,  // e.g. ["IO"], ["IO", "Time"], []
    pub tail: Option<&'static str>,   // row variable name, e.g. Some("e")
}
```

File: `src/runtime/base/base_hm_signature.rs`

```rust
pub struct BaseHmSignature {
    pub type_params: Vec<&'static str>,  // e.g. ["a", "b"]
    pub row_params:  Vec<&'static str>,  // e.g. ["e"]
    pub params:      Vec<BaseHmType>,
    pub ret:         BaseHmType,
    pub effects:     BaseHmEffectRow,
}
```

---

## 3. Helper Constructors

All defined in `src/runtime/base/helpers.rs`:

| Helper | Produces |
|--------|---------|
| `t_any()` | `Any` |
| `t_int()` | `Int` |
| `t_bool()` | `Bool` |
| `t_string()` | `String` |
| `t_unit()` | `Unit` |
| `t_option(T)` | `Option<T>` |
| `t_array(T)` | `Array<T>` |
| `t_list(T)` | `List<T>` |
| `t_map(K, V)` | `Map<K, V>` |
| `t_tuple(elems)` | `(T1, T2, ...)` |
| `t_var("a")` | `TypeVar("a")` |
| `t_fun(params, ret, effects)` | `fn(...) -> T with e` |
| `row(conc, tail)` | `BaseHmEffectRow` |
| `sig(params, ret, effects)` | monomorphic `BaseHmSignature` |
| `sig_with_row_params(tps, rps, params, ret, effects)` | polymorphic `BaseHmSignature` |

---

## 4. Full Signature Reference

### IO builtins

| Function | Signature |
|----------|-----------|
| `print` | `Any -> Unit with IO` |
| `read_file` | `String -> String with IO` |
| `read_lines` | `String -> Array<String> with IO` |
| `read_stdin` | `() -> String with IO` |
| `now_ms` | `() -> Int with Time` |
| `time` | `() -> Int with Time` |

### String builtins

| Function | Signature |
|----------|-----------|
| `split` | `(String, String) -> Array<String>` |
| `join` | `(Array<String>, String) -> String` |
| `trim` | `String -> String` |
| `upper` | `String -> String` |
| `lower` | `String -> String` |
| `starts_with` | `(String, String) -> Bool` |
| `ends_with` | `(String, String) -> Bool` |
| `replace` | `(String, String, String) -> String` |
| `chars` | `String -> Array<String>` |
| `substring` | `(String, Int, Int) -> String` |
| `to_string` | `Any -> String` |
| `parse_int` | `String -> Option<Int>` |
| `parse_ints` | `Array<String> -> Array<Int>` |
| `split_ints` | `(String, String) -> Array<Int>` |

### Collection builtins (polymorphic)

| Function | Signature |
|----------|-----------|
| `first` | `forall a. Array<a> -> Option<a>` |
| `last` | `forall a. Array<a> -> Option<a>` |
| `rest` | `forall a. Array<a> -> Array<a>` |
| `push` | `forall a. (Array<a>, a) -> Array<a>` |
| `slice` | `forall a. (Array<a>, Int, Int) -> Array<a>` |
| `sort` | `forall a. Array<a> -> Array<a>` |
| `zip` | `forall a b. (Array<a>, Array<b>) -> Array<(a, b)>` |
| `flatten` | `forall a. Array<Array<a>> -> Array<a>` |
| `range` | `(Int, Int) -> Array<Int>` |

### Collection builtins (deferred — require type classes)

| Function | Current signature | Target (post-0053) |
|----------|------------------|--------------------|
| `len` | `Any -> Int` | `forall a. Array<a> -> Int` (overloaded for List, String, Map) |
| `reverse` | `Any -> Any` | `forall a. Array<a> -> Array<a>` |
| `contains` | `(Any, Any) -> Bool` | `forall a. (Array<a>, a) -> Bool` |
| `concat` | `(Any, Any) -> Any` | `forall a. (Array<a>, Array<a>) -> Array<a>` |
| `sum` | `Any -> Any` | `Array<Int> -> Int` (or `Array<Float> -> Float`) |
| `product` | `Any -> Any` | `Array<Int> -> Int` |
| `abs` | `Any -> Any` | `Int -> Int` / `Float -> Float` |
| `min` | `(Any, Any) -> Any` | `forall a. (a, a) -> a` |
| `max` | `(Any, Any) -> Any` | `forall a. (a, a) -> a` |

### Map builtins (polymorphic)

| Function | Signature |
|----------|-----------|
| `keys` | `forall k v. Map<k, v> -> Array<k>` |
| `values` | `forall k v. Map<k, v> -> Array<v>` |
| `has_key` | `forall k v. (Map<k, v>, k) -> Bool` |
| `merge` | `forall k v. (Map<k, v>, Map<k, v>) -> Map<k, v>` |
| `delete` | `forall k v. (Map<k, v>, k) -> Map<k, v>` |
| `put` | `forall k v. (Map<k, v>, k, v) -> Map<k, v>` |
| `get` | `forall k v. (Map<k, v>, k) -> Option<v>` |
| `is_map` | `Any -> Bool` |

### List builtins (polymorphic)

| Function | Signature |
|----------|-----------|
| `hd` | `forall a. List<a> -> Option<a>` |
| `tl` | `forall a. List<a> -> List<a>` |
| `to_list` | `forall a. Array<a> -> List<a>` |
| `to_array` | `forall a. List<a> -> Array<a>` |
| `is_list` | `Any -> Bool` |
| `list` | `Any -> Any` (variadic; deferred) |

### HOF builtins (effect-polymorphic)

| Function | Signature |
|----------|-----------|
| `map` | `forall e. (Any, (Any -> Any with \|e)) -> Any with \|e` |
| `filter` | `forall e. (Any, (Any -> Bool with \|e)) -> Any with \|e` |
| `fold` | `forall e. (Any, Any, (Any, Any -> Any with \|e)) -> Any with \|e` |
| `flat_map` | `forall e. (Any, (Any -> Any with \|e)) -> Any with \|e` |
| `any` | `forall e. (Any, (Any -> Bool with \|e)) -> Bool with \|e` |
| `all` | `forall e. (Any, (Any -> Bool with \|e)) -> Bool with \|e` |
| `find` | `forall e. (Any, (Any -> Bool with \|e)) -> Option<Any> with \|e` |
| `sort_by` | `forall e. (Any, (Any -> Any with \|e)) -> Any with \|e` |
| `count` | `forall e. (Any, (Any -> Bool with \|e)) -> Int with \|e` |
| `assert_throws` | `forall e. (fn() -> Any with \|e) -> Unit with \|e` |

> **Note:** HOF element types remain `Any` pending proposal 0053 (traits). The row-variable `|e` part is already precise.

### Type predicate and assertion builtins

| Function | Signature |
|----------|-----------|
| `type_of` | `Any -> String` |
| `is_int` | `Any -> Bool` |
| `is_float` | `Any -> Bool` |
| `is_string` | `Any -> Bool` |
| `is_bool` | `Any -> Bool` |
| `is_array` | `Any -> Bool` |
| `is_hash` | `Any -> Bool` |
| `is_none` | `Any -> Bool` |
| `is_some` | `Any -> Bool` |
| `assert_eq` | `(Any, Any) -> Unit` |
| `assert_neq` | `(Any, Any) -> Unit` |
| `assert_true` | `Bool -> Unit` |
| `assert_false` | `Bool -> Unit` |

---

## 5. Lowering to `Scheme`

`scheme_for_signature_id(id, interner)` lowers a `BaseHmSignature` to a `Scheme`:

1. Allocates fresh `TypeVarId`s for `type_params` entries (shared across all uses of the same name within the signature).
2. Allocates fresh `TypeVarId`s for `row_params` entries (stored as `InferEffectRow::tail`).
3. Calls `lower_type` recursively to convert `BaseHmType` → `InferType`.
4. Calls `lower_effect_row` to convert `BaseHmEffectRow` → `InferEffectRow`.
5. Wraps in `Scheme { forall: all_allocated_vars, ty }`.

The resulting `Scheme` is installed in `TypeEnv` under the function's interned name. At each call site, `instantiate(scheme)` creates fresh type variables, enabling independent polymorphic usage across call sites.

---

## 6. Adding or Updating a Signature

1. Add/modify the arm in `signature_for_id` in `src/runtime/base/helpers.rs`.
2. Use `sig_with_row_params` for polymorphic signatures; `sig` for monomorphic.
3. Add helper constructors (`t_*`) if a new type form is needed.
4. Run `cargo test --test type_inference_tests --test base_functions_tests` to verify.
5. Update this document's signature table.
6. If the change is a tightening (replacing `Any` with a concrete type), check that existing fixtures in `examples/type_system/` still pass, and add a failing fixture if a new error class is reachable.
