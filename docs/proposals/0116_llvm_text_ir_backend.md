- Feature Name: CoreToLlvm — Text IR Code Generation Backend
- Start Date: 2026-03-23
- Status: Draft
- Proposal PR:
- Flux Issue:

# Proposal 0116: CoreToLlvm — Text IR Code Generation Backend (GHC-style)

## Summary

Build a **new, fully independent LLVM backend** for Flux — named `core_to_llvm` following GHC's `CmmToLlvm` naming convention — that generates LLVM IR as text (`.ll` files). This is not a port of the existing `src/llvm/` backend — it is a clean-room implementation that:

1. Translates Core IR into a Rust-native LLVM IR AST (`src/core_to_llvm/ir/`)
2. Pretty-prints the AST to `.ll` text files
3. Invokes `opt` and `llc` as external processes for optimization and native code generation
4. Links against a minimal, **self-contained Flux runtime** (written in C) — not the existing VM/JIT runtime

The generated LLVM IR is self-contained. There are no `rt_*` helper calls back into the Rust process, no `JitContext`, no `NativeHelpers`, no shared runtime module. Every operation — arithmetic, closures, ADTs, effects, GC — is either inlined as LLVM IR or calls into a small C runtime library that is compiled and linked separately.

The module is named `core_to_llvm` to mirror GHC's `CmmToLlvm` convention (`<source IR>To<target>`) and to coexist cleanly with the existing `src/llvm/` module, which remains untouched.

---

## Motivation

### The current LLVM backend is a JIT adapter, not a true backend

The existing `src/llvm/` backend (Proposal 0105) works by translating CFG IR into LLVM IR via the `llvm-sys` C API, then executing it in-process via MCJIT. Every non-trivial operation — `rt_add`, `rt_make_closure`, `rt_call_jit_function`, `rt_make_string`, `rt_make_adt` — calls back into Rust `extern "C"` functions defined in `runtime/native_helpers.rs`. The LLVM IR is essentially a thin dispatch layer that bounces every operation back into the Flux runtime.

This means:
- LLVM cannot optimize across runtime boundaries (it sees opaque `call` instructions)
- The generated code is not standalone — it requires the Flux Rust binary to be running
- No AOT binary emission is possible without shipping the entire Flux runtime
- LLVM's inliner, vectorizer, and loop optimizations are blocked by the opaque `rt_*` calls

### What a real LLVM backend looks like

GHC's LLVM backend generates **complete, self-contained LLVM IR**. Arithmetic is native LLVM `add`/`sub`/`mul`. Memory allocation calls a small C runtime (`rts/`). Closures are LLVM structs with known layouts. Pattern matching compiles to `switch` instructions. The generated `.ll` file can be compiled to a standalone binary with `opt` + `llc` + `cc`, with no dependency on the GHC process.

This is what Flux needs. A backend where LLVM can see, inline, and optimize the actual computation — not opaque calls to a Rust runtime.

### Specific benefits

1. **LLVM can optimize**: When integer addition is `add i64 %a, %b` instead of `call i64 @rt_add(...)`, LLVM can constant-fold, vectorize, and eliminate dead code.

2. **Standalone binaries**: `flux program.flx --core-to-llvm --emit-binary -o program` produces a native executable that links against a ~5KB C runtime. No Rust, no Cargo, no Flux installation required to run it.

3. **No `llvm-sys` build dependency**: Text IR generation is pure Rust. LLVM tools (`opt`, `llc`) are only needed at runtime, and only the command-line tools — not the 300 MB dev headers.

4. **Debuggable output**: `--emit-llvm` writes a readable `.ll` file. Feed it to `opt`, `llc`, `lli`, or any LLVM tool directly.

5. **Cross-compilation**: `llc --target=aarch64-unknown-linux-gnu` works out of the box with text IR.

6. **LLVM version flexibility**: Text IR is stable across LLVM 15–19+. No `llvm-sys` version pinning.

### The GHC precedent

GHC's LLVM backend has been in production since 2010 (GHC 7.0). It generates LLVM IR as text, invokes `opt` + `llc` as subprocesses, and links against a small C runtime (the RTS). Key lessons:

- **Text IR is stable**: GHC supports LLVM 15–19 from one codebase
- **Self-contained IR works**: The generated `.ll` files compile independently
- **The abstraction is small**: ~900 lines for the LLVM IR AST + pretty-printer
- **mem2reg strategy**: GHC emits all variables as `alloca` + `store`/`load`, relying on LLVM's `mem2reg` pass to promote them to SSA registers — this massively simplifies code generation

---

## Guide-level explanation

### For Flux users

```bash
# Compile and run via CoreToLlvm (AOT)
cargo run --features core_to_llvm -- program.flx --core-to-llvm

# Emit standalone binary
cargo run --features core_to_llvm -- program.flx --core-to-llvm --emit-binary -o program
./program   # runs without Flux installed

# Inspect generated LLVM IR
cargo run --features core_to_llvm -- program.flx --core-to-llvm --emit-llvm -o program.ll
cat program.ll

# Use standard LLVM tools
opt -O2 program.ll -o program.bc
llc program.bc -o program.s
cc program.s -lflux_rt -o program
```

The `--core-to-llvm` flag activates the new backend. It coexists with the existing `--llvm` flag (which uses the old `llvm-sys` backend) indefinitely — they are completely independent code paths.

### For compiler contributors

The new backend is a completely separate module tree, named after GHC's `CmmToLlvm` convention:

```
src/core_to_llvm/
├── ir/           Pure Rust LLVM IR AST + pretty-printer
├── codegen/      Core IR → LLVM IR translation
├── pipeline.rs   External tool invocation (opt, llc, cc)
├── target.rs     Host triple / data layout detection
└── mod.rs        Public API

runtime/c/
├── flux_rt.h     Runtime header
├── flux_rt.c     Minimal C runtime (~500 lines)
├── gc.c          Simple bump allocator + mark-sweep GC
└── Makefile      Build to libflux_rt.a
```

This backend does **not** depend on:
- `src/runtime/` (VM runtime)
- `src/runtime/native_context.rs` / `native_helpers.rs` (JIT/LLVM shared runtime)
- `src/jit/` (Cranelift backend)
- `src/llvm/` (existing LLVM backend)
- `llvm-sys` (LLVM C API bindings)

### How GHC does it — and how Flux adapts it

| GHC component | Flux equivalent |
|---------------|-----------------|
| STG → Cmm lowering | Core IR (post-Aether) |
| `GHC.Llvm.Syntax` / `GHC.Llvm.Types` | `src/core_to_llvm/ir/syntax.rs` / `types.rs` |
| `GHC.Llvm.Ppr` (pretty-printer) | `src/core_to_llvm/ir/ppr.rs` |
| `GHC.CmmToLlvm.CodeGen` | `src/core_to_llvm/codegen/` |
| External `opt` + `llc` | `src/core_to_llvm/pipeline.rs` |
| GHC RTS (`rts/`) | `runtime/c/flux_rt.c` |
| `ghccc` calling convention | `fastcc` or standard `ccc` with known signatures |

---

## Reference-level explanation

### Value representation

Flux uses NaN boxing. Every value is an `i64`:

```llvm
; Constants
@FLUX_TAG_INT   = private constant i64 4607182418800017408  ; int tag
@FLUX_TAG_TRUE  = private constant i64 9221120237041090562  ; true
@FLUX_TAG_FALSE = private constant i64 9221120237041090563  ; false
@FLUX_TAG_NONE  = private constant i64 9221120237041090564  ; unit/none
@FLUX_TAG_MASK  = private constant i64 -4503599627370496    ; pointer mask

; Tag an integer
define i64 @flux_tag_int(i64 %raw) {
  %tagged = or i64 %raw, 4607182418800017408
  ret i64 %tagged
}

; Extract integer from tagged value
define i64 @flux_untag_int(i64 %val) {
  %raw = and i64 %val, 4503599627370495
  ret i64 %raw
}

; Check if value is a heap pointer
define i1 @flux_is_ptr(i64 %val) {
  %masked = and i64 %val, -4503599627370496
  %is_ptr = icmp eq i64 %masked, 0
  ret i1 %is_ptr
}
```

