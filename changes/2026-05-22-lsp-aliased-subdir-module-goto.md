### Fixed
- LSP goto-definition on a member accessed through an `import … as` alias of a
  *subdirectory* user module (e.g. `import Lib.App.Util as U` then `U.helper`)
  now lands in the module file. The alias resolver returned the module's short
  segment, but sibling user modules are cached under their full declared name, so
  the lookup missed; it now prefers the full name when that is the cached key
  (matching how hover already resolved aliases).
