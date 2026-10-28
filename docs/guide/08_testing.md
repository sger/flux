# Chapter 8 — Testing

> Full examples: [`examples/tests/`](../../examples/tests/), [`lib/Flow/FTest.flx`](../../lib/Flow/FTest.flx)

## Unit Test Framework

A function whose name starts with `test_` is a test. It takes no arguments.
Run a file's tests with `--test`:

```bash
cargo run -- --test examples/tests/array_test.flx
```

Output:

```
Running tests in array_test.flx

  PASS  test_len_empty             (0ms)
  PASS  test_len_nonempty          (0ms)
  PASS  test_reverse_single        (0ms)
  ...

10 tests: 10 passed, 0 failed

OK
```

Tests run one at a time, in the order they are defined. A failing test does
not stop the others; it prints `FAIL`, then the reason on the next line:

```
  FAIL  test_fails
          panic: assert_true failed

2 tests: 1 passed, 1 failed

FAILED
```

The exit code is `0` when every test passes, and also when the file has no
tests. It is `1` when a test fails, or when the file does not compile. Set
`NO_COLOR` to turn off the colours.

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

A test, like `main`, can print or read files without declaring `with
Console` or `with IO`.

## Assert Base Functions

These come with the prelude, so no import is needed.

| Base | Passes when |
|---------|-------------|
| `assert_eq(a, b)` | `a == b`, compared by structure |
| `assert_neq(a, b)` | `a != b` |
| `assert_true(expr)` | `expr` is `true` |
| `assert_false(expr)` | `expr` is `false` |
| `assert_gt(a, b)`, `assert_lt(a, b)` | `a > b`, `a < b` |
| `assert_gte(a, b)`, `assert_lte(a, b)` | `a >= b`, `a <= b` |
| `assert_len(collection, n)` | the collection has `n` elements |
| `assert_throws(fn)` | calling `fn()` raises any runtime error |
| `assert_throws(fn, msg)` | calling `fn()` raises an error containing `msg` |
| `assert_msg(cond, msg)` | `cond` is `true`, otherwise fails with `msg` |

`try_catch(fn)` runs `fn()` and returns `("ok", …)` or `("error", message)`,
for checking an error by hand.

```flux
fn test_string_ops() {
    assert_eq(upper("hello"), "HELLO")
    assert_eq(len([|1, 2, 3|]), 3)
    assert_true(contains([1, 2, 3], 2))
    assert_false(contains([1, 2, 3], 99))
}

fn test_ordering() {
    assert_gt(3, 2)
    assert_lte(2, 2)
    assert_len([1, 2, 3], 3)
}

fn test_error_message() {
    let (status, _) = try_catch(\() -> 10 / 0)
    assert_eq(status, "error")
    assert_throws(\() -> 10 / 0, "division by zero")
}
```

> See [`examples/tests/array_test.flx`](../../examples/tests/array_test.flx).

## Discovery Rules

- `test_*` functions are found in every module the program compiles: the
  file you run and every module it imports.
- Tests in a module named **`Tests`** are listed under their own heading.

A file that declares a module holds only that module, so put the `Tests`
module in its own file, `Tests.flx`, and import it:

```flux
// Tests.flx
module Tests {
    fn test_add() { assert_eq(1 + 1, 2) }
    fn test_sub() { assert_eq(5 - 3, 2) }
}
```

```flux
// main_test.flx
import Tests

fn test_top_level() {
    assert_true(true)
}
```

```
Running tests in main_test.flx

  [Tests]
  PASS  test_add             (0ms)
  PASS  test_sub             (0ms)

  [top-level]
  PASS  test_top_level       (0ms)

3 tests: 3 passed, 0 failed

OK
```

## Running a Single Test

Use `--test-filter` to match by name substring:

```bash
cargo run -- --test examples/tests/array_test.flx --test-filter test_len
```

Only tests whose name contains `"test_len"` will run. The filter sees the
full name, so `--test-filter Tests.test_add` picks one test from the `Tests`
module. When nothing matches, the runner says so and exits with `0`.

## Native Backend

Tests run the same way when compiled natively with LLVM. This needs a build
with the `llvm` feature:

```bash
cargo run --features llvm -- --test --native examples/tests/array_test.flx
```

## FTest Standard Library

`Flow.FTest` (in `lib/Flow/FTest.flx`) provides richer wrappers and helpers.
Import it like any other `Flow` module. FTest functions use Console, so a
test that calls them declares `with Console`, and one that calls `describe`
declares `with IO`:

```flux
import Flow.FTest as T

fn test_math() with Console {
    T.eq(1 + 1, 2)
    T.approx_eq(3.14, 3.141, 0.01)  // float comparison with tolerance
}

fn test_suite() with IO {
    T.describe("string ops", [
        T.it("upper works", \() -> T.eq(upper("hi"), "HI")),
        T.it("trim works", \() -> T.eq(trim("  x  "), "x")),
    ])
}
```

`describe` runs each case and prints `PASS` or `FAIL` for it. It does not
make the enclosing test fail, so read its output, or use the wrappers
directly when a failure should fail the test.

Available wrappers: `eq`, `neq`, `is_true`, `is_false`, `gt`, `lt`, `gte`,
`lte`, `has_len(collection, n)`, `approx_eq(actual, expected, epsilon)` and
`throws(fn, msg)`.

Available helpers:

- `describe(name, cases)`, with each case made by `it(name, fn)`;
- `for_each(items, fn)`, which runs `fn` on every item;
- `with_fixture(setup, fn)`, which passes `setup()`'s result to `fn`.