These are defined as LLVM functions in the generated `.ll` file and inlined by LLVM's optimizer. No external runtime call needed.

### Arithmetic — native LLVM, not runtime calls

The current backend emits `call i64 @rt_add(...)` for every addition. The new backend emits native LLVM IR:

```llvm
; Integer addition (type-specialized path)
define i64 @flux_iadd(i64 %a, i64 %b) alwaysinline {
  %a_raw = call i64 @flux_untag_int(i64 %a)
  %b_raw = call i64 @flux_untag_int(i64 %b)
  %sum = add i64 %a_raw, %b_raw
  %result = call i64 @flux_tag_int(i64 %sum)
  ret i64 %result
}

; Float addition
define i64 @flux_fadd(i64 %a, i64 %b) alwaysinline {
  %a_bits = bitcast i64 %a to double
  %b_bits = bitcast i64 %b to double
  %sum = fadd double %a_bits, %b_bits
  %result = bitcast double %sum to i64
  ret i64 %result
}
```

After LLVM's inliner and constant propagation, `flux_iadd(flux_tag_int(3), flux_tag_int(4))` reduces to a single constant. This is impossible with opaque `rt_add` calls.

For polymorphic `Add` (where operand types are unknown at compile time), a type-dispatch function checks the NaN-box tag at runtime and branches to the appropriate path — but this is still pure LLVM IR, not a Rust FFI call.

### Closure representation

Closures are heap-allocated structs with a known LLVM layout:

```llvm
; Closure: function pointer + captures array
%Closure = type {
  ptr,       ; function pointer (code)
  i32,       ; arity (number of remaining params)
  i32,       ; num_captures
  [0 x i64]  ; captures (variable-length, sized at allocation)
}

; Allocate a closure
define ptr @flux_make_closure(ptr %fn_ptr, i32 %arity, i64* %captures, i32 %ncaptures) {
  %size = add i32 16, ...  ; header + ncaptures * 8
  %mem = call ptr @flux_gc_alloc(i32 %size)
  ; store fn_ptr, arity, ncaptures, captures...
  ret ptr %mem
}

; Call a closure
define i64 @flux_call_closure(ptr %closure, i64* %args, i32 %nargs) {
  %fn_ptr = load ptr, ptr %closure
  %arity = ... ; load arity
  ; if nargs == arity: direct call
  ; if nargs < arity: partial application (new closure with accumulated args)
  ; if nargs > arity: over-application (call, then call result with remaining args)
}
```

These helper functions are emitted directly in the `.ll` file as LLVM IR, not as external C calls. LLVM can inline them at call sites.

### ADT representation

ADTs are heap-allocated tagged structs:

```llvm
; ADT object: tag + fields
%ADT = type {
  i32,       ; constructor tag
  i32,       ; num_fields
  [0 x i64]  ; fields (variable-length)
}

; Construct: Some(42)
define i64 @construct_Some(i64 %field0) {
  %mem = call ptr @flux_gc_alloc(i32 24)     ; 8 header + 8 tag + 8 field
  %tag_ptr = getelementptr %ADT, ptr %mem, i32 0, i32 0
  store i32 1, ptr %tag_ptr                   ; constructor tag = 1 (Some)
  %nf_ptr = getelementptr %ADT, ptr %mem, i32 0, i32 1
  store i32 1, ptr %nf_ptr                    ; 1 field
  %field_ptr = getelementptr %ADT, ptr %mem, i32 0, i32 2, i32 0
  store i64 %field0, ptr %field_ptr
  %tagged = ptrtoint ptr %mem to i64          ; NaN-box as pointer
  ret i64 %tagged
}

; Pattern match: switch on constructor tag
define i64 @match_option(i64 %val) {
  %ptr = inttoptr i64 %val to ptr
  %tag_ptr = getelementptr %ADT, ptr %ptr, i32 0, i32 0
  %tag = load i32, ptr %tag_ptr
  switch i32 %tag, label %unreachable [
    i32 0, label %case_None
    i32 1, label %case_Some
  ]
case_None:
  ...
case_Some:
  %field_ptr = getelementptr %ADT, ptr %ptr, i32 0, i32 2, i32 0
  %field = load i64, ptr %field_ptr
  ...
unreachable:
  unreachable
}
```

