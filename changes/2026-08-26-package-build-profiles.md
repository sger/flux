### Added

- Added `dev` and `release` package build profiles in `flux.toml`.
- Added `--profile`, `--vm`, and `--no-optimize` package CLI controls.
- Added profile-aware package metadata and build plans.

### Changed

- `dev` defaults to the VM without optimization; `release` selects the native
  LLVM backend with optimization enabled.
- Explicit CLI backend and optimization flags override the selected profile.
- Semantic cache keys now include optimization settings using default-eliding
  configuration documents.

### Fixed

- VM module bytecode cache keys now distinguish optimized and unoptimized
  builds.
