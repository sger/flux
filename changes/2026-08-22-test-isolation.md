### Fixed

- Test suite flakiness under parallel execution. `cargo test --all --all-features`
  intermittently failed targets that passed in isolation, with failures moving
  between runs and looking unrelated to each other — a
  `missing global mapping for local index N` escaping the module linker, or a
  native fixture emitting nothing so the harness reported
  `no native summary for <fixture>`.

  Both came from one cause: tests shared `target/test-scratch/` and, more
  damagingly, the single compilation cache at `target/flux`. `resolve_cache_root`
  walks up to the nearest `Cargo.toml`, so every fixture written under the repo
  resolved to the same cache root, and concurrent test binaries read and wrote
  each other's `.flxi` interfaces and bytecode.

  Added `tests/support/scratch.rs`: a `Scratch` guard giving each test a unique
  directory (pid + counter, removed on drop) and its own `--cache-dir`.

  Two gaps `--no-cache` did not close, and this does:
  - the native backend writes shared build artifacts under the cache root even
    with `--no-cache`, which is why `*_native_tests` lost these races most often;
  - a test that *exercises* caching cannot pass `--no-cache` at all —
    `field_order_survives_the_warm_module_cache` needs a cold run then a warm
    one, and now gets a private cache rather than the shared one.

- `run_fixture` folds stderr into its returned text when a run fails, so a
  native compile or link error appears in the panic message instead of
  surfacing as an unexplained missing summary.

### Changed

- `stdlib_process_tests` runs its fixture through one `assert_backends_agree`
  test rather than a VM test plus a parity test. The two ran on separate threads
  over the same fixture, and `assert_backends_agree` already covers the VM leg.

### Docs

- `docs/known_issues.md` KI-010 records the remaining exposure: several fixtures
  hardcode `target/test-scratch/...` paths inside Flux source, which cannot be
  redirected from the Rust side. Each is currently unique to one test, so they do
  not collide today.
