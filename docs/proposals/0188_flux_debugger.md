- Feature Name: `flux_debugger`
- Start Date: 2026-10-13
- Proposal PR:
- Flux Issue:
- Status: Draft
- Depends on: [0038](0038_deterministic_effect_replay.md) (replay semantics and CLI; implemented by this proposal's S1), [0162](implemented/0162_unified_effect_handler_runtime.md) (yield-based handler runtime), [0174](0174_async_effect_concurrency.md) (fibers, tasks, channels), [0163](0163_flux_language_server.md) (crate and editor-extension pattern)
- Relates to: [0129](0129_vm_improvements_ghc_inspired.md) (absorbs Phase 3), [0024](0024_runtime_instrumentation_and_value_tracer.md) (first consumer of Part A), [0076](0076_debug_toolkit.md) (print and dump tooling), [0122](implemented/0122_explicit_stack_and_tco.md) (its native-stack description no longer holds), [0175](0175_interactive_repl.md) / [0176](0176_interactive_repl_persistent_engine.md) (REPL `:break` / `:step`)

# Proposal 0188: Flux Debugger — DAP, DWARF and time travel

## Summary
[summary]: #summary

Flux gets one debugger with two engines behind one Debug Adapter Protocol
(DAP) server, `crates/flux-dap`:

- **The VM engine** runs inside the bytecode VM. It provides breakpoints,
  stepping, locals, stacks that follow captured continuations, fibers shown as
  threads, and **reverse execution**.
- **The native engine** uses DWARF emitted by the LLVM backend. lldb drives it:
  in VS Code through `lldb-dap`, in Xcode directly. Flux lldb formatters print
  values as Flux values rather than tagged `i64`s. On Windows, the same
  metadata is emitted as CodeView, so Visual Studio shows Flux call stacks
  (call stacks only; S11).

Both engines share one **backend-neutral effect tape** (`flux run --record` /
`--replay`). A run recorded on either backend can be replayed on either
backend. A native recording can also be opened in the VM engine and stepped
backwards.

Time travel is part of the design from the first stage, not added at the end.
Flux code is pure apart from its declared effects, so recording the results at
the effect boundary is enough to make a run reproducible. In most languages that
takes a whole-process recorder such as `rr`.

## Motivation
[motivation]: #motivation

### What exists today

**Native backend: no debug information at all.**
- The LLVM IR model has no metadata: `LlvmModule` (`src/llvm/ir/syntax.rs:13`)
  has no named metadata, no module flags, and no `!dbg` slot on functions,
  instructions or terminators. `source_filename` is hardcoded to `"flux_lir"`
  (`src/lir/emit_llvm.rs:170`).
- LIR carries a source span only on `IDiv` and `IRem` (`src/lir/mod.rs:145-157`),
  used for runtime error messages. Terminators and `LirFunction` (`:420`) have
  none.
- Core has a span on every `CoreExpr` (`src/core/mod.rs:2040-2127`). But `Span`
  has no file id, and `CoreBinder` and `CorePat` have no span.
- The runtime has a shadow-stack API, `flux_trace_push` (`runtime/c/flux_rt.c:2228`),
  that the compiler never calls. `flux_panic` prints it (`:2295`), so native
  panics print an empty trace.
- On macOS no `dsymutil` step runs after `run_linker` (`src/llvm/pipeline.rs:262`).

The result is that lldb, in Xcode or VS Code, shows symbols like `flux_Main_go$w`
with no file, line or variables.

**VM: locations, but nothing interactive.**
- `FunctionDebugInfo` (`src/bytecode/debug_info.rs:35`) maps instruction offset
  to file and span, and runtime errors print a Flux stack trace
  (`src/vm/trace.rs:117-162`).
- It has no local-variable table (only `num_locals`), no breakpoints and no
  stepping. A native debugger attached to `flux` sees the Rust dispatch loop,
  not Flux frames.
- While a handler arm runs, the frames that called `perform` are not in
  `VM.frames`. They are inside the captured `Continuation`
  (`src/runtime/continuation.rs:20-48`), so even the current stack trace leaves
  them out.

**Related proposals: pieces, each incomplete.**
- [0129](0129_vm_improvements_ghc_inspired.md) Phase 3 sketches `OpBreakpoint`.
- [0024](0024_runtime_instrumentation_and_value_tracer.md) sketches VM observer
  hooks. Part A is not implemented: there is no observer trait in `src/`.
- [0038](0038_deterministic_effect_replay.md) sketches record and replay. Its
  event list predates Env, Process, `Fs*` and async.
- [0163](0163_flux_language_server.md) mentions a `flux-dap` crate as future work.

None of them describes the whole design, and they disagree on details.

### Ownership

0188 is the umbrella. It takes over or depends on the earlier proposals as
follows:

| Proposal | What 0188 takes over | What stays with the original |
|---|---|---|
| 0129 Phase 3 | All of it. `OpBreakpoint(u16)` and the breakpoint table are kept, but whether a breakpoint is enabled moves from the bytecode into debugger state. `local_names` becomes a full local-variable table with live ranges and types (§3). | Phases 1, 2 and 4 |
| 0024 Part A | Defines the `VmObserver` trait and is its first user (S2). | Part B, the value-graph tracer, is reused for the reference-count view (S10). |
| 0038 | Its implementation is 0188 S1. 0188 extends the event list (§4.1) and adds a native backend, a step counter and checkpoints. | The user-facing CLI and the mismatch diagnostics |

### Why now

- The roadmap's 0.6.0 exit criterion is "users can debug Flux programs ...
  without immediately dropping into Rust internals". 0076 covers printing and
  dumps. Nothing covers interactive debugging.
- Every recent native-backend bug was found with lldb on unnamed `flux_*$w`
  symbols and raw registers ([debugging_native_backend.md](../debugging_native_backend.md)).
  Line tables alone would have made the KI-013 investigation much shorter.
- Recording has to be decided early. Once the effect runtime and the async
  backend have been built without a recording boundary, adding one later
  touches every primop twice.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Running under the debugger

```text
flux run --debug app.flx              # VM, waits for a DAP client
flux build --native --debug app.flx   # native binary + app.dSYM (macOS)
flux run --record trace.flxtape app.flx
flux run --replay trace.flxtape app.flx --native
```

`--debug` selects the **debug profile**:
- VM: emit breakpoint sites, record local-variable tables, disable Core inlining.
- Native: emit DWARF, use the `Og` pipeline, emit frame pointers.

The debug profile has its own cache entries, so switching between profiles
never mixes objects.

### VS Code

The Flux extension contributes a `flux` debugger type and enables breakpoints
in `.flx` files:

```json
{
  "type": "flux",
  "request": "launch",
  "program": "${workspaceFolder}/src/main.flx",
  "backend": "vm",
  "record": true
}
```

- `"backend": "native"` builds with `--debug` and starts `lldb-dap` underneath
  `flux-dap`. You see the same Flux views, and VS Code configuration is
  identical.
- `"replay": "trace.flxtape"` replays a recording. With `"backend": "vm"`, the
  Step Back and Reverse Continue buttons are enabled.

### Xcode

1. Create an External Build System target that runs
   `flux build --native --debug`.
2. Set the produced binary as the scheme's executable.

Breakpoints in `.flx` files, the call stack and stepping work through Xcode's
built-in lldb. `flux_lldb.py` is embedded in the dSYM, so lldb loads it
automatically and values print as Flux values.

Xcode cannot host a DAP adapter, so **the VM backend cannot be debugged in
Xcode**. This proposal does not try to change that.

### Visual Studio (Windows): call stacks only

On Windows, `flux build --native --debug app.flx` also writes `app.pdb`. You
can open `app.exe` in Visual Studio, or attach to it while it runs. When it
stops at a crash, a panic, or a pause, the **Call Stack** window shows Flux
function names with `.flx` file and line, and double-clicking a frame opens
that source line.

That is all Visual Studio gets. Values show as raw integers, and there is no
Flux-aware stepping, VM debugging or recording. For those, use VS Code.

### A paused session

Paused inside a `Console` handler, the Call Stack view shows:

```text
▼ fiber 1 (main)                                 [paused]
    log_handler        app.flx:31:9
    ⟂ handling Console.print — performed at app.flx:12:5
    render_row         app.flx:12:5   (suspended, resumable)
    List.map           Flow/List.flx:88:13
    main               app.flx:20:3
    … 1,203 calls elided by tail calls (last: go at app.flx:7:5)
▶ fiber 2 (fetch_user)                         [blocked on Channel#3 recv]
```

- Dictionary arguments (`Show<List<Int>>`) and compiler wrappers are hidden.
  The adapter's "Show compiler internals" setting reveals them.
- The Variables view groups each frame's values into *Locals*, *Captured* and
  *Handler state*.
- During a replay, **Step Back** goes to the previous stopping point, and
  **Reverse Continue** goes to the previous breakpoint hit. Both work because
  replay is deterministic.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### §1 The shared debug-info model

The VM and native backends both compile from Core and then Aether:
- VM path: `cfg/mod.rs` lowers to the CFG IR, then to bytecode.
- Native path: Aether lowers to LIR (`src/lir/lower.rs:742`), then to LLVM IR.

So the debug-info model is **produced once, at the Core/Aether level**. Each
backend then adds only its own location tables.

The new module is `src/debuginfo/`:

```rust
pub struct ProgramDebugInfo {
    pub files: Vec<DebugFile>,          // FileId(u32) -> {abs path, content hash}
    pub functions: Vec<DebugFunction>,  // DebugFnId
    pub types: Vec<DebugType>,          // DebugTypeId
    pub effects: Vec<DebugEffect>,      // effect/op names and tags
}

pub struct DebugFunction {
    pub id: DebugFnId,
    pub binder: Option<CoreBinderId>,
    pub source_name: String,            // "Flow.List.map", "<lambda app.flx:12:5>"
    pub file: FileId,
    pub decl_span: Span,
    pub kind: FnKind,
    pub artificial: bool,               // step through; hidden unless "internals"
    pub scopes: Vec<DebugScope>,        // lexical tree: {span, parent, vars}
}

pub enum FnKind {
    User,
    Lambda { parent: DebugFnId },
    ContSplit { parent: DebugFnId, resume_span: Span },
    Worker { of: DebugFnId },
    DirectCapture { of: DebugFnId },
    ClosureWrapper { of: DebugFnId },
    DictDef,
    DictMethod { class: String, instance: String },
    RuntimeGlue,
}

pub struct DebugVar {
    pub name: String,
    pub binder: CoreBinderId,
    pub decl_span: Option<Span>,
    pub ty: DebugTypeId,
    pub kind: VarKind,                  // Param | Let | PatternBound | Captured | Evidence
    pub artificial: bool,               // evidence, compiler temporaries
}

pub enum DebugType {
    Int, Float, Bool, String, Unit,
    List(DebugTypeId), Array(DebugTypeId), Map(DebugTypeId, DebugTypeId),
    Tuple(Vec<DebugTypeId>), Option(DebugTypeId), Either(DebugTypeId, DebugTypeId),
    Adt { name: String, ctors: Vec<DebugCtor> }, // {name, tag, fields: [{name?, ty}]}
    Function,
    Dynamic,                            // polymorphic: render from the runtime tag
}
```

**Inputs that already exist:**
- Variable types: `CoreDef.binder_types` (`src/core/mod.rs:2149`).
- The artificial flag: `CoreDef.is_dict_def` and `is_dictionary_name`.
- ADT layouts: `DataVariant.field_names` (`src/syntax/data_variant.rs:21`)
  together with `LirProgram.constructor_tags` (`src/lir/mod.rs:458`).

**What S0 needs, and what it defers.** S0 adds no fields to Core:

- **File identity.** Each module is compiled by its own `Compiler`, whose
  `file_path` (`src/compiler/mod.rs:1286`) is set per module
  (`src/driver/run_program/modules.rs:180`,
  `src/driver/pipeline/native/parallel.rs:152`). The model is built there, so
  every function belongs to that module's file. No `CoreDef.file` field is
  needed while Core inlining is off in the debug profile. Cross-module
  provenance is S9.
- **Variable spans.** A variable's declaration span is the span of the
  expression that binds it: the `Let`, the `Lam`, or the case arm. Every
  `CoreExpr` already has one. Exact binder spans (`CoreBinder.span` or a side
  table built during lowering, like `binder_types`) can come later if the
  coarser spans prove inadequate.

Three items move to the stages that first need them:

- **Variable identity** → S2 (VM) and S6 (native). Each backend already keeps a
  binder→variable map while it lowers: `FnCtx.env` (`src/core/to_ir/fn_ctx.rs:30`)
  and `FnLower.env` (`src/lir/lower.rs:1036`). Both are discarded afterwards.
  Each stage keeps its map as a side table (`IrFunction.var_binders`,
  `LirFunction.var_binders`) rather than adding fields to `IrVar` or `LirVar`.
- **Missing spans** → S5. Several passes create nodes with `Span::default()`:
  `dict_elaborate.rs`, `cokc.rs`, `static_contract.rs` and `primop_promote.rs`.
  Each such node either inherits the enclosing span or is marked artificial,
  once LIR stamps source locations.
- **Core inlining off in the debug profile** → S2 and S5, so that `-g` changes
  no generated code in S0.

`--dump-debug-info` prints the model. It is the snapshot surface for S0 tests.

### §2 Native engine: DWARF

#### 2.1 Why DWARF works here (and what 0122 got wrong)

[0122](implemented/0122_explicit_stack_and_tco.md) describes an explicit,
growable stack in the `core_to_llvm` backend. **That backend has been removed**:
`src/core_to_llvm/` no longer exists.

The current native path is Aether → LIR → LLVM:
- Ordinary calls use `fastcc` on the system stack. The linker reserves 64 MB
  (`pipeline.rs:333`).
- Every function carries `uwtable` (`src/llvm/ir/ppr.rs:147-164`).

So standard unwind information already describes the stack, and lldb can walk
native Flux frames with no custom unwinder.

The cost is tail calls:
- `promote_tail_calls` (`src/lir/lower.rs:895`) produces plain `tail call fastcc`.
  Nothing in `src/lir` or `src/llvm` emits `musttail`.
- When LLVM honours the hint, the frame is gone.
- With `DISPFlagAllCallsDescribed` and call-site entries, lldb can rebuild some
  of those frames as "artificial" tail-call frames.

#### 2.2 Additions to the IR model

`src/llvm/ir/syntax.rs` and `ppr.rs` gain:
- `LlvmModule.metadata: Vec<(MetaId, MetaNode)>` and named metadata:
  `!llvm.dbg.cu`, plus `!llvm.module.flags` with `"Debug Info Version" 3` and
  `"Dwarf Version"`.
- `LlvmFunction.dbg: Option<MetaId>`, printed after the attributes.
- `dbg: Option<MetaId>` on every instruction and terminator, printed as
  `, !dbg !N` (block printing is at `ppr.rs:175`).
- `source_filename` set to the module's real path.

Variable records use the `llvm.dbg.value` intrinsic form. Current LLVM still
parses it and converts it to `#dbg_value` records. Every supported clang
version gets a pipeline test.

#### 2.3 Compile units and subprograms

- One `DICompileUnit` per emitted module, since native compiles and caches per
  module:
  - `producer: "flux <version>"`
  - `isOptimized` from the profile
  - `emissionKind: FullDebug` or `LineTablesOnly`
- One `DIFile` per `DebugFile`.

Every function symbol gets a `DISubprogram`, according to its kind:

| Symbol | Source (`emit_llvm.rs`) | `DISubprogram` |
|---|---|---|
| `flux_X$w` (worker) | `:230` | The real body. `name` is the Flux name, `linkageName` the symbol. |
| `flux_X`, closure wrappers | `:208`, `:279`, `:548` | `DIFlagArtificial`, line-0 locations, so stepping passes through. |
| `flux_X$dc` | `:254` | The real body. |
| `{parent}$cont${block}` (`src/lir/cont_split.rs:264`) | | `name` is the parent's; `scopeLine` is the resume span. Shown as "parent (resumed after perform at L:C)". |
| `..._lambda_{id}` | `lower.rs:1937` | `"<lambda file:line:col>"` |
| dictionary definitions | `is_dict_lir_function` (`:95`) | `DIFlagArtificial` |

#### 2.4 Source locations

LIR gains locations without touching every instruction variant: each
`LirBlock` gets a parallel `locs: Vec<Option<SpanId>>` for its instructions,
plus a location for its terminator.

- `FnLower` tracks a `current_span`, updated as it enters each `AetherExpr`.
  `FnLower::emit` (`src/lir/lower.rs:1301`) is the single place every
  instruction passes through, so it stamps the span there.
- Columns convert from 0-based (`Position`) to 1-based (DWARF).
- `Dup`, `Drop`, `Reuse` and yield-check glue get line 0. A user never stops on
  reference-counting code.
- Inlining done by LLVM gets `inlinedAt` automatically. Inlining done by Core
  needs span provenance (S9); until then the debug profile turns Core inlining
  off.

#### 2.5 The debug profile

`DriverFlags.debug_info: DebugLevel { None, Lines, Full }`
(`src/driver/flags.rs:114`) is set by `--debug` or `-g`
(`src/cli/shared.rs:162-312`). In the debug profile, codegen uses an **Og**
pipeline (`opt -passes=default<O1>` plus `llc -O1`). That needs a new O1 case
next to O0/O2 (`pipeline.rs:96-205`) and `"frame-pointer"="all"`.

Plain `-O0` is not used. `tail call` is only a hint, and at `-O0` LLVM's fast
instruction selector does not honour it, so deep tail recursion would overflow
in debug builds and not in release builds.

- **Exit gate:** a 10⁶-deep tail-recursive fixture must complete in the debug
  profile.
- **Fallback if Og is not enough:** emit `musttail` where caller and callee
  prototypes match. That is common here: every argument is `i64` and every
  function uses `fastcc`.

#### 2.6 Variables and values

- Every parameter and `let` gets a `DILocalVariable`. Statically known `Int`s
  get `DIExpression(DW_OP_constu 1, DW_OP_shra)` to undo the `(raw<<1)|1` tag.
  Unboxed worker parameters use real base types.
- Every other variable has type `typedef FluxValue = i64`.
- ADTs become a `DW_TAG_structure_type` that matches the heap layout:
  - the header, then `i32 ctor_tag, i32 field_count, i64 fields[]`
    (`runtime/c/flux_rt.h:38-117`);
  - a `DW_TAG_variant_part` keyed on `ctor_tag`.

DWARF can't describe pointer tagging or the sentinels (None=0, False=2, True=4,
EmptyList=6). So **the lldb formatters in `tools/lldb/flux_lldb.py` are the
main way native values are printed**; DWARF types are secondary.
- The formatters read the sidecar `<binary>.fluxdbg` (constructor and field
  names, effect names, function kinds), written from `ProgramDebugInfo`.
