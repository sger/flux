# Flux Compiler Architecture

This document describes the current architecture of the Flux compiler.

---

## Canonical Type/Effects Spec

For current type-system + effects semantics and diagnostics contracts, see:
- `docs/internals/type_system_effects.md`

This architecture document focuses on component layout; semantic truth for type/effects lives in that dedicated spec.

---

## Full Pipeline Diagram

```
                              ┌─────────────────┐
                              │   Source (.flx)  │
                              └────────┬────────┘
                                       │
                          ┌────────────▼────────────┐
                          │      Lexer (syntax/)     │
                          │  Source → Token stream    │
                          └────────────┬────────────┘
                                       │
                          ┌────────────▼────────────┐
                          │   Parser (syntax/)       │
                          │  Tokens → AST            │
                          │  (recursive descent)     │
                          └────────────┬────────────┘
                                       │
                    ┌──────────────────▼──────────────────┐
                    │       AST Passes (ast/)              │
                    │  desugar → constant_fold → rename    │
                    │  → free_vars → find_tail_calls       │
                    └──────────────────┬──────────────────┘
                                       │
                    ┌──────────────────▼──────────────────┐
                    │    HM Type Inference (ast/type_infer/)│
                    │  Algorithm W + effect rows            │
                    │  → TypeEnv + hm_expr_types            │
                    │                                       │
                    │  ┌─────────────────────────────────┐  │
                    │  │ Optional: type_informed_fold     │  │
                    │  │ (2-phase: infer → fold → reinfer)│  │
                    │  └─────────────────────────────────┘  │
                    └──────────────────┬──────────────────┘
                                       │
                    ┌──────────────────▼──────────────────┐
                    │    Core IR Lowering (core/lower_ast/) │
                    │  AST → CoreExpr                        │
                    │  Type-directed: IAdd/FAdd from HM     │
                    └──────────────────┬──────────────────┘
                                       │
              ┌────────────────────────▼────────────────────────┐
              │           Core Passes (core/passes/)             │
              │                                                  │
              │  1. beta_reduce        — App(Lam) → subst        │
              │  2. case_of_case       — push outer into inner   │
              │  3. case_of_known_ctor — static pattern match    │
              │  4. inline_lets        — occurrence-based inline │
              │  5. elim_dead_let      — remove unused bindings  │
              │  6. evidence_pass      — TR Handle/Perform → App │
              │  7. anf_normalize      — flatten to let-chains   │
              └────────────────────────┬────────────────────────┘
                                       │
              ┌────────────────────────▼────────────────────────┐
              │              Aether (aether/)                    │
              │                                                  │
              │  borrow_infer → insert → fusion →                │
              │  drop_spec → reuse → reuse_spec →                │
              │  verify / semantic FBIP                          │
              │                                                  │
              │  Emits explicit Core Aether nodes:               │
              │  AetherCall / Dup / Drop / Reuse /               │
              │  DropSpecialized                                 │
              └────────────────────────┬────────────────────────┘
                                       │
              ┌────────────────────────▼────────────────────────┐
              │      Core → Backend IR (core/to_ir/)             │
              │  CoreExpr → IrFunction/IrBlock/IrInstr           │
              │  • Uncurrying (Lam chains → multi-param fns)     │
              │  • Closure detection (free var capture)           │
              │  • Case → branch blocks + join                   │
              │  • Lower Aether nodes to backend-neutral IR      │
              │  • Typed IrParam from CoreType                   │
              └────────────────────────┬────────────────────────┘
                                       │
              ┌────────────────────────▼────────────────────────┐
              │        CFG Passes (cfg/passes.rs)                │
              │                                                  │
              │  1. dead_block_elimination                       │
              │  2. canonicalize_cfg                              │
              │  3. constant_fold                                │
              │  4. tail_call_introduction                       │
              │  5. local_cse                                    │
              │  6. intern_unit_adts                             │
              │  7. type_directed_unboxing                       │
              └────────────────────────┬────────────────────────┘
                                       │
                          ┌────────────┼────────────────────────┐
                          │            │                        │
               ┌──────────▼──────────┐ │ ┌─────────▼──────────┐ │ ┌──────────▼──────────┐
               │   VM Bytecode Path  │ │ │    JIT Path         │ │ │    LLVM Path         │
               │                     │ │ │                     │ │ │                      │
               │  Bytecode Compiler  │ │ │  Cranelift Compiler │ │ │  LLVM Compiler       │
               │  (bytecode/compiler)│ │ │  (jit/compiler.rs)  │ │ │  (llvm/compiler/)    │
               │                     │ │ │                     │ │ │                      │
               │  IrFunction →       │ │ │  IrFunction →       │ │ │  IrFunction →        │
               │  OpCode stream      │ │ │  Cranelift IR →     │ │ │  LLVM IR →           │
               │                     │ │ │  Machine code       │ │ │  Machine code        │
               │  ┌───────────────┐  │ │ │                     │ │ │                      │
               │  │ Evidence path │  │ │ │  ┌───────────────┐  │ │ │  ┌────────────────┐  │
               │  │ TR handlers → │  │ │ │  │ JitValueKind  │  │ │ │  │ Tagged values   │  │
               │  │ OpGetLocal +  │  │ │ │  │ Int/Float/Bool│  │ │ │  │ {i64 tag, i64  │  │
               │  │ OpCall        │  │ │ │  │ (unboxed in   │  │ │ │  │  payload}       │  │
               │  └───────────────┘  │ │ │  │  registers)   │  │ │ │  └────────────────┘  │
               │                     │ │ │  └───────────────┘  │ │ │                      │
               │  ┌───────────────┐  │ │ │                     │ │ │  ┌────────────────┐  │
               │  │ Static handler│  │ │ │  ┌───────────────┐  │ │ │  │ 50+ rt_*       │  │
               │  │ resolution    │  │ │ │  │ rt_perform /  │  │ │ │  │ helpers shared │  │
               │  │ OpPerformDirect│ │ │ │  │ rt_push_handler│ │ │ │  │ with JIT       │  │
               │  │ Indexed       │  │ │ │  │ (runtime      │  │ │ │  │                │  │
               │  └───────────────┘  │ │ │  │  helpers)     │  │ │ │  │ AOT: .o / .s   │  │
               └──────────┬──────────┘ │ │  └───────────────┘  │ │ │  └────────────────┘  │
                          │            │ └─────────┬──────────┘ │ └──────────┬──────────┘
                          │            │           │            │            │
               ┌──────────▼──────────┐ │ ┌────────▼────────────▼────────────▼─┐
               │   VM Execution      │ │ │       Native Execution              │
               │  (bytecode/vm/)     │ │ │  (Cranelift or LLVM output)         │
               │                     │ │ │                                     │
               │  Stack-based VM     │ │ │  Direct machine code execution      │
               │  dispatch loop      │ │ │  JitContext (shared by both)        │
               │                     │ │ │                                     │
               │  handler_stack      │ │ │  handler_stack (via rt_* helpers)   │
               │  (effect runtime)   │ │ │                                     │
               └──────────┬──────────┘ │ └──────────────────┬──────────────────┘
                          │            │                    │
                          └────────────┴──────┬─────────────┘
                                              │
                          ┌───────────────────▼───────────────┐
                          │         Shared Runtime             │
                          │                                    │
                          │  Value enum + Rc-based ownership   │
                          │  No mark/sweep GC                  │
                          │  Base functions                    │
                          │  Closures / Continuations          │
                          │  HAMT persistent maps              │
                          │  JitContext + rt_* helpers         │
                          └────────────────────────────────────┘
```

