### Added

- `Flow.Fs` with `read_file` and `read_file_or`, backed by the new
  `TryReadFile` primop (proposal 0178). `Flow.Fs.read_file` returns
  `Result<String, IoError>` instead of aborting, so a missing or unreadable
  file is an ordinary outcome the caller can recover from:

  ```flux
  match Fs.read_file(path) {
      Ok(content) -> use(content),
      Err(e) -> match Io.is_not_found(e) { true -> default(), false -> report(e) },
  }
  ```

  Both operations carry the `FileSystem` effect, so a function that reads a
  file says so in its signature.

  This is the first primop that reports failure as a value rather than
  panicking, on both backends: the VM classifies `std::io::ErrorKind` and the
  C runtime classifies `errno`, independently, and the two agree. The prelude's
  aborting `read_file` is unchanged — existing code depends on it.
