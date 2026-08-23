### Fixed

- Flux now finds its standard library when run from outside the source checkout
  (KI-008). The stdlib was resolved as the bare relative path `lib/Flow` against
  the process working directory, and the lookup returned silently when that was
  missing — so running anywhere else reported an empty module root list rather
  than a diagnosis, which blocked installing Flux as a tool.

  Discovery now tries `$FLUX_LIB_DIR`, then `lib/Flow` walking up from the entry
  file, then `lib/Flow` walking up from the executable — covering a project
  checkout, a Cargo workspace, a dev binary in `target/debug`, and an installed
  `<prefix>/bin/flux` with `<prefix>/lib/Flow`. Module search roots are resolved
  relative to the entry file as well as the working directory.

### Added

- `FLUX_LIB_DIR` overrides standard-library discovery, naming the directory that
  contains `Flow/`.
