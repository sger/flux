### Fixed
- `flux test` discovered tests only in the entry file, so tests declared in a
  package's other modules were compiled but silently never run (KI-020).
  Discovery now matches the last dot-separated segment of a global's name, so
  `test_parses`, `Tests.test_parses`, and `Json.Parse.test_parses` are all
  found and reported by qualified name. `--test-filter` matches the qualified
  name, so it can select a whole module's tests.
