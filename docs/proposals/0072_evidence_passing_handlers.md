- Feature Name: Evidence Passing for Tail-Resumptive Effect Handlers
- Start Date: 2026-03-01
- Proposal PR: pending
- Flux Issue: pending

# Proposal 0072: Evidence Passing for Tail-Resumptive Effect Handlers

## Summary
[summary]: #summary

Optimize effect handler dispatch for *tail-resumptive* handlers — handlers whose every
operation arm immediately resumes exactly once — by compiling them to direct function
calls with an implicit evidence parameter rather than heap-allocating a continuation.
This eliminates the continuation allocation overhead for the most common effect patterns:
`State`, `Reader`, `Writer`, and similar monad-like effects.

## Motivation
[motivation]: #motivation

Flux's current effect handler mechanism uses `OpHandle`, `OpPerform`, and
`Continuation(Rc<RefCell<Continuation>>)`. Every `perform` allocates a continuation
that captures the current call frame stack. For tail-resumptive effects, this allocation
is unnecessary — the continuation is always resumed immediately, so it never needs to
be stored on the heap.

The overhead matters for effects used in tight loops:

```flux
-- Every call to get() and set() allocates a Continuation today
fn count_elements(xs: List<Int>) -> Int {
    handle {
        let result = fold(xs, 0, \(acc, _) -> do {
            set(get() + 1)   -- allocates Continuation twice per iteration
            acc + 1
        })
        result
    } with {
        get()     -> resume(state_ref)
        set(v)    -> do { state_ref = v; resume(unit) }
    }
}
```

With evidence passing, `get()` and `set()` compile to direct calls with no allocation.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### What changes for users

Nothing changes in Flux surface syntax. The optimization is a compiler pass that
detects tail-resumptive handlers and emits more efficient code. The semantic behavior
is identical.

### What "tail-resumptive" means

An effect handler is tail-resumptive if every operation arm has the form:

```flux
handle { body } with {
    op(args) -> resume(expr)          -- resumes immediately
    op(args) -> do { side_effect; resume(expr) }  -- resumes after pure work
}
```

The key constraint: `resume(...)` is the *last* call in every arm, and it is called
exactly once.

These handlers are **NOT** tail-resumptive (they need heap continuations):

```flux
-- Multi-shot: resume called more than once
amb() -> do { resume(true); resume(false) }

-- Non-resuming: continuation is discarded (like exceptions)
throw(e) -> error(e)

-- Stored continuation: resume is passed to another function
get_continuation() -> resume(resume)
```

### Effect patterns that benefit

| Effect | Pattern | Speedup |
|---|---|---|
| `State<s>` | get/set as mutable reference | 5–10× per operation |
| `Reader<e>` | ask as implicit parameter | 5–10× per operation |
| `Writer<w>` | tell as append to buffer | 5–10× per operation |
| `Error<e>` | raise as early return | Not applicable (non-resuming) |
| `Actor` | recv as blocking call | Not applicable (suspends) |

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Compiler pass: tail-resumptive detection

Add a new analysis pass that runs during PASS 2 on `handle` expressions:

```rust
// src/bytecode/compiler/tail_resumptive.rs

/// Returns true if all arms of a handler are tail-resumptive.
pub fn is_tail_resumptive(handler: &HandleExpression) -> bool {
    handler.arms.iter().all(is_arm_tail_resumptive)
}

fn is_arm_tail_resumptive(arm: &HandlerArm) -> bool {
    is_tail_resumptive_expr(&arm.body)
}

fn is_tail_resumptive_expr(expr: &Expression) -> bool {
    match expr {
        // resume(v) — the terminal form
        Expression::Call { callee, .. }
            if is_resume_call(callee) => true,

        // do { stmts; resume(v) } — resume is the last expression
        Expression::Block { statements, final_expr, .. } => {
            // All statements must be pure (no nested performs)
            statements.iter().all(|s| is_pure_statement(s))
                && is_tail_resumptive_expr(final_expr)
        }

        // let x = pure_expr in tail_resumptive_body
        Expression::Let { value, body, .. } => {
            is_pure_expr(value) && is_tail_resumptive_expr(body)
        }

        // if/else: both branches must be tail-resumptive
        Expression::If { then_branch, else_branch, .. } => {
            is_tail_resumptive_expr(then_branch)
                && else_branch.as_ref()
                    .map(|e| is_tail_resumptive_expr(e))
                    .unwrap_or(true)
        }

        // Any other form: not tail-resumptive (conservative)
        _ => false,
    }
}
```

