- Feature Name: Modular Aether Borrow Inference & Module Interface Caching
- Start Date: 2026-03-31
- Proposal PR:
- Flux Issue:

## Summary
[summary]: #summary

Make Aether borrow inference modular by exporting `BorrowSignature` as part of a serializable module interface (`ModuleInterface`), alongside existing type `Scheme`s. This eliminates whole-program borrow analysis, enables cached compilation of the standard library, and unblocks future parallel module compilation.

## Motivation
[motivation]: #motivation

Today, every invocation of `cargo run -- foo.flx` re-parses, re-infers types, and re-runs Aether borrow analysis for all of `lib/Base/` (6 modules: List, Option, String, IO, Assert, Numeric). This work is redundant — the standard library never changes between user compilations.

The root cause is that `infer_borrow_modes()` in `src/aether/borrow_infer.rs` requires the entire `CoreProgram` — every function definition must be visible so it can compute strongly connected components (Tarjan's SCC) and run fixed-point constraint solving across mutually recursive groups. There is no way to supply pre-computed borrow information for imported modules.

This has three consequences:

1. **Slow compilation**: The standard library is compiled from scratch on every run. Type inference and Aether analysis for 6 Base modules is repeated unnecessarily.

2. **No parallel module compilation**: Even though Dup/Drop insertion, reuse analysis, and drop specialization are all per-function (they only read the `BorrowRegistry`, never modify it), the borrow inference phase serializes everything.

3. **No separate compilation**: Adding a package system or larger standard library will make this worse linearly — every new imported module adds to the whole-program analysis.

### Use cases

- **User writes `examples/hello.flx`**: Today this re-compiles all of Base. With module interfaces, Base's cached `ModuleInterface` is loaded in microseconds and only `hello.flx` is compiled.

- **Multi-module project**: A project with 10 user modules that don't depend on each other could compile them in parallel, merging only at the final link step.

- **Standard library development**: When editing `lib/Base/List.flx`, only List needs re-analysis. Other Base modules use their cached interfaces.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Module interfaces

After compiling a module, the compiler writes a `.flxi` (Flux Interface) file alongside the source. This file contains:

- **Type signatures**: The `Scheme` for each exported function (already computed by type inference).
- **Borrow signatures**: The `BorrowSignature` for each exported function (computed by Aether borrow inference).
- **Source hash**: SHA-256 of the source file, used for cache invalidation.

When compiling a downstream module that imports this one, the compiler loads the `.flxi` file instead of re-parsing and re-analyzing the source.

### What changes for users

Nothing. The interface files are an implementation detail. Compilation produces the same bytecode/LLVM output. The only observable difference is speed.

For the VM backend, interface files are only half of the story. `.flxi` files let the compiler skip re-running HM and Aether for imported modules, but the VM still needs executable definitions for imported functions at runtime. In practice this means:

- `.flxi` carries semantic metadata only: exported `Scheme`s and `BorrowSignature`s
- VM execution still requires cached executable module artifacts for imported modules
- this executable caching is orthogonal to borrow metadata and does not imply any runtime notion of borrowing

### What changes for compiler contributors

Borrow inference gains a new mode: **modular inference**. Instead of requiring all definitions in a single `CoreProgram`, it accepts a `BorrowRegistry` pre-populated with signatures from imported modules. The inference then runs only on the current module's definitions, treating imported signatures as fixed.

```
Before:
  [All modules] → infer_borrow_modes(entire CoreProgram) → BorrowRegistry

After:
  [Base modules] → load .flxi → pre-populated BorrowRegistry
  [User module]  → infer_borrow_modes(user CoreProgram, pre-populated registry) → extended BorrowRegistry
```

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Phase 1: `ModuleInterface` type

Define a new serializable struct that captures a module's compiled interface:

```rust
// src/types/module_interface.rs (new file)
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleInterface {
    /// Module name (e.g., "Base.List")
    pub module_name: String,
    /// SHA-256 of the source file
    pub source_hash: String,
    /// Compiler version — invalidate on version mismatch
    pub compiler_version: String,
    /// Type schemes for exported functions: (fn_name → Scheme)
    pub schemes: HashMap<String, Scheme>,
    /// Borrow signatures for exported functions: (fn_name → BorrowSignature)
    pub borrow_signatures: HashMap<String, BorrowSignature>,
}
```

This requires adding `#[derive(Serialize, Deserialize)]` to:
- `Scheme` (`src/types/scheme.rs`)
- `InferType` (`src/types/infer_type.rs`)
- `BorrowSignature` (`src/aether/borrow_infer.rs`)
- `BorrowMode` (`src/aether/borrow_infer.rs`)
- `BorrowProvenance` (`src/aether/borrow_infer.rs`)
- `TypeVarId`, `TypeConstructor` (transitive dependencies)

### Phase 2: Emit module interfaces after compilation

After `infer_borrow_modes()` runs for a module, extract the public function signatures and write a `.flxi` file:

```rust
// In the compilation pipeline, after Aether passes complete:
fn emit_module_interface(
    module_name: &str,
    source_hash: &str,
    program: &CoreProgram,
    schemes: &HashMap<(Identifier, Identifier), Scheme>,
    registry: &BorrowRegistry,
) -> ModuleInterface {
    let mut iface = ModuleInterface::new(module_name, source_hash);
    for def in &program.defs {
        if def.is_public {
            let name = def.name.clone();
            if let Some(scheme) = schemes.get(&(module_name.into(), name.clone())) {
                iface.schemes.insert(name.clone(), scheme.clone());
            }
            if let Some(sig) = registry.by_binder.get(&def.binder_id) {
                iface.borrow_signatures.insert(name, sig.clone());
            }
        }
    }
    iface
}
```

Write to `lib/Base/.flxi_cache/List.flxi` (or similar cache directory).

### Phase 3: Load module interfaces for imports

Before compiling a module, check whether each imported module has a valid `.flxi`:

```rust
fn load_or_compile_module(module_path: &Path) -> ModuleInterface {
    let source_hash = sha256(fs::read_to_string(module_path));
    let cache_path = module_path.with_extension("flxi");

    if let Ok(cached) = ModuleInterface::load(&cache_path) {
        if cached.source_hash == source_hash && cached.compiler_version == VERSION {
            return cached;  // Cache hit — skip compilation entirely
        }
    }

    // Cache miss — compile the module, emit interface
    let (program, schemes, registry) = compile_module(module_path);
    let iface = emit_module_interface(...);
    iface.save(&cache_path);
    iface
}
```

### Phase 4: Modular borrow inference

Change `infer_borrow_modes()` to accept a pre-populated registry:

```rust
// Current signature:
pub fn infer_borrow_modes(
    program: &mut CoreProgram,
    interner: Option<&Interner>,
) -> BorrowRegistry;

// New signature:
pub fn infer_borrow_modes(
    program: &mut CoreProgram,
    interner: Option<&Interner>,
    preloaded: BorrowRegistry,  // NEW: pre-populated with imported signatures
) -> BorrowRegistry;
```

The key changes inside `infer_borrow_modes()`:

1. **Initialization**: Start from `preloaded` instead of an empty registry. Imported signatures are already in `by_name`.

2. **SCC computation**: Only compute SCCs over the current module's definitions. Imported functions are leaf nodes (their signatures are fixed, not part of any SCC).

3. **Constraint solving**: When a local function calls an imported function, look up the imported `BorrowSignature` from `by_name` directly — no need to analyze the callee's body.

4. **Unknown callees**: The existing `BorrowProvenance::Imported` path already defaults to all-`Owned`. This is the correct conservative fallback for any function not in the registry.

### Phase 5: Integrate with type inference caching

The `InferProgramConfig` struct already has `preloaded_module_member_schemes`:

```rust
// src/ast/type_infer/mod.rs:266
pub struct InferProgramConfig {
    pub preloaded_module_member_schemes: HashMap<(Identifier, Identifier), Scheme>,
    // ...
}
```

Load schemes from `ModuleInterface` into this field. Type inference for imported modules is then skipped entirely — the schemes are trusted from the cache.

### Phase 6: VM executable module caching

For the VM backend, interface reuse alone is not enough to skip imported module compilation. The VM still needs imported modules' executable artifacts:

- compiled function bodies in the constant pool
- top-level bytecode instructions that install globals
- global symbol bindings
- debug metadata needed to preserve source locations

To support true dependency reuse on the VM path, add a separate cached module artifact alongside `.flxi`:

```text
source.flx
  ├── source.flxi   # semantic interface: schemes + borrow signatures
  └── source.fxm    # executable VM module artifact: globals + constants + instructions
```

The `.fxm` artifact is append-only relative to compiler state. It stores the module-local delta that was added during compilation and can later be hydrated back into the compiler/runtime state without recompiling source. This is intentionally separate from `.flxi`:

- `.flxi` is for HM/Aether reuse
- `.fxm` is for VM execution reuse
- neither artifact introduces runtime borrow tracking

### Runtime invariant: borrowing remains compile-time only

This proposal does **not** add any runtime notion of borrowing.

- the runtime continues to see only ordinary RC operations and executable bytecode
- borrowed vs owned is decided entirely in the compiler when Aether emits or elides `Dup`/`Drop`
- `.flxi` and `.fxm` are compile-time/cache artifacts, not runtime metadata for borrow checking

Phase 6 therefore does not change the borrowing model. It only lets the VM load cached executable module state so that imported modules do not need to be recompiled when their cache artifacts are valid.

### Interaction with existing Aether phases

Only borrow inference changes. All downstream phases are unaffected:

| Phase | Change needed | Reason |
|-------|--------------|--------|
| Borrow inference (`borrow_infer.rs`) | Accept preloaded registry | Core change |
| Dup/Drop insertion (`insert.rs`) | None | Already takes `&BorrowRegistry` read-only |
| Drop specialization (`drop_spec.rs`) | None | Per-function, no cross-module data |
| Dup/Drop fusion (`fusion.rs`) | None | Per-function |
| Reuse analysis (`reuse_analysis.rs`) | None | Per-function, no cross-module data |
| Reuse specialization (`reuse_spec.rs`) | None | Per-function |
| FBIP checking (`check_fbip.rs`) | None | Per-function annotation check |

The VM executable cache introduced in Phase 6 is outside this table because it does not change any Aether phase. It only changes how previously compiled imported modules are restored on the VM path.

### Correctness argument

**Soundness**: A pre-loaded `BorrowSignature` with `Owned` for a parameter that could be `Borrowed` is conservative — it causes an extra `Dup` at the call site, but never a missing one. The existing `BorrowProvenance::Imported` path already handles this.

**Completeness**: If the cached signature says a parameter is `Borrowed`, the downstream module avoids `Dup`. This is correct as long as the cached signature matches the actual function — guaranteed by the source hash invalidation.

**Convergence**: Fixed-point iteration in `infer_borrow_modes()` only runs over the current module's SCCs. Imported signatures are constants, not variables in the constraint system. Convergence properties are unchanged.

### Cache invalidation

A `.flxi` file is valid when:
1. Source hash matches the current source file
2. Compiler version matches (borrow inference semantics may change between versions)
3. All transitive dependencies' interfaces are valid (if Base.List depends on Base.Option, Option's interface must also be valid)

