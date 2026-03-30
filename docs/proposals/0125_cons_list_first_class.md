- Feature Name: Cons Lists as First-Class Default Collection
- Start Date: 2026-03-26
- Proposal PR:
- Flux Issue:
- Depends on: Proposal 0120 (Flow Standard Library)

## Summary

Make cons lists the primary collection type in Flux. `map`, `filter`, `fold`, and all higher-order functions in `Flow.List` operate on cons lists via `[h | t]` pattern matching instead of array indexing. Arrays become an explicit performance-oriented type with their own `Flow.Array` module. This aligns Flux with Haskell, Erlang, and ML-family languages where linked lists are the default and arrays are opt-in.

## Motivation

### The current split

Flux has two list-like types:

| Syntax | Type | Backing | Access |
|--------|------|---------|--------|
| `[1, 2, 3]` | Cons list | `Rc<ConsCell>` linked list | O(1) head/tail, O(n) index |
| `[|1, 2, 3|]` | Array | `Rc<Vec<Value>>` contiguous | O(1) index, O(n) prepend |

The `Flow.List` module (`map`, `filter`, `fold`, etc.) currently uses array indexing internally:

```flux
public fn map(arr, f) {
    fn map_go(i, result) {
        if i >= len(arr) { result }
        else {
            match arr[i] {
                Some(v) -> map_go(i + 1, push(result, f(v))),
                _ -> map_go(i + 1, result)
            }
        }
    }
    map_go(0, [||])
}
```

This means `map([1, 2, 3], f)` silently fails or produces wrong results when given a cons list, because cons lists don't support `arr[i]` indexing. The LLVM native backend surfaces this as empty output; the VM may produce runtime errors.

### Why this matters

1. **Syntax mismatch**: `[1, 2, 3]` creates a cons list, but `map`/`filter`/`fold` only work on arrays. Users must write `[|1, 2, 3|]` or call `to_array` to use HOFs — surprising for a functional language.

2. **Backend parity**: The VM and native backend handle the type confusion differently, causing silent divergence.

3. **Language identity**: Flux is a pure functional language with Hindley-Milner type inference, algebraic effects, and ADTs. Cons lists with pattern matching are the natural fit. Haskell, Erlang, OCaml, and Lean all default to linked lists.

### The Haskell approach

In Haskell, `[1, 2, 3]` is always `1 : 2 : 3 : []`. There is no ambiguity about which `map` to call — `map` works on lists. Arrays (`Data.Vector`, `Data.Array`) are a separate import with separate functions (`Vector.map`).

Flux should follow this model:
- `[1, 2, 3]` is a cons list (already the case)
- `map`, `filter`, `fold` work on cons lists
- `[|1, 2, 3|]` is an array — use `Array.map` for array-specific operations

## Design

### Phase 1: Rewrite Flow.List to cons list recursion

Rewrite all HOFs in `lib/Flow/List.flx` to use `[h | t]` pattern matching instead of index-based iteration:

```flux
// Before (array-based):
public fn map(arr, f) {
    fn map_go(i, result) {
        if i >= len(arr) { result }
        else {
            match arr[i] {
                Some(v) -> map_go(i + 1, push(result, f(v))),
                _ -> map_go(i + 1, result)
            }
        }
    }
    map_go(0, [||])
}

// After (cons list):
public fn map(xs, f) {
    match xs {
        [h | t] -> [f(h) | map(t, f)],
        _ -> []
    }
}
```

Functions to rewrite:

