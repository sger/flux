### Added

- `Flow.Result` — the `Result<a, e>` type (`Ok` / `Err`) plus combinators:
  `is_ok`, `is_err`, `unwrap_result`, `unwrap_or_result`, `unwrap_err_result`,
  `map_result`, `map_err_result`, `and_then_result`, `or_else_result`,
  `result_to_option`, `option_to_result`.

  This is the error model for the fallible standard-library operations in
  proposal 0178. A single `import Flow.Result` brings the type and both
  constructors into scope unqualified.

  Two deliberate choices. `Result` is an ordinary data declaration rather than
  a compiler built-in, so `Ok` and `Err` stay ordinary constructor names that a
  local declaration may reuse — existing code declaring its own
  `type Result<T, E> = Ok(T) | Err(E)` keeps working, and neither Haskell nor
  Rust reserves its result constructors. And the module is imported explicitly
  rather than auto-injected: injection cannot be walked back without breaking
  every program that relied on a bare `Result`, while adding it later would
  break nothing.
