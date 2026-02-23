# PrimOps vs Base Calls

This document describes Flux call routing after the mirrored-primop hard cutover.

## Mental Model

Flux has two execution categories for direct calls:

1. **True PrimOps** (`OpPrimOp`)
2. **Base calls** (`OpCallBase` fastcall or generic call path)

There are no mirrored PrimOps.

## Call Routing

For a direct unshadowed **identifier call** `foo(arg1, ..., argN)`:

1. Try true-primop resolution: `resolve_primop_call(foo, arity)`.
2. If no primop match, try Base fastcall allowlist and emit `OpCallBase`.
3. Otherwise use generic call lowering (`OpGetBase + OpCall` or regular symbol call).

If `foo` is shadowed by a local/function/global symbol, primop and Base-fastcall lowering are both skipped.

Note: this routing applies to identifier calls. Qualified/member calls (for example `Base.len(...)`) are lowered through member access first and do not use the identifier fastcall shortcut directly.

## Routing Table

| Call kind | Lowering | Runtime path |
|---|---|---|
| True PrimOp match | `OpPrimOp` | `execute_primop` |
| Allowlisted Base call | `OpCallBase` | Base function directly |
| Other call | Generic call ops | Generic call dispatch |

## True PrimOps

The `PrimOp` enum now contains only compiler/runtime-native primitives:

- Numeric arithmetic: `iadd`, `isub`, `imul`, `idiv`, `imod`, `fadd`, `fsub`, `fmul`, `fdiv`
- Comparisons: `icmp_*`, `fcmp_*`, `cmp_eq`, `cmp_ne`
- Array IR: `array_len`, `array_get`, `array_set`
- Map IR: `map_get`, `map_set`, `map_has`
- String IR: `string_len`, `string_concat`, `string_slice`
- Effect/control: `println`/`print`, `read_file`, `clock_now`/`now_ms`, `panic`
- Numeric utilities: `abs`, `min`, `max`
- Array concat fast op: `concat`

### PrimOp Name Aliases

Some primop entries accept multiple source names:

- `map_get` aliases: `get`, `map_get`
- `map_set` aliases: `put`, `map_set`
- `map_has` aliases: `has_key`, `map_has`
- `string_slice` aliases: `substring`, `string_slice`
- `println` aliases: `print`, `println`
- `clock_now` aliases: `now_ms`, `clock_now`

## Base Fastcall Allowlist

`OpCallBase` is emitted for allowlisted Base names:

- Higher-order: `map`, `filter`, `fold`, `flat_map`, `any`, `all`, `find`, `sort_by`, `count`, `zip`, `flatten`
- Core/type: `len`, `type_of`, `is_int`, `is_float`, `is_string`, `is_bool`, `is_array`, `is_hash`, `is_none`, `is_some`, `to_string`
- Collections: `first`, `last`, `rest`, `contains`, `slice`, `reverse`, `sort`
- String utils: `trim`, `upper`, `lower`, `starts_with`, `ends_with`, `replace`, `chars`
- Map utils: `keys`, `values`, `delete`, `merge`, `is_map`
- Parse helpers: `parse_int`, `parse_ints`, `split_ints`

## Examples

- `iadd(1, 2)` -> true primop -> `OpPrimOp`
- `len([1,2,3])` -> Base fastcall -> `OpCallBase`
- `reverse([1,2,3])` -> Base fastcall -> `OpCallBase`
- `split("a,b", ",")` -> generic Base call path (not allowlisted)
- `print("x")` -> true primop (`print/1` only)
- `print("a", "b")` -> generic Base call path (`print/N`, `N != 1`)
- `let len = fn(x) { 0 }; len([1,2,3])` -> shadowed name -> generic symbol call
- `Base.len([1,2,3])` -> qualified member-access lowering (not identifier fastcall matching)

## Effects

True PrimOps carry `PrimEffect` metadata (`Pure`, `Io`, `Time`, `Control`) and are available to effect-summary/optimization analysis.

## Compatibility Note

PrimOp IDs were hard-cutover to the reduced true-primop enum, and bytecode cache format was bumped.

## Current Tier Inventory

The lists below reflect the current code (`resolve_primop_call`, Base fastcall allowlist, Base registry).

### Tier 1: True PrimOps (`OpPrimOp`)

Identifier-call names that resolve directly to primops:

- Numeric: `iadd`, `isub`, `imul`, `idiv`, `imod`, `fadd`, `fsub`, `fmul`, `fdiv`, `abs`, `min`, `max`
- Compare: `icmp_eq`, `icmp_ne`, `icmp_lt`, `icmp_le`, `icmp_gt`, `icmp_ge`, `fcmp_eq`, `fcmp_ne`, `fcmp_lt`, `fcmp_le`, `fcmp_gt`, `fcmp_ge`, `cmp_eq`, `cmp_ne`
- Array IR: `array_len`, `array_get`, `array_set`
- Map IR: `get`/`map_get`, `put`/`map_set`, `has_key`/`map_has`
- String IR: `string_len`, `string_concat`, `substring`/`string_slice`
- Effects/control: `print`/`println`, `read_file`, `now_ms`/`clock_now`, `panic`
- Utility: `concat`

### Tier 2: Base fastcall (`OpCallBase`)

Allowlisted Base names:

- Higher-order: `map`, `filter`, `fold`, `flat_map`, `any`, `all`, `find`, `sort_by`, `count`, `zip`, `flatten`
- Core/type: `len`, `type_of`, `is_int`, `is_float`, `is_string`, `is_bool`, `is_array`, `is_hash`, `is_none`, `is_some`, `to_string`
- Collections: `first`, `last`, `rest`, `contains`, `slice`, `reverse`, `sort`
- String utils: `trim`, `upper`, `lower`, `starts_with`, `ends_with`, `replace`, `chars`
- Map utils: `keys`, `values`, `delete`, `merge`, `is_map`
- Parse: `parse_int`, `parse_ints`, `split_ints`

### Tier 3: Generic Base call (`OpGetBase + OpCall`)

Base names that are not in Tier 1 or Tier 2:

- Collections/utilities: `push`, `split`, `join`
- Lists: `hd`, `tl`, `is_list`, `to_list`, `to_array`, `list`
- I/O/runtime: `read_lines`, `read_stdin`, `time`
- Numeric helpers: `range`, `sum`, `product`
- Test helpers: `assert_eq`, `assert_neq`, `assert_true`, `assert_false`, `assert_throws`

## Future Optimization Track

### NaN-boxing (deferred)

NaN-boxing is tracked as a future runtime optimization, not part of the current primop/Base routing architecture.

- Status: deferred (evaluate when numeric-heavy workloads justify runtime representation changes)
- Goal: reduce `Value` representation overhead for numeric paths
- Preconditions:
  - stable baseline benchmarks for VM and JIT
  - hotspot evidence that value representation is a primary bottleneck
- Risks:
  - higher runtime/GC/tagging complexity
  - portability and correctness risk around floating-point bit patterns
  - harder debugging and maintenance
- Adoption gate:
  - measurable end-to-end win on numeric suites without regressions in non-numeric programs