| Function | Current | After |
|----------|---------|-------|
| `map(xs, f)` | index loop | `[f(h) \| map(t, f)]` |
| `filter(xs, f)` | index loop | recursive cons |
| `fold(xs, acc, f)` | index loop | `fold(t, f(acc, h), f)` |
| `any(xs, f)` | index loop | `f(h) \|\| any(t, f)` |
| `all(xs, f)` | index loop | `f(h) && all(t, f)` |
| `find(xs, f)` | index loop | recursive match |
| `count(xs, f)` | index loop | recursive count |
| `each(xs, f)` | index loop | `f(h); each(t, f)` |
| `flat_map(xs, f)` | index loop | concat + recurse |
| `flatten(xs)` | index loop | concat + recurse |
| `reverse(xs)` | index loop | accumulator recursion |
| `zip(xs, ys)` | index loop | parallel destructure |
| `range(lo, hi)` | index loop | cons build |
| `sum(xs)` | inlined index loop | `fold(xs, 0, \(a, x) -> a + x)` |
| `product(xs)` | inlined index loop | `fold(xs, 1, \(a, x) -> a * x)` |
| `contains(xs, x)` | `any` wrapper | unchanged (delegates to `any`) |
| `sort_by(xs, f)` | index-based quicksort | cons list quicksort |

### Phase 1b: Add missing GHC Data.List functions

Expand `Flow.List` with the standard list utility functions found in Haskell's `Data.List`, adapted to Flux conventions (collection-first argument order, `[h | t]` pattern matching). All implementations are pure cons list recursion.

**Slicing:**

| Function | Signature | GHC equivalent |
|----------|-----------|----------------|
| `take(xs, n)` | `(List a, Int) -> List a` | `take` |
| `drop(xs, n)` | `(List a, Int) -> List a` | `drop` |
| `take_while(xs, pred)` | `(List a, a -> Bool) -> List a` | `takeWhile` |
| `drop_while(xs, pred)` | `(List a, a -> Bool) -> List a` | `dropWhile` |
| `split_at(xs, n)` | `(List a, Int) -> (List a, List a)` | `splitAt` |
| `span(xs, pred)` | `(List a, a -> Bool) -> (List a, List a)` | `span` |

**Fold & Scan variants:**

| Function | Signature | GHC equivalent |
|----------|-----------|----------------|
| `foldr(xs, acc, f)` | `(List a, b, (a, b) -> b) -> b` | `foldr` |
| `fold1(xs, f)` | `(List a, (a, a) -> a) -> a` | `foldl1` |
| `scanl(xs, acc, f)` | `(List a, b, (b, a) -> b) -> List b` | `scanl` |
| `scanr(xs, acc, f)` | `(List a, b, (a, b) -> b) -> List b` | `scanr` |

**Zip variants:**

| Function | Signature | GHC equivalent |
|----------|-----------|----------------|
| `zip_with(xs, ys, f)` | `(List a, List b, (a, b) -> c) -> List c` | `zipWith` |
| `unzip(xs)` | `List (a, b) -> (List a, List b)` | `unzip` |
| `enumerate(xs)` | `List a -> List (Int, a)` | `zip [0..]` |

**Set-like operations:**

| Function | Signature | GHC equivalent |
|----------|-----------|----------------|
| `nub(xs)` | `List a -> List a` | `nub` (remove duplicates) |
| `partition(xs, pred)` | `(List a, a -> Bool) -> (List a, List a)` | `partition` |
| `delete(xs, x)` | `(List a, a) -> List a` | `delete` |
| `intersperse(xs, sep)` | `(List a, a) -> List a` | `intersperse` |
| `intercalate(xss, sep)` | `(List (List a), List a) -> List a` | `intercalate` |

**Prefix/suffix:**

| Function | Signature | GHC equivalent |
|----------|-----------|----------------|
| `is_prefix(prefix, xs)` | `(List a, List a) -> Bool` | `isPrefixOf` |
| `is_suffix(suffix, xs)` | `(List a, List a) -> Bool` | `isSuffixOf` |

**Utility:**

