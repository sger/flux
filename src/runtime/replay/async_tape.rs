//! Async code on the effect tape (proposal 0188 S1b).
//!
//! While a tape is active an async run has one worker, so its schedule
//! depends only on what the async backend hands back and when. The program's
//! backend therefore sends every submission through [`submit`] and every
//! completion through [`deliver`]:
//!
//! - recording writes each submission, and each completion where the
//!   scheduler received it;
//! - replaying checks each submission without performing it, and hands the
//!   scheduler the recorded completions at the same points.
//!
//! Requests and fibers are written with logical numbers, from 1 in the order
//! they first appear, so the tape does not depend on the runtime's global id
//! counters.
//!
//! With tasks (0188 S8c), each thread knows which task it runs
//! ([`enter_task`]); tasks and channels get logical numbers too; and a
//! completion another thread hands to the scheduler goes through
//! [`deliver_cross`], which puts it on the tape where the scheduler took it.

use std::cell::Cell;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use crate::runtime::r#async::backend::{Completion, CompletionPayload, IoHandle, RequestId};

use super::{
    EffectRequest, Outcome, ReplayMode, TapeBytes, TapeError, TapeEvent, TapeState, abandon,
    append, lock, mode, unsupported,
};

/// How long a replay waits for the program to reach the tape's next event
/// while the scheduler waits for a completion the tape does not have next.
const STALL_LIMIT: Duration = Duration::from_secs(2);

/// Logical numbering of requests and fibers for the active tape.
#[derive(Default)]
pub(super) struct Ids {
    req_logical: HashMap<u64, u64>,
    req_real: HashMap<u64, u64>,
    fibers: HashMap<u64, u32>,
    stalled_since: Option<Instant>,
    /// Tasks spawned so far, for numbering the next.
    tasks: u32,
    /// Real channel ids by logical number, and back.
    chan_logical: HashMap<u64, u64>,
}

static IDS: Mutex<Option<Ids>> = Mutex::new(None);

pub(super) fn reset_ids() {
    *ids_lock() = Some(Ids::default());
}

fn ids_lock() -> std::sync::MutexGuard<'static, Option<Ids>> {
    IDS.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn with_ids<R>(f: impl FnOnce(&mut Ids) -> R) -> R {
    f(ids_lock().get_or_insert_with(Ids::default))
}

/// The logical number for real request `req`, assigned on first sight.
fn logical_req(req: u64) -> u64 {
    with_ids(|ids| {
        let next = ids.req_logical.len() as u64 + 1;
        let logical = *ids.req_logical.entry(req).or_insert(next);
        ids.req_real.entry(logical).or_insert(req);
        logical
    })
}

fn real_req(logical: u64) -> Option<u64> {
    with_ids(|ids| ids.req_real.get(&logical).copied())
}

/// Gives request `req` its logical number now, when the scheduler makes it
/// rather than when it is first submitted: a request another thread
/// completes (a task's result, a channel hand-over) is never submitted, and
/// must still be numbered in an order replay repeats.
pub fn note_request(req: u64) {
    if super::is_active() {
        logical_req(req);
    }
}

// ── Tasks and channels (0188 S8c) ───────────────────────────────────────

thread_local! {
    /// The logical task this thread runs: 0 on the main program's thread.
    static CURRENT_TASK: Cell<u32> = const { Cell::new(0) };
}

/// This thread runs task `task` from now on (0 when it stops).
pub fn enter_task(task: u32) {
    let _ = CURRENT_TASK.try_with(|current| current.set(task));
}

/// The logical task running on this thread: 0 for the main program.
pub fn current_task() -> u32 {
    // At process exit this thread's locals may be gone: the main program.
    CURRENT_TASK.try_with(|current| current.get()).unwrap_or(0)
}

/// The logical number for the next task spawned. Spawns are ordered on the
/// tape, so a replay numbers them alike.
pub fn next_task() -> u32 {
    with_ids(|ids| {
        ids.tasks += 1;
        ids.tasks
    })
}

