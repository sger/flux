- Feature Name: Internal PrimOp Contract and Stdlib Surface
- Start Date: 2026-04-20
- Status: Proposed
- Proposal PR:
- Flux Issue:
- Depends on: Proposal 0133 (Unified PrimOp enum), Proposal 0154 (CLI/driver split)

# Proposal 0164: Internal PrimOp Contract and Stdlib Surface

## Summary

Reframe Flux primops as an internal compiler/runtime contract rather than the long-term public function surface.

After this proposal:

- `CorePrimOp` remains the internal backend contract used by Core lowering, the VM, and the native backend.
- the standard library becomes the source of truth for user-facing collection and text APIs.
- only a smaller, well-classified subset of operations remains as internal primops.
- convenience operations move toward stdlib ownership, with optional compiler recognition where measurement justifies it.

This proposal does **not** remove primops. It narrows and clarifies their role.

## Motivation

### Current state

Flux already has a unified primop enum in [src/core/mod.rs](src/core/mod.rs:423), and both maintained backend families lower from Core through explicit primop handling.

However, `CorePrimOp` currently mixes several different kinds of operation:

1. low-level arithmetic and comparison operations
2. representation-sensitive runtime operations
3. effectful runtime calls
4. collection primitives
5. convenience string helpers
6. parsing helpers
7. higher-order collection combinators

This creates several problems:

- the architectural boundary between internal backend contract and public library API is blurred
- adding or changing a primop requires evaluating not just lowering/runtime impact, but also source-language stability impact
- convenience functions that should live in stdlib are coupled directly to backend/runtime machinery
- the compiler has fewer clean opportunities to optimize stdlib calls selectively, because many operations are already treated as builtin names
- backend/runtime support obligations are larger than necessary

### Why this matters now

Flux has reached the point where:

- `CorePrimOp` is stable enough to treat as a real internal contract
- the stdlib is large enough to own public APIs directly
- backend work benefits from a smaller and better-specified primitive surface
- future runtime work such as VM/native bridge, FFI growth, and backend parity becomes easier if the public library surface is decoupled from internal primitive shape

## Goals

- Define primops as the internal contract between compiler, VM, and native runtime.
- Make stdlib the public semantic surface for arrays, maps, strings, and higher-level helpers.
- Reduce the set of public names that are implicitly "special" to the compiler.
- Keep backend parity by making VM and native lower the same internal primop set.
- Migrate incrementally without rewriting the whole stdlib at once.

## Non-Goals

- No new user-visible syntax.
- No removal of `CorePrimOp`.
- No immediate rewrite of all builtins into stdlib code.
- No change to Core as the canonical semantic IR.
- No commitment that every stdlib wrapper must be implemented in Flux source immediately.
- No commitment to expose raw primops directly in source.

## Current PrimOp Shape

Today, `CorePrimOp` includes:

- arithmetic and comparison:
  - `IAdd`, `ISub`, `IMul`, `IDiv`, `IMod`
  - `FAdd`, `FSub`, `FMul`, `FDiv`
  - `ICmp*`, `FCmp*`, generic `Eq`/`Lt`/etc.
- core collection operations:
  - `ArrayLen`, `ArrayGet`, `ArraySet`, `ArrayPush`, `ArrayConcat`, `ArraySlice`
  - `HamtGet`, `HamtSet`, `HamtDelete`, `HamtKeys`, `HamtValues`, `HamtMerge`, `HamtSize`, `HamtContains`
- string operations:
  - `StringLength`, `StringConcat`, `StringSlice`, `Split`, `Join`, `Trim`, `Upper`, `Lower`, `StartsWith`, `EndsWith`, `Replace`, `Substring`, `Chars`, `StrContains`
- effect/runtime operations:
  - `Print`, `Println`, `ReadFile`, `WriteFile`, `ReadStdin`, `ReadLines`, `ClockNow`, `Try`, `AssertThrows`, `Panic`
- compatibility/type predicates:
  - `TypeOf`, `IsInt`, `IsFloat`, `IsString`, `IsBool`, `IsArray`, `IsNone`, `IsSome`, `IsList`, `IsMap`
