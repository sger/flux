# Semicolon Rules in Flux

Verification-focused reference for Flux semicolon behavior.

## Verified behavior (current parser)

The parser currently treats semicolons as optional in the tested statement forms.

- Top-level call expressions: semicolon optional.
- Top-level `let` bindings: semicolon optional.
- The same forms with trailing semicolons: also valid.
- Statements after earlier parse errors are still parsed when recovery succeeds.

Both of these are valid:

```flux
print("hi")
print("hi");
```

```flux
let test = "this compiles"
let test2 = "this compiles";
```

## Why this works

Flux does not lex newline tokens. Statement boundaries are determined by parser progress:

- `parse_statement()` parses one statement from `current_token`.
- `parse_program()` then advances once and parses the next statement.
- Expression parsing stops on hard terminators such as `;`, `)`, `]`, `}`, `,`, `:`, `->`, and EOF.

Because of that, semicolons are accepted but not required for the verified cases above.

## Recovery interaction

Missing-comma recovery (E073) in expression lists works without requiring a trailing semicolon:

```flux
print(1 2 3)
let test = "this compiles"
```

The parser reports E073 and continues to parse the following `let` statements.

## Locked by tests

Behavior is covered by parser tests in `tests/parser_tests.rs`:

- `test_top_level_call_with_and_without_semicolon_both_parse`
- `test_top_level_let_with_and_without_semicolon_both_parse`
- `test_missing_commas_in_numeric_call_args_emit_e073_without_cascade`
- `test_semicolon_verification_program_recovers_and_parses_late_statements`

## Note

If Flux later introduces newline-sensitive parsing or stricter block rules, this document and the tests should be updated together.
