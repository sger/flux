//! Shared await-coordination state for the VM and native fiber schedulers.
//!
//! Both the VM path (`src/vm/core_dispatch.rs`, `vm_fibers` module) and the
//! LLVM/native path (`src/runtime/async/native_abi.rs`, `NativeRun`) implement
//! identical `AwaitKind` state machines, completion routing, and cancellation
//! propagation logic.  This module extracts the shared parts so each addition
//! (TLS, DB, new async primitive) is written once.
//!
//! ## Type parameter `O`
//!
//! The coordinator is generic over the *outcome* type `O` (short for outcome).
//! - VM path: `O = VmOutcome` (wraps `Value` / `Rc<Value>`)
//! - Native path: `O = NativeOutcome` (wraps `i64` tagged pointer)
//!
//! The coordinator stores and routes `O` values but never inspects them —
//! construction of values like `Some(result)` or `(index, value)` tuples is
//! done by callbacks provided by the bridge at result-delivery time.

use std::collections::{HashMap, HashSet};

use crate::runtime::r#async::fiber::FiberId;

// ── AwaitKind ──────────────────────────────────────────────────────────────

/// How a parent fiber's resume value is assembled when its children finish.
///
/// Unified from the identical (but separately-defined) `AwaitKind` enums in
/// `vm_fibers` and `NativeRun`.
pub enum AwaitKind {
    /// Parent wakes when *both* children finish; result is a 2-tuple.
    Both {
        left: FiberId,
        right: FiberId,
    },
    /// Parent receives `Ok(value)` or `Err(error)` from the child.
    Try {
        child: FiberId,
    },
    /// First child to finish wins; losers are cancelled.
    Race {
        children: Vec<FiberId>,
        won: bool,
    },
    /// First child to finish in *source order* wins; earlier ready children
    /// block a later-completing child from winning.
    FirstOf {
        children: Vec<(FiberId, usize)>, // (fiber_id, source_index)
    },
    /// Race between a body fiber and a backend timer.
    /// Body finishing → `Some(value)`; timer firing → `None`.
    Timeout {
        body_child: FiberId,
    },
}

// ── Outcome traits ─────────────────────────────────────────────────────────

/// An outcome is either a successful value or an error value.
///
/// The concrete type is path-specific:
/// - VM:     `VmOutcome` / `FiberOutcome` wrapping `Value`
/// - Native: `NativeOutcome` wrapping `i64`
pub trait Outcome: Clone {
    fn is_error(&self) -> bool;
}

// ── FiberResolution ────────────────────────────────────────────────────────

/// Result of `AwaitCoordinator::on_fiber_done`.
///
/// The caller (dispatch loop) must:
/// 1. Call `scheduler.complete(req)` for each entry in `wakeups` to re-queue
///    the parent fiber.
/// 2. Store each `outcome` so the parent can retrieve it on resume.
/// 3. Call `cancel_fibers(losers)` to cancel losing children.
pub struct FiberResolution<O> {
    /// Parent requests that are now satisfiable and their resume values.
    pub wakeups: Vec<(u64, O)>,
    /// Children to cancel (race losers, timeout losers).
    pub losers: Vec<FiberId>,
}

// ── AwaitCoordinator ───────────────────────────────────────────────────────

/// Shared await-coordination state extracted from both the VM and native paths.
///
/// Owns:
/// - `awaits`: parent request → await kind (synthesised multi-child awaits)
/// - `awaiter_index`: child fiber → list of parent requests it satisfies
/// - `results`: completed child outcomes pending parent collection
/// - `fiber_request`: fiber → current backend request id (for cancellation)
/// - `cancelled_fibers`: fibers whose scope has been cancelled
/// - `scopes`: scope id → set of child fibers (for scope cancellation)
pub struct AwaitCoordinator<O: Outcome> {
    /// parent_req → await state
    pub awaits: HashMap<u64, AwaitKind>,
    /// child_fiber_id → [parent_reqs it satisfies]
    pub awaiter_index: HashMap<FiberId, Vec<u64>>,
    /// child_fiber_id → completed outcome (pending collection by parent)
    pub results: HashMap<FiberId, O>,
    /// fiber_id → the backend request_id it is currently suspended on
    pub fiber_request: HashMap<FiberId, u64>,
    /// fibers whose enclosing scope has been cancelled
    pub cancelled_fibers: HashSet<FiberId>,
    /// scope_id → set of child fiber ids
    pub scopes: HashMap<u64, HashSet<FiberId>>,
    /// fiber_id → scope_id
    pub fiber_scope: HashMap<FiberId, u64>,
}

