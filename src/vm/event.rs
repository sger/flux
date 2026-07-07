use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::{Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use crate::runtime::{
    RuntimeContext,
    value::{AdtFields, AdtValue, Value},
};

use super::task::VmSendValue;

#[derive(Clone)]
enum VmEvent {
    Recv(i64),
    Send(i64, Value),
    SendMove(i64, VmSendValue),
    After(Instant),
    Always(Value),
    Never,
    Choose(Vec<i64>),
    Wrap(i64, Value),
    /// CML guard: a thunk `() -> Int` (returns the child event id), evaluated
    /// once at sync-time and memoized into `resolved` (proposal 0177 T2.5).
    Guard {
        thunk: Value,
        resolved: Option<i64>,
    },
    /// CML negative-ack: transparent wrapper over `child`; if `child`'s subtree
    /// does not contain the winning leaf at commit, the runtime fires `nack_ch`
    /// (sends Unit). `nack_ch` is a channel id the node neither owns nor frees.
    WithNack {
        nack_ch: i64,
        child: i64,
    },
}

thread_local! {
    static NEXT_EVENT_ID: RefCell<i64> = const { RefCell::new(1) };
    static EVENTS: RefCell<HashMap<i64, VmEvent>> = RefCell::new(HashMap::new());
}

struct WaitQueue {
    tx: mpsc::Sender<u64>,
    rx: Mutex<mpsc::Receiver<u64>>,
}

static WAIT_COMPLETIONS: OnceLock<WaitQueue> = OnceLock::new();
static ACTIVE_WAITS: OnceLock<Mutex<HashSet<u64>>> = OnceLock::new();

fn wait_queue() -> &'static WaitQueue {
    WAIT_COMPLETIONS.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        WaitQueue {
            tx,
            rx: Mutex::new(rx),
        }
    })
}

fn active_waits() -> &'static Mutex<HashSet<u64>> {
    ACTIVE_WAITS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn insert(event: VmEvent) -> i64 {
    let id = NEXT_EVENT_ID.with(|next| {
        let mut next = next.borrow_mut();
        let id = *next;
        *next += 1;
        id
    });
    EVENTS.with(|events| {
        events.borrow_mut().insert(id, event);
    });
    id
}

fn get(id: i64) -> Result<VmEvent, String> {
    EVENTS.with(|events| {
        events
            .borrow()
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("event {id} not found"))
    })
}

pub(super) fn recv(ch: i64) -> i64 {
    insert(VmEvent::Recv(ch))
}

pub(super) fn send(ch: i64, value: Value) -> i64 {
    insert(VmEvent::Send(ch, value))
}

pub(super) fn send_move(ch: i64, value: Value) -> Result<i64, String> {
    Ok(insert(VmEvent::SendMove(
        ch,
        VmSendValue::try_transfer_from_value(value)?,
    )))
}

pub(super) fn after(ms: i64) -> Result<i64, String> {
    if ms < 0 {
        return Err("event_after: ms must be non-negative".to_string());
    }
    Ok(insert(VmEvent::After(
        Instant::now() + Duration::from_millis(ms as u64),
    )))
}

pub(super) fn always(value: Value) -> i64 {
    insert(VmEvent::Always(value))
}

pub(super) fn never() -> i64 {
    insert(VmEvent::Never)
}

pub(super) fn choose(ids: Vec<i64>) -> Result<i64, String> {
    if ids.is_empty() {
        return Err("Event.choose called on empty list".to_string());
    }
    Ok(insert(VmEvent::Choose(ids)))
}

pub(super) fn wrap(id: i64, f: Value) -> i64 {
    insert(VmEvent::Wrap(id, f))
}

pub(super) fn guard(thunk: Value) -> i64 {
    insert(VmEvent::Guard {
        thunk,
        resolved: None,
    })
}

pub(super) fn with_nack(nack_ch: i64, child: i64) -> i64 {
    insert(VmEvent::WithNack { nack_ch, child })
}

pub(super) fn poll_value(ctx: &mut dyn RuntimeContext, id: i64) -> Result<Value, String> {
    if let Some((value, winner_leaf)) = poll(ctx, id)? {
        // CML nack (proposal 0177 T2.5): before tearing the committed tree
        // down, fire the nack channel of every `with_nack` branch that the
        // winning leaf does not belong to.
        fire_losing_nacks(id, winner_leaf)?;
        remove_tree(id);
        Ok(Value::Adt(Rc::new(AdtValue {
            constructor: Rc::new("Ready".to_string()),
            fields: AdtFields::One(value),
        })))
    } else {
        Ok(Value::AdtUnit(Rc::new("Pending".to_string())))
    }
}

