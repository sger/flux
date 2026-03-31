- Feature Name: Flux Parity Ways and Differential Validation
- Start Date: 2026-03-31
- Proposal PR:
- Flux Issue:
- Depends on: Proposal 0118 (Backend Consolidation), Proposal 0136 (Backend Parity Harness), Proposal 0137 (Modular Aether and Module Interfaces)

## Summary

Introduce a Flux parity layer that runs the same fixture corpus under multiple execution "ways" and compares normalized results.

Initial ways:

- `vm`
- `llvm`
- `vm_cached`
- `llvm_cached`
- `vm_strict`
- `llvm_strict`

The parity layer is not just a shell script. It is a maintained compiler-validation subsystem with:

- a canonical parity corpus
- a structured result model
- staged mismatch classification
- optional debug artifact capture (`--dump-core`, `--dump-core=debug`, `--dump-aether=debug`)
- CI/release gating for maintained ways

This proposal does not add new language semantics. It adds an engineering contract: backend, cache, and strict-mode behavior must remain observably equivalent unless a difference is explicitly declared and documented.

## Motivation

Flux now has enough moving parts that correctness bugs are no longer isolated to a single backend:

```text
AST -> Core -> cfg -> bytecode -> VM
AST -> Core -> lir -> LLVM -> native
```

And now also:

```text
source -> .flxi   (semantic interface cache)
source -> .fxm    (VM executable dependency cache)
program -> .fxc   (top-level bytecode cache)
```

This creates several classes of drift:

1. **Backend drift**
   - VM and LLVM execute the same source differently.

2. **Cache drift**
   - fresh compile and cached compile behave differently.

3. **Mode drift**
   - `--strict` changes behavior beyond intended diagnostics/boundary checks.

4. **Tooling drift**
   - `--dump-core` or `--dump-aether` no longer reflect what the real run path compiled.

Today these failures are found manually, often late, and often without a consistent way to localize them.

The right answer here is not one giant end-to-end test. It is a maintained testsuite that runs programs in multiple "ways" and records which way failed. Flux should adopt that discipline in its own terms.

## Goals

- Make parity a first-class engineering contract, not an occasional manual check.
- Detect backend/cache/mode regressions immediately.
- Localize mismatches quickly by preserving the semantic checkpoint at Core.
- Turn every discovered parity bug into a durable regression fixture.
- Provide a path to release gating for `0.0.5` and later.

## Non-goals

- Replacing `Core` as Flux's semantic IR
- Automatically proving the absence of backend bugs
- Comparing debug-only tooling output as a semantic contract
- Testing every possible optimization combination in phase 1
- Replacing existing unit/integration/snapshot tests

## Guide-level explanation

### What is a "way"?

A way is a named execution configuration for the same Flux fixture.

Examples:

- `vm`: default bytecode VM, fresh compile
- `llvm`: native path via `core_to_llvm`, fresh compile
- `vm_cached`: run twice, compare the cached second run
- `llvm_cached`: same idea for the native/cache-aware path
- `vm_strict`: VM with `--strict`
- `llvm_strict`: LLVM with `--strict`

The parity runner executes the same source under each requested way and compares normalized results.

### What counts as parity?

Parity means the following match across ways unless the fixture explicitly says otherwise:

- exit kind
- stdout
- stderr
- diagnostics class
- runtime error shape

Examples:

- If `examples/basics/arithmetic.flx` prints `42` on VM and `43` on LLVM, that is a parity failure.
- If a type error renders as a type error on VM and as a runtime crash on LLVM, that is a parity failure.
- If `vm_cached` behaves differently from fresh `vm`, that is a cache parity failure.

### What does the developer run?

The intended UX is:

```bash
cargo run -- parity-check examples/basics
cargo run -- parity-check tests/parity
cargo run -- parity-check examples/aoc/2024/day06.flx --ways vm,llvm,vm_cached
```

And for release/CI:

```bash
scripts/check_parity.sh tests/parity
scripts/check_parity.sh examples/basics
scripts/check_parity.sh examples/primop
```

### How parity failures should be debugged

The parity layer should push contributors into a consistent workflow:

1. Confirm the mismatch in the parity runner.
2. Inspect `--dump-core`.
3. If needed, inspect `--dump-core=debug`.
4. If ownership/reuse is implicated, inspect `--dump-aether=debug`.
5. Only after Core/Aether look correct, inspect backend-specific lowering/runtime behavior.

That keeps debugging aligned with Flux's architecture contract.

## Reference-level explanation

## Architecture

The parity subsystem has four layers:

1. **Corpus**
   - fixture discovery and metadata

2. **Ways**
   - named execution configurations

3. **Normalization**
   - convert raw process outputs into a structured result

4. **Classification**
   - determine whether a mismatch is semantic, cache-related, tooling-related, or infra-related

### Result model