---

## Architectural Layers

| Layer | Module | ~Lines | Role |
|-------|--------|--------|------|
| **Frontend** | `syntax/` | 5K | Lexing, parsing, string interning |
| **AST Transforms** | `ast/` | 3K | Desugar, constant fold, free vars, tail calls |
| **Type Inference** | `ast/type_infer/` | 4K | HM Algorithm W + effect rows |
| **Core IR** | `core/` | 5K | Semantic IR + standard Core optimization passes |
| **Aether** | `aether/` | 7K+ | Ownership, borrow, reuse, verification, FBIP on Core |
| **Backend IR** | `cfg/` | 3K | CFG representation + 7 passes + `lower_program_to_ir()` |
| **VM Backend** | `bytecode/` | 15K | `compiler/` (7-phase pipeline, CFG → opcodes) + `vm/` (stack-based executor) |
| **JIT Backend** | `jit/` | 6K | CFG → Cranelift → machine code |
| **LLVM Backend** | `llvm/` | 3K | CFG → LLVM IR → machine code / object files |
| **Runtime** | `runtime/` | 8K | Rc-based values, base functions, closures, helpers — shared by all 3 backends |
| **Diagnostics** | `diagnostics/` | 3K | Elm-style error rendering |

---

## Intermediate Representations

Flux uses four main compiler representations in sequence, with Aether as a
Core-stage transformation layer between Core passes and CFG lowering:

### 1. AST (`syntax/expression.rs`)
- Tree-shaped, close to source syntax
- ~25 expression variants, ~10 statement variants
- Identifiers are interned symbols (`u32`)
- Spans preserved on every node for diagnostics

### 2. Core IR (`core/mod.rs`)
- semantic IR with source sugar eliminated
- standard computation nodes plus explicit Aether nodes after the Aether stage
- Binders use `CoreBinder` (stable ID + name)
- `CoreType` carries HM-inferred types on definitions
- Pattern matching preserved as `Case` with `CoreAlt` alternatives

Important Aether-related Core nodes:

- `AetherCall`
- `Dup`
- `Drop`
- `Reuse`
- `DropSpecialized`

These remain explicit in Core until CFG lowering.

### Aether stage (`src/aether/`)

Aether is not a separate production IR. It is a Core-stage ownership and reuse
pipeline that runs after the standard Core passes and before CFG lowering.

Main responsibilities:

- infer borrow signatures
- insert `AetherCall`, `Dup`, and `Drop`
- recognize `Reuse`
- recognize `DropSpecialized`
- verify the transformed Core
- run semantic `@fip` / `@fbip` checks

Primary modules:

- `src/aether/borrow_infer.rs`
- `src/aether/insert.rs`
- `src/aether/drop_spec.rs`
- `src/aether/reuse.rs`
- `src/aether/reuse_spec.rs`
- `src/aether/verify.rs`
- `src/aether/check_fbip.rs`

See also:

- `docs/internals/aether.md`

### 3. Backend IR / CFG (`cfg/mod.rs`)
- Function-oriented: `IrFunction` with `IrBlock` basic blocks
- Each block: sequential `IrInstr` + `IrTerminator` (Jump, Branch, Return, TailCall)
- `IrVar` for SSA-like temporaries
- `IrType` for type-directed optimizations (Int, Float, Bool, etc.)
- `HandleScope` instruction for effect handler boundaries
- `IrFunction` carries both source annotations and HM-inferred types

### 4. Bytecode (`bytecode/op_code.rs`)
- Stack-based instruction set (~85 opcodes)
- Compact bytecode cached as `.fxc` files
- Effect opcodes: `OpHandle`, `OpHandleDirect`, `OpPerform`, `OpPerformDirect`, `OpPerformDirectIndexed`

---

## Core IR Passes

The Core IR optimization pipeline (`core/passes/`) runs 7 passes in order:

| # | Pass | File | What it does |
|---|------|------|-------------|
| 1 | `beta_reduce` | `beta.rs` | Eliminate `App(Lam(x, body), arg)` → `body[x := arg]` |
| 2 | `case_of_case` | `case_of_case.rs` | Push outer case into inner case arms |
| 3 | `case_of_known_constructor` | `cokc.rs` | Reduce `Case(Con/Lit, alts)` statically |
| 4 | `inline_lets` | `inliner.rs` | Dead elimination + single-use + small-RHS inlining |
| 5 | `elim_dead_let` | `dead_let.rs` | Remove unused pure bindings |
| 6 | `evidence_pass` | `evidence.rs` | Rewrite TR Handle/Perform into evidence-passing calls |
| 7 | `anf_normalize` | `anf.rs` | Flatten nested subexpressions into let-chains |

Shared infrastructure in `helpers.rs`: substitution, tree walking, free-variable analysis, expression size counting.

After those seven passes, `core/passes/mod.rs` runs Aether and only then lowers
the resulting Core to CFG.

---

## CFG Passes

The backend IR optimization pipeline (`cfg/passes.rs`) runs 7 passes:

| # | Pass | What it does |
|---|------|-------------|
| 1 | `dead_block_elimination` | Remove unreachable blocks |
| 2 | `canonicalize_cfg` | Convert trailing Unreachable → Return |
| 3 | `constant_fold` | Fold constant expressions and branches |
| 4 | `tail_call_introduction` | Convert tail-position Call → TailCall terminator |
| 5 | `local_cse` | Common subexpression elimination (per-block) |
| 6 | `intern_unit_adts` | Optimize zero-field ADT constructors |
| 7 | `type_directed_unboxing` | Specialize binary ops based on IrType |

---

## Execution Backends

All three backends consume the same `IrProgram` (from `cfg/`) and share the same
`runtime/` (Rc-based values, base functions, closures, `rt_*` helpers). Parity
between backends is enforced by `scripts/release/check_parity.sh`.

### VM Backend (`bytecode/`)

The default backend. Compiles CFG IR to a stack-based bytecode instruction set (~85 opcodes), then executes it in an interpreter loop.

**Compiler** (`bytecode/compiler/`):
- 7-phase pipeline (proposal 0044): reset → collection → predeclaration → type_inference → ir_lowering → codegen → finalization
- Produces `Vec<u8>` instruction streams with `OpCode` variants
- Handles effect handler compilation: `OpHandle`, `OpHandleDirect`, `OpPerform`, `OpPerformDirectIndexed`
- Two-phase type-informed optimization (proposal 0077): infer → fold → reinfer for dead branch elimination

**VM** (`bytecode/vm/`):
- Stack-based dispatch loop (`dispatch.rs`) — fetches opcodes, manipulates value stack and call frames
- `handler_stack` for algebraic effect runtime — push/pop handler frames, continuation capture
- Function calls via `function_call.rs` — handles closures, base functions, tail calls
- Supports `--trace` flag for instruction-level tracing
- `test_runner.rs` implements `--test` flag (collects and runs `test_*` functions)

**Bytecode format**:
- Opcodes are `#[repr(u8)]` with u16 operands (3 bytes per instruction for jumps)
- Compiled bytecode cached as `.fxc` files (SHA-2 content hashing)
- Constants pool for literals, debug info for source mapping

### Cranelift JIT Backend (`jit/`)

Compiles CFG IR to native machine code via Cranelift, runs in-process. Enabled with `--jit` flag (requires `--features jit`).

**Compiler** (`jit/compiler.rs`, ~5.4K lines):
- Translates `IrFunction`/`IrBlock`/`IrInstr` → Cranelift IR → machine code
- Two-tier value system via `JitValueKind`: `Int`/`Float`/`Bool` stay as raw i64 in registers; `Boxed` uses `*mut Value` arena pointers — boxing deferred until values escape
- `JitCtx<'a>` bundles read-only compilation context (helpers, interner, return_block, tail_call flag)
- Effect handlers via `rt_perform`/`rt_push_handler` runtime helper calls

**Runtime bridge** (`jit/runtime_helpers.rs`):
- 50+ `rt_*` native callbacks shared with LLVM backend
- Handles value allocation/reuse helpers, effect perform/handle, closure creation, base function dispatch
- `value_arena.rs` provides pointer-stable allocation for JIT values

### LLVM Backend (`llvm/`)

Compiles CFG IR to LLVM IR for native AOT compilation or in-process execution. Enabled with `--llvm` flag (requires `--features llvm` and LLVM 18).

**Compiler** (`llvm/compiler/`):
- `{i64, i64}` tagged value representation (tag + payload) for all values
- Shares the same 50+ `rt_*` runtime helpers as the Cranelift JIT
- Can emit object files (`.o`/`.s`) for AOT compilation or execute in-process
- `LlvmCompilerContext` wraps LLVM module, builder, type cache, and helper declarations
- Safe wrapper (`wrapper.rs`) over ~30 LLVM C API functions

### Shared Runtime (`runtime/`)

Pure infrastructure consumed by all three backends — no backend-specific code:

| Component | What it provides |
|-----------|-----------------|
| `value.rs` | `Value` enum — the universal runtime representation |
| `base/` | 77 base functions (IO, array, string, math, higher-order, assertions) |
| `cons_cell.rs`, `hamt.rs`, `adt.rs` | Rc-based persistent data structures and sharing helpers |
| `closure.rs` | Closure struct (compiled function + captured values) |
| `continuation.rs` | Continuation struct (captured frames for effect resume) |
| `handler_frame.rs` | HandlerFrame (effect name + arms + boundary info) |
| `native_helpers.rs` | 50+ `rt_*` functions called by JIT and LLVM backends |
| `native_context.rs` | `JitContext` — shared execution context for JIT/LLVM |
| `nanbox.rs` | NaN-boxing representation for compact value encoding |

---

## Effect Handler Optimization Tiers

Flux has a 3-tier optimization for algebraic effect handlers:

### Tier 1: Tail-Resumptive Detection
- Analysis: `is_handler_tail_resumptive()` checks if all handler arms end with `resume(v)`
- Bytecode: `OpHandleDirect` marks handler frame as direct
- VM: skips continuation capture, uses identity closure for resume

### Tier 2: Static Handler Resolution
- Analysis: `resolve_handler_statically()` checks compile-time handler scopes
- Bytecode: `OpPerformDirectIndexed(depth, arm_idx, arity)` — no runtime search
- VM: direct index into handler_stack

### Tier 3: Evidence-Passing
- Core level: `evidence_pass` rewrites TR `Handle`/`Perform` → `Let`/`App` at Core IR level
- Bytecode level: arm closures stored in local variables, performs become `OpGetLocal` + `OpCall`
- Both VM and JIT benefit from Core-level rewrite

---

## Source Layout (`src/`)