- convenience and higher-order helpers:
  - `Len`, `ToList`, `ToArray`
  - `ArrayReverse`, `ArrayContains`
  - `Sort`, `SortBy`
  - `HoMap`, `HoFilter`, `HoFold`, `HoAny`, `HoAll`, `HoEach`, `HoFind`, `HoCount`, `Zip`, `Flatten`, `HoFlatMap`

This proposal keeps the internal contract but introduces a classification that makes the architectural role of each group explicit.

## Design Overview

### Principle 1: Primops are internal

A primop is an operation that deserves explicit backend/runtime support because at least one of the following is true:

- it is representation-sensitive
- it is performance-critical enough to justify backend support
- it is effectful runtime machinery
- it is required for backend parity
- it is difficult or impossible to express efficiently as ordinary stdlib code

If an operation does not meet one of those criteria, it should not remain a long-term primop by default.

### Principle 2: Stdlib is public

The public user-facing API for common operations should live in stdlib modules, even if the compiler later recognizes and lowers some of those functions specially.

That means the stdlib owns:

- naming
- documentation
- semantic expectations
- composition patterns

while primops own:

- backend/runtime implementation contract
- effect classification
- representation-sensitive execution

### Principle 3: Compiler recognition is allowed, but selective

This proposal does not ban compiler recognition of stdlib functions.

Instead, it tightens the rule:

- the compiler may recognize selected stdlib functions and lower them to internal primops
- but the stdlib function remains the public API
- compiler recognition must be justified by representation needs, backend parity, or measurement

This avoids turning every convenience helper into a permanently exposed builtin.

## Surface Syntax and API Design

This proposal distinguishes two layers:

1. public stdlib API, which may be ordinary `public fn` or `public intrinsic fn`
2. backend/runtime primop implementations

The syntax and API design should make those layers explicit.

### Public surface: stdlib modules

Public code should prefer module-owned APIs rather than direct builtin-style names.

Existing stdlib modules already point in this direction:

- `Flow.Array` in [lib/Flow/Array.flx](lib/Flow/Array.flx)
- `Flow.Map` in [lib/Flow/Map.flx](lib/Flow/Map.flx)
- `Flow.String` in [lib/Flow/String.flx](lib/Flow/String.flx)

Target public usage:

```flux
import Flow.Array as Array
import Flow.Map as Map
import Flow.String as String

fn demo(xs, m, s) {
    let n = Array.length(xs)
    let head = Array.first(xs)
    let name = Map.get(m, "name")
    let size = String.string_len(s)
    (n, head, name, size)
}
```

This proposal does not require an immediate rename of existing public stdlib functions. It does require that the stdlib module be treated as the canonical ownership boundary.

### No raw user-visible primop syntax in v1

This proposal does **not** introduce public syntax like:

```flux
primop ArrayLen(xs)
```

or:

```flux
array_len(xs)
```

for ordinary user code.

Reason:

- primops are internal compiler/runtime machinery
- the public API should stay stable even if the internal primop set changes
- raw primop syntax would leak backend/runtime concerns into user programs

### Public intrinsic syntax

To make the contract explicit without introducing a second layer of internal helper names, this proposal introduces a stdlib-facing declaration form:

```flux
public intrinsic fn length<a>(arr: Array<a>) -> Int = primop ArrayLen
public intrinsic fn get<a>(arr: Array<a>, i: Int) -> Option<a> = primop ArrayGet
public intrinsic fn update<a>(arr: Array<a>, i: Int, val: a) -> Array<a> = primop ArraySet
```

Likewise for maps and strings:

```flux
public intrinsic fn get<m, k>(m: m, key: k) = primop HamtGet
public intrinsic fn set<m, k, v>(m: m, key: k, value: v) = primop HamtSet
public intrinsic fn string_len(s: String) -> Int = primop StringLength
```

This syntax is proposed for approved stdlib/runtime modules such as `lib/Flow/*`.

It is **not** intended for general user modules in v1.

### Intrinsic declaration rules

A `public intrinsic fn`:

- has a full Flux type signature
- has no executable body
- binds directly to a specific `CorePrimOp`
- is part of the stdlib public API
- may only appear in approved internal modules in v1
- must match the target primop's arity and representation constraints
- participates in type checking using its declared Flux type

Rejected forms:

