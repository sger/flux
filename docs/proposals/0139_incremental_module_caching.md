- Feature Name: Incremental Module Caching and Centralized Cache Layout
- Start Date: 2026-04-01
- Status: Implemented (Phases 1-6 complete)
- Proposal PR:
- Flux Issue:
- Supersedes: Proposal 0128 (Incremental Compilation), Proposal 0137 (Modular Aether and Module Interfaces)
- Builds on: Proposal 0121 (Separate Module Compilation — Phases 1 & 4 already implemented)
- Integrates with: Proposal 0138 (Flux Parity Ways — Phase 5 only)

## Summary

Redesign Flux caching around per-module semantic and backend artifacts so that:

- changing a `.flx` module always recompiles that module
- downstream modules recompile only when the changed module's exported interface changes
- VM and LLVM share one semantic invalidation contract
- VM and LLVM keep separate backend artifact caches
- cache artifacts live under a centralized cache root, not beside source files

This proposal preserves Flux's architecture:

```text
AST -> Core -> cfg -> bytecode / JIT / LLVM
```

`Core` remains the semantic checkpoint. This proposal does not introduce a second semantic IR.

## Motivation

Flux already has partial caching infrastructure:

- `.flxi`
  - semantic interface cache for exported `Scheme`s and borrow signatures
- `.fxm`
  - VM module artifact cache
- `.fxc`
  - whole-program bytecode cache

But the current system is still too coarse:

1. **Invalidation is source-oriented, not interface-oriented**
   - A source change can force recompilation even when the public semantic surface is unchanged.

2. **Cache placement is inconsistent**
   - some artifacts are centralized
   - `.flxi` has historically been source-adjacent

3. **LLVM/native is still whole-program**
   - the native path rebuilds a merged program and re-runs HM/lowering for the whole graph

4. **Logs are ambiguous**
   - "interface loaded" can happen in the same run that compiled and stored that interface

Flux should adopt a module-first incremental model:

- private implementation edits rebuild only the changed module
- public API changes rebuild the changed module and its dependents
- unrelated modules stay cached
- VM and LLVM make the same semantic invalidation decision

### Examples

#### Private change

```flux
fn helper(x) {
    x + 1
}

public fn answer() -> Int {
    helper(41)
}
```

Change only `helper`'s body:

```flux
fn helper(x) {
    40 + 2
}
```

Desired behavior:

- recompile this module
- do **not** recompile dependents

#### Public change

```flux
public fn answer() -> Int {
    42
}
```

Change to:

```flux
public fn answer() -> String {
    "42"
}
```

Desired behavior:

- recompile this module
- recompile dependents transitively

## Guide-level explanation

### For users

Caching is automatic.

Flux resolves a cache root using:

1. `--cache-dir <path>` if provided
2. nearest ancestor of the entry file containing `flux.toml` -> `<root>/target/flux/`
3. nearest ancestor of the entry file containing `Cargo.toml` -> `<root>/target/flux/` (development fallback for the compiler repository)
4. otherwise `<entry-dir>/.flux/cache/`

**Note:** Rule 2 uses `flux.toml` as the project marker for Flux user projects. Rule 3 exists as a development convenience while Flux is built inside a Cargo workspace, but `Cargo.toml` is not the long-term project marker for Flux end users.

Artifacts live under that cache root:

```text
<cache-root>/
  interfaces/
  vm/
  native/
  tmp/
```

Users do not need to manage `.flxi` files beside source files.

### For compiler contributors

Flux caching should have two layers:

1. **Semantic layer**
   - `.flxi`
   - backend-independent
   - defines whether dependents are invalidated

2. **Backend layer**
   - `.fxm` for VM
   - native per-module artifact for LLVM/native
   - defines reusable executable payload only

The semantic layer decides whether a module is stale for dependency reasons.
The backend layer decides whether executable payload can be reused for a selected backend.

## Reference-level explanation

## Architecture

### Semantic artifact: `.flxi`

`.flxi` is the canonical per-module semantic cache.

It stores:

- module name
- compiler version
- cache format version
- source hash
- semantic config hash
- exported public members
- exported `Scheme`s
- exported ADT/effect declarations required downstream
- exported borrow signatures
- dependency interface fingerprints
- interface fingerprint

### Semantic config hash

The semantic config hash captures compiler settings that affect the semantic output of a module. Two compilations of the same source with different semantic configs must produce different cache keys.

The semantic config hash includes:

- strict mode (`--strict`)
- optimization level (`-O`) when it affects type-informed folding or Core passes
- any future flags that alter type inference, Core lowering, or Aether behavior

It excludes:

- backend-only flags (e.g., LLVM opt level, emit format)
- diagnostic/display flags (e.g., `--verbose`, `--stats`)

The current codebase uses `strict_hash` for VM but has no equivalent for LLVM. This proposal unifies both backends under a single semantic config hash.

### Cache format versioning

`.flxi` must include a format version field. When the `.flxi` format changes (e.g., adding ADT/effect serialization in Phase 2), bumping the format version invalidates all cached interfaces. This prevents stale interfaces from being loaded after a compiler upgrade that changes the interface structure.

### Interface fingerprint

The interface fingerprint must be computed from exported semantic surface only.

It includes:

- public function names
- public visibility
- public type schemes
- exported ADTs/constructors/types visible downstream
- exported effects visible downstream
- exported borrow signatures (always included — see rationale below)

It excludes:

- private helpers
- function bodies
- formatting and comments
- debug metadata
- backend-specific artifacts

**Note on ADTs and effects:** The current `ModuleInterface` only stores `schemes` and `borrow_signatures`. ADT constructors and effect declarations are not yet serialized. Phase 2 should begin with a schemes-and-borrow-only fingerprint, then extend to ADTs and effects once the serialization infrastructure is in place.

**Note on borrow signatures:** Borrow signatures must always be part of the fingerprint. They affect Aether dup/drop insertion in downstream modules — if a dependency changes from `Borrowed` to `Owned`, the downstream module's Aether pass produces different code. Making this conditional adds complexity for marginal savings.

### Backend artifacts

#### VM

Use `.fxm` as the per-module VM payload cache.

It stores:

- globals/bindings
- constants
- instructions
- debug metadata

Validity depends on:

- local source/config invalidation
- compiler/cache format version
- dependency interface fingerprints

#### Native

Add a per-module native artifact cache under `native/`.

The exact payload format may be one of:

- lowered module IR
- LLVM IR text
- object file
- another stable linkable module artifact

Phase 1 of this proposal does not require choosing the final native artifact format, but it does require:

- one native artifact per module
- dependency validity based on `.flxi`
- versioning separate from VM artifacts

## Cache root resolution

Use one shared resolver for all cache subsystems.

Resolution order:

1. `--cache-dir <path>`
2. nearest ancestor of the entry file containing `flux.toml` -> `<root>/target/flux/`
3. nearest ancestor of the entry file containing `Cargo.toml` -> `<root>/target/flux/` (development fallback)
4. fallback -> `<entry-dir>/.flux/cache/`

Directory layout:

```text
<cache-root>/
  interfaces/
    <module>-<pathhash>.flxi
  vm/
    <module>-<pathhash>-<cachekey>.fxm
  native/
    <module>-<pathhash>-<cachekey>.<ext>
  tmp/
```

Rules:

- new cache artifacts must not be written beside source files
- legacy source-adjacent `.flxi` may be read temporarily during migration only
- all path logic must go through one shared resolver

## Invalidation algorithm

For each module `M` in topological order:

1. Detect whether `M` itself changed
   - source hash changed
   - semantic config hash changed
   - compiler/cache format version changed

2. If `M` changed
   - recompile `M`
   - regenerate `.flxi`
   - compare old vs new interface fingerprint

3. If the fingerprint is unchanged
   - keep dependents valid
   - regenerate backend artifacts only for `M`

4. If the fingerprint changed
   - invalidate direct dependents
   - propagate invalidation transitively

