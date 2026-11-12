- Feature Name: `runtime_observer`
- Start Date: 2026-11-12
- Proposal PR:
- Flux Issue:
- Status: Draft
- Depends on: [0188](0188_flux_debugger.md) (debugger: `Inspector`, fiber records, engine protocol)
- Relates to: [0024](0024_runtime_instrumentation_and_value_tracer.md) (observer hooks, never built), [0174](0174_async_effect_concurrency.md) (fibers, tasks, channels), [0190](0190_verification_toolkit.md) (`--metrics`), [0129](0129_vm_improvements_ghc_inspired.md) (`--prof` cost centres), [0199](0199_handler_linearity_and_scoped_concurrency.md) (scopes)

# Proposal 0202: A runtime observer

## Summary
[summary]: #summary

A Flux program started with `--observe` answers questions about itself while
it runs. A separate terminal client, `flux-observer`, connects to it and shows
live views, in the spirit of Erlang's Observer:

- an **overview**: uptime, instructions per second, allocation rate, fibers,
  tasks and channels;
- **hot functions**, sampled, sorted like `top`;
- a **fiber, task and channel table**, Flux's nearest equivalent of
  Observer's process table, with what each fiber waits on.

The program serves owned snapshots and periodic streams as JSON lines over a
loopback socket. Nothing in the language changes, and a program run without
`--observe` pays one predictable branch per call.

The VM comes first. Most of the data is already reachable there: proposal
0188's debugger built the inspection code, and it only runs while the program
is stopped. The native backend follows later, for a subset of the views.

Verdict: on the **VM, moderate**. On **native, hard in general and moderate
for the subset in §6**. Some Observer features have no Flux counterpart: GC
pauses, supervision trees, ETS tables and remote nodes. §1 says so instead of
inventing analogues.

In this proposal, **[verified]** marks a claim read in the source at the cited
place on 2026-11-12, and **[inferred]** marks a reading of the code that was
not run or not traced end to end.

## Motivation
[motivation]: #motivation

### Nothing watches a program while it runs

Every runtime tool Flux has either stops the program or reports when it exits:

| Tool | When it reports | Backend |
|---|---|---|
| `flux debug`, `flux-dap` (0188) | only while the program is stopped | both |
| `--prof` (`OpEnterCC`, `src/vm/profiling.rs`) | at exit, on stderr | VM, single-program path only |
| `--metrics`, `--stats`, `--stats-json` | at exit | VM (`--metrics`), both (`--stats`) |
| `--leak-detector`, `FLUX_HEAP_STATS`, `--leak-check` | at exit | VM / native debug heap |
| `--trace` (`VM::trace_instruction`, `src/vm/trace.rs`) | every instruction, on **stdout**, mixed with the program's output | VM |
| `FLUX_FIBER_TRACE=1` (`src/runtime/async/fiber_trace.rs`) | text lines on stderr | both |

[verified] for each row, from `src/driver/flags.rs:43-68` and the files named.

These suit programs that run and finish. They are no help with the programs
that run long: a TCP server, a pipeline of fibers over channels, a task pool.
For those the questions are "where is the time going right now", "which fibers
are stuck, and on what", "is a channel backing up", "is the allocation rate
climbing". Today, answering any of them means stopping the program under the
debugger, or adding `print`s.

### Most of the data already exists

**VM** [verified]:
- Values are `Value` (`src/runtime/value.rs:246`), held in NaN-boxed slots
  (`src/runtime/nanbox.rs`). Heap values are `Rc`, and the heap is acyclic by
  construction (`src/runtime/mod.rs:3-7`). There is no garbage collector, so
  there are no collections to show.
- ADT values carry their constructor's **name** (`AdtValue`,
  `src/runtime/value.rs:12`; `OpMakeAdt`, `src/vm/dispatch.rs:987`). A heap
  census by constructor needs no side table.
