use lsp_types::{PublishDiagnosticsParams, Uri};

use crate::convert::diagnostic_to_lsp;
use crate::snapshot::Snapshot;

pub fn build(uri: &Uri, version: i32, snapshot: &Snapshot) -> PublishDiagnosticsParams {
    let diagnostics = snapshot
        .diagnostics
        .iter()
        .map(|d| diagnostic_to_lsp(d, &snapshot.position_map))
        .collect();
    PublishDiagnosticsParams {
        uri: uri.clone(),
        diagnostics,
        version: Some(version),
    }
}
