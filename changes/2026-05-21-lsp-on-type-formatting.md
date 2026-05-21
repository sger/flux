### Added
- `flux-lsp`: `textDocument/onTypeFormatting` — auto-indent as you type. Pressing
  Enter indents the new line to the enclosing brace depth, and typing `}` dedents
  its own line to line up with the opener. Indentation is computed lexically
  (counting `{`/`}` tokens before the cursor, so braces in strings and comments
  don't count), which stays correct while the buffer is mid-edit and
  syntactically incomplete. An already-correctly-indented line yields no edit, so
  the handler never fights an editor that has its own indent rules — it mainly
  brings this behaviour to LSP clients without VS Code's `language-configuration.json`.
