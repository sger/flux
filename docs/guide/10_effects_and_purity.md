# Chapter 10 — Effects and Purity

> Examples: [`examples/guide_type_system/04_with_io_and_with_time.flx`](../../examples/guide_type_system/04_with_io_and_with_time.flx), [`05_perform_handle_basics.flx`](../../examples/guide_type_system/05_perform_handle_basics.flx)

## Learning Goals

- Understand pure-by-default execution and what it means in practice.
- Declare and use built-in `IO` and `Time` effects.
- Use `perform` to invoke effect operations and `handle` to discharge them.
- Understand top-level effect policy (`E413`, `E414`) and the `main` entrypoint model.

## Pure-by-Default

In Flux, **typed functions are pure by default**. A pure function has no side effects — it cannot print, read files, access the clock, or perform any observable I/O. Purity is enforced statically at compile time for typed code paths.

If a function calls an effectful base function like `print` without declaring the required effect, the compiler rejects it with `E400`:

```flux
fn greet(name: String) -> String {
    print("Hi!")    // E400: missing IO effect
    "Hello, #{name}!"
}
```

To allow I/O, declare the effect with `with`:

```flux
fn greet(name: String) -> String with IO {
    print("Hi!")    // OK — IO is in scope
    "Hello, #{name}!"
}
```

---

## Built-in Effects

| Effect | Description | Triggered by |
|--------|-------------|-------------|
| `IO` | Reads, writes, file I/O, terminal | `print`, `read_file`, `read_lines`, `read_stdin` |
| `Time` | Wall-clock access | `now_ms`, `time` |

These are statically enforced — you cannot call `print` in a typed pure context. The compiler traces effect requirements transitively through the call graph.

---

## Effect Annotations

Use `with <EffectList>` after the return type in a function signature:

```flux
fn log(msg: String) with IO {
    print(msg)
}

fn timed_log(msg: String) with IO, Time {
    let t = now_ms()
    print("[#{t}] #{msg}")
}
```

Multiple effects are comma-separated. Order doesn't matter.

---

## Top-Level Purity Policy

Effectful code **cannot** appear at the top level of a Flux program:

```flux
// Error E413: top-level effectful expression
print("Hello!")
```

Flux also requires a `main` function when the program has side effects:

```flux
// Error E414: effectful program without main
fn greet() with IO { print("Hi!") }
greet()  // top-level call is also effectful → E413
```

The correct pattern:

```flux
fn greet(name: String) with IO {
    print("Hello, #{name}!")
}

fn main() with IO {
    greet("Alice")
}
```

`main` is the **root effect handler** — it may carry `IO` and `Time` in its effect set. Custom effects must be discharged before `main` returns (see `handle` below).

### `main` signature rules

| Violation | Error |
|-----------|-------|
| More than one `main` | `E410` |
| `main` has parameters | `E411` |
| `main` has a non-Unit return type | `E412` |
| Custom effect not discharged at root | `E406` |

---

## `perform` and `handle`

Algebraic effects extend pure-by-default to **user-defined** effect operations.

### Declaring an effect

```flux
effect Console {
    fn print(msg: String) -> Unit
}
```

This declares a `Console` effect with one operation, `print`.

### Performing an operation

Inside a function that carries `with Console`, use `perform` to invoke the operation:

```flux
fn say(msg: String) with Console {
    perform Console.print(msg)
}
```

### Handling an effect

`handle` discharges an effect by providing implementations for each declared operation:

```flux
fn main() with IO {
    say("Hello!") handle Console {
        print(msg) -> print(msg)   // delegate to built-in IO print
    }
}
```

The `handle` expression wraps a computation and intercepts every `perform Console.print(...)` call, running the handler arm instead. When the handler arm returns, the result flows back to the `perform` call site.

### Semantics and error codes

| Situation | Error |
|-----------|-------|
| `perform` references undeclared effect | `E403` |
| `perform` references unknown operation | `E404` |
| `handle` references undeclared effect | `E405` |
| `handle` has arm for unknown operation | `E401` |
| `handle` is missing declared operations | `E402` |
| Missing ambient effect for a call | `E400` |
| Custom effect escapes `main` boundary | `E406` |

---

## Example: IO + Time in `main`

`04_with_io_and_with_time.flx` defines a `Time` function consumed by an `IO, Time` `main`.

Run:

```bash
cargo run -- --no-cache examples/guide_type_system/04_with_io_and_with_time.flx
cargo run --features jit -- --no-cache examples/guide_type_system/04_with_io_and_with_time.flx --jit
```

---

## Example: `perform` / `handle` basics

`05_perform_handle_basics.flx` performs `Console.print` and discharges it with a handler.

Run:

```bash
cargo run -- --no-cache examples/guide_type_system/05_perform_handle_basics.flx
cargo run --features jit -- --no-cache examples/guide_type_system/05_perform_handle_basics.flx --jit
```

---

## Effect Propagation

Effects propagate transitively through call chains. If `f` calls `g`, and `g` requires `IO`, then `f` must also declare `IO` (or handle it):

```flux
fn inner() with IO {
    print("inner")
}

fn outer() with IO {
    inner()    // OK — outer carries IO
}

fn bad() {
    inner()    // E400: missing IO
}
```

The compiler traces these requirements **at call boundaries**, not dynamically.

---

## Failure Patterns to Remember

| Pattern | Error |
|---------|-------|
| Effectful top-level expression | `E413` |
| Effectful program without `main` | `E414` |
| Calling effectful function in pure context | `E400` |
| `perform` without required effect in scope | `E400` |
| `handle` for unknown effect | `E405` |
| `handle` missing required arms | `E402` |

---

## Next

Continue to [Chapter 11 — HOF and Effect Polymorphism](11_hof_effect_polymorphism.md).
