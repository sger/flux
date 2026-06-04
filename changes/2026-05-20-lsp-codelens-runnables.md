### Added
- `flux-lsp` + VS Code extension: CodeLens runnables. A "▶ Run" lens sits above
  a top-level `fn main`, and a "▶ Run Test" lens above each top-level `fn
  test_*`. The server (`textDocument/codeLens`, `handlers::code_lens`) locates
  the runnables and names the command; the extension's `flux.run` / `flux.runTest`
  commands launch the Flux CLI in a terminal — `cargo run -- <file>` for a run,
  `… <file> --test --test-filter <name>` for a single test. The base command is
  configurable via the new `flux.runCommand` setting (default `cargo run --`).
