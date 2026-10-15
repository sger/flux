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
- On macOS no `dsymutil` step runs after `run_linker` (`src/llvm/pipeline.rs`).

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
- VM: emit breakpoint sites, record local-variable tables.
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
- `"backend": "native"` (S6) builds with `--native -g` and starts `lldb-dap`
  underneath `flux-dap`. You see the same Flux views, and VS Code
  configuration is identical. As built in S6b, `flux-dap` passes DAP
  through to `lldb-dap` and rewrites what makes the views Flux's: the stack
  keeps Flux frames, the variables the Locals scope with `flux_lldb.py`'s
  values (unbound ones left out), source paths are resolved through
  symlinks for lldb and returned as the editor wrote them, and Continue
  first drops a step a breakpoint interrupted, which lldb would otherwise
  finish. Step Back is not offered: a native run is not recorded.
  As built in S9b, `"record"` and `"replay"` work natively too (the program
  writes and checks its tape itself), and a **Flux Timeline** view lists the
  tape's effects on both backends: clicking one goes there (on the VM,
  earlier or later; natively, forward).

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
  needed: Core never inlines one function into another (S9c found every
  substitution stays inside one `CoreDef`), so a function's code comes from
  its own module's file.
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
  As built in S9c, a `-g` build runs every pass again, under one rule
  (`src/core/passes/keep.rs`): a binder the program wrote — named, not `__…`,
  bound at a place in the source — is never substituted away. The let
  inliners and `elim_dead_let` leave such a `let`; `case_of_known_constructor`
  and `constant_fold` bind such a pattern variable with a `let`;
  `specialize_known_shapes` keeps such a `let f = fn…`; `beta_reduce` leaves a
  lambda with such a parameter applied, a call as on the VM. Compiler-made
  binders inline and go as in a plain build. A lambda that is still inlined
  (one without parameters the program named) is recorded as an `InlineSite`
  and described with `inlinedAt` (§2.4).

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
  needs span provenance. As built in S9c: `beta_reduce` records each lambda
  it inlines (`InlineSite { lambda, call }`), `AetherProgram.inline_sites`
  carries them to LIR, where a location inside one gets `LirLoc.inlined` (an
  index into `LirFnDebug.inline_sites`, with the call's own location, itself
  perhaps inlined). `emit_debug.rs` then emits a `distinct !DISubprogram` per
  inlined lambda and `!DILocation(…, scope: !lambda, inlinedAt: !call)`; its
  variables take the lambda's scope, and their `llvm.dbg.declare` a location
  inlined the same way. lldb shows the lambda as an inlined frame, which
  `flux bt` marks `[inlined]`.

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
- **Windows (MSVC):** CodeView, linked into `<exe>.pdb` (S11, §2.8).
- **Linux:** DWARF stays in the binary.
- **Runtime:** already built with `-O2 -g` (`ensure_runtime_lib` in
  `src/llvm/pipeline.rs`).
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
changes. As built in S11:

- **Metadata format:** `NativeDebug.codeview` (`src/lir/emit_debug.rs`), set on
  MSVC targets, emits the module flag `!{i32 2, !"CodeView", i32 1}` instead of
  `"Dwarf Version"`. LLVM then writes CodeView `.debug$S` / `.debug$T` sections
  from the same `DICompileUnit`, `DISubprogram` and `DILocation` nodes, and
  from the S6 variables, which show as raw words. The `FluxCtor` types and
  their holder global are skipped: only `flux_lldb.py` reads them, and the
  global exists only for `dsymutil`. The source path loses the `\\?\` prefix
  of a canonical Windows path, which debuggers do not resolve. The native
  cache tag became `native-cache-abi-v9:codeview`, so `-g` objects cached
  before S11 (built without metadata) are rebuilt.
- **Linking:** `link_objects` takes the debug level. Under `-g` both Windows
  branches of `run_linker` pass `/DEBUG /OPT:REF /OPT:ICF /PDB:<exe>.pdb`:
  the `lld-link` / `link.exe` branch used with `cl`, and the clang-driver
  branch (`-Wl,…`), which is the usual one, since `detect_c_toolchain` prefers
  clang. `/DEBUG` alone turns off `/OPT:REF` and kept the whole Rust staticlib
  (1.5 MB → 18.8 MB). A cached run reuses its binary only if the PDB is there
  too, and `--no-cache` removes both `program.exe` and its PDB.
- **Runtime:** clang's `-g` already emits CodeView on the MSVC triple, so the
  C runtime needed no new flag; `cl` gets `/Z7`. The compile flags are kept
  in `flux_rt.lib.flags`, and a change to them rebuilds the runtime. The Rust
  staticlib carries CodeView from its own debug profile.
- **Names:** use the `DISubprogram` `name` (`Flow.List.map`,
  `<lambda app.flx:12:5>`). Visual Studio shows `name`, not the `flux_*$w`
  linkage symbol. Artificial wrappers keep `DIFlagArtificial`; Visual Studio
  shows them, but with line 0 and no source.
- **lldb on Windows:** lldb reads the PDB, and `flux_lldb.py` loads, but it
  names frames by their symbols (`flux_calls_square`), not by the
  `DISubprogram` names. So `vm_debug`/`llvm_debug` debug parity reports a
  skip on Windows, as `native_debug_tests` and `parity_debug_tests`, which
  are Unix-only, already did.
- **Explicitly not included:** `.natvis` value formatting, a Visual Studio
  extension to host `flux-dap`, VM debugging, recording, and lldb debug
  parity on Windows.

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
  raises under `-g` and is optimized away in a plain run. Native `-g` keeps
  that binding too (S9c), as the VM debugger does; otherwise a native `-g`
  build runs the passes a plain build does, and `xtask verify` checks that
  every guide example prints the same natively with and without `-g`
  (the `llvm_g` parity way).

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
`next`, `finish`, `bt`, `locals`, `print`, `internals on|off`, `quit`, the
S3 reverse commands, and `handlers` and `catch` (S7). It reads commands from a terminal as typed, or from
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

`OpPerform` (`src/vm/dispatch.rs`) captures the performing frames into a
`Continuation` (`capture_to_boundary`, `src/vm/function_call.rs`). The
handler arm then runs with those frames *off* `VM.frames`. While a non-tail
`resume` runs the continuation again (`execute_resume`), the arm that called
it is off the stack instead, kept in a Rust local until the continuation
returns.

As built in S7a (`src/vm/debug/effects.rs`), the debugger notes both, only
while it is attached:

- **Suspensions.** A perform on the general path, or on the path of a
  handler that never resumes, records the frames it takes off the stack:
  name and place, and whether they can be resumed. The record attaches to
  the arm's frame when it is pushed, and lasts while that call does.
- **Resume notes.** A non-tail `resume` keeps the frames it covers, the arm
  among them, with their variables and what the debugger knew of them (last
  line, call serial, tail calls), and puts that back when the continuation
  returns.
- **Call serials.** The continuation keeps the serials of its frames
  (`Continuation.debug_serials`, empty without a debugger), so a resumed
  frame is the same call it was before the perform.

`bt` then shows the logical stack:

```text
#0 <handler Flip.flip> at app.flx:20:25
   ⟂ handling Flip.flip — performed at app.flx:9:13
