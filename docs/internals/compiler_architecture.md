# Flux Compiler Architecture

This document describes the current compiler architecture as implemented in the
repository today.

For detailed IR semantics, see:
- `docs/internals/ir_pipeline.md`
- `docs/internals/aether.md`
- `docs/internals/core_aether_backend_boundaries.md`
- `docs/internals/bytecode.md`

## Current Shape

Flux has one canonical semantic pipeline and two backend families:

```text
Source
  -> syntax/        (lexer, parser, module graph)
  -> Program AST
  -> HM inference   (ast/type_infer + types/)
  -> core/          (canonical semantic IR)
  -> aether/        (backend-only ownership/reuse lowering product)

Production backend path:
  -> cfg/           (backend-neutral CFG IR)
  -> bytecode/      (VM compiler + VM runtime)

Native backend path:
  -> lir/           (native-only low-level IR)
  -> llvm/  (LLVM IR + native compilation pipeline)
```

The key boundaries are:

```text
AST -> Core -> Aether -> cfg -> bytecode/VM
AST -> Core -> Aether -> lir -> LLVM/native
```

`src/shared_ir/` is shared ID/plumbing support. It is not a compiler stage.

## Entry Points

There are three high-value orchestration entrypoints in the current codebase:

| Entry point | Path | Responsibility |
|---|---|---|
| CLI orchestration | `src/main.rs` | Parse flags, load modules, select VM vs native path, emit dumps |
| VM compiler driver | `src/bytecode/compiler/pipeline.rs` | Run the staged compiler pipeline ending in bytecode |
| Semantic backend lowering | `src/cfg/mod.rs` | Build Core, run Core+Aether, lower to CFG IR |

For the native backend, the CLI calls native-specific helpers on
`bytecode::compiler::Compiler`, especially:
- `infer_expr_types_for_program`
- `dump_lir`
- `lower_to_lir_llvm_module`
- `dump_lir_llvm`

## Architectural Rules

- `src/core/` is the only semantic IR.
- `src/cfg/` is the backend IR for the VM-oriented production pipeline.
- `src/lir/` is a separate native backend IR used only for the LLVM/native path.
- `structured_ir` is retired and should not be reintroduced into production paths.
- Aether is not a second semantic IR. It is a backend-only lowering product
  derived from Core for maintained RC backends.

## Directory Map

| Area | Path | Role |
|---|---|---|
| Frontend syntax | `src/syntax/` | Lexer, parser, formatter, module graph, linter |
| AST utilities | `src/ast/` | Desugaring, constant folding, rename, free-vars, tail-call analysis, HM entrypoints |
| Type system | `src/types/` | HM types, schemes, substitutions, unification, effect-row inference |
| Semantic IR | `src/core/` | Core IR, AST lowering, Core passes, Core->CFG lowering support |
| Ownership pass | `src/aether/` | Borrow inference, dup/drop insertion, reuse, drop specialization, FBIP checks |
| Production backend IR | `src/cfg/` | CFG IR plus CFG passes and validation |
| VM compiler/runtime | `src/bytecode/` | Bytecode compiler, cache, opcodes, VM |
| Native backend IR | `src/lir/` | Low-level native IR and LLVM emission bridge |
| LLVM backend | `src/llvm/` | LLVM IR model, rendering, codegen prelude, binary pipeline |
| Runtime | `src/runtime/` | Shared runtime values and helpers |
| Shared IDs | `src/shared_ir/` | Shared identifier/plumbing types for backend layers |
| Diagnostics | `src/diagnostics/` | Error/warning model and rendering |

## Frontend and Analysis

### 1. Parsing and module loading

The CLI in `src/main.rs` builds a module graph, reads source files, lexes, and
parses them into `syntax::program::Program`.

Primary frontend components:
- `src/syntax/lexer/`
- `src/syntax/parser/`
- `src/syntax/module_graph/`
- `src/syntax/interner.rs`

### 2. AST-level transforms

Flux still uses AST-level helper passes before Core lowering in a few places:
- `desugar`
- `constant_fold_with_interner`
- `rename`
- `collect_free_vars_in_program`
- `find_tail_calls`

These are utility passes over the parsed `Program`, not the canonical semantic
pipeline.

In practice they are used in two main ways:
- analysis and diagnostics helpers on the parsed AST
- optional pre-Core optimization in dump/native helper flows

### 3. HM type inference

Type inference is driven from:
- `src/ast/type_infer/`
- `src/types/`

The bytecode compiler stores inferred expression types in
`Compiler.hm_expr_types`, and those inferred types are then used by later
lowering stages.

