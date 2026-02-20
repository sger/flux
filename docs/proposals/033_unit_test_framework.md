# Proposal 033: Unit Test Framework for Flux

**Status:** Draft
**Date:** 2026-02-19
**Scope:** CLI, runtime builtins, stdlib module, VM test runner

---

## Motivation

Flux has no mechanism for users to write unit tests in Flux code. Testing currently means
either writing Rust integration tests against the compiler pipeline (which tests the
compiler, not user code), or running programs manually and inspecting `print` output.

This proposal defines a minimal, idiomatic unit test framework for Flux that:

- Requires no new syntax or language features
- Uses Flux's existing functional style (functions, pattern matching, modules)
- Leverages the existing `invoke_value` / `Result<Value, String>` runtime mechanism for
  test isolation
- Adds a `--test` CLI flag for discovery and reporting
- Provides a small set of assertion builtins

---

## Design Principles

1. **No new syntax.** Tests are ordinary Flux functions. No decorators, attributes, or
   special declarations.
2. **Discovery by convention.** Functions named `test_*` are automatically collected by
   the test runner.
3. **Isolation at the Rust level.** The test runner calls each test via `invoke_value`,
   which returns `Result<Value, String>`. A runtime error (from an assertion or otherwise)
   becomes an `Err(String)` that the runner catches — other tests continue.
4. **Assertions are builtins.** They return `Err(String)` on failure, which propagates
   through the VM's normal error path and is caught by the runner.
5. **Pure functional test bodies.** Tests are zero-argument functions. Setup via `let`
   bindings or shared helper functions.

---

## Comparison: Functional Language Test Frameworks

| Language | Framework | Mechanism |
|---|---|---|
| Haskell | HUnit / Hspec | Assertions throw exceptions; runner catches per-test |
| Elm | elm-test | Tests return `Expectation` values (pass/fail); no exceptions |
| Elixir | ExUnit | Built-in, `assert` macro, process-level isolation |
| Rust | `#[test]` | Panic-based assertions; runner isolates per-thread |
| **Flux** | **This proposal** | `Result`-based; runner catches `Err` per `invoke_value` call |

Flux is closest to Rust: assertions produce runtime errors, the runner catches them at
the Rust boundary per test function. No new VM exception mechanism needed.

---

## User-Facing API

### Writing Tests

Tests are zero-argument functions whose name starts with `test_`:

```flux
fn test_add_basic() {
    assert_eq(1 + 1, 2)
}

fn test_string_length() {
    assert_eq(len("hello"), 5)
}

fn test_filter_empty() {
    assert_eq(filter([], \x -> x > 0), [])
}
```

Tests can use `let` bindings and call helpers freely:

```flux
fn make_data() {
    [1, 2, 3, 4, 5]
}

fn test_sum_of_range() {
    let data = make_data()
    assert_eq(sum(data), 15)
}
```

Shared setup via top-level `let`:

```flux
let fixture = [10, 20, 30]

fn test_fixture_len() {
    assert_eq(len(fixture), 3)
}

fn test_fixture_sum() {
    assert_eq(sum(fixture), 60)
}
```

### Importing Modules Under Test

```flux
import Math
import Collections as C

fn test_math_sqrt() {
    assert_eq(Math.sqrt(9), 3)
}

fn test_collection_reverse() {
    assert_eq(C.reverse([1, 2, 3]), [3, 2, 1])
}
```

### Test Output

```
Running tests in math_test.flx

  PASS  test_add_basic              (0ms)
  PASS  test_string_length          (0ms)
  FAIL  test_multiply
          assert_eq failed
          expected: 12
          actual:   8
  PASS  test_filter_empty           (0ms)
  FAIL  test_divide_by_zero
          runtime error: division by zero

5 tests: 3 passed, 2 failed

FAILED
```

Exit code: `0` if all pass, `1` if any fail.

---

## Assertion Builtins

Four new builtins added to `runtime/builtins/`:

### `assert_eq(actual, expected)`

