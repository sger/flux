- Feature Name: Total Functions and Safe Arithmetic
- Start Date: 2026-03-25
- Proposal PR:
- Flux Issue:

## Summary

Eliminate runtime panics from Flux programs by making all built-in operations total (always return a value, never crash). Division and modulo return `Option<Int>` instead of panicking on zero. Pattern matching is already exhaustive. The long-term path adds refinement types for zero-cost safety where the compiler proves invariants statically.

## Motivation

### The problem

Flux is a pure functional language. Pure functions should be **total** — they must return a value for every valid input. Yet several core operations panic at runtime:

```flux
fn main() with IO {
    print(42 / 0)    // panic: Division by zero
    print(42 % 0)    // panic: Division by zero
}
```

This violates the language's purity guarantee. A function that panics is not pure — it has an observable side effect (process termination) that isn't declared in its type.

### Why this matters

1. **Correctness**: If `div` can panic, every function that uses division is potentially partial. The type `Int -> Int -> Int` lies — it should be `Int -> Int -> Option<Int>` or require a proof that the divisor is non-zero.

2. **Backend parity**: The VM produces a rich diagnostic (`error[E1008]: Division By Zero` with source span and stack trace). The native backend now produces `panic: Division by zero` with a shadow stack trace. But both crash the process. A total function would let the programmer handle the error.

3. **Composability**: Panics don't compose. You can't `map(list, \x -> x / y)` safely if `y` might be zero. With `Option`, you get `map(list, \x -> safe_div(x, y))` which returns `Array<Option<Int>>` — the caller decides what to do.

### Current runtime error inventory

| Operation | Error | Current behavior | Proposed behavior |
|-----------|-------|-----------------|-------------------|
| `x / 0` | E1008 | Panic | Return `None` |
| `x % 0` | E1008 | Panic | Return `None` |
| `arr[i]` (out of bounds) | — | Returns `None` | Already safe (returns `Option`) |
| Non-exhaustive match | E015 | Compile error | Already safe |
| Type mismatch | E1004 | Runtime error | Already caught by HM for typed code |
| Wrong arity | E1000 | Runtime error | Already caught by HM |
| parse_int failure | E1009 | Panic (VM) / returns int (native) | Type says `Option<Int>`, should match |

Division and modulo are the only arithmetic operations that can fail on valid-typed inputs. All other runtime errors are either already caught at compile time or involve untyped/dynamic code.

## Guide-level explanation

### Phase 1: Safe division functions (non-breaking)

Add safe alternatives alongside the existing operators:

```flux
// New safe functions (always total)
fn safe_div(a: Int, b: Int) -> Option<Int> {
    if b == 0 { None } else { Some(a / b) }
}

fn safe_mod(a: Int, b: Int) -> Option<Int> {
    if b == 0 { None } else { Some(a % b) }
}

// Usage
fn average(total: Int, count: Int) -> Option<Int> {
    safe_div(total, count)
}

fn main() with IO {
    match average(100, 0) {
        Some(avg) -> print("Average: " + to_string(avg)),
        None -> print("Cannot compute average of zero items"),
    }
}
```

The existing `/` and `%` operators continue to panic on zero (for backward compatibility). Users migrate at their own pace.

### Phase 2: Checked operators (edition change)

In a future edition, `/` and `%` return `Option`:

```flux
// Future edition: / returns Option<Int>
let result = 42 / x    // result : Option<Int>

// Use `unwrap` or match to extract
let value = match 42 / x {
    Some(v) -> v,
    None -> 0,          // provide default
}

// Or use `divides` for the common case where you know it's safe
let half = 100 /! 2    // /! is unchecked division (panics on zero)
```

### Phase 3: Refinement types (long-term)

The ultimate solution — the compiler proves safety statically:

```flux
// Refinement type: b is proven non-zero at compile time
fn div(a: Int, b: {Int | b != 0}) -> Int {
    a / b    // guaranteed safe — compiler verified b != 0
}

// Caller must prove the invariant
fn half(x: Int) -> Int {
    div(x, 2)    // OK: 2 != 0 is trivially true
}

fn ratio(a: Int, b: Int) -> Int {
    div(a, b)    // ERROR: cannot prove b != 0
}

fn safe_ratio(a: Int, b: Int) -> Option<Int> {
    if b == 0 { None }
    else { Some(div(a, b)) }    // OK: guard proves b != 0
}
```

With refinement types, there is zero runtime overhead — no `Option` wrapping, no runtime check. The compiler statically eliminates the error case.

## Reference-level explanation

### Phase 1: Safe functions (implementation)

Add `safe_div` and `safe_mod` to the Base function registry:

```rust
// In src/runtime/base/math_ops.rs
pub fn base_safe_div(args: &[&Value]) -> Result<Value, String> {
    let a = arg_int(args, 0)?;
    let b = arg_int(args, 1)?;
    if b == 0 {
        Ok(Value::None)
    } else {
        Ok(Value::some(Value::Integer(a / b)))
    }
}
```

HM signature:
```
safe_div : (Int, Int) -> Option<Int>
safe_mod : (Int, Int) -> Option<Int>
```

C runtime (for native backend):
```c
int64_t flux_safe_div(int64_t a, int64_t b) {
    int64_t rb = flux_untag_int(b);
    if (rb == 0) return flux_make_none();
    return flux_wrap_some(flux_tag_int(flux_untag_int(a) / rb));
}
```

