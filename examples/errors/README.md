# Error Examples

This directory contains example files that demonstrate different types of errors in Flux.
Each file is designed to trigger a specific error to verify the error formatting and behavior.

## Compiler Errors

### Syntax Errors
- **syntax_error.flx** - Missing closing parenthesis `[E105]`
- **unterminated_string.flx** - String literal missing closing quote `[E031]`
- **multiple_errors.flx** - Multiple syntax errors in one file

### Variable Errors
- **undefined_variable.flx** - Using undefined variable `[E004]`
- **immutable_reassignment.flx** - Reassigning immutable variable `[E002]`

## Runtime Errors

- **division_by_zero.flx** - Division by zero `[E1008]`
- **wrong_arguments.flx** - Wrong number of function arguments `[E1000]`

## How to Test

Run each example to see the error output:

```bash
# Compiler errors (fail during parsing/compilation)
cargo run -- examples/errors/syntax_error.flx
cargo run -- examples/errors/undefined_variable.flx
cargo run -- examples/errors/unterminated_string.flx
cargo run -- examples/errors/immutable_reassignment.flx
cargo run -- examples/errors/multiple_errors.flx

# Runtime errors (fail during execution)
cargo run -- examples/errors/division_by_zero.flx
cargo run -- examples/errors/wrong_arguments.flx
```

## Expected Format

All errors should follow this format:

```
-- <Error Type>: <title> [EXXX]

<Message>

  --> <file>:<line>:<column>
  |

<line number> | <source code>
              | <caret>

Hint:
  <hint text>

Stack trace: (runtime errors only)
  at <location>
```

## Error Categories

- **E001-E060**: Compiler errors
- **E1000-E1021**: Runtime errors
- **EXXX**: Unmigrated errors (temporary)
