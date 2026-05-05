//! Direct CorePrimOp dispatch for the VM (Proposal 0133 Step 5).
//!
//! Replaces the old PrimOp → execute_primop() path with a single dispatch
//! keyed by CorePrimOp.  Same Rust implementations, no translation layer.

use std::fs;
use std::io::Read as IoRead;
use std::rc::Rc;
use std::time::{Instant, SystemTime};

use crate::core::CorePrimOp;
use crate::runtime::RuntimeContext;
use crate::runtime::hamt as rc_hamt;
use crate::runtime::hash_key::HashKey;
use crate::runtime::value::{Value, format_value};

// ── TCP handle table (proposal 0174 Phase 1b-vii) ────────────────────────────
// The VM path manages TCP connections as Rust TcpStream/TcpListener handles
// stored in a thread-local table keyed by integer handle IDs.  This avoids
// NaN-box encoding complexity and keeps the VM path self-contained.
mod vm_tcp {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};

    enum TcpHandle {
        Stream(TcpStream),
        Listener(TcpListener),
    }

    thread_local! {
        static TCP_HANDLES: RefCell<HashMap<i64, TcpHandle>> = RefCell::new(HashMap::new());
        static NEXT_ID: RefCell<i64> = RefCell::new(1);
    }

    fn alloc_id() -> i64 {
        NEXT_ID.with(|n| {
            let id = *n.borrow();
            *n.borrow_mut() = id + 1;
            id
        })
    }

    pub fn tcp_connect(host: &str, port: i64) -> Result<i64, String> {
        let addr = format!("{}:{}", host, port);
        let stream = TcpStream::connect(&addr)
            .map_err(|e| format!("tcp_connect {}: {}", addr, e))?;
        let id = alloc_id();
        TCP_HANDLES.with(|h| h.borrow_mut().insert(id, TcpHandle::Stream(stream)));
        Ok(id)
    }

    pub fn tcp_read(handle: i64, max: i64) -> Result<String, String> {
        TCP_HANDLES.with(|h| {
            let mut map = h.borrow_mut();
            match map.get_mut(&handle) {
                Some(TcpHandle::Stream(stream)) => {
                    let cap = if max > 0 && max <= (1 << 24) { max as usize } else { 4096 };
                    let mut buf = vec![0u8; cap];
                    let n = stream.read(&mut buf)
                        .map_err(|e| format!("tcp_read {}: {}", handle, e))?;
                    Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
                }
                _ => Err(format!("tcp_read: invalid handle {}", handle)),
            }
        })
    }

    pub fn tcp_write_all(handle: i64, data: &str) -> Result<(), String> {
        TCP_HANDLES.with(|h| {
            let mut map = h.borrow_mut();
            match map.get_mut(&handle) {
                Some(TcpHandle::Stream(stream)) => {
                    stream.write_all(data.as_bytes())
                        .map_err(|e| format!("tcp_write_all {}: {}", handle, e))
                }
                _ => Err(format!("tcp_write_all: invalid handle {}", handle)),
            }
        })
    }

    pub fn tcp_close(handle: i64) {
        TCP_HANDLES.with(|h| { h.borrow_mut().remove(&handle); });
    }

    pub fn tcp_listen(host: &str, port: i64) -> Result<i64, String> {
        let addr = format!("{}:{}", host, port);
        let listener = TcpListener::bind(&addr)
            .map_err(|e| format!("tcp_listen {}: {}", addr, e))?;
        let id = alloc_id();
        TCP_HANDLES.with(|h| h.borrow_mut().insert(id, TcpHandle::Listener(listener)));
        Ok(id)
    }

    pub fn tcp_accept(listener: i64) -> Result<i64, String> {
        TCP_HANDLES.with(|h| {
            let mut map = h.borrow_mut();
            match map.get_mut(&listener) {
                Some(TcpHandle::Listener(l)) => {
                    let (stream, _) = l.accept()
                        .map_err(|e| format!("tcp_accept {}: {}", listener, e))?;
                    let id = NEXT_ID.with(|n| { let id = *n.borrow(); *n.borrow_mut() = id + 1; id });
                    drop(map);
                    TCP_HANDLES.with(|h| h.borrow_mut().insert(id, TcpHandle::Stream(stream)));
                    Ok(id)
                }
                _ => Err(format!("tcp_accept: invalid listener {}", listener)),
            }
        })
    }
}

// ── Async backend handle (proposal 0174 Phase 1b-vi-a) ───────────────────────
// Lazy process-global `MioBackend` for fiber primops that need real reactor
// roundtrips (`FiberSleep` today; `Async.timeout` and TCP-suspending ops
// follow in 1b-vi-b/c). The reactor thread starts on first use and lives
// for the rest of the process — `MioBackend` is intentionally leaked via
// `Box::leak` rather than parked behind a `thread_local!` Drop. Reason:
// on Windows, TLS destructors fire during `RtlExitUserProcess`, which has
// already started killing other threads, so `JoinHandle::join` from
// `MioBackend::Drop` panics with "threads should not terminate
// unexpectedly". OS process exit reaps the reactor thread anyway, so we
// give up the (unused) graceful-shutdown semantics. With one fiber the
// OS-thread call stack *is* the continuation; this module deliberately
// does NOT involve the FiberScheduler or capture continuations — that's
// 1b-vi-b/c work.
mod vm_async {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::runtime::r#async::backend::{AsyncBackend, RequestId};
    use crate::runtime::r#async::backends::mio::MioBackend;

    static BACKEND: OnceLock<&'static MioBackend> = OnceLock::new();
    static NEXT_REQ: AtomicU64 = AtomicU64::new(1);

    pub fn backend() -> Result<&'static MioBackend, String> {
        if let Some(b) = BACKEND.get() {
            return Ok(*b);
        }
        let owned = Box::leak(Box::new(MioBackend::new()));
        owned
            .start()
            .map_err(|e| format!("MioBackend::start failed: {e}"))?;
        // Race-tolerant: if another thread set this first, our leaked box
        // is harmlessly orphaned (process-lifetime anyway).
        Ok(*BACKEND.get_or_init(|| owned))
    }

    pub fn alloc_request_id() -> RequestId {
        RequestId(NEXT_REQ.fetch_add(1, Ordering::Relaxed))
    }
}

// ── Fiber registry (proposal 0174 Phase 1b-vi-b₁) ────────────────────────────
// Plumbing-only: a per-thread `FiberScheduler` lives for the duration of an
// `Async.run_async` boundary, with a depth counter so nested boundaries share
// one instance. Each `FiberFork` goes through `scheduler.spawn` to allocate a
// real `FiberId`, and `with_current` tracks which fiber id the OS thread is
// currently executing so b₂ can target wakeups correctly. Execution order is
// unchanged — `FiberFork`'s body still runs inline to completion. The point
// is to land the registry shape without behaviour change so b₂ can replace
// the inline call with a real park/resume cycle without touching bookkeeping.
mod vm_fibers {
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::rc::Rc;

    use crate::runtime::RuntimeContext;
    use crate::runtime::r#async::backend::{AsyncBackend, RequestId};
    use crate::runtime::r#async::context::WorkerId;
    use crate::runtime::r#async::fiber::{Fiber, FiberId, FiberState};
    use crate::runtime::r#async::scheduler::FiberScheduler;
    use crate::runtime::value::Value;

    /// How a parent fiber's resume value is assembled when its child(ren)
    /// finish (proposal 0174 Phase 1b-vi-b₂.2).
    enum AwaitKind {
        Both {
            left: FiberId,
            right: FiberId,
        },
        Race {
            children: Vec<FiberId>,
            won: bool,
        },
        /// `Async.timeout`: parent_req is shared between the body fiber's
        /// completion (delivers `Some(result)`) and a backend timer
        /// completion (delivers `None`). Whichever fires first wins.
        Timeout {
            body_child: FiberId,
        },
    }

