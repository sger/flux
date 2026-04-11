- Feature Name: NaN-boxing Runtime Optimization
- Start Date: 2026-02-23
- Updated: 2026-03-17
- Status: Implemented (v0.0.5)
- Proposal PR:
- Flux Issue:

# Proposal 0041: NaN-boxing Runtime Optimization

## Summary

Replace the 24-byte `Value` enum with a 8-byte NaN-boxed representation that encodes
integers, floats, booleans, None, and small tags as immediate 64-bit values, and stores
heap pointers (strings, arrays, closures, ADTs, etc.) in the NaN payload bits.

## Motivation

### Current State

Flux's `Value` enum is 24 bytes on 64-bit platforms:

```rust
pub enum Value {
    Integer(i64),          // 8 bytes payload
    Float(f64),            // 8 bytes payload
    Boolean(bool),         // 1 byte payload
    String(Rc<str>),       // 16 bytes payload (fat pointer!)
    None,                  // 0 bytes
    BaseFunction(u8),      // 1 byte payload
    Gc(GcHandle),          // 4 bytes payload (u32)
    Adt(Rc<AdtValue>),     // 8 bytes payload
    Closure(Rc<Closure>),  // 8 bytes payload
    // ... 25 variants total
}
// size_of::<Value>() == 24 (due to Rc<str> fat pointer + discriminant + alignment)
```

Every value on the VM stack, in every array, in every closure capture, and in every ADT
field occupies 24 bytes. This means:

- **Stack**: VM stack of 1024 slots = 24 KB (vs 8 KB with NaN boxing)
- **Arrays**: `[1, 2, 3, ..., 1000]` = 24 KB (vs 8 KB)
- **Cache pressure**: 3x more memory traffic on numeric-heavy workloads
- **GC pressure**: Rc allocations for every Some/Left/Right wrapper

### Why Now

With HM type inference (v0.0.3+) and the Core IR optimization framework (v0.0.4), Flux
now has the type information needed to make NaN boxing safe. The compiler knows whether a
value is Int, Float, Bool, or a heap type — this information can guide boxing decisions
and catch representation bugs at compile time.

## Guide-level explanation

### What is NaN Boxing?

IEEE 754 doubles use a special bit pattern for NaN (Not-a-Number). There are ~2^51
distinct NaN bit patterns, but only one is needed for actual NaN. The rest are "quiet NaNs"
that hardware ignores — we can store arbitrary data in those bits.

```
IEEE 754 double (64 bits):
┌─────┬────────────┬──────────────────────────────────────────────────┐
│sign │ exponent   │ mantissa                                        │
│ 1   │ 11 bits    │ 52 bits                                         │
└─────┴────────────┴──────────────────────────────────────────────────┘

NaN: exponent = all 1s, mantissa ≠ 0
Quiet NaN: exponent = all 1s, bit 51 = 1, remaining 51 bits = free storage
```

We get **51 bits of free payload** inside quiet NaN values. On 64-bit systems, user-space
pointers only use 48 bits, so heap pointers fit with room for a type tag.

### Encoding Scheme

```
Float:    any bit pattern that is NOT a NaN-box (passes as raw f64)
          (canonical NaN = 0x7FF8_0000_0000_0000 is reserved for actual NaN)

NaN-box:  0x7FFC_XXXX_XXXX_XXXX  (bits 51-50 = 11, signals "this is a NaN-box")
          ├── tag (4 bits, bits 49-46) ── selects value kind
          └── payload (46 bits) ──────── holds integer/pointer/tag data

Tag assignments (4 bits = 16 kinds):
  0x0  Integer      payload: 46-bit signed integer (range: ±35 trillion)
  0x1  Boolean      payload: 0 = false, 1 = true
  0x2  None         payload: 0
  0x3  Uninit       payload: 0
  0x4  EmptyList    payload: 0
  0x5  BaseFunction payload: function index (u8)
  0x6  GcHandle     payload: heap slot index (u32)
  0x7  GcAdt        payload: heap slot index (u32)
  0x8  Pointer      payload: 46-bit heap pointer (Rc<T> thin pointer)
  0x9  FatPointer   payload: index into fat-pointer table (for Rc<str>)
  0xA  Continuation payload: heap pointer
  0xB  ReturnValue  payload: index into return-value slab
  0xC  reserved
  0xD  reserved
  0xE  reserved
  0xF  BigInt       payload: index into big-integer slab (overflow from 46-bit)
```

