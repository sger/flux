# Modules in Flux

This document describes the current module system behavior in Flux.

## Quick Start

### Module file

```flux
// examples/Modules/Analytics/Rules.flx
module Modules.Analytics.Rules {
    let PASSING_SCORE = 60

    fn passing_score() {
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

### Inside a Module Body

The module body validator (`src/bytecode/compiler/statement.rs`) whitelists exactly six statement kinds:

| Statement | Description | Since |
|-----------|-------------|-------|
| `Statement::Function` | `fn` / `public fn` declarations | original |
| `Statement::Data` | `data` / `public data` ADT declarations | Phase 2 (Proposal 0151) |
| `Statement::Class` | `class` / `public class` type class declarations | Phase 1 (Proposal 0151) |
| `Statement::Instance` | `instance` / `public instance` declarations | Phase 1 (Proposal 0151) |
| `Statement::Import` | `import` inside module body | Phase 1 (Proposal 0151) |
| `Statement::EffectDecl` | `effect` declarations | Phase 4a-prereq (Proposal 0151) |

Any other statement triggers `INVALID_MODULE_CONTENT`.

```flux
module Flow.Logger {
    // All six kinds in one module:
    import Flow.Console as Console

    effect LogLevel {
        level: () -> Int
    }

    data Handle { StdoutHandle, FileHandle(String) }

    class Logger<h> {
        fn log(hnd: h, msg: String) -> Unit with Console
    }

    instance Logger<Handle> {
        fn log(hnd, msg) with Console {
            perform Console.print(msg)
        }
    }

    public fn create() -> Handle {
        StdoutHandle
    }
}
```

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

Flux exports only members marked `public`:
- `public fn` / `public class` / `public instance` / `public data` are externally accessible
- plain `fn` / `class` / `instance` / `data` are private to the module
- `_name` remains private by convention as well

```flux
module Demo.PrivateTest {
    fn helper(x) { x * 2 }

    public fn run(x) {
        helper(x) + 1
    }
}
```

External access like `Demo.PrivateTest.helper(5)` is rejected with `E011 PRIVATE MEMBER`.

### Visibility Rules for Type Classes (Proposal 0151)

- **E450**: A `public instance` of a private class is rejected.
- **E451**: A `public class` whose method signatures reference a private data type is rejected.
- **E455**: A `public instance` whose head type is a private data type is rejected.

```flux
module Flow.Example {
    // Private class — cannot have public instances
    class Internal<a> {
        fn check(x: a) -> Bool
    }

    // Public class — instances may be public
    public class Visible<a> {
        fn show(x: a) -> String
    }

    // OK: public instance of public class
    public instance Visible<Int> {
        fn show(x) { to_string(x) }
    }
}
```

## Module-Level `let` Bindings

Module-level constants/bindings are supported:

```flux
module Modules.Analytics.Transforms {
    let SCORE_SCALE = 2
    let shout_prefix = "["

    public fn double(x) {
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
fn a() { 42 }
fn b(x) { x * 2 }
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

## Module-Scoped Type Classes and Effects (Proposal 0151)

Modules are the namespace boundary for type classes. Classes get a globally unique identity `ClassId = (ModulePath, ClassName)`, so two modules can define classes with the same short name without collision.

### Importing and Using Module-Scoped Classes

```flux
import Flow.Comparable as Comparable

// Class methods are called through the module alias
let equal = Comparable.same(1, 2)
```

### Selective Imports with `exposing`

```flux
import Flow.Comparable exposing (..)
// Now `same` is available unqualified
let equal = same(1, 2)

import Flow.Comparable exposing (same)
// Only `same` is unqualified; other methods require Comparable.method
```

### Effects Inside Modules

Effect declarations can live inside module bodies (Phase 4a-prereq). The effect is referenced by its bare name from any module that imports it:

```flux
module Flow.Console {
    effect Console {
        print: String -> ()
    }
}

// In another module:
import Flow.Console as Console

fn greet(name: String) with Console {
    perform Console.print("Hello " ++ name)
}
```

### Effectful Class Methods

Class methods can declare an effect floor — a minimum set of effects that every instance must provide:

```flux
module Flow.Logger {
    public class Logger<h> {
        fn log(hnd: h, msg: String) -> Unit with Console  // floor = {Console}
    }
}
```

Instance methods must satisfy the floor (can add effects, cannot remove them):

```flux
// OK: matches the floor exactly
instance Logger<StdoutHandle> {
    fn log(hnd, msg) with Console {
        perform Console.print(msg)
    }
}

// OK: superset of floor — adds AuditLog
instance Logger<AuditHandle> {
    fn log(hnd, msg) with Console, AuditLog {
        perform AuditLog.record(msg)
        perform Console.print(msg)
    }
}

// COMPILE ERROR E452: missing Console — violates the floor
instance Logger<NullHandle> {
    fn log(hnd, msg) { () }
}
```

Different instances of the same class can have different effect rows. The compiler propagates the correct row to each call site through type-directed dispatch.

### Row-Polymorphic Class Methods

Class methods can use row variables for effect polymorphism:

```flux
module Flow.Foldable {
    public class Foldable<f> {
        fn fold<a, b>(
            xs: f<a>,
            init: b,
            step: (b, a) -> b with |e
        ) -> b with |e
    }
}
```

The row variable `|e` is instantiated per call site. A pure callback produces a pure fold; an effectful callback propagates its effects through the fold.

### Orphan Rule

An instance `instance C<T>` in module `M` is legal only if:
- `M` defines class `C`, **or**
- `M` defines the head type `T`

This prevents conflicting instances across modules and keeps the `.fxc` cache sound.

## Recommended Structure

- Put reusable code in module files under a root like `examples/Modules/...` or `src/...`.
- Keep import list at the top of every file.
- Use aliases for long module paths.
- Use `_name` for internal helpers.
- Use module-level `let` for constants and config values.
- Define type classes and their core instances in dedicated `Flow.*` modules.
- Place orphan-rule-compliant instances near the data type definition.
