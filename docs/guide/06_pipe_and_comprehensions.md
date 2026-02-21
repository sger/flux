# Chapter 6 — Pipe Operator and List Comprehensions

> Full examples: [`examples/basics/pipe_operator.flx`](../../examples/basics/pipe_operator.flx), [`examples/basics/list_comprehension.flx`](../../examples/basics/list_comprehension.flx), [`examples/advanced/functional_pipeline.flx`](../../examples/advanced/functional_pipeline.flx)

## The Pipe Operator

`a |> f` is sugar for `f(a)`. `a |> f(b)` is sugar for `f(a, b)` — the left value becomes the *first* argument.

Both are desugared to ordinary function calls at parse time; there is no runtime overhead.

```flux
fn double(x) { x * 2 }
fn add(x, y) { x + y }

// Without pipe
print(add(double(5), 3))    // 13

// With pipe
let result = 5 |> double |> add(3)
print(result)               // 13
```

### Chaining

```flux
let result = 2
    |> double       // 4
    |> add(10)      // 14
    |> double       // 28

print(result)  // 28
```

### Pipe with builtin functions

```flux
let greeting = "  HELLO WORLD  "
    |> trim
    |> lower

print(greeting)  // hello world
```

### Pipe in a data pipeline

```flux
let words = [|"hello", "world", "flux"|]

let result = words
    |> filter(\w -> len(w) > 4)
    |> map(\w -> upper(w))
    |> sort

print(result)  // [|HELLO, WORLD|]
```

> The pipe operator desugars to `Expression::Call` — there is no `Pipe` AST node.

## List Comprehensions

Haskell-style syntax for building collections declaratively. Also desugared at parse time to `map`, `filter`, and `flat_map` calls.

```
[expression | variable <- collection, guard, ...]
```

### Single generator

```flux
let nums = [1, 2, 3, 4, 5]

print([x * 2 | x <- nums])
// [|2, 4, 6, 8, 10|]
```

Equivalent to: `map(nums, \x -> x * 2)`

### With a guard

```flux
print([x | x <- nums, x % 2 == 0])
// [|2, 4|]
```

Equivalent to: `map(filter(nums, \x -> x % 2 == 0), \x -> x)`

### Multiple guards

```flux
let r = range(1, 11)
print([x | x <- r, x > 3, x < 8])
// [|4, 5, 6, 7|]
```

### Multiple generators (cartesian product)

```flux
let colors = ["red", "blue"]
let sizes  = ["S", "M", "L"]

print([color + "-" + size | color <- colors, size <- sizes])
// [|red-S, red-M, red-L, blue-S, blue-M, blue-L|]
```

Equivalent to: `flat_map(colors, \color -> map(sizes, \size -> color + "-" + size))`

### Guards across generators

```flux
let small = [1, 2, 3]

// Pairs where sum > 4
print([to_string(x) + "+" + to_string(y) | x <- small, y <- small, x + y > 4])
// [|2+3, 3+2, 3+3|]
```

### Pythagorean triples

```flux
let r = range(1, 11)

let triples = [
    to_string(a) + "," + to_string(b) + "," + to_string(c)
    | a <- r, b <- r, b >= a, c <- r, c >= b, a*a + b*b == c*c
]

print(triples)  // [|3,4,5, 6,8,10|]
```

### Disambiguation: comprehension vs cons cell

Both `[x | xs]` (cons) and `[x * 2 | x <- xs]` (comprehension) use `|`. The parser disambiguates by lookahead: if after `|` there is `Identifier LeftArrow (<-)`, it is a comprehension; otherwise it is a cons cell.

> See [`examples/basics/list_comprehension.flx`](../../examples/basics/list_comprehension.flx).

## Combining Pipe and Comprehensions

```flux
let data = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]

let result = [x * x | x <- data, x % 2 == 0]
    |> filter(\n -> n > 10)
    |> sum

print(result)  // 4^2 + 6^2 + 8^2 + 10^2 = 16+36+64+100 = 216, filtered >10: 216
```

## Next

Continue to [Chapter 7 — Modules](07_modules.md).
