### Added
- LSP "View compiler stage" commands, the Flux analogue of rust-analyzer's
  "View HIR / MIR". The server answers three custom requests — `flux/viewTokens`,
  `flux/viewCoreIr`, `flux/viewBytecode` — rendering the token stream, Core IR,
  or VM bytecode of a document as text (a parse/compile error comes back as
  comment text so the view always shows something). The VS Code extension adds
  matching commands — **Flux: View Tokens / View Core IR / View Bytecode** — that
  open the dump in a scratch editor beside the source.
