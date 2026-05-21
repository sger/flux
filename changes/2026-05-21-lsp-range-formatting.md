### Added
- `flux-lsp`: `textDocument/rangeFormatting` — "Format Selection" now formats
  just the selected range instead of the whole file. Flux's formatter is
  whole-file, so the handler formats the buffer, diffs the result against the
  original at line granularity, and returns only the change hunks whose lines
  intersect the selection — text outside the selection is left untouched.
  (Contiguous changed lines coalesce into one hunk, so a selection inside such a
  block formats that whole contiguous region; in practice the surrounding
  unchanged lines keep hunks small and local.)
