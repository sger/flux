# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What is Flux?

Flux is a pure functional programming language written in Rust with two execution backends: a stack-based bytecode VM and an LLVM native backend (llvm). Features include Hindley-Milner type inference, algebraic effects with row-polymorphic effect types, ADTs, pattern matching, persistent collections (Rc-based cons lists, HAMT maps), a module system, and Perceus-inspired compile-time reference counting (Aether memory model).

## Build Commands

```bash
cargo build                       # Dev build (opt-level=1)
cargo build --features native         # With LLVM text IR backend (llvm is a backward-compatible alias; llvm depends on native)
cargo build --all-features           # All backends
cargo build --profile dev-fast    # opt-level=3, lighter debug info
cargo build --release             # Release build
```

## Test Commands

```bash
cargo test --all --all-features           # Full suite
cargo test --test parser_tests            # Single test file
cargo test test_base_len                  # Single test by name
cargo insta review                        # Review snapshot diffs
cargo insta test --accept                 # Accept all new snapshots
```

Snapshot tests use `insta` with YAML format in `tests/snapshots/`.

## Lint & Format

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
```

CI treats all clippy warnings as errors. Rust toolchain: **1.93.0** (pinned in CI).

## Running Flux Programs

```bash
cargo run -- examples/basics/print.flx
cargo run -- examples/basics/fibonacci.flx --native  # LLVM backend (needs --features llvm + LLVM tools)
cargo run -- examples/basics/fibonacci.flx --native --emit-binary  # Native binary
cargo run -- --test examples/tests/math_test.flx      # Run test_* functions
cargo run -- bytecode examples/basics/print.flx       # Show compiled bytecode
cargo run -- --dump-core examples/basics/arithmetic.flx  # Inspect Core IR
cargo run -- --dump-lir examples/basics/arithmetic.flx   # Inspect LIR
cargo run -- parity-check examples                     # Run the maintained example corpus
```

### Parity Checks (VM vs LLVM)

```bash
cargo run -- parity-check tests/parity
cargo run -- parity-check examples/basics
cargo run -- parity-check tests/parity --ways vm,llvm,vm_cached,vm_strict,llvm_strict
```

Key flags: `--trace` (VM instruction trace), `--strict` (enforce type annotations on public fn), `--stats` (timing), `--no-cache` (bypass .fxc cache), `-O` (AST optimizations), `-A` (analysis passes), `--dump-core` / `--dump-core=debug` (Core IR inspection), `--dump-lir` / `--dump-lir-llvm` (LIR inspection), `--dump-aether` (Aether RC annotations), `--emit-llvm` (LLVM text IR), `--native` (LLVM backend).

## Architecture

### Compilation Pipeline

```
Source (.flx) → Lexer → Parser → AST Passes (desugar, constant fold, free vars, tail calls)
  → HM Type Inference → Flux Core (core/) → Core passes (+ primop promotion) → Aether (dup/drop/reuse)
    ├── cfg/ → Bytecode Compiler → VM execution
    └── LIR (lir/) → emit_llvm.rs → LLVM text IR → opt → llc → cc → Native binary (feature-gated: native)