Passes if `actual == expected`. On failure, produces a descriptive error:

```
assert_eq failed
expected: 42
actual:   41
```

### `assert_neq(actual, expected)`

Passes if `actual != expected`.

```
assert_neq failed: both values equal 42
```

### `assert_true(cond)`

Passes if `cond` is `true`.

```
assert_true failed: got false
```

### `assert_false(cond)`

Passes if `cond` is `false`.

```
assert_false failed: got true
```

### Signature

All assertions follow the same pattern — they return `Value::None` on success and
`Err(String)` (a runtime error) on failure. The test runner catches these `Err` values
per-test via `invoke_value`.

```rust
// src/runtime/builtins/assert_ops.rs
pub(super) fn builtin_assert_eq(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 2, "assert_eq", "assert_eq(actual, expected)")?;
    if args[0] == args[1] {
        Ok(Value::None)
    } else {
        Err(format!(
            "assert_eq failed\n  expected: {}\n  actual:   {}",
            args[1], args[0]
        ))
    }
}
```

---

## CLI: `--test` Flag

```bash
cargo run -- --test examples/math_test.flx
cargo run -- --test examples/math_test.flx --root examples/
```

### Behavior

1. Parse and compile the file normally (same pipeline as `--trace`, `--stats`, etc.).
2. Execute all top-level `let` bindings (shared setup/fixtures).
3. Collect all globally-defined functions whose name starts with `test_`.
4. Run each in order via `invoke_value(fn_value, vec![])`.
5. Catch `Ok(_)` as pass, `Err(msg)` as fail.
6. Print a summary and exit with code `0` (all pass) or `1` (any fail).

### Discovery

Test functions are identified at the symbol-table level — any `Global` scope function
whose interned name resolves to a string starting with `"test_"`. No reflection or
metadata needed; the compiler already tracks all global definitions.

### Test Ordering

Tests run in definition order (top-to-bottom in source). This is predictable and
consistent with how Flux currently processes top-level statements.

---

## Isolation Model

Since Flux has no user-level exception handling, isolation is provided at the Rust level:

```rust
// In the test runner (src/runtime/vm/test_runner.rs or similar)
for (name, fn_value) in test_functions {
    let result = vm.invoke_value(fn_value, vec![]);
    match result {
        Ok(_)    => record_pass(name),
        Err(msg) => record_fail(name, msg),
    }
}
```

This means:
- An assertion failure in `test_a` does not abort `test_b`.
- A genuine runtime error (division by zero, index out of bounds) is also caught and
  reported as a test failure with its error message.
- VM state between tests is shared (same global scope). Tests should not mutate shared
  state. Since `let` bindings are immutable, this is naturally enforced.

---

## `Test` Standard Library Module

A thin stdlib module wrapping the builtins, providing grouped tests and richer output:

```flux
// stdlib/Test.flx
module Test {

    fn assert_eq(actual, expected) {
        assert_eq(actual, expected)
    }

    fn assert_neq(actual, expected) {
        assert_neq(actual, expected)
    }

    fn assert_true(cond) {
        assert_true(cond)
    }

    fn assert_false(cond) {
        assert_false(cond)
    }

    // Run a named group of tests and print a section header.
    // Each test is a [name, fn] pair where fn is \() -> ...
    fn describe(name, tests) {
        print("  " ++ name)
        map(tests, \t -> {
            let test_name = t.0
            let test_fn   = t.1
            test_fn()
        })
    }
}
```

Usage with `Test` module:

```flux
import Test

fn test_collections() {
    Test.assert_eq(len([1, 2, 3]), 3)
    Test.assert_eq(reverse([1, 2, 3]), [3, 2, 1])
    Test.assert_true(contains([1, 2, 3], 2))
}
```

---

## File Conventions

| Convention | Description |
|---|---|
| `*_test.flx` | Test files (recommended suffix, not enforced) |
| `test_*` | Test function prefix (enforced by runner for discovery) |
| `setup_*` | Helper functions (ignored by runner, callable from tests) |

