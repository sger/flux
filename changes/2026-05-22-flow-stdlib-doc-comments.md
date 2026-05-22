### Added
- Documented the core Flow standard-library modules with `///` doc comments:
  every public function (and `Flow.Math`'s `pi`) in `Flow.Array`, `Flow.List`,
  `Flow.String`, `Flow.Map`, and `Flow.Math` now carries a one-line summary.
  These surface on hover, in completion (`completionItem/resolve`), and in
  signature help — previously these modules used only `//` comments, so editors
  showed just the inferred type with no description. No behavior change; doc
  comments do not affect compilation or runtime.