- `VM.handler_stack: Vec<HandlerFrame>` (`src/vm/mod.rs:114`,
  `src/runtime/handler_frame.rs:9`) holds the installed handlers. A
  continuation captured by `OpPerform` (`src/vm/dispatch.rs:1329`,
  `capture_to_boundary` at `src/vm/function_call.rs:86`) is a value holding
  frames, stack and inner handlers (`Continuation`,
  `src/runtime/continuation.rs:18`).
- Fibers are parked continuations (`Fiber.parked`,
  `src/runtime/async/fiber.rs:97`). `FiberScheduler::fibers()`
  (`src/runtime/async/scheduler.rs:424`) lists every fiber with whether it is
  ready. The scheduler is a thread-local in `vm_fibers`
  (`src/vm/core_dispatch.rs:366`).
- Tasks (`TASKS`, `src/vm/task.rs:95`) and channels (`CHANNELS`,
  `src/vm/channel.rs:32`) are process-global registries behind a `Mutex`.
  `ChannelCell` (`src/vm/channel.rs:11`) holds the buffer and both waiter
  queues.
- `instructions_executed` goes up once per dispatched instruction
  (`src/vm/dispatch.rs:177`). It is the VM's unit of work, the equivalent of
  Erlang's reductions.
- The counting global allocator (`src/alloc_count.rs`), installed by the
  `flux` binary (`src/main.rs:2`), counts every allocation in a relaxed atomic
  that any thread can read.
- `FunctionDebugInfo` (`src/bytecode/debug_info.rs:97`) keeps each function's
  name and line table in every build. Local-variable tables exist only under
  `-g`.

**The debugger built the inspection code** [verified]:
- `Inspector` (`src/vm/debug/inspect.rs:108`) reads the logical stack
  (`stack`, `:264`), variables, `refs` and handlers (`handlers`, `:662`).
  `ValueView` (`src/vm/debug/values.rs`) renders a value with bounds.
- `src/vm/fiber_debug.rs` records each fiber's name and what it parks for, but
  only while a debugger is attached. Otherwise recording costs one
  thread-local flag read.
- `VmObserver::on_interrupt` (`src/vm/debug/mod.rs:190-204`), together with
  the thread-safe `request_interrupt()`, already gives the front end a callback
  without stopping the program. It is polled only at `-g` stop sites
  (`OpBreakpoint`, `src/vm/dispatch.rs:1780`) and in the scheduler's idle loop
  (`src/vm/core_dispatch.rs:1957`).
- `src/debug_protocol.rs` is a JSON-lines protocol over loopback TCP
  (`src/driver/debug_engine.rs`), and `crates/flux-dap` is a separate crate
  that speaks it.

**Native** [verified]:
- `flux_debug_fibers()` (`src/runtime/async/native_abi.rs:2025`) writes the
  scheduler's fibers as JSON. It uses only `try_lock` and writes to a static
  buffer, because it was built for lldb to call in a stopped program.
- The allocator (`runtime/c/rc.c:357-641`) has no counters in a plain build.
  The debug heap (`rc.c:161-349`) counts allocations, frees and live blocks.
- Every native build has a DWARF line table, used by the program's own panic
  backtrace (`src/runtime/backtrace_abi.rs`, the `backtrace` crate). There are
  no stack maps and no shadow stack. Frame pointers are kept only under full
  `-g`.

What is missing is small and specific:
1. a safe point that exists without `-g`;
2. a reader that does not need the debugger's `DebugControl`;
3. a listener that runs next to a live program;
4. aggregate counters;
5. a client that shows tables rather than a stack.

Proposal 0024 Part A described per-instruction and allocation hooks. They were
never built, and §2 explains why this proposal does not revive them.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### Observing a program

```text
$ flux run server.flx --observe
observing on 127.0.0.1:50213 (pid 41872)
listening on :8080
```

In another terminal:

```text
$ flux-observer
  pid    program      started   address
  41872  server.flx   12:03:10  127.0.0.1:50213
> 41872
```

`flux-observer` with no argument lists the programs that are being observed on
this machine. `flux-observer 41872` or `flux-observer 127.0.0.1:50213`
connects to one directly. To reach another machine, forward the port over SSH.
The listener only binds to loopback.

