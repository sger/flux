- Feature Name: Parser Error Experience — Keyword Aliases, Structural Messages, and Symbol Suggestions
- Start Date: 2026-02-28
- Status: Partially Implemented
- Proposal PR: pending (feature/type-system merge PR)
- Flux Issue: pending (type-system merge-readiness tracker, March 1, 2026)

# Proposal 0059: Parser Error Experience — Keyword Aliases, Structural Messages, and Symbol Suggestions

## Summary
[summary]: #summary

Improve parser error UX for common mistakes through keyword alias hints, structural context messages, and clearer symbol-level corrections.

## Motivation
[motivation]: #motivation

Parser recovery existed, but many high-frequency mistakes had generic messages that slowed correction.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 3. Goals

1. Catch common foreign-keyword aliases and suggest Flux equivalents.
2. Improve structural expectations (`if`/`else` braces, function parameter list, etc.) with context-specific E034 messages.
3. Detect missing `=` in `let` early to reduce cascades.
4. Improve `match` separator and arrow correction messaging.
5. Preserve AST and runtime semantics.

### 4. Non-Goals

1. No lexer token-set changes.
2. No support for alternate block terminators like `end`.
3. No HM/type/effect semantic changes.
4. No error code family changes beyond current parser diagnostics classes.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Consolidated technical points

- Keyword alias diagnostics remain in parser path with actionable suggestions.
- Structural parser messages now name construct and expected token intent.
- Recovery remains bounded and deterministic to avoid duplicate cascades.

### Detailed specification (migrated legacy content)

Parser fixtures and snapshot tracks define the locked behavior.

### Historical notes

- Proposal normalized to canonical template during branch consolidation.

## Drawbacks
[drawbacks]: #drawbacks

- More targeted parser messages require stricter snapshot governance.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- Targeted contextual messages are preferred over broad generic parser errors.

## Prior art
[prior-art]: #prior-art

- Prior parser UX track plus language-transfer error patterns from mainstream languages.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions for the implemented scope.

## Future possibilities
[future-possibilities]: #future-possibilities

- Expand contextual parser messaging carefully, preserving deterministic recovery behavior.
