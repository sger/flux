# Value System

> Source: `src/runtime/value.rs`

All runtime values in Flux are represented by the `Value` enum. Understanding its variants and memory model is essential for working on the VM, builtins, JIT, and GC.

## Value Variants

```rust
enum Value {
    // Primitives (stack-allocated, no heap)
    Uninit,              // Internal VM sentinel — never exposed to user code
    Integer(i64),
    Float(f64),
    Boolean(bool),
    String(Rc<str>),     // Immutable, shared
    None,                // Empty list AND the None option value
    EmptyList,           // Alias — both None and EmptyList represent empty cons list

    // Option / Either constructors (keywords, not library functions)
    Some(Rc<Value>),
    Left(Rc<Value>),
    Right(Rc<Value>),

    // Control flow sentinel
    ReturnValue(Rc<Value>),  // Wraps the return value of a block; unwrapped by the VM

    // Collections
    Array(Rc<Vec<Value>>),   // Rc-backed; NOT GC-managed
    Tuple(Rc<Vec<Value>>),   // Fixed-size, heterogeneous; NOT GC-managed
    Gc(GcHandle),            // GC-managed: cons cells and HAMT maps

    // Functions
    Function(Rc<CompiledFunction>),  // Named function (bytecode)
    Closure(Rc<Closure>),            // Function + captured free variables
    Builtin(u8),                     // Index into BUILTINS array
    JitClosure(Rc<JitClosure>),      // JIT-compiled function (feature-gated)
}
```

## Memory Model

### Rc for Sharing

Most heap values use `Rc` for cheap cloning — cloning a `Value` is O(1) regardless of size. This works safely because of the **no-cycle invariant**.

### No-Cycle Invariant

Values must form a **DAG (directed acyclic graph)**, never cyclic graphs. Cycles would cause `Rc` to leak memory. The language enforces this through immutability — there is no mutation, so you cannot wire two values to point at each other after creation.

The only exception is the GC heap, which uses mark-and-sweep rather than reference counting precisely because structural sharing in HAMT and cons lists could otherwise complicate ownership.

### Rc vs GC Collections

| Collection | Variant | Memory | Sharing |
|-----------|---------|--------|---------|
| Array | `Array(Rc<Vec<Value>>)` | Rc | Clone-on-write style |
| Tuple | `Tuple(Rc<Vec<Value>>)` | Rc | Immutable, shared |
| Cons list | `Gc(GcHandle)` | GC heap | Structural (O(1) prepend) |
| Hash map | `Gc(GcHandle)` | GC heap | Structural (HAMT) |

Arrays and tuples do **not** go through the GC — they live on the Rc heap. The GC heap is only for persistent data structures that require structural sharing.

## Key Methods

### `type_name() -> &'static str`

Returns the canonical type label used by `type_of()` builtin and error messages:

| Variant | `type_name()` |
|---------|--------------|
| `Integer` | `"Int"` |
| `Float` | `"Float"` |
| `Boolean` | `"Bool"` |
| `String` | `"String"` |
| `None` | `"None"` |
| `Some(_)` | `"Some"` |
| `Array` | `"Array"` |
| `Tuple` | `"Tuple"` |
| `Gc` | `"List"` or `"Map"` (resolved at runtime) |
| `Function` / `Closure` | `"Function"` |
| `Builtin` | `"Builtin"` |

### `is_truthy() -> bool`

Only two values are falsy: `Boolean(false)` and `None` (/ `EmptyList`). Everything else — including `0`, `""`, empty arrays — is truthy.

### `to_hash_key() -> Option<HashKey>`

Only `Integer`, `Boolean`, and `String` values can be used as hash map keys. Returns `None` for all other types, which triggers a `KEY_NOT_HASHABLE` runtime error.

### `to_string_value() -> String`

Used in string interpolation (`"#{expr}"`). Unlike `Display`, strings are returned without surrounding quotes.

## Displaying GC Values

`Display` for `Value::Gc` prints `<gc@N>` because it has no access to the heap:

```rust
// Wrong: prints <gc@42>
println!("{}", value);

// Correct: prints [1, 2, 3]
use crate::runtime::builtins::list_ops;
println!("{}", list_ops::format_value(&value, ctx));
```

Always use `format_value()` when rendering cons lists or maps in user-visible output.

## Uninit and ReturnValue Sentinels

- **`Uninit`** — placed on the stack for locals that haven't been assigned yet. Accessing an `Uninit` value is a VM internal error, not a user-visible one.
- **`ReturnValue(v)`** — wraps a value returned from a block via `return`. The VM unwraps it when propagating through frames. Users never see this variant.
