- Feature Name: CLI / Driver Architecture Split
- Start Date: 2026-04-11
- Status: Implemented
- Proposal PR:
- Flux Issue:
- Depends on: Proposal 0153

## Summary
[summary]: #summary

Split the current monolithic binary entrypoint into three explicit layers:

- a **thin process entrypoint** in `src/main.rs`
- a **CLI/application layer** responsible for argument parsing, subcommand dispatch, and user-facing rendering
- a **driver/session layer** responsible for orchestration of compiler pipelines, backend execution, dumping, parity capture, and test harness execution

The target architecture is:

```text
main.rs
  -> cli/
      -> commands/
      -> render/
  -> driver/
      -> session/
      -> pipeline/
      -> runtime/
```

This proposal does **not** change Flux semantics or IR boundaries. It restructures binary/application code so that:

- `src/main.rs` becomes a small entrypoint instead of a multi-thousand-line control file
- command behavior becomes testable without spawning the whole binary for every path
- runtime diagnostics, parity orchestration, native execution, and dump surfaces are owned by dedicated modules instead of being spread through `main.rs`

## Motivation
[motivation]: #motivation

`src/main.rs` has become the operational center for too many responsibilities at once. It currently mixes:

- command-line parsing
- usage/help rendering
- subcommand dispatch
- module graph construction
- backend selection
- native compilation orchestration
- child-process execution
- runtime panic capture and diagnostic rendering
- parity command orchestration
- test harness execution
- cache inspection/reporting

This creates several problems.

### 1. The process entrypoint is far too large

The binary entrypoint is currently thousands of lines long. That makes it harder to:

- understand which code is CLI plumbing vs compilation behavior
- make localized changes without unintentionally affecting unrelated commands
- review changes with confidence
- reuse shared behavior across commands

### 2. Application concerns and compiler concerns are mixed together

Compiler internals already have clear architecture boundaries:

- `syntax/`
- `core/`
- `aether/`
- `cfg/`
- `lir/`
- `core_to_llvm/`

But the binary/application layer does not. The current file mixes:

- “how do we parse `--dump-aether`?”
- “how do we run native compilation in parallel?”
- “how do we translate a runtime panic into a Flux diagnostic?”

These are different kinds of concerns and should not live in one file.

### 3. Runtime diagnostics are now an application-layer responsibility

After the recent native/runtime parity work, Flux now has logic that:

- executes native child processes
- captures stderr
- parses runtime panic text
- reconstructs Flux diagnostics
- renders stack traces

That is a real application subsystem. It should be a named module with tests, not a set of helper functions embedded in `main.rs`.

### 4. Command orchestration is hard to test in isolation

Features like:

- `parity-check`
- `native-cache-info`
- `--dump-core`
- `--dump-aether`
- `--native`

need orchestration tests. Today, much of that behavior is only reachable through one large control path in `main.rs`. A clearer CLI/driver split makes it much easier to:

- test command selection without running full compilation
- test runtime error rendering without re-entering unrelated logic
- test parity configuration and output formatting independently

### 5. Flux now needs an explicit “driver” boundary

Proposal 0153 clarified the compiler pipeline:

- `Core` is semantic-only
- `Aether` is backend-only lowering

The binary/application side needs the same discipline. We now need an explicit layer that answers:

- which pipeline should run?
- for which file(s)?
- with which backend?
- with which dump mode?
- with which cache behavior?

That is a driver concern, not a process-entry concern.

## Goals
[goals]: #goals

- Reduce `src/main.rs` to a small entrypoint plus minimal top-level wiring.
- Introduce a first-class `cli/` layer for argument parsing, command dispatch, and user-facing rendering.
- Introduce a first-class `driver/` layer for orchestration of compilation, running, dumping, parity, and cache inspection.
- Make native runtime diagnostic rendering a dedicated subsystem instead of a collection of helpers in `main.rs`.
- Keep all existing command semantics stable unless this proposal explicitly says otherwise.
- Improve testability of command selection, runtime reporting, and orchestration behavior.

## Non-Goals
[non-goals]: #non-goals

- No change to Flux language semantics.
- No change to the semantic compiler pipeline (`syntax -> core -> aether -> backend IR`).
- No new semantic IR.
- No redesign of the parity ladder itself.
- No requirement to introduce a third-party CLI framework.
- No merge of compiler internals into the application layer.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Before

Today, the binary behaves roughly like this:

```text
main.rs
  -> parse raw args
  -> decide command behavior
  -> build module graph
  -> infer / compile / dump
  -> execute VM or native path
  -> maybe run parity
  -> maybe capture child stderr
  -> maybe render runtime diagnostics
  -> maybe print cache summaries
```

Almost all of that logic lives in a single file.

### After

After this proposal, the binary should behave like this:

```text
main.rs
  -> cli::run(args)
       -> parse into command/config
       -> dispatch to a command module
            -> use driver/session helpers
            -> return structured output / status
       -> render user-facing output
```

The responsibilities become:

- `main.rs`
  - process entry only
- `cli/`
  - user-facing commands, options, and rendering
- `driver/`
  - compiler/runtime orchestration