### Overview

```text
 server.flx  pid 41872  up 00:14:32  VM, 1 worker                 [o]verview
 ─────────────────────────────────────────────────────────────────────────────
 instructions   1.84 G total    12.6 M/s  ▁▂▂▃▅▇▆▅▃▂▂▃
 allocations    96.1 M total     820 k/s  ▁▁▂▂▃▃▃▄▃▂▂▂
 fibers         ready 3   waiting 212   done 4,511
 tasks          running 2   done 17
 channels       open 9   with waiters 2
 memory         rss 48 MB
```

### Hot functions

```text
 hot functions (sampled, last 10 s)                                 [h]ot
 self%  total%  samples  function                  module
  31.2    31.2    3,120  Json.parse_value          Flow.Json
  18.7    52.4    1,870  handle_request            Main
   9.9    11.0      990  Map.insert                Flow.Map
```

"Self" counts the samples where the function was running. "Total" also counts
the samples where it was further down the stack.

### Fibers, tasks and channels

```text
 fibers                                                             [f]ibers
  #    function           state      waiting on            for      stack
  1    main               waiting    task #3               14:30    6
  7    accept_loop        waiting    tcp accept            00:02    4
  12   handle_conn        ready      –                     –        9
  13   handle_conn        waiting    channel #4 recv       03:11    9   ⚠
 channels
  #   capacity  buffered  receivers waiting  senders waiting  closed
  4   16        0         1                  0                no
```

A ⚠ marks a fiber that has waited longer than a threshold. Pressing Enter on a
fiber opens its drill-down (stage S3): its logical stack, the handlers
installed in it, and its variables, read the way `flux debug` reads them.

### What the views mean in Flux

- **Instructions stand in for Erlang's reductions.** They exist only on the
  VM. Natively, hot functions come from CPU samples.
- **A channel stands in for a mailbox.** A fiber has no mailbox of its own.
- **There is no GC view,** because neither backend has a garbage collector.
  Memory is freed by reference counting as soon as it becomes unreachable.
- **There is no supervision tree,** because Flux has no supervisors. The
  nearest structure is the tree of structured-concurrency scopes (`both`,
  `race`, `scope`), which has no restart semantics. The observer may show it
  later (Future possibilities).
- **Observing does not change the program's output or its types.** It reads
  runtime state at safe points and never runs Flux code.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### §1 Observer concepts mapped onto Flux

The VM and native columns say whether the data can be had: **yes** (it exists
today), **work** (it needs new runtime code), or **no**.

