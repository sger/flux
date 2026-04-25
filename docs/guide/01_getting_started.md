# Chapter 1 — Getting Started

> Full examples: [`examples/guide/`](../../examples/guide/)

## Hello, Flux

```flux
print("Hello, Flux!")
```

Run it:

```bash
cargo run -- examples/guide/print.flx
```

## Variables

All bindings are immutable. There is no `var`, `mut`, or reassignment.

```flux
let x = 5
let name = "Flux"
let flag = true
let ratio = 3.14

print(x)      // 5
print(name)   // Flux
```

To "update" a value, introduce a new binding:

```flux
let score = 10
let next_score = score + 2
print(next_score)   // 12
```

Semicolons are optional at the top level. Both styles work:

```flux
let a = 1;   // with semicolon
let b = 2    // without
```

> See [`examples/guide/variables.flx`](../../examples/guide/variables.flx) and [`examples/guide/semicolons.flx`](../../examples/guide/semicolons.flx).

## Primitive Types

| Type      | Example              |
|-----------|----------------------|
| Integer   | `42`, `-7`           |
| Float     | `3.14`, `-0.5`       |
| Boolean   | `true`, `false`      |
| String    | `"hello"`            |
| None      | `None`               |

> See [`examples/guide/arithmetic.flx`](../../examples/guide/arithmetic.flx) and [`examples/guide/float.flx`](../../examples/guide/float.flx).

## Arithmetic and Comparison

```flux
print(2 + 3)    // 5
print(10 - 4)   // 6
print(3 * 4)    // 12
print(10 / 3)   // 3  (integer division)
print(10 % 3)   // 1
print(2.0 / 3)  // 0.666...

print(1 == 1)   // true
print(1 != 2)   // true
print(3 > 2)    // true
print(3 >= 3)   // true
```

> See [`examples/guide/comparison.flx`](../../examples/guide/comparison.flx).

## Strings

```flux
let s = "hello"
print(len(s))                     // 5
print(upper(s))                   // HELLO
print(s + " world")               // hello world  (+ concatenates strings)
```

### String Interpolation

Embed any expression inside `#{ }`:

```flux
let name = "Flux"
let score = 99
print("Language: #{name}")                      // Language: Flux
print("Score: #{score}, doubled: #{score * 2}") // Score: 99, doubled: 198
print("2 + 2 = #{2 + 2}")                       // 2 + 2 = 4
```

> See [`examples/guide/strings.flx`](../../examples/guide/strings.flx) and [`examples/guide/string_interpolation.flx`](../../examples/guide/string_interpolation.flx).

## Conditionals

```flux
let x = 10

if x > 5 {
    print("big")
} else {
    print("small")
}

// else if chains
fn grade(score) {
    if score >= 90 { "A" }
    else if score >= 80 { "B" }
    else if score >= 70 { "C" }
    else { "F" }
}

print(grade(85))  // B
```

`if` is an expression — it returns a value:

```flux
let label = if x % 2 == 0 { "even" } else { "odd" }
print(label)
```

> See [`examples/guide/if_else.flx`](../../examples/guide/if_else.flx).

## Next

Continue to [Chapter 2 — Functions and Closures](02_functions_and_closures.md).
