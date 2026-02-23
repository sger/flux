# Base Functions

> Source: `src/runtime/base/`
> Proposal context:
> - Base prelude architecture: `docs/proposals/028_base.md`
> - Base API classification and review policy: `docs/internals/base_api.md`
> - Flow stdlib architecture: `docs/proposals/030_flow.md`

Flux currently exposes 75 runtime Base function implementations. After Proposal 028 Phase 7, Base naming is canonical.

## Current Architecture (Phase 6)

### 1. Implementation modules (`runtime/base/*`)

Base function implementations live under `src/runtime/base/*`:
- `array_ops.rs`
- `string_ops.rs`
- `hash_ops.rs`
- `list_ops.rs`
- `numeric_ops.rs`
- `io_ops.rs`
- `type_check.rs`
- `assert_ops.rs`

Each Base function uses the same signature:

```rust
fn(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String>
```

### 2. Canonical registry order

`src/runtime/base/mod.rs` defines `BASE_FUNCTIONS`, a deterministic ordered array of `BaseFunction` entries.

`src/runtime/base/registry.rs` exposes that same ordering via `BaseModule::names()` and lookup helpers.

### 3. Compiler registration

`src/bytecode/compiler/mod.rs` derives Base symbol indices from `BaseModule::new().names().enumerate()` and registers each name with the matching index.

This keeps compiler/runtime mapping deterministic without manual duplicated index tables.

## Dispatch Paths

### VM path

- Compiler emits `OpGetBase(index)` using Base-derived index registration.
- VM resolves index to `Value::BaseFunction(index)`.
- `OpCall` dispatches to `BASE_FUNCTIONS[index].func(...)`.

### JIT path

JIT helpers and context dispatch Base functions by the same Base index, so VM/JIT share one canonical registry.

## Lookup helpers

Base lookup helpers:

- `get_base_function(name)`
- `get_base_function_index(name)`
- `get_base_function_by_index(index)`

## Full Base Function Catalog

### array_ops (24 functions)
`len` `push` `reverse` `contains` `slice` `sort` `sort_by` `map` `filter` `fold` `flat_map` `any` `all` `find` `zip` `flatten` `count` `concat` `range` `sum` `product` `first` `last` `rest`

### string_ops (11 functions)
`split` `join` `trim` `upper` `lower` `starts_with` `ends_with` `replace` `chars` `substring` `to_string`

### hash_ops (8 functions)
`keys` `values` `has_key` `merge` `delete` `put` `get` `is_map`

### list_ops (6 functions)
`hd` `tl` `list` `is_list` `to_list` `to_array`

### numeric_ops (3 functions)
`abs` `min` `max`

### io_ops (8 functions)
`print` `read_file` `read_lines` `read_stdin` `parse_int` `parse_ints` `split_ints` `now_ms` `time`

### type_check (9 functions)
`type_of` `is_int` `is_float` `is_string` `is_bool` `is_array` `is_hash` `is_none` `is_some`

### assert_ops (5 functions)
`assert_eq` `assert_neq` `assert_true` `assert_false` `assert_throws`

## Adding a New Base Function (Phase 7)

1. Implement the function in `src/runtime/base/<module>.rs`.
2. Add it to `BASE_FUNCTIONS` in `src/runtime/base/mod.rs` in deterministic order.
3. Do not add manual compiler `define_base_function(...)` wiring; compiler registration is derived from `BaseModule` ordering.
4. Run deterministic index and VM/JIT parity tests.

Notes:
- Index order is ABI-sensitive for bytecode/JIT dispatch.
- Any index-affecting change must be coordinated with cache/versioning policy from `docs/proposals/028_base.md`.
