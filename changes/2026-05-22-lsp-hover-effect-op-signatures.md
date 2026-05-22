### Changed
- LSP hover on an effect operation now shows its declared signature and doc at
  `perform Effect.op(...)` and `handle Effect { op(...) -> … }` use sites, not
  only at the `effect` declaration — reusing the same op-type lookup the
  declaration site uses. The operation's `///` doc comment is also surfaced at
  both the declaration and the use sites.
- Fixed the synthesized op-name span for `perform`/`handle` op references in the
  LSP locator so it lands on the operation name itself (it previously pointed at
  the `perform` keyword / the handle-arm body). This makes hover, goto-definition
  and find-references on an effect operation resolve from the natural cursor
  position.