```
src/
├── syntax/                  Front-end
│   ├── lexer/               Tokenization, one/two-byte dispatch tables
│   ├── parser/              Hybrid recursive descent + Pratt expression parser
│   │   ├── mod.rs           Entry point, token navigation (3-token lookahead)
│   │   ├── expression.rs    Expression parsing, array/hash/cons/comprehension literals
│   │   ├── statement.rs     fn / module / import / let declarations
│   │   ├── literal.rs       Number, string, interpolation parsing
│   │   └── helpers.rs       Error recovery, LIST_ERROR_LIMIT
│   ├── token_type.rs        Token definitions via define_tokens! macro
│   ├── interner.rs          String interning — all identifiers are Symbol (u32 index)
│   ├── linter.rs            Lint passes over AST
│   ├── formatter.rs         Source formatter
│   └── module_graph.rs      Import resolution, cycle detection, topological sort
│
├── ast/                     AST transforms and analysis
│   ├── type_infer/          HM type inference (Algorithm W) with effect rows
│   │   ├── mod.rs           infer_program() entry point, InferCtx
│   │   ├── expression/      Per-expression-variant inference (7 files)
│   │   ├── unification.rs   Contextual error reporting (ReportContext)
│   │   ├── effects.rs       Effect row checking
│   │   └── solver.rs        Constraint solving
│   ├── type_informed_fold.rs  Post-inference AST optimization (proposal 0077)
│   ├── fold/                Constant folding
│   ├── desugar/             Additional desugaring (after parse)
│   ├── free_vars/           Free variable collection for closure compilation
│   ├── tail_calls/          Tail call detection / annotation
│   └── visitor.rs           Visitor + Folder traits for AST traversal
│
├── types/                   Type system primitives
│   ├── infer_type.rs        InferType enum (Var, Con, App, Fun, Tuple)
│   ├── type_constructor.rs  TypeConstructor (13 variants: Int, Float, ..., Adt)
│   ├── unify.rs             Type unification
│   ├── type_env.rs          Type environment (Scheme → monotype)
│   └── scheme.rs            Polymorphic type schemes
│
├── core/                    Core IR — semantic intermediate representation
│   ├── mod.rs               CoreExpr, CoreType, CoreBinder, CoreDef, CoreProgram
│   ├── lower_ast/           AST → Core lowering
│   │   ├── mod.rs           AstLowerer struct, top-level/block lowering
│   │   ├── expression.rs    lower_expr() — 21 expression variants → ~12 Core variants
│   │   ├── pattern.rs       Pattern lowering + destructuring
│   │   └── binder_resolution.rs  Scope-based binder ID resolution
│   ├── passes/              7 Core optimization passes
│   │   ├── mod.rs           Pass pipeline (run_core_passes)
│   │   ├── beta.rs          Beta reduction
│   │   ├── case_of_case.rs  Case-of-case transformation
│   │   ├── cokc.rs          Case-of-known-constructor
│   │   ├── inliner.rs       Occurrence-based inlining
│   │   ├── dead_let.rs      Dead let elimination
│   │   ├── evidence.rs      Evidence-passing for TR effect handlers
│   │   ├── anf.rs           ANF normalization
│   │   ├── tail_resumptive.rs  Core-level TR handler detection
│   │   ├── helpers.rs       Shared: subst, map_children, appears_free, expr_size
│   │   ├── inline.rs        Legacy trivial-let inlining (superseded by inliner.rs)
│   │   └── tests.rs         Unit tests for all passes
│   ├── to_ir/               Core → Backend IR lowering
│   │   ├── mod.rs           ToIrCtx, lower_core_to_ir() entry point
│   │   ├── fn_ctx.rs        FnCtx — per-function IR building context
│   │   ├── case.rs          Case/pattern compilation to CFG branches
│   │   ├── closure.rs       Lambda/handler-arm → closure IR functions
│   │   ├── primop.rs        PrimOp → IR binary/unary operations
│   │   └── free_vars.rs     Free variable analysis for closure capture
│   └── display.rs           Core IR pretty-printer (--dump-core flag)
│
├── cfg/                     CFG-based backend IR (consumed by all 3 backends)
│   ├── mod.rs               IrFunction, IrBlock, IrInstr, IrTerminator, IrType, lower_program_to_ir()
│   ├── passes.rs            7 CFG optimization passes
│   ├── validate.rs          IR validation (locals, terminators, types)
│   ├── metadata.rs          Shared metadata collection for native backends (JIT, LLVM)
│   └── lower.rs             Legacy AST → CFG lowering (being replaced by core/to_ir/)
│
├── bytecode/                Bytecode compiler + format
│   ├── compiler/            CFG IR → stack-based bytecode
│   │   ├── mod.rs           Compiler struct, state management
│   │   ├── pipeline.rs      7-phase compile orchestrator (proposal 0044)
│   │   ├── passes/          Per-phase modules:
│   │   │   ├── reset.rs         Phase 0: clear per-file state
│   │   │   ├── collection.rs    Phase 1: collect definitions + validate structure
│   │   │   ├── predeclaration.rs Phase 2: forward-declare function names
│   │   │   ├── type_inference.rs Phase 3: HM inference (single/two-phase)
│   │   │   ├── ir_lowering.rs   Phase 4: AST → backend IR + IR passes
│   │   │   ├── codegen.rs       Phase 5: pattern validation + bytecode emission
│   │   │   └── finalization.rs  Phase 6: main entry + diagnostic merging
│   │   ├── expression.rs    Expression compilation (~4.6K lines)
│   │   ├── statement.rs     Statement compilation (~1K lines)
│   │   ├── cfg_bytecode.rs  Direct CFG → bytecode path
│   │   ├── tail_resumptive.rs  Bytecode-level TR handler detection
│   │   ├── effect_rows.rs   Effect row tracking
│   │   ├── contracts.rs     Runtime type contracts
│   │   ├── hm_expr_typer.rs HM type lookup helpers
│   │   └── ...              Supporting modules (builder, errors, suggestions)
│   ├── vm/                  Stack-based VM (moved from runtime/ for backend symmetry)
│   │   ├── mod.rs           VM struct, handler_stack, identity closure
│   │   ├── dispatch.rs      Main dispatch loop (~1.3K lines)
│   │   ├── function_call.rs Function call / return / resume mechanics
│   │   ├── test_runner.rs   --test flag: collect and run test_* functions
│   │   ├── trace.rs         Instruction tracing (--trace flag)
│   │   ├── binary_ops.rs    Binary arithmetic operations
│   │   ├── comparison_ops.rs Comparison operations
│   │   ├── index_ops.rs     Index operations
│   │   └── primop.rs        Primitive operation dispatch
│   ├── op_code.rs           ~85 opcodes (OpGetLocal, OpCall, OpPerformDirectIndexed, ...)
│   ├── symbol_table.rs      Variable/function/Base-function tracking per scope
│   └── bytecode_cache/      .fxc bytecode cache (SHA-2 content hashing)
│
├── runtime/                 Shared runtime (all 3 backends)
│   ├── value.rs             Value enum and Rc-based runtime representation
│   ├── compiled_function.rs CompiledFunction struct
│   ├── closure.rs           Closure struct (function + captured values)
│   ├── continuation.rs      Continuation struct (captured frames for effects)
│   ├── handler_frame.rs     HandlerFrame (effect + arms + boundary info)
│   ├── cons_cell.rs         Rc-based cons list representation
│   ├── hamt.rs              Rc-based HAMT persistent maps
│   ├── adt.rs               Rc-based ADT field helpers
│   ├── base/                75 base functions, registered via BASE_FUNCTIONS array
│   │   ├── mod.rs           Registration and dispatch
│   │   ├── helpers.rs       HM type signatures for all base functions
│   │   ├── array_ops.rs     Array operations (sort, slice, push, ...)
│   │   ├── string_ops.rs    String operations (split, trim, replace, ...)
│   │   ├── hash_ops.rs      Hash map operations (get, put, keys, ...)
│   │   ├── list_ops.rs      Cons list operations (cons, head, tail, ...)
│   │   ├── numeric_ops.rs   Math operations (abs, min, max, ...)
│   │   ├── higher_order_ops.rs  map, filter, fold, find, ...
│   │   ├── io_ops.rs        IO operations (print, read_file, ...)
│   │   ├── type_check.rs    Type checking (type_of, is_int, ...)
│   │   └── assert_ops.rs    Test assertions (assert_eq, assert_throws, ...)
│
├── jit/                     Cranelift JIT backend (--features jit)
│   ├── compiler.rs          CFG IR → Cranelift IR → machine code (~5.4K lines)
│   ├── context.rs           JIT execution context
│   ├── runtime_helpers.rs   Native callbacks: rt_perform, rt_push_handler, Aether/runtime helpers
│   └── value_arena.rs       Pointer-stable allocation for JIT values
│
├── llvm/                    LLVM backend (--features llvm, requires LLVM 18)
│   ├── mod.rs               Public API: llvm_compile, llvm_execute, llvm_emit_object
│   ├── context.rs           LlvmCompilerContext (LLVM module, builder, types, helpers)
│   ├── wrapper.rs           Safe wrapper over ~30 LLVM C API functions
│   └── compiler/            Compilation pipeline
│       ├── mod.rs           Orchestration (compile_program, compile_program_ir_only)
│       ├── symbols.rs       ADT/module collection, 50+ rt_* helper declarations
│       ├── function.rs      compile_function + compile_block
│       ├── expressions.rs   compile_expr (~30 IrExpr variants)
│       ├── binary_ops.rs    Arithmetic/comparison operator compilation
│       ├── calls.rs         Function call compilation (direct/named/var/primop)
│       ├── entry.rs         __flux_entry wrapper + __flux_identity for effects
│       └── helpers.rs       Tagged value builders, null checks, boxing utilities
│
├── primop/                  41 primitive operations with frozen discriminants
│
└── diagnostics/             Structured error reporting
    ├── diagnostic.rs        Core Diagnostic struct
    ├── builders/            DiagnosticBuilder trait — 24 with_* methods
    ├── types/               ErrorCode, Severity, Hint, Label, Suggestion, Related
    ├── rendering/           ANSI rendering, source snippets, formatter
    ├── compiler_errors.rs   Compile-time error constructors (67 error codes)
    ├── runtime_errors.rs    Runtime error constructors
    ├── aggregator.rs        Stage-aware filtering, dedup, grouping, sorting
    └── registry.rs          Error code registry
```

