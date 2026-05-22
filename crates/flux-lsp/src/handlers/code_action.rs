//! `textDocument/codeAction` — quick fixes derived from the snapshot's
//! diagnostics (plus a couple driven by the AST / linter).
//!
//! Most actions are anchored on a diagnostic whose range overlaps the request:
//!
//! - **apply suggestion** — a diagnostic carrying a structured
//!   `InlineSuggestion` (its own span + replacement text), e.g. a
//!   misspelled keyword the parser recovered from;
//! - **did-you-mean** — a diagnostic whose hint reads ``Did you mean `X`?``
//!   for a single-token `X`; the flagged span is replaced with `X`;
//! - **fill / catch-all match arms** — a non-exhaustive `match` (`E015`):
//!   real `Variant(_) -> panic("todo")` arms for every uncovered variant of a
//!   known ADT, and always a bare `_ -> ()` catch-all;
//! - **add effect** — a missing ambient effect (`E400`): append the effect to
//!   the enclosing function's `with` row;
//! - **add missing methods** — an incomplete `instance` (`E442`): stub every
//!   required class method it omits as `fn name(params) { panic("todo") }`
//!   inside the block (the HLS "Add placeholders for <class>" analogue).
//!
//! The rest are diagnostic-independent, driven by the cursor / selection: an
//! `import` suggestion for an unimported module path; removing an `import` the
//! linter flags as unused (W003); and the refactor assists (add type
//! annotation, introduce / inline variable). Every edit is computed on the
//! worker thread from the snapshot, so the handler stays a pure function.

use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionResponse,
    Diagnostic as LspDiagnostic, DocumentChanges, OneOf, OptionalVersionedTextDocumentIdentifier,
    Position, Range, TextDocumentEdit, TextEdit, Uri, WorkspaceEdit,
};

use flux::ast::type_infer::display_infer_type;
use flux::diagnostics::Diagnostic as FluxDiagnostic;
use flux::diagnostics::position::{Position as FluxPosition, Span as FluxSpan};
use flux::syntax::Identifier;
use flux::syntax::effect_expr::EffectExpr;
use flux::syntax::expression::{Expression, Pattern};
use flux::syntax::statement::Statement;
use flux::syntax::type_class::ClassMethod;

use crate::convert::diagnostic_to_lsp;
use crate::locator::{NodeRef, find_at};
use crate::snapshot::Snapshot;

/// Build the quick-fix list for `range`. Walks every snapshot diagnostic,
/// keeps the ones whose range overlaps the request, and turns each into
/// zero or more `CodeAction`s, then appends diagnostic-independent
/// auto-import fixes. `workspace_modules` (every `module` name the workspace
/// declares, from the cached symbol index) lets the auto-import pass discover
/// not-yet-imported sibling modules.
pub fn code_actions(
    snapshot: &Snapshot,
    uri: &Uri,
    range: Range,
    workspace_modules: &[String],
    only: Option<&[CodeActionKind]>,
) -> CodeActionResponse {
    let mut actions: Vec<CodeActionOrCommand> = Vec::new();
    for diag in &snapshot.diagnostics {
        let Some(span) = diag.span() else {
            continue;
        };
        let diag_range = snapshot.position_map.flux_span_to_range(span);
        if !ranges_overlap(diag_range, range) {
            continue;
        }
        let lsp_diag = diagnostic_to_lsp(diag, &snapshot.position_map);
        suggestion_actions(snapshot, uri, &lsp_diag, diag, &mut actions);
        did_you_mean_actions(uri, &lsp_diag, diag, diag_range, &mut actions);
        if diag.code() == Some("E015") {
            // Prefer real per-variant arms when the scrutinee's ADT is known;
            // always also offer the bare catch-all.
            if let Some(action) = fill_match_arms_action(snapshot, uri, &lsp_diag, span) {
                actions.push(action);
            }
            if let Some(action) = catchall_arm_action(snapshot, uri, &lsp_diag, span) {
                actions.push(action);
            }
        }
        if diag.code() == Some("E400") {
            effect_annotation_action(snapshot, uri, &lsp_diag, diag, span, &mut actions);
        }
    }
    // Diagnostic-independent: offer an `import` when the cursor is on a
    // module-qualified path whose module isn't imported yet.
    super::auto_import::import_actions(snapshot, uri, range, workspace_modules, &mut actions);
    // Diagnostic-independent: offer removing an unused `import` the cursor sits
    // on. The linter (not inference) reports W003, so this isn't anchored on a
    // `snapshot.diagnostics` entry like the fixes above.
    remove_unused_import_actions(snapshot, uri, range, &mut actions);
    // Refactor assists — driven by the cursor / selection, not a diagnostic.
    if let Some(action) = add_type_annotation_action(snapshot, uri, range) {
        actions.push(action);
    }
    if let Some(action) = introduce_variable_action(snapshot, uri, range) {
        actions.push(action);
    }
    if let Some(action) = inline_let_action(snapshot, uri, range) {
        actions.push(action);
    }
    // Cursor-on-instance assist: stub the class methods an `instance` is missing.
    if let Some(action) = add_missing_methods_action(snapshot, uri, range) {
        actions.push(action);
    }
    // Source action: only when the client asks for `source.organizeImports`.
    if super::organize_imports::organize_imports_requested(only)
        && let Some(action) = super::organize_imports::organize_imports_action(snapshot, uri)
    {
        actions.push(action);
    }
    actions
}