```

Both backends share: HM inference output, Core IR pipeline, Aether RC pass, CorePrimOp enum, C runtime (`runtime/c/`), Flux standard library (`lib/Base/`), and NaN-box value representation. They diverge after Aether: the VM path goes through `cfg/` to bytecode, while the native path goes through `lir/` to LLVM IR.

### Unified Primop Architecture

Both backends use a **single primop enum**: `CorePrimOp` (`src/core/mod.rs`). The old bytecode-level `PrimOp` enum has been deleted. The VM dispatches `CorePrimOp` directly to C runtime functions via `core_dispatch.rs`.

```
VM:     OpPrimOp(CorePrimOp id, arity) → execute_core_primop() → C FFI call
Native: CorePrimOp → builtins.rs → emit call @flux_<name>(i64, ...)
```

Both backends call the same C functions in `runtime/c/`. No duplicated primop implementations.

### C Runtime (`runtime/c/`)

The C runtime is the single implementation for all primops. It uses Aether RC (Perceus-inspired reference counting, Koka-style FluxHeader).

| File | Purpose |
|------|---------|
| `rc.c` | Aether RC allocator: FluxHeader (refcount + scan_fsize + obj_tag), flux_dup/drop with recursive child scanning |
| `flux_rt.c` | I/O, arithmetic, comparisons, type inspection, format/toString |
| `string.c` | String operations (new, concat, slice, upper, lower, etc.) |
| `array.c` | Array operations (new, get, set, push, concat, slice) |
| `hamt.c` | HAMT persistent map (get, set, delete, keys, values) |
| `effects.c` | Effect handlers (push_handler, pop_handler, perform, resume) |
| `flux_rt.h` | All declarations, NaN-box constants, inline tag/untag helpers |

Memory layout (Koka-inspired):
```
  [FluxHeader 8B: refcount(i32) | scan_fsize(u8) | obj_tag(u8) | reserved(u16)]
  [payload]  ← returned pointer, header at ptr - 8
