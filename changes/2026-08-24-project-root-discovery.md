### Changed
- `find_project_root` now looks for `flux.toml` before falling back to
  `Cargo.toml`, so a Flux project outside this checkout has a real project root
  and its build artifacts land in `<project>/target/flux` (proposal 0177
  Phase 1). The `Cargo.toml` fallback is retained for the compiler's own test
  corpus and is consulted only after the whole ancestor chain has been searched
  for `flux.toml`.

### Fixed
- Module search roots no longer depend on the process working directory.
  `collect_roots` looked for `src`/`lib` beside `.`, so `flux run foo/bar.flx`
  and `cd foo && flux run bar.flx` resolved different roots and the latter
  failed to find sibling modules. Roots are now computed from the entry file
  and its project root.
