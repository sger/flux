### Fixed

- **A `let` binding read inside a `match` arm was corrupt after the match on the
  VM backend** (KI-001).

  ```flux
  fn main() with IO {
      let n = 42
      match Some(1) {
          Some(_) -> println(to_string(n)),   // "42"       — correct
          None -> println("none"),
      }
      println(to_string(n))                   // "<uninit>"  — no error
  }
  ```

  `Int` printed `<uninit>` with no diagnostic at all; `String` aborted with
  `E1009: Cannot add Uninit and String values`. The silent case is what made this
  High severity — a program produced wrong output and nothing said so. The LLVM
  backend was always correct, so it was also a parity divergence.

  The VM reads a local either by copying (`OpGetLocal`) or by *moving*
  (`OpConsumeLocal`, which replaces the slot with the `Uninit` sentinel so
  `Rc::try_unwrap` can succeed downstream without a clone). The compiler picks
  the move when a binding is used exactly once.

  `compile_match` merged each arm body's use counts into the enclosing map with
  `or_insert`. For the arm's *own* pattern bindings that is correct — the arm
  body is their whole lifetime. For a binding declared outside it is not: the arm
  is one branch of the function, and `or_insert` installed the arm-local count of
  1 as the whole-function count whenever the enclosing map had no entry. The read
  after the match then found an emptied slot.

  A symbol's arm-body count is now merged only when the symbol belongs to the
  arm's own scope. Arm bindings still compile to `OpConsumeLocal` — the
  optimisation is narrowed, not disabled — which
  `tests/parity/match_arm_outer_binding.flx` asserts directly.

  **The trigger was narrower than the original report recorded**: it reproduced
  only in `main`, whose body is not compiled under an enclosing use-count map.
  The identical code inside a called helper was always correct. This matters for
  the test: a first version of the fixture put its cases in helper functions and
  passed against the *unfixed* compiler. Every case now sits directly in `main`,
  and both assertions were confirmed to fail without the fix before being
  committed.
