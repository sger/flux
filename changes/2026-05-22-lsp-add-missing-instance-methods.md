### Added
- LSP "Add missing methods" quick fix on an `instance` declaration. With the
  cursor on the instance head, the server stubs every *required* class method
  (one declared without a default body) the instance does not implement as
  `fn name(params) { panic("todo") }` inside the block — the missing set is the
  same one the compiler's `E442` (MISSING INSTANCE METHOD) flags, and the
  `panic` body is polymorphic and `Panic`-exempt so each stub type-checks.
  Methods with a default and already-implemented ones are left alone. This is
  the instance-side counterpart to fill-match-arms and the Flux analogue of the
  Haskell language server's "Add placeholders for <class>".
