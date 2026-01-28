# Module Graph (Imports, Cycles, Determinism)

This document outlines the module graph design for Flux: how imports are modeled, how cycles are detected, and how deterministic ordering is enforced.

## Goals

- Deterministic compilation and execution order for identical inputs.
- Early, clear errors for import cycles.
- A graph representation that supports incremental builds later.

## Graph Model

- Node: a module, identified by a normalized `ModuleId`.
- Edge: `module -> import` for each direct import.

## Module Declarations (Script vs Module)

- A file without a `module ...` declaration is a **script** (entry-only, not importable).
- A file with a `module ...` declaration is a **module file** (importable).
- Module files may only contain imports and a single module declaration.
- Module declarations must be top-level (no nested or inline modules).
- Module files must use the exact UpperCamelCase filename that matches the final module segment.
- Parser errors are reported before module-path validation, so syntax issues can appear before path mismatch diagnostics.

## Qualified Access

- Dotted module names are real namespaces; use the full name for access (e.g., `Data.List.value()`).

### ModuleId normalization

- Resolve to an absolute path (canonicalized).
- Use a stable, normalized module name (no relative segments).
- Reject or normalize duplicates (same file via different paths).

## Deterministic Order

- Parse imports in source order, then normalize to `ModuleId`.
- Sort normalized imports by their stable ID (e.g., canonical path).
- Traverse in stable order when building the graph and producing output.

## Module Roots

- Import resolution searches configured module roots (e.g., `src/` and the entry file directory).
- Dotted module names map to nested directories under these roots.

## Cycle Detection

Preferred: DFS with color marks.

- `White`: unvisited
- `Gray`: visiting (on recursion stack)
- `Black`: done

If a `Gray` node is encountered, a cycle exists. Report the cycle path from the stack.

Alternative: Tarjan SCC (if we later need component output).

## Topological Order

- After building the graph, perform a topological sort.
- If a cycle is detected, abort and emit a single cycle error with the path.
- Output order must be stable given the same inputs.

## Error Format (draft)

- Error code: `E1XXX` (reserve in central registry once it exists).
- Message: `import cycle detected: A -> B -> C -> A`
- Attach: full module paths for each entry, plus import site span if available.

## Future Hooks

- Cache graph + topo order for incremental builds.
- Track per-module hashes for rebuild decisions.
- Support parallel compilation once dependencies are explicit.
