# Aether Internals

This document explains what Aether is, why it exists, where it sits in the
Flux compiler pipeline, and what the main Aether passes and invariants are.

For the landed architecture foundation, see:
- `docs/proposals/implemented/0084_aether_memory_model.md`

For the remaining maturity roadmap, see:
- `docs/proposals/0114_aether_perceus_completion_plan.md`

For the reduced formal proof target, see:
- `docs/internals/aether_formal_semantics.md`

For practical dump-reading and backend-debugging guidance, see:
- `docs/internals/aether_debugging.md`
- `docs/internals/core_aether_backend_boundaries.md`

---

## What Aether is

Aether is Flux's backend-only ownership, duplication, drop, reuse, and FBIP
lowering layer for maintained RC backends.

It is not a separate semantic language IR. It is a backend-only lowering
product derived from clean **Core IR** after the standard Core passes and
before lowering to CFG/LIR.

At a high level, Aether:

- materializes ownership operations in Aether
- distinguishes borrowed versus owned call behavior
- inserts `Dup` and `Drop`
- recognizes legal in-place reuse opportunities and emits `Reuse`
- specializes some drop paths with `DropSpecialized`
- verifies that the transformed Core is memory-management-safe
- checks `@fip` / `@fbip` semantically on the transformed program

---

## Purpose

Flux is a pure language at the source level, but its maintained RC backends
still need an efficient memory-management strategy.

Aether exists to give Flux:

- one coherent ownership/runtime story across VM, JIT, and LLVM
- compile-time control over reference-count churn
- zero-allocation functional updates when uniqueness makes them safe
- a shared RC-backend place to reason about memory-management behavior
- a semantic foundation for `@fip` / `@fbip`

In short:

- **without Aether**, Flux would rely on plain reference counting behavior and
  miss many reuse and ownership optimizations
- **with Aether**, ownership and reuse become explicit, testable compiler
  decisions

---

## Pipeline Position

Aether runs after the standard Core passes and before backend IR lowering:

```text
AST
  -> HM type inference
  -> Core lowering
  -> Core passes
  -> Aether lowering
  -> Aether verification / FBIP checking
  -> CFG or LIR lowering
  -> VM bytecode / LLVM
```

This placement matters:

- earlier would be wrong because Core passes still reshape variable use
- later would force ownership reasoning into backend-specific CFG/dataflow code

The main production boundary is:
- `src/aether/mod.rs`
- `lower_core_to_aether_program(...)`

`run_core_passes*` remains semantic-only. Aether lowering happens after that
clean Core boundary.

---

## Aether Representation

Aether is represented directly in Aether-owned types.

Key Aether-related nodes:

- `AetherCall`
- `Dup`
- `Drop`
- `Reuse`
- `DropSpecialized`

Relevant definitions:
- `src/aether/mod.rs`

This is the current design choice:

- semantic Core stays clean
- Aether decisions are explicit in `AetherExpr`
- VM and LLVM lower the same Aether product
- backend code realizes Aether decisions but does not invent competing ones

---

## Main Passes

The Aether implementation lives in:
- `src/aether/`

Important modules:

- `analysis.rs`
  - ownership-demand analysis over Aether expressions
- `borrow_infer.rs`
  - inferred borrow signatures and interprocedural borrow registry
- `insert.rs`
  - inserts `Dup`, `Drop`, and `AetherCall` argument modes
- `fusion.rs`
  - simplifies/cancels some `Dup`/`Drop` patterns
- `drop_spec.rs`
  - emits `DropSpecialized` on safe unique/shared splits
- `reuse_analysis.rs`
  - symbolic provenance analysis for scrutinees and fields
- `reuse.rs`
  - inserts plain `Reuse`
- `reuse_spec.rs`
  - selective-write reuse specialization via `field_mask`
- `verify.rs`
  - Aether contract checker and optional diagnostics
- `fbip_analysis.rs`
  - semantic FBIP fixed-point analysis
- `check_fbip.rs`
  - validates `@fip` / `@fbip` annotations against semantic results

The rough pass shape is:

1. infer borrow information
2. analyze ownership demand
3. insert `Dup` / `Drop` and borrowed/owned call modes
4. fuse simple dup/drop pairs
5. recognize `DropSpecialized`
6. recognize `Reuse`
7. specialize some reuse writes with `field_mask`
8. verify the resulting Aether form
9. run semantic FBIP checks

---

## Borrowing and Ownership

Aether does not require source-level borrow annotations.

Instead, Flux infers borrowing internally using:
- `src/aether/borrow_infer.rs`

Key ideas:

- every call argument is treated as either borrowed or owned
- direct internal functions, known builtins, and imported/name-based fallbacks
  all go through Aether summary plumbing
- unknown or opaque callees stay conservative

Important types:

- `BorrowMode`
- `BorrowSignature`
- `BorrowRegistry`
- `BorrowProvenance`

This information is then consumed by insertion/planning so Flux can avoid
unnecessary `Dup`/`Drop` around read-only calls.

---

## Reuse

`Reuse` is how Aether expresses a zero-allocation functional update.

Conceptually:

- a value is matched/destructured
- the old allocation is dead on the unique path
- a constructor of the same compatible shape is rebuilt
- Aether reuses the old allocation instead of allocating fresh storage

Relevant modules:
- `src/aether/reuse_analysis.rs`
- `src/aether/reuse.rs`
- `src/aether/reuse_spec.rs`

Important implementation details:

- reuse is exact and provenance-driven
- the token must not escape into rebuilt fields
- shared-path conservatism is preserved
- selective writes are encoded explicitly with `field_mask`

At runtime this lowers to backend-specific reuse helpers/opcodes, but the
decision to reuse is made in Aether after semantic Core is already fixed.

---

## Drop Specialization

`DropSpecialized` is Aether's unique/shared branch split.

It corresponds to the Perceus idea that the unique fast path can often avoid
most RC work while the shared path stays conservative.

Relevant modules:
- `src/aether/drop_spec.rs`
- `src/aether/fusion.rs`

Conceptually:

- test whether a scrutinee is uniquely owned
- if unique, run a specialized body that can drop less and/or reuse more
- if shared, run a conservative body

`DropSpecialized` remains a first-class Aether node until backend IR lowering.

---

## Verification

Aether has an internal contract checker:
- `src/aether/verify.rs`

This checker is not a formal proof, but it operationalizes key invariants that
must hold before lowering to CFG/backends.

Examples of checks:

- unresolved Aether variables are rejected
- dropping a still-live value is rejected
- illegal reuse tags are rejected
- reuse-token escape into fields is rejected
- invalid `field_mask` is rejected
- malformed `DropSpecialized` usage is rejected

This is why malformed Aether fails before backend lowering with:
- `E999 Aether Verification Failed`

The verifier should be understood as an executable contract checker. The
reduced formal target for future proof work is documented separately in:
- `docs/internals/aether_formal_semantics.md`

---

## FBIP

Aether also provides the semantic basis for:
- `@fip`
- `@fbip`

Relevant modules:
- `src/aether/fbip_analysis.rs`
- `src/aether/check_fbip.rs`

Important points:

- the analysis is semantic, not just a constructor counter
- it tracks causes like:
  - fresh allocation
  - token unavailability
  - non-provable direct call
  - indirect/unknown call
  - builtin/effect boundary
  - control-flow join imprecision
- annotated functions are checked after Aether transformation, not before

Current public surface:

- `@fip`
- `@fbip`

Bounded forms like `fip(n)` / `fbip(n)` are intentionally deferred.

---

## Reporting and Debugging

### Flags

**`--dump-aether`** — run Aether, print the per-function report, exit without
executing the program. Best starting point for inspecting what Aether decided.

```bash
cargo run -- --dump-aether examples/aether/borrow_calls.flx
```

**`--trace-aether`** — same report, but continues into the backend and runs
the program. Report goes to stderr before program output. Lets you verify
Aether decisions and correct output together.

```bash
cargo run -- --trace-aether examples/aether/borrow_calls.flx
```

Combine with `--stats` to see timing alongside the report:

```bash
cargo run -- --trace-aether --stats examples/aether/borrow_calls.flx
```

Note: `--trace-aether` is incompatible with `--dump-core` and `--dump-aether`.
Use one at a time.