| Observer feature | Flux equivalent | VM | Native | Notes |
|---|---|---|---|---|
| System overview (uptime, schedulers) | uptime, instructions/s, allocations/s, worker count, RSS | yes | work | `instructions_executed`, `alloc_count::allocations()`; native has no instruction count |
| Load charts | the same counters, streamed | yes | work | the client keeps the history |
| Memory breakdown by type | **heap census by value kind and constructor**, on demand | work | work (debug heap only) | walk from the roots, §3; native plain builds have no per-object record |
| GC pauses, collection counts | **none** | no | no | there is no GC; a large drop is a stackless walk (`drop_value_stackless`, `rc.c` `flux_drop_free_recx`), not a pause worth charting |
| Reductions per process | instructions per fiber | work | no | attribute `instructions_executed` deltas at fiber switches in `dispatch_loop` |
| Hot functions (`top`) | sampled functions, self and total | work | work | VM: sample at calls (§3); native: SIGPROF (§6) |
| Process table | **fiber / task / channel table** | yes | yes | `FiberScheduler::fibers()`, `TASKS`, `CHANNELS`; native `flux_debug_fibers()` |
| Mailbox length | channel buffer length and waiter counts | yes | work | `ChannelCell`; native `FluxChannel` table (`runtime/c/channel.c:66`) |
| Supervision tree | **none**; nearest: the scope tree | work | work | no restarts, so not a supervision tree; native `NativeRun.scopes` exists (`native_abi.rs:231`) |
| Process state and stack | a fiber's logical stack, handlers, variables | work | no (lldb only) | `Inspector` needs a read-only split (§3); natively the evidence vector is a thread-local of the running thread |
| Live handler stack | handlers installed in a fiber | work | no | snapshot only: it changes every few microseconds, so a live chart of it means little |
| Suspended continuations | parked fibers; continuations held by handler arms | yes (parked fibers) | yes (fibers) | a continuation held in a user variable is an ordinary value: it appears in the census |
| Effect traffic | performs per effect and operation | work | partial | VM: counters at `OpPerform*`; native: `flux_yield_to` / `flux_perform_direct` only, since a handler arm called directly bypasses the runtime |
| Effects at the program boundary | console, file system, network, clock | yes | yes | the tape hook (`src/vm/replay_hook.rs`) already sees each of them |
| Tracing calls (`ttb`, `dbg`) | tracepoints: log the call and its arguments, then go on | work | hard | VM: a check at the call safe point; native: dtrace or uprobes (Future possibilities) |
| Value and closure inspection | `ValueView` with constructor names | yes | no (lldb only) | closures print as `<closure>` today; S3 adds the function name |
| ETS tables | **none** | no | no | Flux has no mutable tables. Module-level `let`s are immutable; State lives in a handler's state, which the handler view shows |
| Remote nodes | **none**; TCP to one program | – | – | Flux has no distribution; an SSH tunnel covers the remote case |

### §2 Safe points

The VM is single-threaded per worker and its values are `Rc`, so it is
`!Send`. Another thread cannot read VM state. The observer therefore reads
state on the VM thread, at **safe points**.

**Calls and tail calls are enough.** Every jump goes forward:
`apply_ip_delta` (`src/vm/mod.rs:419`) takes an unsigned delta [verified], and
no opcode jumps back [inferred, from the opcode list in
`src/bytecode/op_code.rs`]. The only way code repeats is through a call, so a
poll on the two call paths is reached at least once per loop iteration. Those
paths are `call_closure_with_return_slot` (`src/vm/function_call.rs:229`, which
pushes through `push_frame`) and `tail_call_closure` (`:287`, which reuses the
frame). A long primop (a sort, a big string build) delays the poll until it
returns. That delay is bounded, and it is acceptable for snapshots.

**The scheduler's idle loop** is the second safe point. When every fiber waits,
the VM sits in `dispatch_loop` parking for 1 ms at a time
(`src/vm/core_dispatch.rs:1957`), and nothing calls. The debugger already polls
there for `interrupt_pending()`. The observer adds its own check next to it, so
a server waiting on `accept` still answers.

**Cost when not observing.** `VM` gains `observe: Option<Box<ObserveState>>`.
The poll is `if let Some(o) = &self.observe`, a branch that is never taken. It
sits in `push_frame` (`src/vm/mod.rs:595`), next to the debugger's own
`if self.debugger.is_some()` check (`:609`), which is already paid on every
call. A tail call replaces its frame without `push_frame`, so
`tail_call_closure` gets the same check.
Target: no measurable change in the existing benches. The check is a bench in
the style of `benches/effect_tape_off_bench.rs`. `--trace` keeps its
per-instruction check. Folding both into one flag byte is an optimisation for
later, not a requirement.

**Cost when observing.** At each poll the VM does one relaxed atomic load of a
"something is pending" word, which the listener and the sampling timer set. It
does real work only when that word is set.

Why not 0024's `on_instruction` or `on_alloc` hooks: a callback per
instruction or per allocation costs on every program, and its only use here
would be counts that sampling and the existing counters already give.

### §3 Instrumentation

**Counters.** `instructions_executed` stays a plain field. At each poll the VM
publishes it to an `AtomicU64` that `ObserveState` shares with the listener, at
most once per millisecond. Allocations come from `alloc_count::allocations()`,
which is already atomic. Fiber, task and channel counts are read when the
snapshot is built.

