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
flux debug app.flx                    # VM, a command-line debugger (S2)
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
  "backend": "vm"
}
```

- With `"backend": "vm"` (S4), every session keeps its effects in memory, so
  the Step Back and Reverse Continue buttons work.
  `"record": "trace.flxtape"` also writes them to a tape.
- `"replay": "trace.flxtape"` replays a recording.
- `"backend": "native"` (S6) builds with `--debug` and starts `lldb-dap`
  underneath `flux-dap`. You see the same Flux views, and VS Code
  configuration is identical.

### Xcode

1. Create an External Build System target that runs
   `flux build --native --debug`.
2. Set the produced binary as the scheme's executable.

Breakpoints in `.flx` files, the call stack and stepping work through Xcode's
built-in lldb. `flux_lldb.py` is embedded in the dSYM, so lldb loads it once
`settings set target.load-script-from-symbol-file true` is in `~/.lldbinit`,
and values print as Flux values. As built, the target runs
`flux app.flx --native -g --emit-binary` (`flux build` is the package
command); see `docs/tooling/native_debugging_lldb_xcode.md`.

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
  no generated code in S0. As built in S6, a `-g` native build runs the Core
  simplifier without `disciplined_inline`, `inline_lets` and `elim_dead_let`
  (`run_core_passes_keeping_variables`), so every `let` the program wrote
  reaches native code and the debugger, as on the VM, which compiles from
  the AST.

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

Implemented in S5. `src/llvm/ir/syntax.rs` and `ppr.rs` gain
`LlvmModule.debug: Option<ModuleDebug>`, a side table, so the ~130 places
that build functions and blocks are unchanged:
- numbered metadata nodes, printed `!N = …`, and named metadata:
  `!llvm.dbg.cu`, plus `!llvm.module.flags` with `"Debug Info Version" 3` and
  `"Dwarf Version" 4`;
- per function (by name), its subprogram, printed `!dbg !N` after the
  attributes, and per block a list of change points: "from instruction i
  on, location !N", plus the terminator's. Every instruction of a function
  with a subprogram is printed with `, !dbg !N`; one no mark covers gets the
  function's line-0 location, since LLVM requires a location on every call
  there;
- `source_filename` set to the module's real path, in every per-module
  native build.

Variable records use the `llvm.dbg.declare` intrinsic form (§2.6), which
LLVM 18 and later parse and current LLVM converts to `#dbg_declare` records.
The IR model gained `LlvmType::Metadata` and `LlvmOperand::Metadata` for
their arguments.

#### 2.3 Compile units and subprograms

- One `DICompileUnit` per emitted module, since native compiles and caches per
  module (`src/lir/emit_debug.rs`):
  - `producer: "flux <version>"`
  - `emissionKind: LineTablesOnly` in S5; `FullDebug` from S6, with
    variables and types.
- One `DIFile`, the module's source.

As built in S5, the worker/wrapper split differs from the table below:
`flux_X` is not a thin wrapper but a second full emission of the body, so
`flux_X`, `flux_X$w` and `flux_X$dc` all get real subprograms with the Flux
name, and a line breakpoint resolves to more than one location. The closure
entry wrappers and outlined `run_async` helpers get no subprogram, so a
debugger steps through them like library code built without `-g`. Lambdas
and local functions are `<lambda file:line:col>`, handler clauses
`<handler op>`, continuations `parent (resumed after L:C)`. Dictionary
functions are `DIFlagArtificial`.

Every function symbol gets a `DISubprogram`, according to its kind:

| Symbol | Source (`emit_llvm.rs`) | `DISubprogram` |
|---|---|---|
| `flux_X$w` (worker) | `:230` | The real body. `name` is the Flux name. As built, there is no `linkageName`: with one, lldb names frames by the symbol (`flux_app_square`) instead of `name`. |
| `flux_X`, closure wrappers | `:208`, `:279`, `:548` | `DIFlagArtificial`, line-0 locations, so stepping passes through. |
| `flux_X$dc` | `:254` | The real body. |
| `{parent}$cont${block}` (`src/lir/cont_split.rs:264`) | | `name` is the parent's; `scopeLine` is the resume span. Shown as "parent (resumed after perform at L:C)". |
| `..._lambda_{id}` | `lower.rs:1937` | `"<lambda file:line:col>"` |
| dictionary definitions | `is_dict_lir_function` (`:95`) | `DIFlagArtificial` |

