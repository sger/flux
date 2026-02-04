use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use super::{Diagnostic, RelatedDiagnostic, RelatedKind, Severity, render_display_path};
use crate::frontend::position::Span;

/// Default max error limit to avoid overwhelming output.
pub const DEFAULT_MAX_ERRORS: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DiagnosticCounts {
    pub errors: usize,
    pub warnings: usize,
    pub notes: usize,
    pub helps: usize,
}

impl DiagnosticCounts {
    pub fn total(&self) -> usize {
        self.errors + self.warnings + self.notes + self.helps
    }

    pub fn summary_line(&self) -> Option<String> {
        format_summary(self)
    }
}

#[derive(Debug, Clone)]
pub struct DiagnosticsReport {
    pub counts: DiagnosticCounts,
    pub rendered: String,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct SpanKey {
    start_line: usize,
    start_col: usize,
    end_line: usize,
    end_col: usize,
}

impl SpanKey {
    fn from_span(span: Span) -> Self {
        Self {
            start_line: span.start.line,
            start_col: span.start.column,
            end_line: span.end.line,
            end_col: span.end.column,
        }
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct RelatedKey {
    kind: RelatedKind,
    message: String,
    span: Option<SpanKey>,
    file: Option<String>,
}

impl RelatedKey {
    fn from_related(related: &RelatedDiagnostic) -> Self {
        Self {
            kind: related.kind,
            message: related.message.clone(),
            span: related.span.map(SpanKey::from_span),
            file: related.file.clone(),
        }
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct DiagnosticKey {
    file: Option<String>,
    span: Option<SpanKey>,
    severity: Severity,
    code: Option<String>,
    title: String,
    message: Option<String>,
    related: Vec<RelatedKey>,
}

impl DiagnosticKey {
    fn from_diagnostic(diag: &Diagnostic, default_file: Option<&str>) -> Self {
        let related = diag.related().iter().map(RelatedKey::from_related).collect();
        Self {
            file: effective_file(diag, default_file).map(|f| f.to_string()),
            span: diag.span().map(SpanKey::from_span),
            severity: diag.severity(),
            code: diag.code().map(|c| c.to_string()),
            title: diag.title().to_string(),
            message: diag.message().map(|m| m.to_string()),
            related,
        }
    }
}

#[derive(Debug)]
struct IndexedDiagnostic<'a> {
    index: usize,
    diag: &'a Diagnostic,
}

pub struct DiagnosticsAggregator<'a> {
    diagnostics: &'a [Diagnostic],
    max_errors: Option<usize>,
    default_file: Option<String>,
    sources: HashMap<String, String>,
}

impl<'a> DiagnosticsAggregator<'a> {
    pub fn new(diagnostics: &'a [Diagnostic]) -> Self {
        Self {
            diagnostics,
            max_errors: None,
            default_file: None,
            sources: HashMap::new(),
        }
    }

    pub fn with_max_errors(mut self, max_errors: Option<usize>) -> Self {
        self.max_errors = max_errors;
        self
    }

    pub fn with_default_file(mut self, file: impl Into<String>) -> Self {
        self.default_file = Some(file.into());
        self
    }

    pub fn with_source(mut self, file: impl Into<String>, source: impl Into<String>) -> Self {
        self.sources.insert(file.into(), source.into());
        self
    }

    pub fn with_default_source(
        mut self,
        file: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        let file = file.into();
        self.default_file = Some(file.clone());
        self.sources.insert(file, source.into());
        self
    }

    pub fn report(&self) -> DiagnosticsReport {
        if self.diagnostics.is_empty() {
            return DiagnosticsReport {
                counts: DiagnosticCounts::default(),
                rendered: String::new(),
            };
        }

        let default_file = self.default_file.as_deref();
        let mut seen: HashSet<DiagnosticKey> = HashSet::new();
        let mut unique: Vec<IndexedDiagnostic<'_>> = Vec::new();
        for (index, diag) in self.diagnostics.iter().enumerate() {
            let key = DiagnosticKey::from_diagnostic(diag, default_file);
            if seen.insert(key) {
                unique.push(IndexedDiagnostic { index, diag });
            }
        }

        let counts = count_severity(&unique);

        unique.sort_by(|a, b| {
            let a_file = effective_file(a.diag, default_file).unwrap_or("");
            let b_file = effective_file(b.diag, default_file).unwrap_or("");
            a_file
                .cmp(b_file)
                .then_with(|| line_key(a.diag).cmp(&line_key(b.diag)))
                .then_with(|| column_key(a.diag).cmp(&column_key(b.diag)))
                .then_with(|| severity_rank(a.diag.severity()).cmp(&severity_rank(b.diag.severity())))
                .then_with(|| message_key(a.diag).cmp(&message_key(b.diag)))
                .then_with(|| a.diag.title().cmp(b.diag.title()))
                .then_with(|| a.index.cmp(&b.index))
        });

        let mut rendered = String::new();
        if let Some(summary) = format_summary(&counts) {
            rendered.push_str(&summary);
            rendered.push_str("\n\n");
        }

        let mut file_cache: HashMap<String, String> = self.sources.clone();
        let mut errors_shown = 0usize;
        let max_errors = self.max_errors.unwrap_or(usize::MAX);

        let mut groups: Vec<String> = Vec::new();
        let mut current_file_key: Option<&str> = None;
        let mut current_group = String::new();
        let mut first_in_group = true;

        for indexed in &unique {
            let diag = indexed.diag;
            if diag.severity() == Severity::Error {
                if errors_shown >= max_errors {
                    continue;
                }
                errors_shown += 1;
            }

            let file_key = effective_file(diag, default_file).unwrap_or("");
            if current_file_key.map_or(true, |f| f != file_key) {
                if !current_group.is_empty() {
                    groups.push(current_group);
                    current_group = String::new();
                }
                current_file_key = Some(file_key);
                first_in_group = true;
                let display = file_display(effective_file(diag, default_file));
                current_group.push_str(&format!("--> {}\n", display));
            }

            if !first_in_group {
                current_group.push_str("\n\n");
            }
            first_in_group = false;

            let source = load_source(effective_file(diag, default_file), &mut file_cache);
            current_group.push_str(&diag.render(source, default_file));
        }

        if !current_group.is_empty() {
            groups.push(current_group);
        }

        rendered.push_str(&groups.join("\n\n"));

        let errors_truncated = counts.errors.saturating_sub(errors_shown);
        if errors_truncated > 0 {
            if !rendered.ends_with('\n') {
                rendered.push('\n');
            }
            rendered.push_str(&format!(
                "... and {} more errors not shown (use --max-errors to increase).\n",
                errors_truncated
            ));
        }

        DiagnosticsReport { counts, rendered }
    }

    pub fn render(&self) -> String {
        self.report().rendered
    }
}

pub fn render_diagnostics_multi(diagnostics: &[Diagnostic], max_errors: Option<usize>) -> String {
    DiagnosticsAggregator::new(diagnostics)
        .with_max_errors(max_errors)
        .render()
}

fn normalize_file<'a>(file: Option<&'a str>) -> Option<&'a str> {
    file.filter(|f| !f.is_empty())
}

fn effective_file<'a>(diag: &'a Diagnostic, default_file: Option<&'a str>) -> Option<&'a str> {
    normalize_file(diag.file()).or(normalize_file(default_file))
}

fn file_display<'a>(file: Option<&'a str>) -> Cow<'a, str> {
    file.filter(|f| !f.is_empty())
        .map(render_display_path)
        .unwrap_or_else(|| Cow::Borrowed("<unknown>"))
}

