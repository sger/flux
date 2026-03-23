- Feature Name: Backend Consolidation — Two-Backend Architecture
- Start Date: 2026-03-24
- Proposal PR:
- Flux Issue:

## Summary

Remove the Cranelift JIT (`src/jit/`) and the `llvm-sys` LLVM backend (`src/llvm/`) from Flux, leaving exactly two execution backends: the **bytecode VM** for development and the **core_to_llvm text IR backend** for native compilation. This simplifies the codebase by ~7,000 lines, removes two heavyweight native dependencies, eliminates three Cargo feature flags, and aligns Flux with GHC's proven two-backend model (GHCi interpreter + LLVM/NCG compiler).

## Motivation

### Four backends is three too many

Flux currently has four code paths from Core IR to execution:

```
Core → CFG → Bytecode → VM                          (default)
Core → CFG → Cranelift IR → JIT execution            (--jit, feature "jit")
Core → CFG → LLVM C API → MCJIT execution            (--llvm, feature "llvm")
Core → LLVM text IR → opt → llc → cc → native binary (--core-to-llvm, feature "core_to_llvm")
```

Each backend has its own:
- Compilation pipeline and lowering logic
- Runtime helpers / native context
- Feature flag and conditional compilation
- Test suite and parity checks
- Maintenance burden

The Cranelift JIT and `llvm-sys` backend were valuable for bootstrapping native execution, but `core_to_llvm` supersedes both with a cleaner architecture, better optimization potential, and fewer dependencies.

### Why remove Cranelift JIT?

| Issue | Detail |
|-------|--------|
| **Dependency weight** | 5 Cranelift crates (`cranelift-codegen`, `cranelift-frontend`, `cranelift-module`, `cranelift-jit`, `cranelift-native`) + `target-lexicon` |
| **Code size** | `src/jit/compiler.rs` alone is ~5,000 lines |
| **Opaque runtime calls** | Every non-trivial operation bounces back to Rust via `rt_*` helpers — Cranelift cannot optimize across these boundaries |
| **No AOT compilation** | JIT only — cannot produce standalone binaries |
| **Maintenance burden** | Cranelift API changes across versions require ongoing updates |
| **Superseded** | `core_to_llvm` produces better code (LLVM optimizer), supports AOT, and has no native build dependency |

### Why remove `llvm-sys` backend?

| Issue | Detail |
|-------|--------|
| **Build dependency** | Requires LLVM 18 development headers (~300 MB), `LLVM_SYS_180_PREFIX` env var, platform-specific setup |
| **Opaque runtime calls** | Same problem as Cranelift — `rt_add`, `rt_make_closure`, `rt_call_jit_function` are opaque to LLVM's optimizer |
| **Not standalone** | Uses MCJIT in-process — requires the Flux Rust binary to be running |
| **Code duplication** | `src/llvm/compiler/` duplicates lowering logic already in `core_to_llvm` |
| **Version lock-in** | Pinned to `llvm-sys = "180"` — changing LLVM versions requires crate updates |
| **Superseded** | `core_to_llvm` uses text IR (stable across LLVM 15-19+), no C API, no build dependency |

### The GHC precedent

GHC has exactly two execution modes:

1. **GHCi** — bytecode interpreter for interactive development, REPL, fast iteration
2. **GHC compiler** — produces optimized native code via NCG (native code generator) or LLVM backend

GHCi doesn't try to JIT-compile to native code. It interprets bytecode, and that's fast enough for development. When users need performance, they compile with `ghc -O2`. This clean split has worked for 25+ years.

Flux should adopt the same model:

1. **`flux program.flx`** — bytecode VM for development, REPL, testing
2. **`flux program.flx --core-to-llvm --emit-binary`** — LLVM native backend for production

### Specific benefits of consolidation

