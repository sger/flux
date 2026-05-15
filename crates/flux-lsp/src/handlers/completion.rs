use lsp_types::{CompletionItem, CompletionItemKind, CompletionResponse, Position};

use crate::snapshot::Snapshot;

const KEYWORDS: &[&str] = &[
    "let", "fn", "if", "else", "match", "data", "effect", "alias", "class", "instance", "import",
    "module", "public", "perform", "handle", "do", "true", "false", "return",
];

/// Built-in effect labels seeded by the compiler (`seed_builtin_effect_aliases`
/// / `seed_builtin_effect_operations`). User-declared effects on top of these
/// are pulled from `snapshot.program.statements`.
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

pub fn complete(snapshot: &Snapshot, position: Position) -> CompletionResponse {
    let ctx = CompletionContext::detect(snapshot, position);

    match ctx {
        CompletionContext::ModuleMember(module_name) => module_member_items(snapshot, &module_name),
        CompletionContext::RecordField(variant_name) => {
            record_field_items(snapshot, variant_name).unwrap_or_else(|| default_items(snapshot))
        }
        CompletionContext::EffectRow => effect_row_items(snapshot),
        CompletionContext::ConstructorField(variant_name) => {
            record_field_items(snapshot, variant_name).unwrap_or_else(|| default_items(snapshot))
        }
        CompletionContext::Default => default_items(snapshot),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Context detection
// ─────────────────────────────────────────────────────────────────────────────

enum CompletionContext {
    /// Cursor follows `Name.` where `Name` is a loaded module. Emit module
    /// members.
    ModuleMember(String),
    /// Cursor follows `expr.` where `expr` resolves to a record-shaped data
    /// variant. We pass the parsed identifier name; the renderer figures
    /// out whether it matches a variant.
    RecordField(flux::syntax::Identifier),
    /// Cursor is inside a `with ` clause. Emit effect labels.
    EffectRow,
    /// Cursor is between `Foo { ` and the matching `}` for a named-field
    /// constructor literal. Emit that variant's field names.
    ConstructorField(flux::syntax::Identifier),
    /// Fallback: top-level symbols + keywords (the old behavior).
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

        // Case 1 — `Foo.` immediately before the cursor. Branch into either
        // module-member completion or record-field completion based on
        // whether the LHS identifier matches a loaded module.
        if let Some(name_before_dot) = ident_before_dot(bytes, off) {
            if snapshot.module_members.contains_key(&name_before_dot) {
                return CompletionContext::ModuleMember(name_before_dot);
            }
            // The LHS may be a local binding whose type is a known ADT.
            // Use the binding's inferred type via `expr_types` if we can.
            if let Some(variant) = local_binding_variant(snapshot, &name_before_dot) {
                return CompletionContext::RecordField(variant);
            }
        }

        // Case 2 — inside `with ...` clause. Heuristic: scan back on the
        // current line for `with ` not preceded by an identifier character.
        if cursor_in_with_clause(bytes, off) {
            return CompletionContext::EffectRow;
        }

        // Case 3 — inside `Foo { ` for a named-field constructor. Scan
        // back: find the most recent `{` before the cursor without an
        // intervening matching `}` (depth-aware), then look at what
        // identifier precedes that `{`. If it matches a record variant,
        // suggest its fields.
        if let Some(variant) = enclosing_constructor_name(snapshot, bytes, off) {
            return CompletionContext::ConstructorField(variant);
        }

        CompletionContext::Default
    }
}

/// Return the identifier-shaped token immediately preceding a `.` at
/// byte `off`, or `None` if `off` is not preceded by `<ident>.`.
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
    std::str::from_utf8(&bytes[start..dot_pos]).ok().map(String::from)
}

/// True if the cursor is on the same line as a `with ` keyword and falls
/// after it (and the `with` is not itself preceded by an identifier byte —
/// otherwise we'd mistake `width:` for an effect-row context).
fn cursor_in_with_clause(bytes: &[u8], off: usize) -> bool {
    let line_start = bytes[..off]
        .iter()
        .rposition(|&b| b == b'\n')
        .map(|p| p + 1)
        .unwrap_or(0);
    let line = &bytes[line_start..off];
    // Find `with ` on the line; check the byte before is not an ident byte
    // (so we don't trigger on `width`, etc.).
    if line.len() < 5 {
        return false;
    }
    for i in 0..=line.len() - 5 {
        if &line[i..i + 5] == b"with "
            && (i == 0 || !is_ident_byte(line[i - 1]))
        {
            return true;
        }
    }
    false
}

