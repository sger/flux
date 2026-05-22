### Fixed
- `flux-lsp`: a panic in the compiler frontend during a buffer's type inference
  is no longer swallowed silently. The `catch_unwind` around inference
  previously discarded the panic with `.ok()`, so a real frontend bug surfaced
  only as missing hover/diagnostics for that file with nothing in the logs. The
  caught panic is now logged at `error` (with its message), and the shared
  compiler's per-file state is reset eagerly on the panic path — `class_env` and
  the inference config are mutated in place, so a mid-mutation panic could
  otherwise leave half-built scratch for the next buffer to inherit (previously
  only cleaned up lazily at the start of the next analysis). The panic-message
  extraction is now a shared `util::panic_message` helper, reused by the worker
  pool's panic guard.
