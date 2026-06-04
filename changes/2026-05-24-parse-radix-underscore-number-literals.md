### Fixed
- Hex (`0xFF`), binary (`0b1010`) and underscore-separated (`1_000`, `1_000.5`)
  number literals now parse. The lexer already tokenized these forms (and
  documented them as supported), but the parser decoded every integer with a
  plain decimal `str::parse`, so any non-decimal or underscored literal raised a
  spurious `E032: Invalid Integer`. Integers are now decoded by radix with `_`
  separators stripped, and floats strip `_` before parsing.
