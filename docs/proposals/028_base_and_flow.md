# Proposal 028: Base + Flow — Auto-Imported Prelude and Standard Library

**Status:** Proposed
**Priority:** High
**Created:** 2026-02-12
**Related:** Proposal 003 (Flow Stdlib), Proposal 008 (Builtins Module Architecture), Proposal 017 (Persistent Collections + GC), Proposal 026 (Concurrency Model)

## Summary

Replace the hard-coded builtin system with a **`Base` module** — a privileged module that is auto-imported into every Flux module and script. Functions in `Base` look and feel like language primitives but participate in the module system. Users can exclude specific `Base` functions. The standard library grows as **`Flow`** modules — data flows through Flux.

This follows the Elixir `Kernel` / Haskell `Prelude` pattern: a curated set of essential functions available everywhere, with everything else one import away.

## Motivation

### Problem 1: Builtins Bypass the Module System

Current builtins are a parallel universe. They're registered by index in the compiler, dispatched via `OpGetBuiltin`, and completely invisible to the module system:

```rust
// Compiler::new_with_interner() — hard-coded indices
symbol_table.define_builtin(0, intern("print"));
symbol_table.define_builtin(1, intern("len"));
// ... 40 more, indices MUST match BUILTINS array
```

This creates a brittle coupling: adding a builtin requires synchronized changes in the `BUILTINS` array (runtime) and `define_builtin` calls (compiler). Index mismatches cause silent wrong-function-called bugs.

### Problem 2: No User Control Over Global Names

Every builtin is unconditionally global. Users cannot:
- Shadow a builtin with a local definition without a shadowing warning
- Exclude a builtin they don't want (e.g., a `print` that conflicts with their domain)
- Discover what's available without reading Rust source

In Elixir, you can write `import Kernel, except: [length: 1]`. In Haskell, `import Prelude hiding (map)`. Flux has no equivalent.

### Problem 3: No Clear Boundary Between "Essential" and "Library"

All 42 builtins have equal status. But `print` and `len` are fundamentally different from `starts_with` and `slice`. There's no layering that says "these are the core vocabulary" vs "these are convenience functions." This makes the language feel ad-hoc as builtins accumulate.

### Problem 4: Growing Pains

Proposals 017 (GC/collections) and 026 (concurrency) will add 10+ new builtins each (`hd`, `tl`, `list`, `put`, `get`, `spawn`, `send`, `ask`, `await`, etc.). Without a proper module architecture, the flat builtin list will become unwieldy. The index-coupling problem gets worse with every addition.

## Design

### The Base Module

`Base` is a **synthetic module** — it doesn't correspond to a `.flx` file on disk. It's backed by Rust implementations but presents itself to the module system as a regular module. The compiler auto-injects an implicit import at the start of every module and script:

```flux
// What the user writes:
module MyApp {
    fun greet(name) {
        print("Hello, #{name}")
    }
}

// What the compiler sees:
import Base  // <-- injected automatically
module MyApp {
    fun greet(name) {
        print("Hello, #{name}")  // resolves to Base.print
    }
}
```

### Base Contents

`Base` contains **only** functions that meet at least one of these criteria:
1. **Used in nearly every program** (print, len, type_of)
2. **Cannot be implemented in Flux** (require VM access or native performance)
3. **Are foundational building blocks** (map, filter, fold — used to build everything else)

```
Base
├── I/O
│   └── print(...)
│
├── Collections (polymorphic)
│   ├── len(x)              -- String, Array, Hash, List, Map
│   ├── first(x)            -- Array, List
│   ├── last(x)             -- Array
│   ├── rest(x)             -- Array, List
│   ├── push(arr, elem)     -- Array
│   ├── concat(a, b)        -- Array
│   ├── reverse(x)          -- Array, List
│   ├── contains(x, elem)   -- Array, List, String
│   ├── slice(arr, s, e)    -- Array
│   ├── sort(arr, order?)   -- Array
│   ├── keys(h)             -- Hash/Map
│   ├── values(h)           -- Hash/Map
│   ├── has_key(h, k)       -- Hash/Map
│   ├── merge(h1, h2)       -- Hash/Map
│   └── delete(h, k)        -- Hash/Map
│
├── Higher-Order
│   ├── map(collection, fn)
│   ├── filter(collection, pred)
│   └── fold(collection, init, fn)
│
├── String
│   ├── split(s, delim)
│   ├── join(arr, delim)
│   ├── trim(s)
│   ├── upper(s)
│   ├── lower(s)
│   ├── starts_with(s, prefix)
│   ├── ends_with(s, suffix)
│   ├── replace(s, old, new)
│   ├── chars(s)
│   └── substring(s, start, end)
│
├── Numeric
│   ├── abs(n)
│   ├── min(a, b)
│   └── max(a, b)
│
├── Type
│   ├── type_of(x)
│   ├── is_int(x)
│   ├── is_float(x)
│   ├── is_string(x)
│   ├── is_bool(x)
│   ├── is_array(x)
│   ├── is_hash(x)
│   ├── is_none(x)
│   └── is_some(x)
│
└── Conversion
    └── to_string(x)
```

