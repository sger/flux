### Added
- `flux build`, `flux run`, `flux test`, and `flux check` operate on the
  current package (proposal 0177 Phase 1). The entry point is chosen by
  `Flume.Cli`: `src/main.flx` by convention, a `[lib]` or namespace root module
  for a library, and any declared `[[bin]]` by name with `--bin <name>`.
  `flux run -- <args>` forwards arguments to the program.
- A `check_only` driver flag, so `build` and `check` run the frontend and
  compile — surfacing every compile-time error — without executing the program.

### Changed
- `flux run` with no file argument now runs the current package.
  `flux run <file.flx>` keeps its script-mode meaning, and running a loose file
  outside any project is unaffected.