#### 2.4 Source locations

LIR gains locations without touching every instruction variant: each
`LirBlock` gets a parallel `locs: Vec<LirLoc>` for its instructions (an
instruction past its end has none), plus `term_loc` for its terminator.
`run_async_outline` splits them with the instructions; `cont_split` copies
them with the blocks it clones.

- `FnLower` tracks a `current_loc`, set as it enters each `AetherExpr` or
  `CoreExpr` and restored after. `FnLower::emit` is the single place every
  instruction passes through, so it stamps the location there; a node with a
  made-up span keeps its parent's. A `return` gets the location of the last
  expression lowered, not the function's own line. A `return` in a block
  where branches join (after a `match` or `if`) is at line 0 (S6): it
  belongs to no one branch, and a breakpoint on the last branch would
  otherwise stop for every branch.
- Columns convert from 0-based (`Position`) to 1-based (DWARF).
- `Dup`, `Drop`, `Reuse` and yield-check glue get line 0. A user never stops on
  reference-counting code.
- Inlining done by LLVM gets `inlinedAt` automatically. Inlining done by Core
  needs span provenance (S9); until then the debug profile turns Core's `let`
  inlining off (S6).

#### 2.5 The debug profile

`DriverFlags.debug_info: DebugLevel { None, Lines, Full }`
(`src/driver/flags.rs:114`) is set by `--debug` or `-g`
(`src/cli/shared.rs:162-312`). As built in S5, `-g` keeps the build's opt level (O0, or O2 with
`--optimize`) and adds `"frame-pointer"="all"` to every function with a
subprogram. The concern below did not hold: at O0 the generated `tail call
fastcc` is honoured, and a 2·10⁷-deep tail-recursive loop runs in a `-g`
build on Apple clang 21. So no Og profile was needed. Should a toolchain stop
honouring it, the fallback stays as written: O1 for `-g`, or `musttail`.

The original concern: `tail call` is only a hint, and at `-O0` LLVM's fast
instruction selector might not honour it, so deep tail recursion would
overflow in debug builds and not in release builds.

- **Exit gate:** a 10⁶-deep tail-recursive fixture must complete in the debug
  profile.
- **Fallback if Og is not enough:** emit `musttail` where caller and callee
  prototypes match. That is common here: every argument is `i64` and every
  function uses `fastcc`.

#### 2.6 Variables and values

As built in S6a. The original plan (an untagging `DIExpression` for Ints,
`llvm.dbg.value` on SSA values, a `DW_TAG_variant_part` per ADT and a
`.fluxdbg` sidecar) changed in three places:

- **Debug slots, not `dbg.value`.** Every LIR variable is an SSA value, and
  at O0 the register allocator reuses registers so freely that lldb shows
  most `dbg.value` variables as unavailable. So, as clang does for C, each
  source variable gets an entry-block `alloca i64` that only the debugger
  reads, described by `llvm.dbg.declare` (`DebugBuilder::add_variables`,
  `src/lir/emit_debug.rs`):
  - the slot holds an *unbound* marker until the variable is bound;
    `flux_lldb.py` hides such a variable, so a frame shows only what is in
    scope by now;
  - the value is stored right after the instruction that defines it, or
    after the block's phis, never between a tail call and its `ret`;
  - after `flux_drop` / `flux_drop_reuse` of the value, a *dropped* marker:
    Aether frees a value at its last use, and lldb must not read freed or
    reused memory. The variable then shows as `<dropped>`;
  - the markers (`0x0DEB0DEB00000004`, `…02`) are even, so no tagged Int,
    and far above user-space addresses, so no pointer.

  Which source variable each LIR variable is comes from `LirFnDebug.vars`
  (`LirVarBinder{var, name, kind, loc, rep}`), recorded where lowering binds
  parameters, `let`s, local functions, pattern variables and captures
  (`FnLower::bind_source`, `bind_param`, `bind_captured`).
