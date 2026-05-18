use std::thread;

use crossbeam_channel::{Receiver, Sender, unbounded};
use lsp_types::{PublishDiagnosticsParams, Uri};

use crate::document::DocumentStore;
use crate::line_index::PositionEncoding;
use crate::workspace::{Workspace, WorkspaceRoot};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AnalysisGeneration(pub u64);

#[derive(Clone, Debug)]
pub struct OpenDocumentData {
    pub uri: Uri,
    pub version: i32,
    pub text: String,
}

#[derive(Clone, Debug)]
pub enum AnalysisReason {
    Startup,
    DidOpen,
    DidChange,
    DidSave,
    DidClose,
    WatchedFiles,
}

#[derive(Clone, Debug)]
pub struct AnalysisJob {
    pub generation: AnalysisGeneration,
    pub reason: AnalysisReason,
    pub roots: Vec<WorkspaceRoot>,
    pub open_documents: Vec<OpenDocumentData>,
    pub encoding: PositionEncoding,
    pub discover_on_first_open: bool,
}

pub struct AnalysisSnapshot {
    pub docs: DocumentStore,
    pub workspace: Workspace,
}

impl AnalysisSnapshot {
    pub fn diagnostics(&self) -> Vec<PublishDiagnosticsParams> {
        if self.workspace.is_empty() {
            return Vec::new();
        }
        self.workspace.diagnostics()
    }
}

pub struct AnalysisResult {
    pub generation: AnalysisGeneration,
    pub snapshot: AnalysisSnapshot,
}

// The Flux frontend stores some immutable analysis data behind `Rc`, which
// makes the aggregate snapshot `!Send` even though this worker hands ownership
// of a freshly-built snapshot to the main thread exactly once. The worker never
// shares a snapshot concurrently with the main thread; stale results are moved
// through the channel and either accepted or dropped.
unsafe impl Send for AnalysisResult {}

pub struct AnalysisWorker {
    sender: Sender<AnalysisJob>,
    receiver: Receiver<AnalysisResult>,
    handle: Option<thread::JoinHandle<()>>,
}

impl AnalysisWorker {
    pub fn start() -> Self {
        let (job_sender, job_receiver) = unbounded::<AnalysisJob>();
        let (result_sender, result_receiver) = unbounded::<AnalysisResult>();
        let handle = thread::spawn(move || {
            while let Ok(job) = job_receiver.recv() {
                let result = analyze(job);
                if result_sender.send(result).is_err() {
                    break;
                }
            }
        });
        Self {
            sender: job_sender,
            receiver: result_receiver,
            handle: Some(handle),
        }
    }

    pub fn send(&self, job: AnalysisJob) {
        let _ = self.sender.send(job);
    }

    pub fn try_recv(&self) -> Option<AnalysisResult> {
        self.receiver.try_recv().ok()
    }

    pub fn shutdown(mut self) {
        drop(self.sender);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn analyze(job: AnalysisJob) -> AnalysisResult {
    let mut docs = DocumentStore::new(job.encoding);
    let mut workspace = Workspace::new(job.roots, job.encoding);
    if job.discover_on_first_open {
        workspace.enable_first_open_discovery();
    }
    for doc in job.open_documents {
        docs.open(doc.uri.clone(), doc.version, doc.text.clone());
        workspace.open(&doc.uri, doc.version, doc.text);
    }
    AnalysisResult {
        generation: job.generation,
        snapshot: AnalysisSnapshot { docs, workspace },
    }
}