| Function | Signature | GHC equivalent |
|----------|-----------|----------------|
| `length(xs)` | `List a -> Int` | `length` |
| `null(xs)` | `List a -> Bool` | `null` |
| `init(xs)` | `List a -> List a` | `init` (all but last) |
| `nth(xs, n)` | `(List a, Int) -> Option a` | `(!?)` (safe index) |
| `replicate(n, x)` | `(Int, a) -> List a` | `replicate` |
| `iterate(x, f, n)` | `(a, a -> a, Int) -> List a` | `take n (iterate f x)` |
| `unfold(seed, f)` | `(b, b -> Option (a, b)) -> List a` | `unfoldr` |
| `concat(xss)` | `List (List a) -> List a` | `concat` |
| `sort(xs)` | `List a -> List a` | `sort` (merge sort, uses `<=`) |
| `group_by(xs, eq)` | `(List a, (a, a) -> Bool) -> List (List a)` | `groupBy` |
| `unique_by(xs, f)` | `(List a, a -> b) -> List a` | `nubBy` on key |
| `maximum(xs)` | `List a -> Option a` | `maximum` (safe) |
| `minimum(xs)` | `List a -> Option a` | `minimum` (safe) |

**Differences from GHC:**
- Flux uses collection-first argument order: `take(xs, n)` not `take n xs`
- Safe by default: `first`, `last`, `nth`, `maximum`, `minimum` return `Option`, not partial
- `iterate` is bounded (no laziness): `iterate(x, f, n)` produces `n` elements
- `unfold` replaces Haskell's `unfoldr` — same semantics, Flux naming
- `sort` uses `<=` operator directly (no `Ord` typeclass yet)

### Phase 2: Create Flow.Array module

New module `lib/Flow/Array.flx` with array-specific HOFs using index iteration:

```flux
module Flow.Array {
    public fn map(arr, f) { ... }      // index-based
    public fn filter(arr, f) { ... }   // index-based
    public fn fold(arr, acc, f) { ... } // index-based
    public fn sort(arr) { ... }        // primop
    public fn sort_by(arr, f) { ... }  // index-based quicksort
}
```

### Phase 2b: Add GHC Data.Array-inspired functions to Flow.Array (COMPLETED)

Construction, update, and utility functions inspired by GHC's `Data.Array` and `Data.Vector`, adapted to Flux's 0-indexed arrays.

**Implemented functions (11 total):**

| Function | Signature | GHC equivalent | Status |
|----------|-----------|----------------|--------|
| `take(arr, n)` | `(Array a, Int) -> Array a` | `take` | Done |
| `drop(arr, n)` | `(Array a, Int) -> Array a` | `drop` | Done |
| `update(arr, i, val)` | `(Array a, Int, a) -> Array a` | `(//)` single | Done |
| `swap(arr, i, j)` | `(Array a, Int, Int) -> Array a` | no direct equivalent | Done |
| `enumerate(arr)` | `Array a -> Array (Int, a)` | `zip [0..] (elems arr)` | Done |
| `tabulate(n, f)` | `(Int, Int -> a) -> Array a` | `genArray` / `generate` | Done |
| `from_list(xs)` | `List a -> Array a` | `listArray` | Done (delegates to `to_array` primop) |
| `find_index(arr, pred)` | `(Array a, (a) -> Bool) -> Option Int` | `findIndex` | Done |
| `each_indexed(arr, f)` | `(Array a, (Int, a) -> ()) -> () with IO` | `itraverse_` | Done |
| `update_many(arr, pairs)` | `(Array a, List (Int, a)) -> Array a` | `(//)` | Done |
| `accum(size, init, pairs, f)` | `(Int, a, List (Int, a), (a, a) -> a) -> Array a` | `accumArray` | Done |

```flux
import Flow.Array as Array

let a = [|10, 20, 30, 40, 50|]
Array.take(a, 3)                    // [|10, 20, 30|]
Array.drop(a, 2)                    // [|30, 40, 50|]
Array.update(a, 1, 99)              // [|10, 99, 30, 40, 50|]
Array.swap(a, 0, 4)                 // [|50, 20, 30, 40, 10|]
Array.enumerate(a)                  // [|(0, 10), (1, 20), (2, 30), (3, 40), (4, 50)|]
Array.tabulate(5, \i -> i * i)      // [|0, 1, 4, 9, 16|]
Array.from_list([1, 2, 3])          // [|1, 2, 3|]
Array.find_index(a, \x -> x > 25)  // Some(2)
Array.update_many(a, [(0, 100), (3, 400)])  // [|100, 20, 30, 400, 50|]
Array.accum(5, 0, [(1, 3), (1, 5), (3, 2)], \(a, b) -> a + b)  // [|0, 8, 0, 2, 0|]
```

