# Parity Check

> Source: `src/parity/`

The parity checker compares Flux execution ways over a fixture corpus. Its main job is to catch observable drift between the maintained VM and LLVM backends, including successful program output, failure phase, compiler diagnostics, cache behavior, and strict-mode behavior.

Use it when changing runtime behavior, compiler lowering, diagnostics, type checking, standard library behavior, or source-language semantics.

## Running

```bash
# Single file
cargo run -- parity-check examples/aoc/2024/day06.flx

# Directory; recursively collects .flx files and sorts them
cargo run -- parity-check examples/type_system

# Two-phase run: validate/compile fixtures first, then run parity
cargo run -- parity-check examples/type_system --compile

# Compare only selected ways
cargo run -- parity-check examples/type_system --ways vm,llvm

# Force rebuild of the per-backend parity binaries
cargo run -- parity-check examples/type_system --rebuild
```

The parity CLI is implemented in [`src/parity/cli.rs`](../../src/parity/cli.rs).

## Architecture

```
fixture.flx
    │
    ▼
┌─────────────────┐     ┌─────────────────┐
│ parity_vm/flux  │     │ parity_native/  │
│  VM backend     │     │  flux --native  │
└────────┬────────┘     └────────┬────────┘
         │                       │
         │ stdout/stderr/        │ stdout/stderr/
         │ exit kind             │ exit kind
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

The framework builds two compiler binaries:

- `target/parity_vm/debug/flux` — VM-only build.
- `target/parity_native/debug/flux` — build with `--features llvm`.

Parity subprocesses run with `NO_COLOR=1` so diagnostics are snapshot-friendly.

## Ways

A way is one backend plus one execution mode. The default is `vm,llvm`.

| Way           | Binary          | Flags                              | Purpose                  |
|---------------|-----------------|------------------------------------|--------------------------|
| `vm`          | parity VM       | `--no-cache`                       | Fresh VM run             |
| `llvm`        | parity native   | `--native --no-cache`              | Fresh LLVM/native run    |
| `vm_cached`   | parity VM       | cache enabled                      | Cached VM run            |
| `llvm_cached` | parity native   | `--native`, cache enabled          | Cached LLVM/native run   |
| `vm_strict`   | parity VM       | `--strict --no-cache`              | VM strict-mode run       |
| `llvm_strict` | parity native   | `--native --strict --no-cache`     | LLVM strict-mode run     |

Fixtures can override the ways with metadata:

```flux
// parity: vm_strict, llvm_strict
```

## Fixture Metadata

Fixture metadata is parsed from `//` comments by [`src/parity/fixture.rs`](../../src/parity/fixture.rs). Metadata usually lives at the top of the `.flx` file.

### Outcome

```flux
// expect: success
// expect: compile_error
// expect: runtime_error
```

- `success` is the default.
- `compile_error` requires every selected way to fail during compilation.
- `runtime_error` requires every selected way to fail at runtime.

Expected failures are not loose skips. They are assertions: the failure phase, diagnostic codes, and normalized stderr must match.

### Expected stdout

```flux
// parity-expected-stdout-begin
// Part A: 4778
// Part B: 1618
// parity-expected-stdout-end
```

The parser strips one leading `// ` prefix from each content line. Internal whitespace is preserved.

### Expected stderr

Compile-failing fixtures pin full normalized stderr inline:

```flux
// parity-expected-stderr-begin
// • 1 error • examples/type_system/failing/01_compile_type_mismatch.flx
// error[E300]: Argument Type Mismatch
//
// I found the wrong type in the 1st argument to `add`.
// parity-expected-stderr-end
// expect: compile_error
// expect-error: E300
```

The stderr block is the golden rendered diagnostic. It catches changes to wording, spans, labels, hints, notes, and diagnostic ordering.

Because the block is inline, adding or changing it can move source line numbers. Snapshot regeneration must be run to a fixed point: regenerate once, run again, and repeat until no snapshot changes remain.

### Diagnostic codes

```flux
// expect-error: E300
// expect-error: E400
```

`expect-error` is a stable semantic assertion layered on top of the full stderr snapshot. The checker extracts `error[E...]` codes from normalized stderr and compares the sorted list.

Backend-specific diagnostic expectations are intentionally not supported. VM and LLVM diagnostics should be identical for the selected ways. If they differ, the fixture should fail parity unless it is explicitly skipped for a clear reason.

### Skip

```flux
// skip: historical fixture currently compiles successfully
```

`skip:` reports the fixture as skipped instead of silently filtering it out. Use it only when the fixture does not currently represent an executable parity assertion, or when a known backend limitation makes comparison invalid.

## What Is Compared

For successful fixtures:

1. all ways must have the same exit kind,
2. normalized stdout must match across ways,
3. normalized stderr must match when any way failed,
4. expected stdout must match if a stdout block is present,
5. cached ways must match their fresh base way,
6. strict ways must obey the strict-mode compatibility rules.

For `expect: compile_error` and `expect: runtime_error` fixtures:

1. every selected way must fail in the expected phase,
2. diagnostic codes must match `expect-error` metadata,
3. normalized stderr must match the inline stderr snapshot when present,
4. normalized stderr must match across VM and LLVM.

Normalization lives in [`src/parity/normalize.rs`](../../src/parity/normalize.rs). It removes non-semantic noise such as build progress, backend banners, temp paths, leading `./` project path prefixes, ANSI color, and unstable internal type variable ids like `var #10603`.

## Verdicts

