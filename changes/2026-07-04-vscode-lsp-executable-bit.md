### Fixed
- VS Code extension: the bundled `flux-lsp` server now has its execute bit
  restored at activation (and when staged by `rebuild-vsix.sh`). VSIX zip
  packaging can strip Unix execute bits on install, which made the server fail
  to launch with `spawn ... EACCES` on Linux/macOS.
