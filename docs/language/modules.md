# Modules in Flux

This document describes the current module system behavior in Flux.

## Quick Start

### Module file

```flux
// examples/Modules/Analytics/Rules.flx
module Modules.Analytics.Rules {
    let PASSING_SCORE = 60

    fun passing_score() {
        PASSING_SCORE
    }
}
```

### Script that imports the module

```flux
// examples/Modules/advanced_map_filter_fold_pipeline.flx
import Modules.Analytics.Rules as Rules

print(Rules.passing_score())
```

Run it from repo root:

```bash
cargo run -- --root examples examples/Modules/advanced_map_filter_fold_pipeline.flx
```

## Core Concepts

- A file with a `module ... { ... }` declaration is a **module file**.
- A file without a module declaration is a **script file** (entrypoint).
- Scripts can import modules.
- Modules can import other modules.
- Scripts are not importable by other files.

## Naming Rules

Module names and aliases are validated segment-by-segment.

Valid segment rules:
- first character must be uppercase ASCII (`A-Z`)
- remaining characters must be ASCII alphanumeric (`A-Z`, `a-z`, `0-9`)

Implications:
- `Modules.Analytics.Transforms` is valid.
- `HelloWorld` is valid.
- `HELLO_WORLD` is invalid (underscore not allowed).
- alias `as MyAlias` is valid.
- alias `as myAlias` is invalid.

## File Path Mapping

Dotted names map to directories + file name:

- `Modules.Analytics.Transforms` -> `Modules/Analytics/Transforms.flx`

The module declaration must match the file path under a configured module root.
Otherwise you get `E024 MODULE PATH MISMATCH`.

## Module Roots

Import resolution searches module roots.

CLI options:
- `--root <path>` to add roots
- `--roots-only` to use only explicitly provided roots

Examples:

```bash
cargo run -- --root examples examples/Modules/advanced_map_filter_fold_pipeline.flx
cargo run -- --root examples --roots-only examples/Modules/advanced_map_filter_fold_pipeline.flx
```

## What a Module File May Contain

At top level, module files may contain:
- import statements
- exactly one module declaration

Other top-level statements in a module file are rejected with `E028 INVALID MODULE FILE`.
Multiple module declarations are rejected with `E023 MULTIPLE MODULES`.

## Imports and Scope

### Scope rules

- Imports are allowed at top-level scope.
- Imports inside functions are rejected (`E017 IMPORT SCOPE`).

### Order nuance (current behavior)

Imports are intended to be at the top, but in script files top-level imports may still compile even if placed after other top-level statements.
Best practice is still: keep imports first.

### Aliasing

```flux
import Modules.Analytics.Transforms as T

let doubled = map([1, 2, 3], T.double)
```

When aliased, use the alias (`T.double`), not the full module qualifier.

## Visibility and Private Members

Flux uses naming convention-based privacy:
- names prefixed with `_` are private to the module
- other members are public

```flux
module Demo.PrivateTest {
    fun _private(x) { x * 2 }

    fun public(x) {
        _private(x) + 1
    }
}
```

External access like `Demo.PrivateTest._private(5)` is rejected with `E011 PRIVATE MEMBER`.

## Module-Level `let` Bindings

Module-level constants/bindings are supported:

```flux
module Modules.Analytics.Transforms {
    let SCORE_SCALE = 2
    let shout_prefix = "["

    fun double(x) {
        x * SCORE_SCALE
    }
}
```

Notes:
- Both `UPPER_CASE` and `lower_case` names are valid.
- Semicolons are optional.

## Functions and `return`

Flux returns the last expression by default.
`return` is optional unless you want explicit early-exit style.

```flux
fun a() { 42 }
fun b(x) { x * 2 }
```

Both return values without explicit `return`.

## Modules + map/filter/fold Pattern

This is supported and works with module callbacks:

```flux
let total =
  scores
  |> map(T.double)
  |> filter(T.is_passing)
  |> fold(0, T.sum_step)
```

## Common Errors

- `E008 INVALID MODULE NAME`: bad module/alias segment (lowercase first char, underscore, etc.)
- `E018 IMPORT NOT FOUND`: module could not be found under roots
- `E022 SCRIPT NOT IMPORTABLE`: trying to import a script file
- `E024 MODULE PATH MISMATCH`: declaration doesn’t match file path
- `E026 INVALID MODULE ALIAS`: alias naming rule violation
- `E011 PRIVATE MEMBER`: attempted access to `_private` symbol from outside module
- `E028 INVALID MODULE FILE`: module file contains disallowed top-level statements

## Recommended Structure

- Put reusable code in module files under a root like `examples/Modules/...` or `src/...`.
- Keep import list at the top of every file.
- Use aliases for long module paths.
- Use `_name` for internal helpers.
- Use module-level `let` for constants and config values.