The runner should compare structured results, not raw text blobs:

```rust
enum ExitKind {
    Success,
    CompileError,
    RuntimeError,
    Timeout,
    ToolFailure,
}

struct RunResult {
    way: Way,
    exit_kind: ExitKind,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    normalized_stdout: String,
    normalized_stderr: String,
    backend_banner: Option<String>,
    cache_observations: Vec<CacheObservation>,
}

struct DebugArtifacts {
    dump_core: Option<String>,
    dump_core_debug: Option<String>,
    dump_aether_debug: Option<String>,
}
```

This is intentionally richer than proposal 0136. Cache observations and explicit exit kinds matter now that Flux supports `.flxi`, `.fxm`, and `.fxc`.

### Fixture manifest

Every parity fixture should be representable as metadata, whether inline or in a sidecar manifest:

```toml
path = "examples/basics/arithmetic.flx"
ways = ["vm", "llvm"]
expect = "success"
roots = []
strict = false
capture_debug_artifacts = false
```

Future fixtures may allow:

- expected runtime error
- expected compile error phase/code
- explicit allowed differences
- minimum/maximum timeout
- cache prewarming requirements

### Mismatch classification

The runner should classify failures into a small, stable taxonomy:

- `build_failure`
- `launch_failure`
- `dump_core_failure`
- `core_mismatch`
- `aether_mismatch`
- `backend_mismatch`
- `cache_mismatch`
- `strict_mode_mismatch`
- `diagnostic_mismatch`
- `timeout`

Classification rules:

1. If a way fails to build or launch:
   - `build_failure` or `launch_failure`

2. If requested debug artifacts cannot be produced:
   - `dump_core_failure`

3. If `--dump-core` differs across supposedly equivalent ways:
   - `core_mismatch`

4. If `--dump-core` matches but `--dump-aether=debug` differs in semantically relevant ownership decisions:
   - `aether_mismatch`

5. If Core/Aether match but runtime behavior differs:
   - `backend_mismatch`

6. If fresh and cached runs differ within the same backend:
   - `cache_mismatch`

7. If only strict-mode variants differ unexpectedly:
   - `strict_mode_mismatch`

### Normalization

The runner must normalize only known non-semantic noise:

- backend banners such as `[cfg→vm] ...` or `[lir→llvm] ...`
- cargo/toolchain progress lines
- absolute temp paths in generated binary locations

It must not normalize:

- user stdout
- user stderr
- rendered diagnostics content beyond path normalization
- runtime error messages

### Ways

Phase 1 ways should be implemented as:

```text
vm           -> flux program.flx
llvm         -> flux --native program.flx
vm_cached    -> run vm twice, compare second run and cache observations
llvm_cached  -> run llvm twice where supported, compare second run
vm_strict    -> flux --strict program.flx
llvm_strict  -> flux --strict --native program.flx
```

Additional future ways:

- `vm_opt`
- `llvm_opt`
- `vm_all_errors`
- `llvm_all_errors`
- `vm_roots_only`
- `llvm_roots_only`

The important constraint is that each way must have a crisp meaning and remain stable over time.

## Corpus design

The parity corpus should have three tiers.

### Tier 1: small regression fixtures

Directory:

```text
tests/parity/
```

Each file should isolate one bug shape:

- top-level constant lowering
- imported-name shadowing
- ADT equality edge case
- ownership-sensitive call mode
- cached dependency reuse
- strict boundary mismatch

These are the most valuable fixtures because they are small, fast, and diagnostic.

### Tier 2: maintained example suites

Use existing directories:

- `examples/basics`
- `examples/primop`
- `examples/runtime_errors`
- `examples/namespaces`
- `examples/type_system`

These provide integration coverage and broad semantic pressure.

### Tier 3: stress and benchmark fixtures

Examples:

- selected AoC programs
- multimodule stdlib-heavy examples
- cache-sensitive repeated-run cases

These should not all be release-blocking at first, but they are useful for pre-release sweeps.

## CI and release policy

For `0.0.5`, parity should become a release theme and gradually a release gate.

### Minimum CI gate

Require parity on:

- `tests/parity`
- `examples/basics`
- `examples/primop`

for ways:

- `vm`
- `llvm`

### Extended pre-release gate

Before release candidates, also run:

- `vm_cached`
- `vm_strict`
- `llvm_strict`
- selected multimodule fixtures

### File-change trigger policy

Parity CI should be mandatory for changes touching:

- `src/core/`
- `src/aether/`
- `src/cfg/`
- `src/lir/`
- `src/core_to_llvm/`
- `src/bytecode/`
- `src/runtime/`
- `src/main.rs`
- `lib/Flow/`

## Proposed implementation

### Phase 1: Normalize proposal 0136 into a real parity runner

Deliverables:

- replace the fragile shell-only workflow with a maintained Rust runner or dedicated CLI entry point
- support `vm` and `llvm`
- compare normalized `stdout`, `stderr`, and exit kind
- print structured mismatch reports

Success criteria:

- a contributor can run parity on a file or directory with one command
- failures clearly say which way failed and how

### Phase 2: Add corpus metadata and tiny parity regressions

Deliverables:

- `tests/parity/` directory
- fixture metadata format or convention
- at least 10 focused parity regressions extracted from known bug classes

Suggested starting fixtures:

- imported exposed/local shadowing
- `Some(true) == Some(true)`
- top-level numeric constant use
- borrow-sensitive imported call
- strict-mode boundary case
- cached module interface reuse

Success criteria:

- every newly fixed parity bug leaves behind a tiny dedicated fixture

### Phase 3: Add Core checkpoint capture

Deliverables:

- optional `--dump-core` capture for selected ways
- `core_mismatch` classification
- path normalization for dump output

Success criteria:

- a parity failure can immediately distinguish "semantic IR differs" from "backend execution differs"

### Phase 4: Add Aether checkpoint capture

Deliverables:

- optional `--dump-aether=debug` capture
- `aether_mismatch` classification where relevant
- ownership-focused reporting for dup/drop/reuse regressions

Success criteria:

- Aether/Perceus regressions can be localized without manually reconstructing the pipeline

### Phase 5: Add cache parity ways

Deliverables:

- `vm_cached`
- `llvm_cached` where supported
- explicit cache observation reporting for `.flxi`, `.fxm`, `.fxc`
- `cache_mismatch` classification

This phase should directly validate the work from proposal 0137.

Success criteria:

- fresh compile and cached compile produce identical observable behavior for maintained fixtures

### Phase 6: Add strict-mode parity ways

Deliverables:

- `vm_strict`
- `llvm_strict`
- explicit notion of allowed strict-mode differences

Success criteria:

- `--strict` does not introduce backend-specific semantic drift

### Phase 7: Make parity part of release policy

Deliverables:

- CI integration
- release checklist integration
- documented "how to debug a parity failure" workflow

Success criteria:

- parity green is required for `0.0.5`

## Drawbacks

- More CI time
- More maintenance for fixture metadata and normalization rules
- Risk of over-normalizing output and hiding real bugs if the harness is sloppy

These are acceptable costs. Flux is at the point where the cost of not having this layer is higher.

## Rationale and alternatives

### Why not keep the current shell script?

Because parity is no longer just "compare two outputs once." It now needs:

- stable ways
- structured classification
- cache-aware checks
- artifact capture
- CI/release integration

That exceeds what a thin shell wrapper should own.

### Why not only compare final stdout?

Because many important regressions show up as:

- compile error vs runtime error
- different stderr with same stdout
- identical final output but different Core or Aether dumps
- fresh vs cached divergence

Raw stdout comparison is too weak.

### Why not only test LLVM parity?

Because Flux's current operational surface is broader:

- VM is still the default developer path
- cache correctness matters
- strict-mode correctness matters
- Aether insertion can regress without immediately producing obvious output differences

## Prior art

### Multi-way compiler validation

Mature compilers validate behavior across multiple execution configurations rather than asking only whether a test passed once. The key idea is not merely "does the test pass?" but "which execution mode fails?"

Flux should adopt that same discipline:

- same fixture
- multiple named ways
- stable reporting by way

### OCaml

OCaml historically lives with bytecode and native backends side by side. This is close to Flux's VM/native split and reinforces the need for backend conformance testing.

### Rust / differential testing

Rust's ecosystem uses differential validation across compiler modes and tools such as Miri. The general lesson is the same: multiple execution or analysis modes require deliberate comparison infrastructure.

## Unresolved questions

- Should fixture metadata live in sidecar files, inline comments, or a Rust registry?
- Should the parity runner be a CLI subcommand, a test binary, or both?
- Should `llvm_cached` be release-blocking immediately, or only after the native caching path is fully hardened?
- How much of `--dump-aether=debug` should be normalized versus treated as exact output?
- Should performance thresholds ever be added as a second layer, or should this proposal stay purely semantic?

## Future possibilities

- `cfg`/LIR dump capture as another localization layer
- reduction tooling that auto-minimizes a failing parity fixture
- random differential testing for generated small programs
- matrix execution across optimization levels and allocator/runtime toggles
- nightly deep parity sweeps on AoC and larger multimodule projects

## Conclusion

Flux is now complex enough that backend parity must be treated as architecture, not as an ad hoc debugging practice.

Proposal 0136 established the need for a backend harness. This proposal broadens that into a Flux parity "ways" system that covers:

- VM vs LLVM
- fresh vs cached
- normal vs strict
- Core/Aether checkpoints for diagnosis

That is the right shape for `0.0.5`: fewer new features, stronger guarantees, and a repeatable way to find semantic drift whenever the compiler changes.