**42 functions total** — exactly what exists today, just properly housed.

### Future Base Additions

As new proposals land, Base grows:

```
// Proposal 017: Persistent Collections
Base.hd(list)
Base.tl(list)
Base.list(...)
Base.is_list(x)
Base.is_map(x)
Base.put(map, k, v)
Base.get(map, k)

// Proposal 026: Concurrency (Layer 1 only)
Base.spawn(fn)
Base.send(actor, msg)
Base.receive()
```

Each addition is a single entry in the Base module definition — no index synchronization.

### Compiler Model: Prelude Injection

Base is **not** a normal module import. It is an implicit scope injection step that runs before any user code in every module/script:

1. Before compiling a module, the compiler injects all Base names into the scope
2. If the AST contains an explicit `import Base except [...]`, that **modifies** the injection step (exclusions are applied)
3. `import Base` alone is a no-op (it's already injected)
4. `import Base as X` is **forbidden** — Base is a prelude directive, not a regular module. Use `Base.name(...)` for qualified access.

This means `Base` is a reserved module name. User modules cannot be named `Base`.

### Excluding Base Functions

Users can exclude specific Base functions:

```flux
import Base except [print]

// print is no longer available
// print("hello")  // E006 UNDEFINED IDENTIFIER

// Define your own
fun print(x) {
    // custom logging
}
```

Syntax: `import Base except [name1, name2, ...]`

This is the **only** special syntax. `import Base as X` is rejected with a compile error. Everything else uses the existing module system.

### Qualified Access

Base functions can always be accessed with qualification:

```flux
import Base except [print]

fun my_print(x) {
    Base.print("[LOG] " + to_string(x))
}
```

### Shadowing Rules

Local definitions shadow Base functions without error:

```flux
// No warning — intentional override
fun len(x) {
    Base.len(x) + 1  // can still access original
}
```

**Resolution rule:** If a user defines `fun len(...)` and calls bare `len(...)`, the local definition wins. To call the Base version, use `Base.len(...)`. This is standard lexical scoping — the innermost binding wins.

This is different from shadowing a local variable (which triggers W001). Base functions are expected to be overridable.

**Optional lint (disabled by default):**

```
W011 SHADOWS BASE FUNCTION
  fun len(x) shadows Base.len
  ╰─ Hint: use Base.len to call the original
```

This lint helps catch accidental shadows. Disabled by default; enable in strict mode or via `--warn-shadow-base`.

### Standard Library (Explicit Import)

Everything beyond `Base` requires explicit import. The standard library is named **`Flow`** — data flows through Flux:

```
Flow.List      — take, drop, zip, flatten, any, all, find, foldl, foldr, reverse, append, ...
Flow.Option    — map, flat_map, unwrap_or, filter, ...
Flow.Either    — map, flat_map, fold, bimap, ...
Flow.Math      — sign, clamp, gcd, lcm, factorial, floor, ceil, round, sqrt, pow, ...
Flow.String    — repeat, is_blank, pad_left, pad_right, ...
Flow.Dict      — get_or, map_values, filter_values, from_pairs, ...
Flow.Func      — identity, compose, flip, constant, pipe, ...
```

Usage:

```flux
import Flow.List
import Flow.Option as Opt

let nums = list(1, 2, 3, 4, 5)   // persistent list (Proposal 017)
let result = nums
    |> List.take(3)
    |> map(\x -> x * 2)           // map is Base — no import needed
    |> List.find(\x -> x > 4)
    |> Opt.unwrap_or(0)
```

**Note:** `Flow.List` operates on persistent lists (cons cells from Proposal 017), not arrays. Use `list(...)` or `[h | t]` syntax to create lists. Array operations use Base builtins directly (`first`, `rest`, `slice`, `sort`, etc.).

**Key distinction:**
- `Base` — Rust-backed, auto-imported, ~42 functions
- `Flow.*` — written in Flux, explicit import, grows unbounded

### Flow Module Implementation

Flow modules are regular `.flx` source embedded directly in the compiler binary via `include_str!`. No filesystem dependency:

```rust
// src/runtime/flow.rs
pub const FLOW_LIST: &str = include_str!("../../lib/Flow/List.flx");
pub const FLOW_OPTION: &str = include_str!("../../lib/Flow/Option.flx");
pub const FLOW_EITHER: &str = include_str!("../../lib/Flow/Either.flx");
pub const FLOW_FUNC: &str = include_str!("../../lib/Flow/Func.flx");
// ...
```

The module resolver virtualizes these as if they exist under `lib/`:

```
lib/                 (virtual — embedded in binary, not on disk)
└── Flow/
    ├── List.flx
    ├── Option.flx
    ├── Either.flx
    ├── Math.flx
    ├── String.flx
    ├── Dict.flx
    └── Func.flx
```

When the module resolver encounters `import Flow.List`, it checks the virtual registry before searching the filesystem. This means:
- No `--root lib` needed — always available
- No filesystem dependency — single binary distribution
- Caching works normally — hash of virtual file contents + compiler version
- Users can override with their own `Flow/List.flx` on disk (local file wins over embedded)

## Implementation

### Phase 1: Base Module Infrastructure

Decouple builtins from index-based dispatch.

#### 1.1 Base Module Definition

Replace the `BUILTINS` array + `define_builtin` calls with a single declarative definition.

**Critical:** Builtin index assignment must be deterministic. `HashMap` iteration order is not stable across runs or platforms. Use a `Vec` for the canonical ordered registry and derive the lookup index from it:

```rust
// src/runtime/base.rs

/// The Base module — auto-imported into every Flux module.
pub struct BaseModule {
    entries: Vec<(&'static str, BuiltinFn)>,              // stable order — indices derived from position
    index: std::collections::HashMap<&'static str, u16>,  // name → index for O(1) lookup
}

impl BaseModule {
    pub fn new() -> Self {
        let entries = vec![
            // I/O
            ("print", builtins::util_ops::builtin_print as BuiltinFn),

            // Collections
            ("len", builtins::array_ops::builtin_len as BuiltinFn),
            ("first", builtins::array_ops::builtin_first as BuiltinFn),
            ("last", builtins::array_ops::builtin_last as BuiltinFn),
            // ... all 42 functions in deterministic declaration order
        ];

        let index = entries.iter().enumerate()
            .map(|(i, (name, _))| (*name, i as u16))
            .collect();

        Self { entries, index }
    }

    pub fn by_index(&self, i: u16) -> BuiltinFn {
        self.entries[i as usize].1
    }

    pub fn index_of(&self, name: &str) -> Option<u16> {
        self.index.get(name).copied()
    }

    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.entries.iter().map(|(n, _)| *n)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}
```

The `Vec` is the source of truth. Indices are positions in the Vec. The `HashMap` is a derived lookup cache built from the Vec in the constructor. Adding a builtin means appending to the Vec — existing indices never change.

#### 1.2 Compiler Integration

During module compilation, auto-inject Base bindings into the symbol table:

```rust
// In Compiler::new_with_interner()

// OLD: manual index registration
// symbol_table.define_builtin(0, intern("print"));
// symbol_table.define_builtin(1, intern("len"));
// ... 40 more lines

// NEW: iterate Base module (deterministic order guaranteed by Vec)
let base = BaseModule::new();
for (index, name) in base.names().enumerate() {
    let symbol = interner.intern(name);
    symbol_table.define_builtin(index, symbol);
}
```

The index-based OpGetBuiltin dispatch stays unchanged. The improvement is that the single source of truth is now `BaseModule` — no more dual registration, and index assignment is deterministic.

#### 1.3 Bytecode-Level Change (Optional, Phase 1)

Replace index-based `OpGetBuiltin` with name-based lookup:

```rust
// Current: OpGetBuiltin takes a u8 index
OpGetBuiltin, [idx]  // idx must match BUILTINS array position

// New: OpGetBuiltin takes a constant index pointing to the function name
OpGetBuiltin, [const_idx]  // const_idx points to interned name in constants
```

The VM resolves the name against the Base module at execution time. This eliminates index coupling entirely.

**Trade-off:** Slightly slower (hash lookup vs array index). Mitigate with a pre-resolved function pointer cache in the VM.

**Alternative:** Keep index-based dispatch but generate indices from BaseModule automatically. The compiler assigns indices based on insertion order in `BaseModule::new()`. This preserves O(1) dispatch while removing manual index management.

### Phase 2: Import Base / Except Syntax

#### 2.1 Parser Changes

Extend `import` statement parsing:

```rust
// Current import syntax:
// import ModuleName
// import ModuleName as Alias

// New addition:
// import Base except [name1, name2, ...]
Statement::Import {
    name: Symbol,
    alias: Option<Symbol>,
    except: Vec<Symbol>,    // NEW — only valid for Base
    span: Span,
}
```

The `except` field defaults to empty. The parser recognizes `except` as a contextual keyword after `import Base`.

#### 2.2 Compiler Changes

When compiling a module, check for Base import exclusions:

```rust
fn inject_core_bindings(&mut self, except: &[Symbol]) {
    let core = BaseModule::new();
    for (index, name) in core.names().enumerate() {
        let symbol = self.interner.intern(name);
        if !except.contains(&symbol) {
            self.symbol_table.define_builtin(index, symbol);
        }
    }
}
```

If a module has `import Base except [print]`, `print` is not registered. Any use of bare `print` triggers E006 UNDEFINED IDENTIFIER. `Base.print(...)` still works via qualified access.

#### 2.3 Qualified Access (Synthetic Module Resolution)

Enable `Base.function_name(...)` syntax. Base is resolved **at compile time without a file** — it is a synthetic module recognized by the module resolver:

```rust
// When the compiler sees `Base.print(...)`:
// 1. Module resolver recognizes "Base" as a reserved synthetic module path
//    (not a file path — no filesystem lookup)
// 2. Member lookup uses BaseModule registry: base.index_of("print") → Some(0)
// 3. Emit OpGetBuiltin with the resolved index
```

Implementation in the compiler's `Expression::MemberAccess` handling:

```rust
Expression::MemberAccess { object, member, .. } => {
    if let Expression::Identifier { name, .. } = object.as_ref() {
        if interner.resolve(*name) == "Base" {
            // Synthetic module — resolve from Base registry, not module graph
            let member_name = interner.resolve(*member);
            if let Some(idx) = base_module.index_of(member_name) {
                self.emit(OpGetBuiltin, &[idx as u8]);
                return Ok(());
            }
            return Err(/* E0xx: unknown Base function */);
        }
    }
    // ... normal module member access
}
```

This avoids confusion with normal module path resolution. `Base` never enters the module graph, is never searched on the filesystem, and has no `.flx` backing file.

### Phase 3: Flow Library Infrastructure

#### 3.1 Virtual Module Registry

Add a virtual module registry to the module resolver. When resolving `import Flow.X`, check the embedded sources before the filesystem:

```rust
// In module_graph.rs or module resolution
fn resolve_module(&self, name: &str) -> Option<ModuleSource> {
    // 1. Check virtual (embedded) modules first
    if let Some(source) = self.virtual_modules.get(name) {
        return Some(ModuleSource::Virtual { name, source });
    }

    // 2. Fall through to filesystem resolution (existing behavior)
    self.resolve_from_roots(name)
}
```

This means Flow modules are always available without any `--root` flag, and users can override any Flow module by placing their own `.flx` file in a project root.

#### 3.2 Flow Module Files

Ship `.flx` files with the compiler:

```flux
// lib/Flow/List.flx
// Operates on persistent lists (cons cells) from Proposal 017.
// Lists are built with [h | t] syntax and destructured with hd/tl.
module Flow.List {
    fun take(xs, n) {
        match xs {
            [h | t] -> if n > 0 { [h | take(t, n - 1)] } else { None },
            _ -> None,
        }
    }

    fun drop(xs, n) {
        match xs {
            [_ | t] -> if n > 0 { drop(t, n - 1) } else { xs },
            _ -> None,
        }
    }

    fun zip(xs, ys) {
        match xs {
            [x | xt] -> match ys {
                [y | yt] -> [[x, y] | zip(xt, yt)],
                _ -> None,
            },
            _ -> None,
        }
    }

    fun any(xs, pred) {
        match xs {
            [h | t] -> if pred(h) { true } else { any(t, pred) },
            _ -> false,
        }
    }

    fun all(xs, pred) {
        match xs {
            [h | t] -> if pred(h) { all(t, pred) } else { false },
            _ -> true,
        }
    }

    fun find(xs, pred) {
        match xs {
            [h | t] -> if pred(h) { Some(h) } else { find(t, pred) },
            _ -> None,
        }
    }

    fun foldl(xs, acc, f) {
        match xs {
            [h | t] -> foldl(t, f(acc, h), f),
            _ -> acc,
        }
    }

    fun foldr(xs, acc, f) {
        match xs {
            [h | t] -> f(h, foldr(t, acc, f)),
            _ -> acc,
        }
    }

    fun reverse(xs) {
        foldl(xs, None, \acc, h -> [h | acc])
    }

    fun append(xs, ys) {
        match xs {
            [h | t] -> [h | append(t, ys)],
            _ -> ys,
        }
    }

    fun flatten(xs) {
        match xs {
            [h | t] -> append(h, flatten(t)),
            _ -> None,
        }
    }

    fun flat_map(xs, f) {
        flatten(map(xs, f))
    }

    fun sum(xs) {
        foldl(xs, 0, \acc, x -> acc + x)
    }

    fun product(xs) {
        foldl(xs, 1, \acc, x -> acc * x)
    }

    fun count(xs, pred) {
        foldl(xs, 0, \acc, x -> if pred(x) { acc + 1 } else { acc })
    }

    fun each(xs, f) {
        match xs {
            [h | t] -> {
                f(h)
                each(t, f)
            },
            _ -> None,
        }
    }

    fun zip_with(xs, ys, f) {
        match xs {
            [x | xt] -> match ys {
                [y | yt] -> [f(x, y) | zip_with(xt, yt, f)],
                _ -> None,
            },
            _ -> None,
        }
    }

    fun intersperse(xs, sep) {
        match xs {
            [h | t] -> match t {
                [_ | _] -> [h | [sep | intersperse(t, sep)]],
                _ -> list(h),
            },
            _ -> None,
        }
    }

    fun nth(xs, n) {
        match xs {
            [h | t] -> if n == 0 { Some(h) } else { nth(t, n - 1) },
            _ -> None,
        }
    }

    fun from_array(arr) {
        to_list(arr)
    }
}
```

```flux
// lib/Flow/Option.flx
module Flow.Option {
    fun map(opt, f) {
        match opt {
            Some(x) -> Some(f(x)),
            _ -> None,
        }
    }

    fun flat_map(opt, f) {
        match opt {
            Some(x) -> f(x),
            _ -> None,
        }
    }

    fun unwrap_or(opt, default) {
        match opt {
            Some(x) -> x,
            _ -> default,
        }
    }

    fun unwrap_or_else(opt, f) {
        match opt {
            Some(x) -> x,
            _ -> f(),
        }
    }

    fun filter(opt, pred) {
        match opt {
            Some(x) -> if pred(x) { Some(x) } else { None },
            _ -> None,
        }
    }

    fun or_else(opt, f) {
        match opt {
            Some(_) -> opt,
            _ -> f(),
        }
    }

    fun zip(a, b) {
        match a {
            Some(x) -> match b {
                Some(y) -> Some([x, y]),
                _ -> None,
            },
            _ -> None,
        }
    }
}
```

```flux
// lib/Flow/Either.flx
module Flow.Either {
    fun map(either, f) {
        match either {
            Right(x) -> Right(f(x)),
            Left(e) -> Left(e),
        }
    }

    fun map_left(either, f) {
        match either {
            Left(e) -> Left(f(e)),
            Right(x) -> Right(x),
        }
    }

    fun flat_map(either, f) {
        match either {
            Right(x) -> f(x),
            Left(e) -> Left(e),
        }
    }

    fun unwrap_or(either, default) {
        match either {
            Right(x) -> x,
            Left(_) -> default,
        }
    }

    fun fold(either, on_left, on_right) {
        match either {
            Right(x) -> on_right(x),
            Left(e) -> on_left(e),
        }
    }

    fun to_option(either) {
        match either {
            Right(x) -> Some(x),
            Left(_) -> None,
        }
    }

    fun swap(either) {
        match either {
            Right(x) -> Left(x),
            Left(e) -> Right(e),
        }
    }

    fun is_right(either) {
        match either {
            Right(_) -> true,
            Left(_) -> false,
        }
    }

    fun is_left(either) {
        match either {
            Left(_) -> true,
            Right(_) -> false,
        }
    }

    fun bimap(either, f_left, f_right) {
        match either {
            Right(x) -> Right(f_right(x)),
            Left(e) -> Left(f_left(e)),
        }
    }
}
```

```flux
// lib/Flow/Func.flx
module Flow.Func {
    fun identity(x) { x }

    fun constant(x) {
        \_ -> x
    }

    fun compose(f, g) {
        \x -> f(g(x))
    }

    fun flip(f) {
        \a, b -> f(b, a)
    }

    fun times(n, f) {
        if n == 0 { None }
        else {
            f()
            times(n - 1, f)
        }
    }
}
```

#### 3.3 Flow Bytecode Cache

Flow modules compile like normal user modules. The existing bytecode cache applies — compiled `.fxc` files are keyed on content hash + compiler version. Virtual (embedded) sources get their cache under:

```
target/flux/
├── Flow.List.fxc
├── Flow.Option.fxc
├── Flow.Either.fxc
├── Flow.Math.fxc
├── Flow.String.fxc
├── Flow.Dict.fxc
└── Flow.Func.fxc
```

When the compiler version changes, all Flow caches auto-invalidate (embedded source content changes with each build). No special cache logic needed.

### Phase 4: Migrate Builtins to Base

Mechanical migration:

1. Remove `define_builtin()` calls from `Compiler::new_with_interner()`
2. Replace with `inject_core_bindings()` that reads from `BaseModule`
3. Keep all existing Rust implementations in `src/runtime/builtins/` unchanged
4. Keep `OpGetBuiltin` opcode unchanged
5. Update bytecode cache version

**Zero behavioral change.** The only difference is how the compiler discovers builtins — from a `BaseModule` struct instead of hard-coded calls.

## Architecture Diagram

```
┌─────────────────────────────────────────────────────┐
│                   User Code                         │
│                                                     │
│  import Flow.List                                    │
│  import Flow.Option as Opt                           │
│                                                     │
│  let nums = list(1, 2, 3, 4, 5)                    │
│  nums                                               │
│    |> map(\x -> x * 2)           ← Base (auto)     │
│    |> List.take(3)               ← Flow.List        │
│    |> List.find(\x -> x > 4)     ← Flow.List        │
│    |> Opt.unwrap_or(0)           ← Flow.Option      │
│                                                     │
└───────────────────┬─────────────────────────────────┘
                    │
        ┌───────────┴───────────┐
        │                       │
┌───────▼──────────┐   ┌───────▼──────────┐
│      Base        │   │   Flow Library   │
│  (Rust-backed)   │   │  (Flux source)   │
│                  │   │                  │
│  auto-imported   │   │  explicit import │
│  ~42 functions   │   │  grows unbounded │
│  OpGetBuiltin    │   │  regular modules │
│                  │   │                  │
│  print, len,     │   │  Flow.List       │
│  map, filter,    │   │  Flow.Option     │
│  fold, type_of,  │   │  Flow.Either     │
│  is_int, ...     │   │  Flow.Math       │
│                  │   │  Flow.Func       │
└──────────────────┘   │  Flow.String     │
                       │  Flow.Dict       │
                       └──────────────────┘
```

## Naming: Why `Base` and `Flow`

| Name | Inspiration | Rationale |
|------|-------------|-----------|
| `Base` | Haskell (`base`) | Conveys "foundation." Short. The base everything else builds on. |
| `Flow` | Flux ecosystem | Complements "Flux" — data *flows* through transformations. Gives the stdlib its own identity. |

Alternative considered: **`Kernel`** (Elixir). `Base` is more language-agnostic and avoids OS/systems connotations.

Alternative considered: **`Prelude`** (Haskell). `Base` is shorter and more intuitive for non-Haskell developers.

Alternative considered: **`Std`** (Rust, Go). `Flow` gives the stdlib a unique identity tied to the language's name and functional nature.

## What Stays in Base vs. Moves to Flow

### Decision Framework

A function belongs in **Base** if:
1. It requires VM access (print, map, filter, fold — need `RuntimeContext` for callbacks)
2. It operates on primitive types with no pure-Flux equivalent (type_of, is_int, to_string)
3. It needs native performance for correctness (sort, len on strings — Unicode)
4. It's used so universally that requiring an import would be tedious (len, first, rest)

A function belongs in **Flow** if:
1. It can be implemented in Flux using Base functions
2. It's a combinator or convenience wrapper (take, drop, zip, compose, identity)
3. It's domain-specific (math functions, string utilities)
4. It's used in specific contexts, not universally

### Concrete Classification

| Currently in Base | Should Stay? | Reason |
|-------------------|--------------|--------|
| print | Yes | VM I/O access |
| len | Yes | Universal, polymorphic, Unicode-aware |
| first, last, rest | Yes | Foundational, used in Flow modules themselves |
| push, concat | Yes | Array mutation needs native impl |
| reverse, sort | Yes | O(n log n) sort needs native impl |
| slice, contains | Yes | Native performance |
| map, filter, fold | Yes | Need RuntimeContext for callbacks. Even if Flow could implement these in pure Flux in the future, keeping them in Base establishes a stable, universally-available core vocabulary for FP pipelines — this is a language UX decision, not just a VM limitation. |
| split, join, trim | Yes | String ops need native Unicode handling |
| upper, lower | Yes | Unicode case mapping |
| starts_with, ends_with, replace | Yes | Native string ops |
| chars, substring | Yes | Unicode-aware |
| keys, values, has_key, merge, delete | Yes | Hash internals |
| abs, min, max | Yes | Numeric primitives |
| type_of, is_* | Yes | Type introspection needs runtime access |
| to_string | Yes | Polymorphic conversion needs runtime |

All 42 current builtins stay in Base. They all meet at least one criterion.

### What Goes in Flow (New)

| Module | Functions | Why Not Base |
|--------|-----------|--------------|
| Flow.List | take, drop, zip, flatten, any, all, find, foldl, foldr, reverse, append, sum, product, count, each, flat_map, nth, intersperse, zip_with | Persistent list combinators using `[h\|t]` cons syntax (Proposal 017) |
| Flow.Option | map, flat_map, unwrap_or, filter, or_else, zip | Pure Flux, match-based |
| Flow.Either | map, flat_map, fold, bimap, swap, to_option | Pure Flux, match-based |
| Flow.Func | identity, compose, flip, constant, times | Pure Flux combinators |
| Flow.Math | sign, clamp, gcd, lcm, factorial, is_even, is_odd | Pure Flux arithmetic |
| Flow.String | repeat, is_blank, pad_left, pad_right | Pure Flux string manipulation |
| Flow.Dict | get_or, map_values, filter_values, from_pairs | Pure Flux hash wrappers |

## Interaction With Other Proposals

### Proposal 017 (Persistent Collections + GC)

New Base functions: `hd`, `tl`, `list`, `is_list`, `to_list`, `to_array`, `put`, `get`, `is_map`

These go in Base because they require GC heap access (`&mut GcHeap`). They cannot be written in Flux.

`Flow.List` is designed exclusively for persistent lists (cons cells). It depends on Proposal 017:

```flux
import Flow.List

let xs = list(1, 2, 3)     // Base.list — creates persistent list [1 | [2 | [3 | None]]]
let ys = List.take(xs, 2)  // Flow.List.take — returns [1 | [2 | None]]
let z = List.find(xs, \x -> x > 1)  // => Some(2)
```

Array operations remain in Base (`first`, `rest`, `slice`, `sort`, `push`, `concat`, etc.) and do not require Flow.List.

### Proposal 022 (AST Traversal)

The `import Base except [...]` syntax adds a new field to `Statement::Import`. The `walk_stmt` and `fold_stmt` functions in `visit.rs`/`fold.rs` must be updated to handle the new `except` field. This is a compile-time-safe change — the exhaustive destructuring will catch it.

### Proposal 026 (Concurrency)

New Base functions for async layer: the exact set depends on whether `async`/`await` are keywords or builtins. If builtins, they join Base:

```
Base.spawn(fn)    -- spawn async task
Base.sleep(ms)    -- async timer
```

Actor layer builtins could be a separate synthetic module `Actor` rather than cluttering Base:

```flux
import Actor

let counter = Actor.spawn(Counter, 0)
Actor.send(counter, Increment)
let value = await Actor.ask(counter, Get)
```

This keeps Base focused on single-threaded essentials.

## Implementation Phases

### Phase 1: BaseModule Struct (Low Risk)

| Step | What | Effort |
|------|------|--------|
| 1.1 | Create `src/runtime/core.rs` with `BaseModule` struct | Small |
| 1.2 | Populate with all 42 current builtins by name | Small |
| 1.3 | Replace `define_builtin` loop in compiler with `BaseModule` iteration | Small |
| 1.4 | Remove hard-coded index constants | Small |
| 1.5 | All tests pass, zero behavioral change | — |

**Milestone:** Single source of truth for builtins. No more index coupling.

### Phase 2: Import Except Syntax (Medium Risk)

| Step | What | Effort |
|------|------|--------|
| 2.1 | Add `except` field to `Statement::Import` | Small |
| 2.2 | Parse `import Base except [...]`; reject `import Base as X` | Small |
| 2.3 | Update walk_stmt/fold_stmt for new field | Small |
| 2.4 | Compiler: skip excluded names during Base injection | Small |
| 2.5 | Enable qualified `Base.name(...)` via synthetic module resolution | Medium |
| 2.6 | Add `--no-base` CLI flag to disable prelude injection entirely | Small |
| 2.7 | Tests for except, qualified access, and `--no-base` | Small |

**Milestone:** Users can exclude and qualify Base functions. `--no-base` enables minimal/sandbox environments.

The `--no-base` flag is useful for:
- Sandboxed execution (no `print`, no I/O)
- Teaching ("look, everything is just a module — even `len`")
- Embedded/minimal runtimes
- Testing Base itself

### Phase 3: Flow Library Infrastructure (Medium Risk)

| Step | What | Effort |
|------|------|--------|
| 3.1 | Add virtual module registry to module resolver | Small |
| 3.2 | Embed Flow sources via `include_str!` in compiler binary | Small |
| 3.3 | Write `Flow.List` module | Medium |
| 3.4 | Write `Flow.Option` module | Small |
| 3.5 | Write `Flow.Either` module | Small |
| 3.6 | Write `Flow.Func` module | Small |
| 3.7 | Write `Flow.Math` module (after operators land) | Small |
| 3.8 | Write `Flow.String` module | Small |
| 3.9 | Write `Flow.Dict` module | Small |
| 3.10 | Precompile Flow modules to `.fxc` cache | Small (reuse existing cache) |
| 3.11 | Integration tests for all Flow modules | Medium |

**Milestone:** `import Flow.List` works out of the box.

### Phase 4: Documentation and Examples

| Step | What | Effort |
|------|------|--------|
| 4.1 | Document all Base functions | Medium |
| 4.2 | Document all Flow modules | Medium |
| 4.3 | Example programs using Flow | Small |
| 4.4 | Update language guide | Small |

## Glossary

| Term | Meaning |
|------|---------|
| **Base** | Auto-injected prelude scope + synthetic module. Rust-backed. Not a file on disk. |
| **Flow** | Standard library modules shipped with the compiler. Written in Flux. Require explicit `import`. |
| **Synthetic module** | A module recognized by the compiler without a corresponding `.flx` file. Resolved at compile time from an in-memory registry. |
| **Prelude injection** | The compiler step that injects Base names into scope before user code runs. Modified by `import Base except [...]`. |

## Risks

| Risk | Impact | Mitigation |
|------|--------|------------|
| Non-deterministic builtin ordering | Bytecode cache invalidation, wrong function called | Use `Vec` for ordered registry; never derive indices from `HashMap` iteration |
| Breaking bytecode cache | Stale cache loads wrong builtins | Bump cache version in Phase 1 |
| `except` conflicts with future keyword | Syntax ambiguity | `except` is contextual — only meaningful after `import Base` |
| Flow module performance | Pure Flux slower than native | Profile; promote hot paths to Base if needed |
| Name collisions between Base and Flow | `map` in Base vs `Flow.Option.map` | Base is unqualified; Flow is always qualified by module name |
| Circular imports in Flow | `Flow.List` uses `Flow.Option`? | Keep Flow modules independent; each only uses Base |

## Open Questions

1. **Should `map`/`filter`/`fold` be polymorphic over List (proposal 017)?** Currently they only work on Array. If they become polymorphic, they stay in Base. If List gets separate `List.map`/`List.filter`, those go in Flow.List. Recommendation: polymorphic in Base — match Elixir's `Enum` approach.

2. **Should `except` work on non-Base imports?** E.g., `import Flow.List except [take]`. This is useful but adds parser complexity. Recommendation: defer — only Base gets `except` initially.

3. ~~**Flow embedding strategy?**~~ Resolved: embed via `include_str!`, virtualize in the module resolver. No filesystem dependency. Users can override by placing their own `.flx` on disk (local file wins).

4. **Should `Flow` be `Flux`?** E.g., `import Flux.List` instead of `import Flow.List`. More branded but risks confusion with the language name. Recommendation: `Flow` — distinct from the language, but clearly related.

5. **Should Base be `Flux.Base`?** In Elixir it's just `Kernel`, not `Elixir.Kernel`. Recommendation: just `Base` — simpler.

6. ~~**How does `--no-base` flag work for minimal environments?**~~ Resolved: `--no-base` is a Phase 2 deliverable (step 2.6). Disables prelude injection entirely.

## References

- [Elixir Kernel](https://hexdocs.pm/elixir/Kernel.html) — auto-imported module, `import Kernel, except: [...]`
- [Haskell Prelude](https://hackage.haskell.org/package/base/docs/Prelude.html) — implicit import, `import Prelude hiding (...)`
- [Gleam prelude](https://hexdocs.pm/gleam_stdlib/) — pure FP on BEAM, auto-imported core
- [Elm Core](https://package.elm-lang.org/packages/elm/core/latest/) — `Basics` module auto-imported
- [Lua standard libraries](https://www.lua.org/manual/5.4/manual.html#6) — global functions + explicit `require`