    thread_local! {
        static SCHED: RefCell<Option<FiberScheduler>> = const { RefCell::new(None) };
        static DEPTH: Cell<u32> = const { Cell::new(0) };
        static CURRENT: Cell<Option<FiberId>> = const { Cell::new(None) };
        // (frame_index, sp) recorded at the outermost FiberRunAsync entry —
        // the boundary that FiberSleep captures continuations down to
        // (proposal 0174 Phase 1b-vi-b₂.1).
        static BOUNDARY: Cell<Option<(usize, usize)>> = const { Cell::new(None) };
        // Park request signalled by FiberSleep / FiberSuspend.  The dispatch
        // loop reads-and-clears this after each fiber tick to learn whether
        // the fiber suspended or finished.  Stores (request_id, captured cont
        // as a Value::Continuation).
        static PENDING_PARK: RefCell<Option<(u64, Value)>> = const { RefCell::new(None) };
        // Root fiber id for the current FiberRunAsync boundary; the dispatch
        // loop uses this to recognise root-fiber completion and propagate the
        // result.  None when no run_async is active.
        static ROOT: Cell<Option<FiberId>> = const { Cell::new(None) };
        // Synthetic-await coordination (Phase 1b-vi-b₂.2).
        static AWAITS: RefCell<HashMap<u64, AwaitKind>> = RefCell::new(HashMap::new());
        static AWAITER_INDEX: RefCell<HashMap<FiberId, Vec<u64>>> =
            RefCell::new(HashMap::new());
        static RESULTS: RefCell<HashMap<FiberId, Value>> = RefCell::new(HashMap::new());
        static RESUME_VALUES: RefCell<HashMap<u64, Value>> = RefCell::new(HashMap::new());
    }

    /// Enter an `Async.run_async` boundary: lazy-init the scheduler on the
    /// outermost entry, spawn the root fiber, and make it current. Returns
    /// the root fiber id so the caller can pair this with `exit_run_async`.
    pub fn enter_run_async() -> FiberId {
        let depth = DEPTH.with(|d| {
            let n = d.get();
            d.set(n + 1);
            n
        });
        if depth == 0 {
            SCHED.with(|s| {
                *s.borrow_mut() = Some(FiberScheduler::new(1));
            });
        }
        let root = SCHED.with(|s| {
            s.borrow_mut()
                .as_mut()
                .expect("scheduler must be initialised after enter_run_async")
                .spawn(WorkerId(0))
        });
        CURRENT.with(|c| c.set(Some(root)));
        root
    }

    /// Pop the matching fiber off the ready queue (it was placed there by
    /// `spawn` and never executed via the scheduler — b₁ runs bodies inline).
    /// On outermost exit, tear down the scheduler.
    pub fn exit_run_async(_root: FiberId) {
        // Drain any leftover ready fibers (b₁ FiberFork bookkeeping
        // artifacts; race losers that finished after parent woke; etc).
        SCHED.with(|s| {
            if let Some(sched) = s.borrow_mut().as_mut() {
                while sched.next_ready(WorkerId(0)).is_some() {}
            }
        });
        let new_depth = DEPTH.with(|d| {
            let n = d.get().saturating_sub(1);
            d.set(n);
            n
        });
        if new_depth == 0 {
            SCHED.with(|s| {
                *s.borrow_mut() = None;
            });
            CURRENT.with(|c| c.set(None));
            // Phase 1b-vi-b₂.2: also tear down await coordination state
            // so a second top-level run_async on the same thread starts fresh.
            clear_await_state();
        }
    }

    /// Allocate a child fiber via the scheduler and return its id. Must be
    /// called inside an active `enter_run_async` / `exit_run_async` window.
    pub fn spawn_child() -> FiberId {
        SCHED.with(|s| {
            s.borrow_mut()
                .as_mut()
                .expect("FiberFork outside Async.run_async — scheduler missing")
                .spawn(WorkerId(0))
        })
    }

    /// Run `f` with `id` recorded as the current fiber, restoring the previous
    /// value on return (including on early-return via `?`). The guard's
    /// `Drop` ensures CURRENT is always restored.
    pub fn with_current<F, R>(id: FiberId, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        struct Guard(Option<FiberId>);
        impl Drop for Guard {
            fn drop(&mut self) {
                CURRENT.with(|c| c.set(self.0));
            }
        }
        let prev = CURRENT.with(|c| c.replace(Some(id)));
        let _g = Guard(prev);
        f()
    }

    // ── Phase 1b-vi-b₂.1: park/resume + dispatch loop ───────────────────────

    /// Set the FiberRunAsync boundary, returning the previous value so the
    /// caller can restore it on exit (supports nested run_async).
    pub fn set_boundary(frame_index: usize, sp: usize) -> Option<(usize, usize)> {
        BOUNDARY.with(|b| b.replace(Some((frame_index, sp))))
    }

    pub fn restore_boundary(prev: Option<(usize, usize)>) {
        BOUNDARY.with(|b| b.set(prev));
    }

    pub fn boundary() -> Option<(usize, usize)> {
        BOUNDARY.with(|b| b.get())
    }

    /// FiberSleep / FiberSuspend signal that the current fiber has captured
    /// its continuation and parked on `req`.  The dispatch loop will see
    /// this on the next tick boundary.
    pub fn signal_park(req: RequestId, cont: Value) {
        PENDING_PARK.with(|p| *p.borrow_mut() = Some((req.0, cont)));
    }

    pub fn take_park() -> Option<(u64, Value)> {
        PENDING_PARK.with(|p| p.borrow_mut().take())
    }

    /// Mark `id` as the root fiber for the current run_async boundary, and
    /// stash the body closure on the matching ready-queue Fiber so the
    /// dispatch loop knows to invoke it on first dispatch.
    pub fn set_root_with_body(id: FiberId, body: Value) {
        ROOT.with(|r| r.set(Some(id)));
        SCHED.with(|s| {
            if let Some(sched) = s.borrow_mut().as_mut() {
                // Rebuild the worker 0 ready queue with the body attached
                // to the matching fiber.  The b₁ enter_run_async pushed the
                // root fiber on; we drain, modify, re-push in order.
                let mut buf: Vec<Fiber> = Vec::new();
                while let Some(f) = sched.next_ready(WorkerId(0)) {
                    buf.push(f);
                }
                for mut f in buf {
                    if f.id == id {
                        f.body = Some(body.clone());
                    }
                    sched.spawn_existing(f);
                }
            }
        });
    }

    pub fn root() -> Option<FiberId> {
        ROOT.with(|r| r.get())
    }

    pub fn clear_root() {
        ROOT.with(|r| r.set(None));
    }

    /// Allocate a child fiber and attach `body` so the dispatch loop will
    /// invoke it on first run (proposal 0174 Phase 1b-vi-b₂.2). Mirror of
    /// `set_root_with_body` but for child fibers.
    pub fn spawn_child_with_body(body: Value) -> FiberId {
        let id = SCHED.with(|s| {
            s.borrow_mut()
                .as_mut()
                .expect("spawn_child_with_body outside Async.run_async")
                .spawn(WorkerId(0))
        });
        // Find the just-spawned fiber and set its body.
        SCHED.with(|s| {
            if let Some(sched) = s.borrow_mut().as_mut() {
                let mut buf: Vec<Fiber> = Vec::new();
                while let Some(f) = sched.next_ready(WorkerId(0)) {
                    buf.push(f);
                }
                for mut f in buf {
                    if f.id == id {
                        f.body = Some(body.clone());
                    }
                    sched.spawn_existing(f);
                }
            }
        });
        id
    }

    /// Register a `FiberBoth` await: parent_req fires when both children finish.
    pub fn register_both_await(parent_req: u64, left: FiberId, right: FiberId) {
        AWAITS.with(|a| {
            a.borrow_mut()
                .insert(parent_req, AwaitKind::Both { left, right })
        });
        AWAITER_INDEX.with(|idx| {
            let mut idx = idx.borrow_mut();
            idx.entry(left).or_default().push(parent_req);
            idx.entry(right).or_default().push(parent_req);
        });
    }

    /// Register a `FiberTimeout` await: parent_req fires when either the
    /// body child completes (delivers `Some(result)`) or a backend timer
    /// keyed on `parent_req` fires (delivers `None`).
    pub fn register_timeout_await(parent_req: u64, body_child: FiberId) {
        AWAITS.with(|a| {
            a.borrow_mut()
                .insert(parent_req, AwaitKind::Timeout { body_child })
        });
        AWAITER_INDEX.with(|idx| {
            idx.borrow_mut()
                .entry(body_child)
                .or_default()
                .push(parent_req);
        });
    }

