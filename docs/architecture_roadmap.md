# Architecture Roadmap

This document outlines high-impact architectural improvements for Flux.

## Highest Priority

- Module graph (imports, cycle detection, deterministic order).
- Central error-code registry (`src/frontend/error_codes.rs`).
- AST spans for all nodes (better diagnostics and tooling).
- VM trace + stack dump + source mapping.
- List/Map stdlib + Option ergonomics (`unwrap_or`, `map`, `and_then`).
- Match guards (`pattern if condition -> expr`).

### Module graph (imports, cycle detection, deterministic order)
- Build a graph from each module to its direct imports (edge = `module -> import`).
- Resolve and normalize module IDs (absolute path + canonical name) before graph insertion.
- Enforce deterministic traversal: stable sort imports and visit order for identical builds.
- Detect cycles (DFS with color marks or Tarjan SCC) and emit a single, focused error.
- Produce a topological order for compilation/execution planning (reject if cycles exist).
- Cache graph + topo order to support incremental builds and parallel compilation later.
- See `docs/module_graph.md` for the full design notes.

## v0.0.2 Roadmap (language + tooling)

### Language core
- Add List and Map modules (stdlib) with a minimal, stable API.
- Add match guards: `pattern if condition -> expr` (huge usability win).
- Option ergonomics: `is_some`, `unwrap_or`, `map`, `and_then`.
- Diagnostics polish: consistent file/line/col, better hints for match errors.
- Parser/VM tests: cover match guards, Some patterns, Option helpers.

### Tooling (debugging roadmap)
- See `docs/debugging_tools.md` for the full plan and examples.
- VM instruction trace (flagged; per-step op + stack delta).
- Stack dump (explicit command or trace option).
- Source span mapping (bytecode offsets -> source lines).
- Disassembler improvements (annotate constants, functions, spans).
- REPL stepping (single-step + continue).
- Structured errors (machine-readable diagnostics).
- Symbol table dump (debug-only).
- Differential testing (VM vs interpreter once available).

### Memory & GC
- See `docs/gc_roadmap.md` for a staged plan (Rc now → tracing GC later).

### Nice-to-have
- Simple formatter rules (indent only, preserve comments).
- More stdlib: `contains`, `slice`.
- Better module exports (explicit `pub` list or export rules).

## Near-term (stability + performance)

- Remove per-instruction cloning in the VM loop (instruction fetch should borrow, not allocate).
- Fix VM instruction pointer handling for builtin calls to avoid skipping instructions.
- Add a central error-code registry to prevent duplicates and keep docs in sync.
- Add AST spans for identifiers and function literals to improve diagnostic precision.

## Mid-term (compiler structure)

- Introduce a module graph (file -> module -> dependencies) to support:
  - deterministic import order
  - cycle detection
  - incremental builds
- Separate per-module symbol tables/constants to prepare for caching and parallel compilation.
- Add a lightweight IR layer for optimization and clearer compiler stages.

## Long-term (tooling + ecosystem)

- Define a stable public compiler API for tooling (formatter, LSP, linting).
- Add a standard library boundary with versioned builtins.
- Implement a test harness for language-level examples (golden outputs).
