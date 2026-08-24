### Added
- Module search roots can be scoped to a package namespace (proposal 0177
  Phase 1). A scoped root only satisfies imports whose first segment is its
  namespace, so two path dependencies may each ship a `Json` module without
  colliding. `--root` and script mode keep using unscoped roots, which satisfy
  any import.
- `E469 NAMESPACE COLLISION`: two resolved packages claiming the same namespace
  are reported at resolution time, naming both packages, instead of surfacing
  as a bare `E027 Duplicate Module` listing only the two files.