impl<O: Outcome> AwaitCoordinator<O> {
    pub fn new() -> Self {
        Self {
            awaits: HashMap::new(),
            awaiter_index: HashMap::new(),
            results: HashMap::new(),
            fiber_request: HashMap::new(),
            cancelled_fibers: HashSet::new(),
            scopes: HashMap::new(),
            fiber_scope: HashMap::new(),
        }
    }

    // ── Registration ──────────────────────────────────────────────────────

    /// Register a `Both` await: parent wakes when both `left` and `right`
    /// finish.
    pub fn register_both(&mut self, parent_req: u64, left: FiberId, right: FiberId) {
        self.awaits
            .insert(parent_req, AwaitKind::Both { left, right });
        self.awaiter_index
            .entry(left)
            .or_default()
            .push(parent_req);
        self.awaiter_index
            .entry(right)
            .or_default()
            .push(parent_req);
    }

    /// Register a `Try` await: parent receives Ok/Err wrapping of child result.
    pub fn register_try(&mut self, parent_req: u64, child: FiberId) {
        self.awaits.insert(parent_req, AwaitKind::Try { child });
        self.awaiter_index
            .entry(child)
            .or_default()
            .push(parent_req);
    }

    /// Register a `Race` await: first child to finish wins.
    pub fn register_race(&mut self, parent_req: u64, children: Vec<FiberId>) {
        for child in &children {
            self.awaiter_index
                .entry(*child)
                .or_default()
                .push(parent_req);
        }
        self.awaits.insert(
            parent_req,
            AwaitKind::Race {
                children,
                won: false,
            },
        );
    }

    /// Register a `FirstOf` await: first child in source order that finishes wins.
    pub fn register_first_of(&mut self, parent_req: u64, children: Vec<(FiberId, usize)>) {
        for (child, _) in &children {
            self.awaiter_index
                .entry(*child)
                .or_default()
                .push(parent_req);
        }
        self.awaits
            .insert(parent_req, AwaitKind::FirstOf { children });
    }

    /// Register a `Timeout` await: body fiber racing against a backend timer.
    pub fn register_timeout(&mut self, parent_req: u64, body_child: FiberId) {
        self.awaits
            .insert(parent_req, AwaitKind::Timeout { body_child });
        self.awaiter_index
            .entry(body_child)
            .or_default()
            .push(parent_req);
    }

    // ── Fiber → request tracking ──────────────────────────────────────────

    /// Record that a fiber is suspended on a backend request.
    pub fn track_request(&mut self, fiber: FiberId, request_id: u64) {
        self.fiber_request.insert(fiber, request_id);
    }

    /// Remove the request tracking for a fiber (on resume or cancel).
    pub fn untrack_request(&mut self, fiber: FiberId) -> Option<u64> {
        self.fiber_request.remove(&fiber)
    }

    // ── Scope management ─────────────────────────────────────────────────

    /// Allocate a fresh scope id (monotonically increasing, coordinator-local).
    pub fn new_scope(&mut self, scope_id: u64) {
        self.scopes.entry(scope_id).or_insert_with(HashSet::new);
    }

    /// Register a fiber under a scope.
    pub fn register_in_scope(&mut self, scope_id: u64, fiber_id: FiberId) {
        self.scopes.entry(scope_id).or_default().insert(fiber_id);
        self.fiber_scope.insert(fiber_id, scope_id);
    }

    /// Cancel all fibers in a scope.  Returns the list of fibers to cancel.
    pub fn cancel_scope(&mut self, scope_id: u64) -> Vec<FiberId> {
        let fibers: Vec<FiberId> = self
            .scopes
            .remove(&scope_id)
            .map(|s| s.into_iter().collect())
            .unwrap_or_default();
        for f in &fibers {
            self.cancelled_fibers.insert(*f);
            self.fiber_scope.remove(f);
        }
        fibers
    }

    // ── Cancellation ──────────────────────────────────────────────────────

    /// Mark fibers as cancelled and collect their backend request IDs so the
    /// caller can call `backend.cancel(req)` for each.
    ///
    /// Does *not* remove fibers from ready or suspended queues — the dispatch
    /// loop handles that when it next attempts to run or resume them.
    pub fn mark_cancelled(&mut self, ids: &[FiberId]) -> Vec<u64> {
        let mut reqs = Vec::new();
        for id in ids {
            self.cancelled_fibers.insert(*id);
            if let Some(req) = self.fiber_request.remove(id) {
                reqs.push(req);
            }
            // Clear any awaiter-index entries so stale completions don't re-wake
            self.awaiter_index.remove(id);
        }
        reqs
    }