### Evidence struct generation

For a tail-resumptive handler, the compiler generates an *evidence struct* — a
heap-allocated struct (allocated once per `handle` expression, not per `perform`) that
holds the mutable state the handler needs:

```rust
// Conceptual: for `handle { body } with { get() -> resume(s); set(v) -> resume(unit) }`
// The compiler generates an evidence struct like:

struct StateEvidence {
    value: Cell<Value>,   // the mutable state variable
}

// And the handler operations become:
fn ev_get(ev: &StateEvidence) -> Value {
    ev.value.get().clone()
}

fn ev_set(ev: &StateEvidence, v: Value) {
    ev.value.set(v);
}
```

In bytecode terms, the evidence struct is represented as a single `Rc<RefCell<Vec<Value>>>`
(a tuple of handler-local mutable slots). One slot per `var` in the handler.

### New OpCode: `OpHandleTR` (Handle Tail-Resumptive)

```rust
// src/bytecode/opcode.rs

pub enum OpCode {
    // ... existing opcodes including OpHandle ...

    /// OpHandleTR(evidence_size, handler_id)
    /// For tail-resumptive handlers:
    ///   1. Allocates an evidence record (Vec<Value> of size evidence_size)
    ///   2. Pushes it as an implicit parameter on the handler stack
    ///   3. Executes the body
    ///   4. OpPerformTR operations look up the handler and call it directly
    ///      (no continuation allocation)
    OpHandleTR = 0xD0,

    /// OpPerformTR(handler_offset, op_index, arity)
    /// Direct call to a tail-resumptive handler operation.
    /// handler_offset: how far up the handler stack to look
    /// op_index: which operation in the handler
    /// arity: argument count
    /// No continuation is allocated.
    OpPerformTR = 0xD1,
}
```

### VM execution of `OpHandleTR`

```rust
// src/runtime/vm/dispatch.rs

OpCode::OpHandleTR => {
    let evidence_size = Self::read_u8_fast(instructions, ip + 1) as usize;
    let handler_id    = Self::read_u16_fast(instructions, ip + 2);

    // Allocate evidence record (one allocation per handle expression)
    let evidence = Rc::new(RefCell::new(vec![Value::None; evidence_size]));

    // Push the handler descriptor with evidence reference onto handler stack
    self.handler_stack.push(HandlerFrame::TailResumptive {
        handler_id,
        evidence: Rc::clone(&evidence),
    });

    // Push evidence as implicit first argument to the body
    self.push(Value::Evidence(evidence))?;

    Ok(3)  // opcode + evidence_size + handler_id (2 bytes)
}

OpCode::OpPerformTR => {
    let handler_offset = Self::read_u8_fast(instructions, ip + 1) as usize;
    let op_index       = Self::read_u8_fast(instructions, ip + 2) as usize;
    let arity          = Self::read_u8_fast(instructions, ip + 3) as usize;

    // Find the handler on the handler stack
    let handler_idx = self.handler_stack.len() - 1 - handler_offset;
    let evidence = match &self.handler_stack[handler_idx] {
        HandlerFrame::TailResumptive { evidence, .. } => Rc::clone(evidence),
        _ => return Err("OpPerformTR: handler is not tail-resumptive".to_string()),
    };

    // Pop arguments
    let mut args: Vec<Value> = (0..arity)
        .map(|_| self.pop().expect("stack underflow in OpPerformTR"))
        .collect();
    args.reverse();

    // Call the handler operation directly — NO continuation allocation
    let result = call_tr_handler_op(&evidence, op_index, args)?;
    self.push(result)?;

    Ok(4)
}
```

### State effect example: before and after

**Before (with continuation allocation):**

```
OpPerform STATE_GET 0           ; allocates Continuation, suspends, resumes
; frame count: 3 allocations per get/set pair
```

