- Feature Name: M:N Scheduler as Swappable Actor Handler
- Start Date: 2026-03-01
- Proposal PR: pending
- Flux Issue: pending

# Proposal 0071: M:N Scheduler as Swappable Actor Handler

## Summary
[summary]: #summary

Replace the thread-per-actor handler (proposal 0066) with an M:N green thread scheduler
that maps N lightweight Flux actors onto M OS threads. Adds fuel-based preemption to the
VM instruction loop, cooperative yield at `recv()` for JIT actors, and a work-stealing
run queue per scheduler thread. This removes the OS thread count limit (~10K actors) and
enables the system to scale to hundreds of thousands of simultaneous actors.

This proposal does not change the `Actor` effect interface (proposal 0065). User programs
are unchanged. Only the handler implementation is swapped.

## Motivation
[motivation]: #motivation

The thread-per-actor handler (proposal 0066) is limited by the OS thread limit, typically
32K–64K threads, and by the cost of OS thread context switching (~1–10μs per switch).
With M:N scheduling, context switching between actors costs ~100ns (green thread switch),
and the system can support millions of actors within available memory.

The scheduler is a *handler* — user code does not change. The upgrade is purely in the
runtime. This is the key advantage of modeling actors as an algebraic effect.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

### From the user's perspective

No changes to Flux code. Programs that used `spawn`, `send`, `recv` continue to work.
The behavioral differences are:

1. Programs with many actors no longer hit OS thread limits.
2. CPU-bound actors are preempted after their fuel budget, preventing starvation.
3. `recv()` parks the current green thread without blocking an OS thread.

### Scheduler configuration (CLI)

```bash
# Use M:N scheduler with default worker count (num_cpus)
cargo run -- --no-cache --root lib/ my_actor_program.flx

# Explicit worker count
cargo run -- --no-cache --root lib/ --scheduler-threads 4 my_actor_program.flx

# Fuel budget per scheduling quantum (default: 10000 VM instructions)
cargo run -- --no-cache --root lib/ --actor-fuel 5000 my_actor_program.flx
```

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### Architecture overview

```
OS Thread 1          OS Thread 2          OS Thread 3
┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐
│ Scheduler[0]    │  │ Scheduler[1]    │  │ Scheduler[2]    │
│ run_queue:      │  │ run_queue:      │  │ run_queue:      │
│  Worker<ActorId>│  │  Worker<ActorId>│  │  Worker<ActorId>│
│                 │  │                 │  │                 │
│ current actor:  │  │ current actor:  │  │ current actor:  │
│  Vm + fuel      │  │  Vm + fuel      │  │  Vm + fuel      │
└─────────────────┘  └─────────────────┘  └─────────────────┘
         │                    │                    │
         └────────────────────┴────────────────────┘
                              │
                    ┌─────────▼──────────┐
                    │  ActorRegistry     │
                    │  DashMap<id, Cell> │
                    │                   │
                    │  Each ActorCell:   │
                    │    mailbox: MPSC   │
                    │    state: VmState  │
                    │    fuel: i32       │
                    └───────────────────┘
```

### New file: `src/runtime/actor/scheduler.rs`

