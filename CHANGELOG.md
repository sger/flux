# Changelog

## Unreleased

### Changed
- Function declaration keyword migrated to `fn`.
- `fun` is now deprecated; parser still accepts it in compatibility mode and emits warning `W013`.

## v0.0.2 (2026-01-31)

### Added
- Comparison operators: `<=`, `>=`
- Modulo operator: `%`
- Logical operators: `&&`, `||` (short-circuiting)
- Pipe operator: `|>`
- Either type: `Left` / `Right` with pattern matching
- Lambda shorthand: `\x -> expr`
- Array builtins: `concat`, `reverse`, `contains`, `slice`, `sort`
- String builtins: `split`, `join`, `trim`, `upper`, `lower`, `chars`, `substring`
- `to_string` builtin

### Changed
- Runtime error formatting now includes structured details, hints, and code frames

### Fixed
- `split` with empty delimiter now returns characters without empty ends
