- Feature Name: Macro System for Flux
- Start Date: 2026-02-02
- Status: Not Implemented
- Proposal PR: 
- Flux Issue: 

# Proposal 0009: Macro System for Flux

## Summary
[summary]: #summary

This proposal introduces a **macro system** to Flux, enabling compile-time code generation and transformation. Following Elixir's philosophy of "put power in macros — keep the compiler small," this system would allow moving language features, base functions, and control flow from the compiler into user-space code.

## Motivation
[motivation]: #motivation

The proposal addresses correctness, maintainability, and diagnostics consistency for this feature area. It exists to make expected behavior explicit and testable across compiler, runtime, and documentation workflows.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

This proposal should be read as a user-facing and contributor-facing guide for the feature.

- The feature goals, usage model, and expected behavior are preserved from the legacy text.
- Examples and migration expectations follow existing Flux conventions.
- Diagnostics and policy boundaries remain aligned with current proposal contracts.

### These LOOK like syntax but are actually macros

unless condition do
  body
end

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **__preamble__:** **Superseded by:** `0040_macro_system.md` - **Current Problem:** Flux's compiler must understand every language feature: - Built-in functions (35 hard-coded...
- **Detailed specification (migrated legacy content):** **Superseded by:** `0040_macro_system.md`
- **Current Problem:** Flux's compiler must understand every language feature: - Built-in functions (35 hard-coded in VM) - Control flow (if/else hard-coded in compiler) - Future features require comp...
- **Elixir's Solution:** **Macro-first design:** ```elixir
- **Expands to::** if not(condition) do body end
- **Even 'if' itself is a macro!:** defmacro if(condition, do: do_clause, else: else_clause) do quote do case unquote(condition) do x when x in [false, nil] -> unquote(else_clause) _ -> unquote(do_clause) end end...

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

1. Restructuring legacy material into a strict template can reduce local narrative flow.
2. Consolidation may temporarily increase document length due to historical preservation.
3. Additional review effort is required to keep synthesized sections aligned with implementation changes.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Alternative 1: No Macros (Status Quo)

**Pros:** Simpler compiler, easier to understand
**Cons:** Every feature requires compiler change, 35 hard-coded base functions

### Alternative 2: Preprocessor (C-style)

```flux
#define unless(cond, body) if (!(cond)) { body }
```
**Pros:** Very simple to implement
**Cons:** Text-based, not AST-aware, no hygiene, poor error messages

### Alternative 3: Compiler Plugins (Rust-style)

```flux
#[plugin]
fn my_macro(ast: TokenStream) -> TokenStream { ... }
```
**Pros:** Maximum power, can use full Rust
**Cons:** Requires proc-macro infrastructure, compilation complexity

**Recommendation:** Full macro system (like Elixir) - best balance of power and simplicity

### Alternative 1: No Macros (Status Quo)

### Alternative 2: Preprocessor (C-style)

### Alternative 3: Compiler Plugins (Rust-style)

## Prior art
[prior-art]: #prior-art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### References

- [Writing an Interpreter in Go - Lost Chapter (Macros)](https://interpreterbook.com/lost/)
- [Elixir Macro Guide](https://elixir-lang.org/getting-started/meta/macros.html)
- [Lisp Macros](http://www.gigamonkeys.com/book/macros-standard-control-constructs.html)
- [Rust Macros](https://doc.rust-lang.org/book/ch19-06-macros.html)
- [Racket Macro System](https://docs.racket-lang.org/guide/macros.html)

### References

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions were explicitly listed in the legacy text.
- Follow-up questions should be tracked in Proposal PR and Flux Issue fields when created.

## Future possibilities
[future-possibilities]: #future-possibilities

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
