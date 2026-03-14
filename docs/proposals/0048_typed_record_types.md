- Feature Name: Typed Record Types
- Start Date: 2026-02-26
- Status: Not Implemented
- Proposal PR: 
- Flux Issue: 

# Proposal 0048: Typed Record Types

## Summary
[summary]: #summary

Introduce typed, immutable record types — named product types with labeled fields — into Flux. Records provide compile-time type checking for field construction, access, and functional update, filling the gap left by the existing untyped `Hash` literal.

## Motivation
[motivation]: #motivation

Flux currently has one mechanism for key-value grouping: the untyped `Hash { pairs: Vec<(Expression, Expression)> }`. This is a runtime map where keys and values are arbitrary `Value`s. The type system has no knowledge of its shape: Flux currently has one mechanism for key-value grouping: the untyped `Hash { pairs: Vec<(Expression, Expression)> }`. This is a runtime map where keys and values are arbitrary `Value`s. The type system has no knowledge of its shape:

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### 3. Goals

1. Declare record types with `record Name { field: Type, ... }` syntax.
2. Construct record values with named field syntax: `Point { x: 1.0, y: 2.0 }`.
3. Enforce field presence and field types at compile time.
4. Compile-time type for `record.field` access derived from the record declaration.
5. Typed functional update: `{ ...base, field: new_val }` where the result type is inferred from the spread base.
6. Pattern matching: `Point { x, y }` destructuring in `match` arms.
7. HM integration: record type flows through let-bindings and function arguments.
8. Module boundary: record declarations are exportable; field access across modules respects visibility.

### 4. Non-Goals

1. Mutable record fields or update-in-place.
2. Generic record types (e.g. `record Pair<T, U> { first: T, second: U }`) — deferred.
3. Row polymorphism / structural typing for records (e.g. `fn f(r: { name: String })`) — deferred.
4. Record inheritance or extension.
5. Implicit coercion between record types.
6. Merging `Record` with ADT single-variant constructors — records remain a separate form.
7. Default field values.

### 3. Goals

### 4. Non-Goals

### 4. Non-Goals

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

### Consolidated technical points

- **Consolidated technical points:** - **5.1 Declaration:** record User { name: String, age: Int, } ``` - **5.2 Construction:** - The type name prefix is **required** — this distinguishes record construction from u...
- **5.1 Declaration:** - Declared at module top level via a new `Statement::Record`. - Field names must be unique within a record. - All fields are required; no optional or default fields in this vers...
- **5.2 Construction:** - The type name prefix is **required** — this distinguishes record construction from untyped `Hash` literals. - All fields must be present; extra fields are a compile error. - F...
- **5.3 Field Access:** `MemberAccess` expressions already exist in the AST. The compiler will resolve the field type from the record registry when the left-hand side has a known record type.
- **5.4 Functional Update (Spread):** - A spread `{ ...base, field: expr, ... }` expression creates a new record of the same type as `base`. - The compiler checks: `base` must have a known record type, and each over...
- **5.5 Pattern Matching:** match u { User { name: "Alice", age } => print(age), User { name, .. } => print(name), -- `..` ignores remaining fields } ```

### Detailed specification (migrated legacy content)

This proposal was already largely template-structured before corpus normalization. Detailed normative text is captured in the sections above.

### Historical notes

- No additional historical metadata was found in the legacy document.

## Drawbacks
[drawbacks]: #drawbacks

### 4. Non-Goals

1. Mutable record fields or update-in-place.
2. Generic record types (e.g. `record Pair<T, U> { first: T, second: U }`) — deferred.
3. Row polymorphism / structural typing for records (e.g. `fn f(r: { name: String })`) — deferred.
4. Record inheritance or extension.
5. Implicit coercion between record types.
6. Merging `Record` with ADT single-variant constructors — records remain a separate form.
7. Default field values.

### 4. Non-Goals

### 4. Non-Goals

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

1. A strict template improves comparability and review quality across proposals.
2. Preserving migrated technical content avoids loss of implementation context.
3. Historical notes keep prior status decisions auditable without duplicating top-level metadata.

## Prior art
[prior-art]: #prior-art

The technical details below consolidate implementation, validation, and policy notes from the legacy proposal.

No additional prior art identified beyond references already listed in the legacy content.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- No unresolved questions were explicitly listed in the legacy text.
- Follow-up questions should be tracked in Proposal PR and Flux Issue fields when created.

## Future possibilities
[future-possibilities]: #future-possibilities

- Future expansion should preserve diagnostics stability and test-backed semantics.
- Any post-MVP scope should be tracked as explicit follow-up proposals.
