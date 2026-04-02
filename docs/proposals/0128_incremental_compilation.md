- Feature Name: Incremental Compilation
- Start Date: 2026-03-26
- Status: Superseded by Proposal 0139 (Incremental Module Caching)
- Proposal PR:
- Flux Issue:
- Depends on: Proposal 0121 (Separate Module Compilation)

## Summary

Add incremental compilation to Flux so that changing one module only recompiles that module and its dependents — not the entire program. Today, touching any source file invalidates the whole-program `.fxc` cache and triggers a full rebuild. After this proposal, each module compiles independently to its own cached artifact, and unchanged modules are loaded from cache in milliseconds.

This follows the proven model used by GHC (`.hi` interface files) and Koka (`.kki` interface files), adapted to Flux's existing infrastructure — 80% of which is already built but not yet wired together.

## Motivation

### Current compilation model

Flux compiles all modules into a single bytecode blob cached as one `.fxc` file:

```
Source → Module Graph → Compile ALL modules sequentially → Single .fxc
```

If any source file changes (even a comment), the entire cache is invalidated:

```
$ cargo run -- examples/aoc/2024/day06.flx    # compiles 7 modules (1.2s)
$ touch lib/Flow/List.flx                      # change one module
$ cargo run -- examples/aoc/2024/day06.flx    # recompiles ALL 7 modules (1.2s)
```

### Target compilation model

Each module compiles independently and caches its own bytecode + interface:

```
Source → Module Graph → For each module:
                          if cached & deps unchanged → load from cache
                          else → compile → cache
                        → Link bytecodes → Execute
```

After the change:

```
$ cargo run -- examples/aoc/2024/day06.flx    # compiles 7 modules (1.2s)
$ touch lib/Flow/List.flx                      # change one module
$ cargo run -- examples/aoc/2024/day06.flx    # recompiles 1 module (0.2s)
```

### Why now

1. **Flow.Array module** adds a 7th standard library module — rebuild times grow linearly with module count
2. **Proposal 0123 (type classes)** will add constraint solving per module — making each module slower to compile
3. **AoC examples** import 6+ Flow modules that rarely change — recompiling them every time wastes cycles
4. **LLVM native backend** recompiles everything on every run (no caching at all)

### Prior art

| Compiler | Interface file | Change detection | Per-module cache | Parallel |
|----------|---------------|-----------------|-----------------|----------|
| **GHC** | `.hi` (binary, rich) | MD5 fingerprint per export | `.hi` + `.o` | Yes (`-j`) |
| **Koka** | `.kki` (Core + types) | File timestamps | `.kki` + `.c`/`.js` | Yes (phase-level) |
| **OCaml** | `.cmi` (binary) | File timestamps + hash | `.cmi` + `.cmo` | Yes (ocamldep) |
| **Rust** | Crate metadata | Content hash + dep graph | Per-crate `.rlib` | Yes (cargo -j) |
| **Flux** | `.flxi` (exists, unused) | SHA-256 per source | Whole-program `.fxc` only | No |

---

## Guide-level explanation

### For users

No new flags required. Incremental compilation is automatic:

```bash
cargo run -- examples/aoc/2024/day06.flx       # first build: compiles everything
cargo run -- examples/aoc/2024/day06.flx       # cache hit: instant
vim lib/Flow/List.flx                           # edit one module
cargo run -- examples/aoc/2024/day06.flx       # recompiles Flow.List + Day06Solver only
```

The `--no-cache` flag continues to bypass all caching. A new `--rebuild` flag forces recompilation of all modules while still writing caches.

Cache files live in `target/flux/modules/`:

```
target/flux/modules/
  Flow.Option-a1b2c3d4.fxm      # per-module bytecode + interface
  Flow.List-e5f6g7h8.fxm
  Flow.Array-i9j0k1l2.fxm
  Flow.String-m3n4o5p6.fxm
  Day06Solver-q7r8s9t0.fxm
  day06-u1v2w3x4.fxm            # entry module
```

### For compiler contributors

The compilation pipeline changes from:

```
for module in topo_order:
    compile(module)          # accumulates into single Compiler
bytecode = compiler.emit()   # one blob
cache.store(program, bytecode)
```

To:

```
for module in topo_order:
    if module.cache_valid():
        load_interface(module)   # populate type schemes from .fxm
    else:
        compile(module)          # compile fresh
        store_module_cache(module)  # write .fxm
link(all_modules)                # merge bytecodes, resolve addresses
```

---

## Reference-level explanation

### Phase 1 — Wire up `.flxi` interface loading

**Goal**: When compiling module B that imports module A, load A's type information from a cached interface instead of recompiling A from source.

**What exists today (unused):**

`src/bytecode/compiler/module_interface.rs` already defines:

```rust
pub struct ModuleInterface {
    pub module_name: String,
    pub contracts: Vec<InterfaceContract>,
    pub visibility: HashMap<String, bool>,
    pub source_hash: String,
}
```

And `build_interface()` populates it from the compiler state. But `load_valid_interface()` is never called during compilation.

**Changes:**

1. **Extend `ModuleInterface`** to include type schemes:

```rust
pub struct ModuleInterface {
    pub module_name: String,
    pub source_hash: [u8; 32],
    pub dependency_hashes: Vec<(String, [u8; 32])>,
    pub contracts: Vec<InterfaceContract>,
    pub visibility: HashMap<String, bool>,
    pub type_schemes: Vec<(String, SerializedScheme)>,  // NEW
    pub adt_definitions: Vec<SerializedAdt>,             // NEW
    pub effect_declarations: Vec<SerializedEffect>,      // NEW
}
```

2. **Save interface after compiling each module** (not just entry):

```rust
// In the module compilation loop (src/main.rs):
for node in ordered_nodes {
    let iface_path = module_cache_path(&node.path);

    if let Some(iface) = load_and_validate_interface(&iface_path, &node) {
        // Cache hit: populate compiler state from interface
        compiler.load_cached_member_schemes(&iface.type_schemes);
        compiler.load_cached_contracts(&iface.contracts);
        compiler.load_cached_adts(&iface.adt_definitions);
        continue;  // skip compilation
    }

    // Cache miss: compile from source
    compiler.compile_module(&node);

    // Save interface for next time
    let iface = compiler.build_interface(&node);
    save_interface(&iface_path, &iface);
}
```

3. **Validate interfaces** using dependency hashes:

```rust
fn validate_interface(iface: &ModuleInterface, node: &ModuleNode) -> bool {
    // Source hash must match
    if iface.source_hash != hash_file(&node.path) {
        return false;
    }
    // All dependency hashes must match
    for (dep_path, dep_hash) in &iface.dependency_hashes {
        if hash_file(dep_path) != *dep_hash {
            return false;
        }
    }
    true
}
```

**GHC reference**: `GHC/Iface/Recomp.hs` (`checkOldIface`) validates `.hi` files by checking: source hash, flag hash, dependent module hashes, and per-export fingerprints. Flux Phase 1 uses source+dependency hashes — sufficient for correctness, but without GHC's per-export granularity.

**Koka reference**: `src/Compile/Build.hs` (`moduleLoad`) checks `source_time > iface_time` and loads from `.kki` when the interface is newer. Flux uses hash comparison instead of timestamps — more robust against clock skew.

**Files**: `src/bytecode/compiler/module_interface.rs`, `src/main.rs`

**Impact**: Unchanged modules skip parsing, type inference, and Core lowering. Only the final bytecode link step runs. Expected speedup: 3-10x for typical edits.

### Phase 2 — Per-module bytecode caching

**Goal**: Each module produces its own `.fxm` file containing bytecode + interface + debug info. The entry point links them together.

**Current state**: The `Compiler` maintains a single `instructions: Vec<u8>` and `constants: Vec<Value>` that all modules write into. There's no boundary between modules in the output.

**Changes:**

1. **Per-module bytecode segments**:

```rust
pub struct ModuleByteCode {
    pub module_name: String,
    pub constants: Vec<Value>,          // module-local constants
    pub instructions: Vec<u8>,          // module-local bytecode
    pub exports: Vec<ExportedSymbol>,   // (name, local_offset)
    pub imports: Vec<ImportedSymbol>,   // (name, source_module) — unresolved
    pub interface: ModuleInterface,     // type info for dependents
}

pub struct ExportedSymbol {
    pub name: String,
    pub kind: SymbolKind,  // Function, Adt, Constant
    pub offset: u32,       // offset within this module's bytecode
    pub arity: u8,
}

pub struct ImportedSymbol {
    pub name: String,
    pub source_module: String,
    pub kind: SymbolKind,
}
```

