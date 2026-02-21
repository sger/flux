# Chapter 3 — Collections

> Full examples: [`examples/basics/`](../../examples/basics/)

Flux has three collection types with distinct trade-offs: **arrays** (indexed, `Rc`-backed), **cons lists** (linked, GC-managed), and **hash maps** (persistent HAMT). **Tuples** group heterogeneous values.

## Arrays

Arrays use `[| |]` delimiters to distinguish them from cons lists.

```flux
let scores = [|10, 20, 30, 40|]

print(len(scores))           // 4
print(first(scores))         // 10
print(last(scores))          // 40
print(scores[1])             // Some(20)  — indexing returns Option
print(scores[99])            // None
```

Arrays are `Rc`-backed — operations return new arrays rather than mutating:

```flux
let extended = push(scores, 50)
print(extended)              // [|10, 20, 30, 40, 50|]
print(scores)                // [|10, 20, 30, 40|]  — unchanged
```

Common array builtins:

```flux
let nums = [|3, 1, 4, 1, 5, 9|]

print(reverse(nums))                       // [|9, 5, 1, 4, 1, 3|]
print(sort(nums))                          // [|1, 1, 3, 4, 5, 9|]
print(slice(nums, 1, 4))                   // [|1, 4, 1|]
print(contains(nums, 4))                   // true
print(concat([|1, 2|], [|3, 4|]))          // [|1, 2, 3, 4|]
print(sum(nums))                           // 23
print(range(1, 5))                         // [|1, 2, 3, 4|]
```

> See [`examples/basics/arrays_basic.flx`](../../examples/basics/arrays_basic.flx) and [`examples/basics/array_builtins.flx`](../../examples/basics/array_builtins.flx).

## Cons Lists

Cons lists are immutable linked lists managed by the GC with O(1) prepend. The empty list is `None`.

```flux
let xs = list(1, 2, 3, 4)    // build from arguments
let ys = [1 | [2 | [3 | None]]]  // explicit cons syntax

print(xs)          // [|1, 2, 3, 4|] (displayed as array-like)
print(hd(xs))      // 1
print(tl(xs))      // [|2, 3, 4|]
```

Prepend with cons (`|`):

```flux
let bigger = [0 | xs]         // [|0, 1, 2, 3, 4|]
```

Convert between arrays and lists:

```flux
let arr  = [|1, 2, 3|]
let lst  = to_list(arr)       // cons list
let back = to_array(lst)      // array again
```

Recursive processing via pattern matching (see also [Chapter 4](04_pattern_matching.md)):

```flux
fn sum(lst) {
    match lst {
        [h | t] -> h + sum(t),
        _       -> 0,
    }
}

print(sum(list(1, 2, 3, 4)))  // 10
```

> See [`examples/basics/list_basic.flx`](../../examples/basics/list_basic.flx).

## Hash Maps

Hash maps are persistent HAMT structures — `put` returns a new map without mutating the original.

```flux
let user = {"name": "Alice", "age": 30}

print(get(user, "name"))           // Some(Alice)
print(get(user, "missing"))        // None

let updated = put(user, "email", "alice@example.com")
print(get(updated, "email"))       // Some(alice@example.com)
print(get(user, "email"))          // None  — original unchanged
```

Common hash builtins:

```flux
print(keys(user))                  // [|name, age|]
print(values(user))                // [|Alice, 30|]
print(has_key(user, "name"))       // true
print(delete(user, "age"))         // {"name": "Alice"}
print(merge(user, {"city": "NY"})) // merged map
```

Square-bracket indexing also returns `Option`:

```flux
let h = {"a": 1, "b": 2}
print(h["a"])   // Some(1)
print(h["z"])   // None
```

> See [`examples/basics/hash_basic.flx`](../../examples/basics/hash_basic.flx) and [`examples/basics/hash_builtins.flx`](../../examples/basics/hash_builtins.flx).

## Tuples

Tuples group a fixed number of values of any type:

```flux
let point  = (3, 4)
let triple = (1, "hello", true)
let single = (42,)
let unit   = ()
```

Access by index:

```flux
print(point.0)   // 3
print(point.1)   // 4
```

Destructure in `let`:

```flux
let (x, y) = point
print(x)   // 3
print(y)   // 4
```

Destructure in function parameters:

```flux
fn swap((a, b)) { (b, a) }
print(swap((1, 2)))   // (2, 1)
```

> See [`examples/basics/tuples.flx`](../../examples/basics/tuples.flx).

## Next

Continue to [Chapter 4 — Pattern Matching](04_pattern_matching.md).
