# What's New in Flux v0.0.2

Flux v0.0.2 significantly expands the language with new operators, the pipe operator, the Either type, lambda shorthand, string interpolation, a major builtin expansion, and module system enhancements.

## Highlights

- New operators: `<=`, `>=`, `%`, `&&`, `||`, and pipe `|>`
- Either type: `Left` / `Right` for explicit error handling
- Lambda shorthand: `\x -> expr`, `\(a, b) -> a + b`
- String interpolation: `"Hello, #{name}!"`
- Module enhancements: forward references, module constants, qualified imports, cycle detection
- 35 builtins (up from ~6): arrays, strings, hash maps, numeric, type checks
- Improved diagnostics: central error code registry, AST spans, colorized runtime errors

## New Language Features

### Operators

**Comparison:** `<=`, `>=`

```flux
print(5 <= 5)    // true
print(3 >= 4)    // false
```

**Modulo:** `%`

```flux
print(10 % 3)    // 1
print(7 % 2)     // 1   (odd check)
```

**Logical (short-circuiting):** `&&`, `||`

```flux
print(true && false)    // false (right side not evaluated if left is false)
print(false || true)    // true  (right side not evaluated if left is true)
```

Precedence: `&&` binds tighter than `||`, both lower than comparison.

### Pipe Operator

`a |> f(b)` desugars to `f(a, b)` at parse time. The left side becomes the first argument.

```flux
let result = [1, 2, 3, 4, 5, 6]
    |> filter(\x -> x % 2 == 0)
    |> map(\x -> x * x)
    |> fold(0, \(acc, x) -> acc + x)

print(result)    // 56
```

Pipe has the lowest precedence, so `a + b |> f` means `f(a + b)`.

### Either Type

`Left` and `Right` enable explicit error handling with pattern matching. They mirror `Some`/`None` but carry a value on both sides.

```flux
fn divide(a, b) {
    if b == 0 {
        Left("division by zero")
    } else {
        Right(a / b)
    }
}

match divide(10, 2) {
    Right(v) -> print("Result: " + to_string(v)),    // Result: 5
    Left(e)  -> print("Error: " + e),
}

match divide(10, 0) {
    Right(v) -> print("Result: " + to_string(v)),
    Left(e)  -> print("Error: " + e),                // Error: division by zero
}
```

`Left` and `Right` are keywords — no import needed.

### Lambda Shorthand

```flux
// Single parameter — no parens needed
let double = \x -> x * 2
print(double(5))    // 10

// Multiple parameters
let add = \(a, b) -> a + b
print(add(3, 4))    // 7

// Used inline with higher-order functions
let evens = filter([1,2,3,4,5,6], \x -> x % 2 == 0)
```

### String Interpolation

Embed any expression inside `#{ }`:

```flux
let name  = "Flux"
let score = 99

print("Language: #{name}")                            // Language: Flux
print("Score: #{score}, doubled: #{score * 2}")       // Score: 99, doubled: 198
print("2 + 2 = #{2 + 2}")                             // 2 + 2 = 4
```

## Module System Enhancements

### Forward References

Functions and modules can reference names defined later in the same file or module. No manual ordering required:

```flux
fn even(n) { if n == 0 { true } else { odd(n - 1) } }
fn odd(n)  { if n == 0 { false } else { even(n - 1) } }
```

### Module Constants

Compile-time constants defined inside modules:

```flux
module Config {
    let MAX_SIZE = 100
    let VERSION  = "2.0"
}

print(Config.MAX_SIZE)    // 100
```

Constants are evaluated at compile time. Circular constant dependencies are detected as `E044`.

### Qualified Imports and Aliases

```flux
import Modules.Utils.Math as M
print(M.square(5))    // 25
```

Multi-level module paths are fully supported. Cycle detection raises `E021` at compile time.

### Duplicate Module Detection

Importing the same module from two different roots raises `E027`.

### `--roots-only` Flag

Skip default module root resolution and use only the paths specified via `--root`:

```bash
cargo run -- --root lib/ --roots-only main.flx
```

## Builtins

35 total builtins (up from ~6). All are available without imports.

### Array

| Builtin | Description |
|---------|-------------|
| `concat(a, b)` | Combine two arrays |
| `reverse(arr)` | Reverse an array |
| `contains(arr, elem)` | Check if element is present |
| `slice(arr, start, end)` | Sub-array |
| `sort(arr)` | Sort ascending |

### String

| Builtin | Description |
|---------|-------------|
| `split(s, delim)` | Split by delimiter → array |
| `join(arr, delim)` | Join array with separator |
| `trim(s)` | Remove leading/trailing whitespace |
| `upper(s)` | Uppercase |
| `lower(s)` | Lowercase |
| `chars(s)` | String → array of characters |
| `substring(s, start, end)` | Slice a string |

### Hash Maps

| Builtin | Description |
|---------|-------------|
| `keys(h)` | All keys → array |
| `values(h)` | All values → array |
| `has_key(h, k)` | Check key existence |
| `merge(h1, h2)` | Combine two maps (h2 wins conflicts) |
| `delete(h, k)` | Remove a key |

### Numeric

`abs(n)`, `min(a, b)`, `max(a, b)`

### Type Checks

`type_of(x)`, `is_int(x)`, `is_float(x)`, `is_string(x)`, `is_bool(x)`, `is_array(x)`, `is_hash(x)`, `is_none(x)`, `is_some(x)`

```flux
print(type_of(42))           // "Int"
print(is_string("hello"))    // true
print(is_none(None))         // true
```

## Diagnostics & Tooling

### Central Error Code Registry

All error codes are now defined as stable constants in `compiler_errors.rs` and registered in `registry.rs`. Every error has a code, description, and optional hint template. See [error_codes.md](../internals/error_codes.md) for the full catalog.

### AST Spans

All AST nodes now carry source position information (`Span { start, end }`), enabling precise `line:col` locations and caret highlighting in error messages.

### Colorized Runtime Errors

Runtime errors show structured output with a code frame, error message, and hint — colorized by default, `NO_COLOR=1` to disable.

### VM Trace (`--trace`)

```bash
cargo run -- examples/basics/fibonacci.flx --trace
```

Prints each VM instruction as it executes. Useful for debugging compiler output.

## Compatibility Notes

- All v0.0.1 programs continue to work without changes.
- New operators follow standard precedence: `&&` tighter than `||`, `|>` is lowest.
- `Left` and `Right` are now reserved keywords — do not use as variable names.