- compiler modules
  - compilation semantics and lowering only

### Expected user-visible behavior

The goal is **stable command behavior**:

- `flux path/to/file.flx`
- `flux path/to/file.flx --dump-core`
- `flux path/to/file.flx --dump-aether`
- `flux path/to/file.flx --native`
- `flux parity-check ...`
- `flux native-cache-info ...`

should continue to work the same way.

The change is internal architecture, not a user-facing CLI redesign.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

## Design notes

This proposal follows three established principles for long-lived compiler tooling:

1. Keep the process entrypoint thin.
2. Centralize configuration/state in a session-like structure.
3. Separate command orchestration from compilation internals and from rendering.

The goal is to make command behavior explicit and testable without requiring full compilation for every path, while keeping the compiler pipeline itself isolated from application concerns.

## 1. Introduce an explicit CLI layer

Add `src/cli/` with the following structure:

```text
src/cli/
  mod.rs
  args.rs
  commands/
    run.rs
    parity.rs
    native_cache.rs
    tests.rs
    help.rs
  render/
    diagnostics.rs
    parity.rs
    progress.rs
    text.rs
```

Responsibilities:

- `args.rs`
  - parse raw argv into structured command/config types
- `commands/*`
  - map one user command to one driver invocation
- `render/*`
  - user-facing text rendering
  - diagnostics formatting glue
  - progress output

The CLI layer must not directly implement compiler lowering or backend code generation.

## 2. Introduce an explicit driver layer

Add `src/driver/` with the following structure:

```text
src/driver/
  mod.rs
  session.rs
  config.rs
  pipeline.rs
  module_graph.rs
  execute.rs
  native.rs
  dumps.rs
  parity.rs
  runtime_errors.rs
  cache.rs
```

Responsibilities:

- `session.rs`
  - own process-wide config for one invocation
  - paths, roots, cache settings, backend mode, dump mode
- `pipeline.rs`
  - run shared compile/lower steps
- `module_graph.rs`
  - build and validate entry/dependency graphs for commands
- `execute.rs`
  - run VM/native execution paths
- `native.rs`
  - native module compilation orchestration
  - support object build
  - binary linking
- `dumps.rs`
  - `--dump-core`
  - `--dump-aether`
  - backend dump helpers
- `parity.rs`
  - parity command orchestration, ways expansion, capture selection
- `runtime_errors.rs`
  - native child stderr capture
  - runtime panic decoding
  - structured Flux runtime diagnostic reconstruction
- `cache.rs`
  - native cache inspection and summaries

The driver layer is allowed to depend on compiler internals because it orchestrates them. The compiler internals must not depend on the driver layer.

## 3. Reduce `main.rs` to an entrypoint

The end-state of `src/main.rs` should be approximately:

```rust
fn main() -> std::process::ExitCode {
    flux::cli::run(std::env::args_os())
}
```

Exact function shape may vary, but the file should not own command-specific orchestration.

Allowed contents of `main.rs`:

- process entry
- minimal error-to-exit-code mapping
- feature-gated binary bootstrap if required

Disallowed end-state:

- large command dispatch tables
- native compilation orchestration
- parity ladder logic
- native runtime panic parsing/rendering
- dump rendering helpers

## 4. Define structured command/config types

The CLI should convert raw argv into structured command types such as:

```rust
enum Command {
    Run(RunCommand),
    ParityCheck(ParityCommand),
    NativeCacheInfo(CacheInfoCommand),
    Help(HelpCommand),
}
```

with configuration structs such as:

```rust
struct RunCommand {
    path: PathBuf,
    backend: BackendMode,
    dump: DumpMode,
    trace: TraceMode,
    cache: CacheMode,
    strict_mode: bool,
}
```

This prevents business logic from repeatedly scanning raw `Vec<String>` and re-deriving state in multiple places.

## 5. Make runtime diagnostics a first-class subsystem

The current native runtime diagnostic bridge should move into `src/driver/runtime_errors.rs`.

This module should own:

- native panic message extraction
- runtime frame parsing
- source-span inference
- fallback stack reconstruction
- rendering of Flux-style runtime diagnostics for native execution

Its public API should accept structured inputs and return structured outputs where possible, for example:

```rust
pub fn render_native_runtime_failure(
    path: &Path,
    stderr: &str,
) -> Option<RenderedRuntimeDiagnostic>
```

This isolates a growing subsystem that is currently embedded in `main.rs`.

## 6. Separate renderers from orchestration

Today, command execution and text output are often interleaved. This proposal separates them:

- command/driver code returns structured results
- renderer modules convert those results into terminal output

Examples:

- parity results should be rendered by `cli/render/parity.rs`
- diagnostics output should be rendered by `cli/render/diagnostics.rs`
- progress lines should be rendered by `cli/render/progress.rs`

This makes it easier to:

- keep command logic deterministic
- test rendering independently
- evolve output formats without touching orchestration logic

## 7. Keep compiler/binary dependency direction clean

Dependency direction after this proposal should be:

```text
main -> cli -> driver -> compiler internals
```

but never:

```text
compiler internals -> driver
compiler internals -> cli
```

That prevents application concerns from leaking back into semantic or backend code.

