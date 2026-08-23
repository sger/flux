### Fixed

- The test suite no longer fails intermittently under load (KI-010). Test
  binaries drove the `flux` CLI against a shared scratch directory and the one
  compilation cache under `target/`, so concurrent targets read and wrote each
  other's `.flxi` interfaces and bytecode. The failures moved between runs and
  looked unrelated — a `missing global mapping for local index N` escaping the
  module linker, or a native fixture emitting nothing so no summary could be
  parsed.

  Every test that spawns the binary now routes through the `Scratch` guard,
  which gives each run its own directory and its own cache root. `--no-cache`
  was not enough on its own: the native backend writes shared artifacts under
  the cache root regardless of that flag, which is why the native targets lost
  these races most often despite passing it.