```rust
// src/runtime/actor/scheduler.rs

use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::thread;
use crossbeam_deque::{Worker, Stealer, Injector, Steal};
use parking_lot::{Condvar, Mutex};

const DEFAULT_FUEL: i32 = 10_000;

pub struct Scheduler {
    workers: Vec<SchedulerThread>,
    injector: Arc<Injector<ActorId>>,   // Global overflow queue
    shutdown: Arc<AtomicBool>,
}

struct SchedulerThread {
    worker:  Worker<ActorId>,           // Local LIFO deque
    stealers: Vec<Stealer<ActorId>>,    // Views into other workers' deques
    parker:  Arc<(Mutex<bool>, Condvar)>,
}

impl Scheduler {
    pub fn new(num_threads: usize) -> (Self, Vec<thread::JoinHandle<()>>) {
        let injector = Arc::new(Injector::new());
        let shutdown = Arc::new(AtomicBool::new(false));

        // Create per-thread deques
        let workers: Vec<Worker<ActorId>> = (0..num_threads)
            .map(|_| Worker::new_fifo())
            .collect();

        // Build stealer lists: each thread can steal from all others
        let stealers: Vec<Vec<Stealer<ActorId>>> = (0..num_threads)
            .map(|i| {
                workers.iter().enumerate()
                    .filter(|(j, _)| *j != i)
                    .map(|(_, w)| w.stealer())
                    .collect()
            })
            .collect();

        let parkers: Vec<Arc<(Mutex<bool>, Condvar)>> = (0..num_threads)
            .map(|_| Arc::new((Mutex::new(false), Condvar::new())))
            .collect();

        let all_parkers: Vec<Arc<_>> = parkers.clone();

        let mut handles = Vec::new();
        for i in 0..num_threads {
            let local_worker_stealer = workers[i].stealer();  // not used; just for symmetry
            let stealers_i = stealers[i].clone();
            let parker_i = parkers[i].clone();
            let injector_i = Arc::clone(&injector);
            let shutdown_i = Arc::clone(&shutdown);
            let all_parkers_i = all_parkers.clone();

            let handle = thread::Builder::new()
                .name(format!("flux-scheduler-{}", i))
                .spawn(move || {
                    scheduler_loop(
                        i,
                        workers[i].clone(), // NOTE: Worker is not Clone; adjust design
                        stealers_i,
                        parker_i,
                        injector_i,
                        shutdown_i,
                        all_parkers_i,
                    );
                })
                .expect("failed to spawn scheduler thread");

            handles.push(handle);
        }

        (Self {
            workers,
            injector,
            shutdown,
        }, handles)
    }

    /// Enqueue an actor id for scheduling.
    /// Called when an actor is spawned or woken from blocked state.
    pub fn enqueue(&self, id: ActorId) {
        self.injector.push(id);
        // Wake a parked scheduler thread
        // (In production: pick the least-loaded; for now: wake any)
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
    }
}

fn scheduler_loop(
    thread_idx: usize,
    local: Worker<ActorId>,
    stealers: Vec<Stealer<ActorId>>,
    parker: Arc<(Mutex<bool>, Condvar)>,
    injector: Arc<Injector<ActorId>>,
    shutdown: Arc<AtomicBool>,
    all_parkers: Vec<Arc<(Mutex<bool>, Condvar)>>,
) {
    loop {
        if shutdown.load(Ordering::Acquire) { break; }

        // Find work: local → injector → steal
        let actor_id = find_work(&local, &injector, &stealers);

        match actor_id {
            Some(id) => {
                run_actor_slice(id, DEFAULT_FUEL);
            }
            None => {
                // No work: park this thread
                let (lock, cvar) = &*parker;
                let mut ready = lock.lock();
                if !*ready {
                    cvar.wait(&mut ready);
                }
                *ready = false;
            }
        }
    }
}

fn find_work(
    local: &Worker<ActorId>,
    injector: &Arc<Injector<ActorId>>,
    stealers: &[Stealer<ActorId>],
) -> Option<ActorId> {
    // 1. Try local queue first (cache-hot)
    if let Some(id) = local.pop() { return Some(id); }

    // 2. Try injector (global overflow)
    loop {
        match injector.steal_batch_and_pop(local) {
            Steal::Success(id) => return Some(id),
            Steal::Empty       => break,
            Steal::Retry       => continue,
        }
    }

    // 3. Steal from a random victim's queue
    // Randomize starting point to avoid convoy effects
    let start = rand_usize() % stealers.len().max(1);
    for i in 0..stealers.len() {
        let idx = (start + i) % stealers.len();
        loop {
            match stealers[idx].steal_batch_and_pop(local) {
                Steal::Success(id) => return Some(id),
                Steal::Empty       => break,
                Steal::Retry       => continue,
            }
        }
    }

    None
}
```

### Fuel-based preemption in the VM

