use std::collections::HashSet;

use flux::ast::type_infer::{display_infer_type, render_scheme_canonical};
use flux::diagnostics::position::Span as FluxSpan;
use flux::syntax::Identifier;
use flux::syntax::block::Block;
use flux::syntax::expression::{Expression, Pattern};
use flux::syntax::interner::Interner;
use flux::syntax::program::Program;
use flux::syntax::statement::Statement;
use flux::syntax::type_expr::TypeExpr;
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionResponse, Documentation, MarkupContent,
    MarkupKind, Position,
};

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
        CompletionContext::ModuleMember { key, reference } => {
            module_member_items(snapshot, &key, &reference)
        }
        CompletionContext::ModuleNamespace(p) => module_namespace_items(snapshot, &p),
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
// completionItem/resolve
// ─────────────────────────────────────────────────────────────────────────────

/// Build the `data` payload stashed on a completion item so
/// [`resolve`] can fetch its documentation later. `kind` is the doc family
/// (`"keyword"`, `"effect"`, `"type"`) and `word` the lookup label.
fn doc_data(kind: &str, word: &str) -> Option<serde_json::Value> {
    Some(serde_json::json!({ "kind": kind, "word": word }))
}

/// Resolve a completion item: fill in its `documentation`, keyed by the `data`
/// payload stashed at completion time. Keyword / effect / type items resolve
/// from the static doc tables; a module-member item uses `member_doc` (its
/// `///` comment, fetched by the caller from the module source — see
/// [`member_ref`]). The heavy markdown is deferred to here so the initial
/// completion response — which lists every keyword on each keystroke — stays
/// small. Items with no resolvable doc (or already-set documentation) are
/// returned unchanged.
pub fn resolve(mut item: CompletionItem, member_doc: Option<String>) -> CompletionItem {
    if item.documentation.is_some() {
        return item;
    }
    let static_doc = item
        .data
        .as_ref()
        .and_then(|data| {
            let kind = data.get("kind").and_then(|v| v.as_str())?;
            let word = data.get("word").and_then(|v| v.as_str())?;
            Some((kind, word))
        })
        .and_then(|(kind, word)| match kind {
            "keyword" => crate::keywords::keyword_doc(word),
            "effect" => crate::keywords::effect_doc(word),
            "type" => crate::keywords::builtin_type_doc(word),
            _ => None,
        })
        .map(str::to_string);
    if let Some(doc) = static_doc.or(member_doc) {
        item.documentation = Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: doc,
        }));
    }
    item
}

/// `(module_key, member)` for a module-member completion item, read from the
/// `data` stashed by [`module_member_items`]. `None` for any other item. The
/// caller turns this into the member's doc comment via `Workspace::member_doc`
/// and passes it to [`resolve`].
pub fn member_ref(item: &CompletionItem) -> Option<(String, String)> {
    let data = item.data.as_ref()?;
    if data.get("kind").and_then(|v| v.as_str()) != Some("member") {
        return None;
    }
    let module = data.get("module").and_then(|v| v.as_str())?.to_string();
    let member = data.get("member").and_then(|v| v.as_str())?.to_string();
    Some((module, member))
}

// ─────────────────────────────────────────────────────────────────────────────
// Context detection
// ─────────────────────────────────────────────────────────────────────────────

enum CompletionContext {
    /// `Module.` — complete module exports. `key` is the resolved
    /// `module_programs`/`module_members` key (an `import … as A` alias is
    /// already mapped back to the underlying module name); `reference` is the
    /// dotted name the user actually typed before the dot, used to attach an
    /// auto-import edit when that name isn't imported yet.
    ModuleMember { key: String, reference: String },
    /// `A.B.` where `A.B` is a proper prefix of one or more module names
    /// (e.g. `A.B.C`) but not itself a module — complete the next path
    /// segment. Carries the dotted prefix.
    ModuleNamespace(String),
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

