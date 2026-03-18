- Feature Name: LLVM Native Backend
- Start Date: 2026-03-18
- Status: Draft
- Proposal PR:
- Flux Issue:

# Proposal 0105: LLVM Native Backend

## Summary

Add an LLVM-based native code backend to Flux as an optional feature (`--features llvm`),
producing highly optimised machine code for release builds. The backend consumes the
existing `backend_ir::IrProgram` (CFG IR) directly — no new intermediate representation
is required. A thin safe wrapper over `llvm-sys` is introduced rather than a third-party
binding crate, keeping the dependency surface minimal and LLVM-version-agnostic.

---

## Motivation

Flux currently has two execution backends:

| Backend | Activation | Compile speed | Code quality |
|---------|-----------|---------------|-------------|
| VM bytecode | default | instant | interpreted |
| Cranelift JIT | `--features jit --jit` | fast (~ms) | good |

Both backends serve interactive and development use well. What is missing is a
**release-mode backend** that applies deep optimisations — inlining, loop invariant
code motion, vectorisation, dead code elimination — and produces the fastest possible
machine code for long-running programs, benchmarks, and production deployments.

LLVM is the proven answer for this. It is the backend behind Clang, Rust (`rustc`),
Swift, Julia, Kotlin/Native, and many others. Its optimisation pipeline has decades
of investment and produces code that consistently outperforms Cranelift's output for
compute-intensive workloads.

Specific scenarios that motivate this:

- **Numerical programs**: tight loops over arrays of integers or floats. NaN boxing
  means every value is already an `i64`; LLVM can vectorise such loops and eliminate
  redundant tag checks.
- **Recursive algorithms**: LLVM's inliner can eliminate closure call overhead for
  known higher-order functions. Cranelift does not inline across function boundaries.
- **Production deployments**: a compiled Flux binary via LLVM AOT would have no JIT
  warm-up cost and no dependency on the Cranelift runtime.
- **Cross-compilation**: LLVM targets every major architecture (x86-64, ARM64,
  RISC-V, WASM). Cranelift supports fewer targets.

---

## Guide-level explanation

From a user perspective, the LLVM backend is a new compilation mode:

### Prerequisites

LLVM 18 must be installed on the build machine. An install script is provided
at `scripts/ci/install_llvm.sh` that handles macOS, Ubuntu/Debian, and Fedora.

```bash
# Automated (recommended)
bash scripts/ci/install_llvm.sh

# Or manually:

# macOS (Homebrew)
brew install llvm@18
echo 'export LLVM_SYS_180_PREFIX=$(brew --prefix llvm@18)' >> ~/.zshrc
source ~/.zshrc

# Ubuntu / Debian / WSL
sudo apt-get install -y llvm-18-dev libclang-18-dev lld-18
echo 'export LLVM_SYS_180_PREFIX=/usr/lib/llvm-18' >> ~/.bashrc
source ~/.bashrc

# Verify
$LLVM_SYS_180_PREFIX/bin/llvm-config --version  # should print 18.x.x
```

### Usage

```bash
# Development: VM interpreter (default, instant startup)
cargo run -- program.flx

# Fast JIT: Cranelift (good code, fast compile)
cargo run --features jit -- program.flx --jit

# Release: LLVM AOT (best code, slower compile)
cargo run --features llvm -- program.flx --llvm

# Compile to native binary (AOT)
cargo run --features llvm -- program.flx --llvm --emit-binary -o program
```

No language changes are required. All valid Flux programs compile identically
on all three backends; only performance differs.

For compiler contributors, the LLVM backend is a new module `src/llvm/` that
mirrors `src/jit/` structurally. It consumes `IrProgram` from
`backend_ir::lower_program_to_ir()` — the same entry point the Cranelift JIT uses —
and emits LLVM IR via a thin wrapper over `llvm-sys`.

---

## Reference-level explanation

### Architecture