**`--dump-core`** — print full Core IR after all passes including Aether, then
exit. Shows the actual `Dup`, `Drop`, `Reuse`, `AetherCall`, and
`DropSpecialized` nodes that were inserted.

```bash
cargo run -- --dump-core examples/aether/borrow_calls.flx
```

**`--dump-core=debug`** — same but raw Rust `Debug` format. More verbose,
shows every struct field.

```bash
cargo run -- --dump-core=debug examples/aether/borrow_calls.flx
```

**`scripts/release/bench_aether.sh`** — opt-in release workload runner that
prints Aether totals separately from VM/JIT/LLVM timing data.

### Reading the report

Example output for `examples/aether/borrow_calls.flx`:

```
── Aether Trace ──
file: examples/aether/borrow_calls.flx
backend: vm
pipeline: AST -> Core -> CFG -> bytecode -> VM
cache: disabled
optimize: off
strict: on
modules: 1
────────────────────────
Aether Ownership Report
==========================

── fn main ──
  Dups: 0  Drops: 1  Reuses: 0  DropSpecs: 0
  FreshAllocs: 0
  verifier: ok
λ.
  let %t1 = (let %t2 = (let %t3 = MakeList(1, 2, 3)
  aether_call[borrowed] len_twice(%t3))
  aether_call[borrowed] print(%t2))
  drop %t1
  ...

── Total ──
  Dups: 0  Drops: 1  Reuses: 0  DropSpecs: 0
  FreshAllocs: 0
```

Per-function stats:

| Field | Meaning |
|---|---|
| `Dups` | Number of `Rc::clone` operations inserted |
| `Drops` | Number of early releases inserted |
| `Reuses` | Number of in-place allocation reuses |
| `DropSpecs` | Number of unique/shared runtime splits |
| `FreshAllocs` | New heap allocations tracked by Aether |
| `verifier` | `ok` means post-pass ownership checks passed |

Functions with all-zero stats are omitted from the report.

`aether_call[borrowed]` in the Core body means Aether determined that argument
is read-only — no `Dup` is emitted for it. `aether_call[borrowed, owned]`
means the first argument is borrowed and the second is owned (ownership
transfers to the callee).

The Aether report is built from:
- `src/compiler/mod.rs`

It includes:

- per-function Aether stats
- verifier status
- FBIP status for annotated functions
- full transformed readable Core bodies
- totals

---

## What Aether is not

Aether is not:

- a user-visible ownership type system
- a shared-memory concurrency system
- a proof that arbitrary programs have no leaks in the broadest sense
- a backend-specific optimization layer

Current limitation:

- Flux Aether is intentionally single-threaded today and uses `Rc`, not `Arc`
- actor transfer semantics and concurrent RC design are future work outside the
  current Aether maturity plan

---

## Proof Status

Flux Aether currently has:

- implemented Aether transformations on Core
- verifier-enforced local invariants
- test-backed regression and backend-parity evidence

Flux Aether does **not** currently have:

- a mechanized proof artifact
- theorem-level proof matching the Perceus paper
- concurrency/thread-shared RC formalization

The planned formal target is:

- reduced single-threaded Aether only
- paper-aligned two-layer structure:
  - reference-counted semantics for reduced Aether Core
  - syntax-directed Aether transformation obligations
- explicit mapping from verifier checks to future theorem/lemma targets

Deferred proof areas include:

- actor/concurrency semantics
- atomic/thread-shared RC
- full FBIP completeness proofs
- backend equivalence proofs

This means Flux can claim executable verification and a planned proof scaffold,
but not formal proof completion.

For FBIP/proof comparison language, keep these distinct:

- semantic `@fip` / `@fbip` checking exists
- proof coverage is still partial in higher-order and join-heavy cases
- formal theorem proof is not implemented

In particular, `NotProvable` means the checker could not prove the requested
contract under current precision; it should not be read as a proof that the
program is semantically non-FIP.

---

## Practical Summary

The shortest accurate description is:

> Aether is Flux's compile-time ownership and reuse pipeline. It inserts
> explicit duplication and drop behavior in Core, recognizes legal in-place
> reuse opportunities, verifies the transformed program, and provides the
> semantic basis for `@fip` / `@fbip` across VM, JIT, and LLVM.
