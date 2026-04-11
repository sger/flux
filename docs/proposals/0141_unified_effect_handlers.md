- Feature Name: Unified Effect Handlers — Koka-style Yield Model
- Start Date: 2026-03-27
- Proposal PR:
- Flux Issue:
- Depends on: Proposal 0131 (Aether RC), Proposal 0133 (Unified CorePrimOp)

## Summary

Replace Flux's two incompatible effect handler implementations (VM: Rust frame copying, Native: C setjmp/longjmp) with a single yield-based algorithm inspired by Koka's Perceus runtime. Both backends implement the same algorithm — the VM in Rust, the native backend in C. Parity bugs from algorithmic differences are eliminated.

## Motivation

### Current state: two different mechanisms

| | VM (Rust) | Native (C) |
|---|---|---|
| **Perform** | Copy `Vec<Frame>` + `Vec<Value>` into `Continuation` | `setjmp` saves C stack frame |
| **Resume** | Restore frames + stack slice | `longjmp` back to perform site |
| **Multi-shot** | Clone frames (expensive but works) | Not supported |
| **Nested handlers** | Full support via handler stack | Limited by `jmp_buf` nesting |
| **Tail-resumptive** | `is_direct` flag skips capture | Not optimized |

These are **fundamentally different algorithms**. Even if the output matches for simple cases, edge cases (nested handlers, multiple performs, deep stacks) can diverge.

### What Koka does: yield-based continuation composition

Koka uses **neither** frame copying nor setjmp/longjmp. Instead:

1. `perform` sets a yield flag in the thread context
2. Every function, as it returns, checks the yield flag and adds itself to a continuation array
3. The handler prompt checks if the yield is targeted at it (marker matching)
4. If yes, it composes the accumulated continuations into a single function
5. `resume` calls the composed continuation

**No stack frames are copied.** Continuations are built incrementally as the stack unwinds normally via function returns. This is portable (works in C, JS, Wasm), efficient (one branch per function return), and supports multi-shot effects.

### The parity guarantee

Both backends implement the **same algorithm** — yield + continuation composition + evidence vector + marker matching. The VM implements it in Rust (in the dispatch loop), the native backend implements it in C (in `effects.c`). Unlike frame copying vs setjmp/longjmp, these are the same algorithm in different host languages.

This follows the same pattern as arithmetic: the VM does `i64 + i64` in Rust, the native backend does it in LLVM IR. Different languages, identical behavior.

---

## Design

### Core data structures

**Evidence** — a handler registration:

```
Both backends:
  Evidence { htag, marker, handler, parent_evv }

  htag:       effect identifier (tag)
  marker:     unique i32 id for this handler instance
  handler:    closure handling operations
  parent_evv: evidence vector at handler definition point
```

**Evidence vector** — sorted array of active handlers, stored in context:

```
VM (Rust):   vm.evv: Vec<Evidence>
Native (C):  ctx->evv: int64_t (NaN-boxed pointer to evidence array)
```

**Yield state** — set by perform, checked by every function:

```
Both backends:
  YieldState { yielding, marker, clause, conts[] }

  yielding:  0=no, 1=yielding, 2=yielding_final
  marker:    target handler's marker id
  clause:    operation clause to execute
  conts[]:   accumulated continuation closures (up to 8 inline)
```

### Algorithm (identical in both backends)

#### Handle (install handler)

```
1. w0 = evv_get()                      // save current evidence
2. m  = fresh_marker()                  // unique id
3. ev = Evidence(tag, m, handler, w0)   // create evidence
4. w1 = evv_insert(w0, ev)             // insert sorted by tag
5. evv_set(w1)                          // activate
6. enter prompt loop
```

#### Perform (yield to handler)

```
1. Look up handler in evidence vector by effect tag
2. Extract marker and clause
3. Set: yielding = true, marker = m, clause = f, conts_count = 0
4. Return sentinel value
```

#### Yield propagation (every function return)

```
if yielding:
    conts[conts_count++] = current_continuation
    return YIELD_SENTINEL
else:
    return result  // normal path
```

#### Prompt (handler checks for its yield)

```
if not yielding        → Pure (normal return)
if marker != my_marker → keep yielding (propagate up)
if marker == my_marker → compose conts[] into one continuation,
                          clear yielding, call clause(resume, arg)
```

