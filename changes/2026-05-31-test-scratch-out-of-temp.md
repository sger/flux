### Changed
- Test fixtures and scratch directories are now written under `target/test-scratch/`
  inside the project tree instead of the system temp dir (`%TEMP%`). Every native and
  integration test that wrote a throwaway `.flx` fixture (or `.ll` IR file) into
  `std::env::temp_dir()` now roots it at `<CARGO_MANIFEST_DIR>/target/test-scratch/`.
  Because the fixture then lives inside the project, flux's cache-root resolution finds
  the repo `Cargo.toml` and uses the normal `target/flux` cache rather than creating a
  `.flux/cache` fallback next to the fixture — so test runs no longer seed `.flux`
  caches (previously hundreds of MB) into `%TEMP%`. Leftover scratch now lands under
  `target/` (git-ignored, removed by `cargo clean`).
