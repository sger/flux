use flux::diagnostics::Diagnostic as FluxDiagnostic;
use flux::diagnostics::types::Severity as FluxSeverity;
use lsp_types::{Diagnostic as LspDiagnostic, DiagnosticSeverity, NumberOrString};

use crate::line_index::PositionMap;

pub fn severity_to_lsp(s: FluxSeverity) -> DiagnosticSeverity {
    match s {
        FluxSeverity::Error => DiagnosticSeverity::ERROR,
        FluxSeverity::Warning => DiagnosticSeverity::WARNING,
        FluxSeverity::Note => DiagnosticSeverity::INFORMATION,
        FluxSeverity::Help => DiagnosticSeverity::HINT,
    }
}

pub fn diagnostic_to_lsp(d: &FluxDiagnostic, position_map: &PositionMap) -> LspDiagnostic {
    let range = d
        .span()
        .map(|s| position_map.flux_span_to_range(s))
        .unwrap_or_default();
    let message = match (d.message(), d.title()) {
        (Some(msg), title) if !title.is_empty() => format!("{title}: {msg}"),
        (Some(msg), _) => msg.to_string(),
        (None, title) => title.to_string(),
    };
    LspDiagnostic {
        range,
        severity: Some(severity_to_lsp(d.severity())),
        code: d.code().map(|c| NumberOrString::String(c.to_string())),
        source: Some("flux".to_string()),
        message,
        ..Default::default()
    }
}