#1 decide at app.flx:9:13   (suspended)
#2 twice at app.flx:14:13   (suspended)
#3 main at app.flx:19:18
```

While `resume` runs them again, the resumed frames sit above the arm, which
shows at its `resume` call. A handler that never resumes marks its frames
`(suspended, not resumable)`. A tail-resumptive handler leaves the
performing frames on the stack, so its arm simply sits above them. A
suspended frame's variables are read from the continuation the arm holds as
`resume`.

**Stepping** measures depth on the logical stack: suspended frames count
below the arm, and resumed frames above the arm that resumed them. So:
- stepping over a perform runs the handler and stops at the next line of the
  same call, after `resume` brings it back;
- stepping over `resume(x)` runs the continuation and stops back in the arm;
- stepping out of resumed code returns into the arm;
- stepping into `resume` stops at once, at the `perform` the continuation
  resumes from.

Shots of a multi-shot continuation are told apart by the arm's position in
the stack (the column of the `resume` running), not by a shot number: the VM
does not count them.

**Handlers.** `handlers` in `flux debug`, the `Handlers` scope in VS Code
and `.handlers` in its Debug Console list the active handlers, innermost
first, with a parameterized handler's state and the function the `handle` is
in. The default handlers around `main` are internals.

**Exceptions.** A runtime error the debugger catches stops where it
happened, before the error unwinds anything, with reason `exception`:
`stopped: exception (panic: too big) at app.flx:5:9 in check`. Going on ends
the program with the error, as without a debugger. `catch` chooses the kinds
(panic, division by zero and unhandled effect by default; other runtime
errors on request); VS Code shows them as exception breakpoint filters, with
`exceptionInfo`. A call to an intrinsic such as `panic` shows as the
function that made it, when a tail call replaced that function's frame.

#### 3.5 Fibers as threads

As built in S8a (`src/vm/fiber_debug.rs`, `src/vm/debug/threads.rs`):

- **Threads.** Thread 1 is the program: outside `run_async`, or its root
  fiber inside it. Each fiber the program spawns is the next number, in
  spawn order, which with one worker is the program's own order on both
  backends. A task (`Task.spawn`) is thread `1000 + id`, named
  `task N (function)`.
- **Records.** While a debugger is attached, the scheduler records each
  fiber's function, its number, and what it parks for: `sleep N ms`,
  `channel #c recv|send`, `task #t`, `event #e`, `both|race|first_of
  (threads 2, 3)`, `try`, `timeout`, `yield`, file system or TCP I/O. Without
  a debugger, recording costs one thread-local flag read per spawn and park.
- **One worker.** Under a debugger, `run_async` runs every fiber on the one
  VM, as under a tape, and says so once when more workers were asked for.
  A task runs on a thread of its own, which the debugger lists with its
  state (`running (not paused)` or `done`) and no stack: breakpoints in a
  task's code do not stop.
- **Stacks.** The fiber that stopped is the VM's stack, cut where the fiber
  starts: a child fiber's stack ends at its function, not at `run_async`'s
  caller; the root fiber's goes on down to `main`. A parked fiber's frames
  are in its continuation, read as a perform's are below its arm (S7), with
  their variables, and marked `(suspended)`; the primop it parked in has no
  source, so it is left out. A fiber that has not started shows its
  function.
- **Commands.** `threads` lists them (`* 3 fast [stopped] at app.flx:11:5`,
  `  2 slow [waiting sleep 20 ms] at app.flx:4:5`); `thread N` makes `bt`,
  `locals` and `print` read thread N until the next stop. A stop in a fiber
  says which (`(thread 3, position 19)`). The protocol has `Threads`, a
  `thread` on `Stack`, `Variables` and `Evaluate`, and the thread on
  `Stopped`; VS Code shows each fiber as a thread, named with what it waits
  for.
- **Stepping** stays in the fiber it started in: `next` over `sleep` or
  `recv` lets the other fibers run and stops at the next line of the same
  one. A parked fiber keeps the serials of its calls in its continuation,
  so it does not stop again on the line it parked at. Once the fiber has
  finished, the step stops anywhere.
- **Fiber failure.** `Async.fail` stops as a `fiber failure` exception
  (`catch fibers`, on by default), where it is raised. A panic or another
  runtime error in a fiber stops as itself, in that fiber, before
  `Async.try`, `race` or `timeout` would recover it. A cancelled fiber
  unwinding, a fiber parking, and `run_async` failing again with its root
  fiber's error are not stops.
- **Pause** lands while every fiber waits: the scheduler hears the front
  end while it waits for a completion, and stops at the `run_async` call.

Natively, as built in S8b (`src/runtime/async/native_abi.rs`,
`tools/lldb/flux_lldb.py`):

- **Records.** Fibers are not OS threads: a waiting fiber is a continuation
  closure the scheduler holds. `NativeRun` keeps what each request parks
  for (the runtime's C side notes channels, tasks, events and TCP through
  `flux_async_debug_block`), each fiber's function, and the OS thread a
  running fiber is on. A fiber's thread number is its id from the root's,
  plus one: spawn order, as on the VM.
- **`flux_debug_fibers()`** writes them as JSON into a static buffer. lldb
  calls it by address in the stopped program; it takes only locks it can
  get at once, and answers `busy` otherwise.
- **lldb.** `flux threads [--json]`, `flux thread N` and `flux bt --thread
  N`: the stopped fiber's stack is cut at the scheduler's `execute_fiber`;
  a waiting fiber's frames are its continuation's, without variables; a
  standard-library frame at the top of a waiting fiber is left out. `flux
  where` adds `(thread N)`. `flux next`, `step` and `finish` stay in their
  fiber: a breakpoint hit in another fiber is passed over, and a parked
  fiber is followed into its continuation.
- **Fiber failure.** `flux_fiber_fail` calls `flux_debug_on_fiber_failure`
  first, while the failing fiber's frames are intact (`flux catch fibers`).
  `run_async`'s re-raise of a failed root, and `run_async_with`'s (which now
  checks, as the VM does), use `flux_panic_unhooked`, so they do not stop
  again.
- **DWARF.** A lambda's continuation is named after the lambda, and the line
  a continuation resumes on has no location in it, so a breakpoint there
  resolves only where the line runs first.
- **Proxy.** lldb-dap's threads are replaced by `flux threads --json`: the
  stopped fiber keeps its OS thread id, a waiting one is `5_000_000 + N`
  with frames from `flux bt --json --thread N`. Sessions run with
  `FLUX_WORKERS=1`.
- **Builds.** `dsymutil` takes one of two machine-wide lock slots
  (`FLUX_DSYMUTIL_JOBS`): many `-g` builds at once (a test suite, the
  parity harness) each ran one at about 700 MB, enough to exhaust memory.

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
- **Native without the driver (S9b):** the driver links into every native
  program an object defining `flux_program_hash`, the hash above. The
  program's `flux_tape_init(hash, argc, argv)` writes a new tape's header
  itself and runs the same checks on a tape it replays
  (`LoadedTape::check_for`), so a program started by lldb or an editor
  records and replays as under `flux`. A tape error is printed rendered,
  then as the wire line; the driver sets `FLUX_TAPE_WIRE` for its child to
  get the line alone.
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

  Channels between fibers need nothing: they complete on the running
  fiber's thread.

  **Tasks (S8c).** A task (`Task.spawn`) runs on an OS thread of its own,
  so its thread and the program's interleave differently on every run. The
  tape orders what crosses threads, and a replay runs the threads in the
  recorded order:
  - **Streams.** Each event says which task's thread performed it (`task`,
    absent for the main program). Tasks are numbered from 1 in the order
    they are spawned; channels from 1 in the order the tape first uses them.
  - **The turnstile.** While replaying, a thread whose next event is not the
    tape's next event waits for its turn. Console text prints at its turn,
    so replayed output comes out in the recorded order. A thread that waits
    30 seconds for an event that never comes stops the replay with E1022,
    naming the thread it waited for.
  - **Crossings** are ordered events that a replay does again, at their
    turn, checking they come out the same: `task_spawn`, `task_start`
    (whether the task ran or was cancelled first), `task_end`,
    `task_cancel`, `task_join`, `task_await` (natively, whether the task had
    already finished). Once the program has spawned a task, channel
    operations (`send`, `recv`, `try_send`, `try_recv`, `close`, `len`,
    `is_closed`) are `channel` events, with what they found.
  - **Hand-overs.** A completion another thread hands to the scheduler (a
    task's result for `Task.await`, a value a channel passes to a parked
    fiber) is a `wake{waiter}` event where the scheduler took it, naming
    the fiber that waited for it (S9d; `wake{req}` before). A replay
    delivers the real completion there.
  - **Unchanged:** tasks keep their own threads and the VM's pool keeps its
    size. Tasks cannot perform I/O (`spawn` takes `() -> a`), so their own
    events are their start, end and channel operations. A task the program
    leaves running goes on past the end of the tape, unchecked, as it did
    unrecorded.
  - **Refused:** `run_async` inside a task (E1027). Its scheduler would
    interleave with the program's in ways the tape does not order, and
    natively it would share the process's one active run.

  **One schedule (S9d).** The VM's scheduler follows native's rules, so
  an async tape replays on either backend. The order of completions,
  hand-overs and thread turns on the tape is then the whole schedule, and
  no event records which fiber runs next:
  - A channel or event completion on the run's own thread wakes the
    waiting fiber at once, at the back of the ready queue (the fiber that
    made the operation last, when it parks). Only another thread's
    completion is a hand-over.
  - `Task.await` is a `task_await` turn on both backends. A task that had
    ended by then gives its result at once; otherwise its end is handed
    over. A task cancelled before the await gives no result, even one that
    had already ended, and a task cancelled before it started ends
    `failed: false`.
  - A fiber that loses a race, or that a timeout cuts off, is dropped and
    never runs again (the VM used to resume it with `()`), and a timeout
    whose body wins cancels its timer.
  - The run ends with its root: fibers still ready then never run.
  - The names on the tape do not depend on either backend's id counters.
    A request belongs to the fiber that made it, so its submission is that
    fiber's even when the scheduler sends it (a connect after a DNS
    lookup). A request is numbered when it is first submitted. A fiber is
    numbered when it first appears or makes a request.

#### 4.3 Tape format

A tape is JSON Lines (format version 3 since S9d; a version 1 or 2 tape is
refused with E1026): one header line, then one event per line in the order
the program performed them.

```text
{"magic":"FLXTAPE","version":3,"flux_version":"0.0.7","program_hash":"sha256:…","argv":["app.flx"],"backend":"vm"}
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
  otherwise fibers numbered from 1 in the order they first appear or make
  a request. A submission is the fiber's whose request it is. `task`
  (omitted when 0, S8c) is the logical task whose thread performed it.
