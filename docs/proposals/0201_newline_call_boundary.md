- Feature Name: `newline_call_boundary`
- Start Date: 2026-11-11
- Proposal PR:
- Flux Issue:
- Status: Draft
- Relates to: [0027](0027_language_syntax_specification.md) (newline-separated syntax), [0014](implemented/0014_lexer_parser_code_review.md) (parser)

# Proposal 0201: A call does not continue onto the next line

## Summary
[summary]: #summary

A `(` or `[` that starts a line begins a new expression. It is no longer a
call or an index on the expression that ends the line before it. With that
one rule, a sequence of statements never needs `;`, in any function, with or
without effects.

`;` stays legal. On a block's last expression it keeps its one meaning:
discard the value, so the block is `Unit`.

## Motivation
[motivation]: #motivation

Flux's parser does not see newlines. `parse_expression`
(`src/syntax/parser/expression.rs:147`) keeps extending an expression while
the next token can continue it, and `(` and `[` after a value continue it as a
call and an index (`parse_infix`, `expression.rs:227-228`). So this function:

```flux
fn both_ops() -> Int with E {
    let x: Int = perform E.a(1)
    let y: Int = perform E.b(1)
    ((x * 1000) + y)
}
```

parses as if it were written

```flux
fn both_ops() -> Int with E {
    let x: Int = perform E.a(1)
    let y: Int = perform E.b(1)((x * 1000) + y)
}
```

and is rejected with two errors that point away from the cause:

- `Argument Type Mismatch` (E300): the result of `perform E.b(1)` is called
  with `(x * 1000) + y`;
- `Return Type Mismatch` (E300): the block now ends with a `let`, so its type
  is `Unit`.

Nothing here involves effects. The same happens in a pure function:

```flux
fn area(w: Int, h: Int) -> Int {
    let a = w * h
    (a + 1)
}
```

Today the fix is to know the rule and put `;` after the line before. The
`;` after `perform E.a(1)` above is not needed, because `let` cannot continue
an expression. Only the one before a line that starts with `(` or `[` is. A
reader cannot tell the two apart, and a later edit that reorders lines can
turn a correct program into a different one. This came up while writing the
KI-185 parity fixture (`tests/parity/effect_multi_op_dispatch_ki185.flx`):
the draft without `;` hit exactly these errors.

`examples/guide/semicolons.flx` already says "Semicolons OPTIONAL" for
statements inside function bodies, and `docs/guide/01_getting_started.md:39`
says they are optional at the top level. Both are true except for this case.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

Statements are separated by line breaks. You never need `;` between them.

```flux
fn both_ops() -> Int with E {
    let x: Int = perform E.a(1)
    let y: Int = perform E.b(1)
    ((x * 1000) + y)              // the block's value
}
```

A line that starts with `(` or `[` is a new expression, so the last line above
is the value of the block.

To write a call or an index across lines, keep the `(` or `[` on the same
line as what it applies to, and break inside it:

```flux
let total = sum_all(
    first,
    second,
)
let item = table[
    index_of(key)
]
```

Lines that start with an operator still continue the expression before them,
as they do today. Pipelines and long conditions do not change:

```flux
let result = Array.range(1, 11)
    |> Array.filter(\x -> x % 2 == 0)
    |> Array.map(\x -> x * x)

let ok = package_name(candidate) == name
    && satisfies_all(candidate, constraints)
```

`;` is still accepted anywhere it is today. On the last expression of a block
it still means "evaluate this, then return `Unit`":

```flux
fn log_it(x: Int) with IO {
    println(x);          // the block's value is Unit, not println's result
}
```

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### The rule

In the expression loop (`parse_expression`, `expression.rs:147-164`), before
the parser takes `LParen` or `LBracket` as a postfix operator, it compares
lines. If the `(` or `[` (the peek token) starts on a later line than the
current token ends, the loop stops and the expression ends there.

`Token` already carries `position` and `end_position`
(`src/syntax/token.rs:12-18`), so the check is
`peek_token.position.line > current_token.end_position.line`. The lexer does
not change, and no newline token is added.

### What it applies to

- Only the **optional postfix** uses of `(` and `[`: a call
  (`parse_call_expression`) and an index (`parse_index_expression`).
- Not to a `(` that belongs to another construct: `perform E.op(...)`, a
  function declaration's parameter list, `handle E(init)`. Their own parsers
  read that `(`, not the expression loop, so a line break before it is fine.
- Not to prefix uses: a `(` that starts a grouped expression or a tuple
  (`parse_grouped_expression`) and a `[` that starts a list literal. Those are
  what the new line begins.
- Not to anything inside `( )`, `[ ]` or `{ }` delimiters. Arguments, list
  elements and record fields are already separated by `,`.
- Not to infix operators, `.`, `|>` or `handle`. A line that starts with one
  continues the expression, as today.

### Unchanged

- `;` stays an optional statement separator (`let`: `statement.rs:843`,
  assignment: `statement.rs:878`, expression statements: `statement.rs:298`).
