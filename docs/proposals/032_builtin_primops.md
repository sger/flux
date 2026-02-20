# Proposal 032: Builtin Primops — Specialized Opcode Dispatch for Builtins

**Status:** Draft
**Date:** 2026-02-19
**Scope:** VM bytecode compiler, VM dispatch loop
**JIT impact:** None (separate pipeline, separate effort)

---

## Motivation

Flux has 63 registered builtin functions. Every direct call to any of them — regardless of
simplicity — goes through the same two-instruction generic path:

```
OpGetBuiltin(idx)   ; push Value::Builtin(idx) onto stack
OpCall(n)           ; generic call dispatch
```

At runtime, `OpCall` inspects the top of the stack, matches on `Value::Builtin`, calls
`get_builtin_by_index()` a second time, then invokes the function through a `Vec<Value>`
calling convention.

The compiler already knows at compile time which builtin is being called — it resolves
identifiers to `SymbolScope::Builtin` bindings via the symbol table. That knowledge is
discarded when it emits a generic `OpGetBuiltin + OpCall` sequence, forcing the VM to
re-discover it at runtime on every execution.

Additionally, several opcodes already exist that perform operations equivalent to builtins
(`OpConsHead` ≡ `hd`, `OpConsTail` ≡ `tl`, `OpToString` ≡ `to_string`, `OpIsSome` ≡
`is_some`) — but these opcodes are only emitted for language constructs (pattern matching,
string interpolation), never for direct builtin function calls.

This proposal defines three phases to address this, inspired by how GHC handles primops:
encode compiler-known operations directly into bytecode rather than deferring dispatch to
runtime.

---

## Background: GHC Primops vs BEAM BIFs vs Current Flux

| Approach | Description |
|---|---|
| GHC primops | Compiler inlines operation as machine instructions. No function call. |
| BEAM BIFs | C function, but bypasses Erlang dispatch. Still a call. |
| Flux (current) | Rust function via generic dispatch. Double lookup + stack push/pop. |

The goal is to move Flux closer to the GHC end of this spectrum for simple, pure-value
operations, while keeping the existing path for complex operations that require
`RuntimeContext` or GC interaction.

---

## Current Call Path (Detailed)

For `len(arr)` the compiler emits:

```
OpGetBuiltin(1)   ; 2 bytes
OpGetLocal(0)     ; 2 bytes  (arr)
OpCall(1)         ; 2 bytes
```

At runtime:

1. **`OpGetBuiltin`**: read 1-byte index → `get_builtin_by_index(idx)` (bounds check +
   array index) → push `Value::Builtin(idx)` onto stack
2. **`OpCall`**: read 1-byte arity → compute `callee_idx = sp - 1 - n` → read
   `stack[callee_idx]` → `match` on `Value` enum → `Value::Builtin` arm →
   `get_builtin_by_index(idx)` again → `builtin_fixed_arity(name)` (string comparisons) →
   collect args into `Vec<Value>` → call `(builtin.func)(self, args)`

Overhead per call:
- 2 array lookups (`get_builtin_by_index` called twice)
- 1 stack push + 1 stack pop of `Value::Builtin`
- 1 type-dispatch branch in `OpCall`
- `builtin_fixed_arity` string comparison
- `Vec<Value>` heap allocation for arguments

---

## Current State: What Already Uses Primop-Style Opcodes

These specialized opcodes exist and fire correctly — but only from language constructs,
never from direct builtin function calls:

| Opcode | Fires from | Equivalent builtin | Direct call behavior |
|---|---|---|---|
| `OpConsHead` | Pattern `[h \| t]` → bind `h` | `hd` | `OpGetBuiltin + OpCall` |
| `OpConsTail` | Pattern `[h \| t]` → bind `t` | `tl` | `OpGetBuiltin + OpCall` |
| `OpIsCons` | Pattern `[h \| t]` → check | `is_list` (partial) | `OpGetBuiltin + OpCall` |
| `OpIsEmptyList` | Pattern `[]` → check | — | n/a |
| `OpIsSome` | Pattern `Some(x)` → check | `is_some` | `OpGetBuiltin + OpCall` |
| `OpUnwrapSome` | Pattern `Some(x)` → unwrap | — | n/a |
| `OpToString` | String interpolation `"${x}"` | `to_string` | `OpGetBuiltin + OpCall` |
| `OpIsLeft/Right` | Left/Right pattern matching | — | n/a |

All 63 builtins use the generic path when called directly as functions.

---

## Proposal

### Phase 1 — Wire Existing Opcodes to Direct Calls

**Effort:** Low. Compiler only. No new opcodes. No VM changes.

The 4 builtins that have matching opcodes should emit those opcodes when called directly:

| Direct call | Currently emits | Should emit |
|---|---|---|
| `hd(x)` | `OpGetBuiltin(idx) + OpCall(1)` | `OpConsHead` |
| `tl(x)` | `OpGetBuiltin(idx) + OpCall(1)` | `OpConsTail` |
| `to_string(x)` | `OpGetBuiltin(idx) + OpCall(1)` | `OpToString` |
| `is_some(x)` | `OpGetBuiltin(idx) + OpCall(1)` | `OpIsSome` |

**Implementation:**

In `src/bytecode/compiler/expression.rs`, inside the `Expression::Call` arm, before the
generic path, add a check:

```rust
Expression::Call { function, arguments, .. } => {
    // --- Phase 1: wire existing opcodes ---
    if let Some(opcode) = self.try_emit_as_existing_primop(function, arguments) {
        return opcode;
    }
    // ... existing generic path unchanged ...
}
```

Where `try_emit_as_existing_primop` resolves the callee identifier, checks if it maps to
a known `SymbolScope::Builtin` with a pre-existing opcode, compiles the arguments, and
emits the opcode.

**What does NOT change:**

- Higher-order use (`map(xs, hd)`) still emits `OpGetBuiltin` since the value must be
  representable as `Value::Builtin`.
- All other builtins are unaffected.
- The VM dispatch for these opcodes is unchanged.

**Risk:** Very low. These opcodes are already tested via pattern matching paths.

---

### Phase 2 — True Primops for Simple Builtins

**Effort:** Medium. New opcodes + compiler detection + VM inline handlers.

Add specialized opcodes for the most frequently called, pure-value builtins. These
eliminate the Rust function call and the `Vec<Value>` allocation entirely.

**Candidate builtins** — selected because they:
- Take no `RuntimeContext` (or trivially use it)
- Operate on value types without GC interaction
- Are called frequently in hot paths

| Builtin | New opcode | Operands | VM inline implementation |
|---|---|---|---|
| `len(x)` | `OpLen` | none | `match Array(a)→a.len(), String(s)→s.chars().count()` |
| `abs(x)` | `OpAbs` | none | `match Integer(n)→n.abs(), Float(f)→f.abs()` |
| `min(a,b)` | `OpMin` | none | inline comparison, pop 2, push smaller |
| `max(a,b)` | `OpMax` | none | inline comparison, pop 2, push larger |
| `is_none(x)` | `OpIsNone` | none | `matches!(val, Value::None)` → bool |
| `is_array(x)` | `OpIsArray` | none | `matches!(val, Value::Array(_))` → bool |
| `is_int(x)` | `OpIsInt` | none | `matches!(val, Value::Integer(_))` → bool |
| `is_float(x)` | `OpIsFloat` | none | `matches!(val, Value::Float(_))` → bool |
| `is_string(x)` | `OpIsString` | none | `matches!(val, Value::String(_))` → bool |
| `is_bool(x)` | `OpIsBool` | none | `matches!(val, Value::Boolean(_))` → bool |

**Implementation — three touch points:**

1. **`src/bytecode/op_code.rs`** — add enum variants, `operand_widths` entries (all
   zero-operand), `From<u8>` arms, `disassemble` display.

2. **`src/bytecode/compiler/expression.rs`** — detect direct calls to these builtins and
   emit the specialized opcode instead of `OpGetBuiltin + OpCall`. The compiler already
   has `SymbolScope::Builtin` with the index; a static mapping from builtin name to opcode
   is sufficient.

3. **`src/runtime/vm/dispatch.rs`** — add inline VM handlers:

```rust
OpCode::OpLen => {
    let val = self.pop();
    let n = match &val {
        Value::Array(a)  => a.len() as i64,
        Value::String(s) => s.chars().count() as i64,
        _ => return Err(format!("len: expected Array or String, got {}", val.type_name())),
    };
    self.push(Value::Integer(n))?;
    Ok(1)
}
```

**What does NOT change:**

- Higher-order use of these builtins (`map(xs, len)`) still uses `OpGetBuiltin`.
- Builtins requiring GC or `RuntimeContext` are unaffected.

**Risk:** Low-medium. New opcodes require updating disassembler, snapshot tests, and any
tooling that inspects bytecode.

---

### Phase 3 — `OpCallBuiltin` Superinstruction (Catch-All)

**Effort:** Medium. New opcode + compiler + VM.

For all remaining builtins that cannot be true primops (need `RuntimeContext`, GC
interaction, complex logic), add a superinstruction that fuses `OpGetBuiltin + OpCall`
into a single dispatch.

```
OpCallBuiltin(idx, arity)   ; 3 bytes total
```

This is the BEAM BIF level — still a Rust function call, but eliminates:
- The second `get_builtin_by_index` lookup
- The `Value::Builtin` stack push/pop
- The type-dispatch branch in `OpCall`
- The `builtin_fixed_arity` string comparison

