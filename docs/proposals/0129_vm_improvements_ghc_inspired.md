- Feature Name: VM Improvements — GHCi-Inspired Optimizations
- Start Date: 2026-03-26
- Proposal PR:
- Flux Issue:
- Depends on: None (incremental improvements to existing VM)

## Summary

Adopt targeted features from GHC's bytecode interpreter (GHCi) to improve Flux VM performance, developer experience, and extensibility. This is not a rewrite — it's a set of 7 independent improvements, each backported from GHCi's 30+ years of production hardening, adapted for Flux's strict evaluation model and Rc-based memory.

## Motivation

### Current state

Flux's VM (`src/bytecode/vm/`) is a clean, correct implementation: 92 opcodes, explicit frame stack, NaN-boxed values, algebraic effect handlers with delimited continuations. It works well for development and testing.

However, compared to GHCi, several opportunities for improvement exist:

| Area | GHCi | Flux VM |
|------|------|---------|
| Dispatch speed | Computed goto (1 jump per instruction) | Rust `match` (2 jumps typical) |
| Superinstructions | 3 peephole-fused variants | 8 variants (could be 20+) |
| Interactive debugging | Breakpoints, `:step`, variable inspection | `--trace` (instruction dump only) |
| FFI | Arbitrary C calls via libffi | Hardcoded C runtime functions only |
| Compiled code interop | Seamless (shared stack layout with native code) | None (VM and LLVM are separate worlds) |
| Bytecode format | 16-bit word-aligned | 8-bit byte-oriented |
| Profiling | Cost centres, ticks | `--stats` (wall-clock timing only) |

### Why now

1. **Flow.Array module** added 20 new functions — each call through the VM dispatch loop; dispatch overhead matters
2. **AoC benchmarks** show VM is 5-20x slower than native — closing the gap improves the development experience
3. **Interactive debugging tooling** needs breakpoint support
4. **Type classes** (Proposal 0123) will add dictionary-passing overhead — the VM must be fast enough to absorb it

---

## Reference-level explanation

### Phase 1 — Expanded superinstructions

**Goal**: Fuse common instruction sequences into single opcodes, reducing dispatch overhead and stack traffic.

**Current superinstructions** (8):

```
OpReturnLocal(n)         = GetLocal(n) + ReturnValue
OpConsumeLocal0          = ConsumeLocal(0)
OpConsumeLocal1          = ConsumeLocal(1)
OpGetLocal0              = GetLocal(0)
OpGetLocal1              = GetLocal(1)
OpCmpEqJumpNotTruthy     = CmpEq + JumpNotTruthy
OpCmpNeJumpNotTruthy     = CmpNe + JumpNotTruthy
OpCmpGtJumpNotTruthy     = CmpGt + JumpNotTruthy
```

**New superinstructions** (12 additions):

| Superinstruction | Fuses | Hot path |
|-----------------|-------|----------|
| `OpAddLocals(a, b)` | `GetLocal(a) + GetLocal(b) + OpAdd` | Arithmetic on locals |
| `OpSubLocals(a, b)` | `GetLocal(a) + GetLocal(b) + OpSub` | Loop counters |
| `OpGetLocalCall1(n)` | `GetLocal(n) + OpCall(1)` | HOF callbacks `f(x)` |
| `OpConstantAdd(idx)` | `OpConstant(idx) + OpAdd` | `x + 1` patterns |
| `OpGetLocalIndex(n)` | `GetLocal(n) + OpIndex` | Array access `arr[i]` |
| `OpGetLocalIsAdt(n, tag)` | `GetLocal(n) + OpIsAdt(tag)` | Pattern match without stack push |
| `OpSetLocalPop(n)` | `OpSetLocal(n) + OpPop` | Assignment |
| `OpGetLocalGetLocal(a, b)` | `GetLocal(a) + GetLocal(b)` | Two-arg function setup |
| `OpCall0` | `OpCall(0)` | Zero-arg thunk-like calls |
| `OpCall1` | `OpCall(1)` | Single-arg calls (most common) |
| `OpCall2` | `OpCall(2)` | Two-arg calls |
| `OpTailCall1` | `OpTailCall(1)` | Recursive single-arg tail calls |

