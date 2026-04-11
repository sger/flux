- Feature Name: Named Fields for Data Types
- Start Date: 2026-04-10
- Status: Not Implemented
- Proposal PR:
- Flux Issue:
- Supersedes: Proposal 0048 (Typed Record Types)

## Summary
[summary]: #summary

Extend the existing `data` declaration with optional named fields, enabling compile-time typed field access via dot syntax, functional update, and field-based pattern matching. Named fields are erased to positional indices after type checking, requiring zero changes to Core IR, Aether RC, or backend code generation. This supersedes Proposal 0048, which introduced a separate `record` keyword.

## Motivation
[motivation]: #motivation

Flux currently has two mechanisms for product-type data, both with significant gaps:

1. **ADTs with positional fields** -- `data User { User(String, Int) }` provides full type safety but no named access. Extracting a field requires pattern matching (`match u { User(name, _) => name }`), which doesn't scale past 3-4 fields and produces brittle code when field order changes.

2. **Untyped Hash literals** -- `{ "name": "Alice", "age": 30 }` provides named access but zero compile-time safety. The type system sees `Map<String, a>`, so field presence, field types, and typos are all runtime errors.

This gap matters for real programs:

```flux
-- Today: positional, brittle, no dot access
data Config { Config(String, Int, Bool, String, Int) }

fn get_host(c: Config) -> String =
  match c { Config(host, _, _, _, _) => host }

-- Today: untyped, no compile-time checking
let config = { "host": "localhost", "port": 8080 }
let host = hash_get(config, "hsot")  -- typo: runtime error, not compile error
```

With named fields:

```flux
data Config { Config { host: String, port: Int, debug: Bool } }

let c = Config { host: "localhost", port: 8080, debug: false }
let h = c.host          -- type-safe, IDE-friendly
let c2 = { ...c, port: 9090 }  -- functional update
```

### Use cases

- **Domain modeling**: User profiles, HTTP requests/responses, configuration, AST nodes
- **API boundaries**: Named fields serve as documentation; positional fields at boundaries produce unreadable code
- **Refactoring safety**: Adding a field to a named-field variant is a compile error at every incomplete construction site; adding a positional field silently shifts indices

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Declaring named fields

Named fields are an extension of the existing `data` syntax. A variant uses `{ }` with `name: Type` pairs instead of positional `( )`:

```flux
data Point { Point { x: Float, y: Float } }

data Shape {
  Circle { center: Point, radius: Float },
  Rect { top_left: Point, bottom_right: Point },
}
```

Positional and named variants **cannot** be mixed within the same data type. A data type is either all-positional or all-named.

Generic data types work as expected:

```flux
data Pair<a, b> { Pair { first: a, second: b } }

data Result<t, e> {
  Ok { value: t },
  Err { error: e },
}
```

### Construction

Named-field construction requires the variant name prefix and all fields:

```flux
let p = Point { x: 1.0, y: 2.0 }
let s = Circle { center: p, radius: 5.0 }
let r = Ok { value: 42 }           -- Result<Int, e>
```

Field order does not matter:

```flux
let p = Point { y: 2.0, x: 1.0 }  -- same as Point { x: 1.0, y: 2.0 }
```

Missing or extra fields are compile errors:

```flux
let p = Point { x: 1.0 }           -- error: missing field `y`
let p = Point { x: 1.0, z: 3.0 }  -- error: unknown field `z`
```

### Field punning

When a variable in scope has the same name as a field, you can omit the `: value` part. This works in construction, pattern matching, and functional update:

```flux
let x = 1.0
let y = 2.0
let p = Point { x, y }            -- same as Point { x: x, y: y }
```

Mixed punning and explicit fields:

```flux
let x = 3.0
let p = Point { x, y: 7.0 }      -- Point { x: 3.0, y: 7.0 }
```

In pattern matching, punning binds the field value to a variable with the field's name:

```flux
match p {
  Point { x, y } => x + y,        -- binds x and y from the fields
}
```

This follows the same convention as Rust, OCaml, and JavaScript.

### Field access (dot syntax)

Field access uses dot syntax, resolved at compile time from the inferred type:

```flux
let p = Point { x: 1.0, y: 2.0 }
print(p.x)        -- 1.0
print(p.y)        -- 2.0
```

Dot access works through let-bindings and function arguments:

```flux
fn distance(a: Point, b: Point) -> Float =
  let dx = a.x - b.x
  let dy = a.y - b.y
  sqrt(dx * dx + dy * dy)
```

For multi-variant data types, dot access has two modes depending on whether the field exists in all variants:

**Common fields** -- when a field exists in **all** variants with the same type, dot access returns the value directly:

```flux
data Shape {
  Circle { center: Point, radius: Float },
  Rect { center: Point, width: Float, height: Float },
}

fn get_center(s: Shape) -> Point = s.center   -- OK: `center` in all variants, returns Point
```

**Partial fields** -- when a field exists in only some variants, dot access returns `Option<T>`:

```flux
fn get_radius(s: Shape) -> Option<Float> = s.radius
-- Circle { radius: 5.0 }.radius   => Some(5.0)
-- Rect { ... }.radius             => None
```

This compiles to an implicit match:

```flux
-- s.radius desugars to:
match s {
  Circle { radius, .. } => Some(radius),
  _ => None,
}
```

This avoids forcing users into verbose pattern matches just to attempt field access, while keeping the type system honest -- the `Option` return makes partiality explicit. For the common case where a field exists in all variants, there's no `Option` wrapper overhead.

**Same-name, same-type rule**: When a field name appears in multiple variants, it must have the same type in all of them. This is enforced at declaration time:

```flux
-- OK: `center` is Point in both variants
data Shape {
  Circle { center: Point, radius: Float },
  Rect { center: Point, width: Float, height: Float },
}

-- error[E467]: field `value` has type `Int` in `Ok` but `String` in `Err`
data Bad {
  Ok { value: Int },
  Err { value: String },    -- use type parameters instead
}

-- correct: use generics
data Result<t, e> {
  Ok { value: t },
  Err { error: e },         -- or use a different field name
}
```

If you want to assert that you have a specific variant, use pattern matching directly:

```flux
match s {
  Circle { center, radius } => do_circle_stuff(center, radius),
  Rect { center, width, height } => do_rect_stuff(center, width, height),
}
```

### Functional update (spread syntax)

The spread syntax `{ ...base, field: val }` creates a new value of the same type, copying all fields from `base` and overriding the specified ones:

```flux
let p = Point { x: 1.0, y: 2.0 }
let p2 = { ...p, x: 3.0 }         -- Point { x: 3.0, y: 2.0 }
```

Spread with no overrides copies the entire value:

```flux
let p3 = { ...p }                  -- Point { x: 1.0, y: 2.0 } (shallow copy)
```

Spread works with field punning:

```flux
let x = 5.0
let p4 = { ...p, x }              -- Point { x: 5.0, y: 2.0 }
```

Multiple fields can be overridden:

```flux
data Config { Config { host: String, port: Int, debug: Bool, timeout: Int } }

let default_config = Config { host: "localhost", port: 8080, debug: false, timeout: 30 }
let dev_config = { ...default_config, debug: true, port: 3000 }
```

The base expression must have a known named-field type. Override fields must exist in that type and have compatible types:

```flux
let p2 = { ...p, z: 3.0 }         -- error: no field `z` in Point
let p2 = { ...p, x: "hello" }     -- error: expected Float, got String
```

Spread on multi-variant types requires the base to have a **statically known variant** (typically from a constructor call or a pattern binding inside a match arm). This is enforced because different variants have different fields, and silently reconstructing an unknown variant would hide bugs:

```flux
-- OK: variant is known from the constructor
let c = Circle { center: origin, radius: 5.0 }
let c2 = { ...c, radius: 10.0 }

-- OK: variant is known from pattern matching
match shape {
  Circle { center, radius } as c => { ...c, radius: radius * 2.0 },
  Rect { center, width, height } as r => { ...r, width: width + 1.0 },
}

-- error[E468]: spread requires a statically known variant
fn scale(s: Shape) -> Shape = { ...s, center: origin }
```

### Pattern matching