```

`flux_drop` uses `scan_fsize` to recursively drop child NaN-boxed fields before freeing. The LLVM backend calls `flux_dup`/`flux_drop` as external C functions (not inline LLVM IR).

### LIR (Low-Level IR)

The LIR (`src/lir/`) is the low-level IR for the **native backend only**. It sits between Core IR (functional, high-level) and LLVM IR emission.

- `src/lir/mod.rs` — LirInstr, LirTerminator, LirBlock, LirFunction, LirProgram
- `src/lir/lower.rs` — Core → LIR lowering
- `src/lir/emit_llvm.rs` — LIR → LLVM text IR emission (nearly 1:1 since LIR is already flat CFG with SSA variables)

### Flux Standard Library (`lib/Base/`)

The standard library is written in Flux and compiled through the same pipeline as user code.

| Module | Functions |
|--------|-----------|
| `lib/Base/List.flx` | `map`, `filter`, `fold`, `flat_map`, `flatten`, `any`, `all`, `find`, `count`, `each`, `sort_by`, `first`, `last`, `rest`, `range`, `reverse`, `zip`, `contains`, `sum`, `product`, `abs`, `min`, `max` |
| `lib/Base/Option.flx` | `unwrap`, `unwrap_or`, `map_option`, `flat_map_option`, `is_some`, `is_none_opt` |
| `lib/Base/String.flx` | `starts_with`, `ends_with`, `chars`, `join`, `str_contains` |
| `lib/Base/Numeric.flx` | `max_list`, `min_list` |
| `lib/Base/IO.flx` | `print_all`, `println_all`, `read_lines`, `parse_ints`, `split_ints` |
| `lib/Base/Assert.flx` | `assert_eq`, `assert_neq`, `assert_true`, `assert_false`, `assert_gt`, `assert_lt`, `assert_gte`, `assert_lte`, `assert_len`, `assert_msg` |

Auto-prelude (`inject_base_prelude` in main.rs) injects `import Base.* exposing (..)` for all modules. Skipped for `--dump-aether`/`--dump-core`/`--trace-aether`.

**Flux syntax constraints for lib/Base/**: match arms require commas between them; `let` cannot appear inside match arms (extract to helper function); each `fn` name must be unique within a module (no reuse of `go` across functions — use `map_go`, `filter_go`, etc.); Base modules cannot reference each other's functions (no cross-module imports within lib/Base/).

### Type Classes

Flux implements Haskell-style type classes with GHC-style dictionary passing. See `docs/proposals/0145_type_classes.md` for the full design.

**Key files:**

| File | Purpose |
|------|---------|
| `src/types/class_env.rs` | `ClassEnv`, `ClassDef`, `InstanceDef` — class/instance registry |
| `src/types/class_dispatch.rs` | Phase 1b: generates mangled instance functions (`__tc_{Class}_{Type}_{method}`), polymorphic stubs, pre-interns `__dict_*` names |
| `src/types/class_solver.rs` | Constraint solving: checks instances exist for emitted constraints |
| `src/core/passes/dict_elaborate.rs` | Core-to-Core pass: builds `__dict_{Class}_{Type}` CoreDefs, rewrites constrained function bodies to extract methods from dict params |
| `src/core/lower_ast/mod.rs` | `try_resolve_class_call()` — monomorphic dispatch; `resolve_dict_args_for_call()` — concrete dict resolution at call sites |
| `src/ast/type_infer/constraint.rs` | `SchemeConstraint`, `WantedClassConstraint` — constraint types |

**Pipeline flow for type classes:**
1. **Phase 1b** (`class_dispatch.rs`): Generate `__tc_*` mangled instance functions + polymorphic dispatch stubs from AST class/instance declarations. Pre-intern `__dict_*` names.
2. **Type inference**: Emit `WantedClassConstraint` for class method calls. Promote constraints to `Scheme.constraints` during generalization.
3. **AST→Core lowering**: `try_resolve_class_call()` resolves monomorphic calls to mangled names. `resolve_dict_args_for_call()` inserts concrete `__dict_*` args at call sites.
4. **Dict elaboration** (Core-to-Core pass): Builds `__dict_*` CoreDefs, prepends dict params to constrained functions, rewrites method calls to `TupleField(dict, index)`.

**Naming conventions:**
- Instance methods: `__tc_{Class}_{Type}_{method}` (e.g., `__tc_Eq_Int_eq`)
- Dictionary values: `__dict_{Class}_{Type}` (e.g., `__dict_Eq_Int`)

**Method effect floors (`with` clauses):** Class and instance methods may declare an effect floor via a `with` clause (e.g., `fn log(x: a) with IO`). Parsed into `ClassMethod.effects` / `InstanceMethod.effects`, propagated through `MethodSig.effects` and the Phase 1b mangling pass. Instance methods must satisfy their class's declared effect floor — violations emit **E452**, validated by a compiler walker over instance declarations.

### Bytecode Compiler Pipeline

The bytecode compiler (`src/compiler/pipeline.rs`) orchestrates these phases:

| Phase | Name | What it does |
|-------|------|-------------|
| 0 | Reset | Clear per-file compiler state |
| 1 | Collection | Collect definitions, validate structure |
| 1b | Class dispatch | Generate `__tc_*` instance functions + polymorphic stubs |
| 2 | Predeclaration | Forward-declare function names in symbol table |
| 3 | Type inference | HM inference, constraint solving, scheme generalization |
| 4 | IR lowering | AST → Core IR → Core passes (dict_elaborate, primop_promote, Aether) → CFG IR |
| 5 | Codegen | Compile statements to bytecode (CFG primary path, AST fallback) |
| 6 | Finalization | Error reporting, diagnostic suppression |

**Dual-mode compilation (Phase 5):** For each function, the bytecode compiler first tries the CFG path (`try_compile_ir_cfg_function_body`), which compiles from Core-derived CFG IR. If CFG compilation fails (unresolved names, unsupported constructs), it rolls back and falls through to the AST path. The CFG path is preferred because it benefits from Core passes (dict elaboration, Aether RC, etc.). The AST fallback compiles directly from the syntax tree.

### Module Map

| Module | Purpose |
|--------|---------|
| `syntax/` | Lexer, parser (recursive descent), interner, module graph, linter, formatter |
| `ast/` | AST transforms: desugar, constant fold, free vars, tail calls |
| `ast/type_infer/` | Hindley-Milner inference (Algorithm W) with effect rows |
| `types/` | Type system primitives: InferType, Scheme, TypeEnv, unification, ClassEnv, class dispatch, constraint solving |
| `core/` | Flux Core IR: AST→Core lowering, Core passes (beta, cokc, case_of_case, inline, dead_let, evidence, anf, primop_promote, dict_elaborate), CorePrimOp enum |
| `lir/` | Native backend Low-Level IR: Core→LIR lowering, LIR→LLVM IR emission |
| `cfg/` | VM backend IR: Core→CFG lowering, blocks, terminators, passes, validation → bytecode |
| `compiler/` | CFG IR → bytecode compilation with type/effect validation; `pipeline.rs` orchestrates 7 phases |
| `bytecode/` | OpCode enum, bytecode format, .fxc cache |
| `vm/` | Stack-based VM: dispatch loop, binary/comparison ops, function calls, core_dispatch (CorePrimOp → C FFI) |
| `runtime/` | Value enum, closures, frames, continuations, cons cells, HAMT maps, NaN-box, leak detector |
| `diagnostics/` | Elm-style errors with stable codes, source snippets, builder pattern |
| `aether/` | Perceus-style compile-time RC optimization: dup/drop insertion, borrow inference, reuse tokens, FBIP analysis |
| `llvm/` | Core IR → LLVM IR lowering: NaN-box layout, closures, ADTs, arithmetic prelude (feature-gated `llvm`) |
| `shared_ir/` | Shared ID types (AdtId, BlockId, FunctionId, IrVar, etc.) used across backends |
| `cli/` | CLI argument parsing and command dispatch |
| `driver/` | Compilation driver orchestrating the full pipeline |
| `parity/` | VM vs native parity-check infrastructure |
| `shared/` | Shared utilities across compiler stages |

### Key Files

- **Entry point**: `src/main.rs` (CLI dispatcher, manual arg parsing, auto-prelude injection)
- **Library root**: `src/lib.rs` (16 public modules; `llvm` is feature-gated behind `llvm`)
- **Value type**: `src/runtime/value.rs` — the core `Value` enum (Int, Float, String, Array, Closure, Cons, HashMap, Adt, etc.)
- **VM dispatch**: `src/vm/dispatch.rs` — main instruction loop
- **VM primop dispatch**: `src/vm/core_dispatch.rs` — CorePrimOp → C FFI calls
- **Type inference**: `src/ast/type_infer/mod.rs` — `infer_program()` entry point
- **Flux Core IR types**: `src/core/mod.rs` — CoreExpr, CoreProgram, CorePrimOp (unified primop enum for both backends)
- **LIR types**: `src/lir/mod.rs` — LirInstr, LirTerminator, LirBlock, LirFunction
- **LIR lowering**: `src/lir/lower.rs` — Core → LIR
- **LIR LLVM emission**: `src/lir/emit_llvm.rs` — LIR → LLVM text IR (native backend only)
- **Primop promotion**: `src/core/passes/primop_promote.rs` — rewrites `App(Var("println"), args)` → `PrimOp(Println, args)` for known builtins
- **Dict elaboration**: `src/core/passes/dict_elaborate.rs` — GHC-style dictionary passing for type classes (Core-to-Core pass)
- **CFG/Backend IR**: `src/cfg/mod.rs` — IrProgram, IrFunction, IrBlock, IrCallTarget
- **LLVM backend**: `src/llvm/` — Core IR → LLVM text IR (NaN-boxed, GHC-style pipeline)
- **LLVM builtins**: `src/llvm/codegen/builtins.rs` — maps CorePrimOp to C runtime function names
- **Aether RC pass**: `src/aether/insert.rs` — dup/drop insertion entry point
- **Error codes**: `src/diagnostics/compiler_errors.rs` (~108 codes), `src/diagnostics/runtime_errors.rs`
- **C runtime**: `runtime/c/flux_rt.c` — NaN-box ops, BigInt support, I/O, string/array/HAMT memory ops
- **C runtime RC**: `runtime/c/rc.c` — Aether RC allocator with Koka-inspired FluxHeader
- **Auto-prelude**: `src/main.rs` `inject_base_prelude()` — injects `import Base.* exposing (..)` for all programs

### Architecture Invariants

- `src/core/` is the only semantic IR. Do not add a second one.
- `src/cfg/` is the backend IR — backends consume `IrProgram` from here.
- `src/shared_ir/` is shared ID/plumbing only, not a compiler stage.
- `structured_ir` is retired and must not be reintroduced into production paths.
- Prefer fixing semantics in AST/Core lowering or Core passes over patching around them in backend code.
- For parity regressions, prefer adding or shrinking a fixture under `tests/parity/` rather than normalizing around a wrong fixture.

### Debugging Compiler Issues

When investigating compiler behavior:

1. Inspect the source fixture.
2. Inspect Core with `--dump-core` or `--dump-core=debug`.
3. If ownership/reuse matters, inspect `--dump-aether`.
4. Inspect backend IR only after Core and Aether look correct:
   - VM path: CFG / bytecode / `--trace`
   - Native path: `--dump-lir`, `--dump-lir-llvm`, `--emit-llvm`
5. If Core already shows wrong behavior, the bug is upstream of CFG / backend.
6. If Core matches but Aether differs, the bug is in `src/aether/` or the handoff from Core passes into Aether.
7. If Core and Aether match across backends, the bug is downstream (`compiler/`+`bytecode/`+`vm/` or `lir/`+`llvm/`).
8. Use `--dump-core=debug` when binder identity, synthetic temporaries, or unresolved/external names matter.

## Coding Patterns

### Memory Model (Aether)

Two memory spaces coexist:

**VM (Rust heap):** Runtime values use `Rc` for heap types. `Value::Cons(Rc<ConsCell>)`, `Value::HashMap(Rc<HamtNode>)`, `Value::Adt(Rc<AdtValue>)`. Values must form DAGs — no cycles allowed (would leak via Rc).

**C runtime (C heap):** Objects allocated via `flux_gc_alloc_header()` with FluxHeader at `ptr - 8`. Reference counted by `flux_dup`/`flux_drop`. Both the LLVM native backend and C runtime functions use this layout.

Key patterns:
- `Rc::try_unwrap` for zero-clone moves when values are uniquely owned
- Iterative `Drop` on `ConsCell` to prevent stack overflow on deep lists
- `flux_drop` recursively scans `scan_fsize` child fields (Koka-inspired)

### NaN-boxing

Both VM and native backends use NaN-boxed 64-bit values. Inline integers fit in 46 bits (±35 trillion); larger values are heap-boxed as BigInt (`FLUX_OBJ_BIGINT`). The C runtime's `flux_tag_int`/`flux_untag_int` handle overflow automatically. When working with type inspection functions (`is_int`, `is_float`, etc.), remember that BigInt values have `FLUX_TAG_BOXED_VALUE` tag, not `FLUX_TAG_INTEGER`.

### Parameter Grouping

`#![deny(clippy::too_many_arguments)]` is enforced. Functions with >6 params must use spec structs:
- `FnSpec<'a>` — type_infer (8 params → 1)
- `CompileFnSpec<'a>` — `src/compiler/statement.rs`