### Integer Representation

The **key design decision**: integers are 46-bit, not 64-bit.

Most integers in Flux programs are small (loop counters, array indices, ADT tags). The
46-bit range covers ±34,359,738,367,999 which is sufficient for almost all programs.

For the rare case of full 64-bit integers, values outside the 46-bit range are stored in
a **BigInt slab** — a side table of `Vec<i64>` indexed by the NaN-box payload. This
preserves full i64 semantics with zero cost for small integers.

```rust
fn box_integer(n: i64) -> NanBox {
    if n >= MIN_INLINE_INT && n <= MAX_INLINE_INT {
        NanBox::inline_int(n)     // fast path: single u64
    } else {
        NanBox::slab_int(n)       // slow path: store in slab, return index
    }
}
```

**Alternative: full 64-bit integers via double encoding.** Instead of a BigInt slab, we
could encode all integers as f64 (which can exactly represent integers up to 2^53). This
is what JavaScript/LuaJIT do. However, this changes integer semantics (wrapping arithmetic
behaves differently) and Flux guarantees i64, so the slab approach preserves correctness.

### Pointer Encoding

On x86-64 and ARM64, user-space virtual addresses use at most 48 bits, and the top bit
is always 0 in user space. So we have 47 significant bits — but we only have 46 bits of
payload.

