### Added

- `Flow.Crypto` with `sha256`, `sha256_file`, and `verify_sha256`
  (proposal 0178, item 4). Digests are 64 lowercase hex characters, matching
  what `sha256sum` and `git hash-object` print, so they can be compared
  against other tools directly.

  `sha256` carries **no effect** — hashing observes nothing outside its
  argument, so it is callable from a function with no effect annotation at
  all. This is the first pure primop in this proposal, and it is what lets a
  manifest parser be provably pure while the fetcher feeding it wears its I/O
  in its type.

  `sha256_file` carries `FileSystem` and returns `Result<String, IoError>`.
  It streams the file in fixed-size chunks rather than loading it whole, so
  hashing a large artifact costs a fixed buffer instead of its full size in
  memory — the reason it exists separately from `sha256(read_file(p))`.

  SHA-256 is implemented twice with no shared code: the VM uses the `sha2`
  crate, and the native backend runs a new hand-written implementation in
  `runtime/c/sha256.c`. Both are checked against the published FIPS 180-4
  vectors, and the digests are additionally cross-checked against the
  compiler's own `sha2` output rather than only against pasted constants.

### Changed

- Hex encoding is consolidated into `shared::hex`. Three byte-identical
  private copies (`compiler/module_interface.rs`,
  `bytecode/bytecode_cache/module_cache.rs`, and `llvm/module_cache.rs`) now
  share one implementation, which `Flow.Crypto` also uses — so a digest
  computed in Flux and one written into a cache fingerprint are spelled
  identically by construction.
