# Linter

> Source: `src/syntax/linter.rs`

The Flux linter performs static analysis over the AST and emits `Diagnostic` warnings. It runs as a separate pass after parsing.

## Running the Linter

```bash
cargo run -- lint examples/basics/variables.flx
```

Returns exit code `0` if no warnings, `1` if any warnings are found.

## What the Linter Checks

### Complexity Thresholds

| Check | Threshold | Warning |
|-------|-----------|---------|
| Function length | > 50 lines | Function is too long |
| Parameter count | > 5 parameters | Too many parameters |
| Cyclomatic complexity | > 10 | Function is too complex |
| Nesting depth | > 4 levels | Nesting is too deep |

Cyclomatic complexity is computed by counting decision points (`if`, `match` arms, `&&`, `||`) within a function body.

### Binding and Variable Checks

- **Unused bindings** — `let x = ...` where `x` is never referenced. Suppressed for names starting with `_`.
- **Shadowing** — A binding that shadows an outer binding of the same name (warning, not error).
- **Unused parameters** — Function parameters that are never used in the body.

### Code Quality

- **Dead code** — Expressions whose results are discarded in statement position (e.g., `1 + 2` as a standalone statement).
- **Redundant expressions** — Identity operations or no-ops detectable statically.

## Linter Struct

```rust
pub struct Linter<'a> {
    scopes: Vec<HashMap<Symbol, BindingInfo>>,  // scope stack for binding tracking
    warnings: Vec<Diagnostic>,                  // collected warnings
    file: Option<String>,                       // current file path for spans
    interner: &'a Interner,                     // for resolving symbol names
}
```

The linter walks the AST using a scope stack, pushing a new scope on entering a block and popping on exit.

## Entry Point

```rust
pub fn lint(program: &Program, interner: &Interner, file: Option<String>) -> Vec<Diagnostic>
```

Returns a list of warning diagnostics. These are rendered via the same diagnostics pipeline as compiler errors.

## Adding a New Lint Rule

1. Identify the AST node type to check (expression, statement, or function declaration).
2. In the appropriate `visit_*` method of `Linter`, add the check and push to `self.warnings`:

```rust
fn visit_function(&mut self, func: &FunctionDecl) {
    let param_count = func.params.len();
    if param_count > MAX_FUNCTION_PARAMS {
        self.warnings.push(
            diag_enhanced(&compiler_errors::SOME_WARNING)
                .with_span(func.span)
                .with_message(format!(
                    "function has {} parameters (max {})",
                    param_count, MAX_FUNCTION_PARAMS
                ))
        );
    }
    // ... continue walking
}
```

3. If it's a new warning category, add an error code constant in `compiler_errors.rs` and register it in `registry.rs`.
