# Proposal 048: Typed Record Types

**Status:** Draft
**Date:** 2026-02-26
**Depends on:** `032_type_system_with_effects.md`, `046_typed_ast_hm_architecture.md`, `047_adt_semantics_deepening.md`

---

## 1. Summary

Introduce typed, immutable record types — named product types with labeled fields — into Flux. Records provide compile-time type checking for field construction, access, and functional update, filling the gap left by the existing untyped `Hash` literal.

---

## 2. Motivation

Flux currently has one mechanism for key-value grouping: the untyped `Hash { pairs: Vec<(Expression, Expression)> }`. This is a runtime map where keys and values are arbitrary `Value`s. The type system has no knowledge of its shape:

```flux
let user = { name: "Alice", age: 30 }
user.name  -- type: Any at compile time
```

There is no way to express "this function takes a user with a `name: String` and returns an updated user". The spread update pattern `{ ...user, age: user.age + 1 }` has appeared in proposals but was never implemented under type checking. As a result:

- `user.name` has no type at compile time — all member accesses on hashes are `Any`
- Missing field typos are runtime errors, not compile errors
- Functions accepting records cannot express or enforce their shape
- HM inference cannot propagate field types through expressions

Records solve this by giving the type system nominal knowledge of field shapes.

---

## 3. Goals

1. Declare record types with `record Name { field: Type, ... }` syntax.
2. Construct record values with named field syntax: `Point { x: 1.0, y: 2.0 }`.
3. Enforce field presence and field types at compile time.
4. Compile-time type for `record.field` access derived from the record declaration.
5. Typed functional update: `{ ...base, field: new_val }` where the result type is inferred from the spread base.
6. Pattern matching: `Point { x, y }` destructuring in `match` arms.
7. HM integration: record type flows through let-bindings and function arguments.
8. Module boundary: record declarations are exportable; field access across modules respects visibility.

---

## 4. Non-Goals

1. Mutable record fields or update-in-place.
2. Generic record types (e.g. `record Pair<T, U> { first: T, second: U }`) — deferred.
3. Row polymorphism / structural typing for records (e.g. `fn f(r: { name: String })`) — deferred.
4. Record inheritance or extension.
5. Implicit coercion between record types.
6. Merging `Record` with ADT single-variant constructors — records remain a separate form.
7. Default field values.

---

## 5. Language Design

### 5.1 Declaration

```flux
record Point {
  x: Float,
  y: Float,
}

record User {
  name: String,
  age:  Int,
}
```

- Declared at module top level via a new `Statement::Record`.
- Field names must be unique within a record.
- All fields are required; no optional or default fields in this version.
- Field order is stable and significant for construction sugar.

### 5.2 Construction

```flux
let p = Point { x: 1.0, y: 2.0 }
let u = User { name: "Alice", age: 30 }
```

- The type name prefix is **required** — this distinguishes record construction from untyped `Hash` literals.
- All fields must be present; extra fields are a compile error.
- Field order in the literal is irrelevant; fields are matched by name.

Construction shorthand (when binding names match field names):

```flux
let x = 1.0
let y = 2.0
let p = Point { x, y }    -- equivalent to Point { x: x, y: y }
```

### 5.3 Field Access

```flux
p.x        -- type: Float (derived from Point declaration)
u.name     -- type: String
```

`MemberAccess` expressions already exist in the AST. The compiler will resolve the field type from the record registry when the left-hand side has a known record type.

### 5.4 Functional Update (Spread)

```flux
let p2 = { ...p, x: 0.0 }          -- type: Point
let u2 = { ...u, age: u.age + 1 }  -- type: User
```

- A spread `{ ...base, field: expr, ... }` expression creates a new record of the same type as `base`.
- The compiler checks: `base` must have a known record type, and each override field must be a valid field of that type with a compatible value type.
- The result type is the same record type as `base`.
- Spread must come first (one spread only); additional fields override individual keys.
- If `base` type is unknown (`Any`), the spread degrades gracefully to an untyped hash update (no compile-time checking, runtime carries the record layout).

### 5.5 Pattern Matching

```flux
match p {
  Point { x, y } => x + y,
}

match u {
  User { name: "Alice", age } => print(age),
  User { name, .. }           => print(name),   -- `..` ignores remaining fields
}
```

- Record patterns use the same type-name-prefixed form.
- Shorthand `{ field }` binds `field` to the field value.
- `..` ignores unlisted fields (not a wildcard match — just field elision in the pattern).
- Match arms over a record type do **not** require exhaustiveness the same way ADTs do; there is only one constructor. An unmatched pattern error is already covered by the existing guard policy.

### 5.6 Type Annotations

Records are referenced by name in type annotations:

```flux
fn distance(a: Point, b: Point) -> Float {
  let dx = a.x - b.x
  let dy = a.y - b.y
  sqrt(dx * dx + dy * dy)
}
```

`Point` is parsed as `TypeExpr::Named { name: "Point", args: [] }` — no change to `TypeExpr`. The compiler resolves named types against the record registry (in addition to the ADT registry).

---

## 6. Type System Integration

### 6.1 HM Inference (`src/types/infer_type.rs`)

Add a new type variant:

```rust
Ty::Record(Symbol, Vec<(Symbol, Ty)>)
//         ^name   ^fields in declaration order
```

- Record construction `RecordName { f1: e1, f2: e2 }` unifies each `eN` type with the declared field type.
- Field access `expr.field` — if `expr` has type `Ty::Record(name, fields)`, the result type is `fields[field]`.
- Spread update `{ ...base, f: e }` — infer `base` type as `Ty::Record(name, fields)`, unify `e` with `fields[f]`, result type is the same `Ty::Record`.
- If the record type is unknown at the `{ ...base }` site, emit a type constraint deferring to runtime (gradual typing safety).

### 6.2 `TypeExpr` — No Changes

`TypeExpr::Named` already represents record types by name. `contracts.rs` (convert_type_expr) gains a branch: when a `Named` type resolves against the record registry, produce `RuntimeType::Record(symbol)`.

### 6.3 `RuntimeType`

```rust
RuntimeType::Record(Symbol)
// stores only the type name; field shape comes from the record registry at runtime
```

`matches_value` checks `Value::Record(rv)` where `rv.type_name == symbol`.

---

## 7. Runtime Representation

### 7.1 `RecordValue` (`src/runtime/value.rs`)

```rust
pub struct RecordValue {
    pub type_name: Symbol,
    pub fields: Vec<(Symbol, Value)>,   // declaration order
}
```

### 7.2 `Value::Record`

```rust
Value::Record(Rc<RecordValue>)
```

`Rc<RecordValue>` is a thin pointer — 8 bytes. With discriminant, the variant fits within the existing 24-byte `Value` bound. The `value_size_is_compact` unit test must remain green.

### 7.3 Field Access at Runtime

Linear scan over `fields` by `Symbol`. For the record sizes typical in Flux programs (2–10 fields) this is fast. A future optimization (HashMap field table) is out of scope.

### 7.4 Spread Update at Runtime

`OpRecordUpdate { base: reg, overrides: Vec<(Symbol, reg)> }` — clones the base `RecordValue`, applies field overrides by name, produces a new `Value::Record`. This is purely functional; the original is unchanged.

---

## 8. Compiler Infrastructure

### 8.1 New Statement

`src/syntax/statement.rs`:
```rust
Statement::Record {
    name:   Identifier,
    fields: Vec<(Identifier, TypeExpr)>,
    span:   Span,
}
```

### 8.2 New Expression Variants

`src/syntax/expression.rs`:
```rust
Expression::RecordLiteral {
    type_name: Identifier,
    fields:    Vec<(Identifier, Expression)>,
    span:      Span,
}

Expression::RecordUpdate {
    base:      Box<Expression>,      // the `...base` spread expression
    overrides: Vec<(Identifier, Expression)>,
    span:      Span,
}
```

`Expression::Hash` remains unchanged for untyped map literals.

### 8.3 New Pattern Variant

```rust
Pattern::Record {
    type_name: Identifier,
    fields:    Vec<(Identifier, Pattern)>,  // bound fields
    rest:      bool,                         // `..` present
    span:      Span,
}
```

### 8.4 Record Registry (`src/bytecode/compiler/`)

A `record_registry.rs` analogous to `adt_registry.rs`. Populated during the global-predeclaration pass (pass 1). Maps `Symbol → Vec<(Symbol, RuntimeType)>` (field name → field type, declaration order).

### 8.5 New Opcodes (`src/bytecode/op_code.rs`)

```
OpRecordNew    { type_sym: Symbol, field_syms: Vec<Symbol> }  -- pops N values, pushes Record
OpRecordField  { field_sym: Symbol }                           -- pops Record, pushes field value
OpRecordUpdate { overrides: Vec<Symbol> }                      -- pops base + N values, pushes new Record
```

Discriminants are appended at the tail of the existing enum — **never reorder**.

---

## 9. Implementation Plan

