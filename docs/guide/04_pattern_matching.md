# Chapter 4 — Pattern Matching

> Full examples: [`examples/patterns/`](../../examples/patterns/), [`examples/basics/either_and_option.flx`](../../examples/basics/either_and_option.flx)

## Basic match

`match` tests a value against a sequence of patterns. The first match wins.

```flux
let x = 2

let label = match x {
    0 -> "zero",
    1 -> "one",
    2 -> "two",
    _ -> "other",   // wildcard — matches anything
}

print(label)  // two
```

## Option — Some and None

`Some` and `None` are built-in keywords, not library functions:

```flux
let val = Some(42)

let result = match val {
    Some(x) -> "got: " + to_string(x),
    None    -> "nothing",
}

print(result)  // got: 42
```

Array and hash indexing always return `Option`:

```flux
let arr = [|10, 20, 30|]

match arr[1] {
    Some(v) -> print("found: " + to_string(v)),
    None    -> print("out of bounds"),
}
// found: 20
```

## Either — Left and Right

`Left` and `Right` represent a two-branch result (error / success convention):

```flux
fn safe_divide(a, b) {
    if b == 0 { Left("division by zero") }
    else { Right(a / b) }
}

let result = safe_divide(10, 2)

match result {
    Right(v) -> print("result: " + to_string(v)),
    Left(e)  -> print("error: " + e),
}
// result: 5
```

## Match Guards

Add a condition after the pattern with `if`:

```flux
let score = Some(82)

let band = match score {
    Some(v) if v >= 90 -> "A",
    Some(v) if v >= 80 -> "B",
    Some(v) if v >= 70 -> "C",
    Some(_)            -> "D",
    None               -> "N/A",
}

print(band)  // B
```

Guards work with any pattern:

```flux
let x = 3

match x {
    n if n % 2 == 0 -> print("even"),
    _               -> print("odd"),
}
// odd
```

> See [`examples/patterns/match_guards.flx`](../../examples/patterns/match_guards.flx).

## Cons List Patterns

The `[h | t]` pattern destructures the head and tail of a cons list:

```flux
fn sum(lst) {
    match lst {
        [h | t] -> h + sum(t),
        _       -> 0,
    }
}

print(sum(list(1, 2, 3, 4)))  // 10
```

Reverse a list using an accumulator:

```flux
fn rev(lst, acc) {
    match lst {
        [h | t] -> rev(t, [h | acc]),
        _       -> acc,
    }
}

print(rev(list(1, 2, 3), None))  // [|3, 2, 1|]
```

> See [`examples/patterns/list_patterns.flx`](../../examples/patterns/list_patterns.flx).

## Tuple Patterns

```flux
let point = (1, (2, 3))

match point {
    (a, (b, c)) -> print(a + b + c),   // 6
    _           -> print("no match"),
}
```

## Binding in Patterns

Any part of a pattern can bind a name:

```flux
match Some(42) {
    Some(n) -> print("value is " + to_string(n)),
    _       -> print("none"),
}
```

Wildcard `_` discards without binding:

```flux
match list(1, 2, 3) {
    [_ | t] -> print("has tail"),
    _       -> print("empty"),
}
```

## Next

Continue to [Chapter 5 — Higher-Order Functions](05_higher_order_functions.md).