### String representation

Strings are heap-allocated length-prefixed byte arrays:

```llvm
%FluxString = type {
  i32,       ; length
  [0 x i8]   ; UTF-8 bytes (variable-length)
}
```

String operations that are simple (length, indexing, concatenation) are inlined as LLVM IR. Complex operations (regex, formatting) call the C runtime.

### Minimal C runtime (`runtime/c/flux_rt.c`)

The C runtime is intentionally tiny (~500 lines). It provides only what cannot be expressed as pure LLVM IR:

```c
// runtime/c/flux_rt.h

// --- GC ---
void  flux_gc_init(size_t heap_size);
void* flux_gc_alloc(uint32_t size);
void  flux_gc_collect(void);
void  flux_gc_push_root(void** root);
void  flux_gc_pop_root(void);

// --- I/O ---
void  flux_print(int64_t value);
void  flux_println(int64_t value);
int64_t flux_read_line(void);
int64_t flux_read_file(int64_t path);
int64_t flux_write_file(int64_t path, int64_t content);

// --- String helpers (complex ops only) ---
int64_t flux_string_concat(int64_t a, int64_t b);
int64_t flux_string_slice(int64_t s, int64_t start, int64_t end);
int64_t flux_int_to_string(int64_t n);
int64_t flux_float_to_string(int64_t f);
int64_t flux_string_to_int(int64_t s);

// --- Effect handlers ---
void    flux_push_handler(int64_t effect_tag, void* handler_fn, void* resume_fn);
void    flux_pop_handler(void);
int64_t flux_perform(int64_t effect_tag, int64_t arg);
int64_t flux_resume(int64_t continuation, int64_t value);

// --- HAMT (persistent map) ---
int64_t flux_hamt_empty(void);
int64_t flux_hamt_get(int64_t map, int64_t key);
int64_t flux_hamt_set(int64_t map, int64_t key, int64_t value);
int64_t flux_hamt_delete(int64_t map, int64_t key);

// --- Entry point ---
void    flux_rt_init(void);
void    flux_rt_shutdown(void);
```

Everything else — integer/float arithmetic, boolean logic, comparisons, closure construction, ADT construction, pattern matching, control flow — is **pure LLVM IR** in the generated `.ll` file.

### Aether (Perceus RC) integration

The Aether passes (dup/drop/reuse) run on Core IR *before* LLVM codegen. The generated LLVM IR emits the actual reference counting operations inline:

```llvm
; Dup: increment reference count
define void @flux_dup(i64 %val) alwaysinline {
  %is_ptr = call i1 @flux_is_ptr(i64 %val)
  br i1 %is_ptr, label %do_dup, label %done
do_dup:
  %ptr = inttoptr i64 %val to ptr
  %rc_ptr = getelementptr i64, ptr %ptr, i64 -1  ; RC stored before object
  %old = load i64, ptr %rc_ptr
  %new = add i64 %old, 1
  store i64 %new, ptr %rc_ptr
  br label %done
done:
  ret void
}

; Drop: decrement RC, free if zero
define void @flux_drop(i64 %val) alwaysinline {
  %is_ptr = call i1 @flux_is_ptr(i64 %val)
  br i1 %is_ptr, label %do_drop, label %done
do_drop:
  %ptr = inttoptr i64 %val to ptr
  %rc_ptr = getelementptr i64, ptr %ptr, i64 -1
  %old = load i64, ptr %rc_ptr
  %new = sub i64 %old, 1
  store i64 %new, ptr %rc_ptr
  %is_zero = icmp eq i64 %new, 0
  br i1 %is_zero, label %free, label %done
free:
  call void @flux_gc_free(ptr %ptr)
  br label %done
done:
  ret void
}

; Reuse: if RC==1, reuse allocation; otherwise allocate fresh
define ptr @flux_drop_reuse(i64 %val, i32 %size) alwaysinline {
  %ptr = inttoptr i64 %val to ptr
  %rc_ptr = getelementptr i64, ptr %ptr, i64 -1
  %rc = load i64, ptr %rc_ptr
  %unique = icmp eq i64 %rc, 1
  br i1 %unique, label %reuse, label %fresh
reuse:
  ret ptr %ptr  ; reuse in-place
fresh:
  call void @flux_drop(i64 %val)
  %new_mem = call ptr @flux_gc_alloc(i32 %size)
  ret ptr %new_mem
}
```

