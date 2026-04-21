- Feature Name: Test Suite Reorganization by Compiler Phase
- Start Date: 2026-04-21
- Status: Proposed
- Proposal PR:
- Flux Issue:
- Depends on: —

# Proposal 0169: Test Suite Reorganization by Compiler Phase

## Summary
[summary]: #summary

Reorganize the ~105 flat integration test files under `tests/` into subdirectories aligned with the Flux compilation pipeline (lexer → parser → AST → type inference → core IR → aether → bytecode → VM/runtime → native/LLVM → diagnostics → integration). Remove five snapshot re-export shim files, merge four clusters of duplicated test files, and extract shared helpers into `tests/support/`. The change is purely organizational — no test logic is deleted.

## Motivation
[motivation]: #motivation

The `tests/` directory has grown to 105 flat files with no structural grouping. Consequences:

- Contributors cannot tell which phase a test exercises without opening the file.
- Phase ownership is invisible in PR review and CI failure output.
- Helpers have been copy-pasted (e.g. `run_native` / `run_vm` across three primop files; `collect_rs_files` across three type-infer guards).
- Multiple files differ only in fixture directory (examples snapshots, error-fixture snapshots).
- Snapshot wrapper files (`snapshot_lexer.rs`, `snapshot_parser.rs`, …) are 3–4 line re-exports of `tests/snapshots/<phase>/mod.rs` and add no value.

A phase-aligned layout makes failure locality obvious, reduces duplication, and gives each future compiler phase an obvious home for its tests.

## Guide-level explanation
[guide-level]: #guide-level-explanation

After this proposal, `tests/` maps 1:1 to the pipeline described in `CLAUDE.md`:

```
tests/
├── support/                 shared helpers (+ new primop_parity.rs)
├── lexer/                   lexer, token, interner, parse perf
├── parser/                  parser, recovery, list literal syntax, spans, linter
├── ast_passes/              desugar, constant_fold, free_vars, rename,
│                            tail_position, complexity, visit/fold smoke, tail_call
├── type_inference/          HM inference, semantics matrices, static typing,
│                            static type validation, contract, constrained params,
│                            non-zero, pattern validation, perf, guards/
├── core_ir/                 core type contract, primop lowering/effects,
│                            backend repr, ir pipeline
├── aether/                  cli snapshots, core regressions, FBIP diagnostics
├── bytecode/                opcodes, superinstruction, cmp/jump fusion
├── vm_runtime/              vm, program, stdlib, stackless drop, contextual instance
├── native_llvm/             llvm_adt, llvm_codegen (each merged with its _snapshots
│                            twin), llvm_closures, llvm_type_class,
│                            native_runtime_error_spans, primop_native (merged),
│                            safe_arithmetic
├── diagnostics/             render, span render, aggregator, env, registry,
│                            unified, extended rendering, docs guard
├── integration/             modules, flow prelude, cache invalidation,
│                            compiler rules, test runner CLI, cross module,
│                            compiler_optimization (merged)
├── examples/                basics / advanced / fixtures snapshot harnesses
├── error_fixtures/          parser / compiler / runtime error fixture snapshots
└── snapshots/               (unchanged, already phase-structured)
```

The `tests/flux/`, `tests/fixtures/`, `tests/parity/` fixture directories keep their current roles. Only Rust harness files move.

## Reference-level explanation
[reference-level]: #reference-level-explanation

### Duplication to eliminate

1. **Snapshot re-export shims (5 files).** `snapshot_lexer.rs`, `snapshot_parser.rs`, `snapshot_formatter.rs`, `snapshot_bytecode.rs`, `snapshot_diagnostics.rs` each contain only `mod snapshots;` plus a `pub use`. They exist because `tests/snapshots/<phase>/mod.rs` holds the real logic. Delete the shims and register the phase modules directly via `Cargo.toml` `[[test]]` entries or via the new phase subdirectories' `mod.rs`.

2. **LLVM pairs (2 pairs).** `llvm_adt.rs` + `llvm_adt_snapshots.rs` and `llvm_codegen.rs` + `llvm_codegen_snapshots.rs` each redefine a `parse_and_lower_core`-style helper. Merge each pair into one file under `tests/native_llvm/` with snapshots gated by `#[cfg(feature = "insta")]` or submodules.

3. **Primop native triplet.** `bitwise_primop_native_tests.rs`, `math_primop_native_tests.rs`, `safe_arithmetic_tests.rs` each copy `run_native` / `run_vm` stubs that resolve `tests/parity/`. Extract to `tests/support/primop_parity.rs` and make the three files thin callers.

