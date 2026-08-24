### Added
- `flux init [name] [--lib]` and `flux new <name> [--lib]` scaffold a package
  (proposal 0177 Phase 1). A binary gets `src/main.flx`; a library gets the
  namespace root module at `src/<Namespace>.flx`, so `http-client` produces
  `src/HttpClient.flx`. Without a name, `init` names the package for its
  directory.
- `Flume.Cli`: the command surface in Flux. It owns the manifest template,
  namespace derivation, and entry-point selection — honouring `[[bin]]` and
  `[lib]` targets when declared and the conventional layout otherwise. The Rust
  CLI only forwards the command and reports the result, so no packaging
  decision is made outside Flux.

### Docs
- KI-019: some CLI commands print an error but exit `0`, because `run_command`
  ends with an unconditional `ExitCode::SUCCESS`. Affects the implicit
  `flux <file.flx>` run path, `cache-info`, and `fmt`.
