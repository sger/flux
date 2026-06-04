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
- The one CLI test that exercises the cached/relocatable module-linker path
  (`run_compiles_top_level_match_and_destructure`) now uses an isolated per-fixture
  `--cache-dir` under `target/test-scratch/` instead of the shared `target/flux` cache,
  so concurrent test threads no longer race the prelude artifacts ("interface
  fingerprint changed").