2. **Bytecode linker**:

```rust
fn link_modules(modules: &[ModuleByteCode]) -> LinkedProgram {
    let mut linked_instructions = Vec::new();
    let mut linked_constants = Vec::new();
    let mut symbol_table: HashMap<String, u32> = HashMap::new();

    // Pass 1: Assign global offsets
    let mut instruction_offset = 0u32;
    let mut constant_offset = 0u32;
    for module in modules {
        for export in &module.exports {
            let global_offset = export.offset + instruction_offset;
            symbol_table.insert(
                format!("{}.{}", module.module_name, export.name),
                global_offset,
            );
        }
        instruction_offset += module.instructions.len() as u32;
        constant_offset += module.constants.len() as u32;
    }

    // Pass 2: Relocate and merge
    for module in modules {
        let base = linked_instructions.len() as u32;
        let mut relocated = module.instructions.clone();
        relocate_imports(&mut relocated, &module.imports, &symbol_table, base);
        linked_instructions.extend(relocated);
        linked_constants.extend(module.constants.iter().cloned());
    }

    LinkedProgram { instructions: linked_instructions, constants: linked_constants, symbol_table }
}
```

3. **Cache format** (`.fxm` — Flux Module):

```
MAGIC (4 bytes: "FXMD")
FORMAT_VERSION (u16)
module_name (string)
source_hash (32 bytes)
dependency_hashes (vec of (string, 32 bytes))
interface_data (serialized ModuleInterface)
constants_count + constants
instructions_len + instructions
exports_count + exports
imports_count + imports
```

**GHC reference**: GHC produces separate `.hi` (interface) and `.o` (object code) files. The linker (`GHC/Driver/Pipeline.hs`) merges `.o` files using the system linker. Flux's approach is simpler — bytecode linking is just concatenation + offset relocation, not machine code linking.

**Koka reference**: Koka generates backend-specific files (`.c`, `.js`) per module, then compiles and links them via the backend toolchain. Koka doesn't do bytecode linking — it delegates to `cc` or `node`.

**Files**: new `src/bytecode/linker.rs`, `src/bytecode/bytecode_cache/module_cache.rs`, updates to `src/bytecode/compiler/pipeline.rs`

**Impact**: Changing one module only recompiles that module. The linker merges cached bytecodes in microseconds.

### Phase 3 — Interface stability detection

**Goal**: If module A's source changes but its exported interface doesn't change (e.g., only a private function body changed), don't recompile modules that depend on A.

**Implementation:**

Add an **interface hash** — a hash of only the exported signatures, not the full source:

```rust
pub struct ModuleInterface {
    // ... existing fields ...
    pub interface_hash: [u8; 32],  // NEW: hash of exports only
}

fn compute_interface_hash(iface: &ModuleInterface) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for contract in &iface.contracts {
        hasher.update(contract.name.as_bytes());
        hasher.update(&contract.type_signature_bytes());
    }
    for (name, visible) in &iface.visibility {
        hasher.update(name.as_bytes());
        hasher.update(&[*visible as u8]);
    }
    for scheme in &iface.type_schemes {
        hasher.update(scheme.0.as_bytes());
        hasher.update(&scheme.1.to_bytes());
    }
    hasher.finalize().into()
}
```

**Recompilation decision:**

```
Module A changed → recompile A → compute new interface_hash
  if interface_hash unchanged → dependents are still valid, skip
  if interface_hash changed → invalidate dependents, recompile
```

**GHC reference**: GHC's `GHC/Iface/Recomp.hs` uses per-export **ABI fingerprints**. Each exported entity (function, type, class) gets its own MD5 hash. If module B uses only `foo` from module A, and A changes `bar` but not `foo`, B is not recompiled. This is more granular than Flux's whole-interface hash but significantly more complex.

**Koka reference**: Koka uses file timestamps only — no interface stability tracking. If A's source is newer than A's `.kki`, A is recompiled and all dependents are invalidated. Flux's approach (interface hash) is between Koka (no stability) and GHC (per-export fingerprints).

**Files**: `src/bytecode/compiler/module_interface.rs`

**Impact**: Refactoring internal implementation of a library module doesn't cascade to dependents. This is the biggest win for standard library development.