1. **~7,000 fewer lines of code**: Remove `src/jit/` (~5,000 lines) and `src/llvm/` (~2,000 lines)
2. **Two fewer native dependencies**: No Cranelift crates, no `llvm-sys`
3. **Faster `cargo build`**: Cranelift and `llvm-sys` are the slowest dependencies to compile
4. **No feature flags**: Both remaining backends are always available (VM needs no features; `core_to_llvm` is pure Rust)
5. **Simpler CI**: No matrix of feature combinations to test
6. **One optimization story**: "Use `--core-to-llvm` for fast binaries" instead of "try `--jit` or `--llvm` or `--core-to-llvm` and compare"
7. **Clearer mental model**: Interpret or compile. No in-between.

---

## Guide-level explanation

### For Flux users

After consolidation, there are two ways to run Flux programs:

```bash
# Development (fast startup, interpreted)
flux program.flx

# Production (optimized native binary)
flux program.flx --native -o program
./program
```

The `--jit` and `--llvm` flags are removed. The `--core-to-llvm` flag is renamed to `--native` (or kept as `--core-to-llvm` for explicitness).

Migration guide:
- `flux program.flx` — unchanged (bytecode VM)
- `flux program.flx --jit` → `flux program.flx --native`
- `flux program.flx --llvm` → `flux program.flx --native`
- `flux program.flx --llvm --emit-obj` → `flux program.flx --native --emit-binary`

### For compiler contributors

The module structure simplifies to:

```
src/
├── syntax/           Lexer, parser, interner
├── ast/              AST transforms, type inference
├── core/             Core IR, passes, lowering
├── aether/           Perceus RC optimization
├── cfg/              CFG IR (used by bytecode compiler)
├── bytecode/         Bytecode compiler + VM
├── core_to_llvm/     LLVM text IR backend
│   ├── ir/           LLVM IR AST + pretty-printer
│   ├── codegen/      Core → LLVM lowering
│   ├── pipeline.rs   opt/llc/cc orchestration
│   └── target.rs     Host detection
├── runtime/          Value, closures, cons cells, HAMT (shared)
├── primop/           Primitive operations (shared)
├── shared_ir/        Shared ID types
├── diagnostics/      Error reporting
└── types/            Type system
```

Removed:
- ~~`src/jit/`~~ — Cranelift JIT compiler, runtime helpers, value arena
- ~~`src/llvm/`~~ — LLVM C API wrapper, compiler, runtime helpers

### Compilation pipeline after consolidation

```
Source (.flx)
  → Lexer → Parser → AST
  → HM Type Inference
  → Core IR → Core passes → Aether (dup/drop/reuse)
    ├── → CFG IR → Bytecode → VM execution          (default)
    └── → LLVM text IR → opt → llc → cc → binary    (--native)
```

Both backends share everything up to and including the Aether pass. They diverge only at the final lowering step. Crucially, they also share the same **NaN-box value representation** — every Flux value is an `i64` with the same bit layout (sentinel `0x7FFC`, 4-bit tag at [49:46], 46-bit payload). This means:

- The VM stack and `core_to_llvm` generated code use identical value encoding
- The C runtime (`runtime/c/`) and the VM runtime (`src/runtime/`) agree on how integers, booleans, pointers, and floats are represented
- Debugging is straightforward — a NaN-boxed value means the same thing in both backends
- The primop semantics (tag/untag/compare) are shared, not duplicated

---

## Reference-level explanation

### Files removed

| Path | Lines | Purpose |
|------|-------|---------|
| `src/jit/compiler.rs` | ~5,000 | Cranelift IR generation from Core/CFG IR |
| `src/jit/runtime_helpers.rs` | ~500 | `rt_*` Rust functions called from JIT code |
| `src/jit/value_arena.rs` | ~200 | Arena allocator for JIT values |
| `src/jit/mod.rs` | ~100 | JIT public API |
| `src/llvm/compiler/` | ~1,200 | CFG IR → LLVM C API translation |
| `src/llvm/context.rs` | ~300 | LLVM context and module management |
| `src/llvm/wrapper.rs` | ~200 | Safe wrappers around `llvm-sys` |
| `src/llvm/mod.rs` | ~200 | LLVM public API |
| **Total** | **~7,700** | |

### Cargo.toml changes

