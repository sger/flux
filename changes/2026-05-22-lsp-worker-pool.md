### Changed
- `flux-lsp`: read requests are now served by a small pool of worker threads
  instead of a single one. Previously every read (hover, completion, goto,
  symbol search, …) ran on one worker, so a genuinely slow request — a
  workspace-wide symbol query over a big index, or `workspace/diagnostic` with
  `scanAllFiles` — blocked every other read behind it until it finished. The
  pool (sized to available parallelism, clamped to 2–8) shares one MPMC work
  queue, so a slow request occupies a single worker while interactive requests
  keep flowing through the others. Jobs only read immutable `Arc<Snapshot>`
  data and the one shared cache (semantic tokens) is mutex-guarded, so the
  results are unchanged — only the latency under concurrent load improves.
