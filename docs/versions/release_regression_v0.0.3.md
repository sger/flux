# Flux v0.0.3 Release Regression Checklist

Use this checklist before tagging `v0.0.3`.

---

## 1) Baseline Quality Gates (Must Pass)

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo test --features jit
```

Notes:
- `cargo test --features jit` is required for backend parity confidence.
- If snapshots changed intentionally, update and re-run tests (see section 3).

---

## 2) VM vs JIT Parity Gates (Must Pass)

Run representative examples/tests on both backends and compare outputs/errors.

```bash
cargo run -- --test examples/tests/math_test.flx
cargo run --features jit -- --test examples/tests/math_test.flx --jit

scripts/run_examples.sh --all
scripts/run_examples.sh --all --jit
```

Minimum parity checks:
- same pass/fail outcome
- same visible output (stdout/stderr semantics)
- same error class for failing programs

---

## 3) Snapshot Regression Gates (Must Pass)

Flux already has snapshot coverage for lexer/parser/bytecode/diagnostics/examples.

```bash
cargo test snapshot_lexer
cargo test snapshot_parser
cargo test snapshot_bytecode
cargo test snapshot_diagnostics
cargo test regression_snapshots
```

If expected changes occurred:

```bash
cargo insta test --accept
cargo test
```

Rule:
- never accept snapshot diffs without a matching changelog/proposal note.

---

## 4) Compile-Fail / Diagnostics Gates (Must Pass)

Focus on error stability for parser + diagnostics + recovery:

```bash
cargo test parser_recovery
cargo test parser_tests
cargo test diagnostic_render_tests
cargo test unified_diagnostics_tests
```

Manual spot checks (recommended):
- unterminated strings
- malformed match patterns
- module/import resolution errors
- wrong builtin arity/type errors

---

## 5) Performance Regression Gates (Should Pass)

Run at least one stable benchmark suite and regenerate perf report:

```bash
cargo bench --bench lexer_bench
rm -rf baseline_criterion
cp -r target/criterion baseline_criterion
cargo bench --bench lexer_bench
rust-script scripts/bench_report.rs
```

Expected output:
- `reports/PERF_REPORT.md` updated.

Release policy (recommended):
- investigate any >5% slowdown on key benches before release.

---

## 6) Docs / Release Integrity Gates (Must Pass)

Verify release docs are in sync:
- `CHANGELOG.md`
- `docs/versions/whats_new_v0.0.3.md`
- `README.md` feature and command examples

Also ensure:
- new/renamed flags are documented
- proposal statuses reflect shipped behavior

---

## 7) Freeze Policy (Recommended)

For final release week:
1. Feature freeze.
2. Bugfix-only merges.
3. Every bugfix must include a regression test.
4. Re-run sections 1-4 before tagging.

---

## 8) What Mature Compilers Commonly Do (Reference)

Rust (`rustc`):
- heavy compile-fail/UI test coverage
- strict perf tracking and regression monitoring

Haskell (GHC):
- broad regression test suite with expected output files
- optimizer correctness checks

Elixir/BEAM:
- strong semantic/backward-compatibility tests
- runtime behavior consistency checks across versions

Flux mapping:
- snapshots + diagnostics tests = UI/regression layer
- VM/JIT parity runs = backend consistency layer
- benchmark report = perf guardrail layer