This inferred type map is the bridge between the frontend and both lowering
families:
- Core lowering uses it for type-directed Core construction
- the VM path carries it into `cfg::IrProgram`
- the native path reuses it before Core -> LIR lowering

## Canonical Semantic Pipeline

### 1. AST -> Core

`src/core/lower_ast/` lowers `Program` into `CoreProgram`.

Core is the semantic source of truth:
- surface syntax sugar is eliminated here
- semantic constructs like pattern matching remain explicit
- typed primops are selected using HM inference results
- top-level declarations are preserved in Core-owned metadata

Primary entrypoint:
- `core::lower_ast::lower_program_ast`

### 2. Core passes

`src/core/passes/` runs the standard Core simplification pipeline.

This is where semantic optimization belongs before any backend-specific IR is
introduced.

The implemented pass pipeline is:

### Stage 0: builtin promotion
- `promote_builtins`

### Stage 1: simplification
- `beta_reduce`
- `case_of_case`
- `case_of_known_constructor`
- `inline_lets`
- `elim_dead_let`

When `-O` is enabled, this simplifier stage iterates up to a small fixed point.

### Stage 2: normalization
- `evidence_pass`
- `anf_normalize`

### Stage 3: handoff to Aether
- semantic Core is complete at this point
- borrow-mode inference and Aether lowering happen after `run_core_passes*`
- Aether verification / FBIP checks operate on Aether-owned data

Important implementation points:
- `run_core_passes_with_interner`
- `run_core_passes_with_interner_and_warnings`

### 3. Aether after Core

`src/aether/` runs after the standard Core passes and before backend lowering.

Aether:
- infers borrow signatures
- inserts `AetherCall`, `Dup`, and `Drop`
- introduces `Reuse` and `DropSpecialized`
- verifies ownership/reuse invariants
- performs FBIP/FIP-related checks and diagnostics

Aether no longer lives inside `CoreExpr`. It produces a backend-only lowering
product:
- `AetherExpr`
- `AetherDef`
- `AetherProgram`

Operationally:
- `run_core_passes*` produces clean semantic Core
- `lower_core_to_aether_program(...)` produces `AetherProgram`
- maintained RC backends lower from `AetherProgram`

## Production Backend Path: Core -> Aether -> CFG -> Bytecode -> VM

This is the default execution path.

### CFG lowering

`src/cfg/mod.rs` orchestrates:

```text
Program AST
  -> core::lower_ast
  -> core passes
  -> lower_core_to_aether_program
  -> core::to_ir::lower_aether_to_ir
  -> IrProgram
```

Important entrypoints:
- `cfg::lower_program_to_ir`
- `cfg::lower_program_to_ir_with_optimize`
- `cfg::lower_program_to_ir_with_interner_and_warnings`
- `cfg::run_ir_pass_pipeline`

The resulting `IrProgram` is the backend-neutral CFG IR used by the VM path.

### CFG passes

`src/cfg/passes.rs` runs backend-level optimizations and cleanup on `IrProgram`
before bytecode emission.

The exact pass set evolves, but the CFG stage is the place for backend-neutral
control-flow cleanup and low-level optimization on the VM path.

### Bytecode compiler

`src/bytecode/compiler/` owns the VM compilation pipeline. The main staged flow
is:

```text
collection
  -> predeclaration
  -> type inference
  -> CFG IR lowering
  -> CFG/statement codegen
  -> finalization
```

The pass driver lives in:
- `src/bytecode/compiler/pipeline.rs`

The concrete driver is `Compiler::run_pipeline()`.

Important phase files:
- `passes/collection.rs`
- `passes/predeclaration.rs`
- `passes/type_inference.rs`
- `passes/ir_lowering.rs`
- `passes/codegen.rs`
- `passes/finalization.rs`

This is important architecturally:
- the bytecode compiler owns HM inference state (`hm_expr_types`, `type_env`)
- it lowers to CFG IR in `phase_ir_lowering()`
- it then performs VM codegen from that staged compiler context

The VM compiler is therefore not just an opcode emitter. It is the main
front-to-back compiler driver for the default execution path.

### Hybrid detail: VM codegen still owns frontend/compiler context

`phase_codegen()` receives both:
- the source `Program`
- the lowered `IrProgram`

So the VM pipeline today is not “pure IR in, bytecode out” in the strictest
sense. The compiler still carries semantic/frontend state through codegen and
finalization even though CFG IR is the production backend IR boundary.

### VM runtime

`src/bytecode/vm/` executes the bytecode with a stack-based VM.

