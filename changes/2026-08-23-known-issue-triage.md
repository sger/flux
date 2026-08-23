### Fixed

- A class-constrained function imported from another module no longer accepts
  the wrong container type (`KI-003`). `List.contains` is declared
  `(List<a>, a) -> Bool`, but passing an `Array` compiled silently and returned
  `false`; `not_elem` reported a present element as absent and `nub` returned
  `[]`. Unification failed correctly — the argument-mismatch diagnostic was
  discarded because it required both types to be free of type variables, and an
  `Eq`-constrained element type always leaves one. The diagnostic now also fires
  when the two outermost type constructors conflict, which stays decidable while
  element variables remain unsolved. This affected every `Eq`-constrained stdlib
  function, not just `contains`.

### Docs

- Retired the `println` known issue (`KI-002`) as not reproducible. `println`
  renders lists and arrays correctly on both backends, including arrays returned
  from a primop; the original report is explained by a terminal filter that
  dropped `[...]` output lines along with the compiler's `[ n of m ]` progress
  lines. Locked in with a regression test that asserts against raw stdout.