```
Source (.flx)
  → Lexer → Parser → AST passes
  → HM Type Inference
  → lower_program_to_ir() → IrProgram (CFG IR)   ← shared entry point
      ├── Bytecode compiler  → VM execution
      ├── Cranelift JIT      → JIT execution
      └── LLVM backend       → AOT / JIT execution  ← new
```

The `backend_ir` boundary introduced in proposal 0086 makes this addition
possible without touching anything above it.

### New module: `src/llvm/`

```
src/llvm/
├── mod.rs              — llvm_compile() / llvm_execute() / llvm_emit_binary()
├── compiler.rs         — IrProgram → LLVM IR translation (~4000 lines est.)
├── context.rs          — LlvmContext: module, execution engine, globals
├── runtime_helpers.rs  — extern "C" helpers: GC, ADT, closures, effects
├── wrapper.rs          — thin safe Rust API over llvm-sys (~300 lines)
└── value_repr.rs       — NanBox ↔ LLVM i64 encoding helpers
```

### Dependency: `llvm-sys` via thin wrapper

Rather than depending on `inkwell`, a thin safe wrapper over `llvm-sys` is
introduced in `src/llvm/wrapper.rs`. This wraps only the ~30 LLVM API functions
the backend actually needs:

```rust
// src/llvm/wrapper.rs (illustrative)
pub struct LlvmModule(LLVMModuleRef);
pub struct LlvmBuilder(LLVMBuilderRef);
pub struct LlvmFunction(LLVMValueRef);
pub struct LlvmBasicBlock(LLVMBasicBlockRef);
pub struct LlvmValue(LLVMValueRef);

impl LlvmBuilder {
    pub fn build_int_add(&self, lhs: LlvmValue, rhs: LlvmValue, name: &str) -> LlvmValue
    pub fn build_int_sub(&self, lhs: LlvmValue, rhs: LlvmValue, name: &str) -> LlvmValue
    pub fn build_int_mul(&self, lhs: LlvmValue, rhs: LlvmValue, name: &str) -> LlvmValue
    pub fn build_icmp(&self, op: IntPredicate, lhs: LlvmValue, rhs: LlvmValue) -> LlvmValue
    pub fn build_br(&self, dest: LlvmBasicBlock)
    pub fn build_cond_br(&self, cond: LlvmValue, then: LlvmBasicBlock, else_: LlvmBasicBlock)
    pub fn build_call(&self, func: LlvmFunction, args: &[LlvmValue], name: &str) -> LlvmValue
    pub fn build_ret(&self, value: LlvmValue)
    pub fn build_phi(&self, ty: LlvmType, name: &str) -> LlvmValue
    pub fn build_and(&self, lhs: LlvmValue, rhs: LlvmValue, name: &str) -> LlvmValue
    pub fn build_or(&self, lhs: LlvmValue, rhs: LlvmValue, name: &str) -> LlvmValue
    pub fn build_lshr(&self, lhs: LlvmValue, rhs: LlvmValue, name: &str) -> LlvmValue
    pub fn build_shl(&self, lhs: LlvmValue, rhs: LlvmValue, name: &str) -> LlvmValue
    pub fn build_load(&self, ptr: LlvmValue, name: &str) -> LlvmValue
    pub fn build_store(&self, val: LlvmValue, ptr: LlvmValue)
    pub fn build_gep(&self, ptr: LlvmValue, indices: &[LlvmValue], name: &str) -> LlvmValue
    pub fn build_bitcast(&self, val: LlvmValue, ty: LlvmType, name: &str) -> LlvmValue
    pub fn build_int_to_ptr(&self, val: LlvmValue, ty: LlvmType, name: &str) -> LlvmValue
    pub fn build_ptr_to_int(&self, val: LlvmValue, ty: LlvmType, name: &str) -> LlvmValue
}
```

This gives the Flux project full control over the LLVM API surface and avoids
coupling to `inkwell`'s release schedule.

**Cargo.toml:**
```toml
[features]
llvm = ["llvm-sys"]

[dependencies]
llvm-sys = { version = "180", optional = true }
```

