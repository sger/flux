use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use lsp_types::{
    Diagnostic, DocumentDiagnosticReport, DocumentDiagnosticReportResult,
    FullDocumentDiagnosticReport, PreviousResultId, PublishDiagnosticsParams,
    RelatedFullDocumentDiagnosticReport, RelatedUnchangedDocumentDiagnosticReport,
    UnchangedDocumentDiagnosticReport, Uri, WorkspaceDiagnosticReport,
    WorkspaceDiagnosticReportResult, WorkspaceDocumentDiagnosticReport,
    WorkspaceFullDocumentDiagnosticReport, WorkspaceUnchangedDocumentDiagnosticReport,
};

use crate::convert::diagnostic_to_lsp;
use crate::snapshot::Snapshot;
use crate::workspace::Workspace;

/// A snapshot's diagnostics rendered to LSP form.
fn to_lsp(snapshot: &Snapshot) -> Vec<Diagnostic> {
    snapshot
        .diagnostics
        .iter()
        .map(|d| diagnostic_to_lsp(d, &snapshot.position_map))
        .collect()
}

/// Push-model diagnostics for `textDocument/publishDiagnostics`.
pub fn build(uri: &Uri, version: i32, snapshot: &Snapshot) -> PublishDiagnosticsParams {
    PublishDiagnosticsParams {
        uri: uri.clone(),
        diagnostics: to_lsp(snapshot),
        version: Some(version),
    }
}

/// Pull-model report for `textDocument/diagnostic`.
///
/// Returns an `Unchanged` report when `previous_result_id` still matches the
/// current diagnostics — only a small id crosses the wire and the client keeps
/// what it has. Otherwise a `Full` report tagged with a fresh content-derived
/// id the client echoes back on its next pull.
pub fn report(
    snapshot: &Snapshot,
    previous_result_id: Option<&str>,
) -> DocumentDiagnosticReportResult {
    full_or_unchanged(to_lsp(snapshot), previous_result_id)
}

/// A `Full` report with no diagnostics — the answer for a document with no
/// snapshot (unknown or never opened). A pull result can't be null, so we
/// always return a concrete report.
pub fn empty_report() -> DocumentDiagnosticReportResult {
    full_or_unchanged(Vec::new(), None)
}

fn full_or_unchanged(
    items: Vec<Diagnostic>,
    previous_result_id: Option<&str>,
) -> DocumentDiagnosticReportResult {
    let result_id = content_id(&items);
    if previous_result_id == Some(result_id.as_str()) {
        return DocumentDiagnosticReport::Unchanged(RelatedUnchangedDocumentDiagnosticReport {
            related_documents: None,
            unchanged_document_diagnostic_report: UnchangedDocumentDiagnosticReport { result_id },
        })
        .into();
    }
    DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
        related_documents: None,
        full_document_diagnostic_report: FullDocumentDiagnosticReport {
            result_id: Some(result_id),
            items,
        },
    })
    .into()
}

/// A stable id derived from the rendered diagnostics: an unchanged set yields
/// the same id across pulls, which is what powers the `Unchanged` short-circuit.
fn content_id(items: &[Diagnostic]) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(items)
        .unwrap_or_default()
        .hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

// ─────────────────────────────────────────────────────────────────────────────
// Workspace pull (workspace/diagnostic)
// ─────────────────────────────────────────────────────────────────────────────

/// One file in a `workspace/diagnostic` sweep — owned `Send` data the worker
/// turns into a report without a `Workspace` borrow.
pub struct WorkspaceFile {
    uri: Uri,
    version: Option<i64>,
    snapshot: Arc<Snapshot>,
    previous_result_id: Option<String>,
}

/// Main-thread gather for `workspace/diagnostic`, pairing each file with the
/// result id the client last saw. The (cheap) report assembly then runs
/// off-thread in [`workspace_report`].
///
/// `scan_all` chooses the scope:
///
/// - `false` (default) — only *already-analyzed* files: open buffers plus the
///   modules they import (a file's component is analyzed together), via cached
///   [`Workspace::snapshot`]. A real project carries example/fixture/vendored
///   files (this very repo has ~1k, many that fail by design); reporting only
///   the working set keeps the pull useful without flooding the Problems panel
///   or reddening the explorer with errors from files the user never opened.
/// - `true` — every discovered `.flx`, force-analyzed via
///   [`Workspace::ensure_snapshot`]. Surfaces all project-wide errors at once
///   (and is what reddens folders for fail-by-design files); behind the
///   `flux.workspaceDiagnostics.scanAllFiles` setting for users who want it.
pub fn workspace_gather(
    workspace: &mut Workspace,
    previous: &[PreviousResultId],
    scan_all: bool,
) -> Vec<WorkspaceFile> {
    let seen: HashMap<&str, &str> = previous
        .iter()
        .map(|p| (p.uri.as_str(), p.value.as_str()))
        .collect();
    let mut out = Vec::new();
    for id in workspace.all_file_ids() {
        let snapshot = if scan_all {
            workspace.ensure_snapshot(id).cloned()
        } else {
            workspace.snapshot(id).cloned()
        };
        let Some(snapshot) = snapshot else {
            continue;
        };
        let Some(uri) = workspace.uri_of(id) else {
            continue;
        };
        let version = workspace.version(id).map(i64::from);
        let previous_result_id = seen.get(uri.as_str()).map(|s| s.to_string());
        out.push(WorkspaceFile {
            uri,
            version,
            snapshot,
            previous_result_id,
        });
    }
    out
}

/// Assemble the gathered files into a `workspace/diagnostic` report — one entry
/// per file, `Unchanged` where the result id still matches, `Full` otherwise.
pub fn workspace_report(files: &[WorkspaceFile]) -> WorkspaceDiagnosticReportResult {
    let items = files
        .iter()
        .map(|f| {
            workspace_doc_report(
                &f.uri,
                f.version,
                &f.snapshot,
                f.previous_result_id.as_deref(),
            )
        })
        .collect();
    WorkspaceDiagnosticReport { items }.into()
}

fn workspace_doc_report(
    uri: &Uri,
    version: Option<i64>,
    snapshot: &Snapshot,
    previous_result_id: Option<&str>,
) -> WorkspaceDocumentDiagnosticReport {
    let items = to_lsp(snapshot);
    let result_id = content_id(&items);
    if previous_result_id == Some(result_id.as_str()) {
        return WorkspaceUnchangedDocumentDiagnosticReport {
            uri: uri.clone(),
            version,
            unchanged_document_diagnostic_report: UnchangedDocumentDiagnosticReport { result_id },
        }
        .into();
    }
    WorkspaceFullDocumentDiagnosticReport {
        uri: uri.clone(),
        version,
        full_document_diagnostic_report: FullDocumentDiagnosticReport {
            result_id: Some(result_id),
            items,
        },
    }
    .into()
}