- On macOS, the script is copied into
  `X.dSYM/Contents/Resources/Python/`, which Xcode loads.

#### 2.7 Linking and caching

- **macOS:** run `dsymutil` after `run_linker`. Today the native binary only
  points at cached `.o` files, so its debug information is lost when the cache
  is cleared.
- **Linux:** DWARF stays in the binary.
- **Runtime:** already built with `-O2 -g` (`pipeline.rs:774`).
- **Cache key (done in S0):** `compute_artifact_config_hash(strict, optimize,
  debug)` (`src/compiler/module_interface.rs`) keys compiled artifacts: VM
  bytecode, native objects (`compute_native_cache_key`,
  `src/llvm/module_cache.rs:91`) and store units. Interfaces stay keyed by
  `compute_semantic_config_hash(strict, optimize)`, since the debug level does
  not change them. Without `-g` the two hashes are equal, so existing cache
  keys are unchanged. The VM cache needs the debug level because `OpBreakpoint`
  changes the bytecode, and both caches need it so that a cached module is
  recompiled, and its debug information built, under `-g`.

#### 2.8 Windows: CodeView for Visual Studio call stacks

The only goal on Windows is that Visual Studio's Call Stack window shows Flux
frames with their source locations. The S5 metadata already describes
everything this needs: subprograms, locations and files. Only the output format
changes.