LLVM can inline and optimize these: if a value is provably unique (RC==1), the drop+reuse path reduces to a no-op reuse.

### Module structure

```
src/core_to_llvm/
├── mod.rs                 Public API: core_to_llvm_compile, core_to_llvm_emit_binary, etc.
├── ir/
│   ├── mod.rs             Re-exports
│   ├── types.rs           LlvmType enum (~80 lines)
│   ├── syntax.rs          LlvmModule, LlvmFunction, LlvmBlock, LlvmInstr (~300 lines)
│   └── ppr.rs             Display impls → valid .ll text (~400 lines)
├── codegen/
│   ├── mod.rs             compile_program() top-level
│   ├── function.rs        Core function → LlvmFunction
│   ├── expr.rs            CoreExpr → LlvmInstr sequences
│   ├── arith.rs           Arithmetic (native LLVM, type-specialized)
│   ├── closure.rs         Closure construction, application, partial application
│   ├── adt.rs             ADT construction, field access, pattern matching
│   ├── string.rs          String construction, inline ops
│   ├── effects.rs         Effect handler push/pop/perform/resume
│   ├── aether.rs          Dup/drop/reuse emission
│   ├── builtins.rs        Built-in function code generation
│   └── prelude.rs         Tag constants, helper functions emitted in every module
├── pipeline.rs            Subprocess invocation (opt, llc, cc)
└── target.rs              Host triple / data layout detection

runtime/c/
├── flux_rt.h              Public runtime API
├── flux_rt.c              Core runtime: init, shutdown, print, I/O
├── gc.c                   GC: bump allocator + mark-sweep
├── hamt.c                 Persistent hash map (HAMT)
├── effects.c              Effect handler stack + continuation capture
├── string.c               String helpers (concat, slice, format)
└── Makefile               Build libflux_rt.a / libflux_rt.so
```

### Input IR: Core IR (post-Aether), not CFG IR

The new backend consumes **Core IR** (after all Core passes including Aether), not the CFG IR used by the existing backends. This is a deliberate choice:

- Core IR is higher-level — it preserves `let`, `case`, `lambda`, `app` structure that maps naturally to LLVM IR
- CFG IR is already lowered to basic blocks and terminators, which duplicates work LLVM will do anyway
- GHC's approach: translate from a high-level IR (Cmm, which is higher than machine code) and let LLVM handle the CFG

The code generator uses GHC's `alloca`/`store`/`load` strategy for variables, relying on LLVM's `mem2reg` to construct SSA form. This means:
- No phi nodes in the generated IR
- No manual SSA construction
- Variables are simply `alloca`'d stack slots
- LLVM's optimizer promotes them to registers

```llvm
; Generated from: let x = 42 in x + 1
entry:
  %x = alloca i64
  store i64 42, ptr %x
  %x.0 = load i64, ptr %x
  %result = add i64 %x.0, 1
  ret i64 %result

; After mem2reg (LLVM optimizer):
entry:
  %result = add i64 42, 1
  ret i64 %result

; After constant folding:
entry:
  ret i64 43
```

### Calling convention

Functions use the `fastcc` calling convention for internal Flux functions and `ccc` (standard C) for runtime calls:

```llvm
; Internal Flux function: fastcc for tail call optimization
define fastcc i64 @flux_fibonacci(i64 %n) {
  ...
  %result = tail call fastcc i64 @flux_fibonacci(i64 %n_minus_1)
  ret i64 %result
}

; Runtime call: standard C convention
declare ccc void @flux_print(i64 %value)
```

`fastcc` enables LLVM's tail call optimization without requiring `musttail` — LLVM can use registers for arguments and guarantee TCO for self-recursive and mutually recursive calls.

### External tool pipeline

