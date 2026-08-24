### Fixed
- `flux test` could not see a project's path dependencies (KI-021). The test
  path resolved modules with unscoped roots, so a package that built and ran
  failed to compile under `flux test` with `E012 Unknown Module Member`. It now
  uses the same package-aware root resolution as the run path. The native test
  backend forwards only explicit `--root` flags to its subprocess, which
  resolves the manifest itself.