For `lib/Base/` modules (which cannot cross-import per existing constraints), condition 3 is trivially satisfied.

## Drawbacks
[drawbacks]: #drawbacks

1. **New cache artifacts**: `.flxi` files add to build output. Need a `--no-cache` flag to bypass (already exists for `.fxc` bytecode cache — extend it).

2. **Serialization maintenance**: Any change to `Scheme`, `InferType`, `BorrowSignature`, or their transitive dependencies requires updating the serialization format. Mitigated by the compiler version check in the cache.

3. **Complexity**: The compilation pipeline gains a new code path (load interface vs. compile from source). This is the standard tradeoff for incremental compilation.

4. **Conservative borrow modes for imports**: If a function's true borrow mode is `Borrowed` but the interface wasn't cached yet, the first compilation defaults to `Owned`. This is safe but suboptimal. Subsequent compilations use the correct cached signature.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

### Why module interfaces over whole-program caching?

**Alternative 1: Cache the entire `BorrowRegistry` for the whole program.**
This would cache borrow results but not enable modularity. Adding a new user function would invalidate the entire cache. Module interfaces are finer-grained — adding a user function only recomputes that module.

**Alternative 2: Make borrow inference purely local (no cross-function analysis).**
This would mean every parameter defaults to `Owned` unless explicitly annotated. It's simpler but generates significantly worse code — Flux's current borrow inference finds many `Borrowed` parameters automatically, avoiding unnecessary `Dup`/`Drop` pairs.

