### Added
- `flux-lsp`: hovering a built-in effect label now shows a structured doc
  card — the same ``**`Name`** — summary`` + prose + ```flux shape as keyword
  hovers — instead of a bare `effect: Name`. All 17 built-ins are covered:
  the `IO` / `Time` / `Async` aliases, the fine-grained labels (`Console`,
  `FileSystem`, `Stdin`, `Clock`, `Panic`, `Div`, `Debug`, `NonDet`,
  `Random`, `Exn`), and the async seams (`Suspend`, `Fork`, `GetContext`,
  `AsyncFail`). User-declared effects keep the plain `effect: Name` label.