- **Async events (S1b):**
  - Submissions: `timer_start`, `dns_resolve`, `tcp_connect`,
    `tcp_listen`, `tcp_accept`, `tcp_read`, `tcp_write` and `tcp_close`.
    Each carries a logical `req`, numbered from 1 in the order requests
    are first submitted.
  - `async_complete{req}`, with the result, placed where the scheduler
    received it.
  - `event_clock`, which records each reading of the clock that
    `Flow.Event` deadlines use.
  - Threads (S8c): `task_spawn`, `task_start` (result: whether it ran),
    `task_end`, `task_cancel`, `task_join`, `task_await` (result: whether
    the task had finished), `wake{waiter}` (a completion handed over by
    another thread, to fiber `waiter`) and `channel{chan, op}` (result:
    what it found).

  ```text
  {"seq":0,"fiber":1,"kind":"task_spawn","id":1}
  {"seq":1,"task":1,"kind":"task_start","id":1,"result":{"bool":true}}
  {"seq":2,"fiber":1,"kind":"task_cancel","id":1}
  {"seq":3,"task":1,"kind":"task_end","id":1,"failed":false}
  {"seq":4,"kind":"wake","waiter":1}
  ```

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
- **Backend-neutral:** a tape replays on either backend, whichever recorded
  it. Until S9d a tape with async events replayed only on the backend that
  recorded it (E1026), since the two schedulers ordered fibers differently;
  they now follow one schedule (§4.2).
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
- **Async code (S9a):** the session keeps recording through `run_async`.
  No checkpoint is taken inside async code (fibers run in nested dispatch
  loops), so going back to a position there restores the last checkpoint
  before the outermost `run_async` and runs forward again, the kept events
  answering every effect, completion and hand-over, as they do for synchronous
  code:
  - **The tape mark** keeps the tape's numbering of requests, fibers, tasks
    and channels, and the live tasks; a checkpoint keeps the next channel,
    task and event ids. A re-run numbers everything as the first run did, so
    values like `Channel(1)` read the same.
  - **The abandoned run** unwinds with the debugger's own error, which the
    scheduler passes on rather than turning into a fiber's failure, while the
    tape is paused. Its scheduler state, queues and tasks are dropped. A task
    thread still running finishes without touching the tape (each going back
    starts a new generation), and completions of its requests are dropped.
  - **Stepping back stays in its fiber,** as stepping forward does:
    `reverse-step`, `reverse-next` and `reverse-finish` go to the last place
    in the same fiber, or, from a fiber's first line, to the last place in
    any. `reverse-continue` goes back to a breakpoint any fiber hit.
  - **Running live again.** Where a re-run reaches the end of what was kept,
    it records again. Work the re-run only answered and still waits for
    starts for real: a timer starts over; a socket request in flight cannot,
    and stops the run.
  - **Tasks** run again on the re-run, in the recorded order (§4.2). No
    checkpoint is taken while a task is unjoined, since restoring one would
    not bring the task back.
  - **Limits:** state outside the VM that a re-run does not rebuild, such as
    a channel made and used before `run_async`, is restored only as far as
    the run makes it again; a re-run that reaches a different place says the
    run is not deterministic there.
