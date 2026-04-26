# Parity Fixture Convention

Each `.flx` file in this directory is a self-contained parity regression test.

## Naming

Files follow the pattern: `<bug_class>_<short_description>.flx`

Bug classes:
- `toplevel_` — top-level declarations, constants, pure expressions
- `adt_` — ADT construction, matching, equality
- `closure_` — lambda captures, higher-order functions
- `import_` — module imports, shadowing, qualified access
- `effect_` — algebraic effects, handle/perform
- `match_` — pattern matching exhaustiveness, nested patterns
- `collection_` — array/list/map operations
- `arith_` — arithmetic, comparisons, numeric edge cases
- `string_` — string operations, interpolation
- `tuple_` — tuple construction, field access

## Inline metadata

Each fixture starts with a header comment block:

```
// parity: vm, llvm
// expect: success
// bug: <one-line description of the bug shape this fixture isolates>
```

Fields:
- `parity:` — which ways to compare (default: `vm, llvm`)
- `expect:` — `success` | `compile_error` | `runtime_error`
- `bug:` — what regression this fixture guards against

## Rules

1. Each file isolates exactly one bug shape.
2. Files must be small (under 30 lines preferred).
3. Output must be deterministic — no timestamps, random values, etc.
4. Every newly fixed parity bug should leave behind a fixture here.
5. Files must work with `fn main() with IO { ... }` or top-level expressions.

## Running

```bash
# Basic parity (vm vs llvm)
cargo run -- parity-check tests/parity
cargo run -- parity-check tests/parity/adt_option_equality.flx

# All ways
cargo run -- parity-check tests/parity --ways vm,llvm,vm_cached,vm_strict,llvm_strict

# With Core/Aether checkpoint capture
cargo run -- parity-check tests/parity --capture-core --capture-aether

# CI/release parity commands
cargo run -- parity-check tests/parity --ways vm,llvm
cargo run -- parity-check examples/guide --ways vm,llvm
cargo run -- parity-check tests/parity --ways vm,llvm,vm_cached,vm_strict,llvm_strict
```

## How to debug a parity failure

Follow this workflow top-down. Each step narrows the problem further.

### 1. Confirm the mismatch

```bash
cargo run -- parity-check path/to/failing.flx
```

Note the classification: `stdout differs`, `exit_kind`, `stderr differs`.

### 2. Check Core IR (semantic IR)

```bash
cargo run -- parity-check path/to/failing.flx --capture-core
```

- **`core_mismatch` reported?** The bug is in the frontend — AST lowering, type inference, or Core passes produce different IR per binary. This should not happen; investigate `src/core/` and `src/ast/`.
- **No `core_mismatch`?** Core IR is identical. The bug is downstream.

### 3. Check Aether (ownership model)

```bash
cargo run -- parity-check path/to/failing.flx --capture-aether
```

- **`aether_mismatch` reported?** The Perceus dup/drop/reuse insertion differs. Investigate `src/aether/`.
- **No `aether_mismatch`?** Ownership is identical. The bug is in backend lowering.

### 4. Inspect backend-specific behavior

At this point Core and Aether are identical — the bug is in the backend:

- **VM path:** `src/cfg/`, `src/bytecode/`, `src/runtime/vm/`
- **LLVM path:** `src/lir/`, `src/llvm/`, `src/runtime/c/`

Useful flags:
```bash
# VM instruction trace
cargo run -- path/to/failing.flx --trace

# Dump LIR (native backend intermediate)
cargo run --features native -- path/to/failing.flx --native --dump-lir

# Dump LLVM IR text
cargo run --features native -- path/to/failing.flx --native --emit-llvm
```

### 5. For cache mismatches

```bash
cargo run -- parity-check path/to/failing.flx --ways vm,vm_cached
```

Compare the fresh run and cached run. Check `.fxc` and `.flxi` file contents.

### 6. For strict-mode mismatches

```bash
cargo run -- parity-check path/to/failing.flx --ways vm,vm_strict
```

Allowed: strict mode rejecting what normal mode accepts (additional diagnostics).
Not allowed: strict mode producing different output when both succeed.

### 7. Leave a regression fixture

Once fixed, add a minimal `.flx` file to `tests/parity/` following the naming convention above.
