### Changed
- The parser now records `///` / `/** */` doc comments on the `Program` it
  produces (`Program::doc_comments`, keyed by the source line of the declaration
  each contiguous run sits above), exposed via `Program::doc_for_line`. Doc
  comments were previously dropped after lexing, surviving only in source text.
- `flux-lsp`: hover, `completionItem/resolve`, and signature help now read a
  declaration's documentation from this parsed index instead of re-scanning the
  source buffer for `///` runs on every request. The scan was correct but
  repeated per request and per program; the line-keyed lookup is O(1), covers
  nested declarations (class/instance methods, data variants) uniformly, and
  also picks up `/** */` block doc comments, which the old `///`-only scan
  missed. The `doc_comments` module's `doc_comment_above` source scanner is
  removed in favor of `doc_for` / `member_doc` lookups over the AST.
