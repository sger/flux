### Changed
- `flux-lsp`: `textDocument/didChange` analysis is now debounced. Previously
  every keystroke re-parsed and re-ran HM inference for the edited file's whole
  module component synchronously on the main loop before the next message could
  be read, so a fast typist paid one full cross-file analysis per character. An
  edit is now *staged* cheaply (buffer + symbol index updated, stale snapshots
  invalidated) and the expensive component re-analysis is deferred ~150 ms; a
  burst of keystrokes collapses into a single analysis pass once typing settles.
  Read requests (hover, completion, goto, …) arriving mid-burst stay correct —
  they rebuild the snapshot they need lazily from the staged text via
  `ensure_snapshot` — and immediate content events (open/close/save/watched-file
  changes) flush any pending edit first so the two passes never interleave.
