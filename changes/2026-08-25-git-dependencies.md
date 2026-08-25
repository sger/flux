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

- Git dependencies are recorded in `flux.lock`, so a build is reproducible and
  can run offline:

  ```toml
  [[package]]
  name = "flux-greeter"
  source = "git+https://github.com/sger/flux-greeter#50543e14b878..."
  ```

  A `tag` or `branch` dependency resolves its pin once and then builds the
  recorded commit, rather than asking the remote what the ref points at on
  every build. A branch that has since advanced, or a tag that was repointed,
  therefore does not silently change what a locked project compiles — and a
  cached checkout needs no network at all. A `rev` already names a commit and
  ignores the lock. The lockfile is rewritten only when the resolution actually
  differs, so an unchanged build leaves a clean working tree.

  A locked commit that no longer exists is an error rather than a silent
  re-resolution: a rewritten history or a corrupted lockfile should not quietly
  turn a reproducible build into a different one.

- `flux add` and `flux remove`, which record and drop dependencies:

  ```sh
  flux add flux-greeter --git https://github.com/sger/flux-greeter --tag v0.1.0
  flux add shared --path ../shared
  flux add tester --path ../tester --dev
  flux remove flux-greeter
  ```

  The manifest is edited **textually**, not reparsed and re-rendered, so
  comments, blank lines, key order, and hand-aligned spacing all survive: the
  command that adds one dependency must not also produce a diff nobody asked
  for. `Flume.Toml` skips comments as whitespace and `Flume.Document` builds
  semantic values, so a round trip through them would return a correct document
  that is not the user's document. `Flume.Edit` finds the line to change,
  changes it, and leaves every other byte alone.

  Adding a dependency that is already present replaces it in place, keeping its
  position among its neighbours, so `add` twice is the same as `add` once.
  Removing one that is not there is an error rather than a silent success,
  which would otherwise hide a typo until the build failed for an
  unrelated-looking reason.

- `--offline`, `--locked`, and `--frozen`.

  `--offline` forbids reaching the network: a dependency whose commit is
  already known, from `flux.lock` or a `rev`, and whose checkout is cached
  builds normally, and anything else is an error naming what would have had to
  be downloaded. `--locked` makes any change to `flux.lock` an error, which is
  what a CI job wants — a dependency added without committing the lock fails
  rather than resolving silently. `--frozen` is both.

  A refusal names the dependency and what is wrong with it, since "the lockfile
  is out of date" alone does not say what to fix:

  ```text
  `--locked` was given, but `greeter` is not in `flux.lock`;
      run without `--locked` to record it
  `--locked` was given, but `base` is in `flux.lock` and is no longer a dependency
  `--offline` was given, but /tmp/moving at 39e2140… is not in the local cache
  ```

  The resolved-roots cache is bypassed when either flag is set: both are
  checks, and a cached result would skip them — a lockfile that stopped
  matching its manifest has to fail under `--locked` even when the previous
  run's roots are still on disk.

- `flux tree`, which prints the resolved dependency graph:

  ```text
  mix v0.1.0
  ├── mixlocal (path: ../mixlocal)
  │   └── base (git: /tmp/gitdep-base#db95edb)
  ├── greeter (git: /tmp/gitdep-probe#7173dba)
  │   └── base (git: /tmp/gitdep-base#db95edb) (*)
  └── flux-greeter (git: https://github.com/sger/flux-greeter#50543e1)
  ```

  Each dependency is annotated with where it comes from, since "which revision
  of this am I building?" is the question a tree is usually opened to answer,
  and for a git dependency the answer is a commit rather than a version. A
  locked commit is shown in preference to the manifest's ref, because it is
  what the build will actually use.

  A package already shown is marked `(*)` rather than expanded again, which
  also makes a dependency cycle terminate. Nothing is fetched: a git dependency
  whose commit is not yet known from the lockfile or a `rev` is listed by its
  ref and not descended into, so `tree` never reaches the network.

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

### Tests

- `flume_edit_tests` covers `Flume.Edit`: that an edit keeps every comment,
  hand-alignment, and untouched entry; that adding a dependency adds exactly
  one line; and that adding then removing restores the original byte for byte.
  A Rust-side assertion checks the stronger property the fixture cannot state —
  that every line of the original still appears, unchanged and in order, in the
  result.

- `flume_lock` and `flume_manifest` gained coverage for the git forms:
  `git+<url>#<commit>` parsing and rendering, a git source without a commit
  being rejected, a git package needing no checksum, the four pin spellings,
  and the two conflict errors.
