# PrimOps vs Base Functions

> Scope: current runtime/compiler behavior.
> Proposal context:
> - Base prelude semantics: `docs/proposals/028_base.md`
> - Base API classification and review policy: `docs/internals/base_api.md`
> - Flow stdlib modules: `docs/proposals/030_flow.md`

This document defines the two concepts and the current lowering matrix.

Note: Base API classification (`stable-core` vs `provisional-review`) is a surface-governance policy and is tracked separately in `docs/internals/base_api.md`.

Source anchors:
- PrimOp resolver and execution: `src/primop/mod.rs`
- Bytecode call lowering: `src/bytecode/compiler/expression.rs`
- JIT call lowering: `src/jit/compiler.rs`
- Base registry: `src/runtime/base/mod.rs`

## What Is A PrimOp?

A PrimOp is an internal primitive operation with:
- A stable numeric ID (`PrimOp` enum).
- Fixed arity.
- Explicit effect metadata (`Pure`, `Io`, `Time`, `Control`).

When a direct call is recognized as a primop, the compiler/JIT emits a primop path (`OpPrimOp` in VM, primop helper in JIT), and execution goes through `execute_primop`.

## What Is A Base?

A Base-surface base is a regular function from the global Base function table (`BASE_FUNCTIONS`), callable by name and as a value.

Base Functions can execute through:
- Generic call path (`OpGetBase` + `OpCall`).
- Base fastcall superinstruction (`OpCallBase`) for allowlisted higher-order base functions.

## Routing Rules For `foo(args...)`

1. If `foo/arity` matches `resolve_primop_call`, lower to primop.
2. Else if `foo` is in base fastcall allowlist, lower to `OpCallBase`.
3. Else use generic base/function call path.

Shadowing rule (bytecode + JIT): if a local/function/global shadows a base name, primop and base-fastcall lowering are skipped.

## Terms

Simplest way to think about it:

- `Primop`: special fast internal operation.
- `Base`: normal standard library function.

The other terms are routing details for base functions:

- `Allowlisted base`:
  “Hot” means frequently used base functions where call overhead matters.
  For those names, the compiler emits `OpCallBase` (fastcall) instead of `OpGetBase + OpCall`.
  This removes part of call overhead (no base value materialization first), so it is faster.
  It is still a base, not a primop.
  Terminology: “allowlisted” means this base is explicitly in the approved optimization set; names not on the list are excluded by default.
  Example: `map`.
  Current allowlist:
  `map`, `filter`, `fold`, `flat_map`, `any`, `all`, `find`, `sort_by`, `count`
- `Not allowlisted base`:
  A normal base with no special fast opcode.
  Called through the regular base call path.
  Examples: `reverse`, `push`.
  Current list:
  `push`, `reverse`, `sort`, `split`, `join`, `hd`, `tl`, `is_list`, `to_list`, `to_array`, `list`, `read_lines`, `read_stdin`, `time`, `range`, `sum`, `product`, `zip`, `flatten`, `assert_eq`, `assert_neq`, `assert_true`, `assert_false`, `assert_throws`
- `Shadowed name`:
  You defined your own value with the same name as a base.
  The compiler must use your value, so it cannot apply primop/fastcall optimization.
  Example:
  `let print = \x -> x`
  `print("hi")   // your function, not base print/primop`

## Call Routing Graph

```text
Source call: foo(arg1, ... argN)
    │
    ▼
Shadowing check
    │
    ├── yes (foo is local/function/global)
    │       ▼
    │   Generic call lowering
    │   (no primop, no base fastcall)
    │       │
    │       ▼
    │   Base/function implementation
    │
    └── no (unshadowed)
            │
            ▼
        PrimOp resolver: resolve_primop_call(foo, arity)
            │
            ├── match
            │       ▼
            │   PrimOp lowering
            │   VM: OpPrimOp
            │   JIT: primop helper call
            │       │
            │       ▼
            │   execute_primop
            │
            └── no match
                    │
                    ▼
                Base fastcall allowlist check
                    │
                    ├── yes
                    │       ▼
                    │   OpCallBase lowering
                    │       │
                    │       ▼
                    │   Base implementation
                    │
                    └── no
                            ▼
                        Generic call lowering
                            │
                            ▼
                        Base/function implementation
```

Concrete examples for each branch:

```text
PrimOp branch (unshadowed + primop match):
  print("hello")
  -> resolve_primop_call("print", 1) = PrimOp::Println
  -> primop lowering

Base fastcall branch (unshadowed + no primop + allowlisted):
  map([|1, 2, 3|], \x -> x + 1)
  -> not a primop
  -> allowlisted base
  -> OpCallBase

Base generic branch (unshadowed + no primop + not allowlisted):
  reverse([|1, 2, 3|])
  -> not a primop
  -> not allowlisted
  -> OpGetBase + OpCall

Shadowed branch (skip primop + fastcall):
  let print = \x -> x
  print("hello")
  -> shadowed name
  -> generic call lowering
```

## Matrix

Path legend:
- `True PrimOp`: lowered to primop and executed in `execute_primop`.
- `Base fastcall`: lowered to `OpCallBase`, still base implementation.
- `Base generic`: normal base call path.

