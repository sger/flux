### Added

- `Flume.Version` — semantic versions and version ranges, the first module of the
  Flux package manager (proposal 0177, Phase 0). Parses `MAJOR.MINOR.PATCH` with
  optional pre-release tags, orders versions by semver precedence, renders back
  to text, and classifies compatibility ("same leftmost nonzero digit").

  Build metadata is parsed and discarded: semver ignores it for both precedence
  and equality, so retaining it would create values that compare equal without
  being identical. Leading zeroes are rejected so each version has one spelling.

  Every function has an **empty effect row**. `tests/flume/flume_version_tests.rs`
  enforces this by compiling a program whose `main` declares no effects — a stray
  `print` anywhere in the module would fail the build with E400.

- The `Flume` namespace at `lib/Flume/`, alongside the `Flow` standard library.
  Namespacing is a correctness requirement rather than a preference: module names
  are flat, so two roots each containing a bare `Version.flx` collide with E027.
  A namespaced `Flume.Version` coexists with a user's own `Version` module, and
  using the namespace dogfoods the mechanism Phase 1 gives every package.

- `tests/flux/flume_version.flx` — 33 behavioural tests run on both backends,
  including the semver specification's full pre-release precedence chain
  (`1.0.0-alpha < 1.0.0-alpha.1 < 1.0.0-alpha.beta < ... < 1.0.0`).

### Docs

- Proposal 0177 renames the `Pkg.*` modules to `Flume.*` and records why the
  namespace exists.
- `docs/known_issues.md` KI-011: re-wrapping `Err(e)` into a `Result` with a
  different success type fails inference (E430). This is the ordinary shape of a
  multi-step parser, so it is hit constantly; `Flume.Version.parse` uses
  `and_then_result` chaining instead.
