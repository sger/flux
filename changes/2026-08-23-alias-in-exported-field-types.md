### Fixed

- A `public data` field declared with a transparent type alias is now exported
  with the alias expanded, so an importing module sees the underlying type
  rather than the alias name (KI-016). `Flow.Http` types `Response.body` as
  `Bytes` (`public alias Bytes = String`), and reading it from another module
  failed with `error[E300]: String and Bytes` — a type compared against itself.
  This made `Flow.Http`'s response API unusable outside its own module and
  failed three `native_http_client_tests`.

  Aliases are expanded syntactically in the declaring program before inference,
  so exported schemes were already structural; the constructor field metadata
  was collected from the raw AST and still named the alias.

### Changed

- `ModuleInterface` gains a fingerprinted `public_type_aliases` field, so
  changing an alias body invalidates importers whose exported field types were
  expanded through it. `CACHE_EPOCH` is bumped 19 → 20.
