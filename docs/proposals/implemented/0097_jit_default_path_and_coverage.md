- Feature Name: JIT Default Path and Coverage
- Start Date: 2026-03-10
- Status: Obsolete (Cranelift JIT backend removed; superseded by core_to_llvm)
- Proposal PR:
- Flux Issue:

# Proposal 0097: JIT Default Path and Coverage

## Summary

Make the Cranelift JIT the default execution path for Flux programs by adding a
transparent VM fallback, fixing generic closure dispatch, lifting capture expression
restrictions, and completing deep effect handler support. The `--jit` flag and
`--features jit` gate are removed as requirements; the JIT runs automatically and
falls back to the VM for any construct it cannot compile.

## Motivation

The Flux JIT compiler is approximately 85% feature-complete but is gated behind
`--features jit` at compile time and `--jit` at runtime. Users who want JIT
performance must opt in explicitly, and any unsupported construct causes a hard
error with no recovery — the program aborts rather than falling back to the VM.

This creates a false dichotomy: either the entire program runs on the JIT or it
doesn't run at all. Real programs hit unsupported constructs (generic closure
dispatch, expression captures, deep effect handlers) and the JIT becomes unusable
for them.

The JIT already produces significantly faster code than the VM for:
- Recursive integer arithmetic (fibonacci, nqueens, sorting)
- ADT construction and pattern matching
- Tight loops with tail-call optimization
- Shallow effect handlers

The remaining gaps prevent most real functional programs from reaching the JIT
path. Closing them — combined with a VM fallback — would make the JIT the correct
default for all Flux programs.

### Current execution flow

```
cargo run --features jit -- --jit program.flx
    ↓
jit_compile(program)
    ↓ if Err: abort with error message    ← no recovery
    ↓ if Ok: run native code
```

### Proposed execution flow

```
cargo run -- program.flx
    ↓
jit_compile(program)
    ↓ if Err: fall through to VM silently    ← transparent fallback
    ↓ if Ok: run native code
```

## Guide-level explanation

### For users

No change to the language. Programs run faster automatically. The `--jit` flag
is removed; JIT is always attempted. The `--no-jit` flag can be added for
debugging or benchmarking the VM path explicitly.

If the JIT cannot compile a program (unsupported construct, internal error), the
VM runs it transparently. No user-visible error unless both paths fail.

### For contributors

The JIT compilation result becomes a `Option<CompiledProgram>` rather than
`Result<CompiledProgram, String>`. A `None` means "fall back to VM" rather than
"abort". The four gaps described below are the specific constructs that currently
force `None` for most real programs.

## Reference-level explanation

### Change 1: VM fallback

`main.rs` currently has two separate code paths gated on `use_jit`. The change
collapses them into a single path that always attempts JIT and falls back:

```rust
// Before
if use_jit {
    let compiled = jit_compile(&program, &interner, &options)?;
    jit_execute(compiled)
} else {
    vm_execute(&bytecode)
}

// After
#[cfg(feature = "jit")]
let compiled = jit_compile(&program, &interner, &options).ok();

#[cfg(feature = "jit")]
if let Some(compiled) = compiled {
    return jit_execute(compiled);
}

vm_execute(&bytecode)
```

The `jit_compile` function returns `Result`; `.ok()` converts `Err` to `None`
and allows the fallback. When `--stats` is enabled, a note is printed indicating
which path ran and why the JIT was skipped (if applicable).

The `--features jit` compile-time gate remains for environments where Cranelift
is unavailable (e.g., embedded targets). On standard builds, JIT is always
included.

### Change 2: Generic closure dispatch

**Problem**: The JIT requires exact arity and concrete type at every call site.
When a closure is passed as a boxed `Value` to a higher-order function, the JIT
emits an error rather than a dynamic dispatch:

```
"unsupported capture in JIT function literal"
```

The root cause is that `compile_user_function_call` and `compile_generic_call`
do not handle `Value::JitClosure` at runtime — they assume the callee is a
statically-known function ID.

**Fix**: Add a `rt_call_closure` runtime helper that accepts a boxed `*mut Value`
known to be a `JitClosure` or `Closure`, extracts the function pointer and
upvalue array, and performs the call:

```c
// runtime_helpers.rs (exposed via extern "C")
Value *rt_call_closure(JitContext *ctx, Value *callee, Value **args, usize nargs);
```

At JIT compile time, when `compile_generic_call` encounters a callee that is a
`JitValueKind::Boxed` (not a statically-known function), it emits a call to
`rt_call_closure` instead of aborting:

```rust
// compile_generic_call — current
if callee.kind != JitValueKind::Boxed {
    return Err("cannot call non-boxed value".into());
}
// fall through to rt_generic_call

// compile_generic_call — proposed
// emit rt_call_closure(ctx, callee_ptr, args_ptr, nargs)
// check result for NULL (error propagation)
// return JitValue::boxed(result)
```

This unblocks the most common higher-order patterns:

```flux
map(fn(x) { x + 1 }, [1, 2, 3])    -- fn literal passed as Value: now works
filter(is_even, xs)                  -- named function passed as Value: now works
fold(fn(acc, x) { acc + x }, 0, xs) -- accumulator closure: now works
```

### Change 3: Capture expression lifting

**Problem**: Function literals can only capture identifiers. Non-identifier
expressions in the closure body that reference outer scope cause:

```
"unsupported capture in JIT function literal"
```

The restriction exists because capture resolution walks `Expression::Identifier`
nodes in the closure body and looks them up in the enclosing scope. Complex
sub-expressions inside captures are not walked.

**Fix**: During `collect_literal_function_specs`, for each free variable in a
function literal that is not a simple identifier, emit a synthetic `let` binding
before the function literal and rewrite the capture to reference that binding:

```flux
-- Source
fn outer(n: Int) {
    fn(x) { x + n * 2 }   -- n * 2 is not captured, but n is an identifier: OK
}

-- Trickier case (hypothetical)
fn outer() {
    let f = expensive()
    fn(x) { x + f.value }  -- f.value is member access, not identifier
}

-- After lifting (compiler-internal rewrite)
fn outer() {
    let f = expensive()
    let __cap0 = f.value    -- hoisted
    fn(x) { x + __cap0 }   -- now a plain identifier capture
}
```

This is a compile-time AST rewrite that requires no runtime changes. The lifting
pass runs as part of `collect_literal_function_specs` before Cranelift IR is
generated.

### Change 4: Deep effect handler support

**Problem**: The `resume` parameter in handler arms is currently compiled as a
pre-built identity function. This works for shallow handlers (handlers that
immediately return a value without calling resume) and trivial resume cases
(handlers that call `resume(value)` once and return). It does not work for:

- Handlers that call resume multiple times (non-determinism, backtracking)
- Handlers that pass resume to another function (coroutine-style)
- Handlers that capture resume in a closure for later invocation

**Root cause**: `rt_push_handler` receives a pre-compiled identity function for
`resume`. The actual continuation (the suspended computation) is never captured
and threaded through.

**Fix (incremental)**:

Phase 1 — Correct single-resume handlers. The handler arm receives a real
continuation `Value` that, when called with one argument, resumes the suspended
`perform` site. This requires `rt_perform` to:

1. Capture the current JIT stack frame as a `Continuation`
2. Wrap it in a `Value::Continuation`
3. Pass it to the handler arm as the `resume` parameter

The continuation representation already exists in the VM (`Value::Continuation`,
`Rc<RefCell<Continuation>>`). The JIT needs to produce compatible continuations
from its frame state.

Phase 2 — Multi-resume and captured resume. Deferred to a follow-up proposal.
Requires full delimited continuation capture from JIT frames, which interacts
with Cranelift's stack layout.

For this proposal, Phase 1 is in scope. Phase 2 is a non-goal.

### Change 5: Remove --jit flag, add --no-jit

```rust
// Before
let use_jit = args.iter().any(|arg| arg == "--jit");

// After
let skip_jit = args.iter().any(|arg| arg == "--no-jit");
let use_jit = !skip_jit;
```

`--no-jit` is a debug/benchmark flag. It forces the VM path even when the JIT
would succeed. Not exposed in user-facing documentation as a primary flag.

### Implementation order

1. **VM fallback** — change `main.rs` to use `.ok()` on `jit_compile` result.
   No JIT changes needed. Immediate safety net for all subsequent work.

2. **Capture expression lifting** — AST rewrite in `collect_literal_function_specs`.
   No runtime changes. Low risk, removes a common error class.

3. **Generic closure dispatch** — add `rt_call_closure` to `runtime_helpers.rs`,
   wire it into `compile_generic_call`. Requires careful ABI design for the
   closure call convention.

4. **Remove `--jit` flag** — once fallback exists and coverage is improved, make
   JIT the default.

5. **Deep effect handlers (Phase 1)** — real continuation passing through
   `rt_perform`. The most complex change; deferred until 1-4 are stable.

### Interaction with Proposal 0096 (Tracing GC for VM Values)

