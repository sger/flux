### Added

- Git dependencies. A `flux.toml` may now depend on a package in a git
  repository, pinned by `rev`, `tag`, or `branch`:

  ```toml
  [dependencies]
  flux-greeter = { git = "https://github.com/sger/flux-greeter", tag = "v0.1.0" }
  ```

  Omitting all three follows the repository's default branch. A dependency sets
  exactly one of `path`, `version`, and `git`, and at most one pin; violating
  either is a manifest error rather than a silent preference.

  Checkouts live in `$FLUX_HOME/git/checkouts/<slug>/<commit>/`, keyed by
  resolved commit — so they are immutable, shared between projects, and never
  refetched. A fetched package is walked like any other, so a git dependency's
  own path and git dependencies resolve transitively.

  Fetching drives the `git` binary rather than Flux's own HTTP client, which
  speaks no TLS and so cannot reach GitHub or GitLab
  ([KI-035](../docs/known_issues.md)). Prompting is disabled, so a private or
  misspelled repository fails with `Repository not found` instead of hanging a
  build waiting for a username that nobody will type.

- Live progress for git fetches, in the toolchain's right-aligned verb style:

  ```
      Updating https://github.com/sger/flux-greeter
       Fetched https://github.com/sger/flux-greeter (50543e1)
  ```

  `Updating` prints *before* the clone starts, so the seconds a download takes
  are no longer silence that reads as a hang, and each pair is interleaved so
  it is clear which dependency is in flight. A cached checkout prints nothing.

  This required streaming the manifest resolver's stdout rather than waiting
  for it to exit: the resolver is a Flux program run as a child process, so a
  captured pipe could only ever report a download after it had finished. The
  child now runs with `--quiet`, keeping its own compile progress — which
  describes the resolver, not the user's build — out of the output.

  A braille spinner animates between the two lines while the clone runs, since
  a download has no percentage to report and the useful signal is simply that
  the toolchain is waiting on the network rather than wedged. It draws only
  when stderr is a terminal and `NO_COLOR` is unset: repainting a line in place
  produces thousands of control characters in a redirected log rather than an
  animation. The line is erased before `Fetched` prints, so no trace of it
  survives in the finished output.

- `flux clean --deps`, which removes the downloaded git checkouts along with
  the build cache. Clearing both by hand meant knowing that two independent
  caches exist and that either one alone will mask a refetch — `target/` keeps
  the compiled modules, so a build with an emptied `$FLUX_HOME` still would not
  fetch anything.

### Changed

- `--quiet` is now a real flag, suppressing `[n of m] Compiling …` lines. It
  existed internally for the REPL but could not be passed on the command line.

- `flux --help` now lists the package commands (`new`, `init`, `build`, `run`,
  `test`, `check`). They have worked since Phase 1 but appeared nowhere in the
  help, so the only way to find them was to already know they existed.

- E470's hint now says "the dependencies it declares" rather than "the path
  dependencies it declares", which stopped being the whole story once a
  manifest could declare a git dependency.

### Docs

- [KI-035](../docs/known_issues.md): `Flow.Http` rejects every URL that is not
  `http://`, so no HTTPS host is reachable from Flux. Plain HTTP works, so this
  is a missing TLS layer rather than a broken client — but it blocks any
  HTTP-based registry client, and it is why git dependencies shell out to
  `git`.
