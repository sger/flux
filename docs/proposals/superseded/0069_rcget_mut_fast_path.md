- Feature Name: Emit Rc::get_mut Fast Path for Unique Match Sites
- Start Date: 2026-03-01
- Status: Superseded by 0084 and 0114
- Proposal PR: pending
- Flux Issue: pending

# Proposal 0069: Emit Rc::get_mut Fast Path for Unique Match Sites

## Summary
[summary]: #summary

Status note:
This proposal was not implemented as written. The current compiler uses Aether
Core reuse nodes, verifier checks, runtime helpers, and backend lowering from
proposal 0084 instead of the specific opcode and token design described here.

Use the ownership annotations from proposal 0068 to emit `Rc::get_mut()` in-place reuse
at match and reconstruction sites where the input value is annotated as `Unique` or
`Unknown` (runtime check). This makes purely functional transformations over arrays and
tuples as fast as imperative in-place mutation when the input is no longer needed.

## Motivation
[motivation]: #motivation

Every functional transformation over an `Array` currently allocates a new `Vec`:

```flux
fn increment_all(xs: Array<Int>) -> Array<Int> {
    map(xs, \x -> x + 1)
}
```

The `map` base function allocates a new `Rc<Vec<Value>>` and clones each element. If
`xs` is the last reference to its `Vec` (i.e., `Rc::strong_count == 1`), the old `Vec`
is freed immediately after. This is:

```
alloc new Vec  →  copy n elements  →  free old Vec
```

With Perceus reuse:

```
check Rc::strong_count == 1  →  mutate in place  →  done
```

For large arrays, this eliminates one allocation and one deallocation per operation, and
removes the O(n) element copy. The transformation is semantically identical — the program
produces the same output.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### No changes to Flux surface syntax

This is a compiler optimization. No syntax changes. No new flags in default mode. The
`--perceus` flag (introduced in proposal 0068) enables this pass.

### Observable behavior changes

1. **Performance**: programs that transform arrays in a chain where each step is the last
   use of the previous result will allocate O(1) instead of O(n) intermediate arrays.
2. **Memory usage**: peak memory usage decreases for such programs.
3. **No semantic change**: program output is identical.

### Performance target

For a `map` over a 1M-element array where the input is uniquely owned, the expected
speedup is 2–4× due to:
- Eliminating one `Vec` allocation and deallocation.
- Improving cache behavior (in-place vs new allocation).
- Eliminating the element-by-element clone path.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### New OpCode: `OpReuseArray` and `OpReuseCheck`

Two new bytecode instructions are introduced:

```rust
// src/bytecode/opcode.rs

pub enum OpCode {
    // ... existing opcodes ...

    /// OpReuseCheck
    /// Stack: [..., value]
    /// Pops the top value. If it is an Array with Rc::strong_count == 1,
    /// pushes Value::ReuseToken(the Rc pointer) onto the stack.
    /// Otherwise, pushes Value::None (no reuse possible).
    OpReuseCheck = 0xE0,

    /// OpReuseArray(n)
    /// Precondition: stack top is a Value::ReuseToken.
    /// Reads n new element values from the stack, writes them into the reused Vec,
    /// and pushes the updated Array. If the token is None (no reuse), allocates fresh.
    OpReuseArray = 0xE1,
}
```

And a new `Value` variant to carry the reuse token through the stack:

```rust
// src/runtime/value.rs

pub enum Value {
    // ... existing variants ...

    /// Internal: carries a uniquely-owned Rc<Vec<Value>> for in-place reuse.
    /// Never visible to Flux programs; only lives on the VM stack between
    /// OpReuseCheck and OpReuseArray.
    ReuseToken(Rc<Vec<Value>>),
}
```

### VM execution (src/runtime/vm/dispatch.rs)

