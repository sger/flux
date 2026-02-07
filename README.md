# Flux

A small, functional language with a custom bytecode VM.

## Current Features

- **Functions**: `fun` declarations, closures, higher-order functions, forward references, mutual recursion, lambda shorthand (`\x -> x + 1`)
- **Immutability**: `let` bindings are immutable; reassignment is rejected
- **Scoping**: lexical scoping, closures, and free variables
- **Modules**: static, qualified namespaces (`module Name { ... }`), public by default, `_private` hidden; module names must start uppercase
- **Imports**: top-level only, explicit qualified access, aliases supported (Haskell-style); collisions are errors; cycles rejected
- **Data types**: integers, floats, booleans, strings, `None`/`Some`, `Left`/`Right` (Either)
- **Collections**: arrays and hash maps, indexing with `[]`
- **Operators**: comparison (`<=`, `>=`), modulo (`%`), logical (`&&`, `||`), pipe (`|>`)
- **Control flow**: `if` / `else`, `return`
- **Builtins (array)**: `concat`, `reverse`, `contains`, `slice`, `sort`
- **Builtins (string)**: `split`, `join`, `trim`, `upper`, `lower`, `chars`, `substring`
- **Builtins (core)**: `print`, `len`, `first`, `last`, `rest`, `push`, `to_string`
- **Diagnostics**: errors with codes, file/line/column, caret highlighting; multi-line spans supported
- **VM trace**: `--trace` instruction/stack/locals logging
- **Linter**: unused vars/params/imports, shadowing, naming style
- **Formatter**: `flux fmt` (indentation-only, preserves comments)
- **Bytecode cache**: `.fxc` cache with dependency hashing and inspection tools (debug info included)

## Running Flux

```
cargo run -- examples/basics/print.flx
cargo run -- run examples/basics/print.flx --verbose
```

## Diagnostics

Flux diagnostics are structured and phase-aware, with consistent headers, file grouping, and rich guidance.

Header format:
```
--> compiler error[E101]: UNKNOWN KEYWORD
```
```
--> runtime error[E1009]: INVALID OPERATION
```
```
--> warning[W200]: UNUSED VARIABLE
```

Key behaviors:
- Errors are grouped by file with a file header: `--> path/to/file.flx`
- A summary line appears when there are multiple diagnostics, e.g. `Found 3 errors and 2 warnings.`
- Diagnostics are sorted by file, then line/column, then severity
- Error codes are stable identifiers (e.g. `E101`, `E1009`) for lookup and tooling
- Hints, notes, and inline suggestions render with context and source snippets
- Related diagnostics can point to other files (rendered when source is available)
- Runtime errors include stack traces

Example:
```
Found 2 errors.

--> examples/hint_demos/inline_suggestion_demo.flx
--> compiler error[E101]: UNKNOWN KEYWORD

Flux uses `fun` for function declarations.

  --> examples/hint_demos/inline_suggestion_demo.flx:24:1
  |
24 | fn add(a, b) {
  | ^^   |
help: Replace 'fn' with 'fun'
   |
24 | fun add(a, b) {
  | ~~~
```

Options:
- `--max-errors <n>` limits the number of errors displayed (warnings still show)
- `NO_COLOR=1` disables ANSI color output

## Forward References and Mutual Recursion

Functions can reference other functions defined later in the same scope. This enables mutual recursion and flexible code organization:

```flux
// functions/forward_reference.flx
fun main() {
    print(greet("World"));  // calls greet() defined below
    print(isEven(10));      // mutual recursion
}

fun greet(name) {
    "Hello, " + name + "!";
}

// Mutual recursion: isEven and isOdd call each other
fun isEven(n) {
    if n == 0 { true; } else { isOdd(n - 1); }
}

fun isOdd(n) {
    if n == 0 { false; } else { isEven(n - 1); }
}

main();
```

Forward references also work within modules:

```flux
module Math {
    // quadruple uses double which is defined below
    fun quadruple(x) { double(double(x)); }
    fun double(x) { x * 2; }
}
```

## Modules and Imports

Flux uses static, qualified modules (Haskell-style). Imports are required for qualified access.

```flux
// examples/Modules/Data/MyFile.flx
module Modules.Data.MyFile {
  fun value() { 42; }
}

// examples/Modules/Main.flx
import Modules.Data.MyFile
print(Modules.Data.MyFile.value());
```

Aliases replace the original qualifier:

```flux
import Modules.Data.MyFile as MyFile
print(MyFile.value());
// Modules.Data.MyFile.value(); // error: module not imported
```

Cycles are rejected at compile time (E035).

## Module Roots

By default, Flux searches the entry file directory and `./src` as module roots.
Use `--root` to add roots, or `--roots-only` to make them exclusive.

```
cargo run -- --root examples/roots/root_a --root examples/roots/root_b examples/roots/duplicate_root_import_error.flx
cargo run -- --roots-only --root examples/roots/root_a --root examples/roots/root_b examples/roots/duplicate_root_import_error.flx
```

## Tooling

