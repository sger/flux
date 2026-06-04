### Added
- LSP "Convert number format" — the Flux analogue of the Haskell LSP's
  alternate-number-format plugin. With the cursor on an integer literal, code
  actions rewrite it between decimal, hexadecimal (`0x…`), binary (`0b…`) and
  underscore-grouped decimal forms (e.g. `255` ↔ `0xFF` ↔ `0b11111111`, or
  `1000` → `1_000`). The form already written is skipped. Integer literals only.