/// Find the constructor identifier immediately preceding the most recent
/// unclosed `{` before `off`. Returns its `Identifier` if it matches a
/// record-shaped variant.
fn enclosing_constructor_name(
    snapshot: &Snapshot,
    bytes: &[u8],
    off: usize,
) -> Option<flux::syntax::Identifier> {
    // Depth-aware reverse scan: walk backwards from `off`, increment on
    // `}` and decrement on `{`. When depth reaches -1 we've found the
    // unclosed opening brace.
    let mut depth: i32 = 0;
    let mut i = off;
    while i > 0 {
        i -= 1;
        match bytes[i] {
            b'}' => depth += 1,
            b'{' => {
                if depth == 0 {
                    // Look at what's immediately before this `{` (skip
                    // whitespace).
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
                    // Map name → Identifier via the interner. We need a
                    // mutable interner for `intern`, but we only have
                    // `&Interner`. Use `try_intern_cached` if available,
                    // else linear-scan `variant_fields` for a matching name.
                    return find_variant_by_name(snapshot, name);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

/// Linear scan over `variant_fields` keys looking for an entry whose
/// resolved name equals `name`. Used as a read-only fallback because we
/// don't hold a `&mut Interner` at completion time.
fn find_variant_by_name(snapshot: &Snapshot, name: &str) -> Option<flux::syntax::Identifier> {
    for variant in snapshot.variant_fields.keys() {
        if snapshot.interner.try_resolve(*variant) == Some(name) {
            return Some(*variant);
        }
    }
    None
}

/// Try to find a local `let` binding by name and return the ADT variant
/// (if any) that its RHS construction evaluates to. Currently only
/// matches `let <name> = <Variant> { ... }` — the common pattern in the
/// examples.
fn local_binding_variant(
    snapshot: &Snapshot,
    binding_name: &str,
) -> Option<flux::syntax::Identifier> {
    use flux::syntax::statement::Statement;
    for stmt in &snapshot.program.statements {
        if let Some(variant) = local_binding_variant_in_stmt(stmt, binding_name, &snapshot.interner)
        {
            return Some(variant);
        }
        if let Statement::Function { body, .. } = stmt {
            for inner in &body.statements {
                if let Some(variant) =
                    local_binding_variant_in_stmt(inner, binding_name, &snapshot.interner)
                {
                    return Some(variant);
                }
            }
        }
    }
    None
}

fn local_binding_variant_in_stmt(
    stmt: &flux::syntax::statement::Statement,
    binding_name: &str,
    interner: &flux::syntax::interner::Interner,
) -> Option<flux::syntax::Identifier> {
    use flux::syntax::expression::Expression;
    use flux::syntax::statement::Statement;
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

// ─────────────────────────────────────────────────────────────────────────────
// Item builders
// ─────────────────────────────────────────────────────────────────────────────

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

fn record_field_items(
    snapshot: &Snapshot,
    variant: flux::syntax::Identifier,
) -> Option<CompletionResponse> {
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
            detail: Some(render_field_ty(ty, &snapshot.interner)),
            ..Default::default()
        })
        .collect();
    if items.is_empty() {
        return None;
    }
    Some(CompletionResponse::Array(items))
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
    // Add user-declared `effect`/`alias` names visible in this buffer.
    use flux::syntax::statement::Statement;
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

fn default_items(snapshot: &Snapshot) -> CompletionResponse {
    let mut items: Vec<CompletionItem> = snapshot
        .symbol_index
        .names()
        .map(|name| CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::FUNCTION),
            ..Default::default()
        })
        .collect();
    items.extend(KEYWORDS.iter().map(|kw| CompletionItem {
        label: (*kw).to_string(),
        kind: Some(CompletionItemKind::KEYWORD),
        ..Default::default()
    }));
    CompletionResponse::Array(items)
}

fn render_field_ty(
    ty: &flux::syntax::type_expr::TypeExpr,
    interner: &flux::syntax::interner::Interner,
) -> String {
    use flux::syntax::type_expr::TypeExpr;
    match ty {
        TypeExpr::Named { name, .. } => interner.try_resolve(*name).unwrap_or("?").to_string(),
        TypeExpr::Tuple { .. } => "tuple".to_string(),
        TypeExpr::Function { .. } => "function".to_string(),
    }
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}