---

## Key Design Decisions

### Interned Identifiers

All identifiers go through `syntax::interner`. The `Identifier` type is `symbol::Symbol` (a `u32` index), not `String`. This makes identifier comparison O(1) and eliminates string allocation in the AST.

### Rc-Based Values (No-Cycle Invariant)

Runtime values use `Rc` for sharing. Values must form DAGs — no cycles allowed
(would leak via `Rc`). The language enforces this through immutability.
Persistent lists, ADTs, options/eithers, and HAMT maps are all represented
through Rc-based runtime structures; there is no production mark-and-sweep heap.

### Value Enum

```rust
enum Value {
    // Primitives
    Integer(i64), Float(f64), Boolean(bool), String(Rc<str>), None, EmptyList,
    // Wrappers
    Some(Rc<Value>), Left(Rc<Value>), Right(Rc<Value>),
    // ADTs
    Adt(Rc<str>, AdtFields), AdtUnit(Rc<str>),
    // Collections
    Array(Rc<Vec<Value>>),
    Cons(Rc<ConsCell>),
    HashMap(Rc<HamtNode>),
    // Functions
    Function(Rc<CompiledFunction>), Closure(Rc<Closure>), BaseFunction(u8),
    JitClosure(Rc<JitClosure>),
    // Effects
    Continuation(Rc<RefCell<Continuation>>),
    PerformDescriptor(Rc<PerformDescriptor>), HandlerDescriptor(Rc<HandlerDescriptor>),
    // Internal
    ReturnValue(Rc<Value>), Uninit,
}
```

