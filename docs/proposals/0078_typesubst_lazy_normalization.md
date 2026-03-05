- Feature Name: TypeSubst Lazy Normalization
- Start Date: 2026-03-04
- Completion Date: pending
- Status: Draft
- Proposal PR: pending
- Flux Issue: pending
- Depends on: 0032 (type system), 0051 (HM zero-fallback), 0077 (type-informed optimization)

# Proposal 0078: TypeSubst Lazy Normalization

## Summary

Replace the eager idempotency enforcement in `TypeSubst::compose()` with **lazy
normalization on lookup**: stored values may contain variable chains; normalization
happens in `get_type()` and `get_row()` at read time. This eliminates the O(n·m)
re-walk of all stored bindings that currently fires on every `compose` call, reducing
the dominant cost driver in Algorithm W.

## Motivation

### Current hot spot: `compose()` Steps 3 & 4

`TypeSubst::compose()` (in `src/types/type_subst.rs`) currently enforces the
**idempotency invariant** — "no key appears in any value" — eagerly:

```
Step 1 (lines 77-82):  merge other.type_bindings into self, applying self to each new value
Step 2 (lines 87-92):  merge other.row_bindings into self, applying self to each new row
Step 3 (lines 97-104): re-apply over ALL keys in self.type_bindings   ← O(n·m) hot spot
Step 4 (lines 114-121): re-apply over ALL keys in self.row_bindings   ← O(n·m) hot spot
```

Steps 3 and 4 exist because adding new bindings (Step 1/2) may allow further
rewrites of *existing* keys. For example:

```
self  = { ?0 → ?1 }
other = { ?1 → Int }

After Step 1: self = { ?0 → ?1, ?1 → Int }
After Step 3: self = { ?0 → Int, ?1 → Int }   ← ?0 now fully resolved
```

Without Steps 3/4, `get_type(?0)` would return `?1`, which is still a variable —
violating the invariant.

### Why this is expensive

Algorithm W calls `compose()` for every successful unification constraint. In a
program with `n` top-level functions and average inference depth `d`, `compose()`
is called O(n · d) times. Each call re-walks all `m` currently stored bindings.
For real programs this is O(n · d · m) — effectively **quadratic** in the number
of type variables introduced.

The `perf_type_infer.rs` benchmark (`test_large_program_infer_time`) captures this
regression path. It is the primary driver of inference latency on programs with
many generic functions or deeply nested let-bindings.

### The invariant pays for itself on every read

The current design front-loads cost at write time so that `get_type()` / `get_row()`
remain O(1). But `compose()` is called far more frequently than `get_type()` in the
inner loop of Algorithm W:

- `compose()` is called once per `unify_reporting` → `unify_with_span`
- `get_type()` is called during `apply_type_subst`, which is called in `compose()`
  itself — it already pays the chain-following cost

Deferring normalization to read time shifts this cost to be **proportional to
lookup depth**, which is bounded by chain length — typically ≤ 3–5 hops in
well-typed programs.

## Guide-level explanation

From the maintainer's perspective: after this change, `TypeSubst` stored values
may contain **variable references that are themselves mapped** in the same
substitution. The invariant becomes:

> `get_type(v)` always returns the **fully normalized** type for `v`, following
> chains transitively until a non-variable type or an unmapped variable is reached.

`compose()` becomes a pure merge: it no longer re-walks stored bindings. The O(n·m)
Steps 3 and 4 are removed.

This is semantically equivalent to the current design — all callers use `get_type()`
and `apply_type_subst` to read values, never the raw HashMap. The normalization point
moves from write to read.

## Reference-level explanation

### New `get_type` with chain following

```rust
/// Look up a type variable, following chains until fully resolved.
///
/// Returns `None` if `v` is not mapped.
/// Returns a normalized type (no `Var` that is itself mapped) otherwise.
pub fn get_type(&self, mut v: TypeVarId) -> Option<InferType> {
    let mut result = self.type_bindings.get(&v)?.clone();
    // Follow Var chains — depth bounded by acyclicity invariant from occurs check.
    loop {
        match &result {
            InferType::Var(next) if *next != v => {
                match self.type_bindings.get(next) {
                    Some(t) => {
                        v = *next;
                        result = t.clone();
                    }
                    None => break,
                }
            }
            _ => break,
        }
    }
    Some(result)
}
```