**Alternative 3: Annotate borrow modes in Flux source (like Koka's `borrow` keyword).**
This would let users declare `fn map(xs: borrow List<a>, f: borrow (a) -> b) -> List<b>`. It eliminates the need for inference but adds language complexity. This could be a future extension layered on top of module interfaces.

### Why this design?

- It follows Koka's approach (borrow signatures as module metadata) which is proven in production.
- It reuses existing infrastructure (`preloaded_module_member_schemes`, `BorrowProvenance::Imported`, `.fxc` caching pattern).
- It's backward compatible — the compiler produces identical output, just faster.
- It's incremental — Phase 1-2 (emit interfaces) can ship before Phase 3-4 (load interfaces), allowing gradual validation.
- For the VM backend, executable module caching is kept separate from borrow metadata so the runtime model stays unchanged.

## Prior art
[prior-art]: #prior-art

### Koka (direct inspiration)

Verified against the Koka source at `github.com/koka-lang/koka` (commit history through March 2026).

#### Compiler: Borrow annotations in module interfaces

Koka stores borrow annotations in `.kki` (Koka Interface) files as part of `DefSort`:

```haskell
-- Common/Syntax.hs:311-314
data ParamInfo = Borrow | Own  deriving(Eq,Show)

-- Common/Syntax.hs:304-306
data DefSort = DefFun { defFunParamInfos :: ![ParamInfo], defFunFip :: !Fip } | DefVal | DefVar
```

The `Borrowed` environment (`Core/Borrowed.hs:49`) is a `NameMap` from function names to `([ParamInfo], Fip)`:

```haskell
newtype Borrowed = Borrowed (M.NameMap ([ParamInfo], Fip))
```

When a module is compiled, `extractBorrowed` (`Core/Borrowed.hs:90-92`) collects borrow info from all function definitions and externals in the Core IR. This info is serialized into `.kki` files — borrowed parameters are marked with `^` in function signatures:

```
-- From std_core.kki:
pub fun range/for : forall<(e :: E)> (^ start : std/core/types/int, end : std/core/types/int, ...) -> ...
```

Here `^ start` means the `start` parameter is borrowed.

#### Compiler: How Perceus uses borrow info at call sites

During RC insertion (`Backend/C/Parc.hs`), `parcBorrowApp` (lines 192-205) handles function calls:

```haskell
parcBorrowApp :: TName -> [Expr] -> Expr -> Parc Expr
parcBorrowApp tname args expr
  = do bs <- getParamInfos (getName tname)  -- Look up [ParamInfo] from Borrowed env
       if Borrow `notElem` bs
         then App expr <$> reverseMapM parcExpr args  -- No borrowed params: normal path
         else do
           let argsBs = zip args (bs ++ repeat Own)
           -- For each (arg, Borrow): skip dup, insert drop AFTER the call returns
           -- For each (arg, Own): process normally (dup if needed)
```

The lookup (`Parc.hs:928-933`) queries the `Borrowed` environment that was pre-populated from imported `.kki` files:

```haskell
getParamInfos :: Name -> Parc [ParamInfo]
getParamInfos name = do b <- borrowed <$> getEnv
                        case borrowedLookup name b of
                          Nothing -> return []
                          Just pinfos -> return pinfos
```

The Parc monad (`Parc.hs:787-805`) tracks two separate things:
- `owned :: Owned` (local set) — variables the current function owns and must drop
- `borrowed :: Borrowed` (from Core IR) — function parameter borrow info from all modules

A variable is considered borrowed iff it's NOT in the `owned` set (`isBorrowed tn = not <$> isOwned tn`, line 906).

#### Compiler: Borrow inference is NOT cross-module

**Important nuance**: Koka's primary borrow info is **extracted structurally from each function body independently** — not inferred via cross-function constraint solving. Each function's `DefFun` records its parameter modes at definition time based on local usage analysis.

Koka has an experimental `--fbinference` flag for automatic borrow inference, but it is **disabled by default** and documented as:

```haskell
-- Compile/Options.hs:519
, hide $ fflag ["binference"] (\b f -> f{parcBorrowInference=b})
    "enable reuse inference (does not work cross-module!)"
```

This means Koka's production path works per-module, with no whole-program analysis.

#### C Runtime: Borrowing is purely compile-time

The Koka C runtime (`kklib/`) has **no runtime concept of borrowing**. At the C level, borrowing simply means "the compiler omitted the dup at the call site and the drop at the callee exit."

**Block header** (`kklib/include/kklib.h:129-156`):

```c
typedef struct kk_header_s {
  uint8_t   scan_fsize;    // number of fields with heap pointers (0-254)
  uint8_t   _field_idx;    // used for stackless freeing
  uint16_t  tag;           // constructor tag
  _Atomic(kk_refcount_t) refcount;  // reference count (int32_t)
} kk_header_t;
```

This is 8 bytes — identical layout to Flux's `FluxHeader` (which has `refcount(i32) | scan_fsize(u8) | obj_tag(u8) | reserved(u16)`). Both are Koka-inspired.

**Dup/Drop** (`kklib/include/kklib.h:688-701`):

```c
static inline kk_block_t* kk_block_dup(kk_block_t* b) {
  const kk_refcount_t rc = kk_block_refcount(b);
  if kk_unlikely(kk_refcount_is_thread_shared(rc)) {
    return kk_block_check_dup(b, rc);     // atomic path for shared objects
  } else {
    kk_block_refcount_set(b, kk_refcount_inc(rc));  // fast path: just increment
    return b;
  }
}

static inline void kk_block_drop(kk_block_t* b, kk_context_t* ctx) {
  const kk_refcount_t rc = kk_block_refcount(b);
  if (kk_refcount_is_unique_or_thread_shared(rc)) {  // rc <= 0
    kk_block_check_drop(b, rc, ctx);     // unique: free recursively
  } else {
    kk_block_refcount_set(b, rc-1);      // shared: just decrement
  }
}
```

**Child scanning on drop** (`kklib/src/refcount.c:28-86`): When a block becomes unique (refcount reaches 0), `kk_block_fast_drop_free` uses `scan_fsize` to recursively drop all child fields — exactly like Flux's `flux_drop`:

```c
static kk_block_t* kk_block_fast_drop_free(kk_block_t* b, kk_context_t* ctx) {
  const kk_ssize_t scan_fsize = b->header.scan_fsize;
  if (scan_fsize == 0) {
    kk_block_free(b, ctx);              // leaf: free directly
  } else if (scan_fsize == 1) {
    // Single child: tail-recurse into it after freeing parent
    kk_block_t* next = kk_block_fast_field_should_free(b, 0, ctx);
    kk_block_free(b, ctx);
    if (next != NULL) { b = next; goto tailcall; }
  }
  // ... general case uses stackless pointer reversal for deep structures
}
```

**Box dup/drop** (`kklib/include/kklib/box.h:164-171`): Polymorphic values check if they're pointers before RC operations:

```c
static inline kk_box_t kk_box_dup(kk_box_t b, kk_context_t* ctx) {
  if (kk_box_is_ptr(b)) { kk_block_dup(kk_ptr_unbox(b, ctx)); }
  return b;
}
```

This is analogous to Flux's NaN-box tag checking before dup/drop.

**Specialized drop** (`Backend/C/Parc.hs:735-746`): The compiler emits `dropn(x, N)` when the scan field count is known at compile time, avoiding the runtime scan_fsize lookup:

```haskell
dupDropFun False tp (Just (conRepr,_)) (Just scanFields) arg
  | scanFields > 0 && not (conReprIsValue conRepr)
  = App ... (InfoExternal [(C CDefault, "dropn(#1,#2)")]) [arg, makeInt32 scanFields]
```

#### Compiler: Reuse analysis is per-function

Koka's reuse analysis (`Backend/C/ParcReuse.hs`) tracks `Available` reuse slots per allocation size:

```haskell
type Available = M.IntMap [ReuseInfo]  -- allocation size -> reusable constructors
```

When a constructor is pattern-matched, its allocation slot becomes available. When a new constructor of the same size is built, the slot is reused via `kk_reuse_alloc` instead of `kk_block_alloc`. This is entirely local to each function — no cross-module data needed.

#### Key takeaway for Flux

Koka's approach is:
1. Borrow info is **per-function metadata** stored in `DefSort`, not inferred cross-module
2. Module interfaces (`.kki`) carry this metadata to downstream modules
3. The C runtime knows nothing about borrowing — it's purely a compile-time optimization (skip dup/drop)
4. Reuse analysis is per-function
5. The experimental cross-module inference (`--fbinference`) is disabled by default because it doesn't work cross-module

Flux's situation differs: Flux relies entirely on whole-program inference (`infer_borrow_modes` with Tarjan SCC + fixed-point iteration). To achieve Koka-style modularity, Flux has two options:
1. **Make per-function borrow extraction structural** (like Koka's default): analyze each function body in isolation, determine borrow modes from usage patterns without cross-function constraints. Simpler but potentially less precise.
2. **Keep inference but export results** (this proposal): run the existing SCC-based inference per module, export the computed `BorrowSignature`s, and load them as fixed constraints for downstream modules.

This proposal takes approach (2) because it preserves Flux's current inference quality while gaining modularity. Approach (1) could be explored as a future simplification.

Reference: Reinking, Xie, de Moura, Leijen. "Perceus: Garbage Free Reference Counting with Reuse" (MSR-TR-2020-42, PLDI 2021).

### Koka vs Flux runtime comparison

| Aspect | Koka (`kklib/`) | Flux (`runtime/c/`) | Notes |
|--------|-----------------|---------------------|-------|
| Block header | `kk_header_t`: scan_fsize(u8) + field_idx(u8) + tag(u16) + refcount(i32) = 8B | `FluxHeader`: refcount(i32) + scan_fsize(u8) + obj_tag(u8) + reserved(u16) = 8B | Same layout, same Koka inspiration |
| Dup | `kk_block_dup()` — increment refcount, atomic path for shared objects | `flux_dup()` — increment refcount | Koka has thread-shared path; Flux is single-threaded |
| Drop | `kk_block_drop()` → `kk_block_fast_drop_free()` with scan_fsize child scanning | `flux_drop()` with scan_fsize child scanning | Same algorithm |
| Drop optimization | Stackless pointer reversal for deep structures (`refcount.c:337-421`) | Iterative drop on ConsCell to prevent stack overflow | Koka more general; Flux handles cons lists specifically |
| Specialized drop | `dropn(x, N)` emitted when scan count known at compile time | No equivalent — always reads scan_fsize at runtime | Flux could adopt `dropn` |
| Box dup/drop | `kk_box_dup`/`kk_box_drop` — check `kk_box_is_ptr` before RC ops | NaN-box tag check before RC ops | Same concept, different encoding |
| Reuse at runtime | `kk_reuse_alloc` — reuse freed block if same size | `OpReuseAdt`/`OpReuseCons` — check uniqueness, reuse in-place | Both do in-place reuse |
| Allocator | `mimalloc` (size-class aware, per-thread caches) | Raw `malloc`/`free` | Flux could benefit from mimalloc or size-class free lists |
| Thread safety | Negative refcounts for thread-shared objects, atomic ops | Single-threaded only | Not relevant yet for Flux |
| Borrowing at runtime | None — purely compile-time (skip dup/drop) | None — purely compile-time (skip dup/drop) | Same design |

**Key insight for this proposal**: Both runtimes have identical borrowing semantics at the C level — borrowing is invisible to the runtime. The entire optimization is about the compiler deciding when to emit `dup()`/`drop()` calls. This confirms that exporting `BorrowSignature` in module interfaces is sufficient — no runtime changes needed.

### GHC (Haskell)

GHC compiles modules independently using `.hi` (Haskell Interface) files. These contain type signatures, strictness/demand info, unfoldings for inlining, and specialization rules. GHC's `-j` flag enables parallel module compilation using the module dependency graph.

Flux's `ModuleInterface` is analogous to GHC's `.hi` files but simpler — we only need types and borrow signatures, not unfoldings or rewrite rules.

### OCaml

OCaml compiles each `.ml` file independently using `.cmi` (compiled module interface) files. The interface contains type signatures and is validated against the `.mli` (handwritten interface) if present.

### Go

Go caches compiled packages keyed by content hash. The go build tool skips compilation for unchanged packages. While Go's compilation model is simpler (no borrow inference), the caching strategy (hash-based invalidation, per-module granularity) directly informs this proposal.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- **Cache location**: Should `.flxi` files live next to source files, in a centralized cache directory (like `.fxc` files), or in a project-level build directory? The existing `.fxc` pattern suggests a centralized cache.

- **Serialization format**: JSON (human-readable, easy to debug) vs bincode/MessagePack (faster, smaller)? Given that interfaces are small (dozens of function signatures), JSON is likely fine and matches the existing `serde_json` dependency.

- **Effect row signatures**: Should `ModuleInterface` also export effect row information for each function? Currently borrow inference doesn't depend on effect rows, but future effect-directed optimizations might.

- **Transitive dependency validation**: For `lib/Base/` (no cross-imports), this is trivial. For user modules with dependency chains, how deep should validation go? One option: store a hash of all transitive dependency interfaces in each `.flxi`.

## Future possibilities
[future-possibilities]: #future-possibilities

### Parallel module compilation

With module interfaces, modules that don't depend on each other can be compiled in parallel using `rayon`. The dependency graph is already available in `src/syntax/module_graph.rs`. Each module compiles independently, reads imported interfaces from cache, and writes its own interface when done.

### Explicit borrow annotations

Once module interfaces exist, a natural extension is letting users write explicit borrow annotations:

```flux
fn map(xs: borrow List<a>, f: borrow (a) -> b) -> List<b>
```

These would be stored directly in the interface, overriding inference. This is how Koka handles performance-critical APIs.

### Incremental type inference

Module interfaces cache type schemes. A more ambitious extension would cache partial type inference state (constraint sets, substitution maps) to enable incremental re-inference when only part of a module changes.

### Language Server Protocol (LSP)

Module interfaces provide the foundation for an LSP server — type signatures and borrow information for imported modules can be served from cache without re-compilation, enabling fast hover info and diagnostics.

### Separate compilation to object files

Module interfaces are the first step toward separate compilation. With interfaces, each module can be compiled to a `.o` file independently, then linked. This would dramatically improve build times for large projects.