```
Source (.flx)
  → Flux compiler (Rust)
      → Lexer → Parser → AST → Type Inference → Core IR → Aether
      → LLVM codegen: Core IR → LlvmModule (Rust AST)
      → Pretty-print: LlvmModule → program.ll (text file)
  → opt (LLVM optimizer)
      program.ll → program.bc (optimized bitcode)
  → llc (LLVM code generator)
      program.bc → program.o (native object file)
  → cc/lld (system linker)
      program.o + libflux_rt.a → program (native binary)
```

```rust
// src/core_to_llvm/pipeline.rs

pub fn compile_and_run(ll_text: &str, opt_level: u32) -> Result<i32, String> {
    let dir = tempfile::tempdir()?;
    let ll_path = dir.path().join("program.ll");
    std::fs::write(&ll_path, ll_text)?;

    // opt: .ll → .bc
    let bc_path = dir.path().join("program.bc");
    run_tool("opt", &[
        &format!("--passes=default<O{}>", opt_level),
        ll_path.to_str().unwrap(),
        "-o", bc_path.to_str().unwrap(),
    ])?;

    // llc: .bc → .o
    let obj_path = dir.path().join("program.o");
    run_tool("llc", &[
        bc_path.to_str().unwrap(),
        "-o", obj_path.to_str().unwrap(),
        "--filetype=obj",
    ])?;

    // cc: .o + libflux_rt.a → executable
    let exe_path = dir.path().join("program");
    run_tool("cc", &[
        obj_path.to_str().unwrap(),
        "-lflux_rt", "-L", flux_rt_lib_dir(),
        "-o", exe_path.to_str().unwrap(),
    ])?;

    // Execute
    let output = Command::new(&exe_path).output()?;
    Ok(output.status.code().unwrap_or(1))
}
```

### Cargo feature

```toml
[features]
core_to_llvm = []  # pure Rust, no native build dependency

# The existing llvm feature is completely independent
llvm = ["llvm-sys"]
```

`cargo build --features core_to_llvm` works on **any machine** — no LLVM dev headers, no `LLVM_SYS_180_PREFIX`. The `opt` and `llc` tools are only needed at runtime when `--core-to-llvm` is invoked.

### Implementation phases

**Phase 1 — LLVM IR AST and pretty-printer** (~1 week)
- `src/core_to_llvm/ir/types.rs` — LlvmType enum
- `src/core_to_llvm/ir/syntax.rs` — LlvmModule, LlvmFunction, LlvmBlock, LlvmInstr
- `src/core_to_llvm/ir/ppr.rs` — Display impls producing valid `.ll` text
- Validation: hand-written `LlvmModule` → `.ll` → `opt --verify`

**Phase 2 — Prelude and value representation** (~1 week)
- `codegen/prelude.rs` — NaN-box tag constants, `flux_tag_int`, `flux_untag_int`, `flux_is_ptr`, `flux_dup`, `flux_drop`, `flux_drop_reuse`
- `codegen/arith.rs` — `flux_iadd`, `flux_isub`, `flux_imul`, `flux_idiv`, `flux_fadd`, polymorphic dispatch
- Smoke test: `let x = 1 + 2 in x` compiles to `.ll`, runs through `opt` + `llc` + `cc`

**Phase 3 — Functions and control flow** (~2 weeks)
- `codegen/function.rs` — Core function → LlvmFunction (alloca/store/load pattern)
- `codegen/expr.rs` — CoreExpr translation (let, if/else, case)
- `fastcc` calling convention, tail call annotation
- Test: fibonacci, factorial compile and produce correct results

**Phase 4 — Closures and higher-order functions** (~2 weeks)
- `codegen/closure.rs` — Closure struct layout, `flux_make_closure`, `flux_call_closure`
- Partial application, over-application
- Free variable capture
- Test: `map`, `filter`, `fold` over lists work correctly

**Phase 5 — ADTs and pattern matching** (~2 weeks)
- `codegen/adt.rs` — ADT struct layout, constructor emission, `switch` on tag
- Field extraction via GEP
- Nested pattern matching
- Test: `Option`, `List`, user-defined ADTs

