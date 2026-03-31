# Parity Validation

Flux compiles the same source through two independent backends:

```
Source -> Core IR -> CFG -> Bytecode -> VM
Source -> Core IR -> LIR -> LLVM IR  -> Native binary
```

Parity validation ensures both backends produce the same observable behavior. It also checks that caching and strict mode don't introduce drift.

## Quick start

### 1. Build the parity binaries (one-time)

```bash
CARGO_TARGET_DIR=target/parity_vm cargo build
CARGO_TARGET_DIR=target/parity_native cargo build --features core_to_llvm
```

This creates two separate flux binaries — one with only the VM backend, one with the native LLVM backend.

### 2. Run a parity check

```bash
cargo run -- parity-check tests/parity
```

Output:
```
PASS tests/parity/adt_either_match.flx
PASS tests/parity/arith_int_float_ops.flx
MISMATCH tests/parity/collection_array_sort.flx
  stdout differs:
    --- vm
    +++ llvm
    -[|1, 2, 3, 4, 5|]
    +[|5, 3, 1, 4, 2|]

=== Parity Results ===
Total:    13
Pass:     10
Mismatch: 3
Skip:     0
```

### 3. Check an example directory

```bash
cargo run -- parity-check examples/basics
```

```
PASS examples/basics/arithmetic.flx
MISMATCH examples/basics/array_builtins.flx
  stdout differs:
    --- vm
    +++ llvm
     5
     0
     1
     5
     [|2, 3, 4, 5|]
     [|1, 2, 3, 4, 5, 6|]
     None
     None
     [||]
     [|1, 2, 3, 4, 5, 6|]
     [|5, 4, 3, 2, 1|]
     true
    ... (8 more lines)
PASS examples/basics/array_hash_combo.flx
PASS examples/basics/array_iteration.flx
...
```

### 4. Check a single file

```bash
cargo run -- parity-check examples/basics/fibonacci.flx
```

## Ways

A "way" is a named execution configuration for the same source file.

| Way | What it does | Flag passed |
|---|---|---|
| `vm` | Fresh VM compile + run | (none) |
| `llvm` | Fresh native compile + run | `--native` |
| `vm_cached` | VM: warm cache, then run with cache | (none, cache enabled) |
| `llvm_cached` | LLVM: warm cache, then run with cache | `--native`, cache enabled |
| `vm_strict` | VM with strict type/effect checks | `--strict` |
| `llvm_strict` | LLVM with strict checks | `--native --strict` |

Specify ways with `--ways`:

```bash
cargo run -- parity-check tests/parity --ways vm,llvm
cargo run -- parity-check tests/parity --ways vm,vm_cached,vm_strict
cargo run -- parity-check tests/parity --ways vm,llvm,vm_cached,llvm_cached,vm_strict,llvm_strict
```

Default: `vm,llvm` (or per-fixture metadata, see below).

## What counts as parity?

Parity means the following match across ways:

- **exit kind** — Success, CompileError, RuntimeError
- **stdout** — normalized user output
- **stderr** — normalized error output (compared only when at least one way fails)

Output normalization strips:
- Backend banners (`[cfg->vm] ...`, `[lir->llvm] ...`)
- Cargo progress lines
- Absolute temp paths

## Mismatch classification

When a mismatch is found, the runner classifies it to help localize the bug:

| Classification | Meaning | Where to look |
|---|---|---|
| `core_mismatch` | Core IR differs across ways | `src/core/`, `src/ast/` — frontend bug |
| `aether_mismatch` | Aether ownership differs | `src/aether/` — Perceus insertion bug |
| `stdout differs` / `exit_kind` | Backend execution diverges | VM: `src/cfg/`, `src/bytecode/`; LLVM: `src/lir/`, `src/core_to_llvm/` |
| `cache_mismatch` | Fresh vs cached run differ | Cache logic in `src/bytecode/`, `.fxc`/`.flxi` files |
| `strict_mode_mismatch` | Strict mode changes behavior beyond diagnostics | Strict-mode checks in the compiler |

Use `--capture-core` and `--capture-aether` to enable the deeper classifications:

```bash
cargo run -- parity-check tests/parity --capture-core --capture-aether
```

## Debugging a parity failure

Follow this workflow top-down. Each step narrows the problem.

### Step 1: Confirm the mismatch

```bash
cargo run -- parity-check path/to/failing.flx
```

Note what differs: stdout, exit_kind, or stderr.

### Step 2: Check Core IR

```bash
cargo run -- parity-check path/to/failing.flx --capture-core
```

- `core_mismatch` reported? Bug is in the frontend — the same source produces different Core IR per binary. Investigate `src/core/` and `src/ast/`.
- No `core_mismatch`? Core IR is identical. Move to step 3.

### Step 3: Check Aether ownership

```bash
cargo run -- parity-check path/to/failing.flx --capture-aether
```

- `aether_mismatch` reported? The Perceus dup/drop/reuse insertion differs. Investigate `src/aether/`.
- No `aether_mismatch`? Ownership is identical. The bug is in backend lowering.

### Step 4: Inspect the backend

Core and Aether are identical — the bug is in how the backend lowers or executes:

```bash
# VM: instruction trace
cargo run -- path/to/failing.flx --trace

# LLVM: dump intermediate representations
cargo run --features native -- path/to/failing.flx --native --dump-lir
cargo run --features native -- path/to/failing.flx --native --emit-llvm
```

### Step 5: Leave a regression fixture

Once fixed, add a minimal `.flx` file to `tests/parity/`:

```
// parity: vm, llvm
// expect: success
// bug: one-line description of the bug shape

fn main() with IO {
    // minimal reproduction
    print(42);
}
```

See `tests/parity/CONVENTION.md` for naming and metadata rules.

## Fixture metadata

Each fixture in `tests/parity/` can declare inline metadata:

```
// parity: vm, llvm
// expect: success
// bug: Array.sort returns unsorted on LLVM
```

- `parity:` — which ways to compare (overrides CLI default when `--ways` is not specified)
- `expect:` — `success`, `compile_error`, or `runtime_error`
- `bug:` — one-line description of what this fixture guards against

## Strict-mode rules

Strict mode may produce additional compile errors that normal mode doesn't. This is expected. The parity runner treats this as an **allowed difference**:

- Normal=Success, Strict=CompileError: allowed (strict caught more)
- Both succeed with different stdout: `strict_mode_mismatch` (not allowed)
- Normal fails, strict fails differently: `strict_mode_mismatch` (not allowed)

## CI and release integration

### CI (every push/PR)

The CI workflow runs parity on `tests/parity` and `examples/basics` with `vm,llvm`:

```bash
scripts/check_parity.sh tests/parity examples/basics
```

### Release preflight

Before releases, the extended suite adds `vm_cached`, `vm_strict`, `llvm_strict`:

```bash
scripts/check_parity.sh --extended tests/parity examples/basics
```

This is included in `scripts/release/release_check.sh`.

## CLI reference

```
flux parity-check <file-or-dir> [options]

Options:
  --ways <w1,w2,...>     Ways to compare (default: vm,llvm)
  --capture-core         Capture --dump-core per way and compare Core IR
  --capture-aether       Capture --dump-aether=debug per way and compare ownership
  --vm-binary <path>     Path to VM binary (default: target/parity_vm/debug/flux)
  --llvm-binary <path>   Path to native binary (default: target/parity_native/debug/flux)
  --timeout <secs>       Timeout per file per way (default: 15)
  --root <path>          Module root (forwarded to flux, can repeat)
```