- `has_semicolon` keeps its meaning in lowering
  (`src/core/lower_ast/mod.rs:1443-1454`) and in tail position
  (`src/ast/tail_position.rs:37`).
- Two statements on one line still parse without `;`
  (`examples/guide/semicolons.flx`, "Same line (also parses)").
- The type and effect checkers are untouched. A signature with a return type
  and no `with` still means pure. Leaving out the whole signature still infers
  the effects.

### Diagnostics

None is needed for the new meaning, since the program now means what it
looks like. A line that starts with `(` and whose value is unused (not the
block's last expression) could get a hint. See the unresolved questions.

## Drawbacks
[drawbacks]: #drawbacks

- In principle this is a breaking change: code that puts a call's `(` on the
  next line changes meaning. A scan of all 1,660 tracked `.flx` files outside
  `vendor/` (`lib/`, `tests/`, `examples/`, `benchmarks/`) on 2026-11-11
  found no such code. The only file that relies on the old behavior is the
  untracked draft of the KI-185 fixture, and only by accident.
- The rule is about lines, so how a program is laid out now matters in one
  place. Kotlin, Swift and Go users already expect this. OCaml users may not.
- A line that starts with `-` still continues: `a` followed by `-b` on the
  next line is still `a - b`. The scan found no code that starts a line with
  a binary `-` after a complete expression, so either choice is free today.
  This proposal leaves it as it is (see the unresolved questions).

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- **Require `;` after every statement, as Rust does.** This removes the
  ambiguity, but it changes almost every Flux file and goes against the style
  the codebase and the guide use. Rejected.
- **A full layout rule, as Haskell and Koka have.** It is more general, but
  it changes the grammar, the error recovery and the formatter. One token rule
  fixes the only ambiguity the corpus shows. Rejected for now. 0027 can still
  go there later.
- **Go-style semicolon insertion in the lexer.** The lexer would insert `;`
  at a line break after an identifier, a literal, `)`, `]` or `}`. That ends
  an expression before a line that starts with an operator, so the 162
  `|>`-led lines and the roughly 150 `+`/`&&`/`||`-led lines in the corpus
  would all break. Rejected.
- **A warning only.** It flags the trap but keeps the surprising meaning, so
  `;` stays something you have to know about. Rejected.
- **Do nothing.** `;` stays "optional, except when the next line starts with
  `(` or `[`", and the error points at the wrong place.

## Prior art
[prior-art]: #prior-art

- **Kotlin, Swift:** a call's `(` must be on the same line as the callee.
  On a new line it starts a new expression. This proposal takes the same
  rule.
- **Go:** the lexer inserts `;` at line ends after tokens that can end a
  statement. Same effect for `(`, but operators must end the line, not start
  it.
- **JavaScript:** automatic semicolon insertion does not insert one before a
  line that starts with `(` or `[`. `a = b` followed by `(c)` on the next line
  calls `b`. This is the same trap Flux has today. "No-semicolon" JS styles
  write `;(c)` to avoid it.
- **OCaml:** application is juxtaposition across lines, with the same trap.
- **Haskell, Koka:** layout. A line at the block's indentation starts a new
  statement.
- **Rust:** `;` is required between statements, so there is no ambiguity.
  Flux has Rust's "the last expression without `;` is the value" rule but
  makes `;` optional, which is where the gap comes from.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- **Leading `-` and `!`.** Should a line that starts with a prefix-capable
  operator start a new expression too? Swift decides by whitespace (`-b` is
  prefix, `- b` is infix). This is left out here because nothing in the corpus
  needs it.
- **A hint for an unused `(`-led line.** For code written against the old
  rule: when a line that starts with `(` is an expression statement whose
  value is discarded, suggest joining it to the line above. Is it worth the
  noise?
- **The tail `;` meaning.** Should a trailing `;` on a block's last expression
  keep changing the block to `Unit`, or should `;` become purely a separator?
  This is outside this proposal, but it is the other way `;` can matter.

## Future possibilities
[future-possibilities]: #future-possibilities

- The rest of 0027's "no semicolons, newline-separated" syntax, if a layout
  rule is wanted later.
- A formatter rule that removes `;` that are no longer needed.

## Test plan
[test-plan]: #test-plan

For the implementation, not this document:

1. **Parser tests** in `tests/parser/`, written first and seen failing:
   - a `let` followed by a `(`-led line parses as two statements;
   - the same with a `[`-led line;
   - controls: `f(1)(2)` and `xs[0][1]` on one line are still a curried call
     and a nested index; a `(` on the callee's line with arguments on later
     lines is one call; `|>`-, `+`- and `.`-led lines still continue.
2. **Parity fixture** `tests/parity/newline_call_boundary.flx` (`// parity:
   vm, llvm`), with the effectful `both_ops` case, the pure case and the
   controls, written with `--bless`.
3. **Corpus:** `cargo xtask sweep` shows no change in output, and
   `cargo xtask verify <tag> --base <tag>` passes, since this changes how
   existing source parses.
4. **Docs:** `docs/guide/01_getting_started.md:39` and
   `examples/guide/semicolons.flx` say `;` is never needed between statements
   and show the rule for multi-line calls.