Named-field patterns destructure by field name:

```flux
match shape {
  Circle { center, radius } => print(radius),
  Rect { top_left, .. } => print(top_left.x),   -- `..` ignores remaining
}
```

Field punning: `{ center }` is shorthand for `{ center: center }`, binding the field value to a variable with the same name.

### HM integration

Named-field types flow through Hindley-Milner inference like any other type:

```flux
fn swap(p: Pair<a, b>) -> Pair<b, a> =
  Pair { first: p.second, second: p.first }

-- inferred: swap : Pair<a, b> -> Pair<b, a>
```

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Parser changes

Extend `DataVariant` to support an alternative named-field form:

```rust
pub struct DataVariant {
    pub name: Identifier,
    pub fields: DataFields,      // NEW: enum instead of Vec<TypeExpr>
    pub span: Span,
}

pub enum DataFields {
    Positional(Vec<TypeExpr>),                       // existing: Foo(Int, String)
    Named(Vec<(Identifier, TypeExpr)>),              // new: Foo { x: Int, y: String }
}
```

The parser distinguishes the two forms by the token after the variant name: `(` for positional, `{` for named.

Validation: all variants in a `data` declaration must use the same form (all positional or all named). Field names must be unique within a variant. Duplicate field names across variants of the same type are permitted and encouraged for shared-field access, but **must have the same type** across all variants where they appear (enforced at declaration time, E467).

### Field registry

A new `FieldRegistry` maps named-field types to their field metadata:

```rust
struct FieldInfo {
    name: Symbol,
    index: usize,          // positional index for Core IR
    type_expr: TypeExpr,   // declared type
}

struct FieldRegistry {
    // ADT name -> variant name -> ordered fields
    fields: HashMap<Symbol, HashMap<Symbol, Vec<FieldInfo>>>,
    // ADT name -> fields common to ALL variants (for dot access)
    common_fields: HashMap<Symbol, Vec<FieldInfo>>,
}
```

Built during Phase 1 (collection) of the compiler pipeline, before type inference.

### Type inference changes

1. **Construction**: When the parser produces a named-field constructor call, type inference reorders fields to match declaration order, then delegates to existing constructor inference (positional). This means `Point { y: 2.0, x: 1.0 }` becomes `Point(1.0, 2.0)` internally.

2. **Field punning**: When the parser sees `Point { x, y: 2.0 }`, the bare `x` is syntactically an identifier, not a `name: expr` pair. The parser emits a `PunnedField(Identifier)` node. During type inference, this resolves to `field_name: Var(field_name)` -- looking up `field_name` in the current scope. In patterns, punning binds the field value to a fresh variable with the field's name.

3. **Dot access**: `MemberAccess` resolution gains a new path. When the left-hand side has a known ADT type with named fields:
   - Look up the field name in `FieldRegistry`
   - If found in `common_fields` (all variants with the same type), resolve as that type directly
   - If found in only some variants, resolve as `Option<T>` and desugar to a match expression that produces `Some(field)` for matching variants and `None` otherwise
   - If not found in any variant, fall through to existing module/method resolution

4. **Functional update**: Desugar `{ ...base, field: expr }` into a fresh construction where unmentioned fields are `base.field` accesses. Punned fields in updates (`{ ...p, x }`) desugar to `{ ...p, x: x }`. This is purely a type-inference-time rewrite; Core IR sees a normal `Con` node.

5. **Pattern matching**: Named-field patterns reorder bindings to match declaration order, then lower to existing positional `Constructor` patterns. Punned fields (`Point { x, y }`) lower to positional bindings at the correct indices.

### Core IR representation

**No changes to Core IR.** Named fields are erased after type checking:

- `Point { x: 1.0, y: 2.0 }` lowers to `CoreExpr::Con { tag: Named("Point"), fields: [1.0, 2.0] }`
- `Point { x, y: 2.0 }` (punning) lowers to `CoreExpr::Con { tag: Named("Point"), fields: [Var("x"), 2.0] }`
- `p.x` (common field) lowers to `CoreExpr::TupleField { object: p, index: 0 }`
- `s.radius` (partial field) lowers to `CoreExpr::Case { scrut: s, alts: [Circle(.., r) => Some(r), _ => None] }`
- `{ ...p, x: 3.0 }` lowers to `CoreExpr::Con { tag: Named("Point"), fields: [3.0, TupleField(p, 1)] }`
- `{ ...p }` lowers to `CoreExpr::Con { tag: Named("Point"), fields: [TupleField(p, 0), TupleField(p, 1)] }`

