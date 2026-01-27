# Flux

A small, functional language with a custom bytecode VM.

## Current Features

- **Functions**: `fun` declarations, closures, higher-order functions
- **Immutability**: `let` bindings are immutable; reassignment is rejected
- **Scoping**: lexical scoping, closures, and free variables
- **Modules**: namespaced modules (`module Name { ... }`), public by default, `_private` hidden
- **Imports**: top-level only, no semicolons, collisions are errors
- **Data types**: integers, floats, booleans, strings, null
- **Collections**: arrays and hash maps, indexing with `[]`
- **Control flow**: `if` / `else`, `return`
- **Builtins**: `print`, `len`, `first`, `last`, `rest`, `push`
- **Diagnostics**: human-friendly errors with codes, file/line/column, caret highlighting, and actionable hints
- **Linter**: unused vars/params/imports, shadowing, naming style
- **Formatter**: `flux fmt` (indentation-only, preserves comments)
- **Bytecode cache**: `.fxc` cache with dependency hashing and inspection tools

## Running Flux

```
cargo run -- run path/to/file.flx
cargo run -- --verbose run path/to/file.flx
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