**After (with evidence passing):**

```
OpLoadEvidence 0                ; push evidence ref onto stack (O(1), no alloc)
OpPerformTR 0, 0, 0             ; call ev_get(evidence) directly (O(1), no alloc)
```

### Interaction with non-tail-resumptive handlers

The detection pass is conservative. If any arm is not tail-resumptive, the entire handler
uses the existing `OpHandle` + `OpPerform` path. The two paths coexist:

```
Effect handler
├── All arms tail-resumptive → OpHandleTR + OpPerformTR (no heap allocation per perform)
└── Any arm non-tail-resumptive → OpHandle + OpPerform (existing path, continuation allocated)
```

### Validation commands

```bash
# Build with evidence passing
cargo build

# Run a State-effect benchmark
cargo bench --bench state_effect_bench

# Verify semantic equivalence
cargo test --test effect_handler_tests

# Compare allocation counts (before/after)
cargo run -- --no-cache --leak-detector examples/effects/state_counter.flx
```

### Example fixture: State effect benchmark

```flux
-- examples/effects/state_counter.flx

effect State {
    get : () -> Int
    set : (Int) -> Unit
}

fn count_to(n: Int) with State {
    if get() < n {
        set(get() + 1)
        count_to(n)
    } else {
        unit
    }
}

fn main() with IO {
    -- With evidence passing: zero heap allocations for get/set
    -- Without: 2 * n allocations
    let result = handle {
        count_to(1000000)
        get()
    } with State(0) {
        get()   -> resume(state)
        set(v)  -> do { state = v; resume(unit) }
    }
    print(result)   -- 1000000
}
```

## Drawbacks
[drawbacks]: #drawbacks

- Two distinct handler compilation paths increase compiler and VM complexity.
- The tail-resumptive analysis is conservative: some handlers that are semantically
  tail-resumptive may not be recognized (e.g., when `resume` is passed through a
  `let` binding). These handlers fall back to the continuation path.
- A new `Value::Evidence` variant is required (like `Value::ReuseToken` in 0069), adding
  a permanent internal-only variant to the `Value` enum.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

**Why a separate `OpHandleTR` instead of optimizing `OpHandle`?** The two paths have
fundamentally different execution models. Keeping them separate opcodes makes the VM
dispatch table clear and avoids branching in the hot path of `OpPerform`.

**Why not always use evidence passing?** Non-tail-resumptive handlers (backtracking, AMB,
non-local exit) genuinely need heap-allocated continuations. The optimization only applies
where it is semantically valid.

**Alternative: CPS transform.** The entire effect system could be compiled to CPS
(continuation-passing style), which is guaranteed efficient for tail-resumptive handlers.
However, CPS transform of the entire program is a bigger refactoring and affects all
code, not just handlers.

## Prior art
[prior-art]: #prior-art

- **Koka** (Leijen, 2017) — "Type Directed Compilation of Row-Polymorphic Effects"
  introduces evidence passing as the compilation target for algebraic effects. This
  proposal implements the same optimization.
- **Eff language** — compiles tail-resumptive handlers to direct function calls.
- **libhandler** (Leijen) — a C library implementing evidence passing for effects.
- **Proposal 0073** — applies this optimization specifically to State and Reader effects.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

1. Should the evidence record be on the Rust stack (avoiding the `Rc` allocation) when
   the handler does not escape the current scope? This requires lifetime analysis.
   Deferred for now; the `Rc` allocation (one per `handle` expression) is cheap.
2. Should the `Value::Evidence` variant be visible to GC tracing? Yes — it must be
   treated as a root since it holds `Value`s.

## Future possibilities
[future-possibilities]: #future-possibilities

- **Stack-allocated evidence**: when the handler's evidence record provably does not
  escape (it stays within the `handle` block), allocate it on the Rust stack instead
  of the Flux heap.
- **Proposal 0073**: applies evidence passing specifically to `State` and `Reader` effects
  with concrete Rust-level mutable references, eliminating even the `RefCell` overhead.
- **Whole-program specialization**: inline the evidence struct fields when the effect
  type is known at compile time (monomorphization of effects).