Add a `fuel: i32` field to the VM execution context. Decrement per instruction. When
fuel reaches zero, yield back to the scheduler.

```rust
// src/runtime/vm/mod.rs

pub struct Vm {
    // ... existing fields ...

    /// Instruction budget for the current scheduling quantum.
    /// Set by the scheduler before each run_slice() call.
    /// Reset to DEFAULT_FUEL after each preemption.
    pub fuel: i32,
}

// src/runtime/vm/mod.rs — in run_inner():

fn run_inner(&mut self) -> Result<VmExitReason, String> {
    let mut closure = self.frames[self.frame_index].closure.clone();
    let mut instructions: &[u8] = &closure.function.instructions;

    loop {
        // PREEMPTION CHECK: decrement fuel before each instruction
        self.fuel -= 1;
        if self.fuel <= 0 {
            return Ok(VmExitReason::Preempted);
        }

        let ip = self.frames[self.frame_index].ip;
        if ip >= instructions.len() { break; }

        let op = OpCode::from(instructions[ip]);
        // ... existing dispatch ...
    }
    Ok(VmExitReason::Finished)
}

pub enum VmExitReason {
    Finished,
    /// Actor was preempted (fuel exhausted). Continuation is saved in frames + stack.
    Preempted,
    /// Actor called recv() and is waiting for a message.
    BlockedOnRecv,
}
```

### ActorCell: storing paused VM state

```rust
// src/runtime/actor/cell.rs

pub struct ActorCell {
    pub id:   ActorId,
    /// Inbox: messages sent to this actor
    mailbox: crossbeam_channel::Receiver<Envelope>,
    /// VM state saved between scheduling quanta.
    /// None means the actor has not started yet (initial state).
    vm_state: Option<VmState>,
    pub status: ActorStatus,
}

pub enum ActorStatus {
    /// Ready to run; has fuel remaining from last quantum.
    Runnable,
    /// Blocked waiting for a message. Woken when mailbox is non-empty.
    BlockedOnRecv,
    /// Actor function returned; actor is dead.
    Dead,
}

/// A paused VM's complete state.
/// Saving this is free: the frames + stack already exist in the Vm struct.
/// We just move ownership to ActorCell when the actor is preempted.
pub struct VmState {
    pub frames:      Vec<CallFrame>,
    pub stack:       Vec<Value>,
    pub frame_index: usize,
    pub globals:     Vec<Value>,
    // GC roots: any Value::ConsList or Value::HamtMap in frames/stack
    // are kept alive by being in this struct (Rc handles it)
}

impl VmState {
    /// Extract state from a running VM (called on preemption or recv block)
    pub fn from_vm(vm: Vm) -> Self {
        Self {
            frames:      vm.frames,
            stack:       vm.stack,
            frame_index: vm.frame_index,
            globals:     vm.globals,
        }
    }

    /// Restore state into a fresh VM to resume execution
    pub fn into_vm(self, fuel: i32) -> Vm {
        let mut vm = Vm::new_empty();
        vm.frames      = self.frames;
        vm.stack       = self.stack;
        vm.frame_index = self.frame_index;
        vm.globals     = self.globals;
        vm.fuel        = fuel;
        vm
    }
}
```

### Scheduler run_actor_slice

