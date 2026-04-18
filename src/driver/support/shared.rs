use std::path::Path;

use crate::diagnostics::{
    Diagnostic, DiagnosticPhase, DiagnosticsAggregator, render_diagnostics_json,
};
use crate::driver::DiagnosticOutputFormat;
use crate::{
    aether::borrow_infer::{BorrowMode, BorrowProvenance},
    ast::type_infer::render_scheme_canonical,
    syntax::interner::Interner,
    types::scheme::Scheme,
};

/// Grouped rendering settings for driver diagnostic emission.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DiagnosticRenderRequest<'a> {
    pub(crate) diagnostics: &'a [Diagnostic],
    pub(crate) default_file: Option<&'a str>,
    pub(crate) default_source: Option<&'a str>,
    pub(crate) show_file_headers: bool,
    pub(crate) max_errors: usize,
    pub(crate) format: DiagnosticOutputFormat,
    pub(crate) all_errors: bool,
    pub(crate) text_to_stderr: bool,
}

/// Tags diagnostics with a phase when they are still missing one.
pub(crate) fn tag_diagnostics(diags: &mut [Diagnostic], phase: DiagnosticPhase) {
    for diag in diags {
        if diag.phase().is_none() {
            *diag = diag.clone().with_phase(phase);
        }
    }
}

/// Returns whether diagnostic text output should include file headers.
fn should_show_file_headers(diagnostics: &[Diagnostic], requested: bool) -> bool {
    if requested {
        return true;
    }

    let mut files = std::collections::BTreeSet::new();
    for diag in diagnostics {
        if let Some(file) = diag.file() {
            files.insert(file);
            if files.len() > 1 {
                return true;
            }
        }
    }

    false
}

/// Emits diagnostics using either text or JSON formatting according to the request.
pub(crate) fn emit_diagnostics(request: DiagnosticRenderRequest<'_>) {
    let show_file_headers =
        should_show_file_headers(request.diagnostics, request.show_file_headers);
    let mut agg = DiagnosticsAggregator::new(request.diagnostics)
        .with_file_headers(show_file_headers)
        .with_max_errors(Some(request.max_errors))
        .with_stage_filtering(!request.all_errors);
    if let Some(file) = request.default_file {
        if let Some(source) = request.default_source {
            agg = agg.with_default_source(file.to_string(), source.to_string());
        } else {
            agg = agg.with_default_file(file.to_string());
        }
    }

    match request.format {
        DiagnosticOutputFormat::Text => {
            let rendered = agg.report().rendered;
            if request.text_to_stderr {
                eprintln!("{}", rendered);
            } else {
                println!("{}", rendered);
            }
        }
        DiagnosticOutputFormat::Json => {
            let rendered = render_diagnostics_json(
                request.diagnostics,
                request.default_file,
                Some(request.max_errors),
                !request.all_errors,
                true,
            );
            eprintln!("{}", rendered);
        }
        DiagnosticOutputFormat::JsonCompact => {
            let rendered = render_diagnostics_json(
                request.diagnostics,
                request.default_file,
                Some(request.max_errors),
                !request.all_errors,
                false,
            );
            eprintln!("{}", rendered);
        }
    }
}

/// Shortens a long fingerprint to a user-facing prefix.
pub(crate) fn short_hash(hash: &str) -> &str {
    let len = hash.len().min(12);
    &hash[..len]
}

/// Returns the display name used for a module path in progress output.
pub(crate) fn module_display_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string()
}

/// Formats a single build progress line.
pub(crate) fn progress_line(n: usize, total: usize, action: &str, name: &str) -> String {
    let width = total.to_string().len();
    format!("[{:>width$} of {}] {:<10} {}", n, total, action, name)
}

/// Formats a polymorphic scheme for cache and interface inspection output.
pub(crate) fn format_scheme_for_cli(interner: &Interner, scheme: &Scheme) -> String {
    render_scheme_canonical(interner, scheme)
}

/// Formats a borrow mode for CLI-facing inspection output.
pub(crate) fn format_borrow_mode(mode: &BorrowMode) -> &'static str {
    match mode {
        BorrowMode::Owned => "Owned",
        BorrowMode::Borrowed => "Borrowed",
    }
}

/// Formats the provenance attached to a borrow signature.
pub(crate) fn format_borrow_provenance(provenance: BorrowProvenance) -> &'static str {
    match provenance {
        BorrowProvenance::Inferred => "Inferred",
        BorrowProvenance::BaseRuntime => "BaseRuntime",
        BorrowProvenance::Imported => "Imported",
        BorrowProvenance::Unknown => "Unknown",
    }
}
