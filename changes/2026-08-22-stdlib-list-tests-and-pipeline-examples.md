### Added

- `Flow.List` gains a test fixture (`tests/flux/stdlib_list.flx`, 70 tests)
  covering every exported function. Edge cases get their own assertions
  rather than being folded into the happy path: the empty list, the
  single-element list, predicates matching all or nothing, indices past the
  end, and the vacuous `any`/`all` cases. Ordering is asserted explicitly for
  the accumulator-based combinators, where a reversed result of the right
  length is the classic bug.

- Two worked examples of higher-order pipelines over the standard library,
  both verified on the VM and native backends by `parity-check`:

  `examples/guide/stdlib_pipelines.flx` is pure — `Flow.List` and
  `Flow.Array` combinators chained with `|>`, ADTs mapped and matched,
  `Option` and `Result` threaded through, records destructured, and the
  generation combinators (`range`, `iterate`, `unfold`).

  `examples/guide/stdlib_os_pipelines.flx` crosses the I/O boundary using the
  proposal 0178 capabilities. It is written as a pure core with an effectful
  shell, so the functions that decide things have no `with` clause while the
  ones that touch the machine declare `FileSystem` — and it shows the
  recoverable-error model end to end, with failures flowing through the same
  pipeline as successes instead of aborting.

- `tests/support/stdlib_fixture.rs`, a shared driver for the `Flow.*`
  fixtures. `assert_fixture_passes` runs one on the VM; `assert_backends_agree`
  runs it on both and requires the same result, which is how the `Flow.Fs`
  work caught a real native divergence that no VM-only test could have.
