### Fixed
- `flux-lsp`: a panic in any read-request handler no longer wedges the whole
  server. The single worker thread ran each job (`hover`, `completion`,
  `goto`, …) without an unwind guard, so one panic — e.g. span arithmetic or a
  frontend call choking on a pathological buffer — killed the thread; the main
  loop's `work_tx.send` then failed silently and every later read request went
  unanswered until the editor was restarted. The worker now contains the unwind
  (`catch_unwind`), answers just the offending request with a JSON-RPC internal
  error (`-32603`) carrying the panic message, logs it at `error`, and keeps
  serving subsequent requests. Inference was already guarded; this closes the
  same gap for the per-request handlers.
