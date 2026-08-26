### Added

- Phase 3 package-manager support: content-addressed VM module artifacts under
  `$FLUX_HOME/store`, workspace member resolution, versioned `metadata` and
  `build --plan` JSON, and clean-checkout `publish --dry-run` verification.
- `flux clean --store` and `flux build --explain-rebuild`.

### Notes

- Publishing performs archive creation, hashing, and verification locally.
  Upload remains unavailable while HTTPS registry transport is KI-035.
