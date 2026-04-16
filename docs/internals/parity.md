# Parity Check

> Source: `src/parity/`

The parity check compares the Flux VM and LLVM native backends on a fixture corpus, ensuring they produce the same observable behavior (stdout, stderr, exit code, and optionally shared IR surfaces). It is the primary mechanism for catching backend divergence.

## Running

```bash
# Single file
cargo run -- parity-check examples/aoc/2024/day06.flx

# Directory (all .flx files, sorted)
cargo run -- parity-check examples/basics

# Two-phase: compile every fixture first (stop on failure), then parity
cargo run -- parity-check examples/basics --compile

# Force rebuild of per-backend parity binaries
cargo run -- parity-check examples/basics --rebuild
```

The parity CLI lives at [`src/parity/cli.rs`](../../src/parity/cli.rs) and is dispatched from the top-level command surface.

## Architecture

```
fixture.flx
    │
    ▼
┌─────────────────┐     ┌─────────────────┐
│ parity_vm/flux  │     │ parity_native/  │
│  (VM backend)   │     │  flux (LLVM)    │
└────────┬────────┘     └────────┬────────┘
         │                       │
         │ stdout/stderr/        │ stdout/stderr/
         │ exit code             │ exit code
         ▼                       ▼
    ┌────────────────────────────────┐
    │       normalize & compare       │
    │  (src/parity/normalize.rs)      │
    └──────────────┬──────────────────┘
                   ▼
              ┌────────┐
              │Verdict │
              └────────┘
```

The framework builds two copies of the compiler with different feature sets:

- `target/parity_vm/debug/flux` — VM-only (no `--features llvm`)
- `target/parity_native/debug/flux` — with `--features llvm`

This isolates VM and native backends into separate processes. Both share the on-disk cache (`target/flux/`).

## Ways

A "way" is one backend + execution mode. Defined in [`src/parity/mod.rs`](../../src/parity/mod.rs):

| Way           | Binary     | Flags                    | Purpose                       |
|---------------|------------|--------------------------|-------------------------------|
| `vm`          | parity_vm  | `--no-cache`             | Fresh VM (default)            |
| `llvm`        | parity_native | `--native --no-cache` | Fresh native (default)        |
| `vm_cached`   | parity_vm  | (none)                   | Cached VM run                 |
| `llvm_cached` | parity_native | `--native`            | Cached native run             |
| `vm_strict`   | parity_vm  | `--strict --no-cache`    | VM with strict type mode      |
| `llvm_strict` | parity_native | `--native --strict --no-cache` | Native with strict mode |

Select explicitly with `--ways vm,llvm_cached` or let per-fixture metadata choose.

## Fixture Metadata

Fixtures declare expected behavior via comments at the top of the file. Parsed by [`src/parity/fixture.rs`](../../src/parity/fixture.rs).

### Expected stdout block

```flux
// parity-expected-stdout-begin
// "Part A: 4778"
// "Part B: 1618"
// parity-expected-stdout-end

fn main() with IO {
    print("Part A: 4778")
    print("Part B: 1618")
}
```

Each content line has a single `// ` prefix stripped; everything after that is compared verbatim (preserves leading whitespace — important for multi-line string output).

### Expectation markers

```flux
// expect: compile_error     // or: success (default) | runtime_error
// bug: missing closing brace
// parity: vm, llvm, vm_cached
```

- `expect: success` (default) — all backends must succeed with matching stdout (if expected block is present).
- `expect: compile_error` — all backends must exit non-zero; stdout comparison skipped (stderr is too volatile to pin).
- `expect: runtime_error` — same semantics as `compile_error` for parity purposes.
- `bug:` — freeform description; not enforced, surfaces in diagnostics.
- `parity:` — override which ways to run.

## Verdicts

Defined in [`src/parity/mod.rs`](../../src/parity/mod.rs):

- **`Pass`** — all ways agree, stdout matches expected block (or no block present), exit codes satisfy the `expect:` declaration.
- **`Mismatch { details }`** — ways disagree on stdout/stderr/exit code. Most common parity failure.
- **`ExpectedOutputMismatch { expected, actual }`** — ways agree, but output doesn't match the fixture's expected block, or an `expect: compile_error` fixture unexpectedly succeeded.
- **`Skip { reason }`** — fixture was skipped (e.g., LLVM backend not supported for this construct).

Non-`Pass` verdicts cause the CLI to exit with code 1.

## Comparison Layers

When ways run, the framework compares several surfaces:

1. **Exit code** — must match across ways.
2. **Normalized stdout** — `normalize()` in [`normalize.rs`](../../src/parity/normalize.rs) strips ANSI codes, timestamps, and absolute paths.
3. **Normalized stderr** — same normalization.
4. **Expected stdout block** — agreed output must match the fixture's declared expected output.
5. **Cache parity** — for `*_cached` ways, verifies cache artifacts are created/reused correctly.
6. **Strict parity** — for `*_strict` ways, verifies strict-mode behavior differs from normal only in allowed ways (`Success → CompileError` is allowed; anything else is a mismatch).