### JIT/LLVM Tagged Value System

The Cranelift JIT uses `JitValueKind` to avoid unnecessary boxing:
- `JitValueKind::Int` / `Float` / `Bool` — raw machine values in registers
- `JitValueKind::Boxed` — `*mut Value` arena pointers
- Boxing is deferred until values escape (stored in ADT, returned, etc.)

The LLVM backend uses `{i64, i64}` structs (tag + payload) for all values.
Both backends share the same `JitContext`, `JitFunctionEntry`, and 50+ `rt_*`
runtime helpers (`runtime/native_helpers.rs`), making them interchangeable
at the runtime level. Parity is enforced by `scripts/release/check_parity.sh`.

### Base Function Registration

Base functions must be registered in three places with matching indices:
1. **Implementation** in `runtime/base/<module>.rs`
2. **`BASE_FUNCTIONS` array** in `runtime/base/mod.rs`
3. **Symbol table** in `bytecode/compiler/mod.rs`

### Diagnostics

Elm-style error messages with:
- Error codes (`E001`–`E1xxx`)
- Source snippets with colored labels
- Contextual hints and suggestions
- Stage-aware filtering (Parse → Type → Effect cascade)
- `--all-errors` flag to disable filtering

### Bytecode Cache

Compiled bytecode cached as `.fxc` files under `target/flux/`. Cache keys are SHA-2 hashes of source content + dependency graph. `--no-cache` flag disables.

### Aether and Runtime Ownership

Flux no longer uses a production mark-and-sweep heap. Instead:

- runtime sharing is Rc-based
- Aether inserts explicit ownership/reuse nodes in Core
- Core-to-CFG lowering turns those into backend-neutral operations
- VM/JIT/LLVM realize the same Aether semantics through opcodes or `rt_*`
  helpers

See:

- `docs/internals/aether.md`
- `docs/proposals/implemented/0084_aether_memory_model.md`

---

## Parser Structure

Hybrid approach:
- **Recursive descent** for declarations, statements, blocks
- **Pratt / TDOP precedence climbing** for expressions

Three-token lookahead. Error recovery via `sync_to_*` functions. Tokens defined via `define_tokens!` macro.

---

## CLI Flags

| Flag | Description |
|------|-------------|
| `--jit` | Use Cranelift JIT backend instead of VM |
| `--llvm` | Use LLVM backend instead of VM (requires `--features llvm`) |
| `--test` | Run `test_*` functions in the file |
| `--trace` | Print VM instruction trace |
| `--strict` | Enforce type annotations on public functions |
| `--no-cache` | Bypass .fxc bytecode cache |
| `--stats` | Print execution timing statistics |
| `--all-errors` | Show diagnostics from all phases (disable stage filtering) |
| `--dump-core` | Print Core IR (readable mode) and exit |
| `--dump-core=debug` | Print Core IR with binder IDs and types |
| `--dump-aether` | Print the Aether memory-model report and exit |
| `--trace-aether` | Print Aether report plus backend path to `stderr`, then run |
| `-O` | Enable AST-level optimizations |
| `-A` | Enable analysis passes |
| `bytecode <file>` | Show compiled bytecode disassembly |