    /// Called by the backend completion pump just before `scheduler.complete`.
    /// If the request id matches a Timeout await, set the parent's resume
    /// value to `None` and discard the await metadata so that a later body
    /// completion is dropped silently.  Returns `true` if this was a
    /// Timeout-routed completion (caller may still call
    /// `scheduler.complete` to wake the parent).
    pub fn try_route_timer_for_timeout(req: u64) -> bool {
        let kind = AWAITS.with(|a| a.borrow_mut().remove(&req));
        match kind {
            Some(AwaitKind::Timeout { body_child }) => {
                // Drop the body child from the awaiter index so its
                // mark-done sees no waiter for this parent_req.
                AWAITER_INDEX.with(|idx| {
                    let mut idx = idx.borrow_mut();
                    if let Some(reqs) = idx.get_mut(&body_child) {
                        reqs.retain(|r| *r != req);
                    }
                });
                set_resume_value(req, Value::None);
                true
            }
            Some(other) => {
                // Not a Timeout — put it back (we removed-then-checked).
                AWAITS.with(|a| a.borrow_mut().insert(req, other));
                false
            }
            None => false,
        }
    }

    /// Register a `FiberRace` await: parent_req fires when any child finishes.
    pub fn register_race_await(parent_req: u64, children: Vec<FiberId>) {
        AWAITER_INDEX.with(|idx| {
            let mut idx = idx.borrow_mut();
            for c in &children {
                idx.entry(*c).or_default().push(parent_req);
            }
        });
        AWAITS.with(|a| {
            a.borrow_mut().insert(
                parent_req,
                AwaitKind::Race {
                    children,
                    won: false,
                },
            )
        });
    }

    pub fn set_resume_value(req: u64, value: Value) {
        RESUME_VALUES.with(|r| r.borrow_mut().insert(req, value));
    }

    pub fn take_resume_value(req: u64) -> Option<Value> {
        RESUME_VALUES.with(|r| r.borrow_mut().remove(&req))
    }

    /// A child fiber finished with `value`. Stash the result, walk awaiters,
    /// determine which parent requests are now satisfiable, build their
    /// resume values, and return the list of `(parent_req, resume_value)`
    /// pairs.  The caller (dispatch loop) is responsible for storing each
    /// resume value via `set_resume_value` and calling
    /// `scheduler.complete(parent_req)` to wake the parent — done outside
    /// the AWAITS borrow to avoid re-entrant `RefCell` panics.
    pub fn on_fiber_done(id: FiberId, value: Value) -> Vec<(u64, Value)> {
        RESULTS.with(|r| r.borrow_mut().insert(id, value));

        let parent_reqs: Vec<u64> = AWAITER_INDEX
            .with(|idx| idx.borrow_mut().remove(&id))
            .unwrap_or_default();

        let mut completions: Vec<(u64, Value)> = Vec::new();
        for parent_req in parent_reqs {
            let take_result = |fid: FiberId| -> Option<Value> {
                RESULTS.with(|r| r.borrow_mut().remove(&fid))
            };
            let peek_result = |fid: FiberId| -> bool {
                RESULTS.with(|r| r.borrow().contains_key(&fid))
            };

            let kind = AWAITS.with(|a| a.borrow_mut().remove(&parent_req));
            let Some(kind) = kind else { continue };
            match kind {
                AwaitKind::Both { left, right } => {
                    if peek_result(left) && peek_result(right) {
                        let l = take_result(left).expect("left present");
                        let r = take_result(right).expect("right present");
                        // Build (left, right) tuple.
                        let tuple = Value::Tuple(Rc::new(vec![l, r]));
                        completions.push((parent_req, tuple));
                    } else {
                        // Only one of the pair is done — re-insert the await
                        // and keep the index entry alive for the other child.
                        AWAITS.with(|a| {
                            a.borrow_mut()
                                .insert(parent_req, AwaitKind::Both { left, right })
                        });
                        // Also re-add this parent_req to the awaiter index
                        // for whichever child hasn't finished yet, so its
                        // mark-done will see us.
                        let other = if id == left { right } else { left };
                        AWAITER_INDEX.with(|idx| {
                            idx.borrow_mut().entry(other).or_default().push(parent_req)
                        });
                    }
                }
                AwaitKind::Race {
                    children,
                    won,
                } => {
                    if !won {
                        // First-wins: deliver this child's result, mark won.
                        let result = take_result(id).expect("just inserted");
                        completions.push((parent_req, result));
                        // The other children may still finish; their
                        // mark-done won't find AWAITS[parent_req] (we just
                        // removed it), so they'll be dropped silently.
                        let _ = children;
                    } else {
                        // Should not happen — once won, AWAITS is gone, so
                        // we wouldn't have hit this case.
                        unreachable!("race awaiter saw second completion despite won=true");
                    }
                }
                AwaitKind::Timeout { body_child } => {
                    if id == body_child {
                        // Body finished before the timer.  Deliver
                        // Some(result).  The backend timer is still
                        // outstanding; if it fires later,
                        // try_route_timer_for_timeout will see no AWAITS
                        // entry and `scheduler.complete` will silently
                        // fail (parent not in suspended map anymore).
                        let result = take_result(id).expect("just inserted");
                        completions.push((parent_req, Value::Some(Rc::new(result))));
                    } else {
                        // Some other fiber accidentally indexed under this
                        // await — defensive no-op.
                        AWAITS.with(|a| {
                            a.borrow_mut()
                                .insert(parent_req, AwaitKind::Timeout { body_child })
                        });
                    }
                }
            }
        }

        completions
    }

    /// Tear down the await-coordination state at run_async exit
    /// (Phase 1b-vi-b₂.2). Avoids leaks across nested boundaries.
    pub fn clear_await_state() {
        AWAITS.with(|a| a.borrow_mut().clear());
        AWAITER_INDEX.with(|idx| idx.borrow_mut().clear());
        RESULTS.with(|r| r.borrow_mut().clear());
        RESUME_VALUES.with(|r| r.borrow_mut().clear());
    }

    /// Drive the FiberRunAsync dispatch loop until all fibers are done or
    /// the root fiber returns a value.  Invokes fiber bodies and resumes
    /// parked fibers via the supplied `ctx`; pumps the mio backend when no
    /// fiber is ready.
    pub fn dispatch_loop(
        ctx: &mut dyn RuntimeContext,
        backend: &'static crate::runtime::r#async::backends::mio::MioBackend,
    ) -> Result<Value, String> {
        let root_id = root().expect("dispatch_loop called outside FiberRunAsync");
        let mut root_result: Option<Value> = None;

        loop {
            // Drain ready queue.
            loop {
                let next = SCHED.with(|s| {
                    s.borrow_mut()
                        .as_mut()
                        .expect("scheduler missing in dispatch_loop")
                        .next_ready(WorkerId(0))
                });
                let Some(mut fiber) = next else { break };

                // Skip fibers with no work (b₁ FiberFork pushes a fiber but
                // also runs its body inline; the fiber on the ready queue is
                // a bookkeeping artifact with no body and no parked cont).
                if fiber.body.is_none() && fiber.parked.is_none() {
                    continue;
                }

                let fiber_id = fiber.id;
                // Resume value: if the wakeup was caused by a synthetic await
                // (FiberBoth / FiberRace), the dispatch loop stored a value
                // keyed by the request id when the children finished.
                // Default for backend-timer wakeups is Value::None.
                let resume_val = fiber
                    .last_completion_req
                    .take()
                    .and_then(take_resume_value)
                    .unwrap_or(Value::None);
                let outcome = with_current(fiber_id, || {
                    if let Some(cont) = fiber.parked.take() {
                        ctx.resume_from_dispatch(Value::Continuation(cont), resume_val)
                    } else if let Some(body) = fiber.body.take() {
                        ctx.invoke_value(body, vec![])
                    } else {
                        unreachable!("checked above")
                    }
                });

                // Did the fiber park during this tick?  FiberSleep/FiberSuspend
                // capture+unwind+set PENDING_PARK, then return Err so the
                // outcome we observe here is Err with the park signal staged.
                if let Some((req, cont_val)) = take_park() {
                    let cont_rc = match cont_val {
                        Value::Continuation(rc) => rc,
                        _ => return Err(
                            "dispatch_loop: PENDING_PARK contained non-Continuation value".into(),
                        ),
                    };
                    fiber.parked = Some(cont_rc);
                    fiber.state = FiberState::Suspended { request_id: req };
                    SCHED.with(|s| {
                        s.borrow_mut()
                            .as_mut()
                            .expect("scheduler missing")
                            .insert_suspended(WorkerId(0), req, fiber);
                    });
                    // The Err that surfaced was the park-unwind signal, not
                    // a real error; ignore it.
                    let _ = outcome;
                    continue;
                }

                // No park: outcome is the fiber's actual result or a real error.
                match outcome {
                    Ok(v) => {
                        if fiber_id == root_id {
                            root_result = Some(v.clone());
                        }
                        // Synthetic-await coordination: record the result and
                        // wake any parents whose await condition is now met.
                        // `on_fiber_done` returns a list of (parent_req,
                        // resume_value) pairs to flush; we set each value
                        // *before* calling scheduler.complete so the resumed
                        // parent fiber sees its expected resume value.
                        let completions = on_fiber_done(fiber_id, v);
                        for (parent_req, resume_val) in completions {
                            set_resume_value(parent_req, resume_val);
                            SCHED.with(|s| {
                                s.borrow_mut()
                                    .as_mut()
                                    .expect("scheduler missing")
                                    .complete(WorkerId(0), RequestId(parent_req));
                            });
                        }
                    }
                    Err(e) => return Err(e),
                }
            }

            // Exit as soon as the root fiber is done, even if children
            // (e.g. FiberRace losers) remain suspended. Their pending
            // backend completions are abandoned — fine for b₂.2;
            // cooperative cancellation is 1b-vi-c work.
            if root_result.is_some() {
                break;
            }
            // No more ready fibers.  If nothing is suspended either, we're done.
            let suspended = SCHED.with(|s| {
                s.borrow()
                    .as_ref()
                    .map(|sched| sched.suspended_count(WorkerId(0)))
                    .unwrap_or(0)
            });
            if suspended == 0 {
                break;
            }

            // Pump the backend until a completion arrives, then route it.
            loop {
                if let Some(c) = backend.next_completion() {
                    // If this completion is the timer half of a
                    // FiberTimeout await, set the parent's resume value to
                    // None before waking it.
                    let _ = try_route_timer_for_timeout(c.request_id.0);
                    SCHED.with(|s| {
                        s.borrow_mut()
                            .as_mut()
                            .expect("scheduler missing")
                            .complete(WorkerId(0), c.request_id);
                    });
                    break;
                }
                std::thread::park_timeout(std::time::Duration::from_millis(1));
            }
        }

        Ok(root_result.unwrap_or(Value::None))
    }
}

