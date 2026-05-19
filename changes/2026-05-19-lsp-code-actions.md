### Added
- `flux-lsp`: `textDocument/codeAction` support — the server advertises a
  code-action provider and answers with diagnostic-anchored quick fixes:
  - **apply suggestion** — surfaces any structured `InlineSuggestion`
    (span + replacement) a diagnostic carries, e.g. a misspelled keyword the
    parser recovered from;
  - **did-you-mean** — for a diagnostic whose hint reads ``Did you mean `X`?``
    with a single-token `X`, offers replacing the flagged span with `X`;
  - **add catch-all arm** — for a non-exhaustive `match` (`E015`), inserts a
    `_ -> ()` arm before the closing brace, with the leading comma and
    indentation matched to the existing arms.
