### Added

- `Flume.Parse` — a parser-combinator library. A `Parser<a>` is a first-class
  function value, and everything larger is built from `and_then`, `or_else`,
  `many`, `separated_by`, and `choice`. It is generic: nothing in it knows about
  TOML.

  The combinator style is a response to the language rather than a flourish.
  Match arms take an expression, not a statement block, so the imperative shape
  — `let next = advance(current)` threaded through a cursor — cannot be written
  inside one at all. Combinators thread the position instead, so no grammar rule
  ever names it.

  Two combinators exist only because Flux is strict and does not backtrack by
  default. `lazy` defers building a parser until it is applied, without which a
  recursive grammar (`value` → `array` → `value`) builds an infinite parser graph
  before reading a byte. `attempt` restores backtracking past a shared prefix,
  which is what lets `name = "x"` be distinguished from `name.sub = "x"`.

- `Flume.Value` — the TOML value tree, in its own module because three others
  depend on it and none should depend on each other: `Flume.Toml` produces it,
  `Flume.Document` assembles it, and `Flume.Manifest` reads it. This is the
  stable serialization contract the proposal asks for.

- `Flume.Toml` — the grammar: comments, bare and quoted keys, `[table]` and
  `[[array-of-table]]` headers, dotted keys, basic strings with escapes,
  integers, booleans, arrays, and inline tables.

  It rejects rather than guesses. Multi-line strings, literal strings, floats,
  datetimes, and hex integers each get a *named* error, because a manifest that
  silently mis-parses is worse than one that fails. Errors carry a line and
  column so Phase 1 can point a diagnostic at the offending text.

- `Flume.Document` — the rules for assembling parsed items into a document:
  which tables may already exist implicitly (`[a.b]` then `[a]` is legal,
  `[a]` twice is not), which keys may never be overwritten, and where a dotted
  key lands relative to the current `[table]` or `[[array-of-table]]`. Separate
  from the grammar because the two fail differently: a parse error points at a
  character, a conflict points at an item and says what it collides with.

- `Flume.Manifest` — the schema layer: `[package]`, `[dependencies]`,
  `[dev-dependencies]`, `[lib]`, and `[[bin]]`, plus namespace derivation
  (`flux-json` → `FluxJson`, since every module-path segment must start
  uppercase). Registry dependencies parse even though Phase 1 rejects them, so
  that rejection can be a real diagnostic rather than silence. A dependency
  setting both `path` and `version` is an error: two sources of truth is how a
  build ends up using a working directory the manifest appears to pin to a
  release.

- `Flume.Resolve` — the full backtracking resolver, not a placeholder:
  highest-version-first, most-constrained-goal-first, with a conflict cache and
  the one-version-per-package activation key. The candidate set is a parameter
  rather than a fetch, which is the seam that keeps the resolver testable and
  makes resolution reproducible — it cannot depend on when it ran.

  Conflicts carry a **minimized** culprit set with provenance. A greedy pass
  drops each constraint that is not load-bearing, so an unsatisfiable diamond
  reports the two requirements on the contested package and its name — not the
  package the search happened to unwind to, and not the innocent bystanders.

- `Flume.Version` gains `parse_range`, `matches`, and `render_range`. Caret and
  tilde bounds follow the same "leftmost nonzero digit" rule `compat` already
  encodes. Pre-releases are excluded from a range unless the range itself names
  one, and naming `1.0.0-rc` opts into that version's pre-releases only.

- 184 behavioural tests across four fixtures, run on both backends, plus a
  purity test per module: each compiles a program whose `main` declares no
  effects, so a stray `print` or file read anywhere beneath would fail the build
  with E400. Phase 0's headline property is a real assertion, not a convention.

### Fixed

- **String ordering comparisons on the native backend compared heap addresses.**
  `flux_rt_lt` / `le` / `gt` / `ge` in `runtime/c/flux_rt.c` handled floats and
  integers and fell through to `flux_untag_int` for everything else, so a string
  operand was reinterpreted as a tagged integer. `"x" >= "a"` evaluated to
  `false` natively and `true` on the VM; `<` and `<=` happened to agree only by
  accident of allocation order.

  All four now take a lexicographic byte path for two strings, matching the VM,
  which compares Rust `String`s. Found by running the new `Flume` fixtures on
  both backends — the divergence is silent, so nothing short of a parity test
  would have caught it. `CACHE_EPOCH` is bumped accordingly.

### Changed

- The manifest reader was split along the three-stage funnel the proposal
  specifies, into `Flume.Parse` (generic combinators), `Flume.Value` (the value
  tree), `Flume.Toml` (grammar), `Flume.Document` (assembly rules), and
  `Flume.Manifest` (schema). The stages fail differently — syntax errors point
  at a character, schema errors name a key — and separate modules keep the
  distinction structural rather than conventional. No file exceeds 700 lines.

### Docs

- `docs/known_issues.md` KI-012: only the first instance of a type class
  dispatches; the rest type-check and then panic at runtime with "No instance
  ... for the given type". The same failure occurs for an instance whose head
  names an ADT imported from another module. A related note records that a class
  whose type variable appears only in the return position dispatches against the
  *argument* type and cannot resolve at all.

  This shaped two designs. `Flume.Resolve` renders with plain functions instead
  of a `Describe` class, and `Flume.Manifest` reifies what would naturally be a
  `FromToml` class as a first-class `Reader<a>` value. The latter composes
  further than the class would have — `array_of(element: Reader<a>) ->
  Reader<List<a>>` needs no higher-kinded types — so it is not purely a
  downgrade.

- `docs/known_issues.md` KI-013: `Flume.Toml`'s grammar crashes the native
  backend with SIGTRAP on any integer, array, inline table, or dotted key,
  while parsing correctly on the VM. It is not a stack limit, and no reduction
  attempted so far reproduces it, so the minimal case is currently the module
  itself. `flume_toml` and `flume_manifest` therefore run VM-only for now;
  `flume_resolve` and `flume_version` still assert backend parity, so
  divergence is not going unwatched.