```rust
OpCode::OpReuseCheck => {
    let val = self.pop()?;
    let token = match &val {
        Value::Array(arr) if Rc::strong_count(arr) == 1 => {
            // Unique! Extract the inner Vec for reuse.
            // Safety: strong_count == 1 means we have the only reference.
            // Rc::try_unwrap will succeed.
            match Rc::try_unwrap(
                if let Value::Array(a) = val { a } else { unreachable!() }
            ) {
                Ok(vec) => Value::ReuseToken(Rc::new(vec)),
                Err(rc) => Value::None,  // Shouldn't happen; fallback
            }
        }
        Value::Array(_) => Value::None,  // Shared; cannot reuse
        _ => Value::None,               // Not an array
    };
    self.push(token)?;
    Ok(1)
}

OpCode::OpReuseArray => {
    let n = Self::read_u8_fast(instructions, ip + 1) as usize;
    let token = self.pop()?;

    match token {
        Value::ReuseToken(reuse_rc) => {
            // Get unique access to the Vec.
            let mut vec = Rc::try_unwrap(reuse_rc)
                .expect("ReuseToken should have unique Rc");

            // Pop n new values from stack (in reverse order, last-pushed first)
            let mut new_elems: Vec<Value> = (0..n)
                .map(|_| self.pop().expect("stack underflow in OpReuseArray"))
                .collect();
            new_elems.reverse();

            // Write new values into the existing Vec in-place
            if vec.len() == n {
                // Same length: reuse element slots
                for (slot, new_val) in vec.iter_mut().zip(new_elems.into_iter()) {
                    *slot = new_val;
                }
            } else {
                // Different length: clear and refill (still reuses the Vec's allocation
                // if capacity is sufficient)
                vec.clear();
                vec.extend(new_elems);
            }

            self.push(Value::Array(Rc::new(vec)))?;
        }
        Value::None => {
            // No reuse possible: allocate fresh array
            let mut new_elems: Vec<Value> = (0..n)
                .map(|_| self.pop().expect("stack underflow in OpReuseArray fallback"))
                .collect();
            new_elems.reverse();
            self.push(Value::Array(Rc::new(new_elems)))?;
        }
        other => {
            return Err(format!("OpReuseArray: expected ReuseToken or None, got {:?}", other));
        }
    }
    Ok(2)  // opcode + n_byte
}
```

### Compiler emission (src/bytecode/compiler/expression.rs)

When compiling an array reconstruction at a match site where the input is annotated
`Unique` or `Unknown`:

```rust
// In compile_array_constructor(), called when the ownership analysis suggests reuse:

fn compile_array_with_reuse(
    &mut self,
    source_expr: &Expression,     // the value being matched/transformed
    new_elements: &[Expression],  // the new element expressions
) -> CompileResult<()> {
    let ownership = self.ownership_map
        .get(&(source_expr as *const Expression))
        .copied()
        .unwrap_or(Ownership::Unknown);

    match ownership {
        Ownership::Unique => {
            // Statically unique: emit OpReuseCheck (will always succeed at runtime)
            self.compile_expression(source_expr)?;
            self.emit_op(OpCode::OpReuseCheck);

            for elem in new_elements {
                self.compile_expression(elem)?;
            }
            self.emit(OpCode::OpReuseArray, &[new_elements.len()]);
        }
        Ownership::Unknown => {
            // May or may not be unique: emit runtime check
            self.compile_expression(source_expr)?;
            self.emit_op(OpCode::OpReuseCheck);  // pushes ReuseToken or None

            for elem in new_elements {
                self.compile_expression(elem)?;
            }
            self.emit(OpCode::OpReuseArray, &[new_elements.len()]);
            // The runtime check inside OpReuseArray handles both cases
        }
        Ownership::Shared => {
            // Definitely shared: skip reuse, emit normal array construction
            for elem in new_elements {
                self.compile_expression(elem)?;
            }
            self.emit(OpCode::OpMakeArray, &[new_elements.len()]);
        }
    }
    Ok(())
}
```

### The `map` base function with Perceus fast path

The most important target is `map` over arrays. With the in-place reuse path:

```rust
// src/runtime/base/higher_order_ops.rs

pub fn base_map(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    match (args.get(0), args.get(1)) {
        (Some(Value::Array(arr)), Some(func)) => {
            // Perceus fast path: if we have unique ownership, mutate in-place
            if Rc::strong_count(arr) == 1 {
                // SAFETY: strong_count == 1 guarantees we have the only reference.
                // We can safely get mutable access.
                let arr_rc = match args.into_iter().next().unwrap() {
                    Value::Array(a) => a,
                    _ => unreachable!(),
                };
                if let Ok(mut vec) = Rc::try_unwrap(arr_rc) {
                    for elem in vec.iter_mut() {
                        *elem = call_function(ctx, func.clone(), vec![elem.clone()])?;
                    }
                    return Ok(Value::Array(Rc::new(vec)));
                }
            }

            // Fallback: allocate new Vec (Shared or failed try_unwrap)
            let arr = match &args[0] { Value::Array(a) => a, _ => unreachable!() };
            let func = &args[1];
            let result: Result<Vec<Value>, String> = arr.iter()
                .map(|elem| call_function(ctx, func.clone(), vec![elem.clone()]))
                .collect();
            Ok(Value::Array(Rc::new(result?)))
        }
        _ => Err("map: expected (Array, Function)".to_string()),
    }
}
```

