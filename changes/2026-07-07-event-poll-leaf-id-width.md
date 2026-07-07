### Fixed
- **Native runtime: `poll_event` no longer truncates leaf event ids.** In
  `runtime/c/event.c` the `poll_event` helper declared its `out_leaf` parameter
  as `int16_t *`, while leaf event ids are `int64_t` everywhere else (the
  `*out_leaf = id` writes, the `int64_t leaf` caller in `flux_event_poll`, and
  `fire_losing_nacks` / `subtree_contains`). Recent compilers reject the pointer
  mismatch outright (`-Wincompatible-pointer-types`), which broke the C-runtime
  build for the native backend; had it compiled, ids past 2^15 would have been
  truncated, misdirecting `fire_losing_nacks` at commit. The parameter is now
  `int64_t *`.