/// Whether the program has spawned a task under this tape: from then on,
/// channel operations are ordered, since another thread may use the channel.
pub fn has_tasks() -> bool {
    super::is_active() && with_ids(|ids| ids.tasks > 0)
}

/// The logical number of real channel `chan`, assigned on first sight.
pub fn logical_channel(chan: u64) -> u64 {
    with_ids(|ids| {
        let next = ids.chan_logical.len() as u64 + 1;
        *ids.chan_logical.entry(chan).or_insert(next)
    })
}

/// Takes the next completion another thread handed to the scheduler: a
/// task's result, or a value a channel or event passed on.
///
/// `take(None)` takes whichever completion is ready, without waiting;
/// `take(Some(req))` takes request `req`'s, waiting for it. Each returns
/// the real request id with the completion.
///
/// Off: the next one ready. Recording: the same, written to the tape as
/// `wake` where the scheduler took it. Replaying: only when the tape's next
/// event is this thread's `wake`, and then that request's, which the
/// thread handing it over has done by then, since it came first on the tape.
pub fn deliver_cross<T>(mut take: impl FnMut(Option<u64>) -> Option<(u64, T)>) -> Option<T> {
    match mode() {
        ReplayMode::Off => take(None).map(|(_, value)| value),
        ReplayMode::Record => {
            let _order = super::order_lock();
            let (req, value) = take(None)?;
            let req = logical_req(req);
            if let Err(err) = append(EffectRequest::Wake { req }, None) {
                fail(err);
            }
            Some(value)
        }
        ReplayMode::Replay => {
            let event = super::peek_own(|request| matches!(request, EffectRequest::Wake { .. }))?;
            let EffectRequest::Wake { req } = event.request else {
                unreachable!("peek_own matched a wake");
            };
            let Some(real) = real_req(req) else {
                fail(TapeError::PayloadMismatch {
                    index: event.seq,
                    what: "which request was handed over".to_string(),
                    tape: format!("request #{req}"),
                    program: "no such request made yet".to_string(),
                });
            };
            let (_, value) = take(Some(real))?;
            super::advance(&event);
            Some(value)
        }
    }
}

/// How long the scheduler waits for a completion the tape says another
/// thread has handed over before the replay is taken to have diverged.
pub const HAND_OVER_LIMIT: Duration = Duration::from_secs(30);

pub(super) fn progressed() {
    with_ids(|ids| ids.stalled_since = None);
}

// ── The current fiber ───────────────────────────────────────────────────

/// Reports the real id of the fiber running on this thread, if any.
pub type FiberSource = fn() -> Option<u64>;

static FIBER_SOURCE: RwLock<Option<FiberSource>> = RwLock::new(None);

/// Tells the tape how to find the running fiber. Each backend's async
/// runtime registers itself before it runs a program under a tape.
pub fn set_fiber_source(source: FiberSource) {
    *FIBER_SOURCE
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(source);
}

/// The logical number of the fiber running now: 0 outside `run_async`.
pub(super) fn current_fiber() -> u32 {
    let source = *FIBER_SOURCE
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(real) = source.and_then(|source| source()) else {
        return 0;
    };
    with_ids(|ids| {
        let next = ids.fibers.len() as u32 + 1;
        *ids.fibers.entry(real).or_insert(next)
    })
}

// ── Stopping on a tape error ────────────────────────────────────────────

/// Reports a tape error found inside the async runtime. It must not return:
/// the run cannot continue once it has left the tape.
pub type FatalHandler = Box<dyn Fn(&TapeError) + Send + Sync>;

static FATAL: RwLock<Option<FatalHandler>> = RwLock::new(None);

/// Sets how a tape error found inside the async runtime is reported before
/// the process exits.
pub fn set_fatal_handler(handler: FatalHandler) {
    *FATAL
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(handler);
}