/// Execute a `CorePrimOp` with the given arguments.
///
/// This is the single dispatch point for all `OpPrimOp` instructions in the VM.
/// Each arm matches a `CorePrimOp` variant and runs the corresponding Rust
/// implementation inline (no sub-dispatch through `PrimOp`).
pub fn execute_core_primop(
    ctx: &mut dyn RuntimeContext,
    op: CorePrimOp,
    args: Vec<Value>,
) -> Result<Value, String> {
    use CorePrimOp::*;

    match op {
        // ── Typed integer arithmetic ──────────────────────────────────
        IAdd => int2(&args, |a, b| Value::Integer(a + b), "iadd"),
        ISub => int2(&args, |a, b| Value::Integer(a - b), "isub"),
        IMul => int2(&args, |a, b| Value::Integer(a * b), "imul"),
        IDiv => int2_result(
            &args,
            |a, b| {
                if b == 0 {
                    Err("division by zero".into())
                } else {
                    Ok(Value::Integer(a / b))
                }
            },
            "idiv",
        ),
        IMod => int2_result(
            &args,
            |a, b| {
                if b == 0 {
                    Err("modulo by zero".into())
                } else {
                    Ok(Value::Integer(a % b))
                }
            },
            "imod",
        ),

        // ── Safe arithmetic (Proposal 0135) ──────────────────────────
        SafeDiv => safe_arith_div(&args),
        SafeMod => safe_arith_mod(&args),

        // ── Typed float arithmetic ────────────────────────────────────
        FAdd => float2(&args, |a, b| Value::Float(a + b), "fadd"),
        FSub => float2(&args, |a, b| Value::Float(a - b), "fsub"),
        FMul => float2(&args, |a, b| Value::Float(a * b), "fmul"),
        FDiv => float2(&args, |a, b| Value::Float(a / b), "fdiv"),
        FSqrt => float1(&args, |a| Value::Float(a.sqrt()), "fsqrt"),
        FSin => float1(&args, |a| Value::Float(a.sin()), "fsin"),
        FCos => float1(&args, |a| Value::Float(a.cos()), "fcos"),
        FExp => float1(&args, |a| Value::Float(a.exp()), "fexp"),
        FLog => float1(&args, |a| Value::Float(a.ln()), "flog"),
        FFloor => float1(&args, |a| Value::Float(a.floor()), "ffloor"),
        FCeil => float1(&args, |a| Value::Float(a.ceil()), "fceil"),
        FRound => float1(&args, |a| Value::Float(a.round()), "fround"),
        FTan => float1(&args, |a| Value::Float(a.tan()), "ftan"),
        FAsin => float1(&args, |a| Value::Float(a.asin()), "fasin"),
        FAcos => float1(&args, |a| Value::Float(a.acos()), "facos"),
        FAtan => float1(&args, |a| Value::Float(a.atan()), "fatan"),
        FSinh => float1(&args, |a| Value::Float(a.sinh()), "fsinh"),
        FCosh => float1(&args, |a| Value::Float(a.cosh()), "fcosh"),
        FTanh => float1(&args, |a| Value::Float(a.tanh()), "ftanh"),
        FTruncate => float1(&args, |a| Value::Float(a.trunc()), "ftruncate"),
        BitAnd => int2(&args, |a, b| Value::Integer(a & b), "bit_and"),
        BitOr => int2(&args, |a, b| Value::Integer(a | b), "bit_or"),
        BitXor => int2(&args, |a, b| Value::Integer(a ^ b), "bit_xor"),
        BitShl => int2(
            &args,
            |a, b| Value::Integer(a.wrapping_shl(masked_shift_amount(b))),
            "bit_shl",
        ),
        BitShr => int2(
            &args,
            |a, b| Value::Integer(a.wrapping_shr(masked_shift_amount(b))),
            "bit_shr",
        ),

        // ── Numeric helpers ───────────────────────────────────────────
        Abs => match &args[0] {
            Value::Integer(v) => Ok(Value::Integer(v.abs())),
            Value::Float(v) => Ok(Value::Float(v.abs())),
            other => Err(terr("abs", "Number", other)),
        },
        Min => numeric_min_max(&args, "min", true),
        Max => numeric_min_max(&args, "max", false),
        Neg => match &args[0] {
            Value::Integer(v) => Ok(Value::Integer(-v)),
            Value::Float(v) => Ok(Value::Float(-v)),
            other => Err(terr("neg", "Number", other)),
        },

        // ── Typed integer comparisons ─────────────────────────────────
        ICmpEq => int_cmp(&args, |a, b| a == b, "icmp_eq"),
        ICmpNe => int_cmp(&args, |a, b| a != b, "icmp_ne"),
        ICmpLt => int_cmp(&args, |a, b| a < b, "icmp_lt"),
        ICmpLe => int_cmp(&args, |a, b| a <= b, "icmp_le"),
        ICmpGt => int_cmp(&args, |a, b| a > b, "icmp_gt"),
        ICmpGe => int_cmp(&args, |a, b| a >= b, "icmp_ge"),

        // ── Typed float comparisons ───────────────────────────────────
        FCmpEq => float_cmp(&args, |a, b| a == b, "fcmp_eq"),
        FCmpNe => float_cmp(&args, |a, b| a != b, "fcmp_ne"),
        FCmpLt => float_cmp(&args, |a, b| a < b, "fcmp_lt"),
        FCmpLe => float_cmp(&args, |a, b| a <= b, "fcmp_le"),
        FCmpGt => float_cmp(&args, |a, b| a > b, "fcmp_gt"),
        FCmpGe => float_cmp(&args, |a, b| a >= b, "fcmp_ge"),

        // ── Deep structural comparison ────────────────────────────────
        CmpEq => Ok(Value::Boolean(args[0] == args[1])),
        CmpNe => Ok(Value::Boolean(args[0] != args[1])),

        // ── Array operations ──────────────────────────────────────────
        ArrayLen => match &args[0] {
            Value::Array(items) => Ok(Value::Integer(items.len() as i64)),
            other => Err(terr("array_len", "Array", other)),
        },
        ArrayGet => {
            let index = eint(&args[1], "array_get")?;
            match &args[0] {
                Value::Array(items) => {
                    if index < 0 || index as usize >= items.len() {
                        Ok(Value::None)
                    } else {
                        Ok(items[index as usize].clone())
                    }
                }
                other => Err(terr("array_get", "Array", other)),
            }
        }
        ArraySet => {
            let index = eint(&args[1], "array_set")?;
            match &args[0] {
                Value::Array(items) => {
                    if index < 0 || index as usize >= items.len() {
                        return Err(format!(
                            "array_set: index {} out of bounds for length {}",
                            index,
                            items.len()
                        ));
                    }
                    let mut items = items.clone();
                    Rc::make_mut(&mut items)[index as usize] = args[2].clone();
                    Ok(Value::Array(items))
                }
                other => Err(terr("array_set", "Array", other)),
            }
        }
        ArrayPush => {
            let mut args = args;
            let elem = args.swap_remove(1);
            let arr_obj = args.swap_remove(0);
            match arr_obj {
                Value::Array(mut arr) => {
                    Rc::make_mut(&mut arr).push(elem);
                    Ok(Value::Array(arr))
                }
                other => Err(terr("push", "Array", &other)),
            }
        }
        ArrayConcat => {
            let left = earr(&args[0], "concat")?;
            let right = earr(&args[1], "concat")?;
            let mut out = left.clone();
            Rc::make_mut(&mut out).extend(right.iter().cloned());
            Ok(Value::Array(out))
        }
        ArraySlice => {
            let arr = earr(&args[0], "slice")?;
            let start = eint(&args[1], "slice")?;
            let end = eint(&args[2], "slice")?;
            let len = arr.len() as i64;
            let start = if start < 0 { 0 } else { start as usize };
            let end = if end > len {
                len as usize
            } else {
                end as usize
            };
            if start >= end || start >= arr.len() {
                Ok(Value::Array(vec![].into()))
            } else {
                Ok(Value::Array(arr[start..end].to_vec().into()))
            }
        }

        // ── HAMT operations ───────────────────────────────────────────
        HamtGet => {
            let key = args[1]
                .to_hash_key()
                .ok_or_else(|| hkey_err("get", &args[1]))?;
            match &args[0] {
                Value::HashMap(node) => match rc_hamt::hamt_lookup(node, &key) {
                    Some(value) => Ok(Value::Some(Rc::new(value))),
                    None => Ok(Value::None),
                },
                other => Err(terr("get", "Map", other)),
            }
        }
        HamtSet => {
            let key = args[1]
                .to_hash_key()
                .ok_or_else(|| hkey_err("put", &args[1]))?;
            match &args[0] {
                Value::HashMap(node) => Ok(Value::HashMap(rc_hamt::hamt_insert(
                    node,
                    key,
                    args[2].clone(),
                ))),
                other => Err(terr("put", "Map", other)),
            }
        }
        HamtContains => {
            let key = args[1]
                .to_hash_key()
                .ok_or_else(|| hkey_err("has_key", &args[1]))?;
            match &args[0] {
                Value::HashMap(node) => {
                    Ok(Value::Boolean(rc_hamt::hamt_lookup(node, &key).is_some()))
                }
                other => Err(terr("has_key", "Map", other)),
            }
        }
        HamtDelete => {
            let node = ehamt(&args[0], "delete")?;
            let key = args[1]
                .to_hash_key()
                .ok_or_else(|| hkey_err("delete", &args[1]))?;
            Ok(Value::HashMap(rc_hamt::hamt_delete(node, &key)))
        }
        HamtKeys => {
            let node = ehamt(&args[0], "keys")?;
            let pairs = rc_hamt::hamt_iter(node);
            Ok(Value::Array(
                pairs
                    .iter()
                    .map(|(k, _)| hash_key_to_value(k))
                    .collect::<Vec<_>>()
                    .into(),
            ))
        }
        HamtValues => {
            let node = ehamt(&args[0], "values")?;
            let pairs = rc_hamt::hamt_iter(node);
            Ok(Value::Array(
                pairs.into_iter().map(|(_, v)| v).collect::<Vec<_>>().into(),
            ))
        }
        HamtMerge => {
            let node1 = ehamt(&args[0], "merge")?;
            let node2 = ehamt(&args[1], "merge")?;
            let pairs = rc_hamt::hamt_iter(node2);
            let mut result = Rc::clone(node1);
            for (k, v) in pairs {
                result = rc_hamt::hamt_insert(&result, k, v);
            }
            Ok(Value::HashMap(result))
        }
        HamtSize => {
            let node = ehamt(&args[0], "size")?;
            Ok(Value::Integer(rc_hamt::hamt_len(node) as i64))
        }

        // ── String operations ─────────────────────────────────────────
        StringLength => match &args[0] {
            Value::String(s) => Ok(Value::Integer(s.len() as i64)),
            other => Err(terr("string_length", "String", other)),
        },
        StringConcat => match (&args[0], &args[1]) {
            (Value::String(l), Value::String(r)) => Ok(Value::String(format!("{}{}", l, r).into())),
            (l, r) => Err(format!(
                "string_concat expects (String, String), got ({}, {})",
                l.type_name(),
                r.type_name()
            )),
        },
        StringSlice | Substring => {
            let s = estr(&args[0], "string_slice")?;
            let start = eint(&args[1], "string_slice")?;
            let end = eint(&args[2], "string_slice")?;
            let chars: Vec<char> = s.chars().collect();
            let len = chars.len() as i64;
            let start = if start < 0 { 0 } else { start as usize };
            let end = if end < 0 {
                0
            } else if end > len {
                len as usize
            } else {
                end as usize
            };
            if start >= end || start >= chars.len() {
                Ok(Value::String(String::new().into()))
            } else {
                Ok(Value::String(
                    chars[start..end].iter().collect::<String>().into(),
                ))
            }
        }
        ToString => Ok(Value::String(format_value(&args[0]).into())),
        Split => {
            let s = estr(&args[0], "split")?;
            let delim = estr(&args[1], "split")?;
            let parts: Vec<Value> = if delim.is_empty() {
                s.chars()
                    .map(|c| Value::String(c.to_string().into()))
                    .collect()
            } else {
                s.split(delim)
                    .map(|p| Value::String(p.to_string().into()))
                    .collect()
            };
            Ok(Value::Array(parts.into()))
        }
        Trim => Ok(Value::String(
            estr(&args[0], "trim")?.trim().to_string().into(),
        )),
        Upper => Ok(Value::String(
            estr(&args[0], "upper")?.to_uppercase().into(),
        )),
        Lower => Ok(Value::String(
            estr(&args[0], "lower")?.to_lowercase().into(),
        )),
        Replace => Ok(Value::String(
            estr(&args[0], "replace")?
                .replace(estr(&args[1], "replace")?, estr(&args[2], "replace")?)
                .into(),
        )),

        // ── Type tag inspection ───────────────────────────────────────
        IsInt => Ok(Value::Boolean(matches!(args[0], Value::Integer(_)))),
        IsFloat => Ok(Value::Boolean(matches!(args[0], Value::Float(_)))),
        IsString => Ok(Value::Boolean(matches!(args[0], Value::String(_)))),
        IsBool => Ok(Value::Boolean(matches!(args[0], Value::Boolean(_)))),
        IsArray => Ok(Value::Boolean(matches!(args[0], Value::Array(_)))),
        IsNone => Ok(Value::Boolean(matches!(args[0], Value::None))),
        IsSome => Ok(Value::Boolean(matches!(args[0], Value::Some(_)))),
        IsList => Ok(Value::Boolean(matches!(
            args[0],
            Value::None | Value::EmptyList | Value::Cons(_)
        ))),
        IsMap => Ok(Value::Boolean(matches!(args[0], Value::HashMap(_)))),
        TypeOf => {
            let name = match &args[0] {
                Value::Cons(_) => "List",
                Value::HashMap(_) => "Map",
                other => other.type_name(),
            };
            Ok(Value::String(name.to_string().into()))
        }

        // ── I/O ───────────────────────────────────────────────────────
        Print => {
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    print!(" ");
                }
                print!("{}", format_value(arg));
            }
            println!();
            Ok(Value::None)
        }
        Println => {
            println!("{}", format_value(&args[0]));
            Ok(Value::None)
        }
        DebugTrace => {
            // Debug output goes to stderr so program stdout stays clean for
            // piping to other tools. Matches GHC `Debug.Trace`, Rust `dbg!`,
            // Python's default logging behavior. The argument is expected to
            // be a pre-formatted string (the Flow.Debug wrappers call
            // `show()` on values before perform-ing the effect operation).
            eprintln!("{}", format_value(&args[0]));
            Ok(Value::None)
        }
        ReadFile => {
            let path = estr(&args[0], "read_file")?;
            let content = fs::read_to_string(path)
                .map_err(|e| format!("read_file failed for '{}': {}", path, e))?;
            Ok(Value::String(content.into()))
        }
        WriteFile => {
            let path = estr(&args[0], "write_file")?;
            let content = estr(&args[1], "write_file")?;
            fs::write(path, content)
                .map_err(|e| format!("write_file failed for '{}': {}", path, e))?;
            Ok(Value::None)
        }
        ReadStdin => {
            let mut input = String::new();
            std::io::stdin()
                .read_to_string(&mut input)
                .map_err(|e| format!("read_stdin failed: {}", e))?;
            Ok(Value::String(input.into()))
        }
        ReadLines => {
            let path = estr(&args[0], "read_lines")?;
            let content = fs::read_to_string(path)
                .map_err(|e| format!("read_lines failed for '{}': {}", path, e))?;
            let lines = content
                .lines()
                .map(|line| Value::String(line.trim_end_matches('\r').to_string().into()))
                .collect::<Vec<_>>();
            Ok(Value::Array(lines.into()))
        }

        // ── Control ───────────────────────────────────────────────────
        Unwrap => match &args[0] {
            Value::None => Err("unwrap called on None".into()),
            other => Ok(other.clone()),
        },
        Panic => Err(format!("panic: {}", args[0].to_string_value())),
        ClockNow => {
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_err(|e| format!("clock_now failed: {}", e))?;
            Ok(Value::Integer(now.as_millis() as i64))
        }
        Time => {
            let start = Instant::now();
            let _ = ctx
                .invoke_value(args[0].clone(), vec![])
                .map_err(|e| format!("time: callback error: {}", e))?;
            let elapsed_ms = start.elapsed().as_millis();
            Ok(Value::Integer(elapsed_ms.min(i64::MAX as u128) as i64))
        }
        Try => match ctx.invoke_value(args[0].clone(), vec![]) {
            Ok(val) => Ok(Value::Tuple(Rc::new(vec![
                Value::String("ok".to_string().into()),
                val,
            ]))),
            Err(msg) => Ok(Value::Tuple(Rc::new(vec![
                Value::String("error".to_string().into()),
                Value::String(Rc::new(msg)),
            ]))),
        },
        AssertThrows => {
            let expected_msg: Option<&str> = if args.len() >= 2 {
                match &args[1] {
                    Value::String(s) => Some(s.as_ref()),
                    _ => None,
                }
            } else {
                None
            };
            match ctx.invoke_value(args[0].clone(), vec![]) {
                Ok(_) => Err("assert_throws failed: function completed without error".into()),
                Err(msg) => match expected_msg {
                    Some(expected) if msg.contains(expected) => Ok(Value::None),
                    Some(expected) => Err(format!(
                        "assert_throws failed\n  expected error containing: {}\n  actual error: {}",
                        expected, msg
                    )),
                    None => Ok(Value::None),
                },
            }
        }

        // ── Parsing ───────────────────────────────────────────────────
        ParseInt => {
            let text = estr(&args[0], "parse_int")?;
            let parsed = text
                .trim()
                .parse::<i64>()
                .map_err(|_| format!("parse_int: could not parse '{}' as Int", text))?;
            Ok(Value::Integer(parsed))
        }
        // ── Polymorphic length ────────────────────────────────────────
        Len => match &args[0] {
            Value::String(s) => Ok(Value::Integer(s.len() as i64)),
            Value::Array(arr) => Ok(Value::Integer(arr.len() as i64)),
            Value::Tuple(t) => Ok(Value::Integer(t.len() as i64)),
            Value::None | Value::EmptyList => Ok(Value::Integer(0)),
            Value::Cons(_) => {
                let mut count: i64 = 0;
                let mut cur = &args[0];
                loop {
                    match cur {
                        Value::None | Value::EmptyList => break,
                        Value::Cons(cell) => {
                            count += 1;
                            cur = &cell.tail;
                        }
                        _ => break,
                    }
                }
                Ok(Value::Integer(count))
            }
            Value::HashMap(node) => Ok(Value::Integer(rc_hamt::hamt_len(node) as i64)),
            other => Err(terr("len", "String, Array, Tuple, or Map", other)),
        },

        // ── Effect handler ops (native-only, Koka-style yield model) ────
        EvvGet | EvvSet | FreshMarker | EvvInsert | YieldTo | YieldExtend | YieldPrompt
        | IsYielding | PerformDirect => Err(format!(
            "CorePrimOp {:?} is native-backend only (Koka yield model)",
            op
        )),

        // ── Concurrency (proposal 0174 D5-a) ─────────────────────────
        //
        // VM implementation is **sequential / degenerate**: spawn invokes
        // the closure synchronously on the calling thread and stashes the
        // result in a thread-local table keyed by a fresh task id. join
        // returns the stored value; cancel drops the entry and tags the id
        // as cancelled so a subsequent join surfaces an error.
        //
        // Real M:N parallelism (running on the
        // [`TaskScheduler`](crate::runtime::r#async::task_scheduler) worker
        // pool) is gated on the staticlib + extern-C bridge from D5-b/c.
        // VM `Value` is `Rc<...>` which is `!Send`, so genuine parallel
        // execution on the VM path needs a separate value-promotion story.
        TaskSpawn => {
            let result = ctx.invoke_value(args[0].clone(), vec![])?;
            let id = vm_task_state::store(result);
            Ok(Value::Integer(id))
        }
        TaskBlockingJoin => match &args[0] {
            Value::Integer(id) => vm_task_state::take(*id),
            other => Err(terr("task_blocking_join", "Int", other)),
        },
        TaskCancel => match &args[0] {
            Value::Integer(id) => {
                vm_task_state::cancel(*id);
                Ok(Value::None)
            }
            other => Err(terr("task_cancel", "Int", other)),
        },

        // ── Fiber / structured concurrency (proposal 0174 Phase 1b) ──
        //
        // VM path: sequentially-equivalent semantics (same model as Phase 1a
        // TaskSpawn). No OS-thread yields or stack-switching happen here.
        // Real M:N fiber switching lives in the native backend (Phase 1b-vi+).

        // run_async: install the Async boundary and drive the dispatch loop
        // (proposal 0174 Phase 1b-vi-b₂.1). The root fiber's body is the
        // `action` closure; FiberSleep parks the current fiber on a backend
        // request, the loop pumps completions and resumes parked fibers.
        // Behaviour is observably identical to b₁ for single-fiber programs;
        // multi-fiber overlap arrives in 1b-vi-b₂.2.
        FiberRunAsync => {
            let backend = vm_async::backend()?;
            let root = vm_fibers::enter_run_async();
            let prev_boundary =
                vm_fibers::set_boundary(ctx.current_frame_index(), ctx.current_sp());
            vm_fibers::set_root_with_body(root, args[0].clone());

            let result = vm_fibers::dispatch_loop(ctx, backend);

            vm_fibers::clear_root();
            vm_fibers::restore_boundary(prev_boundary);
            vm_fibers::exit_run_async(root);
            result
        }

        // yield_now: cooperative yield point. No-op on the VM sequential path.
        FiberYieldNow => Ok(Value::None),

        // sleep: capture the current fiber's continuation back to the
        // FiberRunAsync boundary, register a timer with the mio backend,
        // signal park-pending, then return Err to bail out to the dispatch
        // loop (proposal 0174 Phase 1b-vi-b₂.1). The dispatch loop reads
        // PENDING_PARK, moves the fiber into the suspended map, and resumes
        // it when the backend's completion arrives. Single-fiber timing is
        // unchanged from 1b-vi-a; multi-fiber overlap lands in b₂.2.
        FiberSleep => match &args[0] {
            Value::Integer(ms) if *ms >= 0 => {
                use crate::runtime::r#async::backend::AsyncBackend;
                let backend = vm_async::backend()?;
                let req = vm_async::alloc_request_id();
                backend.timer_start(req, *ms as u64);
                if let Some((boundary_frame, boundary_sp)) = vm_fibers::boundary() {
                    // Inside Async.run_async: capture continuation, signal
                    // park; dispatch loop resumes us when timer fires.
                    let cont = ctx.capture_to_fiber_boundary(boundary_frame, boundary_sp)?;
                    vm_fibers::signal_park(req, cont);
                    Err("__fiber_park__".to_string())
                } else {
                    // No boundary (e.g. `main() with Async` is the host
                    // handler). Fall back to blocking pump — same shape as
                    // 1b-vi-a. The OS-thread call stack is the continuation.
                    loop {
                        if let Some(c) = backend.next_completion() {
                            if c.request_id == req {
                                break;
                            }
                            return Err(format!(
                                "fiber_sleep: unexpected completion {:?}",
                                c.request_id
                            ));
                        }
                        std::thread::park_timeout(std::time::Duration::from_millis(1));
                    }
                    Ok(Value::None)
                }
            }
            Value::Integer(_) => Err("fiber_sleep: ms must be non-negative".to_string()),
            other => Err(terr("fiber_sleep", "Int", other)),
        },

        // fiber_suspend: on the VM path the "setup closure" is never needed —
        // there is no suspension, so we just return immediately. The closure is
        // discarded (Aether RC drops it). The caller (yield_now / sleep) has
        // already done the real work above via dedicated primops.
        FiberSuspend => Ok(Value::None),

        // fiber_fork: allocate a child fiber via the scheduler, then run its
        // body inline (preserving sequential semantics — child runs to
        // completion before parent resumes). b₂ replaces the inline call
        // with a real park/resume cycle.
        FiberFork => {
            let child = vm_fibers::spawn_child();
            vm_fibers::with_current(child, || ctx.invoke_value(args[0].clone(), vec![]))?;
            Ok(Value::None)
        }

        // fiber_both / fiber_race (proposal 0174 Phase 1b-vi-b₂.2): spawn
        // two child fibers, park the parent on a synthetic completion,
        // bail out to the dispatch loop. When children finish, the loop
        // routes synthetic completions through `on_fiber_done`, builds the
        // resume value (tuple for both, winner for race), and wakes the
        // parent.
        FiberBoth => {
            let (boundary_frame, boundary_sp) = vm_fibers::boundary().ok_or_else(|| {
                "fiber_both called outside Async.run_async — no boundary set".to_string()
            })?;
            let child_a = vm_fibers::spawn_child_with_body(args[0].clone());
            let child_b = vm_fibers::spawn_child_with_body(args[1].clone());
            let req = vm_async::alloc_request_id();
            vm_fibers::register_both_await(req.0, child_a, child_b);
            let cont = ctx.capture_to_fiber_boundary(boundary_frame, boundary_sp)?;
            vm_fibers::signal_park(req, cont);
            Err("__fiber_park__".to_string())
        }

        FiberRace => {
            let (boundary_frame, boundary_sp) = vm_fibers::boundary().ok_or_else(|| {
                "fiber_race called outside Async.run_async — no boundary set".to_string()
            })?;
            let child_a = vm_fibers::spawn_child_with_body(args[0].clone());
            let child_b = vm_fibers::spawn_child_with_body(args[1].clone());
            let req = vm_async::alloc_request_id();
            vm_fibers::register_race_await(req.0, vec![child_a, child_b]);
            let cont = ctx.capture_to_fiber_boundary(boundary_frame, boundary_sp)?;
            vm_fibers::signal_park(req, cont);
            Err("__fiber_park__".to_string())
        }

        // fiber_timeout (proposal 0174 Phase 1b-vi-b₂.2 follow-up): bound
        // `f` by `ms` ms. Spawns the body fiber, registers a backend timer
        // keyed on the SAME request id as the parent's await; whichever
        // fires first delivers the resume value (Some(body_result) or
        // None). Loser is left to its fate (cancellation is 1b-vi-c).
        FiberTimeout => {
            use crate::runtime::r#async::backend::AsyncBackend;
            let (boundary_frame, boundary_sp) = vm_fibers::boundary().ok_or_else(|| {
                "fiber_timeout called outside Async.run_async — no boundary set".to_string()
            })?;
            let ms = match &args[0] {
                Value::Integer(n) if *n >= 0 => *n as u64,
                Value::Integer(_) => {
                    return Err("fiber_timeout: ms must be non-negative".to_string());
                }
                other => return Err(terr("fiber_timeout", "Int", other)),
            };
            let body_child = vm_fibers::spawn_child_with_body(args[1].clone());
            let req = vm_async::alloc_request_id();
            vm_fibers::register_timeout_await(req.0, body_child);
            // Register the backend timer keyed on the SAME request id as
            // the parent's await — when the timer fires, the dispatch
            // loop's pump observes c.request_id == req, calls
            // try_route_timer_for_timeout (which sets resume = None),
            // then scheduler.complete wakes the parent.
            let backend = vm_async::backend()?;
            backend.timer_start(req, ms);
            let cont = ctx.capture_to_fiber_boundary(boundary_frame, boundary_sp)?;
            vm_fibers::signal_park(req, cont);
            Err("__fiber_park__".to_string())
        }

        // fiber_get_context: return a dummy integer context handle. Not
        // observable by user code on the VM path (used by scheduler internals).
        FiberGetContext => Ok(Value::Integer(0)),

        // fiber_fail: surface an async error as a VM runtime error string.
        // The error value is an ADT (AsyncError variant); format it as a string.
        FiberFail => Err(format!("AsyncError: {:?}", args[0])),

        // task_await: fiber-suspending join. On the VM path this is identical
        // to blocking_join — consult the task state table.
        TaskAwait => match &args[0] {
            Value::Integer(id) => vm_task_state::take(*id),
            other => Err(terr("task_await", "Int", other)),
        },

        // ── TCP primops (proposal 0174 Phase 1b-vii) ────────────────
        // On the VM path: blocking Rust std::net calls via vm_tcp module.
        // Returns an integer handle ID on success; propagates errors as
        // runtime error strings (mirrors Async `fail` semantics).
        TcpConnect => match (&args[0], &args[1]) {
            (Value::String(host), Value::Integer(port)) => {
                vm_tcp::tcp_connect(host.as_str(), *port).map(Value::Integer)
            }
            _ => Err(format!("tcp_connect: expected (String, Int)")),
        },

        TcpRead => match (&args[0], &args[1]) {
            (Value::Integer(handle), Value::Integer(max)) => {
                vm_tcp::tcp_read(*handle, *max).map(|s| Value::String(Rc::new(s)))
            }
            _ => Err(format!("tcp_read: expected (Int, Int)")),
        },

        TcpWriteAll => match (&args[0], &args[1]) {
            (Value::Integer(handle), Value::String(data)) => {
                vm_tcp::tcp_write_all(*handle, data.as_str()).map(|()| Value::None)
            }
            _ => Err(format!("tcp_write_all: expected (Int, String)")),
        },

        TcpClose => match &args[0] {
            Value::Integer(handle) => {
                vm_tcp::tcp_close(*handle);
                Ok(Value::None)
            }
            other => Err(terr("tcp_close", "Int", other)),
        },

        TcpListen => match (&args[0], &args[1]) {
            (Value::String(host), Value::Integer(port)) => {
                vm_tcp::tcp_listen(host.as_str(), *port).map(Value::Integer)
            }
            _ => Err(format!("tcp_listen: expected (String, Int)")),
        },

        TcpAccept => match &args[0] {
            Value::Integer(listener) => {
                vm_tcp::tcp_accept(*listener).map(Value::Integer)
            }
            other => Err(terr("tcp_accept", "Int", other)),
        },

        // ── Generic/structural ops (never emitted as OpPrimOp) ───────
        Add | Sub | Mul | Div | Mod | Not | Eq | NEq | Lt | Le | Gt | Ge | And | Or | Concat
        | Interpolate | MakeList | MakeArray | MakeTuple | MakeHash | Index => Err(format!(
            "CorePrimOp {:?} should not appear in OpPrimOp bytecode",
            op
        )),
    }
}