### Value representation

NaN boxing means every Flux value is already an `i64` at the LLVM IR level:

```
LLVM type for all Flux values: i64
```

This is the same representation Cranelift uses. No boxing/unboxing IR is needed
for integer, float, boolean, or pointer values — they are just `i64` constants
or loads. The NaN-box tag dispatch compiles to a single `and` + `icmp` sequence
that LLVM can optimise aggressively.

### CFG IR → LLVM IR translation

Each CFG construct has a direct LLVM IR equivalent:

| CFG IR | LLVM IR |
|--------|---------|
| `IrBlock` | `BasicBlock` |
| `Terminator::Jump(block)` | `br label %block` |
| `Terminator::Branch(cond, t, f)` | `br i1 %cond, label %t, label %f` |
| `Terminator::Return(v)` | `ret i64 %v` |
| `BinOp::Add` | `add i64 %lhs, %rhs` |
| `BinOp::Sub` | `sub i64 %lhs, %rhs` |
| `BinOp::Mul` | `mul i64 %lhs, %rhs` |
| `BinOp::Div` | `sdiv i64 %lhs, %rhs` |
| `CmpOp::Eq` | `icmp eq i64 %lhs, %rhs` |
| `CmpOp::Lt` | `icmp slt i64 %lhs, %rhs` |
| Function call | `call i64 @fn(i64 %ctx, i64* %args, i64 %nargs)` |
| Phi node | `phi i64 [%v1, %block1], [%v2, %block2]` |

### Closure representation

Closures are compiled as structs passed by pointer:

```llvm
; Closure layout in LLVM IR
%Closure = type { i8*, i64*, i64 }
;                 ^fn_ptr ^captures ^ncaptures
```

The calling convention mirrors the Cranelift JIT's Array ABI:

```llvm
define i64 @flux_fn(i64* %args, i64 %nargs, i64* %captures, i64 %ncaptures)
```

This is identical to what the JIT already uses, so `rt_make_closure`,
`rt_call_closure`, and related runtime helpers can be shared between the
Cranelift and LLVM backends.

### GC integration

The shadow-stack approach used by the Cranelift JIT (`rt_push_gc_roots` /
`rt_pop_gc_roots`) is reused unchanged. LLVM's `llvm.gcroot` intrinsic is
not used — the manual shadow stack is simpler, portable, and already proven
correct by the JIT.

### Effect handler integration

Effect handlers (`OpHandle`, `OpPerform`, `OpResume`) are already compiled
by the Cranelift JIT via runtime helpers (`rt_push_handler`, `rt_pop_handler`,
`rt_perform`, `rt_resume`). The LLVM backend calls the same `extern "C"`
helpers — no new effect handling logic is needed.

### Optimisation pipeline

The LLVM optimisation pipeline is applied after IR generation:

```rust
// Standard O2 pass pipeline
let pass_manager = LLVMCreatePassManager();
LLVMAddInstructionCombiningPass(pass_manager);
LLVMAddReassociatePass(pass_manager);
LLVMAddGVNPass(pass_manager);
LLVMAddCFGSimplificationPass(pass_manager);
LLVMAddLoopUnrollPass(pass_manager);
LLVMRunPassManager(pass_manager, module);
```

For AOT compilation a full O2/O3 pipeline is applied. For JIT use the
pipeline is configurable — O0 for fast compile, O2 for optimised output.

### AOT binary emission

```rust
pub fn llvm_emit_binary(program: &IrProgram, output_path: &Path) -> Result<(), LlvmError> {
    let module = compile_to_llvm_ir(program)?;
    apply_optimisations(&module, OptLevel::O2);
    emit_object_file(&module, output_path)?;
    link_with_runtime(output_path) // invoke system linker (lld or cc)
}
```

The emitted binary links against a small Flux runtime static library
containing the GC, base functions, and effect handler stack.

### Cargo feature and CLI

```toml
# Cargo.toml
[features]
llvm = ["llvm-sys"]
```

