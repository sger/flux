### Added
- `flux-lsp`: `completionItem/resolve`. Completion items for keywords,
  built-in effects, and built-in types now defer their documentation: the
  initial completion response — which lists every keyword on each keystroke —
  carries only a small `data` tag, and the rich markdown card (the same one
  hover shows) is filled in lazily when the client resolves the highlighted
  item. New `keywords::builtin_type_doc` table backs the type cards (the
  universal built-ins; `Result` is a `Flow.Async` module type, not a built-in,
  so it resolves to no doc). The server advertises
  `completionProvider.resolveProvider`.