- **Typedefs that say how to read a word.** Every variable is a 64-bit word
  of one of these typedefs of `int64_t`:
  - `FluxValue`: a tagged value;
  - `FluxCaptured`: a tagged value the closure captured;
  - `FluxInt`, `FluxBool`: a raw Int or Bool in an unboxed `$w` worker;
  - `FluxWord`: a worker word whose representation lowering did not know.
  DWARF cannot describe pointer tagging or the sentinels (None=0, False=2,
  True=4, EmptyList=6), so **`tools/lldb/flux_lldb.py` is the main way
  native values are printed**, as planned.
- **Constructor names in the DWARF, not a sidecar.** Each compile unit
  carries `FluxCtor`, an enumeration naming every constructor tag the module
  knows, and a structure `FluxCtor.<Name>` for each constructor it declares
  with fields: `ctor_tag`, `field_count`, then one `FluxValue` per field,
  named. No variable has these types and `dsymutil` drops unreferenced
  types, so a global `flux_debug_types_<hash>` whose type holds them keeps
  them in the dSYM. The script merges `FluxCtor` across all units, so a
  value of a type another module declares is named too.

`flux_lldb.py`:
- reads memory only; it never calls into the paused program;
- renders values as the VM's `ValueView::render` does (floats as Rust's
  `{:?}`, `(a,)`, `[|…|]`, the 120-character cut), and gives compound values
  children (`0`, `1`, … or field names), so Xcode's and VS Code's variable
  views expand them;
- adds `flux bt`, `flux locals` and `flux print <name>[.<part>…]`, printing
  in `flux debug`'s format;
- sets `step-avoid-regexp` so stepping passes over the runtime, which is
  built with debug information.

On macOS, a `-g` build with `-o` or `--emit-binary` copies it to
`X.dSYM/Contents/Resources/Python/X.py`, which lldb and Xcode load once
`settings set target.load-script-from-symbol-file true` is set. `flux debug
--lldb-script` prints it, for Linux and plain lldb sessions.

#### 2.7 Linking and caching

- **macOS:** run `dsymutil` after `run_linker` for a `-g` binary the user
  keeps (`-o`, `--emit-binary`), writing `<binary>.dSYM`. Otherwise the binary
  only points at cached `.o` files, so its debug information is lost when the
  cache is cleared. Run mode, whose binary is temporary, skips it.
- **Windows (MSVC):** `-g` emits no metadata until S11 adds CodeView.
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

Implemented in S2.

**One compiler under `-g`.** Every function that can be compiled by the AST
bytecode compiler is compiled by it under `-g`
(`use_ast_path`, `src/compiler/statement.rs`). That compiler knows each
local's name and where statements start, which the CFG path loses. Most code
already took it: the CFG path rejects any backward jump (every `if` and
`match`) and closures. Functions that take dictionaries from evidence still
need the CFG path; under `-g` they get an entry site but no statement sites
or locals. Two consequences:
- Core passes do not shape `-g` VM code, so `IrFunction.var_binders` and
  "Core inlining off" leave the VM line. They are native work (S5/S6).
- `-g` can differ from a plain run where the two compilers already differ.
  The one case found across `examples/` (1,111 programs, compared with and
  without `-g`) is an unused binding whose value fails: `let ignored = 100 / x`
  raises under `-g` and is optimized away in a plain run.

**Sites.** Under `-g` the compiler emits `OpBreakpoint(site)` (opcode 63,
`src/compiler/debug_sites.rs`) at:

| Site | Where |
|---|---|
| `FnEntry` | the start of each function, lambda and handler arm |
| `Stmt` | each statement in a function body |
| `MatchArm` | the start of each arm's body |
| `PipeStage` | each `|>` stage, right before its call, so a pipeline stops stage by stage in source order |
| `Perform` | each effect operation, after its arguments |