pub(super) fn wait(id: i64, request_id: u64) -> Result<(), String> {
    active_waits()
        .lock()
        .expect("VM event active wait set poisoned")
        .insert(request_id);
    if is_ready(id)? {
        complete_wait(request_id);
        return Ok(());
    }
    register_wait(id, request_id)
}

pub(super) fn complete_wait(request_id: u64) {
    if active_waits()
        .lock()
        .expect("VM event active wait set poisoned")
        .remove(&request_id)
    {
        let _ = wait_queue().tx.send(request_id);
    }
}

pub(super) fn is_wait_active(request_id: u64) -> bool {
    active_waits()
        .lock()
        .expect("VM event active wait set poisoned")
        .contains(&request_id)
}

pub(super) fn try_recv_completion() -> Option<u64> {
    let rx = wait_queue()
        .rx
        .lock()
        .expect("VM event wait completion queue poisoned");
    match rx.try_recv() {
        Ok(request_id) => Some(request_id),
        Err(mpsc::TryRecvError::Empty) | Err(mpsc::TryRecvError::Disconnected) => None,
    }
}

fn remove_tree(id: i64) {
    let event = EVENTS.with(|events| events.borrow_mut().remove(&id));
    match event {
        Some(VmEvent::Choose(ids)) => {
            for child in ids {
                remove_tree(child);
            }
        }
        Some(VmEvent::Wrap(child, _)) => remove_tree(child),
        Some(VmEvent::Guard {
            resolved: Some(child),
            ..
        }) => remove_tree(child),
        // A `with_nack` node owns its `child` subtree but NOT its nack channel
        // (the channel table owns it; the cleanup fiber still reads it).
        Some(VmEvent::WithNack { child, .. }) => remove_tree(child),
        // Leaf events (and an unresolved Guard), including SendMove, have no
        // child IDs to recurse. Dropping the event releases any retained payload.
        _ => {}
    }
}