/// Stops the run on `err`. Async work is found to diverge deep inside the
/// scheduler, where no error can be returned to the program.
pub fn fail(err: TapeError) -> ! {
    {
        let handler = FATAL
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match handler.as_ref() {
            Some(handler) => handler(&err),
            None => {
                let _ = super::finish(true, Some(err.message()));
                eprintln!("error[{}]: {}", err.code().code, err.message());
            }
        }
    }
    abandon();
    std::process::exit(1);
}

// ── Submissions ─────────────────────────────────────────────────────────

/// Work handed to the async backend.
#[derive(Debug, Clone)]
pub enum Submission {
    Timer { ms: u64 },
    Dns { host: String, port: u16 },
    Connect { addr: SocketAddr },
    Listen { addr: SocketAddr },
    Accept { handle: IoHandle },
    Read { handle: IoHandle, max: usize },
    Write { handle: IoHandle, bytes: Vec<u8> },
    Close { handle: IoHandle },
}

impl Submission {
    fn request(&self, req: u64) -> EffectRequest {
        match self {
            Submission::Timer { ms } => EffectRequest::TimerStart { req, ms: *ms },
            Submission::Dns { host, port } => EffectRequest::DnsResolve {
                req,
                host: host.clone(),
                port: *port,
            },
            Submission::Connect { addr } => EffectRequest::TcpConnect {
                req,
                addr: addr.to_string(),
            },
            Submission::Listen { addr } => EffectRequest::TcpListen {
                req,
                addr: addr.to_string(),
            },
            Submission::Accept { handle } => EffectRequest::TcpAccept {
                req,
                handle: handle.0,
            },
            Submission::Read { handle, max } => EffectRequest::TcpRead {
                req,
                handle: handle.0,
                max: *max as u64,
            },
            Submission::Write { handle, bytes } => EffectRequest::TcpWrite {
                req,
                handle: handle.0,
                bytes: TapeBytes(bytes.clone()),
            },
            Submission::Close { handle } => EffectRequest::TcpClose { handle: handle.0 },
        }
    }
}

/// Records or checks work handed to the async backend. Returns whether the
/// backend should perform it: yes unless replaying.
///
/// `req` is `None` for work that has no completion (`tcp_close`).
pub fn submit(req: Option<RequestId>, submission: Submission) -> bool {
    let logical = req.map(|req| logical_req(req.0)).unwrap_or(0);
    let request = submission.request(logical);
    match mode() {
        ReplayMode::Off => true,
        ReplayMode::Record => {
            if let Err(err) = append(request, None) {
                fail(err);
            }
            true
        }
        ReplayMode::Replay => {
            if let Err(err) = super::take_matching(&request) {
                fail(err);
            }
            false
        }
    }
}

// ── Completions ─────────────────────────────────────────────────────────

/// Takes the next completion for the scheduler. Off: from the backend.
/// Recording: from the backend, writing it to the tape. Replaying: from the
/// tape, when its next event is a completion, and otherwise none yet.
pub fn deliver(real_next: impl FnOnce() -> Option<Completion>) -> Option<Completion> {
    match mode() {
        ReplayMode::Off => real_next(),
        ReplayMode::Record => {
            let completion = real_next()?;
            let outcome = payload_outcome(&completion.payload).unwrap_or_else(|err| fail(err));
            let req = logical_req(completion.request_id.0);
            if let Err(err) = append(EffectRequest::AsyncComplete { req }, Some(outcome)) {
                fail(err);
            }
            Some(completion)
        }
        ReplayMode::Replay => replay_next(),
    }
}

