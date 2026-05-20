use std::str::FromStr;

use flux::diagnostics::Diagnostic as FluxDiagnostic;
use flux::diagnostics::types::Severity as FluxSeverity;
use lsp_types::{
    CodeDescription, Diagnostic as LspDiagnostic, DiagnosticSeverity, NumberOrString, Uri,
};

use crate::line_index::PositionMap;

/// Base URL of the error-code reference; a diagnostic's code becomes a
/// lowercased fragment anchor (`…/error_codes.md#e015`) that the doc defines
/// per code. Kept on `main` so the link is stable across branches.
const ERROR_DOCS_BASE: &str =
    "https://github.com/sger/flux/blob/main/docs/internals/error_codes.md";

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
        code_description: d.code().and_then(code_description),
        source: Some("flux".to_string()),
        message,
        ..Default::default()
    }
}

/// A "click for more info" link to the error-code reference for `code`, or
/// `None` for a token that isn't an `E…`/`W…` code (so we never emit a link
/// for a non-code). The fragment is the lowercased code; the doc anchors the
/// documented ones, and GitHub falls back to the page top for the rest.
fn code_description(code: &str) -> Option<CodeDescription> {
    if !is_doc_code(code) {
        return None;
    }
    let href = Uri::from_str(&format!("{ERROR_DOCS_BASE}#{}", code.to_lowercase())).ok()?;
    Some(CodeDescription { href })
}

/// An `E`/`W` prefix followed by at least one digit, all digits — the shape of
/// a Flux error/warning code (`E015`, `E1021`, `W201`).
fn is_doc_code(code: &str) -> bool {
    let Some(digits) = code.strip_prefix(['E', 'W']) else {
        return false;
    };
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_description_links_to_lowercased_anchor() {
        let cd = code_description("E015").expect("E015 should link");
        assert!(
            cd.href.as_str().ends_with("error_codes.md#e015"),
            "unexpected href: {}",
            cd.href.as_str()
        );
    }

    #[test]
    fn non_codes_get_no_description() {
        assert!(code_description("not-a-code").is_none());
        assert!(code_description("E").is_none());
        assert!(code_description("EXYZ").is_none());
    }
}