**Phase 6 — C runtime** (~2 weeks)
- `runtime/c/gc.c` — Bump allocator with mark-sweep collection
- `runtime/c/flux_rt.c` — I/O, init/shutdown
- `runtime/c/string.c` — String concat, formatting, conversion
- `runtime/c/hamt.c` — Persistent hash map
- `runtime/c/effects.c` — Effect handler stack, continuation capture/resume
- Build system: Makefile producing `libflux_rt.a`

**Phase 7 — Aether integration** (~1 week)
- `codegen/aether.rs` — Emit `flux_dup`, `flux_drop`, `flux_drop_reuse` from Core Aether annotations
- Verify: no leaks on recursive data structure examples

**Phase 8 — Pipeline and integration** (~1 week)
- `pipeline.rs` — `opt`/`llc`/`cc` subprocess management
- `target.rs` — detect host triple via `llvm-config --host-target`
- CLI integration: `--core-to-llvm`, `--emit-llvm`, `--emit-binary`
- End-to-end: `cargo run --features core_to_llvm -- examples/basics/fibonacci.flx --core-to-llvm`

**Phase 9 — Parity and benchmarks** (~2 weeks)
- Run all examples through VM, JIT, and CoreToLlvm backends
- Parity script: `scripts/release/check_parity.sh` extended for `--core-to-llvm`
- Benchmark suite: compare runtime performance across all backends
- Expected: CoreToLlvm significantly faster than VM/JIT on numeric workloads

---

## Drawbacks

- **New C runtime to maintain**: The C runtime (`runtime/c/`) is a new maintenance surface separate from the Rust VM runtime. However, it is intentionally minimal (~500 lines) and covers only I/O, GC, strings, HAMT, and effects.

- **Subprocess overhead**: Invoking `opt` + `llc` + `cc` adds ~200-500ms per compilation. This is acceptable for AOT release builds but too slow for interactive use. The VM and Cranelift JIT remain the interactive backends.

- **Duplicate implementations**: Some operations (HAMT, effect handlers, string ops) are implemented in both Rust (`src/runtime/`) and C (`runtime/c/`). The C versions are simpler (no Rc, no Value enum) but must stay semantically equivalent. Parity tests enforce this.

- **GC complexity**: The C runtime needs its own GC. The initial implementation is a simple bump allocator with mark-sweep, adequate for correctness but not optimized. A production-quality GC (generational, concurrent) is a future effort.

- **Large implementation effort**: This is a ground-up backend, not a port. Estimated ~6000 lines of Rust (codegen) + ~2000 lines of C (runtime). However, each phase delivers incremental value and can be tested independently.

---

## Rationale and alternatives

### Why a new backend instead of fixing the existing one?

The fundamental issue with the existing `src/llvm/` backend is architectural: it calls back into the Rust runtime for every operation. Fixing this would require:
1. Rewriting every `rt_*` helper as LLVM IR
2. Removing the `JitContext` dependency
3. Building a separate C runtime for linking

This is equivalent to writing a new backend — but harder, because it must be done incrementally while maintaining backward compatibility with the existing `llvm-sys` infrastructure. A clean-room implementation is simpler and produces a better result.

### Why consume Core IR instead of CFG IR?

The existing backends (VM, Cranelift JIT, old LLVM) consume CFG IR. The new LLVM backend consumes Core IR because:

- **Core preserves structure**: `let`, `case`, `lambda` map cleanly to LLVM `alloca`/`switch`/function-ptr patterns
- **CFG duplicates LLVM's job**: CFG IR is already lowered to basic blocks — LLVM will re-analyze and re-optimize this structure anyway
- **GHC's lesson**: GHC translates from Cmm (higher-level than machine code) and lets LLVM build its own CFG. Giving LLVM a higher-level input produces better optimized output
- **Aether annotations are on Core**: Dup/drop/reuse markers are attached to Core IR nodes, making emission straightforward

### Why a C runtime instead of Rust?

