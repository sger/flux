//! Shared await coordination for structured async combinators.
//!
//! The VM and native runtimes have different value representations, but the
//! bookkeeping for "which child woke which parent request" is identical.  This
//! module keeps that state machine generic and returns host-owned outcomes for
//! the caller to wrap, release, or enqueue.

use std::collections::HashMap;
use std::hash::Hash;

#[derive(Debug, Clone)]
enum AwaitKind<F, O, T> {
    Both {
        left: F,
        right: F,
        left_value: Option<O>,
        right_value: Option<O>,
    },
    Try {
        child: F,
        meta: T,
    },
    Race {
        children: Vec<F>,
        completed: Vec<(F, O)>,
    },
    FirstOf {
        children: Vec<(F, usize)>,
        completed: Vec<(F, usize, O)>,
    },
    Timeout {
        body: F,
    },
}

#[derive(Debug)]
pub enum AwaitEvent<F, O, T> {
    BothReady {
        request: u64,
        left: O,
        right: O,
    },
    BothError {
        request: u64,
        error: O,
        loser: F,
        discarded: Vec<O>,
    },
    TryReady {
        request: u64,
        outcome: O,
        meta: T,
    },
    RaceReady {
        request: u64,
        outcome: O,
        losers: Vec<F>,
        discarded: Vec<O>,
    },
    FirstOfReady {
        request: u64,
        index: usize,
        outcome: O,
        losers: Vec<F>,
        discarded: Vec<O>,
    },
    TimeoutBodyReady {
        request: u64,
        outcome: O,
    },
    TimeoutTimerReady {
        request: u64,
        body: F,
    },
}

#[derive(Debug)]
pub struct AwaitCoordinator<F, O, T = ()> {
    awaits: HashMap<u64, AwaitKind<F, O, T>>,
    awaiter_index: HashMap<F, Vec<u64>>,
}

impl<F, O, T> Default for AwaitCoordinator<F, O, T>
where
    F: Copy + Eq + Hash,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<F, O, T> AwaitCoordinator<F, O, T>