// ── Compact helper functions ─────────────────────────────────────────────────

fn terr(op: &str, expected: &str, got: &Value) -> String {
    format!(
        "primop {} expected {}, got {}",
        op,
        expected,
        got.type_name()
    )
}

fn hkey_err(op: &str, v: &Value) -> String {
    format!(
        "primop {} expects hashable key (String, Int, Bool), got {}",
        op,
        v.type_name()
    )
}

fn estr<'a>(v: &'a Value, op: &str) -> Result<&'a str, String> {
    match v {
        Value::String(s) => Ok(s.as_ref()),
        other => Err(terr(op, "String", other)),
    }
}

fn eint(v: &Value, op: &str) -> Result<i64, String> {
    match v {
        Value::Integer(n) => Ok(*n),
        other => Err(terr(op, "Int", other)),
    }
}

fn efloat(v: &Value, op: &str) -> Result<f64, String> {
    match v {
        Value::Float(n) => Ok(*n),
        other => Err(terr(op, "Float", other)),
    }
}

fn earr<'a>(v: &'a Value, op: &str) -> Result<&'a Rc<Vec<Value>>, String> {
    match v {
        Value::Array(a) => Ok(a),
        other => Err(terr(op, "Array", other)),
    }
}

fn ehamt<'a>(v: &'a Value, op: &str) -> Result<&'a Rc<rc_hamt::HamtNode>, String> {
    match v {
        Value::HashMap(n) => Ok(n),
        other => Err(terr(op, "Map", other)),
    }
}

