# Changelog

All notable changes to Flux are documented here. See [docs/versions/](docs/versions/) for detailed What's New guides per release.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

---

## [Unreleased]

### Added

### Changed

### Docs

---

## [v0.0.3] - 2026-02-21

### Added
- `scripts/check_changelog_fragment.sh` to enforce changelog fragments in PR CI.
- `scripts/changelog_from_fragments.sh` to rebuild `CHANGELOG.md` `[Unreleased]` from `changes/*.md`.
- `scripts/release_cut.sh` to cut a new version section from `[Unreleased]` and update compare links.
- `scripts/release_check.sh` local preflight command documented in `README.md`.
- **Cranelift JIT backend** — compiles Flux programs to native machine code (`--features jit`, `--jit` flag)
- **Persistent cons lists** — GC-managed immutable linked lists with O(1) prepend; `[h | t]` literal and pattern syntax
- **HAMT hash maps** — Hash Array Mapped Trie with structural sharing on update; original maps are never mutated
- **Tuples** — `(a, b)` literals, `.0`/`.1` field access, destructuring in `let` and `match`
- **Do-blocks** — `do { expr1; expr2 }` sequential expression blocks; last expression is the value
- **Where clauses** — `expr where x = val` local bindings scoped after the body expression; multiple clauses chain
- **List comprehensions** — `[x * 2 | x <- xs, x > 0]` desugared at parse time to `map`/`filter`/`flat_map`
- **Pattern guards** — `match n { x if x > 0 -> ..., _ -> ... }`
- **Cons patterns** — `match lst { [h | t] -> ..., _ -> ... }`
- **Unit test framework** — `test_*` function discovery, `--test` / `--test-filter` flags, exit code 0/1
- **`Flow.FTest` stdlib** — `describe`, `it`, `for_each`, `with_fixture`, `approx_eq` helpers (`lib/Flow/FTest.flx`)
- **Mark-and-sweep GC** — manages cons cells and HAMT nodes; configurable threshold (`--gc-threshold`)
- **`--stats` flag** — timing and code metrics: parse, compile, execute, modules, source lines, instructions
- **`--optimize` / `-O`** — AST optimization passes: desugaring → constant folding → alpha-renaming
- **`--analyze` / `-A`** — analysis passes: free variable collection and tail-call detection
- **`--no-gc`** — disable garbage collection
- **`--gc-telemetry`** — GC stats on exit (requires `--features gc-telemetry`)
- **`--leak-detector`** — print allocation stats on exit
- **New CLI subcommands**: `analyze-free-vars`, `analyze-tail-calls`, `cache-info-file`
- **New builtins (75 total, up from 35)**: `flat_map`, `zip`, `find`, `any`, `all`, `count`, `sort_by`, `first`, `last`, `rest`, `range`, `sum`, `product`, `hd`, `tl`, `list`, `is_list`, `to_list`, `to_array`, `read_file`, `read_lines`, `read_stdin`, `parse_int`, `parse_ints`, `split_ints`, `now_ms`, `time`, `assert_eq`, `assert_neq`, `assert_true`, `assert_false`, `assert_throws`, `starts_with`, `ends_with`, `replace`, `flatten`, `is_map`
- **`dev-fast` build profile** — opt-level 3 with lighter debug info
- **VS Code syntax highlighting** extension (`tools/vscode/`)
- **62 opcodes** (up from 44)

### Changed
- Array syntax: `[| 1, 2, 3 |]` for arrays (Rc-backed); `[1, 2, 3]` now creates cons lists
- Arrays and cons lists are distinct types with separate builtins
- CI now runs changelog fragment validation on pull requests.
- Release docs now use a fragment-first changelog workflow.

### Docs
- Added `changes/README.md` and `changes/_template.md` for contributor guidance.

---

## [v0.0.2] - 2026-01-31

