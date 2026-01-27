# Flux

## Language at a glance

- File extension: `.flx`
- Functions use `fun` and return the last expression
- Bindings are immutable by default (`let`)

## Overview and inspiration

Flux is a small, functional language inspired by Elixir’s expressiveness and Rust’s safety ethos. It’s also a learning-focused project: the codebase is designed to be approachable for anyone who wants to understand how lexing, parsing, compilation, and bytecode VMs fit together. It was created while reading *Building a Compiler in Go*.

Example:

```flux
fun add(a, b) {
  a + b;
}

let pi = 3.14;
print(add(1, 2));
```

## What’s supported today

- Literals: integers, floats, booleans, strings, null
- Data: arrays and hash maps
- Control flow: `if` / `else`, `return`
- Functions: first-class functions and closures
- Builtins: `print`, `len`, `first`, `last`, `rest`, `push`

## Running Flux

```
cargo run -- run path/to/file.flx
```

Other commands:

```
cargo run -- tokens path/to/file.flx
cargo run -- bytecode path/to/file.flx
```

## Example with closures

```flux
let newClosure = fun(a) { fun() { a; }; };
let closure = newClosure(99);
print(closure());
```
