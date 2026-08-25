### Fixed
- A user-defined effect performed in one module and handled in another crashed
  the native backend with SIGSEGV, while the VM ran it correctly (KI-034). A
  yield unwinds by returning the yield sentinel up the stack, but the three
  places that decide which call sites test `flux_is_yielding` all reasoned per
  module: an effect row was only treated as suspending when it named `Async`,
  a cross-module call was only treated as suspending when its symbol matched a
  hardcoded `Flow_Async_*` allowlist, and a module got yield checks only if it
  contained a `perform` of its own. Callers therefore ran on with the sentinel
  in hand and eventually dereferenced it as a pointer. "Can suspend" is now
  derived from the declared effect row and travels with the import, so every
  effect suspends except `Console`, `FileSystem`, `Stdin`, and `Clock`, which
  lower to plain C calls. `CACHE_EPOCH` bumped 23 → 24.
- Forwarding a payload destructured from an imported constructor
  (`TString(text) -> Ok(text)`) failed strict types with `E430` under
  `flux --test` (KI-022). Constructor field types from a dependency reached
  inference only through a cached `.flxi`; a module compiled fresh in the same
  run — and the test runner, which compiles the whole graph through one
  `Compiler` — never seeded them, so the pattern bound its payload to an
  unresolved variable. Both fresh-compile paths now seed from the dependency's
  AST.
