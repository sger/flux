### Changed
- LSP hover now works on ADT constructor *patterns* in `match` arms
  (`Circle(r)`, `Point { x }`), where it previously returned nothing. It renders
  the variant's declared shape — named fields as `Name { f: T, … }`, positional
  as `Name(T, …)`, nullary as `Name` — and shows the variant's doc comment,
  falling back to the enclosing `data`/`type` declaration's doc (resolved
  same-file first, then any cached module). Built-in patterns (`Some`, `Left`,
  `Cons`, tuples) keep their existing behavior; their bound sub-patterns still
  hover individually.