- **Metadata format:** on `*-windows-msvc` targets, emit the module flag
  `!{i32 2, !"CodeView", i32 1}` instead of `"Dwarf Version"`. LLVM then emits
  CodeView `.debug$S` / `.debug$T` sections from the same `DICompileUnit`,
  `DISubprogram` and `DILocation` nodes. The emitter needs no other change.
- **Linking:** add `/DEBUG` and `/PDB:<exe>.pdb` to the Windows `lld-link` /
  `link.exe` path in `run_linker` (`src/llvm/pipeline.rs:280-300`). That path
  already passes `/nologo /subsystem:console /STACK:67108864`.
- **Runtime:** build the C runtime with `-gcodeview`, so frames in
  `flux_panic` and the runtime also resolve. The Rust staticlib should be built
  with `debuginfo` in its debug profile.
- **Names:** use the `DISubprogram` `name` (`Flow.List.map`,
  `<lambda app.flx:12:5>`). Visual Studio shows `name`, not the `flux_*$w`
  linkage symbol. Artificial wrappers keep `DIFlagArtificial`; Visual Studio
  shows them, but with line 0 and no source.
- **Explicitly not included:** `.natvis` value formatting, a Visual Studio
  extension to host `flux-dap`, VM debugging, and recording.

### §3 VM engine

