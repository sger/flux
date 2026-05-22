//! Off-thread execution of read requests.
//!
//! The main loop builds a per-request job — a `Send` closure that owns the
//! `Arc<Snapshot>`(s) it needs — and hands it to a pool of worker threads that
//! share one MPMC work queue. Each worker runs a job and writes the JSON-RPC
//! `Response`; because the pool has several threads, a slow request (say a
//! workspace-wide diagnostic sweep) occupies one worker while interactive
//! requests keep flowing through the others. A generation counter lets a worker
//! skip requests whose snapshot was already superseded by a newer edit before
//! the job started, rather than spend time computing a result the client will
//! discard.

use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender};
use lsp_server::{Message, RequestId, Response};

/// LSP error code: the document changed before the request could be served
/// (the client should re-issue the request). Spec value `-32801`.
const CONTENT_MODIFIED: i32 = -32801;
/// LSP error code: the request was cancelled by the client. Spec value `-32800`.
const REQUEST_CANCELLED: i32 = -32800;
/// JSON-RPC internal error: the request handler panicked. Spec value `-32603`.
const INTERNAL_ERROR: i32 = -32603;

/// A read-request computation: a `Send` closure capturing the `Arc<Snapshot>`
/// it needs, producing the JSON-RPC result value. Built on the main thread by
/// `GlobalState::dispatch_*`, run on the worker.
pub type Job = Box<dyn FnOnce() -> serde_json::Value + Send>;

/// A read request to run off the main thread.
pub struct WorkItem {
    pub id: RequestId,
    /// Workspace generation at dispatch time. If the workspace has advanced
    /// past this by the time the worker dequeues the item, the snapshot the
    /// job closed over is stale.
    pub generation: u64,
    /// Produces the JSON result value for the response.
    pub compute: Job,
}

/// Cancellation state shared between the main thread and the worker.
#[derive(Clone, Default)]
pub struct Cancellation {
    /// Latest workspace generation — bumped by the main thread on every
    /// content-mutating notification.
    generation: Arc<AtomicU64>,
    /// Request ids the client explicitly cancelled via `$/cancelRequest`.
    cancelled: Arc<Mutex<HashSet<RequestId>>>,
}

impl Cancellation {
    pub fn new() -> Self {
        Self::default()
    }

