### Added

- `Flow.Fs` gains `exists`, `is_dir`, `is_file`, `write_file`,
  `create_dir_all`, `remove_file`, `remove_dir_all`, and `rename`, all
  carrying the `FileSystem` effect.

  The predicates return `Bool`; the mutations return `Result<Unit, IoError>`.
  `create_dir_all` succeeds when the directory already exists, like `mkdir -p`,
  and `rename` maps to the platform's rename so staging into a temp path and
  swapping it in stays atomic.

### Fixed

- Constructor tags now agree across separately compiled modules on the native
  backend. Each module was compiled with its own `Compiler` and numbered tags
  from whatever it happened to preload, so the same constructor could get a
  different tag in each object file — `Flow.Result`'s `Ok` was 5 in one module
  and 14 in another. Matching inline still worked, because payload extraction
  ignores the tag, but passing the value to a function in another module (such
  as `Result.is_ok`) took the wrong branch. Tags are now assigned once for the
  whole program in sorted path order.

- Recoverable-I/O runtime functions are declared with their real signatures in
  the LLVM backend. They take eight leading constructor tags before their
  arguments, but were declared as taking only the arguments, so the values
  landed in the wrong registers.