- **Linkability**: A C static library (`libflux_rt.a`) links with any linker on any platform. A Rust static library would pull in `libstd` or require `#![no_std]`.
- **Simplicity**: The runtime is ~500 lines of C — no allocator, no trait objects, no generics. It's dead-simple to audit and debug.
- **GHC precedent**: GHC's RTS is written in C for exactly these reasons.
- **ABI stability**: C ABI is the universal FFI. The LLVM IR declares `extern` C functions that link cleanly.

### Why not skip the C runtime entirely?

Some operations cannot be expressed as pure LLVM IR:
- I/O (requires OS syscalls)
- GC (requires stack walking / root tracking)
- HAMT (complex persistent data structure)
- Effect handler continuations (require setjmp/longjmp or stack copying)

These require a runtime. The C runtime is the minimal viable solution.

---

## Prior art

### GHC (Haskell) — primary inspiration

GHC's LLVM backend: Cmm → Haskell LLVM AST → `.ll` text → `opt` → `llc` → `.o`, linked against the RTS (C runtime, ~50K lines). GHC demonstrates that a functional language can produce competitive native code via text-based LLVM IR generation with a separate C runtime. GHC has maintained this architecture for 15+ years across LLVM 7–19.

Key technique adopted: **alloca/store/load + mem2reg** — emit all variables as stack allocations, let LLVM promote to SSA.

### Lean 4

Lean 4's compiler emits LLVM IR as text and links against a C runtime for GC, I/O, and object allocation. The runtime is ~3000 lines of C. Lean demonstrates that a dependently-typed functional language with reference counting can use this architecture effectively.

### Koka

Koka compiles to C, which is then compiled by a C compiler (GCC/Clang). The Perceus reference counting is emitted as C code. While Koka targets C rather than LLVM IR, the principle is identical: emit a textual representation of the target language and invoke an external compiler. Flux's Aether (Perceus implementation) benefits from the same approach — RC operations become visible LLVM IR that the optimizer can reason about.

### Zig

Zig's LLVM backend emits LLVM IR as text (`src/codegen/llvm.zig`). Zig also has a self-hosted x86-64 backend that bypasses LLVM entirely. This dual-backend strategy (fast self-hosted + optimizing LLVM) mirrors Flux's VM/Cranelift (fast) + LLVM (optimizing) split.

### Crystal

Crystal uses `llvm-sys`-style bindings and suffers from version lock-in and build complexity — the exact problems this proposal avoids by using text IR generation.

---

## Unresolved questions

- **GC strategy**: Should the initial C runtime use bump allocation + mark-sweep, or reference counting (matching Aether's model)? Bump+sweep is simpler; RC is more natural with Perceus but requires cycle detection for effects.

- **Effect handler implementation**: Should continuations use `setjmp`/`longjmp` (simple, portable), stack copying (more flexible), or CPS transformation at the Core IR level (no runtime support needed)?

- **LLVM version range**: Minimum LLVM 15 (opaque pointers) or LLVM 18 (matching current `llvm-sys` target)?

- **Long-term coexistence**: Should `core_to_llvm` and `llvm` coexist permanently as separate backends, or should the old `llvm` backend eventually be retired?

- **Build system for C runtime**: Makefile, CMake, or `cc` crate integrated into Cargo build?

- **Polymorphic operations**: When types are not known at compile time (type inference produces `Any`), the codegen must emit tag-checking dispatch. How much of this should be inlined vs. a dispatch table?

---

## Future possibilities

- **Standalone binary distribution**: `flux build program.flx` produces a native binary with no Flux dependency. Ship pre-compiled `libflux_rt.a` for common platforms.

- **WASM target**: `llc --target=wasm32` + WASM-compatible C runtime = Flux in the browser.

- **Cross-compilation**: `llc --target=aarch64-unknown-linux-gnu` works out of the box.

- **LTO**: Emit one `.ll` per Flux module, `llvm-link` them, then `opt -O2` for whole-program optimization.

- **Profile-guided optimization**: LLVM PGO works natively with `.ll` files.

- **Custom LLVM passes**: Flux-specific optimizations (NaN-box tag propagation, closure inlining hints) as `opt` plugins.

- **Shared C runtime with other functional languages**: The minimal C runtime (GC + effects + HAMT) could be extracted as a reusable library for other functional language implementations targeting LLVM.