// ─────────────────────────────────────────────────────────────────────────────
// Fix families
// ─────────────────────────────────────────────────────────────────────────────

/// Surface every structured `InlineSuggestion` carried by the diagnostic as
/// a quick fix. Each suggestion already names its own span and replacement.
fn suggestion_actions(
    snapshot: &Snapshot,
    uri: &Uri,
    lsp_diag: &LspDiagnostic,
    diag: &FluxDiagnostic,
    out: &mut Vec<CodeActionOrCommand>,
) {
    for suggestion in diag.suggestions() {
        let range = snapshot.position_map.flux_span_to_range(suggestion.span);
        let title = suggestion
            .message
            .clone()
            .unwrap_or_else(|| format!("Replace with `{}`", suggestion.replacement));
        out.push(quick_fix(
            title,
            uri,
            vec![TextEdit {
                range,
                new_text: suggestion.replacement.clone(),
            }],
            lsp_diag,
        ));
    }
}

/// For a diagnostic whose hint text reads ``Did you mean `X`?``, offer
/// replacing the flagged span with `X`. Only single-token `X` is accepted —
/// a hint like ``Did you mean `with ..., e`?`` carries prose, not a
/// drop-in replacement, and is skipped.
fn did_you_mean_actions(
    uri: &Uri,
    lsp_diag: &LspDiagnostic,
    diag: &FluxDiagnostic,
    diag_range: Range,
    out: &mut Vec<CodeActionOrCommand>,
) {
    for hint in diag.hints() {
        let Some(name) = parse_did_you_mean(&hint.text) else {
            continue;
        };
        out.push(quick_fix(
            format!("Change to `{name}`"),
            uri,
            vec![TextEdit {
                range: diag_range,
                new_text: name,
            }],
            lsp_diag,
        ));
    }
}

/// For a non-exhaustive `match` (`E015`), insert a `_ -> ()` catch-all arm
/// just before the closing brace.
fn catchall_arm_action(
    snapshot: &Snapshot,
    uri: &Uri,
    lsp_diag: &LspDiagnostic,
    span: FluxSpan,
) -> Option<CodeActionOrCommand> {
    let (insert, arm_indent, terminated) = match_arm_insertion(snapshot, span)?;
    let new_text = arms_insert_text(&["_ -> ()".to_string()], &arm_indent, terminated);
    let pos = snapshot
        .position_map
        .offset_to_lsp(u32::try_from(insert).ok()?.into());
    Some(quick_fix(
        "Add catch-all `_` arm".to_string(),
        uri,
        vec![TextEdit {
            range: Range {
                start: pos,
                end: pos,
            },
            new_text,
        }],
        lsp_diag,
    ))
}

/// For a non-exhaustive `match` (`E015`) over a user ADT, insert a real arm for
/// every uncovered variant — `Circle(_) -> ()`, `Point { .. } -> ()`,
/// `Red -> ()`. Returns `None` (so the caller falls back to the bare catch-all)
/// when the scrutinee's type isn't a buffer-declared ADT, or every variant is
/// already covered (the gap is in a nested pattern, not at the variant level).
fn fill_match_arms_action(
    snapshot: &Snapshot,
    uri: &Uri,
    lsp_diag: &LspDiagnostic,
    span: FluxSpan,
) -> Option<CodeActionOrCommand> {
    let infer = snapshot.infer.as_ref()?;
    // `span.start` is the `match` keyword — the locator's deepest node there is
    // the match expression itself.
    let NodeRef::Expr(Expression::Match {
        scrutinee, arms, ..
    }) = find_at(&snapshot.program, &snapshot.interner, span.start)?
    else {
        return None;
    };
    let adt =
        super::definition::adt_symbol_of_infer_type(infer.expr_types.get(&scrutinee.expr_id())?)?;

    // Variants already covered by an *unguarded* constructor arm — a guarded
    // arm doesn't make its variant exhaustive, matching the compiler's check.
    let covered: std::collections::HashSet<Identifier> = arms
        .iter()
        .filter(|arm| arm.guard.is_none())
        .filter_map(|arm| match &arm.pattern {
            Pattern::Constructor { name, .. } | Pattern::NamedConstructor { name, .. } => {
                Some(*name)
            }
            _ => None,
        })
        .collect();

    // Body is `panic("todo")`, not `()`: `panic` returns a polymorphic `a` so
    // it unifies with whatever the other arms produce (a `()` body would
    // type-mismatch a non-`Unit` match), and the `Panic` effect is exempt from
    // the ambient-effect check, so no `with` clause is needed.
    let missing: Vec<String> = adt_variant_patterns(snapshot, adt)?
        .into_iter()
        .filter(|(id, _)| !covered.contains(id))
        .map(|(_, pat)| format!("{pat} -> panic(\"todo\")"))
        .collect();
    if missing.is_empty() {
        return None;
    }

    let (insert, arm_indent, terminated) = match_arm_insertion(snapshot, span)?;
    let new_text = arms_insert_text(&missing, &arm_indent, terminated);
    let pos = snapshot
        .position_map
        .offset_to_lsp(u32::try_from(insert).ok()?.into());
    let title = if missing.len() == 1 {
        "Fill missing match arm".to_string()
    } else {
        format!("Fill {} missing match arms", missing.len())
    };
    Some(quick_fix(
        title,
        uri,
        vec![TextEdit {
            range: Range {
                start: pos,
                end: pos,
            },
            new_text,
        }],
        lsp_diag,
    ))
}