#### 3.1 Breakpoint sites (takes over 0129 Phase 3)

Under `--debug`, the bytecode compiler emits `OpBreakpoint(site_idx)` at every
**site**:
- `FnEntry` and `Return`
- `Stmt`, one per statement / `let`
- `PipeStage`, one per `|>` stage
- `MatchArm`
- `Perform`, `HandlerArm` and `Resume`

`FunctionDebugInfo` gains:
- `fn_id`
- `sites: Vec<BreakSite { kind, span, ip }>`
- `locals: Vec<LocalVarEntry { slot, var, ip_range }>`
- an `is_stmt` flag on each location

All of these are serialized in the bytecode cache
(`bytecode_cache/cache_serialization.rs:289,339`, `module_cache.rs:182,234`),
with a format version bump.

Design points:
- **Fusion:** superinstruction fusion keeps only the first location of a fused
  run (`src/compiler/mod.rs:7800`), so it must not fuse across a site.
- **Toggling breakpoints:** the enabled bitset lives in `DebugState`, not in
  the bytecode. Setting or clearing a breakpoint never recompiles.
- **Zero overhead:** the loop becomes `run_inner::<const DEBUG: bool>`
  (`src/vm/mod.rs:309`). Without a debugger attached, `OpBreakpoint` costs one
  predictable branch, and non-debug builds never contain it. This is the same
  model as GHCi: ticks are planned at desugaring and become `BRK_FUN`
  (`ghc/rts/Interpreter.c:748`).
- **Observer:** `VmObserver` (the shape 0024 described) is called at sites,
  calls, returns, `perform` and `resume`. It returns
  `ObserverAction::{Continue, Pause}`.

#### 3.2 Stepping

- **Step in:** stop at the next site in any frame.
- **Step over:** stop at the next site whose frame token is the current frame's
  or older. A token, not a frame index, is required because
  `tail_call_closure` (`src/vm/function_call.rs:281`) reuses the frame in
  place.
- **Step out:** stop when the frame returns.
- **Instruction step:** reuses `execute_current_instruction` (`mod.rs:355`).