4. **Type-infer guards (3 files).** `type_infer_docs_guard.rs`, `type_infer_complexity_guard.rs`, `type_infer_naming_guard.rs` each reimplement `collect_rs_files` and `is_fn_start_line`. Merge into `tests/type_inference/guards/type_guards_unified.rs` with one module per guard.

5. **Optimization pair.** `compiler_optimization_integration_test.rs` and `optimization_integration_test.rs` cover nearly the same surface. Merge into a single `tests/integration/compiler_optimization_tests.rs`.

6. **Snapshot harness invocations.** `examples_basics_snapshots.rs`, `examples_advanced_snapshots.rs`, `examples_fixtures_snapshots.rs`, and the three `*_error_fixtures_snapshots.rs` files all drive `support/examples_snapshot.rs` with a different fixture root. Keep the files (they provide distinct `[[test]]` targets for parallel CI) but consolidate them under `tests/examples/` and `tests/error_fixtures/` so the duplication is visible and parameterizable later.

### Ambiguous files (explicit placement)

| File | Destination | Reason |
|---|---|---|
| `tail_call_tests.rs` | `ast_passes/` | Tail-position analysis is an AST pass even though the test observes bytecode output. |
| `pattern_validation.rs`, `match_some_pattern_tests.rs` | `type_inference/` | Parse succeeds; the assertions are on type/exhaustiveness rules. |
| `span_tests.rs` | `parser/` | Drives parser span propagation. |
| `mutual_recursion.rs` | `integration/` | Spans inference and runtime. |
| `cross_module_function_tests.rs` | `integration/` | Module system + evaluation. |

### Mechanics

- Moves are `git mv` only; no content edits on first pass.
- Update `Cargo.toml` `[[test]]` stanzas (if any) to new paths.
- Add a short `tests/README.md` describing the phase layout and the duplication rules:
  - "New tests go under the subdirectory for the phase they assert against."
  - "Shared helpers live in `tests/support/`."
- Run `cargo test --all --all-features` and `cargo insta review` to confirm snapshot paths still resolve (snapshots live alongside fixtures, not alongside test files, so moves should be transparent).

## Drawbacks
[drawbacks]: #drawbacks

- `git blame` gets noisier for one commit per moved file. Mitigated by `.git-blame-ignore-revs`.
- Any external tooling or docs referencing flat `tests/foo.rs` paths must be updated.
- Cargo's default `tests/*.rs` autodiscovery only walks the top level; nested files require explicit `[[test]]` entries (or one `mod.rs` per subdir that re-exports children). This is a one-time configuration cost.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- **Alternative: do nothing.** The flat layout will keep growing; each new phase (e.g. the LSP, actor runtime) will add another 5–10 files with no home.
- **Alternative: rename only, no subdirectories.** Prefixes like `phase04_type_inference_*` would avoid the Cargo config change but make filename length unwieldy and still wouldn't colocate helpers.
- **Alternative: move to a `cargo-nextest` group config.** Solves CI grouping but not navigability.

The subdirectory approach is the only one that addresses both navigability and helper colocation.

## Prior art
[prior-art]: #prior-art

- `rustc`'s `src/test/` split (`ui/`, `codegen/`, `mir-opt/`, …).
- GHC's `testsuite/tests/<phase>` layout.
- The already-phase-structured `tests/snapshots/` directory inside Flux — this proposal brings the Rust harness files into line with what snapshots already do.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- Should `tests/perf_*.rs` move to a dedicated `tests/perf/` root or stay inside the phase they exercise? (Proposal: keep with their phase until there are >5 perf files.)
- Should `snapshots/` be moved *inside* each phase directory (e.g. `tests/lexer/snapshots/`) for locality, at the cost of touching every existing snapshot path? (Proposal: defer — not worth the churn now.)
- Do we want a `tests/regressions/` directory for cross-phase regression suites, or leave `regression_snapshots.rs` at top level?

## Future possibilities
[future-possibilities]: #future-possibilities

- Once layout stabilizes, parameterize the examples/error-fixture harnesses with a `for fixture_dir in [...]` loop driven by a single `[[test]]` target.
- Add a CI check that new test files land under an existing phase directory (lint rule: no new top-level `tests/*.rs`).
- Use the per-phase grouping to drive targeted CI jobs (e.g. run only `tests/native_llvm/` when `src/llvm/` or `src/lir/` changes).