5. For any unchanged module
   - if `.flxi` is valid, preload semantic interface
   - if backend artifact is valid, reuse it
   - skip recompilation only when the backend path supports true module reuse

## Public/private change semantics

### Private change

A private change means the exported interface fingerprint is unchanged.

Examples:

- private helper body changes
- private helper rename
- private helper type change that does not alter public exports

Required behavior:

- changed module recompiles
- dependents stay valid

### Public change

A public change means the exported interface fingerprint changes.

Examples:

- visibility changes from private to public
- new public export
- removed public export
- public signature changes
- exported ADT/effect changes

Required behavior:

- changed module recompiles
- dependents invalidate transitively

## Backend-specific compilation model

### VM path

Target model:

```text
for module in topo_order:
    if valid .flxi and valid .fxm:
        preload interface
        hydrate cached VM artifact
        skip source compilation
    else:
        compile module
        store .flxi
        store .fxm
link/hydrate final program
run VM
```

This should be the first backend to gain true incremental compile skipping.

### LLVM/native path

Current native compilation still rebuilds a merged whole-program AST/program and re-runs HM and lowering over that merged program.

Because of that, true native incremental compilation is a larger architectural change.

Target model:

```text
for module in topo_order:
    if valid .flxi and valid native artifact:
        preload interface
        reuse native artifact
    else:
        compile/lower module
        store .flxi
        store native artifact
link native artifacts
run binary
```

This requires:

- per-module native lowering
- per-module native artifact production
- final link/composition step that does not require re-lowering every module from source each run

## CLI changes

### Existing
- `--no-cache`
  - disables all cache reads/writes

### New
- `--cache-dir <path>`
  - overrides cache root

### Future
- `--rebuild`
  - bypass cache reads but write fresh artifacts
- `flux clean`
  - remove the resolved cache root
- improved cache inspection commands for per-module artifacts

## Logging and diagnostics

Verbose mode must distinguish:

- `module: unchanged` — source unchanged, all caches valid, compilation skipped entirely
- `module: compiled` — source changed or cache invalid, module recompiled
- `module: skipped` — module skipped due to upstream error cascade
- `interface: hit [abi:<hash>]` — loaded from cached `.flxi`, showing ABI fingerprint
- `interface: rebuilt [abi:<hash>]` — freshly compiled, new interface generated
- `interface: stored` — `.flxi` written to cache
- `vm-artifact: hit` — loaded from cached `.fxm`
- `vm-artifact: miss` — cache invalid, will recompile
- `vm-artifact: stored` — `.fxm` written to cache
- `native-artifact: hit` — loaded from cached native artifact
- `native-artifact: miss` — cache invalid, will recompile
- `native-artifact: stored` — native artifact written to cache

The ABI fingerprint in interface logs allows quick visual confirmation of whether a recompile actually changed the public interface.

Logs must print actual cache artifact paths, not just source module paths.

This matters because a module may compile, store its interface, and then have that interface loaded later in the same run by a dependent. The `compiled` vs `hit` distinction must be unambiguous — a module must never appear as both "compiled" and "cached" in confusing order within the same run.

## Phases

### Phase 1 — Centralized cache root and layout ✅ Complete

Implemented in `src/cache_paths.rs`:

- ✅ shared cache root resolver (`resolve_cache_root()` + `CacheLayout` struct)
- ✅ `--cache-dir` CLI flag (both `--cache-dir <path>` and `--cache-dir=<path>`)
- ✅ centralized `interfaces/`, `vm/`, `native/` subdirectories under cache root
- ✅ no new source-adjacent `.flxi` writes (all writes use `cache_layout.root()`)
- ⏭️ legacy `.flxi` read fallback — not needed; codebase already fully centralized. Source-adjacent `.flxi` files in `lib/Flow/` are leftover artifacts that the compiler no longer reads and can be deleted.

### Phase 2 — Modular Aether and interface fingerprint ✅ Complete

*Subsumes Proposal 0137 (Modular Aether and Module Interfaces).*

Implemented:

- ✅ **Modular borrow inference**: `infer_borrow_modes_with_preloaded()` in `src/aether/borrow_infer.rs` accepts a pre-populated `BorrowRegistry`. Imported functions are leaf nodes with fixed signatures. The standard `infer_borrow_modes()` delegates with an empty registry.
- ✅ **Extended `.flxi`**: `ModuleInterface` in `src/types/module_interface.rs` includes `cache_format_version`, `semantic_config_hash`, `interface_fingerprint`, `dependency_fingerprints`, schemes, and borrow signatures. Format version constant: `MODULE_INTERFACE_FORMAT_VERSION = 2`.
- ✅ **Interface fingerprint**: `compute_interface_fingerprint()` in `src/bytecode/compiler/module_interface.rs` computes SHA-256 from canonically sorted public schemes + borrow signatures.
- ✅ **Dependency fingerprint recording**: Each `.flxi` records `DependencyFingerprint` entries (module name, source path, interface fingerprint) for its imports. `dependency_fingerprints_match()` validates them.
- ✅ **Public/private change invalidation**: `module_interface_changed()` compares old vs new fingerprints. Dependents are marked for rebuild via `must_rebuild_due_to_dependency` in `src/main.rs`.
- ✅ **BorrowSignature serialization**: `BorrowSignature`, `BorrowMode`, `BorrowProvenance` all derive `Serialize`/`Deserialize`.

### Phase 3 — VM incremental module skipping ✅ Complete

*Subsumes the VM-specific parts of Proposal 0128 (Incremental Compilation).*

Implemented:

- ✅ **Per-module compile skipping**: When `.flxi` and `.fxm` are both valid and no dependency interface changed, the module loop calls `preload_module_interface` + `hydrate_cached_module_bytecode` and skips compilation entirely (`src/main.rs:1870-1909`).
- ✅ **Rebuild changed modules only**: `must_rebuild_due_to_dependency` checks whether any imported module's `interface_changed` flag is set (`src/main.rs:1842-1846`). Per-module state tracked via `ModuleBuildState` struct.
- ✅ **Dependent invalidation driven by `.flxi`**: After recompiling, `module_interface_changed()` compares old vs new fingerprints (`src/main.rs:2042-2046`). The `interface_changed` flag propagates transitively through the topological order.
- ✅ **Hydrate gap fixed**: `preload_module_interface` (populates `cached_member_schemes` from `.flxi`) is called **before** `hydrate_cached_module_bytecode` (restores bytecode). Schemes come from `.flxi`, bytecode from `.fxm` — two separate artifacts, correct ordering.
- ✅ **Legacy source-adjacent `.flxi` fallback removed**: All interface loading goes through centralized `cache_paths::interface_cache_path()`. No source-adjacent read path exists.

### Phase 4 — Parallel module compilation ✅ Complete

*Subsumes the parallelism goal from Proposal 0128.*

Implemented:

- ✅ **Level-based parallel compilation**: `ModuleGraph::topo_levels()` in `src/syntax/module_graph/mod.rs:179-211` groups modules by dependency level. Each level is compiled with `rayon`'s `.par_iter()` — both VM path (`src/main.rs:460-478`) and native path (`src/main.rs:878-897`). Levels are processed sequentially; modules within each level compile in parallel.
- ✅ **Per-module `Compiler` construction**: `build_module_compiler()` in `src/main.rs:120-146` creates a fresh `Compiler::new_with_interner()` per module. Each compiler gets its own symbol table, constants, and scopes. The `Interner` is cloned (read-only) across modules.
- ✅ **Interface synchronization**: Dependency interfaces are shared via immutable `&HashMap<PathBuf, ModuleInterface>`. `preload_module_interface()` copies data into per-module compiler state. No `Arc`/`Mutex`/`DashMap` needed — sequential level ordering ensures dependencies are complete before dependents compile.
- ✅ **Test coverage**: `topo_levels_group_independent_dependencies()` in `src/syntax/module_graph/module_graph_test.rs:233-289` validates correct level grouping.

### Phase 5 — Native per-module artifacts ✅ Complete