**Differences from GHC `Data.Array`:**
- Flux arrays are always 0-indexed (no arbitrary `Ix` bounds)
- No mutable variants (`STArray`/`IOArray`) — Flux is pure, mutation via functional update
- `update` returns a new array (structural sharing via Rc when possible)
- No lazy elements — Flux arrays are strict

Usage:

```flux
import Flow.Array as Array

let fast = Array.map([|1, 2, 3|], \x -> x * 2)   // O(n), contiguous
let idiomatic = map([1, 2, 3], \x -> x * 2)       // O(n), cons list
```

### Phase 3: Documentation and boundaries (COMPLETED)

- `Array.to_list(arr)` — convert array to cons list (implemented in Flow.Array)
- `Array.from_list(xs)` — convert cons list to array (delegates to `to_array` primop)
- Round-trip: `Array.from_list(Array.to_list([|1,2,3|]))` → `[|1, 2, 3|]`

**Performance tradeoff guide:**

| Use case | Use cons list `[1, 2, 3]` | Use array `[|1, 2, 3|]` |
|----------|--------------------------|-------------------------|
| Recursive processing | Yes — `[h \| t]` pattern matching | No |
| Random access `xs[i]` | No — O(n) | Yes — O(1) |
| Prepend `[x \| xs]` | Yes — O(1) | No — O(n) |
| Append `push(arr, x)` | No — O(n) | Yes — O(1) amortized |
| Map/filter/fold | Both O(n) | Both O(n) |
| Functional update | No built-in | Yes — `Array.update(arr, i, v)` |
| Grid/matrix operations | No | Yes |
| Sorting | `sort_by(xs, f)` merge sort | `Array.sort(arr)` primop |

## Impact

### What changes for users

- `map([1, 2, 3], f)` works correctly (currently broken)
- `map([|1, 2, 3|], f)` still works — `[|...|]` arrays auto-convert via head/tail, or users can use `Array.map`
- No change to syntax
- No change to type system

### What changes for the compiler

- `Flow.List` functions become tail-recursive cons list traversals (Aether dup/drop still applies)
- Aether reuse tokens can optimize `[f(h) | map(t, f)]` to reuse the cons cell
- LLVM native backend works correctly because cons lists use `hd`/`tl` primops, not array indexing

### Performance

| Operation | Cons list | Array |
|-----------|-----------|-------|
| `map(xs, f)` | O(n) | O(n) via `Array.map` |
| `filter(xs, f)` | O(n) | O(n) via `Array.filter` |
| `fold(xs, acc, f)` | O(n) | O(n) via `Array.fold` |
| `xs[i]` | O(n) | O(1) |
| `[x \| xs]` prepend | O(1) | O(n) |
| `reverse(xs)` | O(n) | O(n) |

For most functional programs, cons list traversal is sufficient. Programs that need random access or bulk mutation should use arrays explicitly.

### Phase 4: Primop cleanup — align with GHC's model (COMPLETED)

GHC has **zero list primops**. Lists are a plain ADT (`data [] a = [] | a : [a]`) — `head`, `tail`, `map`, `filter`, `fold`, `length`, `reverse`, `sort`, `concat` are all regular Haskell functions. The only "magic" is that `(:)` and `[]` are wired-in constructors (the compiler knows their tag layout for pattern matching).

GHC has **84 array primops**, but they are all low-level memory operations: `newArray#`, `readArray#`, `writeArray#`, `indexArray#`, `sizeofArray#`, `copyArray#`, `cloneArray#`, `freezeArray#`, `thawArray#`, `casArray#`. There are no `sort`, `map`, `filter`, or `push` primops — those are all library functions.

Flux now follows this model: primops are the low-level memory/hardware primitives; everything else is a library function in `Flow.List` or `Flow.Array`.

**Status:** `Hd`, `Tl`, and `ArraySort` primops were already removed in earlier work. The `Len` primop was narrowed to reject cons lists with an error message directing users to `Flow.List.length(xs)`. A `length` function was added to `Flow.List` as the replacement.

