### Fixed
- LSP integration tests: build temp-workspace URIs through the server's own
  `canonicalize_flux_path` so cross-file `Location`/rename/document-link/workspace-
  diagnostic comparisons match on macOS, where the temp dir lives under `/var` (a
  symlink to `/private/var`) that the server resolves. Identity on Linux/Windows.
