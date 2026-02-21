# Chapter 7 — Modules

> Full examples: [`examples/Modules/`](../../examples/Modules/), [`examples/imports/`](../../examples/imports/), [`examples/advanced/using_modules.flx`](../../examples/advanced/using_modules.flx)

## Declaring a Module

A module groups related functions under a qualified namespace. Module names must be PascalCase and must match the file path.

```flux
// File: examples/Modules/Math.flx
module Modules.Math {
    fn square(x) { x * x }
    fn cube(x)   { x * square(x) }

    // Private: underscore prefix — not accessible outside the module
    fn _helper(x) { x * 2 }

    // Can call _helper from inside the module
    fn doubled(x) { _helper(x) }
}
```

Forward references are allowed — functions may call each other regardless of declaration order:

```flux
module Modules.Math {
    fn quadruple(x) { double(double(x)) }  // double defined below — OK
    fn double(x)    { x * 2 }
}
```

> See [`examples/Modules/Math.flx`](../../examples/Modules/Math.flx).

## Importing a Module

```flux
import Modules.Math

print(Modules.Math.square(5))  // 25
print(Modules.Math.cube(3))    // 27
```

### Import with alias

```flux
import Modules.Math as M

print(M.square(5))  // 25
print(M.cube(3))    // 27
```

> See [`examples/imports/alias_import_script.flx`](../../examples/imports/alias_import_script.flx).

## Module Search Roots

The compiler resolves imports by searching module roots. The entry file's directory and `./src` are defaults. Add extra roots with `--root`:

```bash
# Resolve imports relative to examples/
cargo run -- --root examples examples/advanced/using_modules.flx

# Multiple roots
cargo run -- --root examples --root lib examples/advanced/using_list_module.flx
```

## Multi-File Projects

A module can span multiple files or group utilities logically:

```flux
// examples/Modules/StringUtils.flx
module Modules.StringUtils {
    fn capitalize(s) {
        upper(substring(s, 0, 1)) + substring(s, 1, len(s))
    }

    fn words(s) { split(s, " ") }
}
```

```flux
// main.flx
import Modules.StringUtils as SU

print(SU.capitalize("hello"))   // Hello
print(SU.words("foo bar baz"))  // [|foo, bar, baz|]
```

## Combining Modules with Pipelines

Modules integrate naturally with the pipe operator:

```flux
import Modules.FunctionalUtils as FU

let nums = [|1, 2, 3, 4, 5|]

let result = nums
    |> FU.filter(\x -> x % 2 == 0)
    |> FU.map(\x -> x * x)
    |> FU.reduce(0, \(acc, x) -> acc + x)

print(result)  // 4 + 16 = 20
```

> See [`examples/advanced/using_modules.flx`](../../examples/advanced/using_modules.flx).

## Import Cycle Detection

Circular imports are detected at compile time and reported as error `E035`. The module graph is resolved in topological order before compilation begins.

## Rules Summary

| Rule | Detail |
|------|--------|
| Module names | PascalCase, must match file path |
| Private members | Prefix with `_` |
| Forward references | Allowed within a module |
| Import cycles | Compile-time error `E035` |
| `import` position | Top-level only (not inside functions) |

## Next

Continue to [Chapter 8 — Testing](08_testing.md).