- **OS-thread tasks:** going back across a task works the same way (S9a); a
  `--record` or `--replay` run records and replays tasks (S8c, §4.2).

#### 4.5 What native supports

- Record, and deterministic replay under lldb.
- **Run to event N:** restart the replay and stop when the tape cursor reaches
  N. This needs no instruction counting.
  As built in S9b: each event, recorded or replayed, calls
  `flux_debug_on_tape_event(seq)`, an empty function. `flux event N` in
  `flux_lldb.py` breaks there for N, past other breakpoints, and selects
  the frame that performed the effect, past the default handler arms
  `with IO` installs. An event already passed in a replay is reached by
  launching again; in a recording it is refused. `flux events` lists the
  tape through `flux_debug_tape_events(from)` (JSON, paged, a static
  buffer). Under VS Code, lldb-dap owns the process (Xcode's has no
  `restart`), so the Timeline goes forward only.
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
| Exception breakpoints: panic, division by zero, unhandled effect, fiber failure (`Async.fail`), and other runtime errors on the VM (VM error paths; native symbolic breakpoints on runtime debug hooks). A failed match has no runtime error on either backend (exhaustiveness is checked when compiling), so there is nothing to stop at | ✓ | ✓ | S7 / S8 |
| Stacks that follow continuations; step into `resume` lands at the `perform` site (native: `flux_lldb.py` follows the arm's `resume` to its `$cont$` closures, and lands at the line after the perform) | ✓ | ✓ | S7 |
| Handler stack view (VM `handler_stack`; native evidence vector, read as the runtime's thread-local) | ✓ | ✓ | S7 |
| Fibers, tasks and channels shown as threads with what they are blocked on (native: `flux_debug_fibers()` called through lldb) | ✓ | ✓ | S8 |
| Effect timeline: tape events, click one to travel there | ✓ | run-to-event | S3 / S9 |
| Watch expressions and hovers: a variable or a part of it (`pair.0`) in S4; any expression later, allowed only when its inferred effect row is empty, so evaluating it can't cause side effects | ✓ | formatter-level only | S4 / later |
| Aether view: reference counts, "unique, so reuse is possible", and a debug-heap runtime (`libflux_rt_debug.a`) that poisons freed memory and records allocation sites | ✓ | ✓ | S10 |
| Panic backtraces without a debugger (VM today; native: unwind plus symbolize through `gimli` in the staticlib, then delete the unused `flux_trace_push`) | ✓ | ✓ | S10 |

Native effects need special handling in two cases:
- Tail-resumptive operations go through `flux_perform_direct`
  (`runtime/c/effects.c`), or, performed in the function that handles them,
  a direct call of the arm. The performing frames are still physically on
  the stack, so DWARF shows them with no extra work.
- The general path is `flux_yield_to`. There the performing frames have
  already returned, and only their `$cont$` closures describe what will run
  next.

As built in S7b:
- **The walk through `resume`.** The arm's first parameter, `resume`, is a
  debug variable. `flux_lldb.py` follows it to the continuations: one
  `$cont$` closure, or the compose trampoline's array of them, innermost
  first. It names each by the function its code pointer resolves to,
  `f (resumed after L:C)`. While `resume` runs them, the ones after the
  running one are still in that array, and the walk shows them below it.
  No `flux_debug_yield_chain()` call into the program is needed.
- **Handlers.** `flux handlers` reads the thread's evidence vector,
  `flux_thread_ctx.current_evv`. The runtime is built with debug
  information, so lldb reads the thread-local itself, without running
  code. Each entry is named by its clause closure's function,
  `<handler Effect.op>`, so the DWARF needs no table of effect names.
- **Runtime errors.** `flux_panic` calls `flux_debug_on_panic(msg)` first,
  and unhandled effects go through `flux_unhandled_effect`, which calls
  `flux_debug_on_unhandled_effect(htag)`. Both hooks are empty, never
  inlined and never removed. `flux catch` breaks on them. The breakpoint's
  callback tells panics from division by zero and the runtime's other
  errors by the frames that called `flux_panic`, goes on past the kinds
  not asked for, and selects the Flux frame of the error.
- **Stepping.** `flux next`, `flux step` and `flux finish` step as the VM
  does:
  - when a step leaves its frame while the runtime is yielding, they break
    at that frame's continuation (and, stepping in, at the handler's
    clause);
  - stepping into `resume` breaks where its continuations start;
  - returning into a caller does not stop again on the line of its call.

  A continuation's `scopeLine` is 0, so its prologue does not repeat the
  perform's line. A lambda's closure is made at the line of the code around
  it, so a breakpoint on a lambda's or arm's first line stops only inside
  it.
- **Known differences from the VM, which the debug parity check allows:**
  - an arm whose last call is `resume` stays on the native stack under
    the frames it resumes;
  - native code cannot tell a handler that never resumes, so its frames
    show as `(suspended)`;
  - suspended frames have no variables natively;
  - stepping into `resume` stops at the first line after the perform,
    not on the perform itself.

### §6 Staging

| stage | change | exit |
|---|---|---|
| S0 | `src/debuginfo/` model (functions, lambdas, variables with types, ADT layouts, effects), built per module where Core is lowered and kept on `Compiler`; `--debug` / `-g` and `DebugLevel`; debug level in the cache keys; `--dump-debug-info` | Snapshot tests of `--dump-debug-info` for ~8 fixtures, including a two-module program with each module in its own file. A test that cache keys differ by debug level while the default key is unchanged. `-g` leaves the LLVM IR and bytecode unchanged in S0. |
| S1a | Recording of synchronous effects (implements 0038): the tape format and runtime, VM hook, native C/Rust ABI, `--record`/`--replay`, E1022–E1027; async refused | Tapes recorded on either backend replay on both with identical output; replay reads and changes nothing outside the program; edited tapes fail with E1022–E1026; async under a tape fails with E1027. Off-mode overhead measured with `benches/effect_tape_off_bench.rs`. |
| S1b | Async recording: submissions and completions through the shared async backend, one worker and synchronous fs under a tape, the taped event clock, logical request and fiber ids; `Task.spawn` refused until S8 | Every async fixture (sleep, channels, events, scopes, fs, TCP) and every async example replays on the backend that recorded it, without waiting on timers or opening sockets; edited async tapes fail with E1022/E1023; an async tape on the other backend fails with E1026 (lifted in S9d). |
| S2 | VM engine: `OpBreakpoint` sites (entry, statement, match arm, `|>` stage, perform) from the AST compiler under `-g`, `VmObserver` and `Inspector` (`src/vm/debug/`), step in/over/out, local-variable tables, caller-location fix (also in runtime stack traces), tail-call elision counter, compiler-made code hidden, `flux debug` front end | Golden transcripts of scripted sessions (`tests/debug/vm/*.flxdbg`). Every example runs the same with and without `-g`, apart from timing and one unused failing binding. No slowdown in non-debug runs. |
| S3 | VM reverse execution for programs without async code: positions (site counts), checkpoints at top-level sites in a bounded ring, reverse step/next/finish/continue by restoring a checkpoint and replaying forward, `goto`, run to event, every `flux debug` session recording in memory, console output printed once | Golden transcripts where coming back to a stop shows the stack and values seen going forward, the same wherever checkpoints fall. A memory-bound test (900,000 sites, at most 256 checkpoints). Going back performs no effect twice. |
| S4 | `crates/flux-dap`, mirroring `crates/flux-lsp`: DAP over stdio, the program in a `flux debug --engine` child driven over a JSON-lines socket protocol; breakpoints, stack (tail-call labels), scopes and variables, stepping, Step Back and Reverse Continue, pause; watch, hover and Debug Console evaluate of variable paths, plus console commands for positions and effects; `editors/vscode` `debuggers` and `breakpoints` contributions; launch config with `backend`, `record`, `replay`. Evaluating any pure expression moves to a later stage. | DAP JSON transcript tests in `crates/flux-dap/tests`. A manual VS Code checklist in the crate README. |
| S5 | Native line tables: LIR locations from Aether spans (reference counting at line 0), the IR metadata side table, one compile unit and file per module, subprograms per symbol kind, locations, real `source_filename`, frame pointers under `-g`, `dsymutil` for kept binaries; the opt level unchanged (O0 honours tail calls); Windows waits for S11 | `llvm-dwarfdump --verify` clean on the native debug fixtures (`tests/native_llvm/debug/`). `lldb -b` sessions break at file:line, show Flux frames and step (`tests/native_llvm/native_debug_tests.rs`). 10⁶-deep tail recursion runs in a `-g` build. Every guide example prints the same natively with and without `-g`. A manual Xcode checklist. |
| S6a | Native variables and types: the `LirFnDebug.vars` side table, a debug slot per variable with `llvm.dbg.declare`, unbound and dropped markers, `FluxValue`/`FluxCaptured`/`FluxInt`/`FluxBool`/`FluxWord` typedefs, the `FluxCtor` enumeration and `FluxCtor.<Name>` structures in the DWARF (instead of a `.fluxdbg` sidecar), `flux_lldb.py` (Flux values, children, `flux bt`/`locals`/`print`) embedded in the dSYM and printed by `flux debug --lldb-script`, Core `let` inlining off under `-g` | `tests/native_llvm/native_debug_tests.rs`: lldb with the script shows each kind of value as the VM does, variables only once bound, captured variables, the matched arm's binders, constructors another module declares; the dSYM carries the script. `llvm-dwarfdump --verify` clean; 10⁶-deep tail recursion still runs under `-g`; every guide example prints the same natively with and without `-g`. |
| S6b | The native engine in `flux-dap` (`crates/flux-dap/src/native.rs`: building with `--native -g --emit-binary`, finding and proxying `lldb-dap`), `"backend": "native"`, `lldbDap` and `flux.lldbDapPath` in VS Code; the debug parity harness | **Debug parity harness:** `Way::VmDebug` / `Way::LlvmDebug` (`src/parity/debug.rs`), fixture directives `// dbg: break 12`, `// dbg: step`, … in `tests/parity_debug/`. Stops and frames match line for line; every variable the VM shows is there natively with its value (native code may still hold a value the VM has moved out). Run by `parity_debug_tests` and `parity-check tests/parity_debug --ways vm_debug,llvm_debug`. DAP tests drive a native session through `lldb-dap`. |
| S7a | VM effects: the logical stack (suspensions, resume notes, call serials in continuations; `src/vm/debug/effects.rs`), stepping by logical depth, step into `resume`, `handlers`, exception stops with `catch` filters (panic, division by zero, unhandled effect, other runtime errors), the protocol's `exception` stops, frame kinds and `Handlers` scope; flux-dap exception breakpoint filters, `exceptionInfo`, label and suspended frames, `.handlers` | Golden transcripts `tests/debug/vm/effect_*.flxdbg` and `exception_*.flxdbg`; DAP transcript tests for the Handlers scope, suspended frames and an exception stop. |
| S7b | Native effects: runtime debug hooks (`flux_debug_on_panic`, `flux_debug_on_unhandled_effect`), `<handler Effect.op>` names and `resume` as a debug variable, `flux_lldb.py` following `resume` closures and the compose trampoline, `flux handlers`/`catch`/`where`/`next`/`step`/`finish`; the lldb-dap proxy's exception filters, `exceptionInfo`, spliced suspended frames and Flux stepping | Debug parity fixtures for multi-shot, nested handlers with state, a handler that never resumes, tail-resumptive vs yielding operations, stepping across perform and `resume`, panics and division by zero (`tests/parity_debug/effect_*`, `exception_*`). lldb sessions in `native_debug_tests` (ten frames past the runtime's eight continuation slots, `flux catch`); native DAP tests for the spliced stack and an exception stop. |
| S8a | VM fibers as threads (`src/vm/fiber_debug.rs`, `src/vm/debug/threads.rs`): names, numbers and what each parked fiber waits for, per-thread stacks (a parked fiber's from its continuation), `threads` / `thread N`, stepping that stays in one fiber, fiber failure as an exception, pause while every fiber waits, one worker while debugging; the protocol's `Threads` and per-thread requests; fibers as threads in VS Code | Golden transcripts `tests/debug/vm/async_*.flxdbg` (both, a channel, stepping over a sleep, `Async.fail`, a panic in a fiber, a task); DAP tests for fiber threads and their stacks, and a pause while every fiber waits. |
| S8b | Native fibers: the runtime's fiber records and `flux_debug_fibers()`, a fiber failure hook, `flux threads` / `thread` / `catch fibers` and fiber-aware stepping in lldb, fibers as threads in the lldb-dap proxy; `dsymutil` limited to two at a time per machine | Debug parity fixtures `tests/parity_debug/async_*` (`both`, `race`, `timeout`, a channel, a failing fiber, stepping over a sleep), with `threads` / `thread N`. lldb sessions in `native_debug_tests` (`flux threads`, a waiting fiber's stack, `flux catch fibers`, a failed root stops once); a native DAP test for fiber threads and their stacks. |
| S8c | Record and replay with tasks (§4.2): tape format version 2 with a `task` on each event, the turnstile that runs threads in the recorded order (console text at its turn), task spawns, starts, ends, cancels, joins and awaits as ordered events, completions handed over between threads as `wake` events, channel operations ordered once a task exists, the native tape protocol per thread; E1027 narrowed to `run_async` inside a task | `tests/replay/fixtures/task_*` (join, await, cancel racing the task, a channel shared with fibers) record and replay on both backends; 100 replays of `task_cancel` and `task_channel` print the same on each backend; an edited task tape stops with E1022; `run_async` inside a task fails with E1027. |
| S9a | Going back through async code and tasks on the VM (§4.4): recording through `run_async`, re-running from the checkpoint before it with the kept events (tape marks keep the tape's numbering, checkpoints the next channel/task/event ids), the abandoned run unwinding with its tape paused and its tasks forgotten (tape generations, stale completions dropped), stepping back that stays in its fiber, running live again where a re-run ends (timers start over) | Transcripts `reverse_async`, `reverse_task`, `reverse_both`, `reverse_sleep`, `reverse_channel`, `reverse_async_replayed` in `tests/debug/vm/`; DAP `stepping_back_into_async_code`. |
| S9b | Native replay under lldb: the build embeds `flux_program_hash`, so a native program writes its tape header and checks a replayed tape itself, with rendered errors (§4.2); run to event (`flux_debug_on_tape_event`, `flux events` / `flux event N` in lldb, relaunching a replay to go back); `fluxEvents` / `fluxGoToEvent` in `flux-dap` for both backends and a Flux Timeline view in VS Code; `.events` / `.event N` in native sessions (forward) | `replay_cli_tests`: a binary run directly records a tape the driver replays on both backends, and refuses another program's tape with E1026; lldb sessions in `native_debug_tests` (to an event and back in a replay, forward only in a recording, no tape); DAP `the_timeline_lists_the_effects_and_goes_to_one` and `native_record_and_replay_with_the_timeline` (native record, replay, Timeline, the tape on the VM with Step Back). |
| S9c | Every Core pass back on under `-g`, keeping the binders the program wrote (`keep.rs`: lets, pattern variables and lambda parameters are bound, not substituted; a lambda with a named parameter stays a call); `InlineSite`s from `beta_reduce` through `AetherProgram` to `LirLoc.inlined`, `inlinedAt` and inlined-lambda subprograms in DWARF, `[inlined]` frames in `flux bt`; the `llvm_g` parity way and the guide checked with and without `-g` in `xtask verify` | Core unit tests (kept lets, pattern variables, parameters; inline sites recorded). `native_debug_tests`: the IR has `inlinedAt`, golden `flux bt` with an `[inlined]` frame, `llvm-dwarfdump --verify` over `inlined.flx`. Debug parity 21/21 with `inlined_lambda.flx`. `parity-check examples/guide --ways llvm,llvm_g` 59/59. |
| S9d | Backend-neutral async tapes (§4.2 "One schedule"): the VM's scheduler follows native's (channel and event completions wake at once on the run's thread, `Task.await` as an ordered turn, cancelled fibers dropped, a winning timeout body cancels its timer, the run ends with its root, a task cancelled before it started does not fail); names that do not depend on the backend (a request belongs to the fiber that made it, `req` numbered at its first submission, `wake{waiter}`), tape format version 3; E1026 for async tapes lifted | `replay_cli_tests`: every async and task fixture (and `async_race_loser`) recorded on one backend replays on the other; 100 replays of `task_cancel` and `task_channel` recorded on each backend print the same on the other; unit test `async_names_do_not_depend_on_the_backend`; version 2 tapes refused. |
| S10 | Aether reference-count view, debug-heap runtime, native panic symbolization; `flux_trace_push` deleted | Golden reference counts. A use-after-free fixture that the debug heap catches. Native panic output lists Flux frames. |
| S11 | Windows call stacks for Visual Studio (§2.8): `"CodeView"` module flag on MSVC targets, `/DEBUG /OPT:REF /OPT:ICF /PDB` in both Windows branches of `run_linker`, the PDB kept with cached binaries and removed with uncached ones, runtime rebuilt when its flags change | `native_codeview_tests`: `llvm-pdbutil dump -symbols` lists the Flux functions, `dump -l` their `.flx` lines and one file per module; cdb stopped at `flux_debug_on_panic` prints the Flux frames with `panic.flx` lines (skipped without cdb). A manual Visual Studio checklist (`docs/tooling/native_debugging_lldb_xcode.md`). |

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
- **Debug builds use different code generation from release builds** (O0 vs
  O2, the program's own bindings kept). A bug that only happens at O2 may not reproduce under
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
- **Core inlining in the debug profile:** resolved in S9c. Every pass runs
  under `-g`, keeping the binders the program wrote; Core inlines no function
  into another, and the lambdas it still inlines are described with
  `inlinedAt`.
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
