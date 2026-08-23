### Fixed

- A constructor imported from another module now infers as its ADT rather than
  an unresolved type variable, so class dispatch on an imported constructor
  application resolves instead of panicking at runtime (KI-014). Constructor
  applications are routed by a lookup in `adt_constructor_types`, which was
  populated only from local `data` declarations; imported constructors missed it
  and fell through to the ordinary function-call path. Module interfaces now
  carry `public_ctor_types` — constructor field types and owning ADT — which
  seeds inference on import.
- `flux` now exits non-zero when CLI flag validation rejects the invocation.
  `render_parse_error` printed the message to stderr and returned
  `ExitCode::SUCCESS`, so `--native` without the `llvm` feature — and every
  other rejected flag combination — reported success while producing no output.
  Callers that check the exit status, including the test harness, saw an
  empty-but-successful run.
- Native-parity fixture assertions are skipped when built without the `llvm`
  feature instead of failing. `--native` is rejected at flag validation there,
  so the comparison had no native summary to make.

### Changed

- `ModuleInterface` gains a fingerprinted `public_ctor_types` field, so changing
  a public constructor's field types invalidates importing modules' caches.
  `CACHE_EPOCH` is bumped 18 → 19; existing caches are rebuilt on first run.
