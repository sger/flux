use flux::ast::type_infer::{display_infer_type, render_scheme_canonical};
use flux::diagnostics::position::Span as FluxSpan;
use flux::syntax::Identifier;
use flux::syntax::block::Block;
use flux::syntax::expression::{Expression, Pattern};
use flux::syntax::interner::Interner;
use flux::syntax::statement::Statement;
use lsp_types::{CompletionItem, CompletionItemKind, CompletionResponse, Position};

use crate::line_index::PositionMap;
use crate::snapshot::Snapshot;

const KEYWORDS: &[&str] = &[
    "let", "fn", "if", "else", "match", "data", "effect", "alias", "class", "instance", "import",
    "module", "public", "perform", "handle", "do", "true", "false", "return",
];

const BUILTIN_EFFECTS: &[&str] = &[
    "IO",
    "Time",
    "Async",
    "Console",
    "FileSystem",
    "Stdin",
    "Clock",
    "Random",
    "NonDet",
    "Div",
    "Exn",
    "Panic",
    "Debug",
    "Suspend",
    "Fork",
    "GetContext",
    "AsyncFail",
];

const BUILTIN_TYPES: &[&str] = &[
    "Int", "Float", "String", "Bool", "List", "Array", "Option", "Result",
];

// ─────────────────────────────────────────────────────────────────────────────
// Public entry point
// ─────────────────────────────────────────────────────────────────────────────

