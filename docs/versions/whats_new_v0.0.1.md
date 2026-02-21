# What's New in Flux v0.0.1

Flux v0.0.1 is the initial release — a working bytecode compiler and VM with a functional core, module system, and developer tooling.

## Highlights

- Bytecode compiler + stack-based VM, written in Rust
- Immutable `let` bindings, functions, closures, and pattern matching
- Module system with imports, aliases, and forward references
- Compiler diagnostics with stable error codes
- Linter, formatter, and bytecode cache out of the box

## Language Features

### Types

- **Integers** — `42`, `-7`
- **Floats** — `3.14`, `-0.5`
- **Booleans** — `true`, `false`
- **Strings** — `"hello"` with interpolation: `"Hi #{name}!"`
- **None / Some** — built-in option type

### Operators

- Arithmetic: `+`, `-`, `*`, `/`
- Comparison: `==`, `!=`, `<`, `>`
- Unary: `-x`, `!x`
- String concatenation: `+`

### Variables and Functions

All bindings are immutable.

```flux
let x = 42
let name = "Flux"

fn add(x, y) { x + y }
fn double(x) { x * 2 }
```

### Closures

```flux
fn make_counter(start) {
    fn count() { start + 1 }
    count
}
```

### If / Else

```flux
if x > 0 { "positive" } else { "non-positive" }
```

### Pattern Matching

```flux
fn describe(val) {
    match val {
        0       -> "zero",
        Some(x) -> "got " + to_string(x),
        None    -> "nothing",
        _       -> "other",
    }
}
```

### String Interpolation

```flux
let score = 99
print("Score: #{score}")         // Score: 99
print("Double: #{score * 2}")    // Double: 198
```

### Module System

```flux
// Modules/Math.flx
module Modules.Math {
    fn square(x) { x * x }
    fn _helper(x) { x }   // private: underscore prefix
}

// main.flx
import Modules.Math as M
print(M.square(4))   // 16
```

- Module names must be PascalCase
- `_underscore` prefix = private member
- Forward references within a module are allowed
- Import cycles are detected at compile time

## Builtins

| Category | Functions |
|----------|-----------|
| **I/O** | `print`, `to_string` |
| **Numeric** | `abs`, `min`, `max` |
| **Strings** | `len`, `split`, `join`, `trim`, `upper`, `lower` |
| **Arrays** | `push`, `concat`, `reverse`, `contains`, `slice`, `sort` |
| **Type checks** | `type_of`, `is_int`, `is_float`, `is_string`, `is_bool`, `is_array`, `is_hash`, `is_none`, `is_some` |

## Diagnostics & Tooling

- Stable error codes (`E001`–`E077`) for compiler and parser errors
- Source snippets with caret highlighting and inline suggestions
- Linter: unused bindings, shadowing, unused parameters, dead code
- Formatter: indentation normalization
- Bytecode cache: `.fxc` files under `target/flux/`, invalidated on source change
- `--trace`: print VM instruction trace
- `--verbose`: show cache hit/miss/store status

## CLI

```bash
flux <file.flx>                 Run a program
flux tokens <file.flx>          Show token stream
flux bytecode <file.flx>        Show compiled bytecode
flux lint <file.flx>            Run linter
flux fmt [--check] <file.flx>   Format source
flux cache-info <file.flx>      Inspect bytecode cache
```

## Bytecode VM

Stack-based VM with call frames, closures, and a compact instruction set. Programs compile to portable `.fxc` bytecode.