fn replay_next() -> Option<Completion> {
    let next = {
        let mut guard = lock();
        let Some(TapeState::Replay { tape, cursor, .. }) = guard.as_mut() else {
            return None;
        };
        match tape.events.get(*cursor) {
            // Another thread's event, or a completion handed over from one:
            // not the backend's to deliver, and not a stall.
            Some(event)
                if event.task != current_task()
                    || matches!(event.request, EffectRequest::Wake { .. }) =>
            {
                drop(guard);
                progressed();
                return None;
            }
            Some(TapeEvent {
                request: EffectRequest::AsyncComplete { req },
                result: Some(outcome),
                seq,
                ..
            }) => {
                let found = (*seq, *req, outcome.clone());
                *cursor += 1;
                Ok(found)
            }
            Some(event) => Err(TapeError::KindMismatch {
                index: event.seq,
                expected: event.request.to_string(),
                actual: "waiting for an async completion".to_string(),
            }),
            None => Err(TapeError::Exhausted {
                count: tape.events.len() as u64,
                actual: "waiting for an async completion".to_string(),
            }),
        }
    };
    match next {
        Ok((seq, req, outcome)) => {
            progressed();
            let Some(real) = real_req(req) else {
                fail(TapeError::PayloadMismatch {
                    index: seq,
                    what: "which async request completed".to_string(),
                    tape: format!("request #{req}"),
                    program: "no such request submitted yet".to_string(),
                });
            };
            let payload = outcome_payload(outcome).unwrap_or_else(|reason| {
                fail(TapeError::PayloadMismatch {
                    index: seq,
                    what: "an async completion".to_string(),
                    tape: reason,
                    program: "a timer, socket or DNS result".to_string(),
                })
            });
            Some(Completion {
                request_id: RequestId(real),
                payload,
            })
        }
        Err(err) => {
            // Nothing else can run while the scheduler waits, so the program
            // will never reach the tape's next event: a diverged replay.
            // Polling loops ask more than once before they wait, so allow a
            // moment before deciding.
            let stalled = with_ids(|ids| {
                let since = *ids.stalled_since.get_or_insert_with(Instant::now);
                since.elapsed() >= STALL_LIMIT
            });
            if stalled {
                fail(err);
            }
            None
        }
    }
}

/// Whether a backend should cancel real work: not while replaying, when no
/// work was started.
pub fn cancels_real_work() -> bool {
    mode() != ReplayMode::Replay
}

fn payload_outcome(payload: &CompletionPayload) -> Result<Outcome, TapeError> {
    Ok(match payload {
        CompletionPayload::Unit => Outcome::Unit,
        CompletionPayload::Bytes(bytes) => Outcome::Bytes(TapeBytes(bytes.clone())),
        CompletionPayload::Error(message) => Outcome::Fail(message.clone()),
        CompletionPayload::TcpHandle(handle) => Outcome::Int(handle.0 as i64),
        CompletionPayload::AddressList(addrs) => {
            Outcome::Addrs(addrs.iter().map(|addr| addr.to_string()).collect())
        }
        // Filesystem work runs synchronously under a tape; it never reaches
        // the async backend.
        CompletionPayload::Filesystem(_) => return Err(unsupported("async filesystem work")),
    })
}

fn outcome_payload(outcome: Outcome) -> Result<CompletionPayload, String> {
    Ok(match outcome {
        Outcome::Unit => CompletionPayload::Unit,
        Outcome::Bytes(bytes) => CompletionPayload::Bytes(bytes.0),
        Outcome::Fail(message) => CompletionPayload::Error(message),
        Outcome::Int(handle) => CompletionPayload::TcpHandle(IoHandle(handle as u64)),
        Outcome::Addrs(addrs) => CompletionPayload::AddressList(
            addrs
                .iter()
                .map(|addr| addr.parse())
                .collect::<Result<_, _>>()
                .map_err(|_| "an unreadable address".to_string())?,
        ),
        other => return Err(other.to_string()),
    })
}

#[cfg(test)]
pub(super) fn payload_round_trip(payload: &CompletionPayload) -> Option<CompletionPayload> {
    outcome_payload(payload_outcome(payload).ok()?).ok()
}
