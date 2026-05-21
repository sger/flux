### Added
- `flux-lsp`: a file-level "▶ Run all tests" CodeLens. When a file has more than
  one top-level `test_*` function, a single runnable lens sits above the first
  test and runs the whole file's suite (`flux.runTests` → the Flux CLI with
  `--test` and no filter), complementing the per-test "▶ Run Test" lenses. A
  lone test gets no run-all lens, since it would just duplicate that test's own
  runner. The VS Code extension registers the matching `flux.runTests` command,
  which launches the configured runner (`flux.runCommand`, default
  `cargo run --`) on the file in the shared terminal.
