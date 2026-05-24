### Added
- LSP "Prefix unused binding with `_`" quick-fix — the Flux analogue of HLS
  surfacing hlint fixes as code actions. With the cursor on a `let` binding the
  linter reports as unused (W001), a quick-fix inserts a leading `_`, silencing
  the warning without deleting the binding. Mirrors the existing
  remove-unused-import (W003) action: the linter runs on demand, gated on the
  cursor first overlapping a `let`, so requests elsewhere pay nothing.
