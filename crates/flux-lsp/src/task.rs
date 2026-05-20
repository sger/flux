//! Off-thread execution of read requests.
//!
//! The main loop builds a per-request job — a `Send` closure that owns the
//! `Arc<Snapshot>`(s) it needs — and hands it to a single worker thread. The
//! worker runs the job and writes the JSON-RPC `Response`. A generation
//! counter lets the worker skip requests whose snapshot was already
//! superseded by a newer edit before the job started, rather than spend time
//! computing a result the client will discard.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crossbeam_channel::{Receiver, Sender};
use lsp_server::{Message, RequestId, Response};

/// LSP error code: the document changed before the request could be served
/// (the client should re-issue the request). Spec value `-32801`.
const CONTENT_MODIFIED: i32 = -32801;
/// LSP error code: the request was cancelled by the client. Spec value `-32800`.
const REQUEST_CANCELLED: i32 = -32800;

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

/// Run each [`WorkItem`] from `rx`, skipping ones already cancelled or stale,
/// and write every `Response` to `sender`. Returns when `rx` is disconnected
/// (the main thread dropped its sender on shutdown).
pub fn worker_loop(rx: Receiver<WorkItem>, sender: Sender<Message>, cancel: Cancellation) {
    while let Ok(item) = rx.recv() {
        let response = if cancel.take_cancelled(&item.id) {
            Response::new_err(item.id, REQUEST_CANCELLED, "request cancelled".to_string())
        } else if item.generation < cancel.generation() {
            // A newer edit landed before this job started — don't compute a
            // result the client would have to remap anyway.
            Response::new_err(item.id, CONTENT_MODIFIED, "document changed".to_string())
        } else {
            Response::new_ok(item.id, (item.compute)())
        };
        if sender.send(Message::Response(response)).is_err() {
            break;
        }
    }
}