```
cargo run -- tokens path/to/file.flx
cargo run -- bytecode path/to/file.flx
cargo run -- lint path/to/file.flx
cargo run -- fmt path/to/file.flx
cargo run -- fmt --check path/to/file.flx
cargo run -- cache-info path/to/file.flx
cargo run -- cache-info-file path/to/file.fxc
```

## Running Examples

Use the helper script to run any example with the right module roots:

```
scripts/run_examples.sh basics/print.flx
scripts/run_examples.sh ModuleGraph/ --no-cache
scripts/run_examples.sh ModuleGraph/module_graph_main.flx --no-cache --trace
```

Run with no args to see usage + the example list:

```
scripts/run_examples.sh
```

Run all non-error examples (passes extra flags to each run):

```
scripts/run_examples.sh --all --no-cache
```

`--all` excludes `examples/errors`, `examples/Debug`, and other intentionally failing fixtures.

## Examples

The `examples/` tree currently contains these groups of `.flx` programs:

- `basics` (33): language fundamentals, builtins, interpolation, semicolon and recovery behavior
- `Modules` (18): module declarations, qualified imports, aliases, and composition across files
- `errors` (37): intentional compiler/runtime failures and diagnostic paths
- `functions` (11): closures, forward references, immutability, redeclaration/reassignment errors
- `imports`, `namespaces`, `patterns`, `ModuleGraph`, `suggestions`, `hint_demos`, `lint`, `advanced`, `Debug`, `roots`, `error_messages`

Examples are designed to run through the helper script:

```
scripts/run_examples.sh <path-under-examples>
scripts/run_examples.sh <folder-under-examples>/
```

Representative examples:

| Example | What it demonstrates | Run |
|---|---|---|
| `examples/basics/print.flx` | basic literals and `print` | `scripts/run_examples.sh basics/print.flx` |
| `examples/basics/string_interpolation.flx` | interpolation segments and expressions | `scripts/run_examples.sh basics/string_interpolation.flx` |
| `examples/basics/pipe_operator.flx` | `|>` operator and function chaining | `scripts/run_examples.sh basics/pipe_operator.flx` |
| `examples/basics/math_builtins.flx` | numeric builtins (`abs`, `min`, `max`) | `scripts/run_examples.sh basics/math_builtins.flx` |
| `examples/functions/forward_reference.flx` | forward references and mutual recursion | `scripts/run_examples.sh functions/forward_reference.flx` |
| `examples/functions/closure.flx` | closure capture and lexical scoping | `scripts/run_examples.sh functions/closure.flx` |
| `examples/Modules/Main.flx` | module import and qualified call | `scripts/run_examples.sh Modules/Main.flx` |
| `examples/Modules/pipe_with_modules.flx` | pipe operator with module functions | `scripts/run_examples.sh Modules/pipe_with_modules.flx` |
| `examples/imports/test_import_math.flx` | import resolution with module roots | `scripts/run_examples.sh imports/test_import_math.flx` |
| `examples/namespaces/namespace_alias_ok.flx` | aliasing module namespaces | `scripts/run_examples.sh namespaces/namespace_alias_ok.flx` |
| `examples/patterns/either_match.flx` | pattern matching on `Left` / `Right` | `scripts/run_examples.sh patterns/either_match.flx` |
| `examples/patterns/match_guards.flx` | guarded match arms | `scripts/run_examples.sh patterns/match_guards.flx` |
| `examples/ModuleGraph/module_graph_main.flx` | module graph traversal and topo compile order | `scripts/run_examples.sh ModuleGraph/module_graph_main.flx` |
| `examples/lint/warnings.flx` | linter warnings (`W008-W010`) | `scripts/run_examples.sh lint/warnings.flx` |
| `examples/suggestions/simple_typo.flx` | did-you-mean suggestions for identifiers | `scripts/run_examples.sh suggestions/simple_typo.flx` |
| `examples/hint_demos/inline_suggestion_demo.flx` | intentional parse errors with inline fix suggestions | `scripts/run_examples.sh hint_demos/inline_suggestion_demo.flx` |
| `examples/error_messages/unknown_keyword.flx` | canonical `UNKNOWN_KEYWORD` diagnostic sample | `scripts/run_examples.sh error_messages/unknown_keyword.flx` |
| `examples/errors/unterminated_string.flx` | intentional unterminated string compiler error | `scripts/run_examples.sh errors/unterminated_string.flx` |
| `examples/roots/duplicate_root_import_error.flx` | duplicate module resolution across roots | `scripts/run_examples.sh roots/duplicate_root_import_error.flx` |

Run a whole category:

```
scripts/run_examples.sh basics/
scripts/run_examples.sh patterns/
scripts/run_examples.sh errors/
```

## Cache

Flux caches compiled bytecode under `target/flux/` using `.fxc` files. The cache is invalidated if
- the source file changes
- the compiler version changes
- any imported module changes

To clear the cache:

```
rm -rf target/flux
```

## Tests

```
cargo test
```

Run a single test:

```
cargo test test_builtin_len
```