The parser records which calls came from `|>` (`Program::pipe_calls`).
`FunctionDebugInfo` gains `sites`, `locals` (slot, name, live instruction
range, an artificial flag) and `free_names`. They are serialized in both
bytecode caches (cache epoch 77). A site with no source line (compiler-made
code) is not emitted.

**No cost without `-g`.** A plain build contains no `OpBreakpoint`, and its
bytecode is unchanged. With `-g` and no debugger attached, the opcode does
nothing. A `const DEBUG` dispatch loop was not needed. The VM's hot paths
gain one well-predicted branch on `VM.debugger` per call and tail call, and a
store of the caller's position per call (below). With
`CARGO_PROFILE_BENCH_CODEGEN_UNITS=1`, `effect_tape_off_bench` measures the
same as S1b or faster.

**Observer.** `src/vm/debug/`: `VmObserver::on_stop(reason, &mut Inspector)`
returns a `Resume` (continue, step in, over or out, quit). The `Inspector`
reads the paused VM: the stack, each frame's locals and captured values,
values with their parts, and line breakpoints, resolved against every
function's sites (a line without code moves to the next line that has some).
Breakpoints live in debugger state; setting one never recompiles.

**Interfaces under `-g`.** The AST compiler exports different globals from
the CFG compiler, so VM module interfaces are keyed by the debug level too
(`compute_artifact_config_hash`). The diagnostic replay of a module that
failed in the parallel pipeline compiles with the run's debug level; before,
it compiled without `-g`, found no error, and the run went on without the
module.

#### 3.2 Stepping

- **Step in:** stop at the next site in any frame.
- **Step over:** stop at the next site at this frame index or below.
- **Step out:** stop at the next site below this frame index.
- **One stop per line:** a line stops at its first site; the other sites on
  it (an arm, a stage, a perform) do not stop again until another line runs
  in that frame, or the frame starts a new call or tail call.
- **Tail calls** keep the frame index, so stepping over a tail call stops in
  the function it calls, as GDB does with tail-call optimization.
- **Compiler-made code** (dictionaries, default handlers around `main`,
  intrinsic wrappers: `__` names) is stepped through, not into, unless the
  debugger shows internals. Instance methods are user code and are shown as
  `Class<Type>.method`.

**Caller locations.** When a call pushes a frame, `apply_ip_delta` then moves
the caller's `ip` past the call instruction. `push_frame` now records each
caller's position before that happens (`VM.call_ips`), and caller frames
resolve there, both in the debugger and in runtime stack traces
(`src/vm/trace.rs`). The traces now name the call that was running, where
they used to name the call around it: `at main (app.flx:36:11)` for
`print(Ki076Ord.demo())` rather than column 5.

**Tail-call elision.** While a debugger is attached, each tail call counts
against its frame, with the last four callers it replaced. The stack shows
`… N calls elided by tail calls (last: f at L:C)`.

#### 3.3 Locals

- **Slot tables:** the symbol table logs each local it defines under `-g`,
  and the compiler enters it with the instruction it starts at; leaving a
  block scope ends it. Parameters hold their values from the function's first
  instruction. A name bound twice shows its innermost binding.
- **Captured values:** `Closure.free`, named by `free_names`.
- **Rendering:** one bounded line per value, floats with their decimal
  point; the parts of tuples, lists, arrays, options and data constructors
  load on demand (`print tree.2.1`).

**Front ends.** `flux debug app.flx [--script file]` runs the program on
the VM, built with `-g`, under a command-line debugger
(`src/driver/debug_session.rs`): `break`, `delete`, `continue`, `step`,
`next`, `finish`, `bt`, `locals`, `print`, `internals on|off`, `quit`, and
the S3 reverse commands. It reads commands from a terminal as typed, or from
a script or pipe. It works with `--record` and `--replay`. Golden
transcripts are in `tests/debug/vm/` (`tests/debug/vm_debug_tests.rs`).