## Examples

### PrimOp example

```flx
print("hello")
```

`print/1` is recognized by `resolve_primop_call`, so this lowers to `PrimOp::Println`.

### Base fastcall example

```flx
map([|1, 2, 3|], \x -> x + 1)
```

`map` is not a primop, but it is in the base fastcall allowlist, so this lowers to `OpCallBase`.

### Base generic example

```flx
reverse([|1, 2, 3|])
```

`reverse` is neither a primop nor in the fastcall allowlist, so it uses generic base call lowering.

### Base Names That Lower To True PrimOps

Example (`print` -> `Println`):

```flx
print("hello")
```

For an unshadowed direct call, this is lowered to `PrimOp::Println` (not generic base call).

Shadowed counterexample:

```flx
let print = \x -> x
print("hello")
```

Here `print` is a local symbol, so primop lowering is skipped and normal call resolution is used.

| Base name | PrimOp |
|---|---|
| `print` | `Println` |
| `len` | `Len` |
| `first` | `First` |
| `last` | `Last` |
| `rest` | `Rest` |
| `to_string` | `ToString` |
| `concat` | `ConcatArray` |
| `contains` | `Contains` |
| `slice` | `Slice` |
| `trim` | `Trim` |
| `upper` | `Upper` |
| `lower` | `Lower` |
| `starts_with` | `StartsWith` |
| `ends_with` | `EndsWith` |
| `replace` | `Replace` |
| `chars` | `Chars` |
| `substring` | `StringSlice` |
| `keys` | `Keys` |
| `values` | `Values` |
| `has_key` | `MapHas` |
| `merge` | `Merge` |
| `delete` | `Delete` |
| `abs` | `Abs` |
| `min` | `Min` |
| `max` | `Max` |
| `type_of` | `TypeOf` |
| `is_int` | `IsInt` |
| `is_float` | `IsFloat` |
| `is_string` | `IsString` |
| `is_bool` | `IsBool` |
| `is_array` | `IsArray` |
| `is_hash` | `IsHash` |
| `is_none` | `IsNone` |
| `is_some` | `IsSome` |
| `put` | `MapSet` |
| `get` | `MapGet` |
| `is_map` | `IsMap` |
| `read_file` | `ReadFile` |
| `parse_int` | `ParseInt` |
| `now_ms` | `ClockNow` |
| `parse_ints` | `ParseInts` |
| `split_ints` | `SplitInts` |

### True PrimOps With No Base Entry

| Direct call name | PrimOp |
|---|---|
| `iadd`, `isub`, `imul`, `idiv`, `imod` | Integer arithmetic primops |
| `fadd`, `fsub`, `fmul`, `fdiv` | Float arithmetic primops |
| `icmp_eq`, `icmp_ne`, `icmp_lt`, `icmp_le`, `icmp_gt`, `icmp_ge` | Integer compare primops |
| `fcmp_eq`, `fcmp_ne`, `fcmp_lt`, `fcmp_le`, `fcmp_gt`, `fcmp_ge` | Float compare primops |
| `cmp_eq`, `cmp_ne` | Generic compare primops |
| `array_len`, `array_get`, `array_set` | Array primops |
| `map_get`, `map_set`, `map_has` | Canonical map primop names |
| `string_len`, `string_concat`, `string_slice` | Canonical string primop names |
| `println`, `clock_now`, `panic` | Effect/control primop names |

### Base Functions Using Base Fastcall (`OpCallBase`)

This path is a middle ground between true primops and generic base calls:
- The compiler/JIT still treats the callee as a base function (not a `PrimOp` ID).
- Call lowering uses `OpCallBase` to skip some generic call overhead.
- Runtime behavior remains the base implementation, which is important for callback-heavy/higher-order functions.

Use this category when:
- The operation is performance-sensitive enough to benefit from fused call dispatch.
- Semantics are still better expressed as regular base functions (especially higher-order behavior).

- `map`
- `filter`
- `fold`
- `flat_map`
- `any`
- `all`
- `find`
- `sort_by`
- `count`

### Base Generic Path Only

These base functions stay on the normal call path (`OpGetBase` + `OpCall`).

Reasons a base stays here:
- It is not in the primop resolver and not in the fastcall allowlist.
- It is commonly used as a first-class value (passed around/stored).
- It has lower ROI for adding a dedicated fast path right now.

This is the most flexible path and the baseline semantics for base functions.

- Collection helpers: `push`, `reverse`, `sort`, `split`, `join`, `range`, `sum`, `product`, `zip`, `flatten`
- List API: `list`, `hd`, `tl`, `is_list`, `to_list`, `to_array`
- I/O/misc: `read_lines`, `read_stdin`, `time`
- Test API: `assert_eq`, `assert_neq`, `assert_true`, `assert_false`, `assert_throws`

## Practical Notes

- Direct, unshadowed calls to mapped names above use primop lowering.
- Passing base functions as values still uses base call machinery.
- Primops are optimization/runtime targets; base functions remain the language-level API surface.
