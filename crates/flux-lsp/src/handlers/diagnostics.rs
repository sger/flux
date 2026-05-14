use anyhow::Result;
use lsp_server::{Connection, Message, Notification};
use lsp_types::notification::Notification as _;
use lsp_types::{PublishDiagnosticsParams, Uri};

use crate::convert::diagnostic_to_lsp;
use crate::snapshot::Snapshot;

pub fn publish(conn: &Connection, uri: &Uri, version: i32, snapshot: &Snapshot) -> Result<()> {
    let diagnostics = snapshot.diagnostics.iter().map(diagnostic_to_lsp).collect();
    let params = PublishDiagnosticsParams {
        uri: uri.clone(),
        diagnostics,
        version: Some(version),
    };
    conn.sender.send(Message::Notification(Notification {
        method: lsp_types::notification::PublishDiagnostics::METHOD.to_string(),
        params: serde_json::to_value(params)?,
    }))?;
    Ok(())
}