`flux debug app.flx --engine <address>` serves the same engine to a debug
adapter over a localhost socket (`src/driver/debug_engine.rs`), one JSON
object per line (`src/debug_protocol.rs`). `crates/flux-dap` (S4) is that
adapter: it speaks DAP to the editor over stdio, starts the program as that
child process, and passes on its stdout and stderr as `output` events. A
running program reads requests at its next stop site, so breakpoint changes
apply and a pause stops there.

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
- **Step annotation (S9):** each event can optionally carry a `vm_step: u64`,
  for opening a native tape in the VM. S3 keeps each event's position in
  memory only.
- **Known differences between the backends,** which a tape records as they
  are: native `read_stdin` reads one line where the VM reads all of stdin;
  the VM clock is wall-clock milliseconds, native's is monotonic; native
  `time(f)` is a clock reading, so `time` events appear only in VM tapes.

#### 4.4 Reverse execution (VM)

Implemented in S3 (`src/vm/debug/`), for programs without async code.

- **Position:** the count of stop sites (`OpBreakpoint`) run so far, kept
  only while a debugger is attached. Counting sites, not instructions, keeps
  every non-debug run free of it, and a replay makes it deterministic.
  `flux debug` shows it on each stop (`(position 37)`), `where` prints it,
  and `goto <position>` jumps to it.
- **Checkpoints** copy the roots: the live stack, frames, caller call sites,
  `handler_stack`, `EffectContext`, the globals in use, the debugger's
  per-frame state (line dedupe, tail-call counts, call serials) and the tape
  cursor.
  - Values are immutable and reference-counted, so a checkpoint copies the
    roots and increments counts rather than copying deeply.
  - **Caveat:** a live checkpoint holds extra references, so `Rc::get_mut`
    reuse (`dispatch.rs:1586-1673`) can't happen while it exists. This
    changes performance, not results. The reference-count view (S10)
    subtracts the debugger's own references.
  - **Only at top-level sites.** Higher-order primops (`List.map`, `time`)
    and resumed continuations run nested dispatch loops on the Rust stack,
    which a checkpoint cannot bring back. Sites inside them are reached by
    running forward from a checkpoint taken before.
  - **Bounded:** the first checkpoint is taken before the program runs, then
    one every K sites, with K adapting so they are about 50 ms of execution
    apart. At 256 checkpoints, or 256 MB of copied roots, every other one
    after the first is dropped and K doubles, so coverage thins towards the
    start and memory stays bounded. Any position stays reachable through the
    first checkpoint. `FLUX_DEBUG_CHECKPOINT_EVERY=<sites>` fixes K for tests.
- **Going back** restores a checkpoint and runs forward silently to a
  position. A rewind unwinds the run with an error, out of any nested loops,
  to `VM::run`, which restores the checkpoint and starts the loop again.
  Finding the position takes a first silent pass over the stretch before the
  stop, noting the last site that matches; if none does, the stretch before
  it is scanned, back to the first checkpoint:

  | Command | Goes back to the last site that |
  |---|---|
  | `reverse-step` | a forward step would stop at |
  | `reverse-next` | is in this call or a caller (each call has a serial, so a sibling call's lines don't count) |
  | `reverse-finish` | is in a caller: the line that made the call |
  | `reverse-continue` | is a breakpoint hit |

  With no match before the stop, the debugger stops at the first stop of the
  run, "the start of the recording". At the target, the site must be the one
  the first pass saw; if not, the run is not deterministic and the debugger
  reports it.
- **Run to event:** `events` lists the tape's effects with the position each
  happened at. `event N` goes to the last place a step would stop before
  effect N, backwards or forwards.
- **Recording by default:** every `flux debug` session keeps its effects in
  memory (`--record` also writes them to the file, `--replay` keeps its
  tape). After going back, effects are answered from what was kept until the
  run reaches the end of it, then recorded again, so going back never
  performs an effect twice. Console text is printed only the first time the
  run reaches it.
- **Async code:** once the program enters `run_async`, going back is off for
  the rest of the session, with the reason shown, and the session's own
  recording stops. The scheduler's state lives in thread-locals and global
  tables that checkpoints do not copy yet. S8 adds it.