Each comparison adds a `MismatchDetail` to the result on divergence.

## Debug Surface Capture

Flags to capture and compare intermediate IR surfaces:

| Flag              | Captures                     | Implementation                |
|-------------------|------------------------------|-------------------------------|
| `--capture-core`  | Core IR (`--dump-core`)      | Shared by both backends       |
| `--capture-aether`| Aether ownership annotations | Shared                        |
| `--capture-repr`  | Backend representation       | Backend-specific contracts    |
| `--capture-cfg`   | CFG IR (VM ways)             | VM-only                       |
| `--capture-lir`   | LIR (LLVM ways)              | Native-only                   |
| `--explain`       | All of the above             | For diagnosing the ladder     |

When a mismatch occurs, the framework can explain **where** the divergence started. If Core surfaces match but CFG/LIR differ, the bug is in backend lowering. If Core itself differs, the bug is in the frontend.

Ordered as a debug ladder: Core → Aether → Repr → Backend IR (CFG/LIR).

## The `--compile` Mode

Two-phase mode: compile every fixture, then run parity against the warm cache.

### Phase 1: Compile

For each fixture, runs both backends with cache enabled (no `--no-cache`). Cache is cleared per-file via `clear_cache_files` in [`runner.rs`](../../src/parity/runner.rs) to avoid cross-fixture pollution (native object files from different fixtures can produce incompatible exports for shared stdlib modules).

Per-fixture outcome markers:
- `COMPILE OK` — compiled cleanly (normal fixture).
- `COMPILE EXPECTED FAIL` — fixture with `expect: compile_error`/`runtime_error` failed as declared.
- `COMPILE UNEXPECTED OK` — fixture declared `expect: compile_error` but compiled successfully (bug regressed or fixture out of date). Stops the run.
- `COMPILE FAIL` — compile failed without `expect` declaration. Stops the run.
- `COMPILE SKIP` — LLVM backend reported the construct as unsupported.

If any real failure occurs, Phase 2 is skipped and the run exits 1.

### Phase 2: Parity

Runs using `VmCached`/`LlvmCached` ways instead of fresh `--no-cache` ways, reusing artifacts from Phase 1.

**Tradeoff**: `--compile` catches compile failures up-front with cleaner error output, at the cost of re-clearing + re-populating the cache per fixture (so it is not significantly faster than the default mode on first run; the savings come from not paying the compile cost twice per backend per file at parity time).

## Binary Staleness

The parity binaries are built with `CARGO_TARGET_DIR=target/parity_vm cargo build` and `CARGO_TARGET_DIR=target/parity_native cargo build --features llvm`. [`cli.rs::ensure_parity_binaries`](../../src/parity/cli.rs) checks source freshness and rebuilds automatically. `--rebuild` forces a rebuild.

If cache artifacts persist from an older binary, they may fail to link (undefined symbol errors on stdlib module exports). Clear with:

```bash
rm -rf target/flux/native target/flux/interfaces target/flux/vm
```

This is normally unnecessary — `clear_cache_files` per-fixture handles it — but can be a recovery step if the cache ends up in an unexpected state.

## Extending

To add a new way:

1. Add variant to `Way` enum in `src/parity/mod.rs`.
2. Update `Way::parse`, `Way::backend_id`, `Way::is_cached`, `Way::is_strict`, `Way::base_way`.
3. Handle the new way in `build_way_args` (flags to add).
4. Add command format in `cargo_run_for_way` in `src/parity/report.rs`.
5. Thread through `run_way`/`run_cached_way` in `src/parity/runner.rs` if special handling is needed.

To add a new mismatch comparison:

1. Add variant to `MismatchDetail` in `src/parity/mod.rs`.
2. Extend the comparison pass in `src/parity/cli.rs::check_file`.
3. Add rendering in `src/parity/report.rs::print_mismatch_detail`.

## Files

| File                              | Purpose                                              |
|-----------------------------------|------------------------------------------------------|
| `src/parity/mod.rs`               | Core types: `Way`, `Verdict`, `MismatchDetail`, `RunResult` |
| `src/parity/cli.rs`               | CLI entry point, fixture loop, compile phase, config |
| `src/parity/runner.rs`            | Subprocess execution, cache management, debug capture |
| `src/parity/fixture.rs`           | Fixture metadata parser (`expect:`, `parity-expected-stdout-begin`) |
| `src/parity/normalize.rs`         | Output normalization (ANSI, paths, whitespace)       |
| `src/parity/report.rs`            | Result rendering, diagnostics, mismatch explanation   |

## Related

- [Compiler Architecture](compiler_architecture.md) — how VM and native backends diverge after Aether
- [Backend Representation Contracts](backend_representation_contracts.md) — the `repr` surface
- [Aether Debugging](aether_debugging.md) — the `aether` debug surface
