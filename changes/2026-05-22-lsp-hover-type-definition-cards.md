### Changed
- LSP hover on a `data`/`type`/`class` declaration name, and on a user type used
  in an annotation, now shows the type's definition instead of a bare
  `data: X` / `type: X` label: an ADT's variant list (`data Shape { Circle(Float),
  Rect(Float, Float) }`), a type alias's body (`alias Name = String`), or a
  `class` header. Built-in types (`Int`, `String`) and types declared in other
  modules keep the short label.
