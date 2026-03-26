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

Usage:

```flux
import Flow.Array as Array

let fast = Array.map([|1, 2, 3|], \x -> x * 2)   // O(n), contiguous
let idiomatic = map([1, 2, 3], \x -> x * 2)       // O(n), cons list
```

### Phase 3: Documentation and boundaries

- `to_list(arr)` — convert array to cons list
- `to_array(xs)` — convert cons list to array
- Document the performance tradeoff: cons lists for recursion/pattern matching, arrays for random access/bulk operations
- Update examples to use the appropriate type

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
