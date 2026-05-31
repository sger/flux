### Changed
- Relocated the remaining `#[cfg(test)]` unit-test scratch out of the system temp dir
  into `target/test-scratch/`. The cache-serialization/validation, bytecode and native
  module-cache, module-interface, module-graph, LLVM IR pretty-printer, and parity-fixture
  unit tests all wrote throwaway files into `std::env::temp_dir()`; they now root under
  `<CARGO_MANIFEST_DIR>/target/test-scratch/`. Combined with the earlier integration-test
  relocation, a full `cargo test` run no longer seeds any `flux*` / `.flux` directories
  into `%TEMP%`. (The two intentional temp references remain: the projectless cache-root
  fallback in `llvm::pipeline`, and the native-scratch test assertion that verifies output
  is *not* under `%TEMP%`.)