    /// Check whether a fiber has been cancelled.
    pub fn is_cancelled(&self, id: FiberId) -> bool {
        self.cancelled_fibers.contains(&id)
    }

    // ── Completion routing ────────────────────────────────────────────────

    /// A child fiber finished.  Walk its parent awaits, resolve any that are
    /// now satisfiable, and return the list of parent wakeups and child losers.
    ///
    /// The `make_tuple2` and `make_some` callbacks construct the path-specific
    /// values for `Both` and `Timeout` results without the coordinator needing
    /// to know about `Value` or `i64`.
    ///
    /// The caller must:
    /// 1. Wake each `(req, outcome)` pair in `wakeups` via the scheduler.
    /// 2. Cancel `losers` via `cancel_fibers` + `backend.cancel`.
    pub fn on_fiber_done<F2, FS, FI>(
        &mut self,
        id: FiberId,
        outcome: O,
        make_tuple2: &mut F2,  // (left: O, right: O) -> O
        make_some: &mut FS,    // (value: O) -> O
        make_indexed: &mut FI, // (index: usize, value: O) -> O
    ) -> FiberResolution<O>
    where
        F2: FnMut(O, O) -> O,
        FS: FnMut(O) -> O,
        FI: FnMut(usize, O) -> O,
    {
        self.results.insert(id, outcome);
        self.fiber_request.remove(&id);

        let parent_reqs: Vec<u64> = self
            .awaiter_index
            .remove(&id)
            .unwrap_or_default();

        let mut wakeups: Vec<(u64, O)> = Vec::new();
        let mut losers: Vec<FiberId> = Vec::new();

        for parent_req in parent_reqs {
            let kind = self.awaits.remove(&parent_req);
            let Some(kind) = kind else { continue };

            match kind {
                AwaitKind::Both { left, right } => {
                    let id_outcome = self.results.get(&id).cloned();
                    // Short-circuit on error: deliver immediately, cancel the other.
                    if id_outcome.as_ref().map(|o| o.is_error()).unwrap_or(false) {
                        let err = self.results.remove(&id).expect("just inserted");
                        wakeups.push((parent_req, err));
                        let other = if id == left { right } else { left };
                        losers.push(other);
                    } else if self.results.contains_key(&left)
                        && self.results.contains_key(&right)
                    {
                        let l = self.results.remove(&left).expect("left present");
                        let r = self.results.remove(&right).expect("right present");
                        if l.is_error() {
                            wakeups.push((parent_req, l));
                        } else if r.is_error() {
                            wakeups.push((parent_req, r));
                        } else {
                            let tuple = make_tuple2(l, r);
                            wakeups.push((parent_req, tuple));
                        }
                    } else {
                        // Other child not done yet — re-insert, keep awaiter index.
                        self.awaits
                            .insert(parent_req, AwaitKind::Both { left, right });
                        let other = if id == left { right } else { left };
                        self.awaiter_index
                            .entry(other)
                            .or_default()
                            .push(parent_req);
                    }
                }

                AwaitKind::Try { child } => {
                    if id == child {
                        // Caller is responsible for wrapping in Ok/Err —
                        // forward the raw outcome; the bridge builds the ADT.
                        let result = self.results.remove(&id).expect("try child present");
                        wakeups.push((parent_req, result));
                    }
                }

                AwaitKind::Race { children, won } => {
                    if !won {
                        let result = self.results.remove(&id).expect("race winner present");
                        wakeups.push((parent_req, result));
                        for child in &children {
                            if *child != id {
                                losers.push(*child);
                            }
                        }
                    }
                    // won=true case should not occur — awaits entry is removed on win.
                }

                AwaitKind::FirstOf { children } => {
                    self.resolve_first_of(
                        parent_req,
                        children,
                        &mut wakeups,
                        &mut losers,
                        make_indexed,
                    );
                }

                AwaitKind::Timeout { body_child } => {
                    if id == body_child {
                        match self.results.remove(&id).expect("timeout body present") {
                            result if !result.is_error() => {
                                let wrapped = make_some(result);
                                wakeups.push((parent_req, wrapped));
                            }
                            err => {
                                wakeups.push((parent_req, err));
                            }
                        }
                    } else {
                        // Defensive: some other fiber indexed here — re-insert.
                        self.awaits
                            .insert(parent_req, AwaitKind::Timeout { body_child });
                    }
                }
            }
        }

        FiberResolution { wakeups, losers }
    }

