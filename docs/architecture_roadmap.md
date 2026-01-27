# Architecture Roadmap

This document outlines high-impact architectural improvements for Flux.

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
