### Changed
- The interactive REPL is now behind a `repl` Cargo feature, enabled by default.
  It pulls in `rustyline`, which on Windows transitively depends on `clipboard-win`
  — whose Win32 clipboard / bitmap imports (`SetClipboardData`, `GetDIBits`, …) made
  the small native executables that link the flux staticlib look like
  clipboard-hijacking malware to Windows Defender. Native builds (`cargo native`)
  now compile with `--no-default-features`, so the REPL and `clipboard-win` stay out
  of the staticlib and generated binaries import only `kernel32` / `ntdll`. The
  full REPL is unchanged for normal use (`cargo run -- repl`); a build without the
  feature prints a clear message if `repl` is invoked.

### Fixed
- Native (LLVM) builds on Windows no longer fail to link with `undefined symbol`
  errors for clipboard / bitmap functions. As a safety net for builds that *do*
  include the `repl` feature (e.g. `cargo test --features llvm`), the native linker
  invocation also links `user32` and `gdi32`; with the REPL excluded these are
  unreferenced and add no imports to the output binary.