fn int2(args: &[Value], f: impl FnOnce(i64, i64) -> Value, op: &str) -> Result<Value, String> {
    Ok(f(eint(&args[0], op)?, eint(&args[1], op)?))
}

fn int2_result(
    args: &[Value],
    f: impl FnOnce(i64, i64) -> Result<Value, String>,
    op: &str,
) -> Result<Value, String> {
    f(eint(&args[0], op)?, eint(&args[1], op)?)
}

fn float2(args: &[Value], f: impl FnOnce(f64, f64) -> Value, op: &str) -> Result<Value, String> {
    Ok(f(efloat(&args[0], op)?, efloat(&args[1], op)?))
}

fn float1(args: &[Value], f: impl FnOnce(f64) -> Value, op: &str) -> Result<Value, String> {
    Ok(f(efloat(&args[0], op)?))
}

fn masked_shift_amount(value: i64) -> u32 {
    (value as u64 & 63) as u32
}

fn int_cmp(args: &[Value], f: impl FnOnce(i64, i64) -> bool, op: &str) -> Result<Value, String> {
    Ok(Value::Boolean(f(eint(&args[0], op)?, eint(&args[1], op)?)))
}

fn float_cmp(args: &[Value], f: impl FnOnce(f64, f64) -> bool, op: &str) -> Result<Value, String> {
    Ok(Value::Boolean(f(
        efloat(&args[0], op)?,
        efloat(&args[1], op)?,
    )))
}