pub fn complete(snapshot: &Snapshot, position: Position) -> CompletionResponse {
    let ctx = CompletionContext::detect(snapshot, position);
    match ctx {
        CompletionContext::ModuleMember(m) => module_member_items(snapshot, &m),
        CompletionContext::DotAccess(v) => {
            record_field_items(snapshot, v).unwrap_or_else(|| expr_items(snapshot, position, None))
        }
        CompletionContext::ConstructorField(v) => {
            record_field_items(snapshot, v).unwrap_or_else(|| expr_items(snapshot, position, None))
        }
        CompletionContext::EffectRow => effect_row_items(snapshot),
        CompletionContext::TypeAnnotation => type_annotation_items(snapshot),
        CompletionContext::PerformOp => perform_op_items(snapshot),
        CompletionContext::Expr { enclosing_fn } => expr_items(snapshot, position, enclosing_fn),
        CompletionContext::Default => expr_items(snapshot, position, None),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Context detection
// ─────────────────────────────────────────────────────────────────────────────

enum CompletionContext {
    /// `Module.` — complete module exports.
    ModuleMember(String),
    /// `expr.` — complete record fields of the expression's type.
    DotAccess(Identifier),
    /// `Variant { ` — complete named-field constructor fields.
    ConstructorField(Identifier),
    /// `with ` inside an effect row — complete effect labels.
    EffectRow,
    /// `let x: ` or `fn f(x: ` — complete type names.
    TypeAnnotation,
    /// `perform ` — complete effect operation names.
    PerformOp,
    /// Inside a function body — complete locals + top-level + keywords.
    Expr { enclosing_fn: Option<FluxSpan> },
    /// Top-level or unknown position — complete top-level + keywords.
    Default,
}

impl CompletionContext {
    fn detect(snapshot: &Snapshot, position: Position) -> Self {
        let Some(offset) = snapshot.position_map.lsp_to_offset(position) else {
            return CompletionContext::Default;
        };
        let off: usize = offset.into();
        let text = snapshot.text.as_ref();
        let bytes = text.as_bytes();
        if off > bytes.len() {
            return CompletionContext::Default;
        }

        // Case 1 — `Foo.` immediately before cursor.
        if let Some(name_before_dot) = ident_before_dot(bytes, off) {
            if snapshot.module_members.contains_key(&name_before_dot) {
                return CompletionContext::ModuleMember(name_before_dot);
            }
            if let Some(variant) = local_binding_variant(snapshot, &name_before_dot) {
                return CompletionContext::DotAccess(variant);
            }
        }

        // Case 2 — `perform ` keyword.
        if cursor_after_keyword(bytes, off, b"perform") {
            return CompletionContext::PerformOp;
        }

        // Case 3 — `with ` clause.
        if cursor_in_with_clause(bytes, off) {
            return CompletionContext::EffectRow;
        }

        // Case 4 — inside `Variant { `.
        if let Some(variant) = enclosing_constructor_name(snapshot, bytes, off) {
            return CompletionContext::ConstructorField(variant);
        }

        // Case 5 — type annotation position (`let x: ` or `fn f(x: `).
        if cursor_after_colon(bytes, off) {
            return CompletionContext::TypeAnnotation;
        }

        // Case 6 — inside a function body.
        let enclosing_fn = enclosing_function_span(snapshot, off);
        CompletionContext::Expr { enclosing_fn }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Item builders
// ─────────────────────────────────────────────────────────────────────────────

fn expr_items(
    snapshot: &Snapshot,
    position: Position,
    enclosing_fn: Option<FluxSpan>,
) -> CompletionResponse {
    let mut items = top_level_items(snapshot);

    if let Some(fn_span) = enclosing_fn {
        if let Some(offset) = snapshot.position_map.lsp_to_offset(position) {
            collect_locals(snapshot, fn_span, usize::from(offset), &mut items);
        }
    }

    items.extend(KEYWORDS.iter().map(|kw| CompletionItem {
        label: kw.to_string(),
        kind: Some(CompletionItemKind::KEYWORD),
        sort_text: Some(format!("2_{kw}")),
        ..Default::default()
    }));

    CompletionResponse::Array(items)
}

/// Walk top-level statements and build completion items with correct kinds and
/// type detail from inference results.
fn top_level_items(snapshot: &Snapshot) -> Vec<CompletionItem> {
    let infer = snapshot.infer.as_ref();
    let mut items = Vec::new();

    for stmt in &snapshot.program.statements {
        match stmt {
            Statement::Function { name, span, .. } => {
                let detail = infer.and_then(|r| {
                    let key = span_key(*span);
                    r.resolved_binding_schemes_by_span
                        .get(&key)
                        .map(|s| render_scheme_canonical(&snapshot.interner, s))
                });
                push_item(
                    &snapshot.interner,
                    *name,
                    CompletionItemKind::FUNCTION,
                    detail,
                    "1_",
                    &mut items,
                );
            }
            Statement::Let { name, value, .. } => {
                let detail = infer.and_then(|r| {
                    r.expr_types
                        .get(&value.expr_id())
                        .map(|ty| display_infer_type(ty, &snapshot.interner))
                });
                push_item(
                    &snapshot.interner,
                    *name,
                    CompletionItemKind::VARIABLE,
                    detail,
                    "1_",
                    &mut items,
                );
            }
            Statement::Data { name, variants, .. } => {
                push_item(
                    &snapshot.interner,
                    *name,
                    CompletionItemKind::STRUCT,
                    None,
                    "1_",
                    &mut items,
                );
                for v in variants {
                    push_item(
                        &snapshot.interner,
                        v.name,
                        CompletionItemKind::CONSTRUCTOR,
                        None,
                        "1_",
                        &mut items,
                    );
                }
            }
            Statement::Module { name, .. } => {
                push_item(
                    &snapshot.interner,
                    *name,
                    CompletionItemKind::MODULE,
                    None,
                    "1_",
                    &mut items,
                );
            }
            Statement::EffectDecl { name, .. } | Statement::EffectAlias { name, .. } => {
                push_item(
                    &snapshot.interner,
                    *name,
                    CompletionItemKind::INTERFACE,
                    None,
                    "1_",
                    &mut items,
                );
            }
            Statement::TypeAlias(a) => {
                push_item(
                    &snapshot.interner,
                    a.name,
                    CompletionItemKind::TYPE_PARAMETER,
                    None,
                    "1_",
                    &mut items,
                );
            }
            Statement::Class { name, .. } => {
                push_item(
                    &snapshot.interner,
                    *name,
                    CompletionItemKind::INTERFACE,
                    None,
                    "1_",
                    &mut items,
                );
            }
            _ => {}
        }
    }
    items
}

fn module_member_items(snapshot: &Snapshot, module_name: &str) -> CompletionResponse {
    let Some(members) = snapshot.module_members.get(module_name) else {
        return CompletionResponse::Array(Vec::new());
    };
    let items: Vec<CompletionItem> = members
        .iter()
        .map(|m| CompletionItem {
            label: m.clone(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some(format!("{module_name}.{m}")),
            ..Default::default()
        })
        .collect();
    CompletionResponse::Array(items)
}

fn record_field_items(snapshot: &Snapshot, variant: Identifier) -> Option<CompletionResponse> {
    let fields = snapshot.variant_fields.get(&variant)?;
    let items: Vec<CompletionItem> = fields
        .iter()
        .map(|(field_name, ty)| CompletionItem {
            label: snapshot
                .interner
                .try_resolve(*field_name)
                .unwrap_or("?")
                .to_string(),
            kind: Some(CompletionItemKind::FIELD),
            detail: Some(ty.display_with(&snapshot.interner)),
            ..Default::default()
        })
        .collect();
    if items.is_empty() {
        return None;
    }
    Some(CompletionResponse::Array(items))
}

fn perform_op_items(snapshot: &Snapshot) -> CompletionResponse {
    let mut items: Vec<CompletionItem> = Vec::new();
    for stmt in &snapshot.program.statements {
        if let Statement::EffectDecl { ops, .. } = stmt {
            for op in ops {
                if let Some(name) = snapshot.interner.try_resolve(op.name) {
                    items.push(CompletionItem {
                        label: name.to_string(),
                        kind: Some(CompletionItemKind::FUNCTION),
                        detail: Some("effect operation".to_string()),
                        sort_text: Some(format!("0_{name}")),
                        ..Default::default()
                    });
                }
            }
        }
    }
    // Fall back to full expression items if no ops are in scope.
    if items.is_empty() {
        return CompletionResponse::Array(
            snapshot
                .program
                .statements
                .iter()
                .filter_map(|stmt| {
                    if let Statement::EffectDecl { name, .. } = stmt {
                        snapshot
                            .interner
                            .try_resolve(*name)
                            .map(|n| CompletionItem {
                                label: n.to_string(),
                                kind: Some(CompletionItemKind::INTERFACE),
                                detail: Some("effect".to_string()),
                                ..Default::default()
                            })
                    } else {
                        None
                    }
                })
                .collect(),
        );
    }
    CompletionResponse::Array(items)
}

fn effect_row_items(snapshot: &Snapshot) -> CompletionResponse {
    let mut items: Vec<CompletionItem> = BUILTIN_EFFECTS
        .iter()
        .map(|name| CompletionItem {
            label: (*name).to_string(),
            kind: Some(CompletionItemKind::INTERFACE),
            detail: Some("built-in effect".to_string()),
            ..Default::default()
        })
        .collect();
    for stmt in &snapshot.program.statements {
        match stmt {
            Statement::EffectDecl { name, .. } | Statement::EffectAlias { name, .. } => {
                if let Some(resolved) = snapshot.interner.try_resolve(*name) {
                    items.push(CompletionItem {
                        label: resolved.to_string(),
                        kind: Some(CompletionItemKind::INTERFACE),
                        detail: Some("user-declared effect".to_string()),
                        ..Default::default()
                    });
                }
            }
            _ => {}
        }
    }
    CompletionResponse::Array(items)
}

fn type_annotation_items(snapshot: &Snapshot) -> CompletionResponse {
    let mut items: Vec<CompletionItem> = BUILTIN_TYPES
        .iter()
        .map(|name| CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::STRUCT),
            detail: Some("built-in type".to_string()),
            ..Default::default()
        })
        .collect();
    for stmt in &snapshot.program.statements {
        match stmt {
            Statement::Data { name, .. } => {
                push_item(
                    &snapshot.interner,
                    *name,
                    CompletionItemKind::STRUCT,
                    None,
                    "1_",
                    &mut items,
                );
            }
            Statement::TypeAlias(a) => {
                push_item(
                    &snapshot.interner,
                    a.name,
                    CompletionItemKind::TYPE_PARAMETER,
                    None,
                    "1_",
                    &mut items,
                );
            }
            _ => {}
        }
    }
    CompletionResponse::Array(items)
}

// ─────────────────────────────────────────────────────────────────────────────
// Locals collection
// ─────────────────────────────────────────────────────────────────────────────

fn collect_locals(
    snapshot: &Snapshot,
    _enclosing_fn: FluxSpan,
    cursor_offset: usize,
    out: &mut Vec<CompletionItem>,
) {
    for stmt in &snapshot.program.statements {
        if let Statement::Function {
            span,
            parameters,
            parameter_types,
            body,
            ..
        } = stmt
        {
            if !span_contains_offset(*span, &snapshot.position_map, cursor_offset) {
                continue;
            }
            // Parameters
            for (param, param_ty) in parameters.iter().zip(parameter_types.iter()) {
                let detail = param_ty
                    .as_ref()
                    .map(|ty| ty.display_with(&snapshot.interner));
                push_item(
                    &snapshot.interner,
                    *param,
                    CompletionItemKind::VARIABLE,
                    detail,
                    "0_",
                    out,
                );
            }
            // Let bindings before cursor inside body
            collect_locals_from_block(body, snapshot, cursor_offset, out);
            return;
        }
    }
}

fn collect_locals_from_block(
    block: &Block,
    snapshot: &Snapshot,
    cursor_offset: usize,
    out: &mut Vec<CompletionItem>,
) {
    let infer = snapshot.infer.as_ref();
    for stmt in &block.statements {
        let stmt_end = snapshot
            .position_map
            .flux_to_offset(stmt.span().end)
            .map(usize::from)
            .unwrap_or(usize::MAX);
        if stmt_end >= cursor_offset {
            break;
        }
        match stmt {
            Statement::Let { name, value, .. } => {
                let detail = infer.and_then(|r| {
                    r.expr_types
                        .get(&value.expr_id())
                        .map(|ty| display_infer_type(ty, &snapshot.interner))
                });
                push_item(
                    &snapshot.interner,
                    *name,
                    CompletionItemKind::VARIABLE,
                    detail,
                    "0_",
                    out,
                );
            }
            Statement::LetDestructure { pattern, value, .. } => {
                collect_pattern_bindings(pattern, value, snapshot, out);
            }
            Statement::Function { name, .. } => {
                push_item(
                    &snapshot.interner,
                    *name,
                    CompletionItemKind::FUNCTION,
                    None,
                    "0_",
                    out,
                );
            }
            _ => {}
        }
    }
}

fn collect_pattern_bindings(
    pattern: &Pattern,
    value: &Expression,
    snapshot: &Snapshot,
    out: &mut Vec<CompletionItem>,
) {
    match pattern {
        Pattern::Identifier { name, .. } => {
            let detail = snapshot.infer.as_ref().and_then(|r| {
                r.expr_types
                    .get(&value.expr_id())
                    .map(|ty| display_infer_type(ty, &snapshot.interner))
            });
            push_item(
                &snapshot.interner,
                *name,
                CompletionItemKind::VARIABLE,
                detail,
                "0_",
                out,
            );
        }
        Pattern::Tuple { elements, .. } => {
            for e in elements {
                collect_pattern_bindings(e, value, snapshot, out);
            }
        }
        _ => {}
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Context detection helpers
// ─────────────────────────────────────────────────────────────────────────────

fn enclosing_function_span(snapshot: &Snapshot, offset: usize) -> Option<FluxSpan> {
    for stmt in &snapshot.program.statements {
        if let Statement::Function { span, body, .. } = stmt {
            if span_contains_offset(*span, &snapshot.position_map, offset) {
                // Check one level of nested functions
                for inner in &body.statements {
                    if let Statement::Function {
                        span: inner_span, ..
                    } = inner
                    {
                        if span_contains_offset(*inner_span, &snapshot.position_map, offset) {
                            return Some(*inner_span);
                        }
                    }
                }
                return Some(*span);
            }
        }
    }
    None
}

fn span_contains_offset(span: FluxSpan, pos_map: &PositionMap, offset: usize) -> bool {
    let start = pos_map
        .flux_to_offset(span.start)
        .map(usize::from)
        .unwrap_or(usize::MAX);
    let end = pos_map
        .flux_to_offset(span.end)
        .map(usize::from)
        .unwrap_or(0);
    offset >= start && offset <= end
}

fn span_key(span: FluxSpan) -> (usize, usize, usize, usize) {
    (
        span.start.line,
        span.start.column,
        span.end.line,
        span.end.column,
    )
}

fn push_item(
    interner: &Interner,
    name: Identifier,
    kind: CompletionItemKind,
    detail: Option<String>,
    sort_prefix: &str,
    out: &mut Vec<CompletionItem>,
) {
    let Some(label) = interner.try_resolve(name) else {
        return;
    };
    out.push(CompletionItem {
        label: label.to_string(),
        kind: Some(kind),
        detail,
        sort_text: Some(format!("{sort_prefix}{label}")),
        ..Default::default()
    });
}

fn ident_before_dot(bytes: &[u8], off: usize) -> Option<String> {
    if off == 0 || off > bytes.len() {
        return None;
    }
    if bytes[off - 1] != b'.' {
        return None;
    }
    let dot_pos = off - 1;
    let mut start = dot_pos;
    while start > 0 && is_ident_byte(bytes[start - 1]) {
        start -= 1;
    }
    if start == dot_pos {
        return None;
    }
    std::str::from_utf8(&bytes[start..dot_pos])
        .ok()
        .map(String::from)
}

/// Returns `true` when the text immediately before the cursor (ignoring
/// trailing spaces) ends with `keyword` followed by at least one space, and
/// `keyword` is not a suffix of a longer identifier (e.g. `reperform` won't
/// trigger for `perform`).
fn cursor_after_keyword(bytes: &[u8], off: usize, keyword: &[u8]) -> bool {
    // Skip trailing spaces before cursor.
    let mut i = off;
    while i > 0 && bytes[i - 1] == b' ' {
        i -= 1;
    }
    // There must be at least one space between keyword and cursor.
    if i == off {
        return false;
    }
    // Check keyword ends at position `i`.
    if i < keyword.len() {
        return false;
    }
    if &bytes[i - keyword.len()..i] != keyword {
        return false;
    }
    // Keyword must not be a suffix of a longer identifier.
    let before = i - keyword.len();
    before == 0 || !is_ident_byte(bytes[before - 1])
}

fn cursor_in_with_clause(bytes: &[u8], off: usize) -> bool {
    let line_start = bytes[..off]
        .iter()
        .rposition(|&b| b == b'\n')
        .map(|p| p + 1)
        .unwrap_or(0);
    let line = &bytes[line_start..off];
    if line.len() < 5 {
        return false;
    }
    for i in 0..=line.len() - 5 {
        if &line[i..i + 5] == b"with " && (i == 0 || !is_ident_byte(line[i - 1])) {
            return true;
        }
    }
    false
}

fn cursor_after_colon(bytes: &[u8], off: usize) -> bool {
    let mut i = off;
    // Skip trailing spaces
    while i > 0 && bytes[i - 1] == b' ' {
        i -= 1;
    }
    // Must find ':' preceded by an identifier character (not `::` or `->`)
    if i == 0 || bytes[i - 1] != b':' {
        return false;
    }
    if i >= 2 && bytes[i - 2] == b':' {
        return false; // `::` — path separator, not type annotation
    }
    true
}

fn enclosing_constructor_name(snapshot: &Snapshot, bytes: &[u8], off: usize) -> Option<Identifier> {
    let mut depth: i32 = 0;
    let mut i = off;
    while i > 0 {
        i -= 1;
        match bytes[i] {
            b'}' => depth += 1,
            b'{' => {
                if depth == 0 {
                    let mut j = i;
                    while j > 0 && bytes[j - 1].is_ascii_whitespace() {
                        j -= 1;
                    }
                    if j == 0 {
                        return None;
                    }
                    let end = j;
                    while j > 0 && is_ident_byte(bytes[j - 1]) {
                        j -= 1;
                    }
                    if j == end {
                        return None;
                    }
                    let name = std::str::from_utf8(&bytes[j..end]).ok()?;
                    return find_variant_by_name(snapshot, name);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

fn find_variant_by_name(snapshot: &Snapshot, name: &str) -> Option<Identifier> {
    for variant in snapshot.variant_fields.keys() {
        if snapshot.interner.try_resolve(*variant) == Some(name) {
            return Some(*variant);
        }
    }
    None
}

fn local_binding_variant(snapshot: &Snapshot, binding_name: &str) -> Option<Identifier> {
    for stmt in &snapshot.program.statements {
        if let Some(v) = local_binding_variant_in_stmt(stmt, binding_name, &snapshot.interner) {
            return Some(v);
        }
        if let Statement::Function { body, .. } = stmt {
            for inner in &body.statements {
                if let Some(v) =
                    local_binding_variant_in_stmt(inner, binding_name, &snapshot.interner)
                {
                    return Some(v);
                }
            }
        }
    }
    None
}

fn local_binding_variant_in_stmt(
    stmt: &Statement,
    binding_name: &str,
    interner: &Interner,
) -> Option<Identifier> {
    if let Statement::Let { name, value, .. } = stmt
        && interner.try_resolve(*name) == Some(binding_name)
        && let Expression::NamedConstructor {
            name: variant_name, ..
        } = value
    {
        return Some(*variant_name);
    }
    None
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}