fn load_source<'a>(file: Option<&str>, cache: &'a mut HashMap<String, String>) -> Option<&'a str> {
    let file = file?;
    if file.is_empty() {
        return None;
    }
    if !cache.contains_key(file) {
        if let Ok(source) = fs::read_to_string(Path::new(file)) {
            cache.insert(file.to_string(), source);
        }
    }
    cache.get(file).map(|s| s.as_str())
}

fn count_severity(diags: &[IndexedDiagnostic<'_>]) -> DiagnosticCounts {
    let mut counts = DiagnosticCounts::default();
    for diag in diags {
        match diag.diag.severity() {
            Severity::Error => counts.errors += 1,
            Severity::Warning => counts.warnings += 1,
            Severity::Note => counts.notes += 1,
            Severity::Help => counts.helps += 1,
        }
    }
    counts
}

fn format_summary(counts: &DiagnosticCounts) -> Option<String> {
    let total = counts.total();
    if total <= 1 && !(counts.errors > 0 && counts.warnings > 0) {
        return None;
    }

    let mut parts = Vec::new();
    if counts.errors > 0 {
        parts.push(format!("{} error{}", counts.errors, plural(counts.errors)));
    }
    if counts.warnings > 0 {
        parts.push(format!("{} warning{}", counts.warnings, plural(counts.warnings)));
    }
    if counts.notes > 0 {
        parts.push(format!("{} note{}", counts.notes, plural(counts.notes)));
    }
    if counts.helps > 0 {
        parts.push(format!("{} help{}", counts.helps, plural(counts.helps)));
    }

    Some(format!("Found {}.", join_parts(&parts)))
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

fn join_parts(parts: &[String]) -> String {
    match parts.len() {
        0 => String::new(),
        1 => parts[0].clone(),
        2 => format!("{} and {}", parts[0], parts[1]),
        _ => {
            let mut all = parts.to_vec();
            let last = all.pop().unwrap();
            format!("{}, and {}", all.join(", "), last)
        }
    }
}

fn severity_rank(severity: Severity) -> u8 {
    match severity {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Note => 2,
        Severity::Help => 3,
    }
}

fn line_key(diag: &Diagnostic) -> usize {
    diag.position()
        .map(|pos| if pos.line == 0 { usize::MAX } else { pos.line })
        .unwrap_or(usize::MAX)
}

fn column_key(diag: &Diagnostic) -> usize {
    diag.position()
        .map(|pos| pos.column)
        .unwrap_or(usize::MAX)
}

fn message_key<'a>(diag: &'a Diagnostic) -> &'a str {
    diag.message().unwrap_or("")
}
