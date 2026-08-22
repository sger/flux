### Docs

- Added `docs/known_issues.md`, tracking confirmed bugs, accepted limitations,
  and deliberately deferred design questions in one place. Entries carry a
  stable `#KI-nnn` anchor so code comments and commits can cite them, a
  reproduction, and a verification date.

  It exists because a proposal's "Unresolved questions" stop being read once the
  proposal moves to `docs/proposals/implemented/` — 27 implemented proposals
  currently carry 114 such questions between them. Questions that outlive their
  proposal now move here instead.

  Seeded with ten entries: three confirmed VM bugs (a `let` binding read inside
  a statement-position `match` arm becoming `Uninit` afterwards, `println` on
  collections, the bare `contains` builtin), proposal 0178's four surviving
  questions, the existing stdlib-discovery and TCP-blocking limitations, and the test-suite
  flakiness caused by 34 test files sharing one on-disk compilation cache.

### Changed

- `CLAUDE.md` documents the convention: known issues go in `docs/known_issues.md`
  rather than a proposal's unresolved-questions section.
