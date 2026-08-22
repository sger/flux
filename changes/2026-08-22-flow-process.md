### Added

- `Flow.Process` — subprocess execution (proposal 0178, item 6). `Proc.run(cmd, args)`
  runs a command to completion and returns `Result<ProcOutput, IoError>`, where
  `ProcOutput` carries `status`, `stdout`, and `stderr`. Accessors `status_of`,
  `stdout_of`, `stderr_of`, and `succeeded` read the record; `output_or` collapses
  the common "give me stdout, or a default if anything went wrong" case.

  There is no shell. The argument vector reaches the OS unchanged, so an argument
  containing spaces, quotes, or `;` stays a single argument — shell injection is not
  a failure mode because there is nothing to inject into.

  A command that runs and exits non-zero is `Ok`: it ran, and the status is the
  answer. `Err` is reserved for failing to start the process at all — no such
  binary, or no permission to execute it. A signal-terminated child reports status
  `-1`, which no normal exit produces.

- A `Process` effect label, coarsening to `IO` alongside `Console`, `FileSystem`,
  `Stdin`, and `Env`. Subprocess execution is strictly more authority than
  filesystem access — a child process can do anything the invoking user can — so
  folding it into `FileSystem` would understate what a signature permits.

### Known limitations

- Subprocess execution is POSIX-only on the **native** backend: the C runtime uses
  `posix_spawnp`, and the Windows branch returns an `IoError` with kind `Other`
  (`ENOSYS`) rather than spawning. The VM backend works on Windows, since it goes
  through Rust's `std::process::Command`. This is the one place the two backends
  deliberately differ, and it is a gap to close before Windows is a supported
  target for tooling written in Flux.

### Fixed

- `flux_array_len` returns a *tagged* integer; the native `flux_proc_run` initially
  used it as a raw count and spawned commands with phantom trailing arguments.
  Caught by byte-comparing VM and native output.

### Docs

- Proposal 0178 stage 5 is implemented, completing the proposal.