**Solution**: all Rc allocations are 8-byte aligned (guaranteed by Rust's allocator), so
the bottom 3 bits are always 0. We shift right by 3, giving us 44 significant bits in
46 bits of space. This covers the full 48-bit address space.

```rust
fn box_pointer(tag: u8, ptr: *const ()) -> NanBox {
    let shifted = (ptr as u64) >> 3;
    debug_assert!(shifted < (1 << 46));
    NanBox::from_tag_payload(tag, shifted)
}

fn unbox_pointer(nb: NanBox) -> *const () {
    (nb.payload() << 3) as *const ()
}
```

### Fat Pointer Problem

`Rc<str>` is a **fat pointer** (16 bytes: data pointer + length). It cannot fit in 46
bits. Two options:

**Option A: Fat pointer table** — store fat pointers in a side table, encode the table
index in the NaN box. Adds one indirection for string access.

**Option B: Rc\<String\> instead of Rc\<str\>** — change string representation to use a
thin pointer (`Rc<String>` = 8 bytes). Adds one indirection for string data but
simplifies NaN boxing. Cloning is still O(1) via Rc.

**Recommendation: Option B.** It's simpler, the extra indirection is negligible compared
to the cache wins from 8-byte values everywhere, and it aligns with how most languages
store strings (pointer to heap-allocated string object).

## Reference-level explanation

### NanBox Type

```rust
/// A NaN-boxed runtime value. Exactly 8 bytes.
///
/// Invariant: all Values are representable as NanBox and round-trip losslessly.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct NanBox(u64);

// Bit layout constants
const NANBOX_MASK:     u64 = 0x7FFC_0000_0000_0000; // bits 62-50 = NaN + quiet + box flag
const TAG_SHIFT:       u32 = 46;
const TAG_MASK:        u64 = 0xF;                    // 4-bit tag
const PAYLOAD_MASK:    u64 = (1u64 << 46) - 1;       // 46-bit payload
const MIN_INLINE_INT:  i64 = -(1i64 << 45);          // -35_184_372_088_832
const MAX_INLINE_INT:  i64 =  (1i64 << 45) - 1;      //  35_184_372_088_831

impl NanBox {
    /// Check if this is a float (not a NaN-box).
    #[inline(always)]
    pub fn is_float(self) -> bool {
        (self.0 & NANBOX_MASK) != NANBOX_MASK
    }

    /// Extract the 4-bit tag (panics if is_float).
    #[inline(always)]
    pub fn tag(self) -> u8 {
        debug_assert!(!self.is_float());
        ((self.0 >> TAG_SHIFT) & TAG_MASK) as u8
    }

    /// Extract the 46-bit payload.
    #[inline(always)]
    pub fn payload(self) -> u64 {
        self.0 & PAYLOAD_MASK
    }

    /// Box a float. Canonical NaN is preserved.
    #[inline(always)]
    pub fn from_float(f: f64) -> Self {
        NanBox(f.to_bits())
    }

    /// Box an inline integer (must be in 46-bit range).
    #[inline(always)]
    pub fn from_inline_int(n: i64) -> Self {
        debug_assert!(n >= MIN_INLINE_INT && n <= MAX_INLINE_INT);
        let payload = (n as u64) & PAYLOAD_MASK;
        NanBox(NANBOX_MASK | ((TAG_INT as u64) << TAG_SHIFT) | payload)
    }
}
```

### VM Stack Changes

```rust
// Before (24 bytes per slot):
pub struct Vm {
    stack: Vec<Value>,  // 24 bytes × stack_size
}

// After (8 bytes per slot):
pub struct Vm {
    stack: Vec<NanBox>,  // 8 bytes × stack_size
}
```

The dispatch loop changes from matching on `Value` variants to checking NaN-box tags:

```rust
// Before:
match stack.pop() {
    Value::Integer(a) => { ... }
    Value::Float(a) => { ... }
    _ => type_error!()
}

// After:
let v = stack.pop();
if v.is_float() {
    let a = v.as_float();
    ...
} else if v.tag() == TAG_INT {
    let a = v.as_int();
    ...
} else {
    type_error!()
}
```

### Interaction with Existing Systems

#### GC (runtime/gc/)

`GcHandle(u32)` already fits in 46 bits — no change needed. The GC's mark-and-sweep
algorithm traces from roots; roots change from `Vec<Value>` to `Vec<NanBox>`, but the
GC only needs to scan for GcHandle/GcAdt tags (tags 0x6, 0x7) and pointer tags (0x8+).

```rust
impl NanBox {
    fn is_gc_traceable(self) -> bool {
        !self.is_float() && self.tag() >= TAG_GC_HANDLE
    }
}
```

#### Rc Reference Counting

Values behind `Rc<T>` (String, Array, Closure, Adt, etc.) still use reference counting.
NaN boxing doesn't change ownership — `NanBox::clone()` must increment the Rc when the
value is a pointer tag. This requires careful implementation:

```rust
impl Clone for NanBox {
    fn clone(&self) -> Self {
        if self.is_heap_pointer() {
            // Increment Rc via raw pointer manipulation
            unsafe { Rc::increment_strong_count(self.as_raw_ptr()); }
        }
        NanBox(self.0)
    }
}

impl Drop for NanBox {
    fn drop(&mut self) {
        if self.is_heap_pointer() {
            unsafe { Rc::decrement_strong_count(self.as_raw_ptr()); }
        }
    }
}
```

**This is the most complex part of the implementation.** The current `Value` enum gets
Clone/Drop for free from Rust's derive. With NaN boxing, we must manually manage Rc
lifetimes through raw pointer operations. This is `unsafe` code that must be carefully
audited.

#### JIT Backend (jit/)

The JIT already uses a two-tier representation: `JitValueKind::Int/Bool` as raw i64 in
registers, `JitValueKind::Boxed` as `*mut Value` arena pointers. NaN boxing aligns
naturally:

- JIT int/bool → NaN-boxed int/bool (just change the bit encoding)
- JIT boxed → NaN-boxed pointer (encode the arena pointer in NaN bits)

The JIT's `value_arena.rs` would allocate `NanBox` instead of `Value`, halving arena
memory usage.

#### Base Functions (runtime/base/)

The 75 base functions receive and return `Value`. With NaN boxing, they would receive
and return `NanBox`. This is the largest surface area change — every base function
signature changes. A compatibility shim can ease migration:

```rust
impl NanBox {
    pub fn to_value(self) -> Value { ... }     // For gradual migration
    pub fn from_value(v: Value) -> Self { ... }
}
```

### What Cannot Be NaN-Boxed (Remains Heap-Allocated)

These types always go through a pointer tag:

| Type | Current | NaN-boxed | Notes |
|------|---------|-----------|-------|
| `Integer(i64)` | inline 8B | inline 46-bit / slab overflow | ~0.001% overflow in practice |
| `Float(f64)` | inline 8B | inline 8B (raw bits) | Zero cost |
| `Boolean` | inline 1B | inline (tag + 1 bit) | Zero cost |
| `None/Uninit/EmptyList` | inline 0B | inline (tag only) | Zero cost |
| `BaseFunction(u8)` | inline 1B | inline (tag + 8 bits) | Zero cost |
| `GcHandle(u32)` | inline 4B | inline (tag + 32 bits) | Zero cost |
| `String` | `Rc<str>` 16B | pointer tag → `Rc<String>` | Requires Option B (thin pointer) |
| `Array` | `Rc<Vec<Value>>` | pointer tag → `Rc<Vec<NanBox>>` | 3x smaller elements |
| `Tuple` | `Rc<Vec<Value>>` | pointer tag → `Rc<Vec<NanBox>>` | 3x smaller elements |
| `Closure` | `Rc<Closure>` | pointer tag → `Rc<Closure>` | Captures shrink 3x |
| `Adt` | `Rc<AdtValue>` | pointer tag → `Rc<AdtValue>` | Fields shrink 3x |
| `AdtUnit` | `Rc<str>` 16B | pointer tag → interned index | Could use interner ID |
| `Continuation` | `Rc<RefCell<..>>` | pointer tag | Unchanged complexity |

## Implementation Plan

### Phase 0: Prerequisites (before NaN boxing)

1. **Change `Rc<str>` → `Rc<String>`** for `Value::String` and `Value::AdtUnit`
   - Eliminates fat pointers from the Value enum
   - `size_of::<Value>()` drops from 24 → 16 bytes (immediate win even without NaN boxing)
   - Can be done as a standalone PR

2. **Add benchmark baseline**
   - Criterion benchmarks for: fibonacci, binarytrees, array operations, map operations
   - Capture: wall-clock, allocations, peak RSS
   - These benchmarks validate Phase 2

### Phase 1: NanBox Type + Conversion Layer

1. Implement `NanBox` as `#[repr(transparent)] struct NanBox(u64)` in a new
   `runtime/nanbox.rs` module
2. Implement encoding/decoding for all 25 Value variants
3. Implement `Clone`/`Drop` with correct Rc management (unsafe, needs careful review)
4. Implement `NanBox ↔ Value` conversion functions
5. Property-based tests: all Value variants round-trip through NanBox
6. Feature-gated behind `#[cfg(feature = "nan-boxing")]`

### Phase 2: VM Integration

1. Change VM stack from `Vec<Value>` to `Vec<NanBox>`
2. Update dispatch loop to decode NaN boxes
3. Update base function signatures (or use conversion shim)
4. All existing tests must pass
5. Benchmark comparison against Phase 0 baseline

### Phase 3: GC + Closure Integration

1. Update GC root scanning to trace NaN-boxed heap pointers
2. Update closure capture representation
3. Update ADT field storage (`AdtFields` now holds `NanBox`)
4. GC stress tests + leak detector validation

### Phase 4: JIT Integration

1. Update JIT value arena to use NanBox
2. Update JIT ↔ runtime boundary (rt_* helper functions)
3. VM/JIT parity tests must pass unchanged

### Phase 5: Evaluation + Adoption Decision

1. Run full benchmark suite: compare NaN-boxed vs baseline
2. Run `cargo run -- parity-check tests/parity` — maintained VM/native parity
3. Expected wins:
   - **Memory**: ~3x reduction in stack, array, closure memory
   - **Cache**: significant improvement on numeric-heavy workloads
   - **GC pressure**: fewer Rc allocations for wrappers (Some, Left, Right)
4. If wins are validated: remove feature gate, make NaN boxing the default
5. If wins are marginal: keep as optional feature or remove

## Drawbacks

### Complexity

- **Unsafe code**: Manual Rc management through raw pointers. This is the #1 risk.
  Current Flux has zero `unsafe` in the runtime (except GC internals). NaN boxing adds
  ~50 lines of unsafe Rc manipulation that must be correct or values leak/double-free.

- **Debugging**: Values are opaque u64s in the debugger. Need pretty-printer for GDB/LLDB.

- **Integer range**: 46-bit inline integers vs current 64-bit. The overflow path (BigInt
  slab) adds branching on every integer operation. For benchmarks like binarytrees where
  all integers are small, this costs nothing. For programs using large integers (rare in
  Flux today), there's a measurable overhead.

### Portability

- Assumes 48-bit virtual addresses (true on x86-64 and ARM64, not guaranteed on future
  architectures)
- Assumes 8-byte alignment from Rust's allocator (guaranteed by spec, but sensitive to
  custom allocators)