**Snapshots.** The listener thread owns the socket. A request for a snapshot
sets the pending word and waits on a channel. At the next safe point the VM
builds the snapshot as **owned serde structs** (strings and numbers, never a
`Value`), sends it over `std::sync::mpsc`, and goes on. The listener serialises
it. Building a table costs about the same as `Inspector::stack` does today. The
program is not stopped for longer than that.

**Hot functions by sampling.** A timer thread sets the pending word every 10 ms
by default. At the next poll the VM walks `frames[0..=frame_index]` and adds
one sample to the top frame's function (self) and one to each distinct function
on the stack (total). Functions are keyed by `Arc<CompiledFunction>` address
and named from `FunctionDebugInfo`. The bias toward call sites is a known
property of safepoint sampling: work inside a primop is charged to the function
that called it.

**Fiber records.** Under `--observe`, `fiber_debug` turns on as it does under
the debugger, so every fiber has a name, a number and a `Blocked` reason. The
observer adds two fields to each record: when the fiber last parked (for the
"for" column), and the size of the parked continuation (frames and slots, read
from `Fiber.parked`).

**Heap census on demand** (S3). The walk starts from the roots:
- the VM stack up to `sp`;
- `globals`;
- each frame's closure;
- `handler_stack` (arms and state);
- every fiber's `body` and `parked` continuation;
- channel buffers;
- event payloads.

It counts each `Rc` once, keyed by address, by value kind and, for ADTs, by
constructor name. Sizes are estimated from `size_of` and lengths. The walk
needs no cycle check, because the heap is acyclic. Sharing is handled the way
`owners` in `src/vm/debug/values.rs` already handles it. A walk has a node
budget (default 10⁶), reports `truncated` when it runs out, and holds the VM
for its whole duration. The client says so.

**A read-only inspector** (S3). `Inspector` holds `&mut DebugControl`
(`src/vm/debug/inspect.rs:108-111`). The readers the observer needs (`stack`,
`handlers`, `locals`) only read. They move to a `VmView<'a> { vm: &'a VM }`
that `Inspector` wraps, so both front ends share one implementation.

**Effect counts** (S3). Counters go on the `OpPerform`, `OpPerformDirect` and
`OpPerformDirectIndexed` arms, keyed by the `PerformDescriptor` constant. They
are allocated only under `--observe`.

| Instrumentation strategy | Cost when off | Cost when on | Used for |
|---|---|---|---|
| Always-on counters | one add per event, every run | same | `instructions_executed` (exists), allocation count (exists) |
| Runtime flag (`--observe`) | one untaken branch per call | atomic load per call, work when pending | everything in S0–S4 |
| Build profile (`-g`) | none in normal builds, but `-g` changes the compiled program | breakpoint sites | not used: an observer must see the program as it really runs |
| On demand (census, drill-down) | none | proportional to the heap or the stack, once | census, drill-down |

### §4 Protocol