**Caller locations.** When a call pushes a frame, `apply_ip_delta` (`mod.rs:375`)
has already moved the caller's `ip` past the call instruction. Caller frames
must therefore resolve at the recorded call site (`ip - width`), not at `ip`.
The same off-by-one affects today's runtime stack traces (`trace.rs:117`), and
it is fixed there too.

#### 3.3 Locals

- **Slot tables:** on the CFG path, from the `IrVar → Binding` map
  (`src/compiler/cfg_bytecode.rs:189`). On the AST fallback path, from
  `SymbolTable` (`src/compiler/symbol_table.rs:8`) before each scope closes.
- **Captured values:** from `Closure.free`, named through the lambda's
  `DebugVar`s.
- **Rendering:** `format_value` (`src/runtime/value.rs:542`), with the size
  limit of the REPL's `value_as_literal` (`src/repl/engine.rs:864`). Children
  load lazily through DAP `variablesReference`.

#### 3.4 Stacks that follow continuations

`OpPerform` (`src/vm/dispatch.rs:1286`) captures the performing frames into a
`Continuation` (`capture_to_boundary`, `function_call.rs:81`). The handler arm
then runs with those frames *off* `VM.frames`.

The debugger rebuilds the logical stack as three parts:
1. the live frames, down to the handler's `entry_frame_index`
   (`src/runtime/handler_frame.rs:10-34`);
2. a label pseudo-frame, "⟂ handling `Effect.op` — performed at …";
3. the frames inside `Continuation.frames`, marked "suspended, resumable".

Multi-shot continuations show which shot is running. Stepping into `resume`
(`execute_resume`, `function_call.rs:350`) lands at the `perform` site.

#### 3.5 Fibers as threads

- **Fibers:** add `FiberScheduler::debug_snapshot()`
  (`src/runtime/async/scheduler.rs:83`; its queues are private today), reached
  through the `SCHED`/`CURRENT` thread-locals (`src/vm/core_dispatch.rs:328-390`).
  Each fiber becomes a DAP thread. A parked fiber's stack comes from its
  `parked` continuation (`src/runtime/async/fiber.rs:71-120`).
- **Tasks:** OS-thread tasks (`src/vm/task.rs:95`) use a separate range of
  thread ids.
- **Blocked state:** the reason a fiber is blocked (channel, sleep, join, I/O
  request) comes from the scheduler's `suspended` table.

#### 3.6 Frames removed by tail calls

- A debug-only side table, indexed by frame token, records
  `{count, last_callers: ring<4>}`. `Frame` itself stays the same size.
- The stack view shows "… N calls elided by tail calls (last: f at L:C)".
- The native engine does not get this view. There, lldb's tail-call frame
  reconstruction is the best available.

### §4 Time travel

#### 4.1 The recording boundary

Pure code needs nothing recorded. What gets recorded is the same on both
backends:

- **Inputs (recorded; returned from the tape on replay):**
  - every primop whose fine effect label (`primop_fine_effect_label`,
    `src/syntax/builtin_effects.rs:151`) is Stdin, Filesystem read, metadata or
    list, Env, Process or Clock;
  - every completion from `AsyncBackend` (`src/runtime/async/backend.rs:119`):
    request id, payload and order. This covers TCP, HTTP, timers and sleeps;
  - scheduler decisions whenever more than one fiber is ready;
  - channel send/receive pairing and task join order.
- **Outputs (recorded as expectations; suppressed on replay):**
  - Console;
  - `FsWrite*`, `Remove` and `Rename`;
  - TCP and HTTP writes.
  
  If a replayed run produces a different output, the replay stops at once with
  a mismatch diagnostic, as 0038 specifies.
- **Terminal events:** panic and unhandled effect.

The runtime has no RNG and no randomized hashing (HAMT iteration order is
deterministic), so nothing else needs recording today. Any future source of
randomness must go through the tape. A test that greps for `RandomState` and
`rand` enforces this.

#### 4.2 One implementation for both backends

Implemented in S1a (synchronous effects) and S1b (async code).

- **The tape:** `src/runtime/replay/` holds the format, the record/replay
  state and the errors. Its API is three functions:
  - `effect(request, perform)` for inputs and outputs;
  - `console(request)` for stdout and stderr text;
  - `finish(failed, message)` at the end of the run.

  The mode (`Off`, `Record`, `Replay`) is one atomic, so with no tape the cost
  is one relaxed load per primop.
- **VM:** `src/vm/primop.rs` sends primops through `src/vm/replay_hook.rs`
  while a tape is active. `src/vm/replay_codec.rs` converts VM values to and
  from tape data structurally (text, list, `Ok`/`Err(IoError)`, metadata,
  process output, runtime failure).
- **Native:** the Rust staticlib is linked into every native binary. Each
  synchronous C primop has a wrapper that, while `flux_tape_mode` is set,
  records its result or rebuilds the recorded one (`runtime/c/tape.c`), through
  the `flux_tape_*` functions in `src/runtime/replay/native_abi.rs`. A tape
  error ends the process with one `flux-tape-error: {json}` line on stderr,
  which the driver renders as a diagnostic.
- **Driver:** `src/driver/replay_session.rs` hashes the program's modules,
  writes or checks the tape header, and hands a native program its tape
  through `FLUX_TAPE_RECORD` / `FLUX_TAPE_REPLAY`.
- **Async (S1b):** both backends run async code on the same `MioBackend`,
  and the program's instance sends everything through
  `src/runtime/replay/async_tape.rs`:
  - `submit` for each timer, DNS lookup and socket operation. Recording
    writes it; replaying checks it and does not perform it.
  - `deliver` for each completion the scheduler takes. Recording writes
    it where it arrived; replaying hands back the recorded one at the same
    point.

  This works because, under a tape:
  - **One worker.** The scheduler runs on one worker, printing a note if
    more were requested. Ready fibers then run in a fixed FIFO order, and
    the schedule depends only on the order of completions.
  - **Synchronous fs.** File system calls run synchronously even inside
    `run_async`, so they are recorded as ordinary effects.
  - **Event deadlines.** `Flow.Event` reads its deadline clock as an
    `event_clock` input, and waits on `after` through a backend timer
    instead of a sleeping thread.
  - **Fixed cancellation order.** Scopes, losers and leaked tasks are
    cancelled in id order rather than hash order. This also holds without
    a tape.

  Channels need nothing: they complete on the running fiber's thread.
  Tasks (`Task.spawn`) run on threads of their own; they stop a recording
  or replay with E1027 until S8.

