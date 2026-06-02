### Added
- The interactive REPL now has **tab completion** (GHCi-inspired). Pressing Tab
  completes, via a two-stage dispatch: `:`-commands at the start of a line; file
  paths for `:load`; an identifier for `:type`; and identifiers everywhere else.
  Identifier candidates are the names a user can actually type — the auto-exposed /
  imported library members (`map`, `length`, `filter`, …, sourced from the
  compiler's `exposed_bindings`, NOT the module-qualified symbol table), the builtin
  primops (`len`, `abs`, `map_get`, …), ADT constructors (`Some`, `Red`, …), the
  session's own `let` / `fn` bindings, `it`, and the language keywords. Fully
  qualified names (`Flow.Array.map`) are also offered so non-auto-exposed modules
  (Flow.Array, Flow.Map) are reachable — `.` is treated as a non-break char, so
  `Flow.Array.ma<Tab>` completes while a bare `ma<Tab>` completes unqualified names.
  The in-scope set refreshes after each entered line, so a just-defined binding
  completes on the next line. Multiple candidates are listed
  (`CompletionType::List`). The non-interactive (piped) path is unchanged.
