### Added
- `Flume.Roots`: the Flux half of package resolution (proposal 0177 Phase 1).
  It reads `flux.toml` through `Flume.Manifest`, walks path dependencies
  transitively, derives each package's namespace, and emits one record per
  resolved package. Registry and dev dependencies are rejected with a "not
  supported until Phase 2" message rather than silently ignored.
- A project with a `flux.toml` now builds its path dependencies: each package
  contributes one namespace-scoped module root, so `import Shared.Util`
  resolves through the dependency that owns `Shared`. Script mode and `--root`
  are unchanged and still use unscoped roots.
- `E470 MANIFEST UNRESOLVED`: a manifest that exists but cannot be resolved is
  reported with the Flux resolver's own message. The unscoped roots are kept
  so the error is not buried under missing-`Flow.*` cascades.