```
# CLI flags (src/main.rs)
--llvm              Use LLVM backend (requires --features llvm)
--emit-binary       Compile to native binary (AOT, implies --llvm)
-o <path>           Output path for --emit-binary
--opt-level <0|1|2|3>   LLVM optimisation level (default: 2)
```

### Implementation phases

**Phase 1 — Scaffold and arithmetic** (~2-3 weeks)
- `src/llvm/` module structure, `wrapper.rs`, `LlvmContext`
- `llvm-sys` build integration and CI setup
- Integer arithmetic, comparisons, boolean ops
- Basic block and terminator translation
- Smoke test: fibonacci compiles and runs correctly

**Phase 2 — Functions and closures** (~2-3 weeks)
- Closure construction (`rt_make_closure`)
- Function call dispatch (direct and indirect)
- Free variable capture and access
- Tail call via LLVM `musttail`

**Phase 3 — ADTs and pattern matching** (~2-3 weeks)
- NanBox tag dispatch → LLVM `switch` on tag bits
- ADT construction and field access via runtime helpers
- Array and tuple construction
- String operations

**Phase 4 — Effects and handlers** (~3-4 weeks)
- `OpHandle` / `OpPerform` / `OpResume` via existing `extern "C"` helpers
- Continuation capture and resume
- Tail-resumptive handler fast path

**Phase 5 — GC and runtime integration** (~1-2 weeks)
- Shadow stack (`rt_push_gc_roots` / `rt_pop_gc_roots`)
- GC alloc trigger (`rt_gc_alloc`)
- Arena reset on function exit

**Phase 6 — AOT binary emission** (~1-2 weeks)
- Object file emission via `LLVMTargetMachineEmitToFile`
- Runtime static library (`flux_runtime.a`)
- System linker invocation

**Phase 7 — Optimisation and parity** (~2-3 weeks)
- LLVM O2 pass pipeline
- VM/JIT/LLVM parity test suite
- Benchmark comparison across all three backends

---

## Drawbacks

- **System dependency**: LLVM must be installed on the build machine. This is
  opt-in (`--features llvm`) so it does not affect default builds or CI, but it
  adds friction for contributors who want to work on the LLVM backend.

- **Build time**: `llvm-sys` links against LLVM (~300 MB of libraries). The
  incremental build cost after the first compile is low, but the initial link
  is slow.

- **Maintenance surface**: ~4000-5000 lines of new compiler code that must be
  kept in sync with CFG IR changes. Any new IR instruction requires a
  corresponding LLVM translation.

- **LLVM version drift**: The LLVM C API is stable but occasionally deprecated
  functions are removed. The `llvm-sys` version number must be updated when
  upgrading LLVM.

- **Slower compile for release**: LLVM O2/O3 is significantly slower than
  Cranelift. This is expected and intentional — the LLVM backend is for
  release builds, not interactive use.

---

## Rationale and alternatives

**Why not extend Cranelift further?**

Cranelift's optimiser is intentionally minimal — its design goal is fast
compilation, not maximum code quality. Adding loop optimisations, inlining,
and vectorisation to Cranelift is not on its roadmap. LLVM is the right tool
for release-quality code generation.

**Why not use `inkwell`?**

`inkwell` is a well-designed crate but it locks each version to a specific LLVM
release (`inkwell 0.4` → LLVM 17, `inkwell 0.5` → LLVM 18). This creates
a permanent coupling to `inkwell`'s release cadence on top of LLVM's. A thin
wrapper over `llvm-sys` (~300 lines) gives full control over the API surface
and works across LLVM versions with minor ifdefs.

**Why not use MLIR?**

MLIR is LLVM's next-generation multi-level IR. It is more powerful than LLVM IR
for certain optimisations but significantly more complex and less mature for
language backends. The Flux CFG IR maps cleanly to LLVM IR without needing MLIR's
dialect system. MLIR is worth revisiting in a future proposal once the core LLVM
backend is stable.

**Why not target WASM directly?**

