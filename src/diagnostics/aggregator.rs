use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use super::{
    Diagnostic, Hint, HintChain, HintKind, InlineSuggestion, Label, LabelStyle, RelatedDiagnostic,
    RelatedKind, Severity, quality::downstream_errors_suppressed_note, render_display_path,
    types::DiagnosticPhase,
};
use crate::diagnostics::position::Span;

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
        format_summary(self, 1)
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
struct LabelKey {
    span: SpanKey,
    text: String,
    style: LabelStyle,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct HintKey {
    kind: HintKind,
    text: String,
    span: Option<SpanKey>,
    label: Option<String>,
    file: Option<String>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct SuggestionKey {
    replacement: String,
    span: SpanKey,
    message: Option<String>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct HintChainKey {
    steps: Vec<String>,
    conclusion: Option<String>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct DiagnosticKey {
    file: Option<String>,
    span: Option<SpanKey>,
    severity: Severity,
    code: Option<String>,
    title: String,
    message: Option<String>,
    labels: Vec<LabelKey>,
    hints: Vec<HintKey>,
    suggestions: Vec<SuggestionKey>,
    hint_chains: Vec<HintChainKey>,
    related: Vec<RelatedKey>,
}

impl DiagnosticKey {
    fn from_diagnostic(diag: &Diagnostic, default_file: Option<&str>) -> Self {
        let mut labels = diag
            .labels()
            .iter()
            .map(LabelKey::from_label)
            .collect::<Vec<_>>();
        labels.sort_by(label_sort);

        let mut hints = diag
            .hints()
            .iter()
            .map(HintKey::from_hint)
            .collect::<Vec<_>>();
        hints.sort_by(hint_sort);

        let mut suggestions = diag
            .suggestions()
            .iter()
            .map(SuggestionKey::from_suggestion)
            .collect::<Vec<_>>();
        suggestions.sort_by(suggestion_sort);

        let mut hint_chains = diag
            .hint_chains()
            .iter()
            .map(HintChainKey::from_chain)
            .collect::<Vec<_>>();
        hint_chains.sort_by(chain_sort);

        let mut related = diag
            .related()
            .iter()
            .map(RelatedKey::from_related)
            .collect::<Vec<_>>();
        related.sort_by(related_sort);
        Self {
            file: effective_file(diag, default_file).map(|f| f.to_string()),
            span: diag.span().map(SpanKey::from_span),
            severity: diag.severity(),
            code: diag.code().map(|c| c.to_string()),
            title: diag.title().to_string(),
            message: diag.message().map(|m| m.to_string()),
            labels,
            hints,
            suggestions,
            hint_chains,
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
    show_file_headers: Option<bool>,
    disable_stage_filtering: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct StageFilterStats {
    suppressed_type_count: usize,
    suppressed_effect_count: usize,
}

impl<'a> DiagnosticsAggregator<'a> {
    pub fn new(diagnostics: &'a [Diagnostic]) -> Self {
        Self {
            diagnostics,
            max_errors: None,
            default_file: None,
            sources: HashMap::new(),
            show_file_headers: None,
            disable_stage_filtering: false,
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

    /// Control file grouping headers in output.
    /// When unset, headers are shown for consistency even in single-file output.
    pub fn with_file_headers(mut self, show: bool) -> Self {
        self.show_file_headers = Some(show);
        self
    }

    pub fn with_stage_filtering(mut self, enabled: bool) -> Self {
        self.disable_stage_filtering = !enabled;
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

    fn unique_sorted_suppressed(&self) -> Vec<Diagnostic> {
        let default_file = self.default_file.as_deref();
        let mut seen: HashSet<DiagnosticKey> = HashSet::new();
        let mut unique: Vec<IndexedDiagnostic<'_>> = Vec::new();
        for (index, diag) in self.diagnostics.iter().enumerate() {
            let key = DiagnosticKey::from_diagnostic(diag, default_file);
            if seen.insert(key) {
                unique.push(IndexedDiagnostic { index, diag });
            }
        }

        unique.sort_by(|a, b| {
            let a_file = effective_file(a.diag, default_file).unwrap_or("");
            let b_file = effective_file(b.diag, default_file).unwrap_or("");
            a_file
                .cmp(b_file)
                .then_with(|| line_key(a.diag).cmp(&line_key(b.diag)))
                .then_with(|| column_key(a.diag).cmp(&column_key(b.diag)))
                .then_with(|| {
                    severity_rank(a.diag.severity()).cmp(&severity_rank(b.diag.severity()))
                })
                .then_with(|| message_key(a.diag).cmp(message_key(b.diag)))
                .then_with(|| a.diag.title().cmp(b.diag.title()))
                .then_with(|| a.index.cmp(&b.index))
        });

        suppress_nearby_duplicate_e300(unique, default_file)
    }

    fn apply_stage_filtering(
        &self,
        diagnostics: Vec<Diagnostic>,
    ) -> (Vec<Diagnostic>, StageFilterStats) {
        if self.disable_stage_filtering {
            return (diagnostics, StageFilterStats::default());
        }

        let has_parse_errors = diagnostics.iter().any(|d| {
            d.severity() == Severity::Error && matches!(d.phase(), Some(DiagnosticPhase::Parse))
        });
        let has_type_errors = diagnostics.iter().any(|d| {
            d.severity() == Severity::Error
                && matches!(
                    d.phase(),
                    Some(DiagnosticPhase::TypeInference | DiagnosticPhase::TypeCheck)
                )
        });

        let mut filtered = Vec::with_capacity(diagnostics.len());
        let mut stats = StageFilterStats::default();

        for diag in diagnostics {
            let suppress = match diag.phase() {
                None => false,
                Some(DiagnosticPhase::Runtime) => false,
                Some(DiagnosticPhase::Parse) => false,
                Some(DiagnosticPhase::ModuleGraph) => false,
                Some(DiagnosticPhase::Validation) => false,
                Some(DiagnosticPhase::TypeInference | DiagnosticPhase::TypeCheck) => {
                    has_parse_errors
                }
                Some(DiagnosticPhase::Effect) => has_parse_errors || has_type_errors,
            };

            if suppress {
                if diag.severity() == Severity::Error {
                    match diag.phase() {
                        Some(DiagnosticPhase::TypeInference | DiagnosticPhase::TypeCheck) => {
                            stats.suppressed_type_count += 1;
                        }
                        Some(DiagnosticPhase::Effect) => {
                            stats.suppressed_effect_count += 1;
                        }
                        _ => {}
                    }
                }
                continue;
            }

            filtered.push(diag);
        }

        (filtered, stats)
    }

    fn collapse_parser_cascades(&self, diagnostics: Vec<Diagnostic>) -> Vec<Diagnostic> {
        if self.disable_stage_filtering || diagnostics.len() <= 1 {
            return diagnostics;
        }

        let mut first_parse_idx = None;
        for (idx, diag) in diagnostics.iter().enumerate() {
            if matches!(diag.phase(), Some(DiagnosticPhase::Parse))
                && diag.severity() == Severity::Error
            {
                first_parse_idx = Some(idx);
                break;
            }
        }
        let Some(first_idx) = first_parse_idx else {
            return diagnostics;
        };

        let mut out = Vec::with_capacity(diagnostics.len());
        let mut root_file = effective_file(&diagnostics[first_idx], self.default_file.as_deref())
            .map(ToString::to_string);
        let mut root_line = diagnostics[first_idx]
            .span()
            .map(|s| s.start.line)
            .unwrap_or(0);
        let mut root_code = diagnostics[first_idx].code().map(ToString::to_string);
        let mut seen_parse_error = false;
        let mut suppressed_cascades = 0usize;

        for diag in diagnostics {
            let is_parse_error = matches!(diag.phase(), Some(DiagnosticPhase::Parse))
                && diag.severity() == Severity::Error;
            if !is_parse_error {
                out.push(diag);
                continue;
            }

            let file = effective_file(&diag, self.default_file.as_deref()).map(ToString::to_string);
            let line = diag.span().map(|s| s.start.line).unwrap_or(0);
            let is_generic = diag.code() == Some("E034");
            // Only collapse generic E034 errors that follow a *structural* root
            // error which corrupts the token stream (unterminated string, unclosed
            // delimiter).  Other root errors (e.g. E030 unknown keyword) recover
            // cleanly, so nearby E034s are likely independent issues.
            let root_is_structural = matches!(root_code.as_deref(), Some("E071" | "E076"));
            if seen_parse_error
                && is_generic
                && root_is_structural
                && file == root_file
                && line <= root_line.saturating_add(3)
            {
                suppressed_cascades += 1;
                continue;
            }

            root_file = file;
            root_line = line;
            root_code = diag.code().map(ToString::to_string);
            seen_parse_error = true;
            out.push(diag);
        }

        if suppressed_cascades > 0
            && let Some(last_parse) = out.iter_mut().rfind(|d| {
                matches!(d.phase(), Some(DiagnosticPhase::Parse)) && d.severity() == Severity::Error
            })
        {
            last_parse.hints.push(Hint::note(format!(
                "Suppressed {} cascading parser diagnostic(s) likely caused by an earlier syntax error.",
                suppressed_cascades
            )));
        }

        out
    }

    fn process_with_metadata(&self) -> (Vec<Diagnostic>, StageFilterStats) {
        let deduped = self.unique_sorted_suppressed();
        let (filtered, stats) = self.apply_stage_filtering(deduped);
        let collapsed = self.collapse_parser_cascades(filtered);
        (collapsed, stats)
    }

    fn apply_error_limit(&self, diagnostics: Vec<Diagnostic>) -> (Vec<Diagnostic>, usize) {
        let max_errors = self.max_errors.unwrap_or(usize::MAX);
        let mut errors_shown = 0usize;
        let mut shown = Vec::new();
        for diag in diagnostics {
            if diag.severity() == Severity::Error {
                if errors_shown >= max_errors {
                    continue;
                }
                errors_shown += 1;
            }
            shown.push(diag);
        }
        (shown, errors_shown)
    }

    /// Returns diagnostics after exact dedup + E300 neighborhood suppression
    /// + max-errors filtering, in deterministic render order.
    pub fn processed_diagnostics(&self) -> Vec<Diagnostic> {
        let (processed, _) = self.process_with_metadata();
        let (shown, _) = self.apply_error_limit(processed);
        shown
    }

    pub fn report(&self) -> DiagnosticsReport {
        if self.diagnostics.is_empty() {
            return DiagnosticsReport {
                counts: DiagnosticCounts::default(),
                rendered: String::new(),
            };
        }

        let default_file = self.default_file.as_deref();
        let (processed, stage_stats) = self.process_with_metadata();
        let counts = {
            let indexed_suppressed: Vec<IndexedDiagnostic<'_>> = processed
                .iter()
                .enumerate()
                .map(|(index, diag)| IndexedDiagnostic { index, diag })
                .collect();
            count_severity(&indexed_suppressed)
        };
        let file_count = processed
            .iter()
            .filter_map(|d| effective_file(d, default_file))
            .collect::<HashSet<_>>()
            .len();
        let (mut shown, errors_shown) = self.apply_error_limit(processed);

        let suppressed_type_count = stage_stats.suppressed_type_count;
        let suppressed_effect_count = stage_stats.suppressed_effect_count;
        let suppressed_total = suppressed_type_count + suppressed_effect_count;
        if suppressed_total > 0 {
            let mut suppression_note = downstream_errors_suppressed_note(
                default_file.unwrap_or("<unknown>"),
                suppressed_type_count,
                suppressed_effect_count,
            );
            suppression_note.span = None;
            shown.push(suppression_note);
        }

        let mut file_cache: HashMap<String, String> = self.sources.clone();
        // Default to always showing file headers for consistent output.
        let show_file_headers = self.show_file_headers.unwrap_or(true);

        let summary = format_summary(&counts, file_count);
        let mut rendered = String::new();

        let mut groups: Vec<String> = Vec::new();
        let mut current_file_key: Option<&str> = None;
        let mut current_group = String::new();
        let mut first_in_group = true;
        let mut rendered_items: Vec<String> = Vec::new();

        for diag in &shown {
            let file_key = effective_file(diag, default_file);
            ensure_source(file_key, &mut file_cache);
            for hint in diag.hints() {
                ensure_source(hint.file.as_deref(), &mut file_cache);
            }
            for related in diag.related() {
                ensure_source(related.file.as_deref(), &mut file_cache);
            }
            let rendered_diag = diag.render_with_sources(default_file, Some(&file_cache));

            if show_file_headers {
                if current_file_key.is_none_or(|f| f != file_key.unwrap_or("")) {
                    if !current_group.is_empty() {
                        groups.push(current_group);
                        current_group = String::new();
                    }
                    current_file_key = Some(file_key.unwrap_or(""));
                    first_in_group = true;
                    let display = file_display(file_key);
                    current_group.push_str(&format!("{}\n", display));
                }

                if !first_in_group {
                    current_group.push_str("\n\n");
                }
                first_in_group = false;
                current_group.push_str(&rendered_diag);
            } else {
                rendered_items.push(rendered_diag);
            }
        }

        if show_file_headers {
            if !current_group.is_empty() {
                groups.push(current_group);
            }
            rendered.push_str(&groups.join("\n\n"));
        } else {
            rendered.push_str(&rendered_items.join("\n\n"));
        }

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

        if let Some(summary) = summary {
            if !rendered.is_empty() {
                if !rendered.ends_with('\n') {
                    rendered.push('\n');
                }
                rendered.push('\n');
            }
            rendered.push_str(&summary);
            rendered.push('\n');
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

fn normalize_file(file: Option<&str>) -> Option<&str> {
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

fn ensure_source(file: Option<&str>, cache: &mut HashMap<String, String>) {
    let file = match file {
        Some(file) if !file.is_empty() => file,
        _ => return,
    };
    if !cache.contains_key(file)
        && let Ok(source) = fs::read_to_string(Path::new(file))
    {
        cache.insert(file.to_string(), source);
    }
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

fn format_summary(counts: &DiagnosticCounts, file_count: usize) -> Option<String> {
    let total = counts.total();
    if total <= 1 && !(counts.errors > 0 && counts.warnings > 0) {
        return None;
    }

    let mut parts = Vec::new();
    if counts.errors > 0 {
        parts.push(format!("{} error{}", counts.errors, plural(counts.errors)));
    }
    if counts.warnings > 0 {
        parts.push(format!(
            "{} warning{}",
            counts.warnings,
            plural(counts.warnings)
        ));
    }
    if counts.notes > 0 {
        parts.push(format!("{} note{}", counts.notes, plural(counts.notes)));
    }
    if counts.helps > 0 {
        parts.push(format!("{} help{}", counts.helps, plural(counts.helps)));
    }

    let parts_str = join_parts(&parts);
    if file_count > 1 {
        Some(format!(
            "Found {} across {} modules.",
            parts_str, file_count
        ))
    } else {
        Some(format!("Found {}.", parts_str))
    }
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
    diag.position().map(|pos| pos.column).unwrap_or(usize::MAX)
}

fn message_key(diag: &Diagnostic) -> &str {
    diag.message().unwrap_or("")
}

fn suppress_nearby_duplicate_e300(
    sorted: Vec<IndexedDiagnostic<'_>>,
    default_file: Option<&str>,
) -> Vec<Diagnostic> {
    let mut retained = Vec::new();
    let mut consumed = vec![false; sorted.len()];

    for (i, indexed) in sorted.iter().enumerate() {
        if consumed[i] {
            continue;
        }
        consumed[i] = true;

        let Some((base_file, base_message, base_span)) =
            e300_group_signature(indexed.diag, default_file)
        else {
            retained.push(indexed.diag.clone());
            continue;
        };

        let mut suppressed_count = 0usize;
        for (j, other) in sorted.iter().enumerate().skip(i + 1) {
            if consumed[j] {
                continue;
            }
            let Some((other_file, other_message, other_span)) =
                e300_group_signature(other.diag, default_file)
            else {
                continue;
            };
            if base_file == other_file
                && base_message == other_message
                && spans_overlap_or_adjacent(base_span, other_span)
            {
                consumed[j] = true;
                suppressed_count += 1;
            }
        }

        let mut diag = indexed.diag.clone();
        if suppressed_count > 0 {
            diag.hints.push(Hint::help(format!(
                "Suppressed {} nearby duplicate E300 diagnostic(s).",
                suppressed_count
            )));
        }
        retained.push(diag);
    }

    retained
}

fn e300_group_signature<'a>(
    diag: &'a Diagnostic,
    default_file: Option<&'a str>,
) -> Option<(Option<&'a str>, &'a str, Span)> {
    if diag.code() != Some("E300") || diag.severity() != Severity::Error {
        return None;
    }
    let message = diag.message()?;
    let span = primary_or_diag_span(diag)?;
    Some((effective_file(diag, default_file), message, span))
}

fn primary_or_diag_span(diag: &Diagnostic) -> Option<Span> {
    diag.labels()
        .iter()
        .find(|l| l.style == LabelStyle::Primary)
        .map(|l| l.span)
        .or_else(|| diag.span())
}

fn spans_overlap_or_adjacent(a: Span, b: Span) -> bool {
    spans_overlap(a, b) || line_gap(a, b) <= 1
}

fn spans_overlap(a: Span, b: Span) -> bool {
    let a_start = (a.start.line, a.start.column);
    let a_end = (a.end.line, a.end.column);
    let b_start = (b.start.line, b.start.column);
    let b_end = (b.end.line, b.end.column);
    a_start <= b_end && b_start <= a_end
}

fn line_gap(a: Span, b: Span) -> usize {
    if a.end.line < b.start.line {
        b.start.line.saturating_sub(a.end.line)
    } else if b.end.line < a.start.line {
        a.start.line.saturating_sub(b.end.line)
    } else {
        0
    }
}

impl LabelKey {
    fn from_label(label: &Label) -> Self {
        Self {
            span: SpanKey::from_span(label.span),
            text: label.text.clone(),
            style: label.style,
        }
    }
}

impl HintKey {
    fn from_hint(hint: &Hint) -> Self {
        Self {
            kind: hint.kind,
            text: hint.text.clone(),
            span: hint.span.map(SpanKey::from_span),
            label: hint.label.clone(),
            file: hint.file.clone(),
        }
    }
}

impl SuggestionKey {
    fn from_suggestion(suggestion: &InlineSuggestion) -> Self {
        Self {
            replacement: suggestion.replacement.clone(),
            span: SpanKey::from_span(suggestion.span),
            message: suggestion.message.clone(),
        }
    }
}

impl HintChainKey {
    fn from_chain(chain: &HintChain) -> Self {
        Self {
            steps: chain.steps.clone(),
            conclusion: chain.conclusion.clone(),
        }
    }
}

fn label_sort(a: &LabelKey, b: &LabelKey) -> std::cmp::Ordering {
    span_sort_key(Some(&a.span))
        .cmp(&span_sort_key(Some(&b.span)))
        .then_with(|| label_style_rank(a.style).cmp(&label_style_rank(b.style)))
        .then_with(|| a.text.cmp(&b.text))
}

fn hint_sort(a: &HintKey, b: &HintKey) -> std::cmp::Ordering {
    hint_kind_rank(a.kind)
        .cmp(&hint_kind_rank(b.kind))
        .then_with(|| a.text.cmp(&b.text))
        .then_with(|| span_sort_key(a.span.as_ref()).cmp(&span_sort_key(b.span.as_ref())))
        .then_with(|| a.label.cmp(&b.label))
        .then_with(|| a.file.cmp(&b.file))
}

fn suggestion_sort(a: &SuggestionKey, b: &SuggestionKey) -> std::cmp::Ordering {
    span_sort_key(Some(&a.span))
        .cmp(&span_sort_key(Some(&b.span)))
        .then_with(|| a.replacement.cmp(&b.replacement))
        .then_with(|| a.message.cmp(&b.message))
}

fn chain_sort(a: &HintChainKey, b: &HintChainKey) -> std::cmp::Ordering {
    a.steps
        .cmp(&b.steps)
        .then_with(|| a.conclusion.cmp(&b.conclusion))
}

fn related_sort(a: &RelatedKey, b: &RelatedKey) -> std::cmp::Ordering {
    related_kind_rank(a.kind)
        .cmp(&related_kind_rank(b.kind))
        .then_with(|| a.message.cmp(&b.message))
        .then_with(|| span_sort_key(a.span.as_ref()).cmp(&span_sort_key(b.span.as_ref())))
        .then_with(|| a.file.cmp(&b.file))
}

fn span_sort_key(span: Option<&SpanKey>) -> (u8, usize, usize, usize, usize) {
    match span {
        Some(span) => (
            0,
            span.start_line,
            span.start_col,
            span.end_line,
            span.end_col,
        ),
        None => (1, 0, 0, 0, 0),
    }
}

fn label_style_rank(style: LabelStyle) -> u8 {
    match style {
        LabelStyle::Primary => 0,
        LabelStyle::Secondary => 1,
        LabelStyle::Note => 2,
    }
}

fn hint_kind_rank(kind: HintKind) -> u8 {
    match kind {
        HintKind::Hint => 0,
        HintKind::Note => 1,
        HintKind::Help => 2,
        HintKind::Example => 3,
    }
}

fn related_kind_rank(kind: RelatedKind) -> u8 {
    match kind {
        RelatedKind::Note => 0,
        RelatedKind::Help => 1,
        RelatedKind::Related => 2,
    }
}
