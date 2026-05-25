### Changed
- The interactive `flux repl` now runs on a **persistent compiler + live VM**
  engine (proposal 0176, Phase 2). Instead of re-running the whole session on
  every line (Phase 1), it keeps one prelude-loaded compiler and one VM for the
  session and compiles each entered line as a *delta* that executes on the live
  VM via the new `VM::run_chunk`. Earlier declarations no longer recompile and
  their side effects no longer re-fire — an effectful line's output appears
  exactly once, and the per-line cost no longer grows with session length.

### Added
- The REPL now supports **rebinding** a name: entering `let x = 99` after
  `let x = 1` shadows the earlier binding on a fresh slot instead of erroring as
  a duplicate. (Self-referential rebinds such as `let x = x + 1` are a known v1
  limitation — they report `x` as undefined rather than reading the old value.)

### Internal
- The REPL moved from `src/driver/pipeline/repl.rs` to a dedicated top-level
  `src/repl/` module (`mod.rs` dispatch loop + `engine.rs` persistent engine).
- `Compiler` now derives `Clone` (used for cheap per-line checkpoint/rollback so
  a failed line never corrupts the session), gains a REPL mode that accumulates
  each line's top-level binding schemes so later lines can resolve the *types* of
  earlier session globals, and a `forget_session_binding` hook for rebinding.
- `VM::run_top_level` runs the compiler's full top-level instruction buffer from a
  given offset, so the latest line's tail executes (with correct absolute jump
  targets for top-level `if` / `match`) while the prelude and earlier lines are
  skipped and `globals` persist.

### Known limitations
- A user-defined `data` type with **named fields**, a user `effect`, or a user
  `class` / `instance` declared on one line can't yet be *used* on a later line
  (each works within a single line / in a file). Enum and positional ADTs,
  `let` / `fn`, recursion, `if` / `match`, imports, and `it` all work across
  lines. Threading the remaining declaration metadata across lines is follow-up
  work.