**Acyclicity**: the occurs check in `unify_with_span_and_row_var_counter` (before
calling `insert_type`) guarantees the chain never loops. The loop terminates.

**Complexity**: O(chain length), bounded by the number of variable–variable
unifications for any given type variable. For typical HM inference this is ≤ 5.

### New `get_row` with chain following

`InferEffectRow::apply_row_subst` already follows row-tail chains transitively
(it calls `get_row()` recursively). After this change, `get_row()` returns the
raw stored row and `apply_row_subst` is responsible for full normalization — no
change needed for row lookup.

### Simplified `compose`

```rust
pub fn compose(mut self, other: &TypeSubst) -> TypeSubst {
    // Merge type bindings: apply self to each incoming value to collapse known
    // variable links immediately — this is cheap (single apply pass per new entry).
    for (var, ty) in &other.type_bindings {
        if !self.type_bindings.contains_key(var) {
            let applied = ty.apply_type_subst(&self);
            self.type_bindings.insert(*var, applied);
        }
    }

    // Merge row bindings with the same strategy.
    for (var, row) in &other.row_bindings {
        if !self.row_bindings.contains_key(var) {
            let applied = row.apply_row_subst(&self);
            self.row_bindings.insert(*var, applied);
        }
    }

    // Drop trivial self-bindings that survived the occurs check path.
    self.type_bindings
        .retain(|key, ty| !matches!(ty, InferType::Var(v) if v == key));

    self
    // Steps 3 and 4 removed — no full re-walk.
}
```

The cost drops from O(n · m · avg_type_size) to O(|other| · avg_type_size).

### Impact on `apply_type_subst`

`apply_type_subst` already recurses through type trees. With lazy normalization,
`get_type(v)` may return a type that still contains variable references — but
`apply_type_subst` will continue recursing into those variables, so the net result
is identical. No changes needed in `infer_type.rs`.

### Correctness argument

The current invariant ("values fully normalized at all times") implies:
- `apply_type_subst(ty)` terminates because substitution is finite and values
  contain no keys

The relaxed invariant ("values normalized on read via `get_type` chain following")
implies:
- `apply_type_subst(ty)` still terminates: `get_type` follows chains to a fixpoint,
  `apply_type_subst` recurses into all sub-terms, and acyclicity from the occurs
  check prevents infinite descent.
- `get_type(v)` returns the same result as the eager version would have computed
  during `compose()`.

### Cycle detection

The existing `apply_type_subst_with_seen` cycle guard (in `infer_type.rs`) uses a
`HashSet<TypeVarId>` to detect infinite-loop paths through the substitution. This
guard **is still valid** under lazy normalization — it defends against any bugs that
slip past the occurs check, and its presence means we can safely relax the eager
invariant without risking non-termination in production.

### `insert_type` changes

`insert_type` may now store a value that contains a key present in `type_bindings`
(because we no longer re-walk). The existing `debug_assert!` (occurs check) is
unchanged — it still catches `?t → ?t` (self-reference). What we allow is
`?a → ?b` where `?b → Int` is a separate binding.

The `debug_assert!` comment should be updated to reflect the relaxed invariant:

```rust
/// Insert a new binding `type_var_id → infer_type`.
///
/// Under lazy normalization: values may contain other mapped variables.
/// Full normalization is performed by `get_type` on lookup.
///
/// Panics in debug builds if `infer_type` contains `type_var_id` directly
/// (self-reference would create an infinite chain).
pub fn insert_type(&mut self, type_var_id: TypeVarId, infer_type: InferType) {
    debug_assert!(
        !matches!(&infer_type, InferType::Var(v) if *v == type_var_id),
        "occurs check: inserting {type_var_id} -> {infer_type} would create a self-referential chain"
    );
    self.type_bindings.insert(type_var_id, infer_type);
}
```

> **Note**: The `debug_assert!` now only checks for direct self-reference, not all
> free variables. The full occurs check (which prevents `?a → Foo<?a>`) remains in
> `unify_with_span_and_row_var_counter` before any `insert_type` call — this is the
> authoritative occurs check location.

### Test suite changes

The existing test `compose_reapplies_to_keep_values_idempotent` asserts that after
composing `{?0 → ?1}` with `{?1 → Int}`, `get(?0)` returns `Int` and
`free_vars().is_empty()` holds for all values. This test must be updated to verify
the **observable behaviour** rather than the eager storage invariant:

```rust
#[test]
fn compose_resolves_chains_on_lookup() {
    let mut left = TypeSubst::empty();
    left.insert(0, var(1));

    let mut right = TypeSubst::empty();
    right.insert(1, int());

    let composed = left.compose(&right);

    // Observable: get_type follows chains to full resolution.
    assert_eq!(composed.get(0), Some(int()));
    assert_eq!(composed.get(1), Some(int()));
    // Internal storage may still show ?0 → ?1 — that is now allowed.
    // The invariant is on the read value, not the stored value.
}
```

## Drawbacks

**Read-time cost**: `get_type()` now follows variable chains instead of doing a
single HashMap lookup. In the worst case (long chains), this is O(chain_length).
For typical HM inference where chains are 1–3 hops, the overhead is negligible.

**Diagnostic stability**: `apply_type_subst` called in error messages may need to
follow chains. Since it already calls `get_type` internally via `apply_type_subst`,
this is handled automatically.

**Increased aliasing in stored map**: The stored map is less canonical. Debugging
sessions inspecting `self.type_bindings` directly will see variable–variable
pointers that look "unresolved." This is addressable with a `Debug` implementation
that shows the chain-followed view.

## Alternatives

**Union-find (path compression)**: Replace the HashMap with a union-find structure
with path compression and union by rank. This achieves amortized near-O(1) for
both union and find. Tradeoff: more complex implementation, incompatible with the
current `InferType` tree model (union-find works best on flat integer arrays).
Union-find is the right long-term architecture for a full HM compiler but is a
larger change than lazy normalization.

**Incremental re-walk**: Instead of re-walking all keys (current), only re-walk
keys whose values contain a variable that was just bound. Requires tracking which
values mention which variables (a reverse index). More complex than lazy
normalization with similar or worse constant factors.

**Persistent substitution (Hindley's original)**: Store a functional substitution
as a linked list of bindings, where later entries shadow earlier ones. Lookup
traverses the list. O(n) lookup but eliminates composition entirely. Impractical
for Flux's use pattern.

## Interaction with Proposal 0077

Proposal 0077 introduces a **two-phase inference model**: Phase 1 (type discovery)
and Phase 2 (codegen IDs). Phase 1 runs `infer_program` on the pre-optimized AST
and discards `expr_types`/`expr_ptr_to_id`. Phase 2 runs on the optimized AST.

Lazy normalization directly reduces the cost of **both** Phase 1 and Phase 2
inference runs, making the two-phase model cheap enough to enable under the standard
`--optimize` flag rather than a separate `--type-optimize` gate.

## Implementation sequence

| Step | File(s) | Description |
|------|---------|-------------|
| S1 | `src/types/type_subst.rs` | Implement chain-following `get_type()` and simplified `compose()` |
| S2 | `src/types/type_subst.rs` | Update `insert_type` doc-comment to reflect relaxed invariant |
| S3 | `src/types/type_subst.rs` | Update `compose_reapplies_to_keep_values_idempotent` test to check observable behavior |
| S4 | `tests/perf_type_infer.rs` | Add a micro-benchmark for `compose()` call count × substitution size |
| S5 | `src/types/type_subst.rs` | Optional: `Debug` impl that shows chain-followed view of stored bindings |

S1–S3 are the mechanical change. S4 validates the expected speedup with a
reproducible benchmark. S5 is a developer-ergonomics improvement.

## Unresolved questions

1. Should `get_type` return `Option<InferType>` (owned, chain-followed) or
   `Option<&InferType>` (borrowed, single-hop)? The current signature is
   `Option<&InferType>`. Chain following requires returning an owned value when
   multiple hops are followed. The change to owned return type is a breaking API
   change for all callers — they must switch from `get_type(v).cloned()` to
   `get_type(v)`. This is mechanical but touches many call sites.

2. Should a memoizing `path_compress` method be added alongside `get_type` that
   writes the resolved type back into `type_bindings` (path compression)? This
   would make repeated lookups of the same variable O(1) after the first resolution,
   approximating union-find behaviour without the full structural change.

3. Is the performance improvement measurable on the current `perf_type_infer.rs`
   large-program fixture, or does the fixture need to be expanded to cover programs
   with more mutual-recursion / generic instantiation depth?