### Added
- **Comparison operators**: `<=`, `>=`
- **Modulo operator**: `%`
- **Logical operators**: `&&`, `||` (short-circuiting; `&&` binds tighter than `||`)
- **Pipe operator**: `|>` — `a |> f(b)` desugars to `f(a, b)` at parse time; lowest precedence
- **Either type**: `Left(v)` / `Right(v)` keywords for explicit error handling with pattern matching
- **Lambda shorthand**: `\x -> expr` (single param) and `\(a, b) -> expr` (multiple params)
- **String interpolation**: `"Hello, #{name}!"` — any expression inside `#{ }`
- **Forward references**: functions and modules can reference names defined later in the same scope
- **Module constants**: `let` bindings inside `module { }` evaluated at compile time
- **Qualified imports with aliases**: `import Modules.Utils.Math as M`
- **Duplicate module detection**: importing same module from two roots raises `E027`
- **`--roots-only` flag**: skip default module root resolution
- **Central error code registry**: stable constants in `compiler_errors.rs`, registered in `registry.rs`
- **AST spans**: all AST nodes carry `Span { start, end }` for precise `line:col` error locations
- **Array builtins**: `concat`, `reverse`, `contains`, `slice`, `sort`
- **String builtins**: `split`, `join`, `trim`, `upper`, `lower`, `chars`, `substring`
- **Hash map builtins**: `keys`, `values`, `has_key`, `merge`, `delete`
- **Numeric builtins**: `abs`, `min`, `max`
- **Type check builtins**: `type_of`, `is_int`, `is_float`, `is_string`, `is_bool`, `is_array`, `is_hash`, `is_none`, `is_some`
- **`to_string` builtin**
- **`--trace` flag**: print VM instruction trace

### Changed
- Runtime error formatting now includes structured details, hints, and colorized code frames (`NO_COLOR=1` to disable)

### Fixed
- `split` with empty delimiter returns characters without empty ends

---

## [v0.0.1] - 2026-01-10

Initial release.

### Added
- **Bytecode compiler** — compiles Flux source to portable `.fxc` bytecode
- **Stack-based VM** — executes bytecode with call frames and closures
- **Bytecode cache** — `.fxc` files under `target/flux/`, SHA-2 hash–based invalidation
- **Immutable `let` bindings** and named `fn` functions
- **Closures** — functions capture their lexical environment
- **If / else expressions**
- **Pattern matching** — `match` on literals, `Some(x)`, `None`, wildcards
- **Module system** — `module Name { }` declarations, `import Module.Path as Alias`, `_private` prefix, forward references, import cycle detection
- **String interpolation** — `"Score: #{score}"`
- **Primitive types**: integers, floats, booleans, strings, `None`, `Some`
- **Arithmetic operators**: `+`, `-`, `*`, `/`
- **Comparison operators**: `==`, `!=`, `<`, `>`
- **Unary operators**: `-x`, `!x`
- **String concatenation**: `+`
- **Linter** — unused bindings, shadowing, unused parameters, dead code
- **Formatter** — `flux fmt [--check] <file.flx>`
- **Diagnostics** — stable error codes `E001`–`E077`, source snippets, caret highlighting, inline suggestions
- **CLI subcommands**: `tokens`, `bytecode`, `lint`, `fmt`, `cache-info`
- **`--verbose` flag**: show cache hit/miss/store status
- **Builtins**: `print`, `to_string`, `len`, `push`, `concat`, `reverse`, `contains`, `slice`, `sort`, `split`, `join`, `trim`, `upper`, `lower`, `abs`, `min`, `max`, `type_of`, `is_int`, `is_float`, `is_string`, `is_bool`, `is_array`, `is_hash`, `is_none`, `is_some`

[Unreleased]: https://github.com/sger/flux/compare/v0.0.3...HEAD
[v0.0.3]: https://github.com/sger/flux/compare/v0.0.2...v0.0.3
[v0.0.2]: https://github.com/sger/flux/compare/v0.0.1...v0.0.2
[v0.0.1]: https://github.com/sger/flux/releases/tag/v0.0.1