LLVM can emit WASM as a target, so WASM support is a natural extension of this
proposal rather than an alternative to it. A dedicated WASM proposal can follow.

**Impact of not doing this:**

Without an LLVM backend, Flux has no path to competitive release-mode performance.
Cranelift produces good code but cannot close the gap with `rustc`/`clang` output
on numeric and functional workloads. For Flux to be taken seriously as a performance
language, a production-quality code generator is necessary.

---

## Prior art

- **GHC** — LLVM backend added in GHC 7.0 (2010) alongside the native code
  generator. GHC's LLVM backend translates Cmm → LLVM IR. For Flux, CFG IR
  is already closer to LLVM IR than Cmm is, making the translation simpler.

- **OCaml** — `ocamlopt` generates native code via its own backend (not LLVM).
  There are experimental LLVM backends for OCaml but none are in the main tree.
  The OCaml experience shows that a well-optimised native backend is essential
  for a functional language to compete on performance.

- **Kotlin/Native** — uses LLVM as its sole native backend. The entire compiler
  pipeline terminates at LLVM IR. Shows that LLVM is viable as a functional
  language backend even for complex type systems.

- **Swift** — uses LLVM with SIL (Swift Intermediate Language) as the layer
  between the type system and LLVM IR. SIL is analogous to Flux's CFG IR — a
  mid-level IR that makes ownership and control flow explicit before LLVM
  lowering.

- **Julia** — uses LLVM JIT via `llvm-sys`-style bindings. Julia's approach of
  type-specialised LLVM IR generation is the closest analogue to what Flux would
  do for integer/float-specialised code.

- **rustc** — used `llvm-sys` directly before moving to its own
  `rustc_llvm` wrapper. Demonstrates that a production compiler can manage
  `llvm-sys` bindings without a high-level crate.

---

## Unresolved questions

- **LLVM version to target first**: LLVM 18 is current stable. Should the initial
  implementation target 18 and upgrade, or abstract over versions from the start?

- **JIT vs AOT priority**: Should Phase 1-5 produce a JIT (via LLVM MCJIT or
  OrcJIT) or focus on AOT object file emission first? JIT gives faster feedback
  during development; AOT is more useful for end users.

- **Runtime library boundary**: Which runtime helpers are compiled into the Flux
  binary vs linked as a separate `flux_runtime.a`? This affects binary portability.

- **Shared runtime helpers with Cranelift JIT**: The `src/jit/runtime_helpers.rs`
  functions are `extern "C"` and could be linked into LLVM-compiled programs
  directly. Should they live in a shared `src/runtime/native_helpers.rs` instead?

- **Optimisation level CLI**: Should `--opt-level` be a top-level flag shared
  across all backends, or LLVM-specific?

---

## Future possibilities

- **WASM target**: Once the LLVM backend is functional, emitting WASM is a
  `--target wasm32-unknown-unknown` flag away.

- **Cross-compilation**: `--target aarch64-unknown-linux-gnu` etc. for
  producing binaries for different architectures from a single build machine.

- **Link-time optimisation (LTO)**: Whole-program LLVM IR emission enables
  LTO across Flux modules, eliminating cross-module call overhead.

- **Profile-guided optimisation (PGO)**: LLVM supports PGO natively. Flux
  programs could be instrumented, profiled, and recompiled with profile data
  for maximum performance on specific workloads.

- **LLVM sanitisers**: Address sanitiser, memory sanitiser, and undefined
  behaviour sanitiser via LLVM instrumentation — useful for testing the runtime.

- **Replace Cranelift JIT with LLVM OrcJIT**: Once the LLVM backend is stable,
  the Cranelift JIT could be retired in favour of LLVM OrcJIT for a single
  unified native backend with configurable optimisation levels. This would
  simplify the backend architecture at the cost of slower JIT compilation.

- **MLIR migration**: As Flux's IR matures, migrating from CFG IR to an MLIR
  dialect would unlock more powerful optimisation passes and better
  interoperability with the broader compiler infrastructure ecosystem.