    /// Current workspace generation.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Record that the workspace content changed.
    pub fn bump_generation(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Record a `$/cancelRequest` for `id`.
    pub fn cancel(&self, id: RequestId) {
        if let Ok(mut set) = self.cancelled.lock() {
            set.insert(id);
        }
    }

    /// Whether `id` was cancelled; consumes the entry so the set stays small.
    fn take_cancelled(&self, id: &RequestId) -> bool {
        self.cancelled
            .lock()
            .map(|mut set| set.remove(id))
            .unwrap_or(false)
    }
}

/// Spawn a pool of `size` (at least one) worker threads, all running
/// [`worker_loop`] against clones of the same work queue and response sender.
/// Returns their join handles. Dropping the matching [`Sender<WorkItem>`]
/// disconnects `rx`, ending every worker so the handles can be joined on
/// shutdown.
pub fn spawn_worker_pool(
    rx: Receiver<WorkItem>,
    sender: Sender<Message>,
    cancel: Cancellation,
    size: usize,
) -> Vec<JoinHandle<()>> {
    (0..size.max(1))
        .map(|_| {
            let rx = rx.clone();
            let sender = sender.clone();
            let cancel = cancel.clone();
            std::thread::spawn(move || worker_loop(rx, sender, cancel))
        })
        .collect()
}

/// Run each [`WorkItem`] from `rx`, skipping ones already cancelled or stale,
/// and write every `Response` to `sender`. One of these runs per pool thread
/// (see [`spawn_worker_pool`]); they share `rx`, so each item is handled by
/// exactly one worker. Returns when `rx` is disconnected (the main thread
/// dropped its sender on shutdown).
pub fn worker_loop(rx: Receiver<WorkItem>, sender: Sender<Message>, cancel: Cancellation) {
    while let Ok(item) = rx.recv() {
        let WorkItem {
            id,
            generation,
            compute,
        } = item;
        let response = if cancel.take_cancelled(&id) {
            Response::new_err(id, REQUEST_CANCELLED, "request cancelled".to_string())
        } else if generation < cancel.generation() {
            // A newer edit landed before this job started — don't compute a
            // result the client would have to remap anyway.
            Response::new_err(id, CONTENT_MODIFIED, "document changed".to_string())
        } else {
            // A handler panic must not take down the single worker thread:
            // that would leave every later read request unanswered (the main
            // loop's `work_tx.send` would silently fail) and the editor would
            // appear to hang on hover/completion/goto until restart. Contain
            // the unwind and answer just this request with an internal error.
            // `AssertUnwindSafe` is sound here — jobs read an immutable
            // `Arc<Snapshot>`, and the one shared `Mutex` (the semantic-token
            // cache) is locked with poison recovery everywhere.
            match catch_unwind(AssertUnwindSafe(compute)) {
                Ok(value) => Response::new_ok(id, value),
                Err(payload) => {
                    let detail = crate::util::panic_message(payload.as_ref());
                    tracing::error!(request_id = ?id, "request handler panicked: {detail}");
                    Response::new_err(id, INTERNAL_ERROR, format!("internal error: {detail}"))
                }
            }
        };
        if sender.send(Message::Response(response)).is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expect_response(msg: Message) -> Response {
        match msg {
            Message::Response(resp) => resp,
            other => panic!("expected a response, got {other:?}"),
        }
    }

    /// A panicking job must answer with an internal error *and* leave the
    /// worker alive to serve the next request — otherwise one bad buffer
    /// would silently wedge every later hover/completion/goto.
    #[test]
    fn worker_survives_a_panicking_job_and_serves_the_next() {
        let (work_tx, work_rx) = crossbeam_channel::unbounded::<WorkItem>();
        let (resp_tx, resp_rx) = crossbeam_channel::unbounded::<Message>();
        let cancel = Cancellation::new();
        let worker = std::thread::spawn({
            let cancel = cancel.clone();
            move || worker_loop(work_rx, resp_tx, cancel)
        });

        work_tx
            .send(WorkItem {
                id: RequestId::from(1),
                generation: 0,
                compute: Box::new(|| panic!("boom")),
            })
            .unwrap();
        work_tx
            .send(WorkItem {
                id: RequestId::from(2),
                generation: 0,
                compute: Box::new(|| serde_json::json!("ok")),
            })
            .unwrap();

        let first = expect_response(resp_rx.recv().unwrap());
        assert_eq!(first.id, RequestId::from(1));
        let err = first.error.expect("panicking job should answer with an error");
        assert_eq!(err.code, INTERNAL_ERROR);
        assert!(err.message.contains("boom"));

        let second = expect_response(resp_rx.recv().unwrap());
        assert_eq!(second.id, RequestId::from(2));
        assert!(second.error.is_none());
        assert_eq!(second.result, Some(serde_json::json!("ok")));

        drop(work_tx);
        worker.join().unwrap();
    }

    /// With a pool, a slow job must not block a fast one queued behind it: the
    /// fast response should come back first. A single worker would force the
    /// fast response to wait out the slow job, so this would fail.
    #[test]
    fn pool_runs_jobs_concurrently() {
        let (work_tx, work_rx) = crossbeam_channel::unbounded::<WorkItem>();
        let (resp_tx, resp_rx) = crossbeam_channel::unbounded::<Message>();
        let cancel = Cancellation::new();
        let pool = spawn_worker_pool(work_rx, resp_tx, cancel, 2);

        // A slow job first, then a fast one.
        work_tx
            .send(WorkItem {
                id: RequestId::from(1),
                generation: 0,
                compute: Box::new(|| {
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    serde_json::json!("slow")
                }),
            })
            .unwrap();
        work_tx
            .send(WorkItem {
                id: RequestId::from(2),
                generation: 0,
                compute: Box::new(|| serde_json::json!("fast")),
            })
            .unwrap();

        let first = expect_response(resp_rx.recv().unwrap());
        assert_eq!(
            first.id,
            RequestId::from(2),
            "the fast job should respond before the slow one with a pool"
        );

        drop(work_tx);
        for worker in pool {
            worker.join().unwrap();
        }
    }
}