**Implementation**: Add a peephole optimization pass after bytecode emission in `src/bytecode/compiler/pipeline.rs`:

```rust
fn peephole_optimize(instructions: &mut Vec<u8>) {
    let mut i = 0;
    while i + 4 < instructions.len() {
        match (instructions[i], instructions.get(i + 2)) {
            (OP_GET_LOCAL, Some(&OP_GET_LOCAL)) => {
                let a = instructions[i + 1];
                let b = instructions[i + 3];
                if instructions.get(i + 4) == Some(&OP_ADD) {
                    // Fuse: GetLocal(a) + GetLocal(b) + Add → AddLocals(a, b)
                    instructions[i] = OP_ADD_LOCALS;
                    instructions[i + 1] = a;
                    instructions[i + 2] = b;
                    instructions.drain(i + 3..i + 5);
                    continue;
                }
                // Fuse: GetLocal(a) + GetLocal(b) → GetLocalGetLocal(a, b)
                instructions[i] = OP_GET_LOCAL_GET_LOCAL;
                // a stays at i+1, b stays at i+3 → compact to i+2
                instructions[i + 2] = b;
                instructions.drain(i + 3..i + 4);
                continue;
            }
            _ => {}
        }
        i += opcode_size(instructions[i]);
    }
}
```

**GHC reference**: `GHC/ByteCode/Asm.hs` lines 282-291 merges consecutive `PUSH_L` instructions. GHC's approach is conservative (3 patterns only). Flux can be more aggressive since the instruction set is simpler.