### Phase 4 — Parallel module compilation

**Goal**: Compile independent modules concurrently using `rayon`.

**Prerequisites**: Phase 2 (per-module bytecode) must be complete. Each module must compile independently — no shared mutable `Compiler` state.

**Architecture change:**

```rust
// Before (sequential, shared Compiler):
let mut compiler = Compiler::new();
for node in topo_order {
    compiler.compile_module(node);  // mutates shared state
}

// After (parallel, per-module compilers):
let interfaces: DashMap<ModuleId, ModuleInterface> = DashMap::new();
let module_bytecodes: DashMap<ModuleId, ModuleByteCode> = DashMap::new();

// Group modules by topological level (independent modules at same level)
let levels = topo_levels(&module_graph);

for level in levels {
    level.par_iter().for_each(|node| {
        // Each module gets its own compiler instance
        let mut compiler = Compiler::new();

        // Load dependency interfaces (already compiled at prior levels)
        for dep in &node.imports {
            if let Some(iface) = interfaces.get(&dep.target) {
                compiler.load_interface(&iface);
            }
        }

        // Compile module
        compiler.compile_module(node);

        // Store results
        interfaces.insert(node.id, compiler.build_interface());
        module_bytecodes.insert(node.id, compiler.emit_module_bytecode());
    });
}

// Link (sequential, fast)
let linked = link_modules(&module_bytecodes);
```

**GHC reference**: GHC's `GHC/Driver/Make.hs` (`upsweep`) compiles modules in topological order. With `-j N`, independent modules at the same level compile in parallel using GHC's thread pool. Each module gets its own `HscEnv` (compiler environment).

**Koka reference**: Koka uses 5 phase-level barriers with MVars. Modules at different phases can execute concurrently — e.g., module A can be in codegen while module B is type-checking, as long as B doesn't depend on A's codegen output. This is more fine-grained than GHC's level-based parallelism.

**Flux approach**: Level-based parallelism (like GHC). Simpler than Koka's phase-level approach but still effective. With 6 Flow modules at the same dependency level, we get 6x parallelism.

**Dependencies**: Add `rayon` to `Cargo.toml`. Refactor `Compiler` to be constructable per-module (currently it accumulates state across modules).

**Files**: `src/main.rs` (compilation loop), `src/bytecode/compiler/mod.rs` (per-module construction)

**Impact**: On an 8-core machine, compiling 6 independent Flow modules takes ~1/6 the time. The entry module still compiles sequentially (it depends on everything).

### Phase 5 — LLVM backend incremental compilation

**Goal**: Extend incremental compilation to the `core_to_llvm` native backend.

**Current state**: The LLVM backend recompiles everything on every run. No caching at all. The temp directory (`flux_core_to_llvm_<pid>`) is created fresh each time.

**Changes:**

1. **Per-module LLVM IR caching**: Each module's Core IR → LLVM IR translation is cached as a `.ll` file
2. **Per-module object file caching**: Each module's `.ll` → `.o` compilation is cached
3. **Incremental linking**: Only re-link when any `.o` file changes

```
target/flux/native/
  Flow.Option-a1b2c3d4.o
  Flow.List-e5f6g7h8.o
  Flow.Array-i9j0k1l2.o
  Day06Solver-q7r8s9t0.o
  day06-u1v2w3x4.o
  program                    # linked binary
```

**GHC reference**: GHC produces `.o` files per module and uses the system linker. Unchanged modules keep their `.o` files. Only the final link step re-runs. This is exactly what Flux should do.

**Files**: `src/core_to_llvm/pipeline.rs`

---

## Timeline

| Phase | Feature | Effort | Speedup |
|-------|---------|--------|---------|
| 1 | Wire `.flxi` interface loading | 2-3 days | 3-5x (skip type inference for cached modules) |
| 2 | Per-module bytecode + linker | 5-7 days | 5-10x (skip everything for cached modules) |
| 3 | Interface stability detection | 1-2 days | Prevents false cascading invalidation |
| 4 | Parallel module compilation | 3-5 days | Linear in core count for independent modules |
| 5 | LLVM backend incremental | 3-5 days | Same gains for native compilation |

Phase 1 alone gives the biggest bang for the buck. Phase 2 completes the picture. Phases 3-5 are optimizations.

---

## Drawbacks

