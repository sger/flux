### Fixed
- Native backend: a user-defined `async` function defined in one module and
  called from another now emits its `flux_is_yielding` check. Previously
  cross-module async-ness was decided by a hardcoded `Flow.*` allowlist
  (`is_direct_async_extern_symbol`), so a call into a user-defined async import
  was treated as non-suspending and dereferenced the yield sentinel → SIGSEGV
  when the callee suspended (the VM was unaffected). Async-ness is now
  data-driven from the callee's known effect row: `ImportedNativeSymbol` carries
  an `is_async` flag derived from the imported member's cached type scheme, and
  the suspend-capable symbols are collected into
  `LirProgram::async_extern_symbols`, which the LIR yield-check classifier
  (`call_kind_is_direct_async`, feeding `direct_async_func_ids`,
  `promote_tail_calls`, and `cont_split`) consults alongside the allowlist.
  Resolves known issue KI-1.