        // Case 1 — `Foo.` / `A.B.C.` immediately before cursor. The dotted
        // path is tried first (a multi-segment module name like
        // `ClassMatchableEffects.App.Main` resolves as a whole); the bare
        // last segment then drives record-field completion.
        if let Some(path) = module_path_before_dot(bytes, off)
            && let Some(ctx) = module_completion_context(snapshot, &path)
        {
            return ctx;
        }
        if let Some(name_before_dot) = ident_before_dot(bytes, off)
            && let Some(variant) = local_binding_variant(snapshot, &name_before_dot)
        {
            return CompletionContext::DotAccess(variant);
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
        data: doc_data("keyword", kw),
        ..Default::default()
    }));

    // Known module names (Flow stdlib + imported/sibling modules) so a bare
    // `Arr…` surfaces `Array`. Accepting an item for a not-yet-imported module
    // inserts its `import` in the same step via `additionalTextEdits`.
    items.extend(module_name_items(snapshot));

    CompletionResponse::Array(items)
}

/// Completion items for every known module, labelled by the name the user
/// references it by — a short name for the Flow stdlib (`Array`,
/// `String`, …), the full declared path for sibling modules
/// (`Lib.App.Main`) — deduplicated. Surfaced in expression position so a bare
/// prefix starts a qualified `Module.member` path; an item for a module that
/// isn't imported yet carries the `import` as an `additionalTextEdits` so
/// accepting it imports the module automatically.
fn module_name_items(snapshot: &Snapshot) -> Vec<CompletionItem> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut items = Vec::new();
    for key in snapshot
        .module_programs
        .keys()
        .chain(snapshot.module_members.keys())
    {
        if key.is_empty() || !seen.insert(key.clone()) {
            continue;
        }
        let import_edit = module_full_name(snapshot, key)
            .and_then(|full| crate::handlers::auto_import::import_edit_for(snapshot, &full, key))
            .map(|edit| vec![edit]);
        let detail = if import_edit.is_some() {
            "module (auto-imports)"
        } else {
            "module"
        };
        items.push(CompletionItem {
            label: key.clone(),
            kind: Some(CompletionItemKind::MODULE),
            detail: Some(detail.to_string()),
            sort_text: Some(format!("2_{key}")),
            additional_text_edits: import_edit,
            ..Default::default()
        });
    }
    items
}

/// The full declared name (`module Flow.Array { … }` → `"Flow.Array"`) of the
/// module cached under `key`, or `None` when no parsed program is cached for
/// it. Used to build a correct `import` for an auto-importing completion.
fn module_full_name(snapshot: &Snapshot, key: &str) -> Option<String> {
    let (program, _, _) = snapshot.module_programs.get(key)?;
    program.statements.iter().find_map(|stmt| match stmt {
        Statement::Module { name, .. } => snapshot.interner.try_resolve(*name).map(str::to_string),
        _ => None,
    })
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

/// Items for `Module.` completion. Prefers walking the module's parsed
/// program — uniform for the Flow stdlib and user modules, and yields per-kind
/// `CompletionItemKind`s plus a rendered signature. Falls back to the
/// prelude's scheme-derived name list when no program is cached (e.g. a Flow
/// module that loaded schemes but whose source could not be re-parsed).
fn module_member_items(
    snapshot: &Snapshot,
    module_key: &str,
    reference: &str,
) -> CompletionResponse {
    // If the user reached these members through a module name that isn't
    // imported (e.g. `Array.` where `Flow.Array` is indexed but not imported),
    // accepting any member should also add the `import`. `reference` is the
    // name typed before the dot, so the alias case (`import … as A`, `A.`) is
    // already bound and yields no edit.
    let import_edit = module_full_name(snapshot, module_key)
        .and_then(|full| crate::handlers::auto_import::import_edit_for(snapshot, &full, reference))
        .map(|edit| vec![edit]);

    let mut items = match snapshot.module_programs.get(module_key) {
        Some((program, _, _)) => collect_module_member_items(program, &snapshot.interner),
        None => Vec::new(),
    };
    if items.is_empty()
        && let Some(members) = snapshot.module_members.get(module_key)
    {
        items = members
            .iter()
            .map(|m| CompletionItem {
                label: m.clone(),
                kind: Some(CompletionItemKind::FUNCTION),
                detail: Some(format!("{module_key}.{m}")),
                ..Default::default()
            })
            .collect();
    }

    for item in &mut items {
        // Stash the (module, member) so `completionItem/resolve` can fetch the
        // member's `///` doc comment, and carry the auto-import edit (if any).
        item.data = Some(serde_json::json!({
            "kind": "member",
            "module": module_key,
            "member": item.label.clone(),
        }));
        item.additional_text_edits = import_edit.clone();
    }
    CompletionResponse::Array(items)
}

/// Walk a module's parsed program for its public, externally-referenceable
/// members. A user-module file wraps declarations in a `module Name { … }`
/// block (so does the Flow stdlib), so descend one level into every
/// top-level `module` body in addition to the top-level statements.
fn collect_module_member_items(program: &Program, interner: &Interner) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    for stmt in &program.statements {
        push_module_member(stmt, interner, &mut items);
        if let Statement::Module { body, .. } = stmt {
            for inner in &body.statements {
                push_module_member(inner, interner, &mut items);
            }
        }
    }
    items
}

