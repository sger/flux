### Changed
- Native (LLVM) build intermediates (`.ll` / `.bc` / `.o`) are now written under the
  project build tree (`<cwd>/target/native/`) instead of the system temp dir. A shared
  `native_build_base_dir()` helper now backs both the per-module object compilation
  (`compile_ir_to_object`) and the full binary pipeline (`compile_to_binary`), falling
  back to `%TEMP%` only when no project `target/` exists. This keeps every generated
  native artifact self-contained inside `target/`, so nothing flux produces is scattered
  into `%TEMP%` — where freshly-built unsigned executables trip Windows Defender's
  dropper heuristic.