### DiagnosticBuilder Trait

The `DiagnosticBuilder` trait must be in scope to use `with_*` builder methods:
```rust
use crate::diagnostics::{Diagnostic, DiagnosticBuilder};
let diag = Diagnostic::error("title").with_span(span).with_message("msg");
```

### Error Code Pattern

Define errors in `compiler_errors.rs` or `runtime_errors.rs`, register in `registry.rs`, use `diag_enhanced(ERROR_CODE)` to create structured diagnostics.

### Import Syntax

Flux supports `exposing` for unqualified imports:
```flux
import Base.List exposing (..)           // all public members
import Base.Option exposing (unwrap, is_some)  // selective
import MyModule as M                     // qualified alias
```

### ADT Optimizations

- `Value::AdtUnit(Rc<str>)` — zero-field constructors (no heap alloc beyond shared name)
- `AdtFields` enum — inline storage for 1-3 fields, Vec fallback for larger
- `Rc::try_unwrap` in `OpAdtField` — moves fields without cloning when single-owned

## Benchmarks

```bash
cargo bench                              # All criterion benchmarks
cargo bench --bench lexer_bench          # Specific benchmark
scripts/bench_benchmark_flamewatch.sh binarytrees  # Cross-language benchmark + flamegraph
```

## Release Process

```bash
scripts/release/release_check.sh                 # Full local preflight (same as CI)
cp changes/_template.md changes/$(date +%Y-%m-%d)-topic.md  # Add changelog fragment
scripts/release/release_cut.sh v0.0.4            # Cut release (includes changelog rebuild)
```