### Maintenance

- Every new Value variant requires updating the NaN boxing encoding table
- Rc lifetime bugs in unsafe code are hard to detect (no sanitizer catches them directly)
- Need a comprehensive property-based test suite to maintain confidence

## Risks

- **GC/Rc interaction**: The most subtle risk. If NanBox::clone/drop doesn't perfectly
  mirror Rc's reference count, values will leak or double-free. Mitigation: extensive
  property tests + the existing leak detector.

- **Float edge cases**: IEEE 754 has many NaN bit patterns. The encoding must not confuse
  a legitimate float with a NaN box. Mitigation: use a canonical NaN for actual NaN
  values; all other quiet NaN patterns are reserved for boxing.

- **Performance regression on pointer-heavy code**: Programs that heavily use strings,
  arrays, and closures won't benefit much from NaN boxing (the values are already behind
  Rc pointers). The win is primarily on numeric and ADT-heavy code.

## Rationale and Alternatives

### Why NaN Boxing over Tagged Pointers?

**Tagged pointers** (used by OCaml, Ruby) steal the low bit(s) of pointers to encode
small values. On 64-bit systems, the low 3 bits of aligned pointers are free.

| Approach | Inline types | Pointer overhead | Float handling |
|----------|-------------|-----------------|----------------|
| NaN boxing | Float + Int + Bool + None | 3-bit shift | Zero cost (raw bits) |
| Tagged pointers | Int (63-bit) + Bool + None | 1-bit tag | **Must heap-allocate all floats** |
| Current Value enum | All types | No overhead | Zero cost (inline) |

