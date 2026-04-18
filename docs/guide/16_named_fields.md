# Chapter 16 — Named Fields for Data Types

> Proposal: [`docs/proposals/implemented/0152_named_fields_for_data_types.md`](../proposals/implemented/0152_named_fields_for_data_types.md)

## Learning Goals

- Declare `data` variants with named fields.
- Construct values with named-field syntax and field punning.
- Access fields with dot syntax and destructure them with named patterns.
- Build updated records with the spread operator.

## Declaration

A `data` variant can declare its fields by name in braces:

```flux
data Point { Point { x: Int, y: Int } }

data Record { Record { id: Int, name: String, age: Int } }
```

A single `data` declaration must pick one style — every variant either uses
positional `Ctor(T1, T2)` fields or named `Ctor { f1: T1, f2: T2 }` fields.
Mixing them emits **E465**.

Shared field names across multiple named-field variants must have the same
type. Incompatible shared types emit **E467**.

## Construction

Use the declared variant name followed by a brace-list of `field: value`
pairs. Fields may be listed in any order — the compiler reorders them to
match the declaration.

```flux
let p = Point { x: 1, y: 2 }
let r = Point { y: 2, x: 1 }           // same value; order doesn't matter
let alice = Record { id: 1, name: "Alice", age: 30 }
```

**Field punning.** When a local variable has the same name as a field,
you can omit the `: value` part:

```flux
let id = 42
let name = "Alice"
let age = 30
let alice = Record { id, name, age }   // shorthand for { id: id, name: name, age: age }
```

Unbound punned names emit **E466**. Missing fields emit **E460**; unknown or
duplicate fields emit **E461**/**E462**.

## Dot access

Named fields are reached with `.`:

```flux
print(alice.name)         // "Alice"
print(alice.age)          // 30
```

For multi-variant ADTs, dot access works as long as the field is declared
in every variant with the same type. Access to a field declared in no
variant emits **E463**.

## Named patterns

Destructure a named-field value by listing the fields you care about. Use
`...` to ignore the rest:

```flux
match alice {
    Record { name, age, ... } -> print(name)
}
```

Bare field names inside a pattern are punned: `Record { name }` binds a
local `name`. Explicit binders use `field: subpattern`:

```flux
match p {
    Point { x: 0, y: 0 } -> "origin",
    Point { x, y }       -> "at " ++ to_string(x) ++ ", " ++ to_string(y),
}
```

## Functional update (spread)

Build a new value from an existing one, overriding selected fields:

```flux
let older = { ...alice, age: alice.age + 1 }
```

Spread requires a named-field ADT whose variant is statically known — either
the ADT has a single named-field variant, or the base is a literal named
constructor. Otherwise you get **E468** (unknown variant) or **E464** (not a
named-field type).

## Erasure

Named fields are purely a surface-syntax feature. After type checking, the
compiler reorders every named construction, spread, and dot-access into its
positional equivalent. Downstream stages (Core IR, Aether RC, VM bytecode,
LLVM) see only classic positional constructors, so there is no runtime cost
and no interaction with existing ADT semantics.

## Error codes

| Code | Meaning |
|------|---------|
| **E460** | Missing field in a named constructor call |
| **E461** | Unknown field in construction, pattern, or spread |
| **E462** | Duplicate field in one construction or pattern |
| **E463** | Dot access names a field no variant declares |
| **E464** | Spread on a non-named-field value |
| **E465** | Data type mixes positional and named-field variants |
| **E466** | Punned field has no in-scope binding |
| **E467** | Shared field name has different types across variants |
| **E468** | Spread on multi-variant ADT whose variant isn't known |

Worked examples live under [`examples/compiler_errors/`](../../examples/compiler_errors/).