*Subsumes the LLVM-specific parts of Proposal 0128.*

Implemented:

- ✅ **Per-module Core -> Aether -> LIR -> LLVM lowering**: `lower_to_lir_llvm_module_per_module()` in `src/bytecode/compiler/mod.rs:2973` lowers each module individually. `build_native_extern_symbols()` (mod.rs:701-780) builds the extern symbol map for cross-module references. The `--emit-llvm` flag still uses whole-program merge as the debug surface (intentional).
- ✅ **Per-module object file caching**: `NativeModuleCache` in `src/core_to_llvm/module_cache.rs` stores per-module `.o`/`.obj` files under `<cache-root>/native/` with associated `.fno` metadata files tracking compiler version, cache key, dependency fingerprints, and optimization level.
- ✅ **Linker step**: `link_objects()` in `src/core_to_llvm/pipeline.rs:217-223` merges per-module object files via system linker (cc/clang/link.exe) into the final binary.
- ✅ **Cross-module reference handling**: Imported functions are declared as external symbols mangled as `flux_<Module>_<name>`. LIR lowering emits `CallKind::DirectExtern` for cross-module calls and `MakeExternClosure` for cross-module closures. The system linker resolves these at link time.
- ✅ **Parallel native compilation**: `compile_native_modules_parallel()` in `src/main.rs:2327` uses `rayon` `.par_iter()` per topological level, matching Phase 4's VM parallelism.

### Phase 6 — Tooling and parity hardening ✅ Complete

*Integrates with Proposal 0138 (Flux Parity Ways and Differential Validation).*

- ✅ **Parity observation of artifacts**: `src/parity/runner.rs` supports `VmCached` and `LlvmCached` ways that warm caches, re-run, and compare fresh vs cached output. `scripts/check_parity.sh --extended` runs `vm_cached`, `vm_strict`, `llvm_strict` ways.
- ✅ **Cache inspection commands**: `flux cache-info`, `flux module-cache-info`, `flux native-cache-info`, `flux cache-info-file`, `flux interface-info` all implemented in `src/main.rs:1266-1327`. Verbose mode shows compiler version, format version, dependency fingerprint statuses.
- ⚠️ **Explicit cache mismatch classification**: Cache miss reasons are reported (`DependencyFingerprintMismatch(path)`) but don't break down into specific sub-reasons (source hash vs config hash vs compiler version). Enhance to show exactly which field of which dependency changed.
- ✅ **Regression coverage for private/public invalidation**: `tests/cache_invalidation_tests.rs` — 6 tests covering: private body change preserves fingerprint, new public export changes fingerprint, removed public export changes fingerprint, private-to-public visibility change, comment-only change preserves fingerprint, private helper addition preserves fingerprint.
- ✅ **`flux clean` command**: `flux clean [<file.flx>]` resolves the cache root and removes it. Uses `resolve_cache_layout` so `--cache-dir` is respected.
- ⚠️ **VM == LLVM parity with caching**: Infrastructure ready (`src/parity/cli.rs:183-218` compares cached vs fresh), but extended parity suite not yet running in CI.

## Drawbacks

1. **More artifacts**
   - Per-module caches increase disk usage compared to a single `.fxc`.

2. **More invalidation logic**
   - Interface fingerprint design must be stable and deterministic.

3. **Native backend work is substantial**
   - True LLVM incremental compilation requires architectural refactoring.

4. **Migration complexity**
   - Temporary support for legacy source-adjacent `.flxi` adds transitional complexity.

## Rationale and alternatives

### Why not timestamps?
Timestamps are simpler but too fragile. Content hashes and interface fingerprints are more robust and easier to reason about.

### Why not let backend artifacts define invalidation?
That would allow VM and LLVM to drift semantically. Flux should keep invalidation tied to `Core`-level semantics, not backend payloads.

### Why not keep `.fxc` as the long-term main cache?
`.fxc` is whole-program and too coarse for incremental compilation. It may remain temporarily for compatibility, but it is not the target architecture.

