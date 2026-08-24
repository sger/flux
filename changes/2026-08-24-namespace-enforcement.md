### Added
- `E471 MODULE ESCAPES PACKAGE NAMESPACE`: a package may only declare modules
  under the namespace it owns, so package `json` declaring `module Utils` now
  errors at its own build with the corrected path (`src/Json/Utils.flx`) rather
  than surfacing later as a confusing missing- or duplicate-module error at a
  consumer's build. The namespace root module itself — `src/Json.flx` declaring
  `module Json` — remains the package's public face and is accepted. Unscoped
  roots (`--root` and script mode) are deliberately exempt.
