# Formatter

> Source: `src/syntax/formatter.rs`

The Flux formatter normalizes source code indentation. It operates on raw source text rather than the AST.

## Running the Formatter

```bash
# Format in place
cargo run -- fmt examples/basics/variables.flx

# Check only (exit 1 if formatting is needed, no changes written)
cargo run -- fmt --check examples/basics/variables.flx
```

Also available via `cargo fmt --all` for the Rust codebase itself (separate from the Flux formatter).

## How It Works

The formatter processes source line-by-line:

1. **Count leading close delimiters** — `}`, `]`, `)` at the start of a line decrease indent before the line is emitted.
2. **Emit the line** at the current indent level.
3. **Compute brace delta** — count net `{`/`[`/`(` vs `}`/`]`/`)` on the line (skipping string contents and comments) to determine indent change for the next line.

```rust
pub fn format_source(source: &str) -> String
```

Constants:
- `INDENT = "    "` — 4 spaces per level.

## Scope of the Formatter

The formatter currently handles **indentation only**. It does not:
- Reformat long lines
- Normalize spacing around operators
- Add or remove blank lines
- Sort imports
- Rewrite expressions

This is intentional — the formatter is deliberately minimal to avoid surprising rewrites.

## Extending the Formatter

For changes that go beyond indentation (e.g., operator spacing), the formatter would need to be rebuilt on top of the AST rather than raw text. The AST has span information (`Span { start, end }`) that maps back to source positions, which would enable a proper pretty-printer.