### JIT integration

The JIT backend calls `map` via `rt_call_base_function`. The Perceus fast path is already
in the base function implementation, so the JIT benefits automatically. No JIT-specific
changes are required.

### Test fixtures

```flux
-- tests/testdata/perceus/map_reuse.flx
-- Validates that map over a uniquely-owned array does not allocate

fn main() with IO {
    let xs = [|1, 2, 3, 4, 5|]
    -- xs is Unique here (just constructed, not aliased)
    let ys = map(xs, \x -> x + 1)
    -- Expected: [2, 3, 4, 5, 6]
    -- With --perceus: in-place mutation (no new Vec allocation)
    print(ys)
}
```

```flux
-- tests/testdata/perceus/map_shared.flx
-- Validates that shared arrays are NOT mutated in-place

fn main() with IO {
    let xs = [|1, 2, 3|]
    let ys = map(xs, \x -> x + 1)   -- xs is Shared (used below)
    let zs = map(xs, \x -> x * 2)   -- xs must still be [1, 2, 3]
    print(ys)   -- [2, 3, 4]
    print(zs)   -- [2, 4, 6]
    print(xs)   -- [1, 2, 3] -- must be unchanged
}
```

### Validation commands

```bash
# Run with Perceus enabled
cargo run -- --perceus --no-cache examples/perceus/map_reuse.flx

# Verify shared array correctness
cargo run -- --perceus --no-cache tests/testdata/perceus/map_shared.flx

# Benchmark: compare with and without Perceus
cargo bench --bench perceus_bench
```

## Drawbacks
[drawbacks]: #drawbacks

- Two new opcodes (`OpReuseCheck`, `OpReuseArray`) increase bytecode format surface area.
  These opcodes are stable once introduced (never reorder or reuse discriminants).
- `Value::ReuseToken` is a new internal variant that must not leak to user programs.
  Pattern matches on `Value` throughout the codebase must handle or reject this variant.
- The `Rc::try_unwrap` operation can theoretically fail even after `strong_count == 1`
  in a concurrent setting. Since Flux is single-threaded per actor, this is not an issue.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

**Why not just check `Rc::strong_count` everywhere?** The check is cheap (~1ns) but
adds a branch to every array operation. The analysis from 0068 eliminates the check for
statically `Unique` values, keeping them on the hot path.

**Why `OpReuseCheck` + `OpReuseArray` instead of a single instruction?** The separation
allows the compiler to interleave new element computation between the check and the reuse.
A single instruction would require pre-computing all elements before checking ownership.

**Why not integrate directly into base functions without new opcodes?** The base function
approach (shown for `map`) works for fixed-pattern operations. The opcode approach is
needed for user-defined reconstruction patterns (`match xs { ... -> [|new_elems|] }`).
Both are implemented: base functions use the runtime check; match sites use the opcodes.

## Prior art
[prior-art]: #prior-art

- **Perceus paper** (Reinking et al., 2021) — formal specification of reuse credits
  and the `reuse` token concept this proposal implements.
- **Koka compiler** — Koka's core IR has an explicit `reuse` binder that is inserted
  by the Perceus pass and consumed by constructors. This proposal's `ReuseToken` is
  the bytecode-level equivalent.
- **GHC's arity analysis** — a related technique for detecting when a thunk will be
  evaluated exactly once and can be evaluated eagerly (different domain, same idea).

## Unresolved questions
[unresolved-questions]: #unresolved-questions

1. Should `OpReuseCheck` also handle `Tuple` values, not just `Array`? Tuples are fixed
   size, which simplifies the in-place write. Decision: yes, add `OpReuseCheck` support
   for tuples in the same implementation pass.
2. Should the reuse optimization apply to `Value::String` (for string transformation
   operations)? Strings are `Rc<str>`, which is immutable. Reuse is not directly
   applicable unless we intern mutable string buffers. Deferred.

## Future possibilities
[future-possibilities]: #future-possibilities

- **Reuse for cons lists**: when `Value::Gc` is replaced by Perceus-managed cons cells
  (proposal 0070), the same reuse analysis applies to list operations (`map`, `filter`,
  `fold` over lists).
- **Reuse tokens in the JIT**: Cranelift JIT can emit the `Rc::strong_count` check
  inline as a conditional move, eliminating the indirect call to the base function.
- **Compile-time uniqueness contract**: `unique` keyword in function signatures to
  guarantee callers pass owned values, enabling the `Unique` fast path without a runtime
  check.
