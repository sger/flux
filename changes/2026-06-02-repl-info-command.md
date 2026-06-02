### Added
- The interactive REPL has a new **`:info <name>`** command (`:i` for short), a
  richer, name-keyed sibling of `:type` modelled on GHCi's `:info`. It classifies
  the name and reports accordingly:
  - a **type** (user `data`/`type`, prelude ADT like `Result`, or a built-in such
    as `Option`/`Either`) → its constructors, each with its signature;
  - an **effect** (a user `effect` or a seeded built-in like `Console`) → its
    operations, each with its signature;
  - a **constructor** (`Some`, a user `Green`, …) → its own signature plus the ADT
    it belongs to and that ADT's block;
  - any other **value / function** → its type plus where it's defined (the current
    session, a library module like `Flow.List`, or the builtins).

  Constructor signatures are obtained by inferring a saturated form (Flux
  constructors aren't first-class unapplied), so positional *and* named ADT fields
  show their types. Abstract built-ins (`List`, `Map`, `Int`, …) render as
  `-- built-in type`. Output goes to stdout, consistent with `:type`.