1. **Parser + AST**: Add `record` keyword, parse `Statement::Record`, `Expression::RecordLiteral`, `Expression::RecordUpdate`, `Pattern::Record`. Add shorthand construction and `..` rest-pattern.
2. **Record Registry**: Implement `record_registry.rs`; populate in compiler pass 1; validate field name uniqueness on declaration.
3. **Compilation — construction**: Emit `OpRecordNew` for `RecordLiteral`; validate field presence and types against registry at compile time.
4. **Compilation — field access**: In `MemberAccess` compilation, detect when the left-hand object has a known record type and emit `OpRecordField`; otherwise fall back to dynamic `OpGetField` (hash-style lookup for `Any`-typed expressions).
5. **Compilation — spread update**: Validate base record type and override fields; emit `OpRecordUpdate`.
6. **RuntimeType / Value**: Add `RuntimeType::Record(Symbol)`, `Value::Record(Rc<RecordValue>)`, `RecordValue` struct. Update `matches_value`. Verify `value_size_is_compact`.
7. **HM inference**: Add `Ty::Record` variant; implement field access constraint resolution; integrate construction and update into inference pass (Iter 3 of HM plan).
8. **Pattern matching**: Add `Pattern::Record` compilation; connect to exhaustiveness checker (single-constructor record is always exhaustive unless guarded).
9. **Module boundaries**: Record declarations export their type and field types. Cross-module field access respects the declared types.
10. **JIT**: Add `rt_record_new`, `rt_record_field`, `rt_record_update` runtime helpers in `jit/runtime_helpers.rs`; emit corresponding Cranelift IR in `jit/compiler.rs`.
11. **Diagnostics**: Register new error codes (see §10).
12. **Examples + snapshots**: Add `examples/type_system/` fixtures; update snapshot suite.

---

## 10. Diagnostics

New error codes to register in `src/diagnostics/registry.rs`:

| Code | Name | Trigger |
|------|------|---------|
| E085 | `RECORD_UNKNOWN_FIELD` | Field name in construction or access not declared in record |
| E086 | `RECORD_MISSING_FIELD` | Construction omits one or more required fields |
| E087 | `RECORD_FIELD_TYPE_MISMATCH` | Field value type incompatible with declared field type |
| E088 | `RECORD_SPREAD_TYPE_MISMATCH` | Spread base has wrong record type, or override field not in record |

Example diagnostic for E086:

```
error[E086]: RECORD_MISSING_FIELD
  --> examples/type_system/records.flx:8:11
   |
 8 | let p = Point { x: 1.0 }
   |         ^^^^^^^^^^^^^^^^ record `Point` requires field `y`, which is missing
   |
   = hint: add `y: <Float>` to the record literal
```

All new codes follow the diagnostics compatibility contract: code, title, primary label, and ordering class are stable once assigned.

---

## 11. Fixtures and Tests

### Passing additions (`examples/type_system/`)

- Basic record declaration and construction.
- Field access with inferred types.
- Functional update preserving record type.
- Pattern matching with shorthand and `..`.
- Records as function parameters and return types.
- Records in let-bindings with type annotations.
- Records in arrays: `[Point { x: 0.0, y: 0.0 }, Point { x: 1.0, y: 1.0 }]`.

### Failing additions

- Construction with unknown field → E085.
- Construction missing required field → E086.
- Construction with wrong field type → E087.
- Spread update on non-record expression → E088.
- Spread with wrong record type in override → E088.
- Field access `r.nonexistent` when record type is known → E085.

### Required gates

```bash
cargo fmt --all -- --check
cargo check --all --all-features
cargo test --all --all-features --lib
cargo insta test --accept --test examples_fixtures_snapshots
cargo test --all --all-features purity_vm_jit_parity_snapshots
```

---

## 12. Acceptance Criteria

1. `record` declarations parse and are registered before compilation of any expression.
2. Record construction detects missing and unknown fields at compile time.
3. Field types are known at compile time for typed record expressions; `r.field` has a non-`Any` type.
4. Spread update `{ ...base, f: v }` type-checks when `base` has a known record type.
5. `Pattern::Record` compiles and runs correctly in VM and JIT.
6. `Value::Record` size holds within 24-byte invariant (test remains green).
7. HM inference propagates `Ty::Record` through let-bindings and function calls.
8. VM/JIT parity for all record operations.
9. Error codes E085–E088 produce actionable diagnostics with accurate spans.

---

## 13. Explicit Assumptions and Defaults

1. Untyped `Hash` literals remain unchanged and coexist with typed records — no breaking change.
2. Records are nominal (name matters), not structural — `{ x: 1.0, y: 2.0 }` (Hash) ≠ `Point { x: 1.0, y: 2.0 }` (Record).
3. Field access on an `Any`-typed expression falls back to dynamic hash-style lookup (gradual typing safety).
4. All fields are public within a module; cross-module record access requires the record declaration to be in scope.
5. Generic records (`record Pair<T, U>`) are explicitly deferred; the infrastructure for generic ADTs (post-047) will inform this design.
6. `OpCode` discriminants for new record opcodes are appended at the tail — existing discriminants are untouched.