#### Resume

```
Call the composed continuation with the resume value.
For deep resumption: re-install handler before calling.
```

### VM implementation (Rust)

```rust
pub struct VM {
    // ... existing fields
    evv: Vec<Evidence>,
    yield_state: YieldState,
    marker_counter: i32,
}

// In dispatch loop — OpReturn checks yield flag:
OpCode::OpReturnValue => {
    let result = self.pop()?;
    if self.yield_state.yielding != 0 {
        let cont = self.build_continuation_closure();
        self.yield_state.conts.push(cont);
        self.pop_frame()?;
        return Ok(YIELD_SENTINEL);
    }
    // Normal return (unchanged)
    ...
}
```

### Native implementation (C)

```c
int64_t flux_yield_to(int32_t marker, int64_t clause, FluxEffectCtx *ctx) {
    ctx->yielding = 1;
    ctx->yield.marker = marker;
    ctx->yield.clause = clause;
    ctx->yield.conts_count = 0;
    return FLUX_YIELD_SENTINEL;
}

int64_t flux_yield_extend(int64_t cont, FluxEffectCtx *ctx) {
    if (ctx->yield.conts_count >= 8) {
        // Compose existing into one, make room
        int64_t composed = flux_compose_conts(ctx);
        ctx->yield.conts[0] = composed;
        ctx->yield.conts_count = 1;
    }
    ctx->yield.conts[ctx->yield.conts_count++] = cont;
    return FLUX_YIELD_SENTINEL;
}

FluxPromptResult flux_yield_prompt(int32_t marker, FluxEffectCtx *ctx) {
    if (!ctx->yielding) return PURE;
    if (ctx->yield.marker != marker) return YIELDING;
    int64_t cont = flux_compose_conts(ctx);
    int64_t clause = ctx->yield.clause;
    ctx->yielding = 0;
    return (FluxPromptResult){ YIELD, clause, cont };
}
```

---

## What gets deleted

| Code | Lines | Replaced by |
|------|-------|-------------|
| VM `handler_stack: Vec<HandlerFrame>` | ~50 | Evidence vector |
| VM `OpPerform` (frame copying) | ~140 | yield_to + propagation |
| VM `Continuation` struct | ~60 | Composed closure |
| VM resume (frame restoration) | ~60 | Continuation call |
| `runtime/c/effects.c` (setjmp/longjmp) | ~166 | Yield-based C |

**Total deleted:** ~476 lines
**Total added:** ~350 lines

---

## Migration phases

1. **Add evidence vector to VM** — replace `handler_stack` with sorted evidence vector
2. **Add yield state to VM** — modify `OpPerform` to set yield flag instead of copying frames
3. **Yield check in return path** — modify `OpReturn` to check yield flag, extend continuations
4. **Replace prompt/resume** — compose continuations, delete frame-restoration resume
5. **Rewrite native `effects.c`** — same algorithm in C, delete setjmp/longjmp
6. **Unify optimizations** — tail-resumptive, multi-shot, finalization on both backends

---

## Comparison

| | Flux current (VM) | Flux current (Native) | Koka | Flux proposed |
|---|---|---|---|---|
| Perform | Copy frames | setjmp | Set yield flag | Set yield flag |
| Resume | Restore frames | longjmp | Call composed cont | Call composed cont |
| Multi-shot | Clone frames | Not supported | Re-compose | Re-compose |
| Overhead (no effects) | Zero | Zero | 1 branch/return | 1 branch/return |
| Parity | No guarantee | No guarantee | N/A | **Same algorithm** |

## Drawbacks

- **One branch per function return** — yield check adds ~1 cycle per return even without effects. Koka benchmarks show this is negligible (branch predictor handles it).
- **Large VM refactoring** — frame-copying is deeply integrated. Replacing it touches `OpReturn`, `OpPerform`, `OpHandle`, and `Continuation` type.
- **Continuation representation changes** — `Value::Continuation(Rc<RefCell<Continuation>>)` becomes a composed closure.

## Prior art

- **Koka**: Direct inspiration. Yield + continuation composition + evidence vector. Proven across C, JS, Wasm backends.
- **Effekt**: Similar yield-based effect handlers.
- **libhandler** (Daan Leijen): Standalone C library implementing Koka's model.