## 8. Preserve current command semantics

The following command behaviors are part of the compatibility contract and should remain unchanged during the refactor:

- plain program execution
- `--dump-core`
- `--dump-core=debug`
- `--dump-aether`
- `--dump-aether=debug`
- `--trace`
- `--native`
- `--emit-llvm`
- `--emit-binary`
- `parity-check`
- `native-cache-info`

This proposal is an architectural split, not a CLI redesign proposal.

## Implementation strategy
[implementation-strategy]: #implementation-strategy

This proposal should be implemented in stages.

### Stage 1: Establish the CLI/driver shell

Goals:

- create `src/cli/` and `src/driver/`
- move argument parsing and top-level command selection out of `main.rs`
- leave lower-level execution paths temporarily delegated back into existing helpers if needed

Requirements:

- `main.rs` becomes thin immediately
- the binary still supports the same command set
- no semantic compiler behavior changes

### Stage 2: Move command orchestration into the driver

Goals:

- move run/native/parity/cache command orchestration out of `main.rs`
- move dump-mode orchestration into `driver/dumps.rs`
- move native module compilation orchestration into `driver/native.rs`

Requirements:

- `main.rs` no longer owns backend orchestration
- `cli/commands/*` call driver APIs instead of raw helper code

### Stage 3: Move runtime diagnostic bridging into a dedicated module

Goals:

- move native runtime panic capture/parsing/rendering into `driver/runtime_errors.rs`
- move user-facing diagnostic output into `cli/render/diagnostics.rs`

Requirements:

- runtime diagnostic behavior remains stable
- parity/runtime error fixtures stay green

### Stage 4: Finish render/orchestration split

Goals:

- render parity output through dedicated renderers
- render progress lines through dedicated renderers
- remove terminal-printing logic from driver code where practical

Requirements:

- command logic becomes testable without snapshotting raw side effects everywhere

### Stage 5: Remove legacy helpers from `main.rs`

Goals:

- delete leftover helper functions that now live in `cli/` or `driver/`
- keep `main.rs` as an entrypoint only

Requirements:

- no command-specific helper subsystems remain in `main.rs`

## Testing strategy
[testing-strategy]: #testing-strategy

The refactor must preserve behavior while improving modularity.

### 1. Command compatibility tests

Keep or add tests that verify:

- `flux file.flx`
- `flux file.flx --dump-core`
- `flux file.flx --dump-aether`
- `flux file.flx --native`
- `flux parity-check ...`
- `flux native-cache-info ...`

continue to behave the same as before.

### 2. Renderer tests

Add focused tests for:

- parity summary rendering
- runtime diagnostic rendering
- native runtime panic translation
- progress line formatting

These should not require running the whole compiler pipeline when only formatting is under test.

### 3. Driver tests

Add tests for:

- command-to-driver config translation
- dump-mode selection
- backend selection
- cache option handling
- no-`main` native entry execution orchestration

### 4. Regression suites

The following suites should stay green during the refactor:

- `cargo test --test test_runner_cli`
- `cargo test --test aether_cli_snapshots`
- `cargo test --test examples_fixtures_snapshots`
- `cargo test --test backend_representation_runtime_tests`
- `cargo run -- parity-check examples/runtime_errors`
- `cargo run -- parity-check examples/basics`

### 5. Architecture checks

Add at least one test or static assertion equivalent that enforces:

- `main.rs` no longer owns parity orchestration
- `main.rs` no longer owns native runtime diagnostic reconstruction

These checks do not need to be fancy, but the architecture split should be observable in the codebase.

## Drawbacks
[drawbacks]: #drawbacks

- The refactor touches a central part of the binary application layer.
- Temporary churn is likely in command-related tests and snapshots.
- The split introduces more files and modules, which is good for architecture but increases navigation surface.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Alternative: keep `main.rs` as the orchestration hub

Rejected.

That approach has already produced a file that is too large and mixes too many concerns. It does not scale with:

- new subcommands
- native runtime diagnostic work
- parity/reporting growth
- cache/debugging tooling

### Alternative: move only native helpers out of `main.rs`

Insufficient.

Native orchestration is only one part of the growth. Parity, dumping, and command dispatch would still remain entangled.

### Alternative: move only renderers out of `main.rs`

Also insufficient.

That would improve output code somewhat but would leave the command and driver responsibilities mixed together.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- Should command parsing stay fully manual, or should it be wrapped in a more structured parser module with stronger typed validation?
- Should the driver return structured result objects everywhere, or are there a few high-volume streaming paths where direct rendering is acceptable?
- Should parity command configuration live under `cli::commands::parity` or under `driver::parity` as the canonical configuration owner?

These do not block the proposal. The key architectural split does not depend on their exact resolution.

## Future possibilities
[future-possibilities]: #future-possibilities

Once this split exists, Flux can more easily add:

- richer machine-readable command outputs
- separate batch and interactive frontends
- cleaner editor/tool integration hooks
- command-specific telemetry/timing reports
- better runtime diagnostic introspection for native execution

The main value of this proposal is that it gives the binary/application side the same architectural discipline the compiler pipeline already has.