**Candidates:** `map`, `filter`, `fold`, `flat_map`, `push`, `range`, `sum`, `product`,
`sort`, `reverse`, `contains`, `slice`, `put`, `get`, `has_key`, `keys`, `values`,
`merge`, `delete`, `list`, `to_list`, `to_array`, `hd`, `tl`, `concat`, `split`, `join`,
`trim`, `upper`, `lower`, `starts_with`, `ends_with`, `replace`, `chars`, `substring`,
`print`, `parse_int`, `parse_ints`, `split_ints`, `read_file`, `read_lines`, `read_stdin`,
`now_ms`, `time`, `zip`, `flatten`, `any`, `all`, `find`, `sort_by`, `count`, `type_of`,
`is_list`, `is_hash`, `is_map`.

**Implementation — `src/bytecode/op_code.rs`:**

```rust
OpCallBuiltin = N,   // operands: [u8 idx, u8 arity]
```
```rust
OpCode::OpCallBuiltin => vec![1, 1],
```

**Implementation — `src/runtime/vm/dispatch.rs`:**

```rust
OpCode::OpCallBuiltin => {
    let idx      = Self::read_u8_fast(instructions, ip + 1) as usize;
    let num_args = Self::read_u8_fast(instructions, ip + 2) as usize;
    let builtin  = &BUILTINS[idx];
    let args: Vec<Value> = (0..num_args)
        .map(|_| self.pop())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let result = (builtin.func)(self, args)?;
    self.push(result)?;
    Ok(3)
}
```

**Risk:** Low. The existing `OpGetBuiltin + OpCall` path remains intact as fallback for
higher-order use.

---

## What NOT to Primop

These builtins should remain on the generic path regardless of phase:

| Category | Builtins | Reason |
|---|---|---|
| I/O bound | `print`, `read_file`, `read_lines`, `read_stdin` | I/O latency dominates; dispatch overhead is noise |
| GC-heavy | `list`, `put`, `get`, `merge`, `to_list` | Complexity is in GC allocation, not dispatch |
| Higher-order callbacks | `map`, `filter`, `fold`, `flat_map`, `any`, `all`, `find`, `sort_by` | The callback invocation is the bottleneck |

Note: even for higher-order builtins, `OpCallBuiltin` (Phase 3) still saves the outer
dispatch overhead — just not the inner callback overhead.

---

## JIT Backend

The three phases described here are **VM-only**. The JIT compiles AST directly to
Cranelift IR and does not use opcodes.

The JIT already detects direct builtin calls at the AST level
(`src/jit/compiler.rs:1646`) and dispatches via `rt_call_builtin` — a native C extern.
The JIT equivalent of this proposal would be inlining simple operations as Cranelift IR
(struct field loads, arithmetic), avoiding the `rt_call_builtin` call entirely. That is a
separate, more complex effort and is out of scope here.

---

## Impact Summary

| Phase | Builtins affected | Overhead eliminated | Effort |
|---|---|---|---|
| 1 — Wire existing opcodes | `hd`, `tl`, `to_string`, `is_some` | Double lookup, stack push/pop, type dispatch | Low |
| 2 — True primops | `len`, `abs`, `min`, `max`, type checks (10 builtins) | Rust fn call + `Vec<Value>` alloc + all dispatch | Medium |
| 3 — `OpCallBuiltin` | ~49 remaining builtins | Double lookup, stack push/pop, type dispatch | Medium |

---

## Files to Change

| File | Phases |
|---|---|
| `src/bytecode/op_code.rs` | 2, 3 |
| `src/bytecode/compiler/expression.rs` | 1, 2, 3 |
| `src/runtime/vm/dispatch.rs` | 2, 3 |
| `src/runtime/builtins/mod.rs` | No change required |
| `src/bytecode/symbol_table.rs` | No change required |
| Snapshot tests | 2, 3 (new opcodes change disassembly output) |

---

## Recommended Order

Start with **Phase 1**. It is a pure compiler change, touches one file, carries zero VM
risk, requires no new opcodes, and immediately corrects the inconsistency where `hd(x)`
does not use `OpConsHead` despite the opcode existing for that purpose.

Benchmark after Phase 1, then decide whether Phase 2 or Phase 3 yields better returns for
the programs in `examples/`.

---

## References

- `src/bytecode/op_code.rs` — opcode definitions
- `src/bytecode/compiler/expression.rs` — call compilation
- `src/bytecode/compiler/builder.rs` — `load_symbol` / `SymbolScope::Builtin`
- `src/runtime/vm/dispatch.rs` — VM dispatch loop
- `src/runtime/vm/function_call.rs` — builtin call handling
- `src/runtime/builtins/mod.rs` — BUILTINS array
- `src/jit/compiler.rs` — JIT builtin detection
- `src/jit/runtime_helpers.rs` — `rt_call_builtin`
