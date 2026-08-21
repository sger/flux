### Fixed
- Module-level functions now correctly shadow same-named builtins for bare calls
  inside their own module. Previously a `module M { public fn trim(s) { ... } }`
  lost to the builtin `trim` for every unqualified call within `M`, so the
  builtin ran in place of the local definition with no diagnostic — while the
  qualified call `M.trim(...)` correctly reached the local, making the same
  function return different results depending on call syntax.

  Module members are stored in the symbol table under a qualified key (`M.name`),
  which the bare-name shadow checks did not consult. Two independent builtin
  channels were affected: `route_effectful_primops` rewrote bare calls such as
  `read_file(p)` into `perform FileSystem.read_file(p)` on name and arity alone
  (which also forced `with FileSystem` onto callers of a pure local), and
  identifier compilation fell through to `exposed_bindings` and loaded the
  prelude's binding. Both now resolve module members first.

  Shadowing applies to the whole module body, so a call placed above its
  definition shadows too. Builtins remain reachable wherever they are not
  shadowed, and keep their effect requirements.