The transport is loopback TCP with one JSON object per line, the same choice
as `src/debug_protocol.rs`. `serde_json` is already a dependency. The amount
of data is small: a snapshot is a few kilobytes, sent a few times a second. A
binary format (MessagePack, Cap'n Proto) would not save anything worth having,
and it would lose `nc` and `jq` as debugging tools.

**Discovery.** At startup the program binds `127.0.0.1:0` (or the address
given as `--observe=ADDR`), prints one line on stderr, and writes
`$FLUX_HOME/observe/<pid>.json` holding the pid, program, start time and
address. It removes the file at exit. `flux-observer` lists the files whose
pid is alive.

**Messages.** Requests carry an `id`. Replies echo it. Streams push `event`s.

```text
→ {"id":1,"hello":{"client":"flux-observer","protocol":1}}
← {"id":1,"reply":{"hello":{"protocol":1,"backend":"vm","program":"server.flx","pid":41872,"workers":1}}}

→ {"id":2,"snapshot":{"view":"fibers"}}
← {"id":2,"reply":{"fibers":[{"number":13,"function":"handle_conn","state":"waiting",
     "blocked":"channel #4 recv","parked_ms":191034,"frames":9,"slots":41}, ...]}}

→ {"id":3,"subscribe":{"stream":"overview","interval_ms":1000}}
← {"id":3,"reply":{"ok":{}}}
← {"event":{"stream":"overview","at_ms":872311,"instructions":1840221004,
     "allocations":96104477,"fibers":{"ready":3,"waiting":212,"done":4511}, ...}}

→ {"id":4,"unsubscribe":{"stream":"overview"}}
```

Views in S0–S2: `overview`, `hot`, `fibers`, `tasks`, `channels`. Views added
in S3: `fiber{number}` (drill-down), `census`, `effects`. Trace control
(`trace{function|effect, limit}`) arrives in S4. The protocol version is
checked in `hello`. An unknown view gets an error reply, not a disconnect.

The listener accepts **one client at a time**. A second connection is refused
with a message saying who holds it.

### §5 Client

`crates/flux-observer` is a new workspace member, built on `ratatui` and
`crossterm`. Neither is in `Cargo.lock` today. Keeping them out of the `flux`
crate is the same choice as `crates/flux-dap`, which keeps DAP out of the
compiler.

A TUI suits the MVP:
- it works over SSH, which is where long-running programs run;
- it needs no web stack and no browser;
- observer_cli shows it is enough for this kind of tool.

The client polls snapshots for tables and subscribes to streams for charts.
It keeps its own history, so the program stores none. Because the protocol is
plain JSON lines, a VS Code panel can be added later without touching the
runtime (Future possibilities).

### §6 Native

Native support comes after the VM, in S5. It reuses the same protocol code.
The listener is Rust, in the `flux` staticlib that every native binary already
links (`discover_rust_staticlib`, `src/llvm/pipeline.rs`). The driver passes
`FLUX_OBSERVE` to the child, the same way it passes `FLUX_LEAK_CHECK`
(`src/driver/run_program/backend/native.rs`).

- **Fibers:** `NativeRun::debug_fibers` (`native_abi.rs:335`) already produces
  the table. It writes into one static buffer for lldb, so the observer needs a
  variant that writes into its own buffer, called from the listener thread with
  the same `try_lock` discipline. Work: small.
- **Hot functions:** SIGPROF sampling (`setitimer`). The handler only pushes
  the interrupted PC into a lock-free ring, which is async-signal-safe. The
  listener symbolises PCs with the line tables every build has, through the
  `backtrace` crate already linked. "Total" needs frame pointers, so it is
  available only under full `-g`. Plain builds get "self" only. Work: moderate.
- **Heap footprint:** a counter of chunks taken in `chunk_bump`
  (`runtime/c/rc.c:500`) and of large `malloc`'d blocks in `take_block`. Both
  are slow paths. The inlined allocator (`src/llvm/codegen/prelude.rs:638`) is
  untouched, so generated code does not change and the native ABI tag stays
  [inferred: the runtime archive is rebuilt from its flags stamp and source
  times, `ensure_runtime_lib`, `src/llvm/pipeline.rs:951`]. Live object counts
  by kind exist only under `--debug-heap`.
- **Effect counts:** counters in `flux_yield_to` (`runtime/c/effects.c:424`)
  and `flux_perform_direct`. They are partial, because an arm performed in the
  function that handles it is a direct call that never reaches the runtime.
- **Not available:** instructions (native has none), the per-fiber logical
  stack and handlers (the evidence vector is a thread-local of the running
  thread, readable by lldb but not by a listener), and value inspection.

So native gets the overview without instruction counts, hot functions, and
the fiber, task and channel table. That covers the MVP's purpose.

### §7 Staging

S0–S2 are the MVP, about one to two weeks. Line counts are rough estimates.
Tests go in the folders `tests/README.md` assigns.

| Stage | Change | Exit |
|---|---|---|
| S0 | **Protocol and flag.** `src/observe_protocol.rs`: serde types (~300 lines plus unit tests). The listener thread and discovery file (~250). `--observe[=ADDR]` in `src/cli/shared.rs` and `src/driver/flags.rs`; refused on native until S5 in `src/driver/backend_policy.rs`; wired in `src/driver/run_program/backend/vm.rs` (~120). | Protocol round-trip unit tests. An integration test runs a fixture with `--observe`, connects, and gets `hello` and an `overview` snapshot. The program's stdout is the same with and without `--observe`. |
| S1 | **VM overview and hot functions.** `src/vm/observe/` with `ObserveState`, counters, the sampler and snapshot builders (~400). Poll in `push_frame` (`src/vm/mod.rs`) and `tail_call_closure` (`function_call.rs`) (~30). Idle-loop poll in `core_dispatch.rs` (~30). | An off-mode bench in the style of `effect_tape_off_bench.rs` shows no measurable slowdown. A fixture with one dominant function puts it first in `hot`. A program waiting on a timer still answers a snapshot. |
| S2 | **Fiber, task and channel table, and the TUI.** `fiber_debug` on under `--observe`, plus park time and continuation size (~80). `src/vm/observe/tables.rs` (~250). `crates/flux-observer`: overview, hot, fibers, discovery (~1,200–1,500). | Snapshot tests of the `fibers`, `tasks` and `channels` JSON for the async fixtures (both, a channel, a sleep, a task). TUI rendering tests against `ratatui`'s test backend, replaying a recorded stream. A manual checklist in the crate's README. |
| S3 | **Drill-down, census, effects, lints.** `VmView` split from `Inspector` (~150). `fiber{number}` view (~150). Heap census (~300). Perform counters (~60). Lints: fibers waiting longer than a threshold, channels with waiters on one side only, finished tasks never awaited (~150). | The drill-down's stack matches `flux debug`'s `bt` for the same fixture. Census counts on a fixture with a known heap shape. The census stays within its budget on a 10⁶-node heap. |
| S4 | **Tracing and export.** Tracepoints on functions and effects with argument rendering through `ValueView`. pprof or speedscope export of samples. A fixed-size event ring (park, wake, spawn, perform) for a fiber timeline. Multiple workers. | Golden trace output for a fixture. Exported profiles open in standard tools. |
| S5 | **Native** (§6). Listener in the staticlib, fiber table, SIGPROF hot functions, chunk counter, perform counters. | The VM and native `fibers` snapshots match for the async parity fixtures, apart from fields marked backend-specific. Native hot functions name Flux functions with line tables only. |

**Caches.** S0–S4 do not change the bytecode, so `CACHE_EPOCH`
(`src/shared/cache_paths.rs`) is not bumped [inferred; to be confirmed when S1
lands]. S5 changes only the C runtime and the staticlib, not generated code
(see §6).

## Drawbacks
[drawbacks]: #drawbacks

- **A third front end.** The debugger, the REPL and now the observer all read
  runtime state. The `VmView` split in S3 keeps the readers shared, but each
  front end still has its own protocol and tests.
- **A poll on the call path.** Even an untaken branch on the hottest path has
  to be measured, and kept measured as the VM changes.
- **A listening socket.** It binds to loopback and accepts one client, but any
  local user can connect while the program runs. The data includes values once
  S3 adds drill-down.
- **The two backends show different things.** Native has no instruction
  counts, logical stacks or values. The client has to say which views a
  backend lacks rather than show empty columns.
- **Forcing one worker** (if chosen, see Unresolved questions) makes an
  observed multi-worker program run differently from an unobserved one.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

**Reuse the debugger as it is** (`flux debug --engine`, `on_interrupt`). This
is the least new code, but:
- `-g` compiles a different program: the AST path, Core binders kept, and
  `OpBreakpoint` sites (0188 S2, S9c);
- the debugger forces one worker;
- it keeps no counters.

An observer must see the program as it runs in production. The observer
reuses the debugger's *readers*, not its execution mode.

**Make the inspector an effect handler wrapped around `main`.** This was
considered because it fits Flux's model. It fails for three reasons:
- a handler sees only the effects that reach its scope, so it misses effects
  handled inside the program, all pure work, and every allocation;
- adding it changes the program's effect row and its types;
- it would run Flux code inside the observed program.

What survives of the idea is the runtime boundary hook that already exists,
the tape (`src/vm/replay_hook.rs`). It sees every effect that leaves the
program, and a later stage can stream it as an effect log.

**A web UI served by the runtime.** It would put an HTTP server and assets
into every observed program, and it is awkward over SSH. **A native GUI** needs
a desktop and a GUI toolkit. A TUI client in its own crate keeps the runtime
side to a socket and JSON.

**Always-on, OCaml-style shared-memory counters.** These are attractive for
native, because another process reads them without a socket (Prior art). They
are left to Future possibilities: they need a file per program and a
fixed layout, and they do not help with tables.

**Do nothing.** Long-running async programs remain observable only by
stopping them, or through `FLUX_FIBER_TRACE` text.

## Prior art
[prior-art]: #prior-art

Only what applies to Flux is kept.

- **Erlang Observer and observer_cli.** A process table sortable by reductions,
  memory and mailbox length is the model for the fiber table, with
  instructions, continuation size and waiting time as its columns.
  observer_cli shows that a polling terminal client is enough. Supervision
  trees and ETS have no counterpart (§1).
- **tokio-console.** Instrumentation in the program, with a separate console
  connected over the network. Its *lints* (tasks that are never woken, tasks
  that run too long between yields) are the model for S3's lints.
- **JDK Flight Recorder and JMX.** JFR's always-on, low-overhead event
  buffers, dumped on demand, are the model for S4's event ring. JMX's flat
  named attributes match the existing `--metrics` keys, which the overview
  should reuse rather than rename.
- **Go pprof and the runtime tracer.** Native CPU profiles sample through
  SIGPROF, which §6 adopts. Exporting the pprof format gets existing viewers
  for free (S4). `runtime/trace` shows goroutine scheduling as a timeline,
  which is the model for a fiber timeline.
- **OCaml 5 runtime events and olly.** Ring buffers in a memory-mapped file,
  read by an external tool without stopping the program. This is the best fit
  for native counters later.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- **Multiple workers.** Should `--observe` force one worker, as the debugger
  does (simple, but it changes how the program runs)? Or should each worker
  VM publish its own counters and tables for the listener to merge? The second
  is closer to the point of an observer, and it costs one channel per worker.
- **Attaching to a program not started with `--observe`.** A signal (SIGUSR1)
  could start the listener on demand, as the JVM's attach does. That requires
  the poll to exist in every run, which it does under this design. Decide
  whether to allow it.
- **Authentication.** Loopback with one client, or also a token written to the
  discovery file and checked in `hello`?
- **Census budget.** Is a node budget enough, or should a census be sampled
  (walk one root in N) on large heaps?
- **`--prof`.** Keep `OpEnterCC` cost centres as the exact, at-exit profile,
  or have `--prof` read from the observer's sampler? They answer different
  questions (exact entries and time versus a live view).
- **Overhead budget.** What slowdown is acceptable while observing: 2%? 5%?
- **Effect counts with direct handlers.** On the VM a tail-resumptive operation
  takes `OpPerformDirect`. Should its counts be shown separately from the
  general path?

## Future possibilities
[future-possibilities]: #future-possibilities

- A VS Code panel speaking the same protocol, next to the existing Timeline
  view.
- The scope tree (`both`, `race`, `scope`) as a tree view of fibers, once
  0199 makes `scope` join and cancel its children.
- Shared-memory counters for native, read without a socket, in the style of
  OCaml runtime events.
- dtrace or uprobes on `flux_*` symbols for native tracepoints.
- Continuous recording: keep the event ring always on and dump it on a panic.
- Opening a recorded tape (0188 S1) in the observer: the effect log and fiber
  timeline of a run that has already finished.
- A REPL command (`:observe`) that shows the overview for the REPL's own VM.