- **Cache disk usage**: Per-module caches use more disk space than a single whole-program cache. Mitigated by: `.fxm` files are small (typically 10-50 KB each), and `target/flux/` can be cleaned.

- **Linker complexity**: The bytecode linker adds a new component that must handle symbol resolution, offset relocation, and cross-module references correctly. Mitigated by: bytecode linking is much simpler than machine code linking (no relocation types, no PLT/GOT).

- **Cache coherence bugs**: If the interface format changes but old caches aren't invalidated, compilation may fail silently. Mitigated by: format version in cache header, compiler version check.

- **Parallel compilation refactor**: The current `Compiler` struct accumulates state across modules. Making it per-module-constructable requires splitting global state (constants, ADT registry) from per-module state. This is a moderate refactoring effort.

## Rationale and alternatives

### Why not whole-program caching with better invalidation?

The current approach (hash the entire program, cache the entire bytecode) could be improved by hashing each module's source separately and only invalidating if a transitive dependency changed. But this still requires recompiling all downstream modules — it doesn't help when a leaf module changes.

### Why not file-timestamp based detection (like Koka)?

Timestamps are fragile — `git checkout`, NFS, Docker layer caching, and CI systems can produce incorrect timestamps. SHA-256 content hashing is deterministic and immune to clock skew. The overhead is negligible (~1ms per file).

### Why not per-export fingerprinting (like GHC)?

GHC's per-export ABI fingerprints are the most granular approach — changing a private function doesn't cascade at all, and changing one export only cascades to modules that use that specific export. This is more complex to implement (requires tracking which exports each module uses). Flux should start with whole-interface hashing (Phase 3) and add per-export granularity later if needed.

### Why `rayon` for parallelism instead of `async`/`tokio`?

Module compilation is CPU-bound, not IO-bound. `rayon`'s work-stealing thread pool is designed for CPU parallelism. `tokio` would add complexity (async compiler functions) for no benefit.

## Prior art

- **GHC**: `.hi` interface files + MD5 fingerprints per export + `--make` mode for automatic dependency tracking. The gold standard for incremental compilation in functional languages. 30+ years of refinement.

- **Koka**: `.kki` interface files + file timestamps + 5-phase concurrent pipeline with MVar barriers. Well-designed for IDE responsiveness via `BuildContext` in-memory cache.

- **OCaml**: `.cmi` interface files + `ocamldep` for dependency tracking. Separate compilation is fundamental to OCaml's design. No built-in parallelism in the compiler itself (delegated to build systems like `dune`).

- **Rust/Cargo**: Per-crate compilation with content hashing. Cargo's incremental compilation also works within a crate via query-based demand-driven compilation. This is more complex than Flux needs.

## Unresolved questions

- **Should the bytecode linker support lazy loading?** Instead of merging all module bytecodes at startup, load modules on first access. This would improve startup time for programs that don't use all imported modules. Deferred to a future proposal.

- **Should `.fxm` files be binary or human-readable?** Binary is faster to load/save. Human-readable (JSON/YAML) is easier to debug. Recommendation: binary with a `--dump-module-cache` flag for inspection.

- **How to handle the auto-prelude?** Flow modules are injected by `inject_flow_prelude`. Their interfaces should be pre-cached in `lib/Flow/` alongside the source files, similar to Koka's pre-compiled library interfaces.

- **Cache eviction policy?** Old `.fxm` files accumulate in `target/flux/modules/`. Should the compiler clean up stale caches? GHC doesn't (users run `cabal clean`). Koka doesn't. Recommendation: don't auto-clean; provide `flux clean` command.

## Future possibilities

- **Per-export fingerprinting**: Track which exports each module uses. Only recompile when a used export changes. Matches GHC's granularity.
- **Query-based compilation**: Like Rust's incremental, demand-driven compilation where individual functions are the compilation unit. Much more complex but maximal incrementality.
- **IDE integration**: Expose per-module type information via `.flxi` files for LSP hover/completion. Koka's `BuildContext` model is a good reference.
- **Distributed caching**: Share `.fxm` files across CI builds via content-addressed storage. If the hash matches, skip compilation entirely. Similar to Bazel's remote cache.
- **Hot module reloading**: In a REPL or development server, reload only changed modules without restarting. Requires the bytecode linker to support re-linking at runtime.