NaN boxing wins for Flux because:
1. Flux uses Float heavily (scientific computing, averages, etc.)
2. Tagged pointers would heap-allocate every Float, which is worse than the current 24B enum
3. NaN boxing gives us inline Ints AND inline Floats

### Why Not Pointer Tagging + Boxed Floats (OCaml style)?

OCaml uses 63-bit integers (1 bit for tag) and boxes all floats. This works for OCaml
because it uses floats rarely. Flux programs use floats extensively (grade_analyzer,
benchmarks, effects examples), so boxing floats would be a regression.

## Prior Art

| Language | Approach | Size | Notes |
|----------|----------|------|-------|
| **JavaScriptCore** | NaN boxing | 8B | Exactly this technique. Proven at scale. |
| **LuaJIT** | NaN boxing | 8B | Mike Pall's implementation. Fastest scripting VM. |
| **SpiderMonkey** | NaN boxing | 8B | Firefox JS engine. |
| **CPython** | Tagged `PyObject*` | 8B (ptr) | Everything heap-allocated, different trade-off |
| **OCaml** | Tagged pointers | 8B | 63-bit ints, boxed floats |
| **V8** | Pointer compression | 8B | Compressed 32-bit pointers + Smis |
| **Erlang/BEAM** | Tagged words | 8B | Similar tagging scheme, different use case |

## Unresolved Questions

1. **Should the BigInt slab use a free list or grow-only?** Grow-only is simpler but may
   leak slab slots for short-lived large integers. A free list adds complexity.

2. **Should we intern `AdtUnit` names as integer IDs?** Currently `AdtUnit(Rc<str>)`
   would need a pointer tag. If we intern constructor names (which the interner already
   supports), we could encode them as an immediate tag + interner Symbol ID.

3. **Should NanBox implement `PartialEq` via bits or semantics?** Two NanBoxes with the
   same bits are the same value, but NaN != NaN in IEEE 754. The PartialEq impl must
   match current `Value::PartialEq` behavior.

4. **Phase 0 impact**: How much does the `Rc<str>` → `Rc<String>` change alone improve
   things? If `size_of::<Value>()` drops to 16 and benchmarks improve significantly,
   the urgency of full NaN boxing decreases.

## Future Possibilities

- **SMI (Small Integer) optimization**: If 46-bit integers prove sufficient for 99.9% of
  programs, we could drop the BigInt slab entirely and make integer overflow a runtime
  error (like Rust's debug overflow checks). This eliminates the branch on every int op.

- **String interning in NaN box**: Short strings (≤6 bytes) could be encoded directly in
  the 46-bit payload, avoiding all heap allocation for common strings like `"A"`, `"true"`,
  field names, etc.

- **Unboxed arrays**: With NaN boxing + HM types, the compiler knows when an array
  contains only Ints or only Floats. These could use `Vec<i64>` / `Vec<f64>` directly
  instead of `Vec<NanBox>`, eliminating tagging overhead for numeric arrays entirely.