```rust
fn run_actor_slice(id: ActorId, fuel: i32) {
    let cell = ACTOR_REGISTRY.get(id).expect("actor not in registry");
    let mut cell = cell.lock();

    match cell.status {
        ActorStatus::Dead => return,
        ActorStatus::BlockedOnRecv => {
            // Check mailbox: if message available, unblock and run
            match cell.mailbox.try_recv() {
                Ok(envelope) => {
                    // Put the message on the VM stack as the recv() return value
                    let vm_state = cell.vm_state.take().expect("blocked actor has no VM state");
                    let mut vm = vm_state.into_vm(fuel);
                    vm.push(envelope.payload.into_value()).unwrap();
                    // Resume execution
                    run_vm_slice(&mut cell, vm, fuel);
                }
                Err(_) => {
                    // Still no message: return to scheduler without consuming fuel
                    // Re-enqueue with low priority or park until message arrives
                }
            }
        }
        ActorStatus::Runnable => {
            let vm_state = cell.vm_state.take().expect("runnable actor has no VM state");
            let vm = vm_state.into_vm(fuel);
            run_vm_slice(&mut cell, vm, fuel);
        }
    }
}

fn run_vm_slice(cell: &mut ActorCell, mut vm: Vm, fuel: i32) {
    match vm.run_inner() {
        Ok(VmExitReason::Finished) => {
            cell.status = ActorStatus::Dead;
            ACTOR_REGISTRY.remove(cell.id);
        }
        Ok(VmExitReason::Preempted) => {
            // Save VM state and re-queue
            cell.vm_state = Some(VmState::from_vm(vm));
            cell.status = ActorStatus::Runnable;
            SCHEDULER.enqueue(cell.id);
        }
        Ok(VmExitReason::BlockedOnRecv) => {
            // Save VM state; do NOT re-queue.
            // The actor will be re-queued when a message arrives (in send()).
            cell.vm_state = Some(VmState::from_vm(vm));
            cell.status = ActorStatus::BlockedOnRecv;
        }
        Err(e) => {
            eprintln!("[actor {}] error: {}", cell.id, e);
            cell.status = ActorStatus::Dead;
            ACTOR_REGISTRY.remove(cell.id);
        }
    }
}
```

### Waking a blocked actor on send

```rust
// In ActorRegistry::send():
pub fn send(&self, target: ActorId, sender: ActorId, payload: SendableValue) -> bool {
    match self.get(target) {
        Some(handle) => {
            let ok = handle.tx.send(Envelope { sender_id: sender, payload }).is_ok();
            if ok {
                // If the target was BlockedOnRecv, re-queue it for scheduling
                if let Some(cell) = self.cells.get(&target) {
                    let mut cell = cell.lock();
                    if matches!(cell.status, ActorStatus::BlockedOnRecv) {
                        cell.status = ActorStatus::Runnable;
                        SCHEDULER.enqueue(target);
                    }
                }
            }
            ok
        }
        None => false,
    }
}
```

### JIT actors: cooperative yield at recv()

JIT-compiled actors do not have a fuel counter (native code cannot be interrupted without
safepoints). For the M:N scheduler, JIT actors yield cooperatively at `recv()`:

```rust
// src/jit/runtime_helpers.rs

#[no_mangle]
pub extern "C" fn rt_actor_recv(ctx: *mut JitContext) -> *mut Value {
    let ctx = unsafe { &mut *ctx };

    // Try non-blocking receive first
    match try_recv_current_actor() {
        Some(val) => return ctx.alloc_value(val),
        None => {}
    }

    // No message available: the JIT actor must block its OS thread
    // (JIT actors are pinned to one OS thread in the M:N scheduler)
    // This is acceptable for Phase 1 JIT support; green JIT threads require
    // safepoint-based preemption (future proposal)
    match recv_blocking_current_actor() {
        Ok(val)  => ctx.alloc_value(val),
        Err(e)   => { ctx.set_error(e); std::ptr::null_mut() }
    }
}
```

JIT actors in the M:N scheduler consume one OS thread when blocked on `recv()`. This is
the same behavior as the thread-per-actor handler. Full green-thread support for JIT
requires safepoint insertion (future proposal, not in scope here).

### Cargo.toml additions

```toml
[dependencies]
crossbeam-deque = "0.8"
parking_lot = "0.12"
rand = "0.8"
```

### Validation commands

```bash
# Build with M:N scheduler
cargo build

# Run with 4 scheduler threads
cargo run -- --no-cache --root lib/ \
    --scheduler-threads 4 \
    examples/actors/many_actors.flx

# Stress test: 10K actors
cargo run -- --no-cache --root lib/ examples/actors/stress_10k.flx

# Verify preemption: CPU-bound actor does not starve IO actor
cargo run -- --no-cache --root lib/ examples/actors/preemption_test.flx

# Benchmark: M:N vs thread-per-actor
cargo bench --bench scheduler_bench
```