- user-authored `intrinsic fn` in ordinary modules
- private `intrinsic fn` helper layers by default
- overloaded intrinsic declarations that map ambiguously to multiple primops
- intrinsic declarations with arbitrary user-defined lowering rules

### When to use `public intrinsic fn`

Use `public intrinsic fn` only when the public API is an exact 1:1 surface over an internal primop.

Good fits:

- `Array.length` → `ArrayLen`
- `Array.get` → `ArrayGet`
- `Array.update` → `ArraySet`
- `Map.get` → `HamtGet`
- `Map.set` → `HamtSet`
- `String.string_len` → `StringLength`

Do **not** use `public intrinsic fn` when the public operation is library-shaped rather than runtime-shaped.

Examples that should stay ordinary `public fn` by default:

- `Array.reverse`
- `Array.contains`
- `Array.map`
- `Array.filter`
- `Array.fold`
- `String.trim`
- `String.upper`
- `String.lower`
- `String.replace`

### Stdlib ownership with direct primop mapping

The intended pattern for exact low-level operations is:

```flux
module Flow.Array {
    public intrinsic fn length<a>(arr: Array<a>) -> Int = primop ArrayLen
    public intrinsic fn get<a>(arr: Array<a>, i: Int) -> Option<a> = primop ArrayGet
    public intrinsic fn update<a>(arr: Array<a>, i: Int, val: a) -> Array<a> = primop ArraySet
}
```

and for maps:

```flux
module Flow.Map {
    public intrinsic fn get<m, k>(m: m, key: k) = primop HamtGet
    public intrinsic fn set<m, k, v>(m: m, key: k, value: v) = primop HamtSet
}
```

This preserves:

- stdlib ownership of the public API
- explicit linkage to the primop contract
- no extra helper name layer for exact 1:1 mappings
- a clear boundary between intrinsic-backed APIs and ordinary stdlib functions

### Compatibility with current surface

This proposal treats existing free-function helper spellings as legacy compatibility surface immediately.

That means:

1. `Flow.*` module APIs become the primary documented public surface now
2. free-function spellings may remain temporarily as compatibility aliases
3. compiler hardcoding of raw builtin helper names should be reduced as soon as stdlib intrinsic migration lands

Examples of transitional compatibility aliases:

```flux
public fn string_len(s: String) -> Int {
    String.string_len(s)
}

public fn map_get(m, key) {
    Map.get(m, key)
}
```

These aliases are transitional only. They are not the long-term primary API.

## Compiler Recognition Model

The compiler should handle three increasingly explicit cases.

### Case 1: Direct public intrinsic

If a stdlib declaration is `public intrinsic fn ... = primop ...`, the compiler lowers calls to that declaration directly to the bound `CorePrimOp`.

Example:

```flux
public intrinsic fn length<a>(arr: Array<a>) -> Int = primop ArrayLen
```

lows directly to `CorePrimOp::ArrayLen`.

### Case 2: Recognized ordinary stdlib wrapper

For transitional compatibility, the compiler may recognize selected ordinary stdlib wrappers directly and lower them as if they were declared `public intrinsic`.

This is allowed only for:

- backend-critical operations
- representation-sensitive operations
- compatibility during migration

It should not be the default mechanism for convenience helpers, and it should be removed as the stdlib intrinsic-backed surface becomes available.

### Case 3: Ordinary library function

If no public intrinsic declaration or special recognition applies, the function remains ordinary stdlib code and is compiled normally.

This should become the default for:

- higher-order combinators
- convenience text helpers
- parsing helpers
- library composition utilities

## Public API Design by Area

### Arrays

Canonical public API lives in `Flow.Array`.

Preferred public names:

- `Array.length`
- `Array.get`
- `Array.update`
- `Array.push`
- `Array.concat`
- `Array.slice`
- `Array.reverse`
- `Array.contains`
- `Array.map`
- `Array.filter`
- `Array.fold`

Internal bindings should only exist for the core representation-sensitive subset:

- `length`
- `get`
- `update`
- `push`
- `concat`
- `slice`

Everything else should be ordinary stdlib code unless measurement proves it needs internal lowering.

### Maps

Canonical public API lives in `Flow.Map`.

Preferred public names:

- `Map.get`
- `Map.set`
- `Map.delete`
- `Map.merge`
- `Map.keys`
- `Map.values`
- `Map.has`
- `Map.size`

Internal bindings:

- `get`
- `set`
- `delete`
- `merge`
- `keys`
- `values`
- `has`
- `size`

### Strings

Canonical public API lives in `Flow.String`.

Core representation-sensitive operations may have internal bindings:

- `string_len`
- `string_concat`
- `string_slice`

Convenience helpers should remain stdlib-first:

- `starts_with`
- `ends_with`
- `chars`
- `join`
- `trim`
- `upper`
- `lower`
- `replace`

Some of these may later gain compiler recognition, but the proposal default is stdlib ownership, not permanent primop status.

## Parser and Typechecker Impact

The new syntax needed by this proposal is intentionally small:

```text
public intrinsic fn <name>(...) -> ... = primop <CorePrimOpName>
```

Parser requirements:

- recognize `public intrinsic fn`
- recognize `= primop <Identifier>`
- reject function bodies on intrinsic declarations

Typechecker requirements:

- verify that the declared arity matches the target primop
- verify that the declared type shape is compatible with the primop contract
- reject intrinsic declarations outside approved internal modules in v1

Lowering requirements:

- intrinsic declarations lower to function bindings tagged with their target `CorePrimOp`
- direct calls through those bindings lower to the target primop rather than ordinary function call machinery

## Name Resolution and Export Rules

`public intrinsic fn` names are ordinary public stdlib names, but with restricted declaration behavior.

Rules:

- public intrinsic declarations may only be authored in approved internal modules in v1
- intrinsic-backed names are exported like ordinary `public fn`
- user code can import and call the public name normally
- user code cannot declare new `public intrinsic fn` bindings in arbitrary modules

This keeps the public API stable while avoiding an extra layer of internal helper names.

## Concrete PrimOp Classification Plan

This section replaces the generic bucket model with the intended concrete split for Flux.

### Classification rules

Keep an operation as an internal primop only if at least one is true:

- it is representation-sensitive
- it is effect/runtime machinery
- it is required for backend parity
- it is hard to express efficiently as ordinary stdlib code
- it is proven hot enough to justify direct backend/runtime support

Move an operation to stdlib if it is primarily:

- a convenience wrapper
- a text-processing helper
- a parsing helper
- a higher-order combinator
- a library composition utility

### Keep as internal primops

These stay in `CorePrimOp` as the long-term compiler/runtime contract.

#### Arithmetic

- `Add`, `Sub`, `Mul`, `Div`, `Mod`
- `IAdd`, `ISub`, `IMul`, `IDiv`, `IMod`
- `FAdd`, `FSub`, `FMul`, `FDiv`
- `Abs`, `Min`, `Max`, `Neg`

#### Logic and comparisons

- `Not`, `And`, `Or`
- `Eq`, `NEq`, `Lt`, `Le`, `Gt`, `Ge`
- `ICmpEq`, `ICmpNe`, `ICmpLt`, `ICmpLe`, `ICmpGt`, `ICmpGe`
- `FCmpEq`, `FCmpNe`, `FCmpLt`, `FCmpLe`, `FCmpGt`, `FCmpGe`
- `CmpEq`, `CmpNe`

#### Core string operations

- `StringLength`
- `StringConcat`
- `StringSlice`
- `ToString`

#### Core array operations

- `MakeArray`
- `Index`
- `ArrayLen`
- `ArrayGet`
- `ArraySet`
- `ArrayPush`
- `ArrayConcat`
- `ArraySlice`

#### Core map/HAMT operations

- `MakeHash`
- `HamtGet`
- `HamtSet`
- `HamtDelete`
- `HamtKeys`
- `HamtValues`
- `HamtMerge`
- `HamtSize`
- `HamtContains`

#### Runtime and effect operations

- `Print`
- `Println`
- `ReadFile`
- `WriteFile`
- `ReadStdin`
- `ReadLines`
- `ClockNow`
- `Panic`
- `Try`
- `AssertThrows`
- `Time`

#### Type and runtime inspection

- `TypeOf`
- `IsInt`
- `IsFloat`
- `IsString`
- `IsBool`
- `IsArray`
- `IsNone`
- `IsSome`
- `IsList`
- `IsMap`