#### Primops removed (demoted to library functions) — DONE

| Primop | Status | Replacement |
|--------|--------|-------------|
| `Hd` | Already removed (no CorePrimOp variant exists) | `[h \| t]` pattern match + `Flow.List.first(xs)` |
| `Tl` | Already removed (no CorePrimOp variant exists) | `[h \| t]` pattern match + `Flow.List.rest(xs)` |
| `ArraySort` | Already removed (no CorePrimOp variant exists) | `Flow.Array.sort(arr)` / `Flow.Array.sort_by(arr, f)` |
| `Len` on cons lists | Removed (now returns error) | `Flow.List.length(xs)` (added in Phase 4) |

**Note:** `[h | t]` pattern matching is unaffected — it uses `OpConsHead`/`OpConsTail` bytecode opcodes that destructure the `ConsCell` struct directly, not the removed primops.

#### Primops kept (true primitives) — verified in codebase

| Primop | GHC equivalent | Why it stays | Verified |
|--------|---------------|--------------|----------|
| `MakeList` | `(:)` constructor | Allocates cons cell — this IS the constructor | `CorePrimOp::MakeList` |
| `MakeArray` | `newArray#` | Allocates contiguous memory | `CorePrimOp::MakeArray` |
| `ArrayGet` | `indexArray#` | Direct memory read at offset | `CorePrimOp::ArrayGet` |
| `ArraySet` | `writeArray#` | Direct memory write at offset | `CorePrimOp::ArraySet` |
| `ArrayLen` | `sizeofArray#` | Reads cached size field | `CorePrimOp::ArrayLen` |
| `ArrayPush` | — (no GHC equivalent) | Reallocation + copy (C runtime) | `CorePrimOp::ArrayPush` |
| `ArrayConcat` | — (uses `copyArray#`) | Allocation + bulk copy | `CorePrimOp::ArrayConcat` |
| `ArraySlice` | — (uses `cloneArray#`) | Allocation + partial copy | `CorePrimOp::ArraySlice` |
| `ToList` | — | Cross-representation conversion (Vec → ConsCell) | `CorePrimOp::ToList` |
| `ToArray` | — | Cross-representation conversion (ConsCell → Vec) | `CorePrimOp::ToArray` |
| `Len` | — | Polymorphic O(1) length (string/array/tuple/map only) | `CorePrimOp::Len` |

#### Len primop: dispatch narrowed — DONE

`Len` now dispatches on 4 O(1) types only. Cons lists return an error directing users to `Flow.List.length(xs)`.

| Type | Status | Complexity |
|------|--------|-----------|
| String | Kept | O(1) — len field |
| Array | Kept | O(1) — Vec::len |
| Tuple | Kept | O(1) — Vec::len |
| Map (HAMT) | Kept | O(1) — node count |
| None/EmptyList | Kept | O(1) — returns 0 |
| Cons list | **Removed** | Was O(n) — use `Flow.List.length(xs)` |

#### Primops to consider adding (future)

| New primop | GHC equivalent | Why |
|-----------|---------------|-----|
| `ArrayNew(n, init)` | `newArray# n init` | Allocate array of size n with default value — would make `tabulate` and `accum` faster |
| `ArrayCopy(src, srcOff, dst, dstOff, len)` | `copyArray#` | Bulk copy for efficient `update_many` |
| `ArrayUpdate(arr, i, val)` | — | Single-element functional update (copy + write) — more efficient than slice+concat rebuild |

#### Summary: primop count change

| Category | Before | After |
|----------|--------|-------|
| List primops | 4 (`Hd`, `Tl`, `ToList`, `ToArray`) | 2 (`ToList`, `ToArray`) |
| Array primops | 7 (`ArrayGet`..`ArraySort`) | 6 (remove `ArraySort`) |
| Polymorphic | 1 (`Len` — 6 types) | 1 (`Len` — 4 types, O(1) only) |
| **Total removed** | | **3 primops** |

## Alternatives Considered