### Stress test fixture

```flux
-- examples/actors/stress_10k.flx
-- Spawns 10K actors, each sends one message and exits

import Flow.Actor

fn worker(id: Int, collector: ActorId) with Actor, IO {
    let msg = recv()
    send(collector, (id, msg))
}

fn collector(n: Int, acc: Int) with Actor, IO {
    if n == 0 {
        print(acc)
    } else {
        let (_, _) = recv()
        collector(n - 1, acc + 1)
    }
}

fn main() with Actor, IO {
    let col = spawn(\() with Actor -> collector(10000, 0))
    let ids = range(0, 10000)
    map(ids, \i -> do {
        let w = spawn(\() with Actor -> worker(i, col))
        send(w, "ping")
    })
}
```

## Drawbacks
[drawbacks]: #drawbacks

- Significant implementation complexity over proposal 0066 (~1500 lines vs ~800 lines).
- JIT actors still block OS threads on `recv()`. Full JIT green threads require safepoints
  (not in scope for this proposal).
- Work-stealing schedulers have subtle liveness bugs if the implementation is incorrect.
  `crossbeam-deque` handles the lock-free correctness; the scheduler loop itself must be
  carefully tested.
- The `VmState` extraction on every preemption copies `Vec<CallFrame>` and `Vec<Value>`.
  This is O(stack depth) per context switch. For deeply recursive actors, this is visible.
  Mitigation: use `Box<[Value]>` with fixed-size stacks and swap ownership instead of
  copying.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

**Why `crossbeam-deque` instead of `tokio`?** As discussed in prior proposals, tokio
requires async/await coloring through the VM. The VM is synchronous. `crossbeam-deque`
is a direct building block that composes with the existing synchronous VM architecture.

**Why fuel-based preemption instead of timer-based?** Timer-based preemption (SIGALRM
or `setitimer`) requires signal handlers and can interrupt at arbitrary points. Fuel-based
preemption is deterministic, debuggable, and requires no OS interaction on the hot path.

**Why not copy Go's GMP model exactly?** Go's P (processor) concept requires goroutines
to be aware of which P they are running on for local state (e.g., timers, defer stacks).
Flux actors have no P-local state. A simpler two-level M:N model (M OS threads, N actors,
global + per-thread deques) is sufficient.

## Prior art
[prior-art]: #prior-art

- **BEAM scheduler** — the direct inspiration. One run queue per scheduler thread, work
  stealing, reduction counting for preemption. This proposal implements the same model.
- **Go runtime scheduler (GMP)** — M:N with work-stealing and async preemption.
- **crossbeam-deque** — Rust implementation of Chase-Lev work-stealing deque.
- **Proposal 0066** — the thread-per-actor handler this proposal supersedes.

## Unresolved questions
[unresolved-questions]: #unresolved-questions

1. Should the default fuel budget (10,000 instructions) be configurable per-actor or only
   globally? Decision: global for now; per-actor priorities are a future enhancement.
2. What is the correct behavior when the main thread's actor exits but child actors are
   still running? Decision: wait for all actors to complete (join), then exit the process.
3. Should the scheduler expose a `yield()` primitive to Flux code for cooperative yielding?
   Decision: no; yield is an implementation detail, not a language feature.

## Future possibilities
[future-possibilities]: #future-possibilities

- **Safepoint-based JIT preemption**: emit `SAFEPOINT_FLAG` checks at loop back-edges
  in Cranelift-generated code, enabling JIT actors to also be green threads.
- **Actor priority levels**: `spawn_with_priority(fn, High | Normal | Low)`. High-priority
  actors get larger fuel budgets and are scheduled before normal actors.
- **Sysmon thread**: a background thread that detects actors that have not yielded for
  more than N milliseconds (e.g., stuck in an infinite loop without a `recv()`).
- **NUMA awareness**: bind scheduler threads to NUMA nodes, prefer to steal from same-node
  workers before cross-node steal.