fn numeric_min_max(args: &[Value], op: &str, is_min: bool) -> Result<Value, String> {
    let (a_num, b_num) = match (&args[0], &args[1]) {
        (Value::Integer(x), Value::Integer(y)) => (*x as f64, *y as f64),
        (Value::Integer(x), Value::Float(y)) => (*x as f64, *y),
        (Value::Float(x), Value::Integer(y)) => (*x, *y as f64),
        (Value::Float(x), Value::Float(y)) => (*x, *y),
        (l, r) => {
            return Err(format!(
                "primop {} expects (Number, Number), got ({}, {})",
                op,
                l.type_name(),
                r.type_name()
            ));
        }
    };
    let result = if is_min {
        a_num.min(b_num)
    } else {
        a_num.max(b_num)
    };
    match (&args[0], &args[1]) {
        (Value::Integer(_), Value::Integer(_)) => Ok(Value::Integer(result as i64)),
        _ => Ok(Value::Float(result)),
    }
}

fn hash_key_to_value(key: &HashKey) -> Value {
    match key {
        HashKey::Integer(v) => Value::Integer(*v),
        HashKey::Boolean(v) => Value::Boolean(*v),
        HashKey::String(v) => Value::String(v.clone().into()),
    }
}

// ── Safe arithmetic (Proposal 0135) ─────────────────────────────────────────