Example project layout:

```
src/
  Math.flx
  Collections.flx
tests/
  math_test.flx
  collections_test.flx
```

Running all tests:

```bash
cargo run -- --test tests/math_test.flx --root src/
cargo run -- --test tests/collections_test.flx --root src/
```

---

## Implementation Phases

### Phase 1 — Assert Builtins + `--test` Flag

**Effort:** Low-medium. Touches builtins, CLI, and a small test runner.

Changes:
- `src/runtime/builtins/assert_ops.rs` — 4 new builtins
- `src/runtime/builtins/mod.rs` — register 4 new builtins
- `src/bytecode/compiler/mod.rs` — add `define_builtin` for 4 new names
- `src/main.rs` / CLI — add `--test` flag
- `src/runtime/vm/` — test runner that collects and invokes `test_*` functions

**Result:** Users can write `test_*` functions with `assert_eq` etc. and run them with
`cargo run -- --test file.flx`.

### Phase 2 — `Test` Stdlib Module

**Effort:** Low. Pure Flux code.

- `stdlib/Test.flx` — wraps assert builtins, provides `describe`
- Documentation and examples

### Phase 3 — Property-Based Testing

**Effort:** High. Requires random value generators.

A `Property` module with:

```flux
import Property

fn test_reverse_involution() {
    // forall: for any array of integers, reverse(reverse(xs)) == xs
    Property.for_all(
        Property.array(Property.int),
        \xs -> assert_eq(reverse(reverse(xs)), xs)
    )
}
```

Requires:
- `for_all(generator, property_fn)` builtin
- Built-in generators: `Property.int`, `Property.string`, `Property.bool`,
  `Property.array(gen)`, `Property.option(gen)`
- Shrinking: on failure, find the minimal failing input

---

## Tests Inside Modules

Tests can be grouped inside a module. When a function is defined inside a module, the
compiler stores it under a qualified name via `intern_join` — for example `test_add`
inside `module MathTests` becomes the global `MathTests.test_add`.

The test runner discovers both patterns:

- `test_*` — top-level test functions
- `Tests.test_*` — functions inside a module named exactly `Tests`

Requiring the module to be named `Tests` keeps discovery unambiguous. Private members
(`_prefixed`) are naturally excluded since they cannot be invoked from outside the module.

```flux
// math_test.flx — tests grouped in a module

module Tests {
    fn test_add_basic() {
        assert_eq(1 + 1, 2)
    }

    fn test_add_negative() {
        assert_eq(-1 + 1, 0)
    }

    // Private helper — NOT discovered as a test
    fn _make_fixture() {
        [1, 2, 3]
    }

    fn test_uses_fixture() {
        assert_eq(sum(_make_fixture()), 6)
    }
}

// Top-level tests also allowed in the same file
fn test_standalone() {
    assert_eq(len([]), 0)
}
```

Output groups by source:

```
Running tests in math_test.flx

  [Tests]
  PASS  test_add_basic
  PASS  test_add_negative
  PASS  test_uses_fixture

  [top-level]
  PASS  test_standalone

4 tests: 4 passed, 0 failed
```

---

## JIT Compatibility

The test framework is **fully compatible with JIT mode** (`--jit`). No extra work
required.

**Why it works:**

`JitContext` implements the `RuntimeContext` trait, which includes `invoke_value`. The
test runner calls `invoke_value(test_fn, vec![])` regardless of backend. In VM mode the
callee is `Value::Closure`; in JIT mode it is `Value::JitClosure`. Both are handled by
their respective `invoke_value` implementations and both return `Result<Value, String>`,
which is the runner's isolation mechanism.

Assert builtins work identically in JIT — they are in the `BUILTINS` array and called
via `rt_call_builtin` in JIT-compiled code, dispatching to the same Rust functions.

```bash
# Run tests via VM (default)
cargo run -- --test examples/tests/math_test.flx

# Run tests via JIT
cargo run --features jit -- --test examples/tests/math_test.flx --jit
```