/// Emit a completion item for a single module-level declaration. Functions,
/// `let`s, `data`, and `class`es are gated on `public` — only exported names
/// are reachable via `Module.member`. Effects and type aliases carry no
/// visibility marker in the grammar, so they are always surfaced.
fn push_module_member(stmt: &Statement, interner: &Interner, out: &mut Vec<CompletionItem>) {
    match stmt {
        Statement::Function {
            is_public: true,
            name,
            parameters,
            parameter_types,
            return_type,
            ..
        } => {
            let detail = function_signature(
                interner,
                *name,
                parameters,
                parameter_types,
                return_type.as_ref(),
            );
            push_item(
                interner,
                *name,
                CompletionItemKind::FUNCTION,
                Some(detail),
                "0_",
                out,
            );
        }
        Statement::Let {
            is_public: true,
            name,
            type_annotation,
            ..
        } => {
            let detail = type_annotation.as_ref().map(|t| t.display_with(interner));
            push_item(
                interner,
                *name,
                CompletionItemKind::CONSTANT,
                detail,
                "0_",
                out,
            );
        }
        Statement::Data {
            is_public: true,
            name,
            variants,
            ..
        } => {
            push_item(interner, *name, CompletionItemKind::STRUCT, None, "0_", out);
            for v in variants {
                push_item(
                    interner,
                    v.name,
                    CompletionItemKind::CONSTRUCTOR,
                    None,
                    "0_",
                    out,
                );
            }
        }
        Statement::Class {
            is_public: true,
            name,
            ..
        } => {
            push_item(
                interner,
                *name,
                CompletionItemKind::INTERFACE,
                None,
                "0_",
                out,
            );
        }
        Statement::EffectDecl { name, .. } | Statement::EffectAlias { name, .. } => {
            push_item(
                interner,
                *name,
                CompletionItemKind::INTERFACE,
                None,
                "0_",
                out,
            );
        }
        Statement::TypeAlias(a) => {
            push_item(
                interner,
                a.name,
                CompletionItemKind::TYPE_PARAMETER,
                None,
                "0_",
                out,
            );
        }
        _ => {}
    }
}

/// Render a one-line signature for a module function: `fn name(p: T, …) -> R`.
/// Parameter and return types are included when the declaration annotates
/// them; an unannotated parameter shows just its name.
fn function_signature(
    interner: &Interner,
    name: Identifier,
    parameters: &[Identifier],
    parameter_types: &[Option<TypeExpr>],
    return_type: Option<&TypeExpr>,
) -> String {
    let params: Vec<String> = parameters
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let pn = interner.try_resolve(*p).unwrap_or("_");
            match parameter_types.get(i).and_then(|t| t.as_ref()) {
                Some(ty) => format!("{pn}: {}", ty.display_with(interner)),
                None => pn.to_string(),
            }
        })
        .collect();
    let ret = return_type
        .map(|t| format!(" -> {}", t.display_with(interner)))
        .unwrap_or_default();
    format!(
        "fn {}({}){}",
        interner.try_resolve(name).unwrap_or("?"),
        params.join(", "),
        ret
    )
}