where
    F: Copy + Eq + Hash,
{
    pub fn new() -> Self {
        Self {
            awaits: HashMap::new(),
            awaiter_index: HashMap::new(),
        }
    }

    pub fn clear(&mut self) {
        self.awaits.clear();
        self.awaiter_index.clear();
    }

    /// Remove a pending await registration by parent request id.
    /// Called when a parent fiber aborts at park time (cancelled before
    /// parking) so orphaned child completions are silently discarded
    /// rather than trying to wake a parent that will never consume them.
    pub fn remove(&mut self, parent_req: u64) {
        if let Some(kind) = self.awaits.remove(&parent_req) {
            let children: Vec<F> = match kind {
                AwaitKind::Both { left, right, .. } => vec![left, right],
                AwaitKind::Try { child, .. } => vec![child],
                AwaitKind::Race { children, .. } => children,
                AwaitKind::FirstOf { children, .. } => {
                    children.into_iter().map(|(f, _)| f).collect()
                }
                AwaitKind::Timeout { body } => vec![body],
            };
            for child in children {
                self.unindex_child_request(child, parent_req);
            }
        }
    }

    pub fn remove_child(&mut self, child: F) {
        self.awaiter_index.remove(&child);
    }

    pub fn register_both(&mut self, request: u64, left: F, right: F) {
        self.awaits.insert(
            request,
            AwaitKind::Both {
                left,
                right,
                left_value: None,
                right_value: None,
            },
        );
        self.index_child(left, request);
        self.index_child(right, request);
    }

    pub fn register_try(&mut self, request: u64, child: F, meta: T) {
        self.awaits.insert(request, AwaitKind::Try { child, meta });
        self.index_child(child, request);
    }

    pub fn register_race(&mut self, request: u64, children: Vec<F>) {
        for child in &children {
            self.index_child(*child, request);
        }
        self.awaits.insert(
            request,
            AwaitKind::Race {
                children,
                completed: Vec::new(),
            },
        );
    }

    pub fn register_first_of(&mut self, request: u64, children: Vec<(F, usize)>) {
        for (child, _) in &children {
            self.index_child(*child, request);
        }
        self.awaits.insert(
            request,
            AwaitKind::FirstOf {
                children,
                completed: Vec::new(),
            },
        );
    }

    pub fn register_timeout(&mut self, request: u64, body: F) {
        self.awaits.insert(request, AwaitKind::Timeout { body });
        self.index_child(body, request);
    }

    pub fn route_timeout_timer(&mut self, request: u64) -> Option<AwaitEvent<F, O, T>> {
        match self.awaits.remove(&request) {
            Some(AwaitKind::Timeout { body }) => {
                self.unindex_child_request(body, request);
                Some(AwaitEvent::TimeoutTimerReady { request, body })
            }
            Some(other) => {
                self.awaits.insert(request, other);
                None
            }
            None => None,
        }
    }

    pub fn record_suspended(
        &mut self,
        child: F,
        mut is_ready: impl FnMut(F) -> bool,
    ) -> Vec<AwaitEvent<F, O, T>> {
        let parent_reqs = self.awaiter_index.get(&child).cloned().unwrap_or_default();
        let mut events = Vec::new();
        for request in parent_reqs {
            match self.awaits.remove(&request) {
                Some(AwaitKind::Race {
                    children,
                    completed,
                }) => self.resolve_race(request, children, completed, &mut is_ready, &mut events),
                Some(AwaitKind::FirstOf {
                    children,
                    completed,
                }) => {
                    self.resolve_first_of(request, children, completed, &mut is_ready, &mut events)
                }
                Some(other) => {
                    self.awaits.insert(request, other);
                }
                None => {}
            }
        }
        events
    }

    pub fn record_completed(
        &mut self,
        child: F,
        outcome: O,
        mut is_ready: impl FnMut(F) -> bool,
        mut is_error: impl FnMut(&O) -> bool,
    ) -> Vec<AwaitEvent<F, O, T>> {
        let parent_reqs = self.awaiter_index.remove(&child).unwrap_or_default();
        let mut pending_outcome = Some(outcome);
        let mut events = Vec::new();

        for request in parent_reqs {
            let Some(kind) = self.awaits.remove(&request) else {
                continue;
            };
            match kind {
                AwaitKind::Both {
                    left,
                    right,
                    mut left_value,
                    mut right_value,
                } => {
                    let outcome = pending_outcome
                        .take()
                        .expect("completed outcome consumed by multiple awaiters");
                    if child == left {
                        left_value = Some(outcome);
                    } else if child == right {
                        right_value = Some(outcome);
                    }

                    let child_value = if child == left {
                        left_value.as_ref()
                    } else {
                        right_value.as_ref()
                    };
                    if child_value.is_some_and(&mut is_error) {
                        let error = if child == left {
                            left_value.take().expect("left just set")
                        } else {
                            right_value.take().expect("right just set")
                        };
                        let loser = if child == left { right } else { left };
                        let discarded = left_value
                            .into_iter()
                            .chain(right_value.into_iter())
                            .collect();
                        self.unindex_child_request(loser, request);
                        events.push(AwaitEvent::BothError {
                            request,
                            error,
                            loser,
                            discarded,
                        });
                    } else if left_value.is_some() && right_value.is_some() {
                        events.push(AwaitEvent::BothReady {
                            request,
                            left: left_value.take().expect("left present"),
                            right: right_value.take().expect("right present"),
                        });
                    } else {
                        self.awaits.insert(
                            request,
                            AwaitKind::Both {
                                left,
                                right,
                                left_value,
                                right_value,
                            },
                        );
                    }
                }
                AwaitKind::Try {
                    child: expected,
                    meta,
                } => {
                    if child == expected {
                        let outcome = pending_outcome
                            .take()
                            .expect("try outcome consumed by multiple awaiters");
                        events.push(AwaitEvent::TryReady {
                            request,
                            outcome,
                            meta,
                        });
                    } else {
                        self.awaits.insert(
                            request,
                            AwaitKind::Try {
                                child: expected,
                                meta,
                            },
                        );
                    }
                }
                AwaitKind::Race {
                    children,
                    mut completed,
                } => {
                    let outcome = pending_outcome
                        .take()
                        .expect("race outcome consumed by multiple awaiters");
                    completed.push((child, outcome));
                    self.resolve_race(request, children, completed, &mut is_ready, &mut events);
                }
                AwaitKind::FirstOf {
                    children,
                    mut completed,
                } => {
                    let outcome = pending_outcome
                        .take()
                        .expect("first_of outcome consumed by multiple awaiters");
                    if let Some((_, index)) = children.iter().find(|(c, _)| *c == child) {
                        completed.push((child, *index, outcome));
                    }
                    self.resolve_first_of(request, children, completed, &mut is_ready, &mut events);
                }
                AwaitKind::Timeout { body } => {
                    if child == body {
                        let outcome = pending_outcome
                            .take()
                            .expect("timeout outcome consumed by multiple awaiters");
                        events.push(AwaitEvent::TimeoutBodyReady { request, outcome });
                    } else {
                        self.awaits.insert(request, AwaitKind::Timeout { body });
                    }
                }
            }
        }

        events
    }

    fn index_child(&mut self, child: F, request: u64) {
        self.awaiter_index.entry(child).or_default().push(request);
    }

    fn unindex_child_request(&mut self, child: F, request: u64) {
        if let Some(reqs) = self.awaiter_index.get_mut(&child) {
            reqs.retain(|r| *r != request);
            if reqs.is_empty() {
                self.awaiter_index.remove(&child);
            }
        }
    }

    fn resolve_race(
        &mut self,
        request: u64,
        children: Vec<F>,
        completed: Vec<(F, O)>,
        is_ready: &mut impl FnMut(F) -> bool,
        events: &mut Vec<AwaitEvent<F, O, T>>,
    ) {
        let winner = children.iter().copied().find(|child| {
            completed
                .iter()
                .any(|(completed_child, _)| completed_child == child)
        });

        let Some(winner) = winner else {
            self.awaits.insert(
                request,
                AwaitKind::Race {
                    children,
                    completed,
                },
            );
            return;
        };

        let blocked_by_earlier_ready = children
            .iter()
            .take_while(|child| **child != winner)
            .any(|child| is_ready(*child));

        if blocked_by_earlier_ready {
            self.awaits.insert(
                request,
                AwaitKind::Race {
                    children,
                    completed,
                },
            );
            return;
        }

        let mut outcome = None;
        let mut discarded = Vec::new();
        for (completed_child, completed_value) in completed {
            if completed_child == winner {
                outcome = Some(completed_value);
            } else {
                discarded.push(completed_value);
            }
        }

        let losers = children
            .into_iter()
            .filter(|child| *child != winner)
            .inspect(|child| self.unindex_child_request(*child, request))
            .collect();
        events.push(AwaitEvent::RaceReady {
            request,
            outcome: outcome.expect("race winner outcome present"),
            losers,
            discarded,
        });
    }

    fn resolve_first_of(
        &mut self,
        request: u64,
        children: Vec<(F, usize)>,
        completed: Vec<(F, usize, O)>,
        is_ready: &mut impl FnMut(F) -> bool,
        events: &mut Vec<AwaitEvent<F, O, T>>,
    ) {
        let winner = children.iter().copied().find(|(child, _)| {
            completed
                .iter()
                .any(|(completed_child, _, _)| completed_child == child)
        });

        let Some((winner, index)) = winner else {
            self.awaits.insert(
                request,
                AwaitKind::FirstOf {
                    children,
                    completed,
                },
            );
            return;
        };

        let blocked_by_earlier_ready = children
            .iter()
            .take_while(|(child, _)| *child != winner)
            .any(|(child, _)| is_ready(*child));

        if blocked_by_earlier_ready {
            self.awaits.insert(
                request,
                AwaitKind::FirstOf {
                    children,
                    completed,
                },
            );
            return;
        }

        let mut outcome = None;
        let mut discarded = Vec::new();
        for (completed_child, _, completed_value) in completed {
            if completed_child == winner {
                outcome = Some(completed_value);
            } else {
                discarded.push(completed_value);
            }
        }

        let losers = children
            .into_iter()
            .map(|(child, _)| child)
            .filter(|child| *child != winner)
            .inspect(|child| self.unindex_child_request(*child, request))
            .collect();
        events.push(AwaitEvent::FirstOfReady {
            request,
            index,
            outcome: outcome.expect("first_of winner outcome present"),
            losers,
            discarded,
        });
    }
}