The output and exit code are identical between the two backends.

**One caveat:** `JitContext::invoke_value` currently only handles `Value::Builtin` and
`Value::JitClosure`. If a test function is somehow stored as `Value::Closure` (which
should not happen in a fully JIT-compiled program), it would return an error. In
practice this is not an issue because the JIT compiles all user-defined functions to
`Value::JitClosure`.

---

## What This Does NOT Propose

- **Test parallelism.** Tests run sequentially. Parallel test execution would require a
  more complex VM model.
- **Mocking or stubbing.** Flux's immutable bindings and lack of global mutable state
  make mocking less necessary.
- **Snapshot testing.** The compiler's own test suite uses `insta` for snapshots, but
  user-facing snapshot tests are out of scope here.
- **Benchmarking.** The existing `time(fn)` builtin provides basic timing. A dedicated
  benchmark runner is a separate concern.
- **Watch mode.** `flux --test --watch` (re-run on file change) is a tooling concern
  beyond this proposal.

---

## Files to Change

| File | Phase | Change |
|---|---|---|
| `src/runtime/builtins/assert_ops.rs` | 1 | New file: 4 assert builtins |
| `src/runtime/builtins/mod.rs` | 1 | Register 4 new builtins |
| `src/bytecode/compiler/mod.rs` | 1 | `define_builtin` for 4 new names |
| `src/main.rs` | 1 | `--test` flag, invoke test runner |
| `src/runtime/vm/test_runner.rs` | 1 | New: collect + run `test_*` functions |
| `stdlib/Test.flx` | 2 | New: stdlib Test module |
| `src/runtime/builtins/property_ops.rs` | 3 | Property-based testing |
| `examples/tests/` | All | Example test files |

---

## Example: Complete Test File

```flux
// examples/tests/array_test.flx

fn test_len_empty() {
    assert_eq(len([]), 0)
}

fn test_len_nonempty() {
    assert_eq(len([1, 2, 3]), 3)
}

fn test_push_appends() {
    assert_eq(push([1, 2], 3), [1, 2, 3])
}

fn test_reverse_empty() {
    assert_eq(reverse([]), [])
}

fn test_reverse_single() {
    assert_eq(reverse([42]), [42])
}

fn test_reverse_multiple() {
    assert_eq(reverse([1, 2, 3]), [3, 2, 1])
}

fn test_contains_found() {
    assert_true(contains([1, 2, 3], 2))
}

fn test_contains_not_found() {
    assert_false(contains([1, 2, 3], 99))
}

fn test_sum() {
    assert_eq(sum([1, 2, 3, 4, 5]), 15)
}

fn test_filter_keeps_matching() {
    assert_eq(filter([1, 2, 3, 4], \x -> x > 2), [3, 4])
}
```

Running:

```bash
$ cargo run -- --test examples/tests/array_test.flx

Running tests in array_test.flx

  PASS  test_len_empty              (0ms)
  PASS  test_len_nonempty           (0ms)
  PASS  test_push_appends           (0ms)
  PASS  test_reverse_empty          (0ms)
  PASS  test_reverse_single         (0ms)
  PASS  test_reverse_multiple       (0ms)
  PASS  test_contains_found         (0ms)
  PASS  test_contains_not_found     (0ms)
  PASS  test_sum                    (0ms)
  PASS  test_filter_keeps_matching  (0ms)

10 tests: 10 passed, 0 failed

OK
```

---

## References

- `src/runtime/vm/function_call.rs` — `invoke_value` (the isolation mechanism)
- `src/runtime/builtins/mod.rs` — BUILTINS array and registration pattern
- `src/runtime/builtins/io_ops.rs` — `time(fn)` as a pattern for calling user functions
  from builtins
- `src/main.rs` — existing CLI flag handling
- Elm Test: `package.elm-lang.org/packages/elm-explorations/test`
- HUnit: `hackage.haskell.org/package/HUnit`