/// Map the identifier written before a `.` to a `module_programs` /
/// `module_members` key. A direct hit (the buffer wrote the module's own
/// name) returns it unchanged; otherwise an `import X.Y as A` whose alias is
/// `name` is followed back to the imported module, trying both the fully
/// qualified name and its final dotted segment.
fn resolve_module_key(snapshot: &Snapshot, name: &str) -> Option<String> {
    let known = |k: &str| {
        snapshot.module_programs.contains_key(k) || snapshot.module_members.contains_key(k)
    };
    if known(name) {
        return Some(name.to_string());
    }
    for stmt in &snapshot.program.statements {
        if let Statement::Import {
            name: qualified,
            alias: Some(alias),
            ..
        } = stmt
            && snapshot.interner.try_resolve(*alias) == Some(name)
        {
            let qualified = snapshot.interner.try_resolve(*qualified)?;
            if known(qualified) {
                return Some(qualified.to_string());
            }
            let short = qualified.rsplit('.').next().unwrap_or(qualified);
            if known(short) {
                return Some(short.to_string());
            }
        }
    }
    None
}

/// Classify the dotted path written before the cursor's `.`:
/// - an exact module (`ClassMatchableEffects.App.Main`) → `ModuleMember`;
/// - a proper prefix of one or more module names (`ClassMatchableEffects`,
///   `ClassMatchableEffects.App`) → `ModuleNamespace`;
/// - anything else → `None`, so the caller falls through to record-field
///   completion.
fn module_completion_context(snapshot: &Snapshot, path: &str) -> Option<CompletionContext> {
    if let Some(key) = resolve_module_key(snapshot, path) {
        return Some(CompletionContext::ModuleMember {
            key,
            reference: path.to_string(),
        });
    }
    let prefix = format!("{path}.");
    let is_namespace = snapshot
        .module_programs
        .keys()
        .chain(snapshot.module_members.keys())
        .any(|k| k.starts_with(&prefix));
    is_namespace.then(|| CompletionContext::ModuleNamespace(path.to_string()))
}

/// Items for `A.B.` namespace completion — the distinct path segments that
/// can follow `prefix` across every known module name.
fn module_namespace_items(snapshot: &Snapshot, prefix: &str) -> CompletionResponse {
    let dotted = format!("{prefix}.");
    let mut seen: HashSet<String> = HashSet::new();
    let mut items: Vec<CompletionItem> = Vec::new();
    for key in snapshot
        .module_programs
        .keys()
        .chain(snapshot.module_members.keys())
    {
        if let Some(rest) = key.strip_prefix(&dotted) {
            let segment = rest.split('.').next().unwrap_or(rest);
            if !segment.is_empty() && seen.insert(segment.to_string()) {
                items.push(CompletionItem {
                    label: segment.to_string(),
                    kind: Some(CompletionItemKind::MODULE),
                    sort_text: Some(format!("0_{segment}")),
                    ..Default::default()
                });
            }
        }
    }
    CompletionResponse::Array(items)
}

/// Scan back from a `.` immediately before `off`, collecting a dotted
/// identifier path (`is_ident_byte` runs plus interior `.`s). Returns the
/// well-formed path text, or `None` when the cursor is not after a `.` or the
/// run is empty / malformed (leading, trailing, or doubled dots).
fn module_path_before_dot(bytes: &[u8], off: usize) -> Option<String> {
    if off == 0 || off > bytes.len() || bytes[off - 1] != b'.' {
        return None;
    }
    let dot_pos = off - 1;
    let mut start = dot_pos;
    while start > 0 && (is_ident_byte(bytes[start - 1]) || bytes[start - 1] == b'.') {
        start -= 1;
    }
    if start == dot_pos {
        return None;
    }
    let path = std::str::from_utf8(&bytes[start..dot_pos]).ok()?;
    if path.starts_with('.') || path.ends_with('.') || path.contains("..") {
        return None;
    }
    Some(path.to_string())
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
            data: doc_data("effect", name),
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
            data: doc_data("type", name),
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