Associated pieces:
- bytecode cache: `src/bytecode/bytecode_cache/`
- opcodes: `src/bytecode/op_code.rs`
- runtime values/helpers: `src/runtime/`

## Native Backend Path: Core -> LIR -> LLVM -> Native

This path is selected by `--native` / `--native`.

Unlike the VM path, the native path does not go through `cfg::IrProgram`.

Instead, the native CLI flow:
- merges the module graph into one `Program`
- reruns HM inference for the merged program
- lowers that merged program directly to Core
- runs Core passes and Aether
- lowers Core to LIR
- emits LLVM IR
- compiles/executes the native artifact

### LIR

`src/lir/` defines the native backend IR.

LIR is:
- lower-level than Core
- SSA-like
- explicit about memory operations, tagging, calls, and ownership hooks
- designed for native code generation

Important modules:
- `src/lir/lower.rs`
- `src/lir/emit_llvm.rs`

The main native lowering entrypoint is:
- `lir::lower::lower_program_with_interner`

LIR is explicitly described in code as the native backend IR; the VM path uses
CFG instead.

### LLVM IR generation

`Compiler::lower_to_lir_llvm_module()` in
`src/bytecode/compiler/mod.rs` performs:

```text
Program
  -> Core lowering
  -> Core passes
  -> LIR lowering
  -> LLVM module emission
```

Then `src/llvm/` provides:
- LLVM IR data model
- textual rendering
- runtime/helper prelude generation
- object/binary compilation pipeline

Important pieces:
- `src/llvm/ir/`
- `src/llvm/codegen/`
- `src/llvm/pipeline.rs`

The LLVM backend in this repo is split in two layers:
- `lir/emit_llvm.rs` translates LIR to the internal LLVM module model
- `llvm/` owns the LLVM IR model, rendering, prelude/runtime helpers,
  target data, and the external compile/link pipeline

### Native compilation pipeline

`src/main.rs` uses:
- `Compiler::lower_to_lir_llvm_module()`
- `llvm::render_module()`
- `llvm::pipeline::{compile_and_run, compile_to_binary}`

to emit LLVM IR text, build binaries, or run the produced native executable.

### Native test mode

`run_tests_native()` in `src/main.rs` does not execute the VM test runner.
Instead it synthesizes a temporary harness per `test_*` function and runs the
native backend on each harness individually.

That behavior matters when debugging native-only test failures, because the
native `--test` flow is effectively:

```text
test file
  -> enumerate test_* functions
  -> synthesize one temporary main() per test
  -> run native pipeline separately for each
```

## CLI-Orchestrated Pipelines

### Default run path

`flux file.flx`

```text
source
  -> module graph + parse
  -> Compiler::run_pipeline()
  -> HM inference
  -> Core
  -> Core passes + Aether
  -> CFG
  -> CFG pass pipeline
  -> bytecode
  -> VM
```

### Native run path

`flux file.flx --native`

```text
source
  -> module graph + parse
  -> merged Program
  -> infer_expr_types_for_program()
  -> Core
  -> Core passes + Aether
  -> LIR
  -> LLVM IR
  -> compile_and_run / compile_to_binary
```

### Dump surfaces

The main debugging surfaces are:
- `--dump-core`
- `--dump-core=debug`
- `--dump-aether`
- `--dump-lir`
- `--dump-lir-llvm`

When debugging semantics, `--dump-core` is the first canonical surface.

## Notes on Historical/Non-Canonical Paths

- The repo still contains documentation that mentions a Cranelift JIT backend,
  but there is no `src/jit/` tree in the current codebase.
- The current implemented native path is the LIR -> LLVM backend.
- `src/shared_ir/` should be treated as backend support code, not as a
  semantic or optimization stage.
- `cfg::IrProgram` is the production backend IR for the VM path, but the
  native path intentionally bypasses it and uses LIR instead.

## What To Inspect First

When orienting inside the compiler, the fastest route is:

1. `src/main.rs`
2. `src/bytecode/compiler/pipeline.rs`
3. `src/cfg/mod.rs`
4. `src/core/lower_ast/mod.rs`
5. `src/core/passes/mod.rs`
6. `src/aether/`
7. `src/lir/lower.rs`
8. `src/llvm/pipeline.rs`

## Practical Mental Model

If you need a compact rule for the current compiler:

```text
Core is meaning.
CFG is the production VM backend IR.
LIR is the native LLVM backend IR.
Aether lowers clean Core into the backend-only RC ownership form consumed by the maintained backends.
```
