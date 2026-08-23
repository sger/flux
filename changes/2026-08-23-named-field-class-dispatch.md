### Fixed

- **A class method applied to a named-field constructor reached the no-instance
  panic instead of its instance.**

  ```flux
  data Colour { Red { on: Bool }, Blue { on: Bool } }
  class Describe<a> { fn describe(value: a) -> String }
  instance Describe<Colour> { fn describe(value) { ... } }

  describe(Red { on: true })
  // error[E1009] panic: No instance of Describe.describe for the given type
  ```

  `rewrite_named_constructor` in `src/ast/desugar_named_fields.rs` rewrites
  `Red { on: true }` into an ordinary constructor call, and stamped the
  synthesized `Expression::Call` with `ExprId::UNSET`. That pass runs *after*
  inference, so `hm_expr_types` already held the constructed value's type keyed
  by the original `NamedConstructor`'s id — discarding the id stranded it.
  Compile-time dispatch (`try_resolve_class_call`) looks the first argument's
  type up by id, found nothing, and fell through to the stub whose body is
  `panic("No instance ...")`. The fix carries the original id onto the
  synthesized call.

  The symptom was reported as "only the first instance of a class dispatches",
  but neither instance resolved at compile time. Positional constructors were
  unaffected, which is what made one arm appear to work and sent the diagnosis
  toward instance *selection* rather than toward a missing type.

  `tests/integration/named_field_class_dispatch_tests.rs` covers both spellings,
  so a future change cannot fix one and regress the other.

### Docs

- `docs/known_issues.md`: KI-012 moves to Resolved. Two narrower issues that the
  original entry had folded together are split out and remain open, each with a
  minimal reproduction:

  - **KI-014** — a constructor applied in a module other than the one declaring
    its ADT infers as a type variable rather than the ADT type, so anything
    keyed on that type fails. Class dispatch is the visible casualty. The same
    class dispatches correctly when the value arrives from a function call,
    which locates the gap in constructor scheme import rather than in dispatch.

  - **KI-015** — dispatch selects an instance from the first argument's type, so
    a class whose type variable appears only in the return position resolves
    against the parameter type and cannot match. `Flow.Json`'s `Decode` works
    only because `try_resolve_class_call` special-cases it by name; a general
    return-type-directed rule would subsume that special case.
