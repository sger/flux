# Improvements Backlog

This document tracks proposed features, fixes, and architectural improvements for Flux.

## Feature Ideas

- **Pipe-first dataflow**: `data |> map(f) |> filter(g) |> reduce(h)` with `|>` and placeholder `_` for ergonomic pipelines.
- **Pattern matching on data**: `match` with array/tuple/hash destructuring + guards (great for parsing, ASTs, DSLs).
- **Small actor-style concurrency**: lightweight `spawn`, `send`, `receive` with immutable message passing (Elixir-inspired).
- **Module-level contracts**: optional `@spec` and `@doc` annotations to generate docs and verify call arity/types at compile time.

## Dev Experience

- **Test matrix**: grow parser/compiler/vm tests around new rules (imports, module privacy, duplicate params, immutability).
- **Error UX**: consistent diagnostics (file/line/column), better hints, and a “did you mean?” for typos. (In progress)
- **Language spec alignment**: keep `docs/language_design.md` in sync with actual parser behavior. (In progress)
- **Module system roadmap**: decide on selective/aliased imports, export lists, and how modules map to files.
- **Standard library conventions**: builtins list, naming, and documentation.
- **REPL / formatter**: even minimal tooling makes iteration much faster.

## Recent Findings

- `examples/imports/import_collision_error.flx` hit the parser error for `;` (E102) before the import collision check.  
  **Status:** fixed by removing the semicolon.
- `examples/duplicate_params_literal_error.flx` prints no line snippet (expected, since function literals lack position info).  
  **Status:** known limitation.
- `examples/Errors/expected_token_error.flx` emits two errors: E105 (unexpected token) and E102 (expected expression).  
  **Status:** accepted; could suppress the follow-on error after a peek error.

If needed:
1) Tweak parser error recovery to avoid duplicate E105/E102 on one spot.  
2) Add position tracking for function literals so E012 can show a line.

## Architecture Findings / Opportunities

- **VM instruction fetch clones every step**: `VM::run` cloned instructions per loop.  
  **Status:** fixed (borrowed slice).
- **Builtin calls advanced IP twice**: caused skipped instruction and stack underflow.  
  **Status:** fixed (builtin path increments once).
- **Imports compile into a single global compiler state**: no module graph/per-module namespace yet.  
  **Status:** pending (needs module graph + separate compilation).
- **Diagnostics lack central code registry**: codes are spread across compiler/parser.  
  **Status:** pending (central enum/map would help).
- **No AST spans for literals/functions**: limits diagnostic precision.  
  **Status:** pending (add `Span` to AST nodes).
- **Spec vs implementation drift risk**: future syntax can confuse readers.  
  **Status:** in progress (doc alignment underway).

## Compiler Optimization Ideas

- **Constant pooling / interning**: avoid duplicate constants (strings/functions) to reduce bytecode size and cache footprint.
- **Member name constant reuse**: reuse string constants for repeated member access.
- **Avoid sorting hash keys by `to_string()`**: preserve source order or use stable positions to reduce overhead.
- **Reduce symbol table cloning**: use shared/persistent scopes instead of cloning on every `enter_scope`.
- **Improve error recovery**: short-circuit within a block after fatal compile errors to reduce cascading work.
- **Module initialization strategy**: avoid runtime module init calls when possible (emit hashes directly).
- **Bytecode size optimization**: add small-index opcodes (e.g., `OpGetLocal0`) to shrink instructions.
- **Deterministic free-symbol capture**: stable ordering to improve cache reproducibility.