```toml
# BEFORE
[features]
core_to_llvm = []
jit = [
    "cranelift-codegen",
    "cranelift-frontend",
    "cranelift-module",
    "cranelift-jit",
    "cranelift-native",
    "target-lexicon",
]
llvm = ["llvm-sys"]

[dependencies]
cranelift-codegen = { version = "0.128", optional = true }
cranelift-frontend = { version = "0.128", optional = true }
cranelift-module = { version = "0.128", optional = true }
cranelift-jit = { version = "0.128", optional = true }
cranelift-native = { version = "0.128", optional = true }
target-lexicon = { version = "0.13", optional = true }
llvm-sys = { version = "180", optional = true }

# AFTER
[dependencies]
# No optional native dependencies.
# core_to_llvm is pure Rust — no feature flag needed.
```

### lib.rs changes

```rust
// BEFORE
pub mod aether;
pub mod ast;
pub mod bytecode;
pub mod cfg;
pub mod core;
#[cfg(feature = "core_to_llvm")]
pub mod core_to_llvm;
pub mod diagnostics;
#[cfg(feature = "jit")]
pub mod jit;
#[cfg(feature = "llvm")]
pub mod llvm;
pub mod primop;
pub mod runtime;
pub mod shared_ir;
pub mod syntax;
pub mod types;

// AFTER
pub mod aether;
pub mod ast;
pub mod bytecode;
pub mod cfg;
pub mod core;
pub mod core_to_llvm;
pub mod diagnostics;
pub mod primop;
pub mod runtime;
pub mod shared_ir;
pub mod syntax;
pub mod types;
```

### CLI changes

```
# Removed flags
--jit              (was: Cranelift JIT execution)
--llvm             (was: llvm-sys MCJIT execution)
--emit-obj         (was: llvm-sys AOT object file)

# Kept/renamed flags
--core-to-llvm     → --native (optional rename for ergonomics)
--emit-llvm        (emit .ll text file)
--emit-binary      (compile to native binary)
-o <path>          (output path)
```

### Test changes

- Remove: `tests/jit_phase3_tests.rs`, `tests/jit_phase4_tests.rs`
- Remove: all `vm_jit_parity_*` tests (replace with `vm_native_parity_*`)
- Remove: `--features jit` and `--features llvm` from CI matrix
- Keep: `tests/core_to_llvm_*` tests
- Add: `tests/vm_native_parity_*` comparing VM and `--native` output

### Runtime module cleanup

Files in `src/runtime/` that exist only for the JIT/LLVM backends:

| File | Purpose | Action |
|------|---------|--------|
| `native_context.rs` | Shared context for JIT/LLVM native helpers | Remove |
| `native_helpers.rs` | `rt_*` extern C functions for JIT/LLVM | Remove |

The `Value` enum, cons cells, HAMT, nanbox, and base functions remain — they're used by the VM.

### Migration timeline

**Phase 1 — Deprecation** (immediate)
- Add deprecation warnings to `--jit` and `--llvm` flags
- Update documentation to recommend `--core-to-llvm`
- Ensure `core_to_llvm` is always compiled (remove feature gate)

**Phase 2 — Feature parity** (Proposal 0117)
- Expand primops and write Flux prelude
- Run all examples through `--core-to-llvm`
- Verify output parity with VM

**Phase 3 — Removal** (after parity is confirmed)
- Delete `src/jit/`, `src/llvm/`
- Remove Cranelift and `llvm-sys` dependencies
- Remove feature flags from `Cargo.toml`
- Remove `--jit` and `--llvm` CLI flags
- Update all tests

**Phase 4 — Polish**
- Rename `--core-to-llvm` to `--native` (optional)
- Update all documentation, examples, CI
- Cut release

---

## Drawbacks

- **Loss of JIT for interactive use**: The Cranelift JIT provides faster-than-VM execution without the ~500ms `opt`/`llc`/`cc` overhead. For REPL-like workflows where users want near-native speed with instant startup, there's no replacement. Mitigation: the VM is fast enough for interactive use (GHCi proves this), and `core_to_llvm` can cache compiled binaries for re-execution.

