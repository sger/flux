### Added
- `flux-lsp`: hover now covers `class`/`instance` method declarations and type
  references. Hovering a method inside a `class { … }` or `instance { … }` block
  shows its `///` doc comment and the class's declared signature (an instance
  method, which carries no annotations of its own, borrows the class signature).
  Hovering a type used in an annotation (`let x: MyType`) now surfaces the doc
  comment from `MyType`'s own `data` / `alias` / `class` declaration, not just a
  bare `type:` label. The locator also descends into class/instance method
  bodies, so hover (and goto-definition) work throughout those blocks instead of
  going dark inside them.