- **OS-thread tasks:** a session's own recording stops when the program
  starts a task, and going back is off from there. A `--record` or
  `--replay` run still refuses tasks (E1027) until S8.

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
| Watch expressions and hovers: a variable or a part of it (`pair.0`) in S4; any expression later, allowed only when its inferred effect row is empty, so evaluating it can't cause side effects | ✓ | formatter-level only | S4 / later |
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
| S2 | VM engine: `OpBreakpoint` sites (entry, statement, match arm, `|>` stage, perform) from the AST compiler under `-g`, `VmObserver` and `Inspector` (`src/vm/debug/`), step in/over/out, local-variable tables, caller-location fix (also in runtime stack traces), tail-call elision counter, compiler-made code hidden, `flux debug` front end | Golden transcripts of scripted sessions (`tests/debug/vm/*.flxdbg`). Every example runs the same with and without `-g`, apart from timing and one unused failing binding. No slowdown in non-debug runs. |
| S3 | VM reverse execution for programs without async code: positions (site counts), checkpoints at top-level sites in a bounded ring, reverse step/next/finish/continue by restoring a checkpoint and replaying forward, `goto`, run to event, every `flux debug` session recording in memory, console output printed once | Golden transcripts where coming back to a stop shows the stack and values seen going forward, the same wherever checkpoints fall. A memory-bound test (900,000 sites, at most 256 checkpoints). Going back performs no effect twice. |
| S4 | `crates/flux-dap`, mirroring `crates/flux-lsp`: DAP over stdio, the program in a `flux debug --engine` child driven over a JSON-lines socket protocol; breakpoints, stack (tail-call labels), scopes and variables, stepping, Step Back and Reverse Continue, pause; watch, hover and Debug Console evaluate of variable paths, plus console commands for positions and effects; `editors/vscode` `debuggers` and `breakpoints` contributions; launch config with `backend`, `record`, `replay`. Evaluating any pure expression moves to a later stage. | DAP JSON transcript tests in `crates/flux-dap/tests`. A manual VS Code checklist in the crate README. |
| S5 | Native line tables: LIR locations from Aether spans (reference counting at line 0), the IR metadata side table, one compile unit and file per module, subprograms per symbol kind, locations, real `source_filename`, frame pointers under `-g`, `dsymutil` for kept binaries; the opt level unchanged (O0 honours tail calls); Windows waits for S11 | `llvm-dwarfdump --verify` clean on the native debug fixtures (`tests/native_llvm/debug/`). `lldb -b` sessions break at file:line, show Flux frames and step (`tests/native_llvm/native_debug_tests.rs`). 10⁶-deep tail recursion runs in a `-g` build. Every guide example prints the same natively with and without `-g`. A manual Xcode checklist. |
| S6a | Native variables and types: the `LirFnDebug.vars` side table, a debug slot per variable with `llvm.dbg.declare`, unbound and dropped markers, `FluxValue`/`FluxCaptured`/`FluxInt`/`FluxBool`/`FluxWord` typedefs, the `FluxCtor` enumeration and `FluxCtor.<Name>` structures in the DWARF (instead of a `.fluxdbg` sidecar), `flux_lldb.py` (Flux values, children, `flux bt`/`locals`/`print`) embedded in the dSYM and printed by `flux debug --lldb-script`, Core `let` inlining off under `-g` | `tests/native_llvm/native_debug_tests.rs`: lldb with the script shows each kind of value as the VM does, variables only once bound, captured variables, the matched arm's binders, constructors another module declares; the dSYM carries the script. `llvm-dwarfdump --verify` clean; 10⁶-deep tail recursion still runs under `-g`; every guide example prints the same natively with and without `-g`. |
| S6b | The native engine in `flux-dap` (building with `-g`, proxying `lldb-dap`), `"backend": "native"` in VS Code | **Debug parity harness:** `Way::VmDebug` / `Way::LlvmDebug` in `src/parity/mod.rs:73`, and fixture directives `// dbg: break 12`, `// dbg: step`, `// dbg-expect: …` in `src/parity/fixture.rs`. Normalized transcripts must match: top frame and locals exactly, the full stack modulo tail-call elision. |
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
