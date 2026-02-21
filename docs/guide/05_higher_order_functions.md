# Chapter 5 — Higher-Order Functions

> Full examples: [`examples/basics/higher_order_builtins.flx`](../../examples/basics/higher_order_builtins.flx), [`examples/advanced/functional_pipeline.flx`](../../examples/advanced/functional_pipeline.flx)

Higher-order functions take or return functions. Flux ships them as builtins — no imports needed.

## map

Apply a function to every element, returning a new collection of the same size:

```flux
let nums = [|1, 2, 3, 4, 5|]

let doubled  = map(nums, \x -> x * 2)
let as_strs  = map(nums, \x -> to_string(x))

print(doubled)   // [|2, 4, 6, 8, 10|]
print(as_strs)   // [|1, 2, 3, 4, 5|]
```

## filter

Keep only elements that satisfy a predicate:

```flux
let evens = filter(nums, \x -> x % 2 == 0)
let big   = filter(nums, \x -> x > 3)

print(evens)  // [|2, 4|]
print(big)    // [|4, 5|]
```

## fold

Reduce a collection to a single value by accumulating:

```flux
let total   = fold(nums, 0, \(acc, x) -> acc + x)
let product = fold(nums, 1, \(acc, x) -> acc * x)

print(total)    // 15
print(product)  // 120
```

## flat_map

Map then flatten one level — useful for expanding elements:

```flux
let sentences = [|"hello world", "foo bar"|]
let words     = flat_map(sentences, \s -> split(s, " "))

print(words)  // [|hello, world, foo, bar|]
```

## any and all

Short-circuit predicates:

```flux
print(any(nums, \x -> x > 4))   // true  (5 qualifies)
print(any(nums, \x -> x > 10))  // false
print(all(nums, \x -> x > 0))   // true  (all positive)
print(all(nums, \x -> x > 3))   // false (1, 2, 3 fail)
```

## find

Return the first matching element as `Some`, or `None`:

```flux
let words = [|"banana", "fig", "apple", "kiwi"|]

print(find(nums, \x -> x > 3))               // Some(4)
print(find(words, \w -> starts_with(w, "a"))) // Some(apple)
print(find(nums, \x -> x > 100))             // None
```

## sort_by

Stable sort using a key function that returns an Integer, Float, or String:

```flux
// Sort strings by length
print(sort_by(words, \w -> len(w)))
// [|fig, kiwi, apple, banana|]

// Sort descending (negate the key)
print(sort_by(nums, \x -> 0 - x))
// [|5, 4, 3, 2, 1|]

// Sort tuples by second field
let people = [|("Alice", 30), ("Bob", 25), ("Carol", 35)|]
let sorted = sort_by(people, \p -> p.1)
print(map(sorted, \p -> p.0))
// [|Bob, Alice, Carol|]
```

## zip

Pair elements from two collections into an array of tuples. Stops at the shorter one:

```flux
let keys   = [|"a", "b", "c"|]
let values = [|1, 2, 3|]

print(zip(keys, values))
// [|(a, 1), (b, 2), (c, 3)|]

// Pair each word with its length
let lengths = zip(words, map(words, \w -> len(w)))
print(lengths)
// [|(banana, 6), (fig, 3), (apple, 5), (kiwi, 4)|]
```

## flatten

Collapse one level of nesting:

```flux
let nested = [|[|1, 2|], [|3, 4|], [|5|]|]
print(flatten(nested))   // [|1, 2, 3, 4, 5|]
```

## count

Count elements matching a predicate:

```flux
print(count(nums, \x -> x % 2 == 0))           // 2
print(count(words, \w -> len(w) > 4))           // 2  (banana, apple)
```

## Function Composition

Build reusable pipelines from small functions:

```flux
fn compose(f, g) { \x -> f(g(x)) }

let double  = \x -> x * 2
let add_ten = \x -> x + 10
let square  = \x -> x * x

// add_ten(double(square(x))) — for x=5: 25 → 50 → 60
let transform = compose(add_ten, compose(double, square))
print(transform(5))  // 60
```

> See [`examples/advanced/functional_pipeline.flx`](../../examples/advanced/functional_pipeline.flx).

## Next

Continue to [Chapter 6 — Pipe Operator and List Comprehensions](06_pipe_and_comprehensions.md).
