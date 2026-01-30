# Flux

A small, functional language with a custom bytecode VM.

## Current Features

- **Functions**: `fun` declarations, closures, higher-order functions, forward references, mutual recursion
- **Immutability**: `let` bindings are immutable; reassignment is rejected
- **Scoping**: lexical scoping, closures, and free variables
- **Modules**: static, qualified namespaces (`module Name { ... }`), public by default, `_private` hidden; module names must start uppercase
- **Imports**: top-level only, explicit qualified access, aliases supported (Haskell-style); collisions are errors; cycles rejected
- **Data types**: integers, floats, booleans, strings, `None`/`Some`
- **Collections**: arrays and hash maps, indexing with `[]`
- **Control flow**: `if` / `else`, `return`
- **Builtins**: `print`, `len`, `first`, `last`, `rest`, `push`
- **Diagnostics**: errors with codes, file/line/column, caret highlighting; multi-line spans supported
- **VM trace**: `--trace` instruction/stack/locals logging
- **Linter**: unused vars/params/imports, shadowing, naming style
- **Formatter**: `flux fmt` (indentation-only, preserves comments)
- **Bytecode cache**: `.fxc` cache with dependency hashing and inspection tools (debug info included)

## Running Flux

```
cargo run -- path/to/file.flx
cargo run -- --verbose run path/to/file.flx
```

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

Run all examples (passes extra flags to each run):

```
scripts/run_examples.sh --all --no-cache
```

## Basic Examples

```flux
// basics/print.flx
print("hello world");
print(42);
print(true);
print(false);
```

```flux
// basics/arithmetic.flx
print(1 + 2);
print(10 - 3);
print(4 * 5);
print(15 / 3);
print(2 + 3 * 4);
```

```flux
// basics/prefix.flx
print(-5);
print(-10);
print(-(-5));
print(!true);
print(!false);
print(!!true);
```

```flux
// basics/comparison.flx
print(1 < 2);
print(2 < 1);
print(2 > 1);
print(1 > 2);
print(1 == 1);
print(1 == 2);
print(1 != 2);
print(1 != 1);
```

```flux
// basics/variables.flx
let x = 5;
print(x);
let y = 10;
print(y);
print(x + y);
let name = "hello";
print(name);
let flag = true;
print(flag);
```

```flux
// basics/complex_expr.flx
let a = 5;
let b = 10;
let c = 2;
print(a * a);
print(a * b);
print(b * b);
print(a < b);
print(a > b);
let result = (a + b) * c + 12;
print(result);
```

```flux
// basics/strings.flx
let greeting = "hello";
let target = " world";
print(greeting + target);
let lang = "flux";
let desc = " is awesome";
print(lang + desc);
print("a" + "b" + "c");
```

```flux
// basics/if_else.flx
if true {
    print("yes");
};
if false {
    print("should not print");
} else {
    print("no");
};
let x = 5;
if x > 0 {
    print("positive");
} else {
    print("negative");
};
let a = 10;
let b = 10;
if a == b {
    print("equal");
} else {
    print("not equal");
};
let max = if 10 > 5 { 10; } else { 5; };
print(max);
```

```flux
// basics/arrays_basic.flx
let arr = [1, 2, 3];
print(arr);
let empty = [];
print(empty);
let nums = [1, 2, 3, 4, 5];
print(nums);
let mixed = ["hello", 42, true];
print(mixed[0]);
print(mixed[1]);
print(mixed[2]);
```

```flux
// basics/array_builtins.flx
let arr = [1, 2, 3, 4, 5];
print(len(arr));
print(len([]));
print(first(arr));
print(last(arr));
print(rest(arr));
print(push(arr, 6));
print(first([]));
print(last([]));
print(rest([]));
```

```flux
// basics/hash_basic.flx
let h = {"a": 1, "b": 2, "c": 3};
print(h["a"]);
print(h["b"]);
print(h["c"]);
let nums = {1: "hello", 2: "world"};
print(nums[1]);
print(nums[2]);
```

```flux
// basics/fibonacci.flx
fun fib(n) {
    if n < 2 {
        n;
    } else {
        fib(n - 1) + fib(n - 2);
    };
}
print(fib(0));
print(fib(1));
print(fib(2));
print(fib(3));
print(fib(4));
print(fib(5));
print(fib(6));
print(fib(10));
```

```flux
// basics/array_hash_combo.flx
let users = [
    {"name": "Alice", "age": 25},
    {"name": "Bob", "age": 30}
];
print(users[0]["name"]);
print(users[1]["name"]);
print(users[0]["age"]);
print(users[1]["age"]);
let skills = {
    "languages": ["python", "rust", "go"],
    "count": 3
};
print(skills["languages"][0]);
print(skills["languages"][1]);
print(skills["count"]);
```

```flux
// basics/array_iteration.flx
fun printAll(arr) {
    if len(arr) == 0 {
        print("done");
    } else {
        print(first(arr));
        printAll(rest(arr));
    };
}
printAll([1, 2, 3, 4, 5]);
fun sum(arr) {
    if len(arr) == 0 {
        0;
    } else {
        first(arr) + sum(rest(arr));
    };
}
print(sum([1, 2, 3, 4, 5]));
fun count(arr) {
    if len(arr) == 0 {
        0;
    } else {
        1 + count(rest(arr));
    };
}
print(count([10, 20, 30]));
```

## Error Examples

```flux
// Qualified access without an import.
print(Modules.Data.MyFile.value()); // MODULE NOT IMPORTED
```

```flux
// Alias replaces the original qualifier.
import Modules.Data.MyFile as MyFile
Modules.Data.MyFile.value(); // MODULE NOT IMPORTED
```

```flux
// Import cycle across module files.
import ModuleGraph.ModuleGraphCycleA
ModuleGraph.ModuleGraphCycleA.value(); // IMPORT CYCLE (E035)
```

More error-triggering examples live under `examples/Errors/`.

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
cargo test runtime::vm::tests::test_builtin_len
```
