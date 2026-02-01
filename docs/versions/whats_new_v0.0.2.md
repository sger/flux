# What's New in Flux v0.0.2

Flux v0.0.2 focuses on core language completeness and functional programming essentials.

## Highlights

- New operators: `<=`, `>=`, `%`, `&&`, `||`, and pipe `|>`
- Either type for error handling: `Left` / `Right`
- Lambda shorthand: `\x -> expr`
- Expanded builtins: arrays + strings + `to_string`
- Improved runtime errors with structured details and code frames

## Language Features

### Operators

- Comparison: `<=`, `>=`
- Modulo: `%`
- Logical: `&&`, `||` (short-circuiting)
- Pipe: `|>` for left-to-right composition

Example:

```flux
let result = 10 |> \x -> x * 2 |> \x -> x + 1;
print(result); // 21
```

### Either Type

`Left` and `Right` enable explicit error handling and pattern matching.

```flux
let value = Right(42);

match value {
  Left(err) -> print(err);
  Right(v) -> print(v);
}
```

### Lambda Shorthand

```flux
let inc = \x -> x + 1;
print(inc(5));
```

## Builtins

### Array

- `concat(a, b)`
- `reverse(arr)`
- `contains(arr, elem)`
- `slice(arr, start, end)`
- `sort(arr)` or `sort(arr, "asc"/"desc")`

### String

- `split(s, delim)`
- `join(arr, delim)`
- `trim(s)`
- `upper(s)`
- `lower(s)`
- `chars(s)`
- `substring(s, start, end)`

### Core

- `to_string(value)`

## Diagnostics & Tooling

- Runtime errors now show a structured summary, code frame, and hint.
- VM trace (`--trace`), linter, formatter, and bytecode cache remain available.

## Compatibility Notes

- v0.0.2 is additive and preserves existing syntax and module behavior.
- New operators follow conventional precedence (`&&` tighter than `||`, `|>` lowest).