### Phase 2: NonZero type (intermediate step)

Before full refinement types, a simpler `NonZero` wrapper provides type-level safety:

```flux
data NonZero {
    NonZero(Int)    // invariant: inner value != 0
}

// Smart constructor (the only way to create NonZero)
fn non_zero(x: Int) -> Option<NonZero> {
    if x == 0 { None } else { Some(NonZero(x)) }
}

// Safe division — guaranteed total by construction
fn div(a: Int, b: NonZero) -> Int {
    match b {
        NonZero(bv) -> a / bv    // safe: bv != 0 by construction
    }
}
```

This is the Haskell `newtype` pattern — no runtime overhead (same representation as `Int`), but the type system enforces the invariant.

### Phase 3: Refinement types (design sketch)

Refinement types extend the type system with logical predicates:

```
τ ::= {x : T | φ}     -- refined type
φ ::= φ ∧ φ | φ ∨ φ | ¬φ | x op y | true | false
op ::= == | != | < | <= | > | >=
```

Type checking with refinements requires an SMT solver (like Z3) or a simpler decision procedure for linear arithmetic:

```
Γ ⊢ e : {x : Int | x != 0}
────────────────────────────
Γ ⊢ div(a, e) : Int
```

The compiler generates verification conditions and checks them at compile time. If it can't prove safety, it reports an error with a counterexample.

**Subtyping**: `{x : Int | x > 0}` is a subtype of `{x : Int | x != 0}`, which is a subtype of `Int`. This means refined types are backward-compatible — any `Int` can be used where `Int` is expected, but not where `{Int | x != 0}` is expected without proof.

## Implementation phases

**Phase 1 — Safe functions** (~2 days)
- Add `safe_div`, `safe_mod` to Base function registry
- Add HM signatures
- Add C runtime implementations
- Add to LLVM builtin mappings
- Tests for both VM and native

**Phase 2 — NonZero type** (~1 week)
- Standard library `NonZero` type with smart constructor
- Compiler support for `newtype`-style zero-cost wrappers
- Integration with division operators
- Documentation and migration guide

**Phase 3 — Checked operators** (~2 weeks, edition change)
- `/` and `%` return `Option<Int>` for integers
- `/!` and `%!` unchecked variants (panic on zero)
- Float division unchanged (returns Inf/NaN per IEEE 754)
- Edition flag to opt in/out

**Phase 4 — Refinement types** (~months, research)
- Syntax for type predicates
- Verification condition generation
- Decision procedure for linear arithmetic
- Integration with pattern match exhaustiveness
- Error messages with counterexamples

## Drawbacks

- **Ergonomic cost**: `safe_div(a, b)` returns `Option<Int>` which must be unwrapped. This adds verbosity compared to `a / b`. Mitigated by `match`, pipeline operators, and eventually refinement types.

- **Breaking change (Phase 3)**: Changing `/` to return `Option` breaks all existing code. Must be gated behind an edition flag with a long migration period.

- **Complexity (Phase 4)**: Refinement types are a significant type system extension. They require either an embedded SMT solver or a custom decision procedure. This is a research-level feature.

- **Performance**: Safe division adds a branch. In practice the branch is nearly free (branch predictor handles it), and refinement types eliminate it entirely.

## Prior art

- **Haskell**: `div` is partial (throws on zero). `safe-exceptions` library provides total alternatives. Liquid Haskell adds refinement types with SMT-backed verification.

- **Rust**: Division panics on zero. The `checked_div` method returns `Option<T>`. `NonZeroU32` etc. provide type-level non-zero guarantees.

- **Idris 2**: Full dependent types. Division requires a proof of non-zeroness: `div : Int -> (b : Int) -> {auto prf : NonZero b} -> Int`.

- **F***: Refinement types with SMT-backed verification. `val div : int -> b:int{b <> 0} -> int`.

- **Lean 4**: Dependent types. Division returns 0 for `x / 0` (total by convention, not by proof). `Nat.div_def` provides the specification.

- **Elm**: No runtime exceptions. All operations are total. Division by zero returns 0 (controversial but pragmatic).

## Unresolved questions

- **Float division**: Should `1.0 / 0.0` return `Inf` (IEEE 754) or `None`? IEEE compliance says `Inf`, but that's still a special value. Recommendation: keep IEEE behavior for floats, only change integer division.

- **Operator overloading**: If `/` returns `Option<Int>` for integers, what about user-defined `/` via typeclasses (future)? The typeclass should define the return type.

- **Lean's approach**: Lean makes `n / 0 = 0` — total but arguably wrong. Is this better than `Option`? It avoids unwrapping but silently produces incorrect results. Recommendation: `Option` is more honest.

- **Migration path**: How long should the transition period be? Recommendation: Phase 1 (safe functions) ships immediately, Phase 3 (operator change) ships in the next edition with at least 6 months warning.

## Future possibilities

- **Refinement types for arrays**: `fn get(arr: Array<a>, i: {Int | 0 <= i && i < len(arr)}) -> a` — eliminates bounds checking.
- **Non-empty collections**: `NonEmpty<List<a>>` guarantees `head` and `tail` are total.
- **Positive integers**: `{Int | x > 0}` for array lengths, counts, etc.
- **String patterns**: `{String | matches(s, regex)}` for validated input.
- **Effect refinements**: Prove that an effectful computation always succeeds, eliminating the need for error handling in the caller.