#### 4.3 Tape format

A tape is JSON Lines (format version 1): one header line, then one event per
line in the order the program performed them.

```text
{"magic":"FLXTAPE","version":1,"flux_version":"0.0.7","program_hash":"sha256:…","argv":["app.flx"],"backend":"vm"}
{"seq":0,"kind":"read_file","path":"in.txt","result":{"text":"hello\n"}}
{"seq":1,"kind":"stdout","text":"hello\n"}
{"seq":2,"kind":"exit","failed":false}
```

- **Header:** magic, format version, the recording `flux` version (a
  difference only warns), `program_hash` (SHA-256 over the program's own
  modules; a tape replays only the program it was recorded from), argv, and
  the backend that recorded it.
- **Events:** `seq` (contiguous from 0), `kind`, the request's fields, and a
  `result` for inputs, outputs and completions. `fiber` (omitted when 0)
  is the logical fiber that performed the event: 0 outside `run_async`,
  otherwise fibers numbered from 1 in the order they first appear.
- **Async events (S1b):**
  - Submissions: `timer_start`, `dns_resolve`, `tcp_connect`,
    `tcp_listen`, `tcp_accept`, `tcp_read`, `tcp_write` and `tcp_close`.
    Each carries a logical `req`, numbered from 1 in the order requests
    are first submitted.
  - `async_complete{req}`, with the result, placed where the scheduler
    received it.
  - `event_clock`, which records each reading of the clock that
    `Flow.Event` deadlines use.

  ```text
  {"seq":4,"fiber":2,"kind":"timer_start","req":1,"ms":30}
  {"seq":5,"kind":"async_complete","req":1,"result":"unit"}
  ```
- **Results** are host data, never runtime values: `unit`, `bool`, `int`,
  `text`, `list`, `some`/`none`, `ok`, `err` (an IoError's kind, message and
  path), `meta`, `proc`, `fail` (a runtime error), `bytes` (read from a
  socket) and `addrs` (a DNS answer). Bytes that are not UTF-8 are written
  as `{"hex": …}`.
- **The last event** is `exit`, with whether the run failed. Replay compares
  `failed`, not the message, since the backends word failures differently.
- **Backend-neutral for synchronous effects:** a tape without async events
  replays on either backend. A tape with async events replays only on the
  backend that recorded it, since the two schedulers order fibers
  differently; replaying it on the other backend gives E1026.
  Cross-backend async replay is S9.
- **Step annotation (S3):** each event can optionally carry a `vm_step: u64`.
- **Known differences between the backends,** which a tape records as they
  are: native `read_stdin` reads one line where the VM reads all of stdin;
  the VM clock is wall-clock milliseconds, native's is monotonic; native
  `time(f)` is a clock reading, so `time` events appear only in VM tapes.

#### 4.4 Reverse execution (VM)

- **Step counter:** a global `step: u64`, incremented per instruction, in the
  debug dispatch loop only.
- **Checkpoints:** every K steps (adaptive, about every 50 ms of execution),
  snapshot the roots: stack slots, frames, `handler_stack`, `EffectContext`,
  globals, scheduler queues and the tape cursor.
  - Values are immutable and reference-counted, so a snapshot copies the roots
    and increments counts rather than copying deeply.
  - **Caveat:** a live snapshot holds extra references, so `Rc::get_mut` reuse
    (`dispatch.rs:1586-1673`) can't happen while it exists. This changes
    performance, not results. The reference-count view (S10) subtracts the
    debugger's own references.
- **Step Back / Reverse Continue:** restore the nearest earlier checkpoint,
  then replay forward from the tape to the target step. Reverse Continue
  replays twice: the first pass finds the last breakpoint hit before the
  current step, the second stops there.
- **Default recording:** debug sessions record by default. A ring buffer of
  checkpoints keeps memory bounded.
- **OS-thread tasks:** v1 records and replays them but disables reverse
  execution while they exist, with an explanation in the UI.

#### 4.5 What native supports

- Record, and deterministic replay under lldb.
- **Run to event N:** restart the replay and stop when the tape cursor reaches
  N. This needs no instruction counting.
- **Hand-off:** open the same tape in the VM engine to step backwards through
  the same execution.
- **No reverse execution of native code.** On Linux x86-64, `rr` works with a
  native Flux binary that has DWARF; the docs describe it as an optional
  external tool.
- Multi-worker native runs are recorded with `--workers 1` in v1 (see
  Unresolved questions).

### §5 Flux-specific views

| View | VM | Native | Stage |
|---|---|---|---|
| Dictionaries and wrappers hidden (`artificial` maps to DAP `presentationHint: subtle` and `visibility: internal`), with a "Show compiler internals" toggle | ✓ | ✓ | S2 / S6 |
| Stepping through `\|>` stages (`PipeStage` sites, column breakpoints, DAP `breakpointLocations`) | ✓ | distinct columns | S2 / S5 |
| Exception breakpoints: panic, unhandled effect, division by zero, match failure, fiber failure (VM error paths; native symbolic breakpoints on `flux_panic` and the unhandled-effect path) | ✓ | ✓ | S7 |
| Stacks that follow continuations; step into `resume` lands at the `perform` site (native: `flux_debug_yield_chain()` walks the `$cont$` closure chain) | ✓ | ✓ | S7 |
| Handler stack view (VM `handler_stack`; native evidence vector and markers) | ✓ | ✓ | S7 |
| Fibers, tasks and channels shown as threads with what they are blocked on (native: `flux_debug_fibers()` called through lldb) | ✓ | ✓ | S8 |
| Effect timeline: tape events, click one to travel there | ✓ | run-to-event | S3 / S9 |
| Watch expressions, allowed only when the inferred effect row is empty, so evaluating them can't cause side effects | ✓ | formatter-level only | S4 |
| Aether view: reference counts, "unique, so reuse is possible", and a debug-heap runtime (`libflux_rt_debug.a`) that poisons freed memory and records allocation sites | ✓ | ✓ | S10 |
| Panic backtraces without a debugger (VM today; native: unwind plus symbolize through `gimli` in the staticlib, then delete the unused `flux_trace_push`) | ✓ | ✓ | S10 |