### Why not do native incremental compilation first?
The VM path already has per-module artifact infrastructure (`.fxm`), so it is the lowest-risk backend for first true module skipping. Native requires a larger refactor.

## Testing strategy

### Cache root/layout
- entry inside `flux.toml` project resolves to `target/flux`
- entry inside Cargo project (development) resolves to `target/flux`
- standalone entry resolves to `.flux/cache`
- `--cache-dir` overrides all

### Interface placement
- `.flxi` written under `interfaces/`
- no new source-adjacent `.flxi`
- legacy `.flxi` can still be read during migration

### Semantic invalidation
- private helper body change:
  - changed module recompiles
  - dependents remain valid
- new public export:
  - dependents invalidate
- public signature change:
  - dependents invalidate
- comment/formatting-only change:
  - changed module recompiles
  - dependents remain valid if fingerprint unchanged

### VM behavior
- unchanged second run reuses `.fxm`
- fresh VM == cached VM on parity fixtures

### Native behavior
Before native artifacts:
- centralized `.flxi` works
- logs accurately show semantic reuse but not compile skipping

After native artifacts:
- unchanged second run reuses native module artifacts
- fresh LLVM == cached LLVM
- VM == LLVM across maintained parity corpus

### Failure handling
- corrupt `.flxi` -> cache miss, not crash
- corrupt `.fxm` -> cache miss, not crash
- corrupt native artifact -> cache miss, not crash

## Unresolved questions

1. What exact payload should the native per-module artifact use first:
   - lowered module IR
   - LLVM IR text
   - object file

## Resolved questions

1. **Should borrow signatures always be part of interface fingerprint?**
   Yes. Borrow signatures affect Aether dup/drop insertion in downstream modules. If a dependency changes from `Borrowed` to `Owned`, the downstream module's Aether pass produces different code. Making this conditional adds complexity for marginal savings.

2. **Should `.fxc` remain as a temporary warm-run cache during migration?**
   Yes. The `.fxc` whole-program cache is the fastest path for the common case (nothing changed, run immediately). It should coexist with per-module caching as a warm-start optimization. Deprecate only after per-module caching is proven equivalent in speed for warm runs.

## Relationship to prior proposals

This proposal supersedes and integrates:

- **Proposal 0128 (Incremental Compilation)**: 0128 defined the high-level goal and outlined 5 phases. This proposal provides the concrete implementation plan. 0128's Phase 1 (wire `.flxi`) maps to Phase 2-3 here. 0128's Phase 2 (per-module bytecode + linker) maps to Phase 3. 0128's Phase 3 (interface stability) maps to Phase 2. 0128's Phase 4 (parallel compilation) maps to Phase 4. 0128's Phase 5 (LLVM incremental) maps to Phase 5.

- **Proposal 0137 (Modular Aether and Module Interfaces)**: 0137 defined modular borrow inference and `.flxi` interface caching. Its Phase 1 (ModuleInterface type) and Phase 2 (emit interfaces) are already implemented. Phase 3 (load interfaces) and Phase 4 (modular borrow inference) are integrated into Phase 2 of this proposal. Phase 5 (type inference caching integration) is integrated into Phase 3.

This proposal builds on:

- **Proposal 0121 (Separate Module Compilation)**: Phases 1 & 4 are already implemented — the per-module compilation loop, module graph, and `.flxi`/`.fxm` infrastructure all exist. This is the foundation that 0139 extends.

This proposal integrates with:

- **Proposal 0138 (Flux Parity Ways)**: Parity validation is Phase 6. The parity harness should verify that cached compilation produces identical output to fresh compilation across both backends.

## Recommendation

Adopt this proposal in staged order:

1. centralized cache root/layout
2. modular Aether + interface fingerprint invalidation
3. VM incremental module skipping
4. parallel module compilation
5. native per-module artifact architecture
6. tooling/parity hardening

This is the lowest-risk path that yields real incremental compilation early without forcing a premature native backend rewrite. Phases 1-3 deliver the core value. Phase 4 adds throughput. Phase 5 is a separate architectural effort. Phase 6 hardens everything.
