### Added

- `Flow.Fs` gains `list_dir` and `metadata`, completing the filesystem surface
  in proposal 0178.

  `list_dir` returns `Result<Array<String>, IoError>`. Entries are bare file
  names excluding `.` and `..`, so they compose with `Flow.Path.join` — the
  only place that knows the separator. A failure part-way through the walk
  fails the whole call, so a caller never mistakes a truncated listing for a
  complete directory.

  `metadata` returns `Result<FileMeta, IoError>`, where `FileMeta` carries
  `size`, `modified`, `is_dir`, and `is_file`, read with `file_size`,
  `modified_time`, `meta_is_dir`, and `meta_is_file`. Prefer it over separate
  `is_dir` / `is_file` calls when you want several facts at once: it is one
  syscall, and the answers are consistent with each other rather than sampled
  at different moments. `modified` is milliseconds since the Unix epoch and is
  `0` when the platform records none — comparing it across runs is fine,
  hashing it into a build cache is not.

### Fixed

- The native backend can now report every `IoErrorKind`. Recoverable-I/O
  runtime calls threaded only five of the eight kinds declared in
  `Flow.IoError`, so `AlreadyExists`, `NotADirectory`, and `DirectoryNotEmpty`
  were unreachable natively and collapsed into `Other`. Listing a file as a
  directory reported `NotADirectory` on the VM and `Other` on native; both now
  agree. The C runtime also maps `EEXIST`, `ENOTDIR`, `ENOTEMPTY`, and
  `ETIMEDOUT`, which it previously did not.
