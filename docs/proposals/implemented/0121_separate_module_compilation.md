- Feature Name: Separate Module Compilation and Base Prelude
- Start Date: 2026-03-24
- Status: Implemented (Phases 1-5 complete; Phases 2, 3, 5 completed by Proposals 0137 and 0139)
- Proposal PR:
- Flux Issue:

## Summary

Add separate compilation for Flux modules so that `module Base.List {}` can be compiled independently, cached, and linked with user code. This unblocks the unified Base library (Proposal 0120) by solving two problems: (1) each module gets its own type inference pass (no HM inference hang), and (2) an auto-prelude mechanism imports Base modules unqualified so `map(...)` works without `List.map(...)`.

## Motivation

### The current blocker

Proposal 0120 defines a Flux standard library at `lib/Base/` with modules like `Base.List`, `Base.Numeric`, `Base.String`. The implementation hit two blockers:

1. **HM inference hang**: Prepending Base library functions into the user program creates a combined AST with 25+ polymorphic recursive definitions. The Aether borrow inference iterates until fixed point but oscillates forever with mutually-recursive Base functions. A `MAX_ROUNDS` limit was added as a workaround, but the root fix is separate compilation — each module infers types independently.

2. **Name collisions**: The compiler's internal state (built for the original user program) conflicts with Base library function names. `infer_expr_types_for_program` re-runs on the combined program but pulls in extra definitions from the VM's base function registry, creating phantom definitions.

Both problems disappear if each module is compiled as an independent unit.

### What separate compilation means

```
Without separate compilation (current):
    [Base.List + Base.Numeric + user code]
        → single type inference pass (hangs)
        → single Core IR (name collisions)

With separate compilation:
    [Base.List]      → type infer → Core IR + type signatures  (cached)
    [Base.Numeric]   → type infer → Core IR + type signatures  (cached)
    [user code]      → type infer → Core IR                    (uses cached signatures)
        → merge all Core IRs → LLVM codegen
```

### The GHC model

GHC compiles each module independently:

```
Data/List.hs → GHC → Data/List.hi (interface: type signatures)
                    → Data/List.o  (compiled code)

User.hs (imports Data.List)
    → reads Data/List.hi for type info
    → compiles User.hs with those types
    → links User.o + Data/List.o
```

The `.hi` file is the key innovation — it contains the module's exported type signatures without the implementation. This lets the compiler type-check user code without re-processing the library.

---

## Guide-level explanation

### For Flux users

Base library modules use the standard module system:

```flux
// lib/Base/List.flx
module Base.List {
    fn map(list, f) {
        match list {
            [] -> [],
            [h | t] -> [f(h) | map(t, f)]
        }
    }

    fn filter(list, pred) { ... }
    fn fold(list, acc, f) { ... }
    fn len(list) { ... }
    // ...
}
```

User code imports them:

```flux
import Base.List
import Base.Numeric

fn main() with IO {
    let nums = range(1, 10)
    println(sum(map(nums, \x -> x * 2)))
}
```