Native effects need special handling in two cases:
- Tail-resumptive operations go through `flux_perform_direct`
  (`runtime/c/effects.c:475`). The performing frames are still physically on
  the stack, so DWARF shows them with no extra work.
- The general path is `flux_yield_to` (`effects.c:324`). There the performing
  frames have already returned, and only a chain of `$cont$` closures
  describes what will run next.

### §6 Staging

| stage | change | exit |
|---|---|---|
| S0 | `src/debuginfo/` model (functions, lambdas, variables with types, ADT layouts, effects), built per module where Core is lowered and kept on `Compiler`; `--debug` / `-g` and `DebugLevel`; debug level in the cache keys; `--dump-debug-info` | Snapshot tests of `--dump-debug-info` for ~8 fixtures, including a two-module program with each module in its own file. A test that cache keys differ by debug level while the default key is unchanged. `-g` leaves the LLVM IR and bytecode unchanged in S0. |
| S1a | Recording of synchronous effects (implements 0038): the tape format and runtime, VM hook, native C/Rust ABI, `--record`/`--replay`, E1022–E1027; async refused | Tapes recorded on either backend replay on both with identical output; replay reads and changes nothing outside the program; edited tapes fail with E1022–E1026; async under a tape fails with E1027. Off-mode overhead measured with `benches/effect_tape_off_bench.rs`. |
| S1b | Async recording: submissions and completions through the shared async backend, one worker and synchronous fs under a tape, the taped event clock, logical request and fiber ids; `Task.spawn` refused until S8 | Every async fixture (sleep, channels, events, scopes, fs, TCP) and every async example replays on the backend that recorded it, without waiting on timers or opening sockets; edited async tapes fail with E1022/E1023; an async tape on the other backend fails with E1026. |
| S2 | VM engine behind a `DebugEngine` trait: `OpBreakpoint` sites, debug dispatch loop, `VmObserver`, stepping, local-variable tables, caller location fix (`ip - width`), tail-call elision counter, hiding dictionaries, pipeline stages; `IrFunction.var_binders` side table; Core inlining off in the debug profile | Golden transcripts of scripted sessions (`tests/debug/vm/*.flxdbg`). A benchmark showing no change in non-debug runs. |
| S3 | VM reverse execution: checkpoints, Step Back, Reverse Continue, run to event, recording ring buffer | Golden transcripts where the state after stepping back equals the state reached by replaying forward. A memory-bound test. |
| S4 | `crates/flux-dap`, mirroring `crates/flux-lsp`; `editors/vscode` `debuggers` and `breakpoints` contributions; launch config with `backend`, `record`, `replay`; pure-only evaluate | DAP JSON transcript tests in `crates/flux-dap/tests`. A manual VS Code checklist in the crate README. |
| S5 | Native line tables: IR metadata model, compile units, files, subprograms, locations, LIR location stamping, `Span::default()` audit, Og profile, `dsymutil`, real `source_filename` | `llvm-dwarfdump --verify` clean on all native fixtures. `lldb -b` scripts (break at file:line, `bt`, step) in `tests/native_llvm/debug_*.rs`. 10⁶-deep tail recursion passes in the debug profile. A manual Xcode checklist. |
| S6 | Native variables and types: untagging expression, ADT `variant_part`, `flux_lldb.py` embedded in the dSYM, `.fluxdbg` sidecar, the native engine in `flux-dap` (proxying `lldb-dap`), `LirFunction.var_binders` side table | **Debug parity harness:** `Way::VmDebug` / `Way::LlvmDebug` in `src/parity/mod.rs:73`, and fixture directives `// dbg: break app.flx:12`, `// dbg: step`, `// dbg-expect: …` in `src/parity/fixture.rs`. Normalized transcripts must match: top frame and locals exactly, the full stack modulo tail-call elision. |
| S7 | Effects: stacks that follow continuations (both backends), handler view, exception breakpoints, step into `resume` | Parity fixtures for multi-shot, nested handlers, and tail-resumptive vs yielding operations. |
| S8 | Fibers, tasks and channels as threads; scheduler snapshot APIs; multi-fiber record and replay | Parity fixtures using `both`, `race` and `timeout`. Replay is identical across 100 runs. |
| S9 | Native replay under lldb, run to event, opening a native tape in the VM, timeline UI; Core-inlining provenance → `inlinedAt`, and Core inlining turned back on in the debug profile | Cross-backend tape tests. Golden `lldb bt` output for inlined frames. |
| S10 | Aether reference-count view, debug-heap runtime, native panic symbolization; `flux_trace_push` deleted | Golden reference counts. A use-after-free fixture that the debug heap catches. Native panic output lists Flux frames. |
| S11 | Windows call stacks for Visual Studio (§2.8): `"CodeView"` module flag on MSVC targets, `/DEBUG` and `/PDB` in `run_linker`, runtime built with `-gcodeview` | On a Windows CI runner, `llvm-pdbutil dump -symbols app.pdb` lists Flux subprogram names with `.flx` line entries. `cdb -z app.exe -c "g; k; q"` on a fixture that panics prints Flux frames with `app.flx` file and line. A manual check that Visual Studio's Call Stack window shows them. |

S0 and S1 come first because everything else depends on them. After that, the
**VM line (S2–S4)** and the **native line (S5–S6)** can proceed in parallel.
The S6 parity harness then keeps them consistent for S7–S10.
S11 depends only on S5 and can land any time after it.

## Drawbacks
[drawbacks]: #drawbacks

