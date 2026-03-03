# Chapter 11 — HOF and Effect Polymorphism

> Example: [`examples/guide_type_system/06_hof_with_e_compose.flx`](../../examples/guide_type_system/06_hof_with_e_compose.flx)

## Learning Goals

- Write higher-order functions that accept typed callbacks.
- Use `with e` (row variable) to preserve the callback's effect through the wrapper.
- Understand how effect rows are propagated and why pure callbacks stay pure.
- Recognize effect propagation failures (`E400`).

## The Problem: Higher-Order Functions and Effects

Consider a generic `apply_twice` that calls a function `f` twice:

```flux
fn apply_twice(f: Int -> Int, x: Int) -> Int {
    f(f(x))
}
```

This works for pure callbacks. But what if the callback has an `IO` effect?

```flux
fn log_and_inc(x: Int) -> Int with IO {
    print("x = #{x}")
    x + 1
}

fn main() with IO {
    apply_twice(log_and_inc, 1)   // E400: apply_twice doesn't carry IO
}
```

The problem: `apply_twice` declares a pure callback type (`Int -> Int`). When an IO callback is passed, the effect requirement leaks out but the wrapper has no room for it.

---

## The Solution: `|e` Effect Row Variables

The `|e` tail syntax introduces an **effect row variable** — a variable that stands for "whatever effects the callback carries". The `|` separates concrete effects from the open row:

```flux
-- callback-only row variable
fn apply_twice(f: (Int -> Int with |e), x: Int) -> Int with |e {
    f(f(x))
}

-- concrete effect + row variable
fn log_apply(f: (Int -> Int with |e), x: Int) -> Int with IO | e {
    print("calling f")
    f(x)
}
```

Now:
- When `f` is a pure callback (`Int -> Int`), `e` resolves to empty — `apply_twice` is pure.
- When `f` is an IO callback (`Int -> Int with IO`), `e` resolves to `IO` — `apply_twice` carries `IO` too.

The effect row variable `e` propagates the callback's effect through the wrapper transparently.

> **Note on syntax:** Row variables must appear as a `|e` tail after any concrete effects. `with IO | e` means "IO plus whatever e is". `with |e` means "only whatever e is". The old implicit form `with e` (lowercase identifier without `|`) is **rejected** — always use `|e`.

---

## Worked Example

```flux
fn apply_twice(f: (Int -> Int with |e), x: Int) -> Int with |e {
    f(f(x))
}

fn plus_one(x: Int) -> Int { x + 1 }

fn log_inc(x: Int) -> Int with IO {
    print("incrementing #{x}")
    x + 1
}

fn main() with IO {
    // Pure callback — apply_twice is effectively pure here
    let a = apply_twice(plus_one, 5)
    print(a)    // 7

    // IO callback — apply_twice carries IO here
    let b = apply_twice(log_inc, 5)
    print(b)    // 7 (with log output)
}
```

Run:

```bash
cargo run -- --no-cache examples/guide_type_system/06_hof_with_e_compose.flx
cargo run --features jit -- --no-cache examples/guide_type_system/06_hof_with_e_compose.flx --jit
```

---

## Effect Row Composition

Effect rows support additive composition in annotations:

| Syntax | Meaning |
|--------|---------|
| `with \|e` | Row variable only — inherits exactly the callback's effects |
| `with IO` | Fixed `IO` effect |
| `with IO \| e` | `IO` plus whatever `e` resolves to |
| `with IO, Time` | Fixed `IO` and `Time` |
| `with IO + State - Console` | Row extension/subtraction (advanced) |

The `|e` tail always comes last. The most common patterns:
- `with |e` — pure generic wrapper (propagates whatever the callback has)
- `with IO | e` — wrapper that also requires IO itself
- `with IO` — concrete effectful function, no polymorphism

---

## Propagation Through Chains

Effect rows propagate through multi-level call chains:

```flux
fn step(f: (Int -> Int with |e), x: Int) -> Int with |e {
    f(x + 1)
}

fn pipeline(f: (Int -> Int with |e), x: Int) -> Int with |e {
    step(f, step(f, x))
}

fn main() with IO {
    // Entire pipeline carries IO when callback is IO
    pipeline(\x -> do { print(x); x }, 0)
}
```

The `e` variable threads through `step` → `pipeline` → `main` without any explicit re-annotation at each intermediate level.

---

## Rule: Missing Effect in Caller

If a caller declares an incompatible effect for a polymorphic callback chain, Flux emits `E400`:

```flux
fn apply(f: (Int -> Int with |e), x: Int) -> Int with |e { f(x) }

-- pure context — but f is IO
fn bad_pure() -> Int {
    apply(\x -> do { print(x); x }, 1)   -- E400: IO not in scope
}
```

The fix is either to declare `with IO` on `bad_pure` or use a pure callback.

---

## Row Variable Diagnostics

| Code | Trigger | Fix |
|------|---------|-----|
| `E400` | IO callback passed to wrapper without `with \|e` or `with IO` | add `with IO` or `with IO \| e` |
| `E400` | Caller context doesn't carry the propagated effect | annotate enclosing function with the required effect |
| `E419` | Single row variable remains unresolved after inference | provide a concrete `with` annotation |
| `E420` | Multiple row variables are ambiguous | disambiguate with explicit effect annotations |
| `E421` | Effect subtracted that isn't in the row | remove invalid subtraction |
| `E422` | Required effect subset not satisfied | add the missing effects to the row |

---

## Next

Continue to [Chapter 12 — Modules, Public API, and Strict Mode](12_modules_public_api_and_strict.md).