This means **Aether RC, CFG, LIR, LLVM emission, and the VM all require zero changes.**

### Aether integration

Since named fields compile to the same `CoreExpr::Con` / `TupleField` representation as positional fields:

- `flux_dup`/`flux_drop` work unchanged (scan_fsize counts fields, not names)
- Perceus reuse analysis works unchanged (field indices are stable)
- Borrow inference works unchanged (field access is `TupleField`)

### Interaction with type classes

Named-field types participate in type classes the same way positional ADTs do. A `deriving` clause generates instances using field indices:

```flux
data Point { Point { x: Float, y: Float } } deriving (Eq, Show)
-- generates: __tc_Eq_Point_eq compares field 0, then field 1
```

### Module visibility

Named fields follow the same export rules as ADT constructors. If a variant is exported, its field names are accessible for construction, access, and pattern matching. A future proposal may add per-field visibility.

### Error messages

New error codes:

| Code | Condition |
|------|-----------|
| E460 | Missing field in named-field construction |
| E461 | Unknown field in named-field construction |
| E462 | Duplicate field in named-field construction |
| E463 | Dot access on field not present in any variant |
| E464 | Functional update on non-named-field type |
| E465 | Mixed positional/named variants in same data declaration |
| E466 | Punned field name not found in scope |
| E467 | Shared field name has different types across variants |
| E468 | Spread requires a statically known variant |

Example:

```
error[E460]: missing field `y` in `Point` construction

  12 | let p = Point { x: 1.0 }
     |         ^^^^^^^^^^^^^^^^ missing field `y: Float`

  help: add the missing field:
     | let p = Point { x: 1.0, y: <Float> }
```

## Drawbacks
[drawbacks]: #drawbacks

1. **Parser complexity**: The `{` token after a variant name is ambiguous with the start of a block/hash. Resolution: named-field syntax is only valid in `data` declarations and when prefixed by a known variant name in expressions.

2. **Dot syntax ambiguity**: `x.y` could mean module access, field access, or (future) method call. Resolution: field access is checked *after* module resolution, consistent with current `MemberAccess` priority.

3. **Multi-variant partial field access**: Partial fields (present in some but not all variants) return `Option<T>`, which adds unwrapping overhead compared to pattern matching. This is a trade-off between ergonomics (terse dot access) and explicitness (the `Option` makes partiality visible in the type).

4. **No structural typing**: Two data types with identical field names/types are distinct types. This is consistent with Flux's nominal ADT system and avoids the complexity of row polymorphism.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Why extend `data` instead of a separate `record` keyword?

Proposal 0048 introduced `record` as a parallel declaration form. This approach has significant drawbacks:

- **Duplicated infrastructure**: A separate `record` type needs its own type constructor, pattern matching path, Core IR node, Aether handling, and backend support. Extending `data` reuses all existing machinery.
- **User confusion**: "Should I use `data` or `record`?" becomes a FAQ with no clear answer for single-variant product types.
- **Koka precedent**: Koka's `struct` is explicitly sugar for a single-variant `type` with named fields. This is proven to work well with Perceus RC.
- **Haskell precedent**: Haskell records *are* ADTs. The problems with Haskell records stem from top-level selector functions, not from the ADT integration itself.

### Why dot syntax instead of selector functions?

Haskell generates top-level functions like `name :: User -> String`. This causes:
- Name collisions between record types in the same module
- Decades of GHC extensions to work around it (`DuplicateRecordFields`, `OverloadedRecordDot`, `NoFieldSelectors`)