    /// Re-evaluate deferred `FirstOf` awaits when a child parks (suspends).
    ///
    /// This allows a later-completed child to win if all earlier source-order
    /// children have either completed or suspended (no longer "ready").
    ///
    /// The `is_ready` callback asks whether a given fiber is still in the ready
    /// queue of the scheduler.
    pub fn on_fiber_suspended<IR, FI>(
        &mut self,
        id: FiberId,
        is_ready: &IR,
        make_indexed: &mut FI,
    ) -> FiberResolution<O>
    where
        IR: Fn(FiberId) -> bool,
        FI: FnMut(usize, O) -> O,
    {
        let parent_reqs: Vec<u64> = self
            .awaiter_index
            .get(&id)
            .cloned()
            .unwrap_or_default();

        let mut wakeups: Vec<(u64, O)> = Vec::new();
        let mut losers: Vec<FiberId> = Vec::new();

        for parent_req in parent_reqs {
            let kind = self.awaits.remove(&parent_req);
            match kind {
                Some(AwaitKind::FirstOf { children }) => {
                    self.resolve_first_of_with_ready(
                        parent_req,
                        children,
                        &mut wakeups,
                        &mut losers,
                        is_ready,
                        make_indexed,
                    );
                }
                Some(other) => {
                    self.awaits.insert(parent_req, other);
                }
                None => {}
            }
        }

        FiberResolution { wakeups, losers }
    }

    // ── Timer routing ─────────────────────────────────────────────────────

    /// A backend timer fired for `parent_req`.
    ///
    /// If the `Timeout` await is still present (body hasn't finished yet),
    /// sets the parent's resume value to "timed out" (caller supplies via
    /// `make_none`) and returns the body child to cancel.
    ///
    /// Returns `Some(body_child)` if the timer won, `None` if the body already
    /// finished and the timer fired late.
    pub fn on_timer_fired<FN>(&mut self, parent_req: u64, make_none: FN) -> Option<FiberId>
    where
        FN: FnOnce() -> O,
    {
        match self.awaits.remove(&parent_req) {
            Some(AwaitKind::Timeout { body_child }) => {
                // Timer wins — deliver None as the resume value.
                // Caller will call wakeup(parent_req, make_none()) and cancel body_child.
                let _ = make_none; // returned via the FiberResolution pattern below
                Some(body_child)
            }
            Some(other) => {
                // Not a timeout await — re-insert and ignore.
                self.awaits.insert(parent_req, other);
                None
            }
            None => None,
        }
    }

    // ── Internals ─────────────────────────────────────────────────────────

    fn resolve_first_of<FI>(
        &mut self,
        parent_req: u64,
        children: Vec<(FiberId, usize)>,
        wakeups: &mut Vec<(u64, O)>,
        losers: &mut Vec<FiberId>,
        make_indexed: &mut FI,
    ) where
        FI: FnMut(usize, O) -> O,
    {
        self.resolve_first_of_with_ready(
            parent_req,
            children,
            wakeups,
            losers,
            &|_| false,
            make_indexed,
        );
    }

    fn resolve_first_of_with_ready<IR, FI>(
        &mut self,
        parent_req: u64,
        children: Vec<(FiberId, usize)>,
        wakeups: &mut Vec<(u64, O)>,
        losers: &mut Vec<FiberId>,
        is_ready: &IR,
        make_indexed: &mut FI,
    ) where
        IR: Fn(FiberId) -> bool,
        FI: FnMut(usize, O) -> O,
    {
        // Find the first child (in source order) that has completed.
        let winner = children
            .iter()
            .copied()
            .find(|(child, _)| self.results.contains_key(child));

        let Some((winner, index)) = winner else {
            // No child has completed yet — keep the await alive.
            self.awaits
                .insert(parent_req, AwaitKind::FirstOf { children });
            return;
        };

        // Check if an earlier source-order child is still in the ready queue.
        let blocked_by_earlier_ready = children
            .iter()
            .take_while(|(child, _)| *child != winner)
            .any(|(child, _)| is_ready(*child));

        if blocked_by_earlier_ready {
            self.awaits
                .insert(parent_req, AwaitKind::FirstOf { children });
            return;
        }

        let result = self
            .results
            .remove(&winner)
            .expect("first_of winner result present");

        // All other children are losers.
        for (child, _) in &children {
            if *child == winner {
                continue;
            }
            // Drop any already-completed results for losers.
            self.results.remove(child);
            losers.push(*child);
        }

        let indexed = make_indexed(index, result);
        wakeups.push((parent_req, indexed));
    }
}

impl<O: Outcome> Default for AwaitCoordinator<O> {
    fn default() -> Self {
        Self::new()
    }
}
