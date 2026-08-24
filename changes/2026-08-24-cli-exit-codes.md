### Fixed
- Commands that reported an error exited `0`, so scripts and CI could not
  detect the failure (KI-019). The implicit `flux <file.flx>` run path,
  `flux eval`, and `fmt` now exit non-zero where they report the error;
  `cache-info`, `module-cache-info`, and `native-cache-info` return a status
  the CLI turns into an exit code. `module-cache-info` and `native-cache-info`
  additionally had no existence check at all, and reported an empty cache for a
  path that did not exist.