/// Ordered `(variant_id, pattern_text)` for every variant of the ADT `adt`
/// declared in this buffer — named variants as `V { .. }`, positional as
/// `V(_, …)`, nullary as `V`. `None` when `adt` names no `data`/`type` here.
fn adt_variant_patterns(snapshot: &Snapshot, adt: Identifier) -> Option<Vec<(Identifier, String)>> {
    for stmt in &snapshot.program.statements {
        let Statement::Data { name, variants, .. } = stmt else {
            continue;
        };
        if *name != adt {
            continue;
        }
        let mut out = Vec::with_capacity(variants.len());
        for v in variants {
            let vname = snapshot.interner.try_resolve(v.name)?;
            let pat = if v.field_names.is_some() {
                format!("{vname} {{ .. }}")
            } else if !v.fields.is_empty() {
                let holes = vec!["_"; v.fields.len()].join(", ");
                format!("{vname}({holes})")
            } else {
                vname.to_string()
            };
            out.push((v.name, pat));
        }
        return Some(out);
    }
    None
}

/// The byte offsets of a brace-delimited block's own `{` and matching `}`,
/// scanning from `span.start` (which sits before the block). Brace-depth
/// counting keeps nested braces — record literals, instance/method bodies —
/// balanced, so the close found is the block's own. `None` when the span is
/// past EOF or no balanced pair follows.
fn block_braces(snapshot: &Snapshot, span: FluxSpan) -> Option<(usize, usize)> {
    let bytes = snapshot.text.as_bytes();
    let start: usize = snapshot.position_map.flux_to_offset(span.start)?.into();
    if start >= bytes.len() {
        return None;
    }
    let open = start + bytes[start..].iter().position(|&b| b == b'{')?;
    let mut depth = 0i32;
    for (offset, &byte) in bytes.iter().enumerate().skip(open) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((open, offset));
                }
            }
            _ => {}
        }
    }
    None
}

/// Leading whitespace of the line the byte at `offset` sits on — the indent to
/// align a closing brace (or to derive a one-level-deeper member indent from).
fn line_indent(bytes: &[u8], offset: usize) -> String {
    let line_start = bytes[..offset]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |p| p + 1);
    bytes[line_start..offset]
        .iter()
        .take_while(|&&b| b == b' ' || b == b'\t')
        .map(|&b| b as char)
        .collect()
}

/// Locate where new arm(s) go in a `match` block: the byte offset just after
/// the last arm, the indentation for a new arm, and whether the last arm is
/// already comma-terminated. The diagnostic `span` covers the whole
/// `match … { … }`; brace-depth counting from the first `{` finds the block's
/// own closing brace (record literals in arm bodies stay balanced).
fn match_arm_insertion(snapshot: &Snapshot, span: FluxSpan) -> Option<(usize, String, bool)> {
    let bytes = snapshot.text.as_bytes();
    let start: usize = snapshot.position_map.flux_to_offset(span.start)?.into();
    let (_open, brace) = block_braces(snapshot, span)?;
    // Last non-whitespace byte before the brace — the end of the final arm.
    let mut insert = brace;
    while insert > start && bytes[insert - 1].is_ascii_whitespace() {
        insert -= 1;
    }
    if insert == start {
        return None;
    }
    let prev = bytes[insert - 1];
    // `,` → already terminated; `{` → empty arm list. Either way no comma.
    let terminated = prev == b',' || prev == b'{';
    Some((
        insert,
        format!("{}    ", line_indent(bytes, brace)),
        terminated,
    ))
}

/// Render one or more arm bodies as the text to splice in after the last arm:
/// each on its own indented line, comma-separated, with a leading comma only
/// when the previous arm wasn't already terminated.
fn arms_insert_text(arms: &[String], arm_indent: &str, terminated: bool) -> String {
    let lead = if terminated { "" } else { "," };
    let joined = arms
        .iter()
        .map(|a| format!("\n{arm_indent}{a}"))
        .collect::<Vec<_>>()
        .join(",");
    format!("{lead}{joined}")
}