- **LLVM tools required for native compilation**: Users need `opt`, `llc`, and `cc` on PATH. The Cranelift JIT and `llvm-sys` backend bundled everything into the Rust binary. Mitigation: LLVM tools are widely available (`brew install llvm`, `apt install llvm`), and the VM works without them.

- **Compilation latency**: The `opt` → `llc` → `cc` pipeline adds ~200-500ms per compilation. Cranelift JIT was faster (~50ms). Mitigation: this is acceptable for release builds; development uses the VM.

- **Removal is irreversible**: Once the code is deleted, re-adding Cranelift or `llvm-sys` support requires significant effort. Mitigation: the code lives in git history; the architecture (CFG IR → backend) is preserved.

---

## Rationale and alternatives

### Why not keep Cranelift as a fast-compile option?

Cranelift generates worse code than LLVM (no sophisticated optimization passes) and still requires heavyweight dependencies. The niche it fills (faster-than-VM, slower-than-LLVM) doesn't justify 5,000+ lines of code and 6 crate dependencies when the VM is fast enough for development.

### Why not keep `llvm-sys` for in-process execution?

The `llvm-sys` backend has all the downsides of `core_to_llvm` (needs LLVM) plus additional ones (needs LLVM *headers* at build time, C API bindings, opaque runtime calls). `core_to_llvm` strictly dominates it.

### Why not remove the VM too and go LLVM-only?

The VM provides instant startup, zero external dependencies, and a simple debugging experience. GHCi exists for the same reason — not everything needs to be compiled. The VM also enables the REPL, test runner, and `--trace` debugging.

### Why not use Cranelift as the native backend instead of LLVM?

Cranelift produces reasonable code but lacks LLVM's optimization depth (no loop vectorization, limited inlining heuristics, no LTO). For a functional language where closures, pattern matching, and recursive data structures dominate, LLVM's optimization passes make a measurable difference. GHC experimented with its own NCG vs LLVM and found LLVM produces 10-30% faster code for functional workloads.

---

## Prior art

### GHC (Haskell)

GHC maintains exactly two execution modes: GHCi (bytecode interpreter) and the compiler (NCG or LLVM backend). GHC previously had a "via-C" backend that compiled through GCC — it was removed in GHC 7.0 (2010) when the LLVM backend matured. The removal simplified the codebase significantly.

### Rust

Rust had a single backend (LLVM) for years. The Cranelift backend was added experimentally for faster debug builds but remains optional and secondary. The primary compilation path is always LLVM.

### Go

Go has a single compiler backend (its own SSA-based code generator). It previously had `gccgo` (GCC-based) which was deprecated and removed. Consolidating to one backend simplified the ecosystem.

### Zig

Zig maintains two backends: a self-hosted x86-64 backend for fast debug compilation and an LLVM backend for optimized release builds. This is the same split Flux would have (VM for fast iteration, LLVM for release).

---

## Unresolved questions

- **Naming**: Should `--core-to-llvm` be renamed to `--native`, `--compile`, `--release`, or `--llvm`? The `--native` flag is concise and clear; `--llvm` would conflict with the old flag during the transition period.

- **REPL**: The REPL currently uses the VM. Should `core_to_llvm` ever support incremental compilation for a compiled REPL? This is a future possibility, not a blocker.

- **Timing**: Should removal happen before or after Proposal 0117 (primop expansion + prelude) achieves full parity? Recommended: after parity, to avoid breaking users who depend on `--jit`/`--llvm` for programs that `core_to_llvm` can't yet compile.

---

## Future possibilities

- **Cached native compilation**: Once a program is compiled to a native binary, cache the binary and skip recompilation on subsequent runs (like `cargo`'s incremental compilation).

- **`flux build`**: A build command that produces optimized binaries, similar to `cargo build --release` or `ghc -O2 -o program`.

- **`flux run --native`**: Compile, execute, and clean up in one step (already implemented as `--core-to-llvm` default behavior).

- **Debug info**: Emit DWARF debug info in the LLVM IR so native binaries can be debugged with `lldb`/`gdb`.

- **Profile-guided optimization**: LLVM PGO works natively with `.ll` files. A future `flux build --pgo` could instrument, profile, and recompile automatically.
