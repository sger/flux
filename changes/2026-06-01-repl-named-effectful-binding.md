### Added
- The REPL now persists a **named** effectful binding (`let x = read_line()`,
  `let xs = do { ...; ys }`) instead of dropping it with a note (proposal 0176).
  Such a binding is rejected at top level (E413, top-level effect), so only its
  *initializer* now runs inside a synthesized `fn main() with IO` — the effect
  fires exactly once and yields the value, which is captured and re-bound as
  `let x = <literal>` purely, so `x` is usable on later lines with its value and
  type intact. Reuses the (now compound-capable) literal renderer, so the captured
  value can be a list / tuple / ADT, not just a primitive. A named binding whose
  result has no literal form (`let x = print(..)`, returning `Unit`) still can't
  persist — the effect runs and the REPL prints a note.