/// For `E400` (missing ambient effect), offer adding `with <Effect>` to the
/// enclosing function's signature. The effect name is read from the
/// diagnostic's ``with `Effect` `` hint — every E400 emitter phrases its fix
/// that way. The row-variable variant of E400 (whose hint names a bare effect
/// to thread through a row variable, with no `with` phrase) produces no action,
/// since opening a concrete `with` clause isn't the right fix there.
fn effect_annotation_action(
    snapshot: &Snapshot,
    uri: &Uri,
    lsp_diag: &LspDiagnostic,
    diag: &FluxDiagnostic,
    span: FluxSpan,
    out: &mut Vec<CodeActionOrCommand>,
) {
    let Some(effect) = diag.hints().iter().find_map(|h| parse_with_effect(&h.text)) else {
        return;
    };
    let Some(Statement::Function {
        effects,
        span: fn_span,
        ..
    }) = enclosing_function(&snapshot.program.statements, span)
    else {
        return;
    };
    // Append to the existing effect row when there is one; otherwise open a new
    // `with` clause at the end of the signature.
    let (at, new_text) = match effect_row_end(effects) {
        Some(end) => (end, format!(", {effect}")),
        None => (fn_span.end, format!(" with {effect}")),
    };
    let pos = snapshot
        .position_map
        .flux_span_to_range(FluxSpan { start: at, end: at })
        .start;
    out.push(quick_fix(
        format!("Add effect `{effect}` to the enclosing function"),
        uri,
        vec![TextEdit {
            range: Range {
                start: pos,
                end: pos,
            },
            new_text,
        }],
        lsp_diag,
    ));
}

/// Offer "Remove unused import" for any `import` the cursor's range overlaps
/// that the linter flags as unused (W003). Bulk removal is also available via
/// `source.organizeImports`; this is the per-import lightbulb. The linter run
/// is gated on the range first overlapping an `import`, so a code-action request
/// elsewhere pays nothing.
fn remove_unused_import_actions(
    snapshot: &Snapshot,
    uri: &Uri,
    range: Range,
    out: &mut Vec<CodeActionOrCommand>,
) {
    let imports: Vec<(FluxSpan, String)> = snapshot
        .program
        .statements
        .iter()
        .filter_map(|stmt| match stmt {
            Statement::Import { name, span, .. } => {
                let label = snapshot.interner.try_resolve(*name)?.to_string();
                let stmt_range = snapshot.position_map.flux_span_to_range(*span);
                ranges_overlap(stmt_range, range).then_some((*span, label))
            }
            _ => None,
        })
        .collect();
    if imports.is_empty() {
        return;
    }
    let unused = super::organize_imports::unused_import_starts(snapshot);
    for (span, label) in imports {
        if !unused.contains(&(span.start.line, span.start.column)) {
            continue;
        }
        // Delete the whole statement line, including its trailing newline.
        let stmt_range = snapshot.position_map.flux_span_to_range(span);
        let edit = TextEdit {
            range: Range {
                start: Position {
                    line: stmt_range.start.line,
                    character: 0,
                },
                end: Position {
                    line: stmt_range.end.line + 1,
                    character: 0,
                },
            },
            new_text: String::new(),
        };
        out.push(CodeActionOrCommand::CodeAction(CodeAction {
            title: format!("Remove unused import `{label}`"),
            kind: Some(CodeActionKind::QUICKFIX),
            edit: Some(WorkspaceEdit {
                document_changes: Some(DocumentChanges::Edits(vec![TextDocumentEdit {
                    text_document: OptionalVersionedTextDocumentIdentifier {
                        uri: uri.clone(),
                        version: None,
                    },
                    edits: vec![OneOf::Left(edit)],
                }])),
                ..Default::default()
            }),
            ..Default::default()
        }));
    }
}

/// Extract `E` from a hint containing ``with `E` `` (e.g. "Add `with Console`
/// to …" or "annotate … with `with Console`"). Only a single bare effect name
/// — all identifier characters — is accepted; a multi-effect or prose capture
/// is rejected.
fn parse_with_effect(hint: &str) -> Option<String> {
    let after = hint.split_once("`with ")?.1;
    let effect = after.split_once('`')?.0.trim();
    if effect.is_empty() || effect.chars().any(|c| !c.is_alphanumeric() && c != '_') {
        return None;
    }
    Some(effect.to_string())
}