**Expected impact**: 5-10% speedup on computation-heavy benchmarks (based on GHC's reported 10-15% improvement from superinstructions).

**Files**: `src/bytecode/compiler/pipeline.rs`, `src/bytecode/mod.rs` (new opcodes), `src/bytecode/vm/dispatch.rs` (new handlers)

### Phase 2 — Optimized dispatch

**Goal**: Reduce per-instruction dispatch overhead from 2 jumps to 1.

**Current dispatch** (Rust `match`):

```rust
// Compiles to: jump to match table → jump to handler (2 jumps)
match op {
    OpCode::OpAdd => self.op_add(),
    OpCode::OpSub => self.op_sub(),
    // ...92 arms
}
```

LLVM typically compiles a dense `match` on `#[repr(u8)]` enums into a jump table, which is already close to optimal. However, we can help LLVM by:

**Option A: Ensure jump table generation**

```rust
#[repr(u8)]
pub enum OpCode {
    OpConstant = 0,
    OpGetLocal = 1,
    // ... densely packed, no gaps
}

// In dispatch: use unsafe to skip bounds check (already guaranteed by enum)
#[inline(always)]
fn dispatch(&mut self, op: u8) {
    // The match will compile to a single indirect jump through a table
    match unsafe { std::mem::transmute::<u8, OpCode>(op) } {
        OpCode::OpAdd => { /* inline handler */ }
        // ...
    }
}
```

**Option B: Function pointer table** (if match doesn't optimize well)

```rust
type Handler = fn(&mut VM, &[u8], usize) -> usize;

static DISPATCH_TABLE: [Handler; 256] = {
    let mut table: [Handler; 256] = [VM::op_invalid; 256];
    table[OpCode::OpAdd as usize] = VM::op_add;
    table[OpCode::OpSub as usize] = VM::op_sub;
    // ...
    table
};

fn dispatch(&mut self, instructions: &[u8], ip: usize) -> usize {
    let op = instructions[ip];
    DISPATCH_TABLE[op as usize](self, instructions, ip)
}
```

**Option C: Direct threaded code** (most aggressive)

Each instruction handler ends with a `goto` to the next handler. In Rust, this can be approximated with a tail-calling loop, though true computed goto isn't available. The `musttail` attribute on nightly could enable this in the future.

**GHC reference**: `rts/Interpreter.c` uses `LABEL(opcode)` + `goto *(&&lbl_DEFAULT + jumptable[opcode])` for computed goto, with a `switch` fallback when GNU C extensions aren't available.

**Recommendation**: Start with Option A (verify LLVM generates a jump table). Benchmark. Only move to Option B if the match isn't optimal. Skip Option C until Rust supports `musttail`.

**Expected impact**: 0-15% depending on current LLVM codegen quality. Must benchmark.

**Files**: `src/bytecode/vm/dispatch.rs`

### Phase 3 — Breakpoint and stepping support

**Goal**: Add interactive debugging to the VM, enabling a future REPL with `:step`, `:break`, and variable inspection.

**New instruction:**

```rust
OpBreakpoint(u16) // operand: index into breakpoint info table
```

**Breakpoint info table** (stored alongside bytecode):

```rust
pub struct BreakpointInfo {
    pub source_file: String,
    pub line: u32,
    pub column: u32,
    pub local_names: Vec<(u8, String)>,  // (local_index, variable_name)
    pub enabled: bool,
}
```

**VM changes:**

```rust
// In dispatch loop:
OpCode::OpBreakpoint => {
    let bp_idx = read_u16(instructions, ip + 1);
    if let Some(bp) = self.breakpoints.get(bp_idx as usize) {
        if bp.enabled {
            // Pause execution, invoke debugger callback
            let frame_info = self.capture_frame_state();
            (self.debug_callback)(DebugEvent::BreakpointHit {
                info: bp,
                locals: frame_info,
            });
            // Callback may set self.step_mode = true
        }
    }
    3 // ip delta
}
```

**Step modes** (following GHCi):

| Mode | Behavior |
|------|----------|
| `Continue` | Run until next breakpoint |
| `StepInto` | Break at next instruction in any frame |
| `StepOver` | Break at next instruction in current frame |
| `StepOut` | Break when current frame returns |

**GHC reference**: `BRK_FUN` instruction in `GHC/ByteCode/Instr.hs`. GHCi tracks breakpoints per module via `InternalModBreaks` arrays. The interpreter checks `breakpoint_io_action` to decide whether to pause. Variable binding info is carried in `BreakInfo` structs attached to each `BRK_FUN`.

**Compiler changes**: Insert `OpBreakpoint` at the start of each statement when `--debug` flag is set. In release mode, breakpoints are not emitted (zero overhead).

**Files**: `src/bytecode/mod.rs` (new opcode), `src/bytecode/vm/mod.rs` (step mode state), `src/bytecode/vm/dispatch.rs` (handler), `src/bytecode/compiler/` (breakpoint insertion)

### Phase 4 — Foreign function interface (FFI)

**Goal**: Call arbitrary C functions from Flux, not just hardcoded runtime functions.

**New Flux syntax:**

```flux
// Declare a foreign function
foreign fn sqrt(x: Float): Float = "sqrt"
foreign fn write(fd: Int, buf: String, count: Int): Int = "write"
```

**New instruction:**

```rust
OpFFICall(u16) // index into FFI descriptor table
```

**FFI descriptor:**

```rust
pub struct FFIDescriptor {
    pub c_name: String,
    pub param_types: Vec<FFIType>,  // Int, Float, String, Ptr
    pub return_type: FFIType,
    pub resolved_address: Option<*const ()>,  // filled at link time
}
```

**Implementation options:**

| Approach | Pros | Cons |
|----------|------|------|
| **libffi** | Battle-tested, cross-platform, dynamic dispatch | Runtime dependency, slow for hot paths |
| **dlopen + direct call** | No dependency, fast | Platform-specific, unsafe |
| **Compile-time C stubs** | Zero overhead, safe | Requires recompilation when adding FFI |

**Recommendation**: Use `libffi` for dynamic FFI (development), with an option to generate C stubs for production builds (the LLVM native backend can link them directly).

**GHC reference**: `CCALL` instruction in `rts/Interpreter.c` uses libffi. GHCi prepares `ffi_cif` (call interface) structs at link time (`BCONPtrFFIInfo`), then calls `ffi_call()` at runtime. The marshalling handles boxed/unboxed conversions.

**For Flux**: NaN-boxed values simplify marshalling — `Int` untags to `i64`, `Float` untags to `f64`, `String` extracts the `Rc<String>` pointer. Return values are re-tagged.

**Files**: new `src/bytecode/vm/ffi.rs`, `src/syntax/statement.rs` (foreign declarations), `src/bytecode/compiler/` (emit OpFFICall)

### Phase 5 — Native function bridge

**Goal**: Allow the VM to call pre-compiled native Flux functions for performance-critical standard library code.

**Concept**: Compile Flow library functions (map, filter, fold, sort) to native code once. The VM loads the compiled `.dylib`/`.so` and calls native versions instead of interpreting bytecode.

**Architecture:**

```
lib/Flow/List.flx
  → LLVM backend → libflux_flow.dylib (map, filter, fold, ...)
                    ↑
Flux VM: OpCallNative(fn_id)
  → dlopen("libflux_flow.dylib")
  → resolve symbol "flux_flow_list_map"
  → call with NaN-boxed args
  → receive NaN-boxed result
```

**New instruction:**

```rust
OpCallNative(u16, u8) // (native_fn_id, arity)
```

**Bridge protocol**: Both VM and native code use NaN-boxed `i64` values. The calling convention is simple:

```c
// Native function signature (generated by LLVM backend):
int64_t flux_flow_list_map(int64_t xs, int64_t f);

// VM calls:
let result = native_fn(args[0].as_nan_boxed(), args[1].as_nan_boxed());
push(Value::from_nan_boxed(result));
```

**GHC reference**: GHCi achieves this transparently — compiled `.o` files are loaded via the RTS linker, and interpreted code calls compiled code through shared `stg_ap_*` apply frames. The stack layout is identical. Flux can't do this transparently (different stack layouts), but the NaN-box bridge is a practical alternative.

**Expected impact**: 5-10x speedup for standard library hot paths (map, filter, fold, sort run at native speed while user code stays interpreted for fast iteration).

**Files**: new `src/bytecode/vm/native_bridge.rs`, `src/core_to_llvm/` (emit shared library), `src/main.rs` (load bridge at startup)

### Phase 6 — Profiling and cost centres

**Goal**: Add function-level profiling to identify hot spots, following GHCi's cost centre approach.

**Current state**: `--stats` shows wall-clock timing for parse/compile/execute phases. No per-function breakdown.

**New instruction:**

```rust
OpEnterCC(u16)  // cost centre index — pushed at function entry
OpExitCC        // popped at function return
```

**Cost centre data:**

```rust
pub struct CostCentre {
    pub name: String,           // function name
    pub module: String,         // module name
    pub span: Span,             // source location
    pub entries: u64,           // number of calls
    pub alloc_bytes: u64,       // heap bytes allocated
    pub time_ns: u64,           // wall-clock nanoseconds
    pub inner_time_ns: u64,     // time in callees (for exclusive calculation)
}
```

**Output** (with `--prof` flag):

```
COST CENTRE          MODULE         entries   %time  %alloc
─────────────────────────────────────────────────────────
walk_collect         Day06Solver       4779   45.2%   32.1%
build_grid           Day06Solver        130    8.3%   12.4%
drive_loop_jump      Day06Solver      89421   38.1%   41.2%
map                  Flow.Array          131    2.1%    5.3%
sort                 <builtin>           57    4.8%    7.2%
```

**Implementation**: `OpEnterCC`/`OpExitCC` are inserted at function entry/return by the compiler when `--prof` is enabled. Zero overhead when profiling is off (no instructions emitted).

**GHC reference**: GHC's cost centres (`ENTER_CCS_THUNK`, `ENTER_CCS_FUN` in the RTS) track allocation and time per cost centre. The `+RTS -p` flag produces a `.prof` file. GHCi's `BRK_FUN` instruction also records cost centre info. GHC's profiling adds ~5% overhead.

**Files**: `src/bytecode/mod.rs` (new opcodes), `src/bytecode/vm/mod.rs` (cost centre stack), `src/main.rs` (report generation)

### Phase 7 — Stack-on-stack frame optimization (optional)

**Goal**: Improve cache locality by putting frame metadata adjacent to local variables on the value stack, instead of in a separate `Vec<Frame>`.

**Current layout** (two arrays):

```
frames: [Frame{ip,base=0}, Frame{ip,base=20}, Frame{ip,base=45}]
stack:  [val, val, ... val, val, ... val, val, ...]
         ↑ frame 0        ↑ frame 1        ↑ frame 2
         (two cache lines)
```

**Proposed layout** (single array, GHCi-style):

```
stack:  [val, val, ..., FRAME{ip,closure}, val, val, ..., FRAME{ip,closure}, val, ...]
         ↑ frame 0 locals  ↑ frame 0 hdr    ↑ frame 1 locals  ↑ frame 1 hdr
         (one cache line per frame)
```

Frame headers are stored directly on the value stack as a special `Slot::Frame(FrameHeader)` variant. This improves cache locality — a function call touches one contiguous memory region instead of two.

**Tradeoff**: Flux doesn't need liveness bitmaps (no GC), so the main cost of GHCi's mixed-stack approach doesn't apply. However, it makes stack introspection more complex (must skip frame headers when walking locals).

**GHC reference**: GHCi's stack is a single `StgWord*` array. Return addresses (`info_table` pointers) serve as frame markers. The GC walks the stack using liveness bitmaps attached to each info table. Flux can use a tagged `Slot` enum instead of info tables — simpler and type-safe.

**Expected impact**: 5-10% improvement for deep call stacks (better L1 cache utilization). Minimal impact for shallow stacks.

**Recommendation**: Defer until profiling (Phase 6) confirms frame management is a bottleneck.

**Files**: `src/bytecode/vm/mod.rs` (stack layout), `src/runtime/frame.rs`

---

## Priority and effort

| Phase | Feature | Effort | Speedup | DX Impact |
|-------|---------|--------|---------|-----------|
| 1 | Expanded superinstructions | 2-3 days | 5-10% | None |
| 2 | Optimized dispatch | 1-2 days | 0-15% | None |
| 3 | Breakpoints and stepping | 3-5 days | None | High (REPL, debugging) |
| 4 | FFI (libffi) | 5-7 days | N/A (new capability) | High (ecosystem) |
| 5 | Native function bridge | 5-7 days | 5-10x for stdlib | Medium |
| 6 | Profiling / cost centres | 2-3 days | None (diagnostic) | High (optimization) |
| 7 | Stack-on-stack frames | 3-5 days | 5-10% | None |

**Recommended order**: Phase 6 (profiling) first — measure before optimizing. Then Phase 1 (superinstructions) and Phase 3 (breakpoints) in parallel. Phase 4 (FFI) when ecosystem needs arise. Phase 5 (native bridge) when benchmark targets demand it.

---

## What NOT to adopt from GHCi

| GHCi Feature | Why skip |
|--------------|----------|
| Thunks and update frames | Flux is strict — no lazy evaluation needed |
| Liveness bitmaps | Not needed with Rc (no GC scanning) |
| Info table pointers on heap objects | Flux's `Value` enum is self-typed |
| `PUSH_ALTS` (case as separate BCO) | Flux's inline branching is simpler and faster |
| Indirection following (`IND`) | No thunks means no indirections |
| PAP construction at runtime | Flux handles partial application at compile time |
| Shared stack layout with native code | Too invasive; native bridge (Phase 5) is sufficient |
| 16-bit instruction encoding | 8-bit opcodes are more compact and sufficient for 92 instructions |

---

## Drawbacks

- **Superinstructions increase opcode count**: 92 → ~104 opcodes. More `match` arms in dispatch. Mitigated by: each new opcode replaces 2-3 old ones in hot paths.

- **FFI is inherently unsafe**: Calling arbitrary C functions bypasses Flux's type system. Mitigated by: typed foreign declarations, runtime arity/type checks at the bridge boundary.

- **Native bridge adds build complexity**: Users would need the LLVM toolchain to build `libflux_flow.dylib`. Mitigated by: ship pre-built bridge libraries, fall back to interpreted if native not available.

- **Profiling overhead**: Even when disabled, the compiler must track cost centre info for `--prof` mode. Mitigated by: only emit `OpEnterCC`/`OpExitCC` when `--prof` is passed (zero overhead otherwise).

## Rationale and alternatives

### Why adopt from GHCi specifically?

GHCi is the only production bytecode interpreter for a pure functional language with:
- 30+ years of optimization
- Millions of users (every Haskell developer uses GHCi)
- Shared infrastructure with a world-class optimizing compiler

Other bytecode VMs (JVM, CPython, Lua) optimize for different languages. GHCi's patterns (superinstructions for functional code, cost centres for lazy evaluation profiling, BCO-based closures) are directly applicable to Flux.

### Alternative: JIT compilation instead of VM optimization

Instead of optimizing the interpreter, add a JIT (like LuaJIT or JavaScriptCore). This would give much larger speedups (10-100x) but is a massive engineering effort. The native bridge (Phase 5) provides 80% of the JIT benefit for 20% of the effort — native speed for hot standard library functions, interpreted for user code.

### Alternative: Replace VM with tree-walking interpreter

Simpler than bytecode but 5-50x slower. Not viable for real programs. The VM is the right abstraction level.

## Prior art

- **GHCi**: `rts/Interpreter.c` — computed goto dispatch, 105 instructions, `BRK_FUN` breakpoints, `CCALL` FFI via libffi, cost centre profiling
- **Lua 5.4**: Register-based VM, computed goto, 82 instructions, C FFI via `lua_CFunction`, no breakpoints
- **CPython 3.12**: Stack-based VM, computed goto (with specializing adaptive interpreter), 200+ instructions, `ctypes` FFI, `sys.settrace` breakpoints, `cProfile` profiling
- **Erlang BEAM**: Register-based VM, threaded code dispatch, pattern matching instructions, NIFs for native interop, `:observer` profiling

## Unresolved questions

- **Should superinstructions be profile-guided?** Instead of statically choosing which sequences to fuse, profile real programs and fuse the hottest pairs. CPython 3.11+ does this with its specializing adaptive interpreter. More complex but optimal.

- **Should the native bridge use the C calling convention or a custom one?** C calling convention (NaN-boxed i64 args) is simplest. A custom convention could avoid boxing/unboxing for known types. Defer to typed Core IR (Proposal 0123 Phase 7).

- **Should breakpoints be per-line or per-expression?** GHCi supports per-expression stepping. Per-line is simpler and matches most debugger UIs. Start with per-line.

## Future possibilities

- **Adaptive specialization**: Like CPython 3.11's quickening — replace generic opcodes with type-specialized variants after observing runtime types (e.g., `OpAdd` → `OpAddInt` after seeing `Int + Int` N times)
- **Register-based VM**: Convert from stack-based to register-based (like Lua). Reduces stack traffic. Major rewrite but 10-30% faster.
- **Interactive debugger**: Phase 3 (breakpoints) is the foundation for richer step/debug tooling without requiring a dedicated REPL command
- **Hot code reloading**: Combine native bridge (Phase 5) with file watching to reload changed modules without restarting
- **Wasm target for VM**: Compile the Flux VM itself to WebAssembly, enabling browser-based Flux execution