#### Effect-handler/runtime internals

- `EvvGet`
- `EvvSet`
- `FreshMarker`
- `EvvInsert`
- `YieldTo`
- `YieldExtend`
- `YieldPrompt`
- `IsYielding`
- `PerformDirect`

#### Total arithmetic

- `SafeDiv`
- `SafeMod`

#### Compatibility keepers

- `Len`
- `ParseInt`

### Move to stdlib

These should stop being long-term first-class primops and become ordinary stdlib functions.

#### String and text helpers

Move to `Flow.String`:

- `Split`
- `Join`
- `Trim`
- `Upper`
- `Lower`
- `StartsWith`
- `EndsWith`
- `Replace`
- `Substring`
- `Chars`
- `StrContains`

#### Parsing helpers

Move to `Flow.IO` or a future `Flow.Parse`:

- `ParseInts`
- `SplitInts`

`ParseInt` remains part of the core contract.

#### Generic collection convenience

Move to stdlib:

- `ToList`
- `ToArray`

#### Array convenience

Move to `Flow.Array`:

- `ArrayReverse`
- `ArrayContains`

#### Higher-order collection combinators

Mark transitional immediately and move to `Flow.Array` / `Flow.List`:

- `Sort`
- `SortBy`
- `HoMap`
- `HoFilter`
- `HoFold`
- `HoAny`
- `HoAll`
- `HoEach`
- `HoFind`
- `HoCount`
- `Zip`
- `Flatten`
- `HoFlatMap`

These are library combinators first. If a later optimization is needed, it should target the stdlib boundary rather than making the combinator itself a permanent primop.

### Add as new internal primops

These are worth adding to the internal contract because they are low-level, numeric, and backend/runtime-worthy.

#### Math primops

Add:

- `FSqrt`
- `FSin`
- `FCos`
- `FExp`
- `FLog`
- `FFloor`
- `FCeil`
- `FRound`

Optional later additions:

- `FTan`
- `FAsin`
- `FAcos`
- `FAtan`
- `FSinh`
- `FCosh`
- `FTanh`

#### Bitwise primops

Add:

- `BitAnd`
- `BitOr`
- `BitXor`
- `BitShl`
- `BitShr`
- optionally `BitNot`

### Do not add now

Keep these out of scope for now:

- byte arrays / unboxed arrays
- mutable vars
- MVars / STM / threads
- weak refs / stable pointers
- SIMD vectors
- GC/memory lifetime primops
- parallelism primops
- profiling/eventlog primops
- a full FFI primop family

## PrimOp Metadata Contract

To support the internal-contract model, each retained primop should eventually have explicit metadata beyond its enum case.

Required metadata:

- stable internal name
- arity
- effect kind
- can-fail behavior
- operand runtime representation expectations
- result runtime representation
- backend support status:
  - VM
  - native/LLVM
- optimization properties:
  - pure
  - commutative
  - constant-foldable
  - may panic
  - may allocate

This proposal does not require a metadata table implementation immediately, but it makes that the target architecture.

## API Design by Area

### Arrays

Canonical public API lives in `Flow.Array`.

Use `public intrinsic fn` only for exact low-level mappings:

- `length` -> `ArrayLen`
- `get` -> `ArrayGet`
- `update` -> `ArraySet`
- `push` -> `ArrayPush`
- `concat` -> `ArrayConcat`
- `slice` -> `ArraySlice`

Keep ordinary `public fn` for:

- `map`
- `filter`
- `fold`
- `each`
- `any`
- `all`
- `find`
- `count`
- `flat_map`
- `flatten`
- `zip`
- `contains`
- `reverse`
- `sort`
- `sort_by`
- `take`
- `drop`
- `swap`
- `enumerate`
- `tabulate`
- `from_list`
- `to_list`

Current policy for arrays:

- `Flow.Array` is the primary public API now
- raw helper spellings like `array_len`, `array_get`, `array_set`, `array_push`, `array_concat`, `array_slice`, `array_reverse`, `array_contains` are transitional only

### Maps

Canonical public API lives in `Flow.Map`.

Use `public intrinsic fn` for:

- `get` -> `HamtGet`
- `set` -> `HamtSet`
- `delete` -> `HamtDelete`
- `merge` -> `HamtMerge`
- `keys` -> `HamtKeys`
- `values` -> `HamtValues`
- `has` -> `HamtContains`
- `size` -> `HamtSize`

Current policy for maps:

- `Flow.Map` is the primary public API now
- raw helper spellings like `map_get`, `map_set`, `map_delete`, `map_merge`, `map_keys`, `map_values`, `map_has`, `map_size` are transitional only

### Strings

Canonical public API lives in `Flow.String`.

Use `public intrinsic fn` for:

- `string_len` -> `StringLength`
- `string_concat` -> `StringConcat`
- `string_slice` -> `StringSlice`

Keep ordinary `public fn` for:

- `starts_with`
- `ends_with`
- `chars`
- `join`
- `trim`
- `upper`
- `lower`
- `replace`
- future `lines`
- future `words`
- future `unlines`
- future `unwords`

Current policy for strings:

- `Flow.String` is the primary public API now
- direct helper spellings such as `trim`, `upper`, `lower`, `join`, `chars`, `starts_with`, `ends_with`, `replace`, `str_contains` are transitional when treated as globally recognized builtins

### Parsing and function helpers

Move library-shaped helpers into stdlib modules:

- `ParseInts`, `SplitInts` -> `Flow.IO` or future `Flow.Parse`
- future `id`, `const`, `flip`, `compose` -> future `Flow.Function`

## Detailed Phases

### Phase 1: PrimOp inventory and freeze

Goal:

- decide exactly which existing primops remain part of the long-term internal contract

Scope:

- classify every current `CorePrimOp`
- mark retained, moved, added-later, and out-of-scope groups
- identify compatibility aliases that can stay temporarily
- explicitly mark free-function helper spellings as legacy/transitional, not co-equal public APIs

Outputs:

- proposal-level classification table is final
- implementation checklist derived from that table
- list of direct builtin-name spellings that are transitional only
- frozen statement that `Flow.*` is the primary public surface starting in Phase 1

Acceptance criteria:

- every current `CorePrimOp` case has a concrete disposition
- the retained internal contract is explicit and reviewable
- `Len` and `ParseInt` are explicitly retained in the core contract
- the higher-order collection family is explicitly marked transitional

### Phase 2: Fill the low-level numeric gaps

Goal:

- add missing low-level operations that belong in the internal contract

Scope:

- add math primops
- add bitwise primops
- wire VM and native/LLVM lowering/runtime support

Outputs:

- new `CorePrimOp` cases for math and bitwise operations
- backend/runtime implementations
- focused parity and runtime tests

Acceptance criteria:

- new numeric low-level operations exist only as internal primops
- backend parity is preserved

### Phase 3: Add the intrinsic declaration surface

Goal:

- let stdlib own public APIs while binding exact 1:1 operations directly to primops

Scope:

- parser support for `public intrinsic fn name(...) -> T = primop PrimOpName`
- typechecker validation against primop arity/type contract
- Core lowering support for intrinsic-backed declarations
- restrict declaration sites to approved stdlib/internal modules in v1

Outputs:

- syntax support
- semantic checks
- lowering support
- diagnostics for invalid declaration sites/forms

Acceptance criteria:

- stdlib can declare public intrinsic-backed APIs directly
- no extra helper-name layer is required

### Phase 4: Migrate arrays

Goal:

- make `Flow.Array` the canonical public ownership boundary for arrays

Scope:

- convert `Flow.Array.length/get/update/push/concat/slice` to `public intrinsic fn`
- keep higher-order and convenience array helpers as ordinary `public fn`
- stop treating array convenience helpers as long-term primops

Outputs:

- updated `Flow.Array`
- tests/docs/examples using `Flow.Array` as the public surface
- reduced dependence on raw builtin-name treatment for array helpers

Acceptance criteria:

- `Flow.Array` is the canonical public surface
- only the low-level array core remains intrinsic-backed

### Phase 5: Migrate maps

Goal:

- make `Flow.Map` the canonical public ownership boundary for map operations

Scope:

- convert `Flow.Map` core operations to `public intrinsic fn`
- keep map convenience logic in stdlib rather than adding new HAMT-facing helper primops

Outputs:

- updated `Flow.Map`
- docs/tests/examples prefer module-qualified map APIs

Acceptance criteria:

- map public APIs live in stdlib
- HAMT primops remain the internal backend/runtime contract

### Phase 6: Migrate strings and parsing

Goal:

- separate core string/runtime substrate from text and parsing convenience helpers

Scope:

- convert `string_len/string_concat/string_slice` to `public intrinsic fn`
- keep convenience text helpers as ordinary stdlib functions
- move parsing helpers into `Flow.IO` or future `Flow.Parse`

Outputs:

- updated `Flow.String`
- parsing helper placement decided and documented
- reduced public dependence on text helper primops

Acceptance criteria:

- the retained string primop set is small and justified
- parsing and convenience text helpers are stdlib-owned

### Phase 7: Remove transitional builtin treatment

Goal:

- make stdlib ownership, not builtin-name hardcoding, the default model

Scope:

- remove or de-emphasize direct builtin-name recognition for functions that moved to stdlib
- keep compatibility aliases only where needed
- update docs, examples, and tests to prefer stdlib module APIs

Outputs:

- reduced compiler special-casing of moved helper names
- updated docs/examples/tests
- explicit compatibility policy for remaining aliases

Acceptance criteria:

- convenience helpers are no longer treated as the permanent builtin surface
- docs and examples reflect the stdlib-first model

## Compiler and Backend Impact

### Core lowering

Core lowering remains the canonical place where source/library operations become internal `CorePrimOp`s when appropriate.

This proposal does not add a new semantic IR.

### VM backend

The VM continues to dispatch on internal primops. The change is that fewer public-facing convenience names should map directly to internal primops by default.

### Native/LLVM backend

The native backend continues to lower from `CorePrimOp`. The primop set simply becomes more clearly scoped and better classified.

### Stdlib

The stdlib becomes the public ownership layer for arrays, maps, strings, and higher-order helpers.

This is the main architectural consequence of the proposal.

## Migration Strategy

Migration must be incremental.

Rules:

- do not rewrite all stdlib modules in one proposal
- do not change language semantics
- preserve parity at each phase
- keep internal primops where backend/runtime support is already justified
- move convenience functionality only when a stable stdlib ownership path exists

This proposal intentionally starts with arrays because they provide the clearest low-risk pilot.

## Drawbacks

- more explicit layering means more upfront refactoring discipline
- some current builtin paths may need temporary compatibility handling during migration
- public API ownership shifts may require additional stdlib documentation and tests
- compiler recognition of stdlib wrappers adds some indirection compared to hardcoded builtin names

## Alternatives Considered

### Keep the current mixed model

Rejected because `CorePrimOp` would continue to serve too many roles at once:

- internal backend contract
- public builtin surface
- convenience helper bucket
- higher-order library surface

This makes future cleanup harder, not easier.

### Move everything into stdlib immediately

Rejected because some operations clearly deserve first-class internal/backend support:

- arithmetic
- comparison
- arrays
- HAMT/maps
- core string operations
- effect/runtime operations

The right target is not "no primops". It is "smaller, better-scoped primops".

### Expose raw primops directly as a user-level feature

Rejected because the goal of this proposal is the opposite:

- keep internal backend/runtime machinery internal
- make stdlib the stable public surface

## Unresolved Questions

- Which exact string operations belong in the retained internal core?
- How much compiler recognition of stdlib wrappers should be table-driven versus handwritten.
- Whether higher-order combinator primops should be removed entirely or retained selectively behind compiler lowering.

## Verification

Each phase should preserve:

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all --all-features`
- relevant parity slices for VM/native behavior
- stdlib fixture coverage for arrays, maps, strings, and higher-order helpers

Additional verification per pilot area:

- array-focused fixtures under `tests/flux/Flow/Array_test.flx`
- map-focused stdlib/module tests
- string/text fixtures
- backend dump inspection where lowering changes

## Success Criteria

This proposal is successful when:

- the public API for arrays, maps, and most text helpers is stdlib-owned
- `CorePrimOp` is visibly smaller or, at minimum, more clearly classified
- internal primops have explicit backend/runtime justification
- VM and native continue to share the same internal primop contract
- convenience helpers are no longer permanently coupled to the backend/runtime surface by default