/// The innermost `fn` whose signature or body contains `span`, searching
/// top-level statements plus `module` and nested-function bodies.
fn enclosing_function(stmts: &[Statement], span: FluxSpan) -> Option<&Statement> {
    for stmt in stmts {
        match stmt {
            Statement::Function {
                span: fn_span,
                body,
                ..
            } => {
                let here = span_contains(*fn_span, span)
                    || body
                        .statements
                        .iter()
                        .any(|s| span_contains(s.span(), span));
                if here {
                    // Prefer a nested function inside the body.
                    return enclosing_function(&body.statements, span).or(Some(stmt));
                }
            }
            Statement::Module { body, .. } => {
                if let Some(inner) = enclosing_function(&body.statements, span) {
                    return Some(inner);
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether `outer` fully contains `inner` (inclusive on both ends).
fn span_contains(outer: FluxSpan, inner: FluxSpan) -> bool {
    (outer.start.line, outer.start.column) <= (inner.start.line, inner.start.column)
        && (inner.end.line, inner.end.column) <= (outer.end.line, outer.end.column)
}

/// The rightmost end position across a function's declared effect row, or
/// `None` when it declares no effects.
fn effect_row_end(effects: &[EffectExpr]) -> Option<FluxPosition> {
    effects
        .iter()
        .map(effect_expr_end)
        .reduce(|a, b| if pos_le(a, b) { b } else { a })
}

fn effect_expr_end(e: &EffectExpr) -> FluxPosition {
    match e {
        EffectExpr::Named { span, .. } | EffectExpr::RowVar { span, .. } => span.end,
        EffectExpr::Add { left, right, .. } | EffectExpr::Subtract { left, right, .. } => {
            let l = effect_expr_end(left);
            let r = effect_expr_end(right);
            if pos_le(l, r) { r } else { l }
        }
    }
}

fn pos_le(a: FluxPosition, b: FluxPosition) -> bool {
    (a.line, a.column) <= (b.line, b.column)
}

// ─────────────────────────────────────────────────────────────────────────────
// Refactor assists
// ─────────────────────────────────────────────────────────────────────────────

/// "Add type annotation": on an un-annotated `let`, insert `: <inferred type>`
/// after the binding name. Uses the same inferred-type rendering and insertion
/// point as the inlay-hint annotation edit.
fn add_type_annotation_action(
    snapshot: &Snapshot,
    uri: &Uri,
    range: Range,
) -> Option<CodeActionOrCommand> {
    let infer = snapshot.infer.as_ref()?;
    let (insert_at, ty_text) =
        annotatable_let(&snapshot.program.statements, snapshot, infer, range)?;
    let pos = snapshot.position_map.flux_to_lsp(insert_at);
    Some(refactor_action(
        format!("Add type annotation `: {ty_text}`"),
        CodeActionKind::REFACTOR_REWRITE,
        uri,
        vec![TextEdit {
            range: Range {
                start: pos,
                end: pos,
            },
            new_text: format!(": {ty_text}"),
        }],
    ))
}

/// Find an un-annotated `let` whose binding name overlaps `range`; return where
/// to insert the annotation and the rendered inferred type. Searches top-level
/// statements and `fn`/`module` bodies.
fn annotatable_let(
    stmts: &[Statement],
    snapshot: &Snapshot,
    infer: &flux::ast::type_infer::InferProgramResult,
    range: Range,
) -> Option<(FluxPosition, String)> {
    for stmt in stmts {
        match stmt {
            Statement::Let {
                name,
                value,
                type_annotation: None,
                span,
                is_public,
                ..
            } => {
                let name_end = let_name_end(snapshot, *name, *span, *is_public)?;
                let name_range = snapshot.position_map.flux_span_to_range(FluxSpan {
                    start: span.start,
                    end: name_end,
                });
                if ranges_overlap(name_range, range) {
                    let ty = infer.expr_types.get(&value.expr_id())?;
                    return Some((name_end, display_infer_type(ty, &snapshot.interner)));
                }
            }
            Statement::Function { body, .. } | Statement::Module { body, .. } => {
                if let Some(found) = annotatable_let(&body.statements, snapshot, infer, range) {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}

/// "Introduce variable": hoist the selected expression into a `let` on the line
/// above and replace the selection with the new name. Purely textual — Flux is
/// pure, so lifting a subexpression to a binding preserves meaning. Offered only
/// for a non-empty selection.
fn introduce_variable_action(
    snapshot: &Snapshot,
    uri: &Uri,
    range: Range,
) -> Option<CodeActionOrCommand> {
    if range.start == range.end {
        return None;
    }
    let text = snapshot.text.as_ref();
    let start_off: usize = snapshot.position_map.lsp_to_offset(range.start)?.into();
    let end_off: usize = snapshot.position_map.lsp_to_offset(range.end)?.into();
    let selected = text.get(start_off..end_off)?.trim();
    if selected.is_empty() {
        return None;
    }
    // Skip a bare identifier — extracting `foo` to `let x = foo` is pointless,
    // and avoids offering the assist when the user merely selects a name.
    if is_single_identifier(selected) {
        return None;
    }
    let bytes = text.as_bytes();
    let line_start = bytes[..start_off]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |p| p + 1);
    let indent: String = bytes[line_start..start_off]
        .iter()
        .take_while(|&&b| b == b' ' || b == b'\t')
        .map(|&b| b as char)
        .collect();
    let name = fresh_name(text, "extracted");
    let insert_pos = snapshot
        .position_map
        .offset_to_lsp(u32::try_from(line_start).ok()?.into());
    // Two non-overlapping edits against the original buffer: the new `let` line,
    // and the selection rewritten to the new name.
    let edits = vec![
        TextEdit {
            range: Range {
                start: insert_pos,
                end: insert_pos,
            },
            new_text: format!("{indent}let {name} = {selected}\n"),
        },
        TextEdit {
            range,
            new_text: name.clone(),
        },
    ];
    Some(refactor_action(
        format!("Introduce variable `{name}`"),
        CodeActionKind::REFACTOR_EXTRACT,
        uri,
        edits,
    ))
}

/// "Inline variable": replace the sole use of a `let` binding with its value and
/// delete the binding. Offered only when the name occurs exactly twice in the
/// file (the declaration + one use): that rules out shadowing ambiguity from
/// name-based resolution, and a single use means no re-evaluation concern.
fn inline_let_action(snapshot: &Snapshot, uri: &Uri, range: Range) -> Option<CodeActionOrCommand> {
    let (name, value_span, let_span) =
        let_at_cursor(&snapshot.program.statements, snapshot, range)?;
    let mut spans = Vec::new();
    super::references::collect_all_uses(&snapshot.program, name, &mut spans);
    if spans.len() != 2 {
        return None;
    }
    let use_span = *spans.iter().find(|s| **s != let_span)?;

    let text = snapshot.text.as_ref();
    let v_start: usize = snapshot
        .position_map
        .flux_to_offset(value_span.start)?
        .into();
    let v_end: usize = snapshot.position_map.flux_to_offset(value_span.end)?.into();
    let value_text = text.get(v_start..v_end)?.trim();
    // Parenthesize a compound value so precedence is preserved at the use site
    // (`a + b` inlined into `x * _` must become `(a + b)`). A value with no
    // internal whitespace is a single atom (`"hi"`, `foo`, `f(x)`) and is
    // spliced in bare.
    let replacement = if value_text.chars().any(char::is_whitespace) {
        format!("({value_text})")
    } else {
        value_text.to_string()
    };

    let stmt_range = snapshot.position_map.flux_span_to_range(let_span);
    let name_text = snapshot.interner.try_resolve(name)?;
    let edits = vec![
        // Delete the whole `let` line, including its trailing newline.
        TextEdit {
            range: Range {
                start: Position {
                    line: stmt_range.start.line,
                    character: 0,
                },
                end: Position {
                    line: stmt_range.end.line + 1,
                    character: 0,
                },
            },
            new_text: String::new(),
        },
        TextEdit {
            range: snapshot.position_map.flux_span_to_range(use_span),
            new_text: replacement,
        },
    ];
    Some(refactor_action(
        format!("Inline `{name_text}`"),
        CodeActionKind::REFACTOR_INLINE,
        uri,
        edits,
    ))
}

/// A `let` whose binding name overlaps `range`: `(name, value_span, let_span)`.
/// Searches top-level statements and `fn`/`module` bodies.
fn let_at_cursor(
    stmts: &[Statement],
    snapshot: &Snapshot,
    range: Range,
) -> Option<(Identifier, FluxSpan, FluxSpan)> {
    for stmt in stmts {
        match stmt {
            Statement::Let {
                name,
                value,
                span,
                is_public,
                ..
            } => {
                let name_end = let_name_end(snapshot, *name, *span, *is_public)?;
                let name_range = snapshot.position_map.flux_span_to_range(FluxSpan {
                    start: span.start,
                    end: name_end,
                });
                if ranges_overlap(name_range, range) {
                    return Some((*name, value.span(), *span));
                }
            }
            Statement::Function { body, .. } | Statement::Module { body, .. } => {
                if let Some(found) = let_at_cursor(&body.statements, snapshot, range) {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}

/// The position just after a `let` binding's name — where an annotation goes,
/// and the end of the name range used to test whether the cursor is on it.
fn let_name_end(
    snapshot: &Snapshot,
    name: Identifier,
    span: FluxSpan,
    is_public: bool,
) -> Option<FluxPosition> {
    let name_text = snapshot.interner.try_resolve(name)?;
    let keyword_len = if is_public {
        "public let ".len()
    } else {
        "let ".len()
    };
    Some(FluxPosition {
        line: span.start.line,
        column: span.start.column + keyword_len + name_text.len(),
    })
}

/// Whether `s` is a single identifier (`[A-Za-z_][A-Za-z0-9_]*`).
fn is_single_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_')
}

/// A name not already present in `text` (whole-substring check): `base`, else
/// `base1`, `base2`, … Conservative — a match anywhere, even in a comment, just
/// bumps the suffix, which is always safe.
fn fresh_name(text: &str, base: &str) -> String {
    if !text.contains(base) {
        return base.to_string();
    }
    (1..)
        .map(|n| format!("{base}{n}"))
        .find(|cand| !text.contains(cand.as_str()))
        .unwrap_or_else(|| base.to_string())
}

/// Build a refactor `CodeAction` (no anchoring diagnostic) carrying a single
/// document's edits.
fn refactor_action(
    title: String,
    kind: CodeActionKind,
    uri: &Uri,
    edits: Vec<TextEdit>,
) -> CodeActionOrCommand {
    CodeActionOrCommand::CodeAction(CodeAction {
        title,
        kind: Some(kind),
        edit: Some(WorkspaceEdit {
            document_changes: Some(DocumentChanges::Edits(vec![TextDocumentEdit {
                text_document: OptionalVersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version: None,
                },
                edits: edits.into_iter().map(OneOf::Left).collect(),
            }])),
            ..Default::default()
        }),
        ..Default::default()
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Add missing instance methods
// ─────────────────────────────────────────────────────────────────────────────

/// "Add missing methods": with the cursor anywhere on an incomplete
/// `instance C<T> { … }`, stub every *required* method of class `C` (one with
/// no default body) the instance does not implement, as
/// `fn name(params) { panic("todo") }` inside the instance block. The class
/// must be declared in this buffer.
///
/// This is the HLS class-plugin's "Add placeholders for <class>" analogue, and
/// the instance-side counterpart to fill-match-arms: the missing set is exactly
/// what the compiler's `E442` (MISSING INSTANCE METHOD) flags — now surfaced as
/// a squiggle on the instance — and the `panic("todo")` body is polymorphic and
/// `Panic`-exempt, so each stub type-checks whatever the method returns. The
/// overlapping `E442` diagnostics are attached so the fix associates with that
/// squiggle. The whole-instance trigger is noise-free: a complete instance has
/// nothing to stub and yields no action.
fn add_missing_methods_action(
    snapshot: &Snapshot,
    uri: &Uri,
    range: Range,
) -> Option<CodeActionOrCommand> {
    for stmt in &snapshot.program.statements {
        let Statement::Instance {
            class_name,
            methods,
            span,
            ..
        } = stmt
        else {
            continue;
        };
        if !ranges_overlap(snapshot.position_map.flux_span_to_range(*span), range) {
            continue;
        }
        let Some((open, close)) = block_braces(snapshot, *span) else {
            continue;
        };

        // The class must be declared in this buffer; otherwise skip — a wide
        // selection could still overlap a later, resolvable instance.
        let Some(class_methods) = find_class_methods(snapshot, *class_name) else {
            continue;
        };
        let implemented: std::collections::HashSet<Identifier> =
            methods.iter().map(|m| m.name).collect();
        let stubs: Vec<String> = class_methods
            .iter()
            .filter(|m| m.default_body.is_none() && !implemented.contains(&m.name))
            .filter_map(|m| method_stub(snapshot, m))
            .collect();
        if stubs.is_empty() {
            continue;
        }

        let (Some(edit), Some(class_text)) = (
            instance_methods_edit(snapshot, open, close, &stubs),
            snapshot.interner.try_resolve(*class_name),
        ) else {
            continue;
        };
        let title = if stubs.len() == 1 {
            format!("Add missing method for `{class_text}`")
        } else {
            format!("Add {} missing methods for `{class_text}`", stubs.len())
        };
        let diags = overlapping_diags(snapshot, *span, "E442");
        return Some(missing_methods_action(title, uri, edit, diags));
    }
    None
}

/// The `class` declaration's methods, found in this buffer's own statements.
/// `None` when no `class` named `class` is declared here (e.g. it lives in an
/// imported module — out of scope for this assist).
fn find_class_methods(snapshot: &Snapshot, class: Identifier) -> Option<&[ClassMethod]> {
    snapshot
        .program
        .statements
        .iter()
        .find_map(|stmt| match stmt {
            Statement::Class { name, methods, .. } if *name == class => Some(methods.as_slice()),
            _ => None,
        })
}

/// One `fn name(p, …) { panic("todo") }` line for a class method. Instance
/// methods carry no type annotations, so the stub mirrors that: bare parameter
/// names (a parameter whose name won't resolve falls back to `arg{i}`).
fn method_stub(snapshot: &Snapshot, m: &ClassMethod) -> Option<String> {
    let name = snapshot.interner.try_resolve(m.name)?;
    let params: Vec<String> = m
        .params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            snapshot
                .interner
                .try_resolve(*p)
                .map_or_else(|| format!("arg{i}"), str::to_string)
        })
        .collect();
    Some(format!(
        "fn {name}({}) {{ panic(\"todo\") }}",
        params.join(", ")
    ))
}

/// The `TextEdit` that splices `stubs` into an instance body whose braces are at
/// `open`/`close`. An empty body has its inter-brace whitespace replaced with a
/// clean multi-line block (so the close brace lands on its own line); a
/// non-empty body gets the stubs inserted after its last existing method.
fn instance_methods_edit(
    snapshot: &Snapshot,
    open: usize,
    close: usize,
    stubs: &[String],
) -> Option<TextEdit> {
    let bytes = snapshot.text.as_bytes();
    let brace_indent = line_indent(bytes, close);
    let member_indent = format!("{brace_indent}    ");
    let body: String = stubs
        .iter()
        .map(|s| format!("\n{member_indent}{s}"))
        .collect();

    // Walk back over trailing whitespace before the close brace.
    let mut insert = close;
    while insert > open + 1 && bytes[insert - 1].is_ascii_whitespace() {
        insert -= 1;
    }
    if insert == open + 1 {
        // Empty body: replace `{ … }`'s inter-brace whitespace with a clean block.
        let start = snapshot
            .position_map
            .offset_to_lsp(u32::try_from(open + 1).ok()?.into());
        let end = snapshot
            .position_map
            .offset_to_lsp(u32::try_from(close).ok()?.into());
        Some(TextEdit {
            range: Range { start, end },
            new_text: format!("{body}\n{brace_indent}"),
        })
    } else {
        // Non-empty: append after the last existing method; the body's own
        // trailing `\n<indent>}` already puts the close brace on its own line.
        let pos = snapshot
            .position_map
            .offset_to_lsp(u32::try_from(insert).ok()?.into());
        Some(TextEdit {
            range: Range {
                start: pos,
                end: pos,
            },
            new_text: body,
        })
    }
}

/// Every snapshot diagnostic with error `code` whose span overlaps `span`,
/// converted to LSP form — attached to a code action so the client associates
/// it with the underlying squiggle.
fn overlapping_diags(snapshot: &Snapshot, span: FluxSpan, code: &str) -> Vec<LspDiagnostic> {
    let want = snapshot.position_map.flux_span_to_range(span);
    snapshot
        .diagnostics
        .iter()
        .filter(|d| d.code() == Some(code))
        .filter(|d| {
            d.span()
                .is_some_and(|s| ranges_overlap(snapshot.position_map.flux_span_to_range(s), want))
        })
        .map(|d| diagnostic_to_lsp(d, &snapshot.position_map))
        .collect()
}

/// Build the add-missing-methods quick fix carrying a single edit and the
/// `E442` diagnostics it resolves. Kept as a `QUICKFIX` for `Ctrl+.`
/// discoverability — it clears an incomplete-instance error.
fn missing_methods_action(
    title: String,
    uri: &Uri,
    edit: TextEdit,
    diags: Vec<LspDiagnostic>,
) -> CodeActionOrCommand {
    CodeActionOrCommand::CodeAction(CodeAction {
        title,
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: (!diags.is_empty()).then_some(diags),
        edit: Some(WorkspaceEdit {
            document_changes: Some(DocumentChanges::Edits(vec![TextDocumentEdit {
                text_document: OptionalVersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version: None,
                },
                edits: vec![OneOf::Left(edit)],
            }])),
            ..Default::default()
        }),
        ..Default::default()
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Extract the single-token suggestion from a ``Did you mean `X`?`` hint.
/// Returns `None` when the hint has no such phrase or the captured text is
/// empty / contains whitespace (i.e. it is prose, not a token).
fn parse_did_you_mean(hint: &str) -> Option<String> {
    let after = hint.split_once("Did you mean `")?.1;
    let token = after.split_once('`')?.0;
    if token.is_empty() || token.chars().any(char::is_whitespace) {
        return None;
    }
    Some(token.to_string())
}

/// Two LSP ranges overlap when neither ends strictly before the other
/// starts. A zero-width request range (a bare cursor) still matches a
/// diagnostic it sits inside.
fn ranges_overlap(a: Range, b: Range) -> bool {
    !(position_lt(a.end, b.start) || position_lt(b.end, a.start))
}

fn position_lt(a: Position, b: Position) -> bool {
    (a.line, a.character) < (b.line, b.character)
}

/// Build a single-file `QuickFix` code action as a `CodeActionOrCommand`.
fn quick_fix(
    title: String,
    uri: &Uri,
    edits: Vec<TextEdit>,
    diag: &LspDiagnostic,
) -> CodeActionOrCommand {
    CodeActionOrCommand::CodeAction(quick_fix_action(title, uri, edits, diag))
}

/// Build a single-file `QuickFix` [`CodeAction`].
///
/// The edit is expressed via `document_changes` rather than the simpler
/// `changes` map: `WorkspaceEdit::changes` is keyed by `Uri`, and `Uri` has
/// interior mutability, so a `HashMap<Uri, _>` trips `clippy::mutable_key_type`
/// — `rename.rs` builds its edits the same way for the same reason. The edit
/// is unversioned (`version: None`); the buffer the action was requested for
/// has not changed between request and apply.
fn quick_fix_action(
    title: String,
    uri: &Uri,
    edits: Vec<TextEdit>,
    diag: &LspDiagnostic,
) -> CodeAction {
    let edit = WorkspaceEdit {
        document_changes: Some(DocumentChanges::Edits(vec![TextDocumentEdit {
            text_document: OptionalVersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: None,
            },
            edits: edits.into_iter().map(OneOf::Left).collect(),
        }])),
        ..Default::default()
    };
    CodeAction {
        title,
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diag.clone()]),
        edit: Some(edit),
        ..Default::default()
    }
}
