### Performance

- **The native test runner builds one binary per file instead of one per test.**

  `run_tests_native` compiled and linked the whole module graph once per test
  *function*: a 58-test fixture paid 58 native builds at ~2.6s each, near-uniform
  regardless of what the test computed. It now generates a single harness whose
  `main` dispatches to one test, selected by an environment variable, and runs
  that same binary once per test. The first child compiles it; the rest hit the
  compile cache.

  | Fixture | Before | After |
  |---|---|---|
  | `flume_version` (58 tests) | 154s | 59s |
  | `stdlib_list` (70 tests) | >60s | 48s |

  Both had been crossing cargo's 60-second "still running" notice.

  **Isolation is unchanged.** Each test still runs in its own process; only the
  *binary* is shared. A test that panics, aborts, or corrupts memory takes down
  only its own run — which is the property the per-test harness was really
  buying, and it cost a full rebuild per test to get something a shared binary
  already provides.

  Two details are load-bearing and easy to get wrong:

  - The dispatch `match` is written **inline in `main`**, not extracted into a
    helper. A program entry point may leave its effect row to be inferred, but
    that exemption does not extend to a function `main` calls: a helper calling
    a `with IO` test fails with E400.

  - `main` declares `with Env` explicitly rather than relying on inference. When
    every test in a file is `with IO`, the ambient row happens to cover the
    environment read too — but a fixture whose tests are all pure
    (`tests/flux/stdlib_either.flx`) leaves nothing to infer from, and the
    harness fails to compile. Declaring the row makes it independent of what the
    fixture's tests happen to do.

  Children share a cache directory **private to that file's run** rather than
  passing `--no-cache`. Sharing the cache is what makes the build cost O(1) in
  the number of tests; scoping it per file is what keeps concurrent test
  binaries from writing one root, which is KI-010 and which surfaced during this
  work as an intermittent `unhandled effect` from a half-written artifact.
