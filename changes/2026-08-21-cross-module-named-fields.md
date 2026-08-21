### Fixed

- Named-field syntax now works for constructors declared in an *imported*
  module. `IoError { kind: .., message: .. }` as an expression and
  `IoError { kind, message }` as a pattern previously failed with a bogus
  arity mismatch — `Constructor \`IoError\` expects 3 argument(s) but got 0`
  (E082 for construction, E085 for patterns) — whenever the constructor came
  from another module.

  Named-field syntax is desugared to positional form by looking up the
  constructor's declared field order, and that lookup ran only over the current
  program's `data` declarations. An imported constructor has none there, so the
  lookup silently fell back to an empty field list and produced a zero-field
  constructor. Module interfaces now record `ctor_field_names` for their public
  record-style constructors, and importing modules preload it.

  Constructor field order participates in the interface fingerprint, since it
  determines the positional lowering importers emit: reordering fields
  correctly invalidates downstream caches.