### Unify on arrays (Option 2)

Make `[1, 2, 3]` produce an array and support `[h | t]` pattern matching via `slice`. This eliminates the type distinction but makes `[h | t]` deconstruction O(n) instead of O(1) and loses the natural recursive structure that makes Aether reuse effective.

### Runtime dispatch (Option 3)

Make `map`/`filter`/`fold` check `is_list`/`is_array` at runtime and dispatch to the appropriate implementation. This works but adds overhead, hides the type distinction, and makes performance unpredictable.

## Migration

### Breaking changes

None. Cons list syntax `[1, 2, 3]` is already the default. The change is internal — Flow.List HOFs switch from array indexing to cons list recursion.

### Compatibility

- Programs using `[|...|]` arrays with `map`/`filter` will need to switch to `Array.map`/`Array.filter` after Phase 2, or convert to cons lists
- Programs already using cons lists will work correctly for the first time

## Decisions

1. **`map` on an array → error.** No auto-conversion. Passing an array to `map` produces a clear error: `map expects a list, got Array — use Array.map or to_list`. Explicit is better than hiding O(n) conversion costs.
2. **`Flow.Array` requires explicit import.** Not auto-imported in the prelude. The default path is cons lists — reaching for arrays is a conscious performance choice via `import Flow.Array as Array`.
3. **Cons list `sort` uses merge sort.** Stable, O(n log n), needs no random access, splits/merges are natural with `[h | t]`. This is what Haskell's `Data.List.sort` uses.

## Prior Art: GHC Comparison

### GHC list architecture

GHC treats lists as a plain ADT with zero primops:

```
data [] a = [] | a : [a]          -- just two constructors, wired-in
```

All list operations — `map`, `filter`, `foldr`, `foldl'`, `head`, `tail`, `length`, `reverse`, `sort`, `concat`, `take`, `drop`, `zip`, `scanl`, `nub`, `group`, `partition`, `intersperse`, `isPrefixOf` — are regular Haskell functions defined in `GHC.Internal.Base` and `GHC.Internal.List` (~2000 lines total). Performance comes from **build/foldr fusion** rewrite rules, not from primops.

### GHC array architecture

GHC has 84 array primops, but they are exclusively low-level memory operations:

| GHC Primop | Purpose |
|-----------|---------|
| `newArray# n init` | Allocate boxed array |
| `readArray# arr i` | Read element (mutable) |
| `writeArray# arr i e` | Write element (mutable) |
| `indexArray# arr i` | Read element (immutable) |
| `sizeofArray# arr` | Get size |
| `copyArray# src soff dst doff n` | Bulk copy |
| `cloneArray# arr off n` | Clone subrange |
| `freezeArray# marr off n` | Mutable → immutable |
| `thawArray# arr off n` | Immutable → mutable |
| `casArray# marr i old new` | Compare-and-swap |

Plus `SmallArray#` (optimized for <128 elements) and `ByteArray#` (unboxed bytes) variants. There are **no** `sort`, `map`, `filter`, `push`, `slice`, or `concat` array primops. All of those are library functions in `Data.Array` and `Data.Vector`.

### Mapping to Flux

| GHC layer | Flux equivalent | Status |
|-----------|----------------|--------|
| `(:)` / `[]` constructors | `MakeList` / `CoreTag::Cons` / `CoreTag::Nil` | Done |
| `GHC.Internal.List` (80+ functions) | `Flow.List` (17 → 50 functions after Phase 1b) | Phase 1b adds missing |
| `newArray#` / `indexArray#` / `writeArray#` / `sizeofArray#` | `MakeArray` / `ArrayGet` / `ArraySet` / `ArrayLen` | Done |
| `Data.Array` / `Data.Vector` (library) | `Flow.Array` (19 → 30 functions after Phase 2b) | Phase 2b adds missing |
| `build`/`foldr` fusion rules | — | Future work (not in this proposal) |
| `SmallArray#` / `ByteArray#` variants | — | Not needed (single array representation) |
| `STArray` / `IOArray` (mutable) | — | Not needed (Aether RC + functional update) |
