### Added

- Test fixtures for the standard-library modules that had none:
  `Flow.List` (70 tests), `Flow.Array` (46), `Flow.Map` (25),
  `Flow.Stream` (31), `Flow.Option` (17), `Flow.Either` (13), and
  `Flow.String` / `Flow.Numeric` / `Flow.Math` (40 together). Each exported
  function is covered, with the edges asserted separately from the happy
  path: empty collections, single elements, predicates matching all or
  nothing, indices past the end, and the vacuous `any`/`all` cases.

  The `Flow.Stream` fixture is the one that could not be written any other
  way: laziness is only observable over *infinite* streams, so several tests
  bound `repeat` and an unbounded `unfold` with `take` and `take_while`. An
  eager implementation would hang rather than fail, which makes those tests
  liveness checks as much as correctness ones.

- `tests/support/stdlib_fixture.rs`, a shared driver for these fixtures.
  `assert_fixture_passes` runs one on the VM; `assert_backends_agree` runs it
  on both backends and requires the same result. Native-parity targets are
  registered for `Option`, `Either`, and `Map`.

### Fixed

- `Flow.Numeric` no longer appears to export `from_integral` / `real_to_frac`
  in searches: they were only ever mentioned in a comment explaining why they
  are absent (Flux has no Int → Float primop). Noted here because the comment
  reads as an export to `grep`.

### Notes

Three pre-existing defects were found while writing these fixtures and are
documented in the fixtures themselves rather than worked around silently:

- `Array.get` returns a bare element instead of `Some(element)` on the
  in-bounds path, so every `Option` combinator reads a present element as
  absent. `Array.first` / `Array.last` are unaffected — they use `arr[i]`
  indexing rather than the `ArrayGet` primop, so the two access paths
  disagree.
- Array equality is broken on both backends: `==` raises
  `E1009 unsupported comparison` on the VM and answers `false` for identical
  arrays natively. `assert_eq` uses a different path that works only on the
  VM, which is why `stdlib_array.flx` has no native-parity target.
- `Flow.Stream.append` is unreachable: a fully qualified call resolves to the
  `Semigroup` class method and fails with `E444`. That function is therefore
  not covered.