Type-directed dot syntax (Koka's approach) avoids all of these problems.

### Why require generics from day one?

Records without generics are barely useful for real programming. You can't write `Pair<a, b>`, `Result<t, e>`, or `Config<S>`. Since Flux's ADTs already support type parameters, the machinery exists -- there's no reason to defer.

### Impact of not doing this

Without named fields, Flux programs will:
- Use verbose pattern matching for field access on types with 4+ fields
- Rely on untyped Hash literals for named access, losing type safety
- Produce brittle code where adding a field shifts positional indices

## Prior art
[prior-art]: #prior-art

### Koka (direct inspiration)

Koka's `struct` is sugar for a single-variant `type` with named fields:

```koka
struct point
  x : int
  y : int
// desugars to: type point { Point(x: int, y: int) }
```

- Dot syntax (`p.x`) is type-directed
- Functional update via constructor syntax: `p(x = 42)`
- Fields erase to positional indices in the IR
- Perceus reuse works unchanged on named-field types
- No field-name collision problems

Koka's approach is the closest match for Flux given the shared Perceus-inspired memory model.

### Haskell

Haskell records are ADT constructors with named fields:

```haskell
data User = User { name :: String, age :: Int }
```

Generates top-level selector functions (`name :: User -> String`), causing the well-known record field name collision problem. Required 4+ GHC extensions to mitigate (`DuplicateRecordFields`, `OverloadedRecordDot`, `HasField`, `NoFieldSelectors`).

**Lesson**: Don't generate top-level selector functions. Use type-directed dot syntax.

### OCaml

OCaml records are nominal, separate from variant types:

```ocaml
type point = { x: float; y: float }
```

Field names are resolved by type within a module scope. Functional update via `{ p with x = 3.0 }`. Recent OCaml versions disambiguate field names by expected type.

### Elm

Elm has structural record types with row polymorphism:

```elm
type alias Point = { x : Float, y : Float }
extensible : { a | name : String } -> String
```

Powerful but adds significant type system complexity. Flux defers structural/row-polymorphic records.

### Rust

Rust structs have named fields with move semantics and `..base` spread syntax. Nominal typing, no structural subtyping. The `..base` functional update syntax is directly analogous to this proposal's `{ ...base, field: val }`.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

### Resolved during design

- **Shared field name, different types across variants**: Decided: **same name must have same type** (E467). If variants need same-named fields with different types, use type parameters (`Result<t, e>` where `Ok { value: t }` and `Err { value: e }`). This follows Koka's approach and avoids incoherent `Option<???>` return types for partial dot access.

- **Spread on multi-variant types**: Decided: **require statically known variant** (E468). Silent reconstruction of an unknown variant hides bugs. For multi-variant updates, pattern match first, then spread within each arm.

### Open questions

1. **Field order in construction**: The proposal allows any order. Should we lint/warn when construction order doesn't match declaration order? Probably not -- flexibility is the point.

2. **Interaction with Hash literals**: `{ key: val }` is currently a Hash literal. Named-field construction always requires a variant prefix (`Point { x: 1 }`), so there is no ambiguity. Functional update `{ ...base, field: val }` starts with `...` which is not valid in Hash literals, so no ambiguity there either. Confirm during implementation.

## Future possibilities
[future-possibilities]: #future-possibilities

1. **Row-polymorphic records**: `fn getName(r: { name: String | r }) -> String = r.name`. Flux's effect system already uses row polymorphism; extending it to records is a natural evolution. This would enable structural subtyping for records.

2. **Per-field visibility**: `data User { User { pub name: String, age: Int } }` where `age` is module-private.

3. **Default field values**: `data Config { Config { host: String = "localhost", port: Int = 8080 } }`.

4. **Derive macros for named fields**: Auto-generate `Show`, `Eq`, `Ord` using field names in output (e.g., `Point { x: 1.0, y: 2.0 }` instead of `Point(1.0, 2.0)`).

5. **Named-field function arguments**: Extending the named-field concept to function parameters, enabling keyword arguments. This is a separate proposal but benefits from the same infrastructure.

6. **Record update syntax as Aether reuse site**: When `{ ...base, field: val }` is the last use of `base`, Aether could emit a reuse token, enabling in-place field mutation. This falls out naturally from the `Con`-based Core IR lowering.