If `Value::Closure` becomes a `GcHandle` (Proposal 0096), the `rt_call_closure`
helper must dereference through `JitContext::gc_heap` rather than casting a raw
`Rc` pointer. The two proposals are independent and can land in either order, but
the `rt_call_closure` ABI should be designed with GC-managed closures in mind:
accept `*mut Value` (a boxed value pointer) rather than assuming an `Rc` layout.

## Drawbacks

- **JIT compilation time on startup** — currently the JIT is opt-in so users who
  don't need it pay zero cost. Making it default adds compilation time to every
  program run. Mitigation: the `.fxc` bytecode cache already exists; a parallel
  `.fxn` native cache could store compiled native code and skip recompilation on
  unchanged inputs.

- **Diagnostic quality on JIT path** — JIT runtime errors currently produce less
  detailed diagnostics than the VM (no stack trace, limited source location).
  Making JIT default exposes more users to these weaker diagnostics. Mitigation:
  improve `JitContext` error reporting in parallel.

- **Fallback opacity** — users may not know whether their program ran on the JIT
  or VM. `--stats` output should clearly indicate which path ran.

## Rationale and alternatives

### Why not keep --jit opt-in?

The JIT is the correct long-term execution path. Keeping it opt-in means the
majority of users never benefit from it, and JIT coverage gaps remain invisible
because they are never hit in practice. Making it default forces coverage gaps to
surface as VM fallbacks, creating pressure to close them.

### Why not compile per-function rather than per-program?

Per-function JIT (like HotSpot's tiered compilation) would give finer fallback
granularity — JIT-compile only the functions that succeed, VM-interpret the rest
within the same program run. This is the ideal long-term model but requires
significant changes to how the JIT shares state with the VM (globals, GC heap,
handler stack). Per-program compilation is the right first step; per-function
tiering is a follow-up.

### Why Cranelift over LLVM?

Cranelift is already integrated, fast to compile (sub-second for typical Flux
programs), and has no runtime linking complexity. LLVM would produce faster
native code but at significantly higher compilation latency and binary size.
Cranelift is the right choice for a language where startup time matters.

## Prior art

- **LuaJIT** — interpreter and JIT in the same runtime; JIT is always active,
  VM runs when JIT cannot compile a trace. The model of "JIT first, VM fallback"
  is well-established.

- **V8 (JavaScript)** — tiered: interpreter → Sparkplug (baseline JIT) →
  Maglev → Turbofan. Each tier is faster but requires more compilation time.
  Flux's two-tier model (VM + Cranelift) maps directly.

- **GHC** — always compiles to native via STG. No interpreter fallback in
  production. This is the right long-term direction for Flux once JIT coverage
  reaches 100%.

- **Chez Scheme** — compiles everything eagerly, no interpreter fallback. Very
  fast compilation (Cranelift-comparable) with high-quality native code.

- **Proposal 0031** — original Cranelift JIT backend proposal. This proposal
  extends 0031 with a coverage completion and deployment model.

## Unresolved questions

1. **Native code cache** — should compiled native code be cached to disk (`.fxn`
   files alongside `.fxc` bytecode)? This would eliminate JIT startup cost on
   repeated runs but requires a cache invalidation strategy.

2. **Per-function fallback** — when a single function fails JIT compilation, should
   the entire program fall back to the VM, or should that function run on the VM
   while the rest runs on the JIT? The latter requires a mixed-mode execution model
   where VM and JIT frames coexist on the call stack.

3. **`--features jit` compile-time gate** — should the JIT be unconditionally
   compiled in (no feature flag) or remain optional? Removing the flag simplifies
   the build but increases binary size and compile time for all targets.

4. **Diagnostic parity** — the VM produces richer runtime errors (source spans,
   error codes, aggregator output). What is the minimum JIT diagnostic quality
   required before JIT becomes the default?

5. **Effect handler Phase 2 scope** — multi-resume and captured-resume handlers
   are deferred. Are there Flux programs in the current examples or benchmarks
   that require them? If so, Phase 2 should be pulled into this proposal.

## Future possibilities

- **Tiered compilation** — VM → Cranelift baseline → optimized Cranelift with
  profile-guided inlining. Profile hot functions after N VM executions, JIT
  compile only those.

- **Native cache (`.fxn`)** — persist compiled native code to disk, keyed on
  bytecode hash. Eliminates JIT startup cost on repeated runs of unchanged programs.

- **Inline caches** — at polymorphic call sites (generic dispatch), cache the
  last seen callee type and emit a fast-path check. Common in JavaScript VMs;
  would significantly speed up `map`/`filter`/`fold` on the JIT path.

- **LLVM backend** — for programs where startup time is irrelevant (batch
  processing, AOT compilation), an LLVM backend would produce faster native code
  than Cranelift. Cranelift remains the default for interactive use.