fn safe_arith_div(args: &[Value]) -> Result<Value, String> {
    match (&args[0], &args[1]) {
        (Value::Integer(a), Value::Integer(b)) => {
            if *b == 0 {
                Ok(Value::None)
            } else {
                Ok(Value::Some(Rc::new(Value::Integer(a / b))))
            }
        }
        (Value::Float(a), Value::Float(b)) => {
            if *b == 0.0 {
                Ok(Value::None)
            } else {
                Ok(Value::Some(Rc::new(Value::Float(a / b))))
            }
        }
        (Value::Integer(a), Value::Float(b)) => {
            if *b == 0.0 {
                Ok(Value::None)
            } else {
                Ok(Value::Some(Rc::new(Value::Float(*a as f64 / b))))
            }
        }
        (Value::Float(a), Value::Integer(b)) => {
            if *b == 0 {
                Ok(Value::None)
            } else {
                Ok(Value::Some(Rc::new(Value::Float(a / *b as f64))))
            }
        }
        (a, b) => Err(format!(
            "safe_div expects (Number, Number), got ({}, {})",
            a.type_name(),
            b.type_name()
        )),
    }
}

fn safe_arith_mod(args: &[Value]) -> Result<Value, String> {
    match (&args[0], &args[1]) {
        (Value::Integer(a), Value::Integer(b)) => {
            if *b == 0 {
                Ok(Value::None)
            } else {
                Ok(Value::Some(Rc::new(Value::Integer(a % b))))
            }
        }
        (Value::Float(a), Value::Float(b)) => {
            if *b == 0.0 {
                Ok(Value::None)
            } else {
                Ok(Value::Some(Rc::new(Value::Float(a % b))))
            }
        }
        (Value::Integer(a), Value::Float(b)) => {
            if *b == 0.0 {
                Ok(Value::None)
            } else {
                Ok(Value::Some(Rc::new(Value::Float(*a as f64 % b))))
            }
        }
        (Value::Float(a), Value::Integer(b)) => {
            if *b == 0 {
                Ok(Value::None)
            } else {
                Ok(Value::Some(Rc::new(Value::Float(*a % *b as f64))))
            }
        }
        (a, b) => Err(format!(
            "safe_mod expects (Number, Number), got ({}, {})",
            a.type_name(),
            b.type_name()
        )),
    }
}

// ── VM-side task state (proposal 0174 D5-a, sequential dispatch) ───────────
//
// Thread-local because VM `Value` carries `Rc<...>` which is `!Send`. Genuine
// cross-worker tasks land in D5-b/c via the staticlib bridge to the Rust
// scheduler.
mod vm_task_state {
    use super::Value;
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};

    thread_local! {
        static NEXT_ID: RefCell<i64> = const { RefCell::new(1) };
        static RESULTS: RefCell<HashMap<i64, Value>> = RefCell::new(HashMap::new());
        static CANCELLED: RefCell<HashSet<i64>> = RefCell::new(HashSet::new());
    }

    /// Allocate a fresh task id and stash the closure's result against it.
    pub(super) fn store(v: Value) -> i64 {
        let id = NEXT_ID.with(|n| {
            let mut n = n.borrow_mut();
            let id = *n;
            *n += 1;
            id
        });
        RESULTS.with(|r| r.borrow_mut().insert(id, v));
        id
    }

    /// Consume the stored result for `id`. Returns an error if the task was
    /// cancelled (matches the [`TaskJoinError::Cancelled`] semantics from
    /// the Rust scheduler) or never existed.
    pub(super) fn take(id: i64) -> Result<Value, String> {
        if CANCELLED.with(|c| c.borrow_mut().remove(&id)) {
            // Drop any stored value too; cancellation wins.
            RESULTS.with(|r| r.borrow_mut().remove(&id));
            return Err(format!("task {id} was cancelled"));
        }
        RESULTS
            .with(|r| r.borrow_mut().remove(&id))
            .ok_or_else(|| format!("task {id} not found (already joined or never spawned)"))
    }

    /// Mark a task cancelled. Idempotent. A subsequent `take` surfaces the
    /// cancellation; if the task was already joined, the cancel is a no-op.
    pub(super) fn cancel(id: i64) {
        CANCELLED.with(|c| {
            c.borrow_mut().insert(id);
        });
    }
}
