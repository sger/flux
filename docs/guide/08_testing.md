# Chapter 8 — Testing

> Full examples: [`examples/tests/`](../../examples/tests/), [`lib/Flow/FTest.flx`](../../lib/Flow/FTest.flx)

## Unit Test Framework

Any zero-argument function whose name starts with `test_` is a test. Run them with `--test`:

```bash
cargo run -- --test examples/tests/array_test.flx
```

Output:

```
running 10 tests
test test_len_empty          ... ok
test test_len_nonempty       ... ok
test test_reverse_single     ... ok
...
test result: ok. 10 passed; 0 failed
```

Exit code is `0` when all tests pass, `1` when any fail.

## Writing Tests

```flux
fn test_add() {
    assert_eq(1 + 1, 2)
}

fn test_negation() {
    assert_false(1 == 2)
}

fn test_divide_by_zero() {
    assert_throws(\() -> 10 / 0)
}
```

## Assert Builtins

| Builtin | Passes when |
|---------|-------------|
| `assert_eq(a, b)` | `a == b` |
| `assert_neq(a, b)` | `a != b` |
| `assert_true(expr)` | `expr` is `true` |
| `assert_false(expr)` | `expr` is `false` |
| `assert_throws(fn)` | calling `fn()` raises a runtime error |

```flux
fn test_string_ops() {
    assert_eq(upper("hello"), "HELLO")
    assert_eq(len([|1, 2, 3|]), 3)
    assert_true(contains([|1, 2, 3|], 2))
    assert_false(contains([|1, 2, 3|]), 99)
}
```

> See [`examples/tests/array_test.flx`](../../examples/tests/array_test.flx).

## Discovery Rules

- **Top-level** functions named `test_*` are discovered automatically.
- Functions inside a module named exactly **`Tests`** are also discovered (accessed as `Tests.test_*`):

```flux
module Tests {
    fn test_add() { assert_eq(1 + 1, 2) }
    fn test_sub() { assert_eq(5 - 3, 2) }
}
```

## Running a Single Test

Use `--test-filter` to match by name substring:

```bash
cargo run -- --test examples/tests/array_test.flx --test-filter test_len
```

Only tests whose name contains `"test_len"` will run.

## JIT Mode

Tests run identically under the JIT backend:

```bash
cargo run --features jit -- --test examples/tests/array_test.flx --jit
```

## FTest Standard Library

`Flow.FTest` (in `lib/Flow/FTest.flx`) provides richer wrappers and helpers. Import with `--root lib/`:

```bash
cargo run -- --root lib --test my_tests.flx
```

```flux
import Flow.FTest as T

fn test_math() {
    T.eq(1 + 1, 2)
    T.approx_eq(3.14, 3.141, 0.01)  // float comparison with tolerance
}

fn test_suite() {
    T.describe("string ops", \() -> do {
        T.it("upper works", \() -> T.eq(upper("hi"), "HI"))
        T.it("trim works",  \() -> T.eq(trim("  x  "), "x"))
    })
}
```

Available wrappers: `eq`, `neq`, `is_true`, `is_false`, `throws`, `approx_eq`
Available helpers: `describe`, `it`, `for_each`, `with_fixture`
