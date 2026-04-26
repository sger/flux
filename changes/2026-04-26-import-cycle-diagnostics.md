### Fixed
- Cleaned up import-cycle diagnostics to show relative paths and point at the import that enters the cycle.
- Stopped module-resolution failures from cascading into later type and backend diagnostics.
- Reworded unresolved concrete type diagnostics to lead with the source-level issue.
