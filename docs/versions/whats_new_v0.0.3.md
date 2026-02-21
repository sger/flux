# What's New in Flux v0.0.3

Flux v0.0.3 is the largest release yet — a second execution backend, persistent collections with GC, new language constructs, and a built-in test framework.

## Highlights

- **Cranelift JIT** — compile Flux programs to native machine code
- **Persistent collections** — GC-managed cons lists and HAMT hash maps
- **Tuples, do-blocks, and `where` clauses** — new language constructs
- **List comprehensions** — `[x * 2 | x <- xs, x > 0]`
- **Pattern matching: cons and tuple patterns** — `[h | t]`, `(a, b)`
- **Unit test framework** — `test_*` functions + assert builtins + `FTest` stdlib
- **`--stats` analytics** — timing and code metrics per run
- 75 builtins (up from 35), 62 opcodes (up from 44)

## New Language Features

### Cranelift JIT Backend

```bash
cargo build --features jit
cargo run --features jit -- examples/basics/fibonacci.flx --jit
```

Compiles the AST directly to native machine code via [Cranelift](https://cranelift.dev/). Both backends share the same builtin table and GC heap — new builtins are automatically available in JIT mode.

### Persistent Cons Lists

Immutable linked lists allocated on the GC heap with O(1) prepend.

```flux
let nums = [1, 2, 3, 4]       // list literal
let more = [0 | nums]          // prepend: [0, 1, 2, 3, 4]

fn sum(lst) {
    match lst {
        [h | t] -> h + sum(t),
        _       -> 0,
    }
}

print(sum(nums))   // 10
```

Use `list(1, 2, 3)`, `hd`, `tl`, `to_list`, `to_array` to work with cons lists.

### Arrays vs. Lists

Arrays (`[| |]`) and cons lists (`[]`) are now distinct types:

```flux
let arr  = [|10, 20, 30|]             // array — Rc-backed, indexed
let lst  = [10, 20, 30]               // cons list — GC-managed, prepend

print(slice(arr, 1, 3))               // [|20, 30|]
print(to_array(lst))                  // [|10, 20, 30|]
```

### Persistent Hash Maps (HAMT)

Hash maps now use a Hash Array Mapped Trie (HAMT) with structural sharing on update — the original is never mutated.

```flux
let user    = {"name": "Alice", "age": 30}
let updated = put(user, "email", "alice@example.com")

print(get(user, "email"))       // None  (original unchanged)
print(get(updated, "email"))    // Some(alice@example.com)
```

### Tuples

```flux
let point = (3, 4)
let x = point.0        // 3
let (a, b) = point     // destructuring in let

fn swap((a, b)) { (b, a) }
print(swap((1, 2)))    // (2, 1)
```

Match on tuples:
```flux
match pair {
    (0, y) -> "starts at zero",
    (x, y) -> x + y,
}
```

### Do-Blocks

Sequential expressions in a single block — last expression is the value:

```flux
let result = do {
    let x = 10;
    let y = x * 2;
    y + 5
}
print(result)   // 25
```

### Where Clauses

Local bindings scoped to a block, written after the body expression:

```flux
fn hypotenuse(a, b) {
    sqrt(sum_of_squares)
    where sum_of_squares = a * a + b * b
}
```

Multiple clauses chain left-to-right; later ones can reference earlier ones.

### List Comprehensions

Desugared at parse time to `map`/`filter`/`flat_map` — no VM changes needed.

```flux
let xs = [1, 2, 3, 4, 5]

let doubled = [x * 2 | x <- xs]                    // map
let evens   = [x | x <- xs, x % 2 == 0]            // filter + map
let pairs   = [(x, y) | x <- xs, y <- xs, x < y]   // flat_map
```

### Pattern Guards

```flux
fn grade(score) {
    match score {
        n if n >= 90 -> "A",
        n if n >= 80 -> "B",
        n if n >= 70 -> "C",
        _            -> "F",
    }
}
```

### Cons Patterns in Match

```flux
fn to_string_list(lst) {
    match lst {
        [h | t] -> to_string(h) + ", " + to_string_list(t),
        _       -> "",
    }
}
```

## New Builtins (75 total)

### Higher-Order

`flat_map`, `zip`, `find`, `any`, `all`, `count`, `sort_by`, `first`, `last`, `rest`, `range`, `sum`, `product`

```flux
let evens = filter([1,2,3,4,5,6], \x -> x % 2 == 0)
let total = fold([1,2,3,4,5], 0, \(acc, x) -> acc + x)
let pairs = zip([1,2,3], ["a","b","c"])      // [(1,"a"), (2,"b"), (3,"c")]
```

### Cons Lists

`list`, `hd`, `tl`, `is_list`, `to_list`, `to_array`

### I/O

`read_file`, `read_lines`, `read_stdin`, `parse_int`, `parse_ints`, `split_ints`, `now_ms`, `time`

### Testing / Assertions

`assert_eq`, `assert_neq`, `assert_true`, `assert_false`, `assert_throws`

## Unit Test Framework

Functions named `test_*` are automatically discovered and run with `--test`:

```flux
fn test_add() {
    assert_eq(1 + 1, 2)
}

fn test_negation() {
    assert_false(1 == 2)
}
```

```bash
cargo run -- --test examples/tests/math_test.flx
cargo run -- --test examples/tests/math_test.flx --test-filter test_add
```

Also supports a `Tests` module:

```flux
module Tests {
    fn test_something() { assert_eq(1, 1) }
}
```

The `FTest` stdlib module (`lib/Flow/FTest.flx`) provides `describe`, `it`, `for_each`, `with_fixture`, and `approx_eq` helpers. Import with `--root lib/`.

## Tooling

### `--stats` Analytics

```
  ── Flux Analytics ───────────────────────────
  parse                    1.54 ms
  compile                  0.31 ms  [bytecode]
  execute                  0.19 ms  [vm]
  total                    2.04 ms

  modules                     2
  source lines              119
  instructions              529 bytes
  ────────────────────────────────────────────
```

### `--optimize` / `-O`

Runs AST optimization passes: desugaring → constant folding → alpha-renaming.

### `--analyze` / `-A`

Runs analysis passes: free variable collection and tail-call detection.

### New CLI Subcommands

```bash
flux analyze-free-vars <file.flx>    Show free variable analysis
flux analyze-tail-calls <file.flx>   Show tail-call sites
flux cache-info-file <file.fxc>      Inspect a .fxc cache file directly
```

### VS Code Extension

Syntax highlighting for `.flx` files. See `tools/vscode/`.

### `dev-fast` Build Profile

```bash
cargo build --profile dev-fast    # opt-level 3 + lighter debug info
```

## GC

A mark-and-sweep garbage collector manages cons cells and HAMT nodes. Runs automatically when allocation count exceeds the threshold (default 10,000).

```bash
cargo run -- examples/lists.flx --gc-threshold 5000
cargo run -- examples/lists.flx --no-gc
cargo run --features gc-telemetry -- examples/lists.flx --gc-telemetry
```

## Compatibility Notes

- Array syntax changed: use `[| |]` for arrays, `[]` for cons lists
- `fun` keyword removed; `fn` is the only function keyword
- All v0.0.2 programs that do not mix `[]`/`[| |]` syntax continue to work
