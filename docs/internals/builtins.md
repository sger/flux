# Builtin Functions

> Source: `src/runtime/builtins/`

Flux has 75 builtin functions available without any import. This document covers how they are registered internally and how to add a new one.

## Registration — Three Locations

Builtins must be registered in **three places** with a matching array index. The index is the builtin's runtime ID.

### 1. Implementation (`runtime/builtins/<module>.rs`)

```rust
// src/runtime/builtins/array_ops.rs
pub fn builtin_len(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    match args.as_slice() {
        [Value::Array(arr)] => Ok(Value::Integer(arr.len() as i64)),
        [Value::Tuple(t)]   => Ok(Value::Integer(t.len() as i64)),
        [Value::Gc(_)]      => { /* cons list: traverse and count */ }
        [v] => Err(format!("len: expected Array or List, got {}", v.type_name())),
        _   => Err(format!("len: expected 1 argument, got {}", args.len())),
    }
}
```

Signature: `fn(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String>`

- `ctx` gives access to the GC heap, interner, and other runtime state.
- Return `Ok(Value)` on success, `Err(String)` on type/arity error.

### 2. BUILTINS Array (`runtime/builtins/mod.rs`)

```rust
pub static BUILTINS: &[BuiltinFunction] = &[
    BuiltinFunction { name: "len",     func: array_ops::builtin_len },      // index 0
    BuiltinFunction { name: "push",    func: array_ops::builtin_push },     // index 1
    BuiltinFunction { name: "reverse", func: array_ops::builtin_reverse },  // index 2
    // ...
];
```

The **array index is the builtin's ID**. `OpGetBuiltin` emits this index at compile time.

### 3. Symbol Table (`bytecode/compiler/mod.rs`)

```rust
// Must use the same index as the BUILTINS array
symbol_table.define_builtin(0, interner.intern("len"));
symbol_table.define_builtin(1, interner.intern("push"));
symbol_table.define_builtin(2, interner.intern("reverse"));
```

This tells the compiler to emit `OpGetBuiltin(index)` when it sees the name in source code.

## How Dispatch Works

At compile time, the compiler emits `OpGetBuiltin(N)` where `N` is the symbol table index.

At runtime, the VM executes:
```rust
OpGetBuiltin(index) => {
    let func = get_builtin_by_index(index);
    stack.push(Value::Builtin(index));
}
OpCall(arity) if top == Value::Builtin(index) => {
    let args = stack.pop_n(arity);
    let result = BUILTINS[index].func(ctx, args)?;
    stack.push(result);
}
```

The JIT backend calls `rt_call_builtin(ctx, index, args)` in `jit/runtime_helpers.rs`, which resolves the same `BUILTINS[index]` entry — so every new builtin is automatically available in JIT mode.

## Lookup Functions

```rust
get_builtin(name: &str) -> Option<&BuiltinFunction>      // linear scan by name
get_builtin_index(name: &str) -> Option<usize>            // index by name
get_builtin_by_index(index: usize) -> &BuiltinFunction    // direct lookup by ID
```

## Full Builtin Catalog

### array_ops (24 builtins)
`len` `push` `reverse` `contains` `slice` `sort` `sort_by` `map` `filter` `fold` `flat_map` `any` `all` `find` `zip` `flatten` `count` `concat` `range` `sum` `product` `first` `last` `rest`

### string_ops (11 builtins)
`split` `join` `trim` `upper` `lower` `starts_with` `ends_with` `replace` `chars` `substring` `to_string`

### hash_ops (8 builtins)
`keys` `values` `has_key` `merge` `delete` `put` `get` `is_map`

### list_ops (6 builtins)
`hd` `tl` `list` `is_list` `to_list` `to_array`

### numeric_ops (3 builtins)
`abs` `min` `max`

### io_ops (8 builtins)
`print` `read_file` `read_lines` `read_stdin` `parse_int` `parse_ints` `split_ints` `now_ms` `time`

### type_check (9 builtins)
`type_of` `is_int` `is_float` `is_string` `is_bool` `is_array` `is_hash` `is_none` `is_some`

### assert_ops (5 builtins)
`assert_eq` `assert_neq` `assert_true` `assert_false` `assert_throws`

## Adding a New Builtin

1. **Write the function** in the appropriate `runtime/builtins/<module>.rs`:
   ```rust
   pub fn builtin_clamp(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
       match args.as_slice() {
           [Value::Integer(n), Value::Integer(lo), Value::Integer(hi)] =>
               Ok(Value::Integer((*n).clamp(*lo, *hi))),
           _ => Err(format!("clamp: expected (Int, Int, Int), got {} args", args.len())),
       }
   }
   ```

2. **Append to BUILTINS** in `runtime/builtins/mod.rs` — note the index (e.g., 75):
   ```rust
   BuiltinFunction { name: "clamp", func: numeric_ops::builtin_clamp }, // index 75
   ```

3. **Register in the symbol table** in `bytecode/compiler/mod.rs`:
   ```rust
   symbol_table.define_builtin(75, interner.intern("clamp"));
   ```

The index must match across all three. The JIT backend picks it up automatically.
