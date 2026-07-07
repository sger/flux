### Added
- **Async VM↔native parity fixtures** (proposal 0177 T3.2). Blind spots in the
  gated `tests/parity/` set — swept on both backends by
  [scripts/release/release_check.sh](../../scripts/release/release_check.sh) — are
  now covered:
  - `tests/parity/async_task_spawn_await.flx` — `Task` spawn/join **lifecycle**
    via the async-surface `Task.await` (primop `TaskAwait`), the fiber-suspending
    join distinct from the `blocking_join` path already covered by
    `task_spawn_move.flx`.
  - `tests/parity/async_scope_cancel_stops_fork.flx` — structured-concurrency
    **scope cancellation**: `cancel(s)` tearing down a fiber forked under scope
    `s` (`scope`/`fork`/`cancel`), moved from the *ungated* `examples/async`
    sweep into the gated dir.
  - `tests/parity/http_get_roundtrip.flx` — **HTTP** client/server round-trip
    (`serve`/`get`/`shutdown` over `both`), now that the native HTTP crash is
    fixed (below). Async parity coverage is back to 100% of surfaces.

### Fixed
- **Native heap-use-after-free on deep async I/O inside a `both`/`fork` child
  fiber** (was KI-3; all `examples/http/*` and any `Http.get` inside `both`
  SIGSEGV'd on native, VM unaffected). Root-caused with an AddressSanitizer build
  (no debugger available) to a composed-continuation **double-drop**: the
  `flux_compose` trampoline ([runtime/c/effects.c](../../runtime/c/effects.c))
  carried a re-yielding continuation's remaining conts into the next composed
  continuation as **borrows** (`flux_array_get` / `flux_array_new` do not dup),
  so the same cont closures were owned by two continuation arrays at one
  refcount; releasing the executed continuation freed them under the next one.
  Only multi-cont (deep) continuations built the shared array — the single-cont
  fast path was safe, which is why *depth* was the trigger. The trampoline now
  `flux_dup`s each carried cont before `flux_yield_extend`. One line in the C
  runtime; no scheduler change. Verified UAF-free under ASan with no new leaks.
- **C runtime Makefile omitted `event.c` and `json.c`** from `SRCS`, so a clean
  `make` in [runtime/c/Makefile](../../runtime/c/Makefile) produced a
  `libflux_rt.a` missing `flux_event_*` / `flux_json_*` — a latent bug hidden
  only by a stale complete archive on disk. A `make clean` then broke native
  linking of every channel/event/select program (`channel.c` unconditionally
  references `event.c` symbols). Added both sources to `SRCS` so the archive is
  reproducibly complete.

### Docs
- **Confirmed the async parity gate fails on divergence** (proposal 0177 T3.1).
  `flux parity-check` exits non-zero on any VM↔native mismatch and
  `release_check.sh` runs it under `set -e`, so `tests/parity` divergence is
  already a hard release gate; T3.1 needed no code change.
- **KI-3 moved to Resolved** in
  [docs/internals/known_issues.md](../../docs/internals/known_issues.md) with the
  ASan root cause, the fix, and the depth/worker-count masking that hid it from
  the existing native HTTP tests (all pinned `with_worker_count(1)`).

### Tests
- `tests/native_llvm/native_http_client_tests.rs::native_http_client_get_under_both_multiworker_no_uaf`
  — HTTP `get` inside `both` under `with_worker_count(4)`; SIGSEGV'd pre-fix,
  passes post-fix. Deliberately uses 4 workers because the rest of the native
  HTTP suite pins a single worker, which masked the bug.
