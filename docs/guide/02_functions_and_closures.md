# Chapter 2 — Functions and Closures

> Full examples: [`examples/basics/`](../../examples/basics/), [`examples/functions/`](../../examples/functions/)

## Named Functions

```flux
fn add(x, y) { x + y }
fn double(x) { x * 2 }

print(add(3, 4))    // 7
print(double(5))    // 10
```

The last expression in the body is the return value. `return` is available but rarely needed.

Functions may reference each other regardless of declaration order (forward references are allowed within a file or module):

```flux
fn isEven(n) {
    if n == 0 { true } else { isOdd(n - 1) }
}

fn isOdd(n) {
    if n == 0 { false } else { isEven(n - 1) }
}

print(isEven(4))  // true
print(isOdd(3))   // true
```

> See [`examples/functions/forward_reference.flx`](../../examples/functions/forward_reference.flx).

## Lambdas

Anonymous functions use the `\` syntax:

```flux
let double = \x -> x * 2
let add    = \(x, y) -> x + y
let greet  = \() -> "hello"

print(double(5))    // 10
print(add(3, 4))    // 7
print(greet())      // hello
```

Multi-statement bodies use a block:

```flux
let compute = \x -> {
    let doubled = x * 2
    doubled + 1
}

print(compute(5))  // 11
```

Lambdas as arguments:

```flux
fn applyTwice(f, x) { f(f(x)) }

print(applyTwice(\x -> x * 2, 3))  // 12
```

> See [`examples/basics/lambda.flx`](../../examples/basics/lambda.flx).

## Closures

Functions capture variables from their enclosing scope:

```flux
fn make_adder(n) {
    \x -> x + n   // captures n
}

let add10 = make_adder(10)
print(add10(5))   // 15
print(add10(20))  // 30
```

```flux
fn make_counter(start) {
    let step = 1
    \() -> start + step   // captures both start and step
}

let counter = make_counter(100)
print(counter())  // 101
```

> See [`examples/functions/closure.flx`](../../examples/functions/closure.flx).

## Do-Blocks

`do { ... }` sequences expressions; the last one is the value:

```flux
let result = do {
    let x = 10
    let y = x * 2
    y + 5
}
print(result)  // 25
```

Useful for side effects in sequence:

```flux
do {
    print("step 1")
    print("step 2")
    print("step 3")
}
```

> See [`examples/basics/do_block.flx`](../../examples/basics/do_block.flx).

## Where Clauses

`where` introduces local bindings after an expression, making the main expression read first:

```flux
fn circle_area(r) {
    pi * r * r
    where pi = 3.14159
}

print(circle_area(5))  // 78.53975
```

Clauses chain and are evaluated in order (later ones can reference earlier ones):

```flux
fn hypotenuse(a, b) {
    result
    where a2 = a * a
    where b2 = b * b
    where result = a2 + b2   // can reference a2 and b2
}
```

## Next

Continue to [Chapter 3 — Collections](03_collections.md).
