use flux::diagnostics::Diagnostic as FluxDiagnostic;
use flux::diagnostics::position::{Position as FluxPosition, Span as FluxSpan};
use flux::diagnostics::types::Severity as FluxSeverity;
use lsp_types::{
    Diagnostic as LspDiagnostic, DiagnosticSeverity, NumberOrString, Position as LspPosition, Range,
};

/// Convert a Flux source position into an LSP position.
///
/// Flux: 1-based `line`, 0-based `column`.
/// LSP: 0-based `line`, 0-based `character`.
pub fn position_to_lsp(p: FluxPosition) -> LspPosition {
    LspPosition {
        line: p.line.saturating_sub(1) as u32,
        character: p.column as u32,
    }
}

pub fn span_to_range(s: FluxSpan) -> Range {
    Range {
        start: position_to_lsp(s.start),
        end: position_to_lsp(s.end),
    }
}

pub fn severity_to_lsp(s: FluxSeverity) -> DiagnosticSeverity {
    match s {
        FluxSeverity::Error => DiagnosticSeverity::ERROR,
        FluxSeverity::Warning => DiagnosticSeverity::WARNING,
        FluxSeverity::Note => DiagnosticSeverity::INFORMATION,
        FluxSeverity::Help => DiagnosticSeverity::HINT,
    }
}

pub fn diagnostic_to_lsp(d: &FluxDiagnostic) -> LspDiagnostic {
    let range = d.span().map(span_to_range).unwrap_or_default();
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