All Base functions are available unqualified — `map(...)` not `List.map(...)`. This is because Base modules are auto-imported as unqualified (like Haskell's `Prelude`).

### For compiler contributors

Each module is compiled independently:

```
Base/List.flx  → parse → HM infer → Core IR → save .flxi (interface) + .flxc (Core IR cache)
Base/Numeric.flx → parse → HM infer → Core IR → save .flxi + .flxc
user.flx → parse → load .flxi files → HM infer → Core IR → merge all Core IRs → LLVM codegen
```

The `.flxi` (Flux interface) file contains:
- Module name
- Exported function names and their inferred type schemes
- Source hash (for cache invalidation)

---

## Reference-level explanation

### Phase 1: Unqualified import syntax

Add `exposing` clause to import statements:

```flux
import Base.List                    // qualified: List.map(...)
import Base.List as L               // aliased: L.map(...)
import Base.List exposing *         // unqualified: map(...)
import Base.List exposing (map, filter)  // selective: map(...), filter(...)
```

Parser changes in `parse_import_statement`:

```rust
Statement::Import {
    name: Identifier,
    alias: Option<Identifier>,
    except: Vec<Identifier>,
    exposing: ImportExposing,  // NEW
    span: Span,
}

enum ImportExposing {
    None,           // default: qualified access only
    All,            // exposing *
    Names(Vec<Identifier>),  // exposing (map, filter)
}
```

Name resolution changes: when `exposing` is `All` or `Names`, the imported functions are added to the current scope without a module prefix.

### Phase 2: Per-module type inference

The module graph already compiles modules in topological order. Extend this so each module gets its own type inference pass:

```rust
// In ModuleGraph::build_with_entry_and_roots:
for node in self.topo_order() {
    // 1. Create a fresh compiler for this module
    let mut module_compiler = Compiler::new();

    // 2. Load type signatures from imported modules
    for import in &node.imports {
        let interface = load_interface(&import.target_path);
        module_compiler.register_imported_types(interface);
    }

    // 3. Run HM inference for this module only
    module_compiler.infer_expr_types_for_program(&node.program);

    // 4. Lower to Core IR
    let core = module_compiler.lower_to_core(&node.program);

    // 5. Save interface (exported types) and Core IR
    save_interface(&node.path, &module_compiler.exported_types());
    save_core_cache(&node.path, &core);
}
```

### Phase 3: Interface files (.flxi)

A `.flxi` file stores the type signatures exported by a module:

```
// Base/List.flxi (generated, not hand-written)
{
    "module": "Base.List",
    "source_hash": "a1b2c3...",
    "exports": {
        "map":    "forall a b. (List a, (a -> b)) -> List b",
        "filter": "forall a. (List a, (a -> Bool)) -> List a",
        "fold":   "forall a b. (List a, b, (b, a) -> b) -> b",
        "len":    "forall a. List a -> Int",
        "range":  "(Int, Int) -> List Int",
        ...
    }
}
```

Cache invalidation: if the source hash of `List.flx` changes, the `.flxi` is regenerated.

### Phase 4: Auto-prelude

The compiler automatically imports Base modules unqualified for every program:

```rust
// Implicit imports added before user code:
import Base.List exposing *
import Base.Numeric exposing *
import Base.Option exposing *
```

This can be disabled with a flag (`--no-prelude`) for programs that define their own `map`, `filter`, etc.

### Phase 5: Core IR merging for core_to_llvm

After separate compilation, each module has its own Core IR. The `core_to_llvm` backend merges them:

```rust
fn compile_with_modules(modules: &[CoreProgram]) -> LlvmModule {
    let mut merged = CoreProgram { defs: vec![] };
    for module in modules {
        merged.defs.extend(module.defs.clone());
    }
    compile_program(&merged)
}
```

Dead code elimination (already in Core passes) removes unused Base functions so the final binary only includes what the program actually uses.

---

## Implementation phases

**Phase 1 — `exposing` syntax** (~3 days) ✅ **DONE**
- `ImportExposing` enum added to `Statement::Import` in `src/syntax/statement.rs`
- Parser supports `exposing (..)` (all public members) and `exposing (name, name, ...)`
- Name resolution adds unqualified names to scope when `exposing` is used
- Test: `import Base.List exposing (..)` makes `map` available

**Phase 2 — Per-module compilation** (~1 week) 🔧 **PARTIAL**
- Modules compile in topological order via the module graph
- Each module gets its own parse + type inference pass
- **Not yet done**: fully independent `Compiler` instance per module (currently modules share state through the prelude injection approach)
- **Not yet done**: isolated Core IR production per module

**Phase 3 — Interface files** (~3 days) 🔧 **PARTIAL**
- `src/bytecode/compiler/module_interface.rs` exists with initial infrastructure
- **Not yet done**: full `.flxi` serialization of exported type schemes
- **Not yet done**: cache invalidation based on source hash
- **Not yet done**: loading `.flxi` instead of re-compiling

**Phase 4 — Auto-prelude** (~2 days) ✅ **DONE**
- `inject_base_prelude()` in `src/main.rs` auto-adds `import Base.* exposing (..)` for all Base modules
- Skipped for `--dump-aether`/`--dump-core`/`--trace-aether` diagnostic modes
- Test: `map(...)` works without explicit import in all user programs

**Phase 5 — Core IR merge** (~2 days) 🔧 **PARTIAL**
- Both VM and LLVM backends compile Base modules alongside user code
- Currently merging happens at source level (prelude injection) rather than Core IR level
- **Not yet done**: per-module Core IR cache + merge step
- **Not yet done**: dead code elimination of unused Base functions at the Core IR merge boundary

---

## Drawbacks

- **Complexity**: Separate compilation adds interface files, cache management, and a multi-stage build. This is significant compiler infrastructure.

- **Incremental**: Changes to Base modules require recompilation of all dependents. Interface files help (only recompile if types change), but the cache management adds complexity.

- **Name shadowing**: With `exposing *`, user-defined `map` shadows `Base.List.map`. This is intentional but may surprise users. Error messages should mention the shadow.

---

## Prior art

- **GHC**: `.hi` interface files, per-module compilation, `Prelude` auto-import. 25+ years of production use.
- **OCaml**: `.cmi` compiled interface files, per-module compilation. Similar architecture.
- **Rust**: Each crate is compiled independently. `use std::*` for unqualified import.
- **Lean 4**: `.olean` compiled module files, auto-import of `Init`.

---

## Unresolved questions

- **Interface file format**: JSON for simplicity or binary for speed? JSON is debuggable; binary is faster to parse. Start with JSON, optimize later.

- **Prelude customization**: Should users be able to specify which Base modules are auto-imported? Or is it all-or-nothing with `--no-prelude`?

- **VM integration**: Should the VM also use separate compilation, or only `core_to_llvm`? If both, the cache can be shared. If only `core_to_llvm`, the VM continues using Rust base functions.

- **Circular imports**: Base modules should not have circular dependencies. The topological sort already enforces this, but it should be documented.

---

## Future possibilities

- **Incremental compilation**: Only recompile modules whose source (or dependencies' interfaces) changed.
- **Parallel compilation**: Independent modules can be compiled in parallel.
- **Package system**: External packages are just modules with separate compilation — the same `.flxi` mechanism works.
- **LSP integration**: Interface files provide type info for IDE features (hover, autocomplete) without recompiling.
