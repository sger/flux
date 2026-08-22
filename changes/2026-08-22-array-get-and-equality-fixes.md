### Fixed

- `Array.get` returns `Some(element)` for an in-bounds index. It was declared
  `-> Option<a>` but returned the bare element, so every `Option` combinator
  read a present value as absent — `Option.unwrap_or(Array.get(a, 0), -1)`
  answered `-1` and `Option.is_some` answered `false` for an element that was
  plainly there. Pattern-matching the result fell through every arm on the VM
  and segfaulted on the native backend, dereferencing a tagged integer as an
  ADT pointer.

  The C runtime now separates the two uses: `flux_array_at` is the raw
  element accessor used inside the runtime (effect continuations, JSON
  stringification, TCP header handling), and `flux_array_get` is the
  Flux-facing `Option`-returning form. Conflating them is what made the first
  attempt at this fix break effect-handler dispatch.

  `flux_rt_index` no longer re-wraps: it used to add the `Some` that
  `flux_array_get` omitted, which is why `Array.first` and `Array.last` were
  correct while `Array.get` was not.

- Array equality works on both backends. `==` on two arrays raised
  `E1009 unsupported comparison` on the VM and answered `false` for identical
  arrays natively. Both now compare structurally, element by element, matching
  the existing tuple behaviour. Ordering is still undefined for arrays; only
  equality is offered.

  This was invisible to VM-only testing: `assert_eq` uses `cmp_eq`, which
  worked on the VM, so array-heavy fixtures passed there and failed wholesale
  natively.
