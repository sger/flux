### Performance
- VM delimited-continuation capture/resume — the path behind effect-handler
  `perform`/`resume` *and* async fiber park/resume (`FiberSleep`, `both`, `race`,
  `first_of`) — is now allocation-free in steady state. Capture composes the
  continuation from a reused per-VM scratch buffer (`cont_pieces`) instead of
  allocating one `Rc<RefCell<Continuation>>` per unwound frame, and a per-VM
  free-list (`cont_pool`, bounded by `continuation::CONT_POOL_CAP`) recycles the
  `Continuation` shell between a resume and the next capture. One-shot resume now
  moves the captured frames/stack out of the `Rc` (via `Rc::try_unwrap`) instead
  of deep-cloning them; the deep clone is kept only for the rare multi-shot
  resume where the continuation value is still aliased.

### Internal
- Added `Continuation::compose_pieces` (consumes a `Vec<Continuation>` of
  innermost-first pieces, recycles spent shells into a pool, returns the composed
  `Continuation` by value with a zero-copy single-piece fast path); `Continuation`
  now derives `Default`. `Continuation::compose` (the `Value`-slice form, used by
  the `OpPerform` unit test) delegates to it. `VM` gains `cont_pieces` / `cont_pool`
  scratch fields and a private `capture_piece_pooled` helper. Added a Criterion
  bench `benches/continuation_capture.rs`.