fn is_ready(id: i64) -> Result<bool, String> {
    match get(id)? {
        VmEvent::Recv(ch) => super::channel::can_recv(ch),
        VmEvent::Send(ch, _) | VmEvent::SendMove(ch, _) => super::channel::can_send(ch),
        VmEvent::After(deadline) => Ok(Instant::now() >= deadline),
        VmEvent::Always(_) => Ok(true),
        VmEvent::Never => Ok(false),
        VmEvent::Choose(ids) => {
            for child in ids {
                if is_ready(child)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        VmEvent::Wrap(child, _) => is_ready(child),
        // An unresolved guard is treated as not-ready: `sync_id` always polls
        // (which resolves it) before reaching the wait path, so by the time
        // `is_ready`/`register_wait` run the guard is resolved.
        VmEvent::Guard {
            resolved: Some(child),
            ..
        } => is_ready(child),
        VmEvent::Guard { resolved: None, .. } => Ok(false),
        VmEvent::WithNack { child, .. } => is_ready(child),
    }
}

fn register_wait(id: i64, request_id: u64) -> Result<(), String> {
    match get(id)? {
        VmEvent::Recv(ch) => super::channel::watch_recv(ch, request_id),
        VmEvent::Send(ch, _) | VmEvent::SendMove(ch, _) => {
            super::channel::watch_send(ch, request_id)
        }
        VmEvent::After(deadline) => {
            if Instant::now() >= deadline {
                complete_wait(request_id);
            } else {
                std::thread::spawn(move || {
                    std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
                    complete_wait(request_id);
                });
            }
            Ok(())
        }
        VmEvent::Always(_) => {
            complete_wait(request_id);
            Ok(())
        }
        VmEvent::Never => Ok(()),
        VmEvent::Choose(ids) => {
            for child in ids {
                register_wait(child, request_id)?;
            }
            Ok(())
        }
        VmEvent::Wrap(child, _) => register_wait(child, request_id),
        VmEvent::Guard {
            resolved: Some(child),
            ..
        } => register_wait(child, request_id),
        VmEvent::Guard { resolved: None, .. } => Ok(()),
        VmEvent::WithNack { child, .. } => register_wait(child, request_id),
    }
}

/// Poll the event tree at `id`. On readiness returns `(value, winner_leaf)`
/// where `winner_leaf` is the id of the leaf event that produced the value —
/// used by `fire_losing_nacks` at commit. Propagated unchanged through
/// `Choose` / `Wrap` / `Guard` / `WithNack`.
fn poll(ctx: &mut dyn RuntimeContext, id: i64) -> Result<Option<(Value, i64)>, String> {
    match get(id)? {
        VmEvent::Recv(ch) => {
            let value = super::channel::try_recv(ch)?;
            if !matches!(value, Value::None) {
                Ok(Some((value, id)))
            } else if super::channel::is_closed(ch)? {
                Ok(Some((Value::None, id)))
            } else {
                Ok(None)
            }
        }
        VmEvent::Send(ch, value) => {
            if super::channel::try_send(ch, &value)? || super::channel::is_closed(ch)? {
                Ok(Some((Value::None, id)))
            } else {
                Ok(None)
            }
        }
        VmEvent::SendMove(ch, value) => {
            if super::channel::try_send_value(ch, value)? || super::channel::is_closed(ch)? {
                Ok(Some((Value::None, id)))
            } else {
                Ok(None)
            }
        }
        VmEvent::After(deadline) => {
            if Instant::now() >= deadline {
                Ok(Some((Value::None, id)))
            } else {
                Ok(None)
            }
        }
        VmEvent::Always(value) => Ok(Some((value, id))),
        VmEvent::Never => Ok(None),
        VmEvent::Choose(ids) => {
            for child in ids {
                if let Some(found) = poll(ctx, child)? {
                    return Ok(Some(found));
                }
            }
            Ok(None)
        }
        VmEvent::Wrap(child, f) => {
            if let Some((value, leaf)) = poll(ctx, child)? {
                Ok(Some((ctx.invoke_value(f, vec![value])?, leaf)))
            } else {
                Ok(None)
            }
        }
        VmEvent::Guard { thunk, resolved } => {
            let child = match resolved {
                Some(child) => child,
                None => {
                    // Run the thunk exactly once, then memoize the child id.
                    // Never hold the EVENTS borrow across `invoke_value` (the
                    // thunk builds events via `insert`) — `get` already cloned
                    // the node out, so we are borrow-free here.
                    let child_val = ctx.invoke_value(thunk.clone(), vec![])?;
                    let child = match child_val {
                        Value::Integer(child) => child,
                        other => {
                            return Err(format!(
                                "event_guard thunk must return an event id (Int), got {}",
                                other.type_name()
                            ));
                        }
                    };
                    EVENTS.with(|events| {
                        events.borrow_mut().insert(
                            id,
                            VmEvent::Guard {
                                thunk,
                                resolved: Some(child),
                            },
                        );
                    });
                    child
                }
            };
            poll(ctx, child)
        }
        VmEvent::WithNack { child, .. } => poll(ctx, child),
    }
}

/// Walk the committed tree and fire the nack channel of every `with_nack`
/// branch whose subtree does not contain `winner_leaf` (proposal 0177 T2.5).
/// Called after `poll` succeeds, before `remove_tree`.
fn fire_losing_nacks(id: i64, winner_leaf: i64) -> Result<(), String> {
    match get(id)? {
        VmEvent::Choose(ids) => {
            for child in ids {
                fire_losing_nacks(child, winner_leaf)?;
            }
        }
        VmEvent::Wrap(child, _) => fire_losing_nacks(child, winner_leaf)?,
        VmEvent::Guard {
            resolved: Some(child),
            ..
        } => fire_losing_nacks(child, winner_leaf)?,
        VmEvent::WithNack { nack_ch, child } => {
            if !subtree_contains(child, winner_leaf)? {
                // Loser: fire the nack. Sending Value::None is fine — `try_recv`
                // wraps a buffered value in `Some`, so the cleanup fiber's recv
                // observes `Some(None)` (ready), distinct from the empty case.
                let _ = super::channel::try_send(nack_ch, &Value::None);
            }
            // Recurse for nested `with_nack` losers within this subtree.
            fire_losing_nacks(child, winner_leaf)?;
        }
        // Leaves and unresolved Guard: no nack-bearing children.
        _ => {}
    }
    Ok(())
}

/// True if the event subtree rooted at `id` contains the leaf `leaf`.
fn subtree_contains(id: i64, leaf: i64) -> Result<bool, String> {
    if id == leaf {
        return Ok(true);
    }
    match get(id)? {
        VmEvent::Choose(ids) => {
            for child in ids {
                if subtree_contains(child, leaf)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        VmEvent::Wrap(child, _) => subtree_contains(child, leaf),
        VmEvent::WithNack { child, .. } => subtree_contains(child, leaf),
        VmEvent::Guard {
            resolved: Some(child),
            ..
        } => subtree_contains(child, leaf),
        _ => Ok(false),
    }
}

pub(super) fn collect_ids(value: &Value) -> Result<Vec<i64>, String> {
    let mut out = Vec::new();
    let mut current = value.clone();
    loop {
        match current {
            Value::EmptyList | Value::None => return Ok(out),
            Value::Cons(cell) => {
                match &cell.head {
                    Value::Integer(id) => out.push(*id),
                    other => {
                        return Err(format!(
                            "event_choose expected List<Int>, got {}",
                            other.type_name()
                        ));
                    }
                }
                current = cell.tail.clone();
            }
            other => {
                return Err(format!(
                    "event_choose expected List<Int>, got {}",
                    other.type_name()
                ));
            }
        }
    }
}