- **Two engines to maintain.** The VM engine and the lldb-based native engine
  share the adapter and the views but not the stepping logic. The S6 parity
  harness is the safeguard against them drifting apart.
- **A broad change.** `ProgramDebugInfo` and binder ids have to pass through
  Core, Aether, CFG and LIR.
- **Native values depend on formatters.** Without `flux_lldb.py` (for example,
  plain `lldb` with script loading disabled), values are raw `i64`s.
- **Debug builds use different code generation from release builds** (Og vs
  O2, Core inlining off). A bug that only happens at O2 may not reproduce under
  the debugger. Recording a release run and replaying it on the VM is the
  intended remedy.
- **Checkpoints block in-place reuse** while a session is stepping backwards.
  Execution is slower, but results are unchanged.
- **The debug profile needs its own bytecode and object cache entries.**

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- **An lldb Python plugin only.** This can't debug the VM, and the VM is the
  default backend and Flume's dev profile. Rejected.
- **Separate adapters:** VS Code talks to `lldb-dap` directly for native, and to
  a VM-only adapter for the VM. This loses the shared Flux views, the tape
  hand-off and the parity harness. Rejected in favour of one `flux-dap` that
  wraps `lldb-dap` for native.
- **Emitting DWARF for the VM's bytecode through the GDB JIT interface.** Flux
  has no JIT. Describing bytecode frames in DWARF would be complex and still
  wouldn't support reverse execution. Rejected.
- **A shadow stack everywhere** (reviving `flux_trace_push`). It costs time on
  every call, and the existing one is a global rather than per-thread. Rejected,
  except possibly as a fallback for panic traces.
- **Going back to an explicit stack, as in 0122.** That would break standard
  unwinding, which is the reason DWARF works here at all. Rejected.
- **Relying on `rr` for time travel.** It only works on Linux x86-64, and never
  on Apple Silicon. It is documented as optional for native; the tape is the
  portable mechanism.
- **Following GHC.** GHC's DWARF is mainly for profilers and unwinding, and it
  does not cover the target platforms (see Prior art). GHCi's breakpoint
  design is followed for the VM (§3.1).

## Prior art
[prior-art]: #prior-art

Checked against the GHC source tree (commit `65bcf9e911`):

- **GHC `-g`** emits DWARF only from the native code generator, only for ELF
  targets, and never for AArch64:
  `ncgDwarfEnabled = osElfTarget … && debugLevel > 0 && arch /= ArchAArch64`
  (`compiler/GHC/Driver/Config/CmmToAsm.hs:74`). On macOS it emits nothing.
  GHC's LLVM backend has a metadata model (`GHC/Llvm/MetaData.hs`) but emits no
  debug metadata. Flux's native DWARF would cover platforms GHC does not.
- **GHCi debugger.** Ticks are inserted during desugaring
  (`GHC/HsToCore/Ticks.hs`, `Breakpoints.hs`). They become `BRK_FUN` at the
  head of a bytecode object (`rts/Interpreter.c:748`). Breakpoints are by line,
  line and column, or qualified name. The docs state that compiled code is
  invisible to the debugger. This is the model for §3.1.
- **GHC exception backtraces**
  (`libraries/ghc-internal/src/GHC/Internal/Exception/Backtrace.hs`) offer four
  mechanisms: cost centres, `HasCallStack`, execution stack (DWARF) and IPE.
  Only `HasCallStack`, an implicit parameter (`Stack/Types.hs:75`), is on by
  default. It is the model for the `@caller` idea in Future possibilities.
- **`haskell-debugger` and `phoityne-vscode`:** DAP adapters built on GHCi,
  the same architecture as the VM engine. They are outside the GHC tree and
  have not been checked against their source for this proposal.
- **rustc:** emits DWARF variant parts for Rust enums and ships lldb and gdb
  formatter scripts. That is the model for §2.6.
- **`rr` / Pernosco, and Elm's time-travel debugger:** the first records a whole
  process; the second replays a pure program from recorded inputs. Flux's tape
  sits between them: it records only effect results, but works on any program.
- **OCaml (`ocamldebug`)** has time travel by checkpoints and re-execution for
  bytecode programs, which is the approach in §4.4.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- **Language code.** `DW_LANG_C99` gives the best lldb behaviour. A
  Haskell or vendor-specific code would describe Flux more accurately, but
  may make lldb fall back to raw output. Decide in S5 by testing in Xcode.
- **DWARF version on Apple:** 4 or 5? Check what Xcode's lldb handles best for
  `variant_part`.
- **Core inlining in the debug profile:** keep it off permanently, or turn it
  back on once provenance exists (S9)?
- **Closures in recorded values.** Channel and task messages may contain
  closures. Options: record by logical identity, or forbid recording such
  programs in v1.
- **Native multi-worker replay.** Is recording every steal decision affordable,
  or is `--workers 1` permanent for recorded runs?
- **Memory budget** for always-on recording in debug sessions.
- **LLVM versions:** which ones the `llvm.dbg.value` / `#dbg_value` form must
  support.
- **Windows beyond call stacks:** S11 gives Visual Studio call stacks only.
  Should a later proposal add `.natvis` value formatting, or a Visual Studio
  extension that hosts `flux-dap` (giving VM debugging and recording)? Not
  planned here.
- **Flume:** should the dev profile (`lib/Flume/Schema/Manifest.flx:319`) turn
  on `--debug` by default?

## Future possibilities
[future-possibilities]: #future-possibilities

- **`@caller` source locations:** an implicit caller-location parameter in the
  language, like `HasCallStack`. The type checker supplies it the way it
  supplies dictionaries, so `panic`, `unwrap` and assertions print the same
  call chain on both backends without a debugger.
- **REPL debugging:** `:break`, `:step` and `:back` in the REPL (0176), built on
  the same `DebugEngine` API.
- **LSP hovers** showing live values while the program is paused.
- **Hot reload** of a function in a paused VM session.
- **Omniscient queries over a tape**, e.g. "when did this value first become
  `None`?"