Defined in [`src/parity/mod.rs`](../../src/parity/mod.rs):

- `Pass` — all requested assertions held.
- `Mismatch { details }` — ways or expected diagnostics differed.
- `ExpectedOutputMismatch { expected, actual }` — ways agreed, but expected stdout/stderr did not match.
- `Skip { reason }` — fixture metadata requested a reported skip, or the native backend reported unsupported code.

Important mismatch details include:

- `ExitKind` — ways disagreed on success/compile error/runtime error/tool failure.
- `Stdout` / `Stderr` — normalized streams differed across ways.
- `DiagnosticCodes` — actual `error[E...]` codes differed from fixture metadata.
- `ExpectedStderr` — a way's normalized stderr differed from the inline stderr block.
- cache/strict/backend IR mismatch details for deeper parity surfaces.

Any non-pass verdict exits with code 1.

## The `--compile` Mode

`--compile` is a two-phase run.

### Phase 1: validate or warm

For successful and runtime-error fixtures, parity first runs each declared base way with cache enabled. This warms cache artifacts for phase 2 and catches compile failures before the parity loop.

For `expect: compile_error` fixtures, parity does **not** use cache-warming semantics. Failed compiles do not produce useful cache artifacts, and enabling `--cache-dir` can change parser/type diagnostic cascades. Instead, compile-error fixtures are validated with the same fresh no-cache way used by normal parity.

Phase 1 messages:

- `COMPILE OK` — success fixture compiled.
- `COMPILE EXPECTED FAIL` — expected failure failed in the declared phase.
- `COMPILE DIAGNOSTIC MISMATCH` — emitted error codes differed from `expect-error`.
- `COMPILE STDERR MISMATCH` — ways emitted different normalized stderr.
- `COMPILE EXPECTED STDERR MISMATCH` — stderr differed from the inline block.
- `COMPILE UNEXPECTED OK` — expected failure compiled successfully.
- `COMPILE FAIL` — success fixture failed to compile.
- `COMPILE SKIP` — fixture or backend was skipped.

If phase 1 finds any real failure, phase 2 is skipped and the process exits 1.

### Phase 2: parity

After a clean phase 1:

- success and runtime-error fixtures run as cached ways (`vm_cached`, `llvm_cached`) when metadata did not explicitly override ways,
- compile-error fixtures stay on their original fresh ways (`vm`, `llvm`, or strict variants), because there is no valid cache artifact to exercise.

## Debug Surface Capture

Optional flags compare intermediate compiler surfaces:

| Flag               | Captures                     | Purpose                       |
|--------------------|------------------------------|-------------------------------|
| `--capture-core`   | Core IR                      | Shared frontend/lowering IR   |
| `--capture-aether` | Aether ownership report      | Ownership/reuse diagnostics   |
| `--capture-repr`   | Backend representation       | Backend contract surface      |
| `--capture-cfg`    | CFG IR                       | VM backend IR                 |
| `--capture-lir`    | LIR                          | LLVM backend IR               |
| `--explain`        | Multiple surfaces            | Locate where divergence began |

Use the ladder Core -> Aether -> Repr -> backend IR (CFG/LIR). If Core differs, investigate frontend/shared lowering. If Core matches but CFG/LIR differs, investigate backend lowering.

## Maintaining Diagnostic Snapshots

There is currently no first-class snapshot update command. To update stderr snapshots safely:

1. run the fixture through the same parity path you intend to verify,
2. capture normalized stderr,
3. write it into `parity-expected-stderr-begin/end`,
4. repeat until the inline block no longer changes line numbers,
5. run the full parity command.

For the type-system suite, the expected verification is:

```bash
cargo test --lib parity:: -- --nocapture
cargo run -- parity-check .\examples\type_system\ --compile
```

Do not accept stderr snapshot churn casually. Snapshot diffs are compiler UI changes and should be reviewed like other user-visible diagnostics.

## Binary Staleness And Cache Recovery

Parity binaries are rebuilt automatically when missing or stale. `--rebuild` forces a rebuild.

If cache artifacts from an older compiler cause confusing failures, clear cache directories:

```bash
rm -rf target/flux/native target/flux/interfaces target/flux/vm target/parity-cache
```

On Windows, use the PowerShell equivalent with care.

## Extending

To add a way:

1. add a `Way` variant in [`src/parity/mod.rs`](../../src/parity/mod.rs),
2. update parsing/display/helper methods,
3. update runner argument construction,
4. update report rendering,
5. add tests or fixtures covering the new mode.

To add a comparison:

1. add a `MismatchDetail` variant,
2. extend comparison logic in [`src/parity/cli.rs`](../../src/parity/cli.rs),
3. render it in [`src/parity/report.rs`](../../src/parity/report.rs),
4. add focused tests where practical.

## Files

| File                        | Purpose                                      |
|-----------------------------|----------------------------------------------|
| `src/parity/mod.rs`         | Core types: ways, verdicts, mismatch details |
| `src/parity/cli.rs`         | CLI, fixture loop, compile phase             |
| `src/parity/runner.rs`      | Subprocess execution and cache management    |
| `src/parity/fixture.rs`     | Fixture metadata parser                      |
| `src/parity/normalize.rs`   | Output normalization                         |
| `src/parity/report.rs`      | Result rendering and mismatch explanation    |

## Related

- [Compiler Architecture](compiler_architecture.md)
- [Backend Representation Contracts](backend_representation_contracts.md)
- [Aether Debugging](aether_debugging.md)
