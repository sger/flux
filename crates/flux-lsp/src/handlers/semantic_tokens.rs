//! Semantic highlighting (`textDocument/semanticTokens/full`).
//!
//! Two layers cooperate to colour a buffer, mirroring how rust-analyzer's
//! semantic tokens augment a TextMate grammar:
//!
//! 1. A fresh lexer pass over the buffer gives every lexical token a real span
//!    (no column arithmetic), so keywords, numbers, strings, doc-comments,
//!    operators and decorators are emitted directly.
//! 2. Identifiers are classified by their *semantic* role — the part a grammar
//!    cannot know. Name sets harvested from the AST (which names are functions,
//!    data types, variants, effects, classes, type aliases, type parameters,
//!    value parameters, fields, modules) plus the prelude's stdlib index turn a
//!    bare `foo`/`Bar` into `function`/`enum`/`parameter`/`namespace`/… with
//!    `declaration`, `readonly`, and `defaultLibrary` modifiers.
//!
//! The legend is built entirely from *standard* LSP token types and modifiers,
//! so VS Code themes colour them out of the box; `editors/vscode/package.json`
//! adds `semanticTokenScopes` fallbacks for themes that only define TextMate
//! scopes.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use flux::diagnostics::position::Span as FluxSpan;
use flux::syntax::Identifier;
use flux::syntax::block::Block;
use flux::syntax::expression::{Expression, Pattern};
use flux::syntax::interner::Interner;
use flux::syntax::lexer::Lexer;
use flux::syntax::statement::Statement;
use flux::syntax::token::Token;
use flux::syntax::token_type::TokenType;
use line_index::TextSize;
use lsp_types::{
    Range, SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens,
    SemanticTokensDelta, SemanticTokensEdit, SemanticTokensFullDeltaResult, SemanticTokensLegend,
};

use crate::snapshot::Snapshot;

// ── Legend ─────────────────────────────────────────────────────────────────
// Token-type indices — order MUST match `TOKEN_TYPES` below.
const TT_NAMESPACE: u32 = 0;
const TT_TYPE: u32 = 1;
const TT_CLASS: u32 = 2;
const TT_ENUM: u32 = 3;
const TT_INTERFACE: u32 = 4;
const TT_TYPE_PARAMETER: u32 = 5;
const TT_PARAMETER: u32 = 6;
const TT_VARIABLE: u32 = 7;
const TT_PROPERTY: u32 = 8;
const TT_ENUM_MEMBER: u32 = 9;
const TT_FUNCTION: u32 = 10;
const TT_METHOD: u32 = 11;
const TT_KEYWORD: u32 = 12;
const TT_COMMENT: u32 = 13;
const TT_STRING: u32 = 14;
const TT_NUMBER: u32 = 15;
const TT_OPERATOR: u32 = 16;
const TT_DECORATOR: u32 = 17;

fn token_types() -> Vec<SemanticTokenType> {
    vec![
        SemanticTokenType::NAMESPACE,      // 0
        SemanticTokenType::TYPE,           // 1
        SemanticTokenType::CLASS,          // 2
        SemanticTokenType::ENUM,           // 3
        SemanticTokenType::INTERFACE,      // 4
        SemanticTokenType::TYPE_PARAMETER, // 5
        SemanticTokenType::PARAMETER,      // 6
        SemanticTokenType::VARIABLE,       // 7
        SemanticTokenType::PROPERTY,       // 8
        SemanticTokenType::ENUM_MEMBER,    // 9
        SemanticTokenType::FUNCTION,       // 10
        SemanticTokenType::METHOD,         // 11
        SemanticTokenType::KEYWORD,        // 12
        SemanticTokenType::COMMENT,        // 13
        SemanticTokenType::STRING,         // 14
        SemanticTokenType::NUMBER,         // 15
        SemanticTokenType::OPERATOR,       // 16
        SemanticTokenType::DECORATOR,      // 17
    ]
}

// Modifier bit positions — order MUST match `token_modifiers()` below.
const MOD_DECLARATION: u32 = 1 << 0;
const MOD_READONLY: u32 = 1 << 1;
const MOD_DEFAULT_LIBRARY: u32 = 1 << 2;
const MOD_DOCUMENTATION: u32 = 1 << 3;

fn token_modifiers() -> Vec<SemanticTokenModifier> {
    vec![
        SemanticTokenModifier::DECLARATION,     // bit 0
        SemanticTokenModifier::READONLY,        // bit 1
        SemanticTokenModifier::DEFAULT_LIBRARY, // bit 2
        SemanticTokenModifier::DOCUMENTATION,   // bit 3
    ]
}

pub fn semantic_tokens_legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: token_types(),
        token_modifiers: token_modifiers(),
    }
}

/// Built-in type names that resolve to `defaultLibrary` types regardless of any
/// buffer declarations.
const BUILTIN_TYPES: &[&str] = &[
    "Int", "Float", "String", "Bool", "Unit", "Char", "List", "Array", "Option", "Either", "Map",
    "Result",
];

/// Built-in constructors that the lexer hands back as plain identifiers (unlike
/// `Some`/`None`/`Left`/`Right`, which are keyword tokens).
const BUILTIN_VARIANTS: &[&str] = &["Ok", "Err"];

/// `textDocument/semanticTokens/full` (pure form — no result id / caching).
/// Retained for direct callers and tests; the request path goes through
/// [`full`], which also tags the result for delta follow-ups.
pub fn semantic_tokens(snapshot: &Snapshot) -> SemanticTokens {
    SemanticTokens {
        result_id: None,
        data: compute(snapshot),
    }
}

/// The classified, line/column-sorted tokens for the whole buffer.
fn raw_tokens(snapshot: &Snapshot) -> Vec<RawToken> {
    let names = NameSets::collect(&snapshot.program, &snapshot.interner);
    let stdlib_members: HashSet<&str> = snapshot
        .module_members
        .values()
        .flat_map(|members| members.iter().map(String::as_str))
        .collect();

    let tokens = Lexer::new(snapshot.text.to_string()).tokenize();
    let mut raw: Vec<RawToken> = Vec::new();
    classify(&tokens, &names, &stdlib_members, snapshot, &mut raw);
    raw.sort_by_key(|t| (t.line, t.start));
    raw
}

/// The delta-encoded token stream for the whole buffer.
fn compute(snapshot: &Snapshot) -> Vec<SemanticToken> {
    delta_encode(raw_tokens(snapshot))
}

// ── Range / delta (incremental) requests ─────────────────────────────────────

/// `textDocument/semanticTokens/full` — compute the tokens, tag them with a
/// fresh `result_id`, and remember them in `cache` so a later `…/full/delta`
/// can answer with a minimal splice.
pub fn full(snapshot: &Snapshot, uri: &str, cache: &Mutex<SemanticTokenCache>) -> SemanticTokens {
    let data = compute(snapshot);
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    let result_id = cache.fresh_id();
    cache.record(uri, result_id.clone(), data.clone());
    SemanticTokens {
        result_id: Some(result_id),
        data,
    }
}

/// `textDocument/semanticTokens/full/delta` — compute the current tokens and,
/// if `previous_result_id` still matches what the client last received, return
/// just the edits that turn the old stream into the new one. If the client is
/// out of sync (unknown `previous_result_id`) it gets a fresh full set instead.
pub fn full_delta(
    snapshot: &Snapshot,
    uri: &str,
    previous_result_id: &str,
    cache: &Mutex<SemanticTokenCache>,
) -> SemanticTokensFullDeltaResult {
    let data = compute(snapshot);
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    let result_id = cache.fresh_id();
    let result = match cache.previous(uri, previous_result_id) {
        Some(old) => SemanticTokensFullDeltaResult::TokensDelta(SemanticTokensDelta {
            result_id: Some(result_id.clone()),
            edits: diff_tokens(old, &data),
        }),
        None => SemanticTokensFullDeltaResult::Tokens(SemanticTokens {
            result_id: Some(result_id.clone()),
            data: data.clone(),
        }),
    };
    cache.record(uri, result_id, data);
    result
}

/// `textDocument/semanticTokens/range` — only the tokens that fall inside
/// `range`. Range responses don't participate in deltas, so they carry no
/// `result_id` and aren't cached.
pub fn range(snapshot: &Snapshot, range: Range) -> SemanticTokens {
    let raw: Vec<RawToken> = raw_tokens(snapshot)
        .into_iter()
        .filter(|t| token_in_range(t, range))
        .collect();
    SemanticTokens {
        result_id: None,
        data: delta_encode(raw),
    }
}

/// Whether a token (a single-line span `start..start+length` on `line`)
/// intersects the half-open `range`.
fn token_in_range(t: &RawToken, range: Range) -> bool {
    let end = t.start + t.length;
    // Below the first line or above the last line → out.
    if t.line < range.start.line || t.line > range.end.line {
        return false;
    }
    // On the first line, the token must end past the range start.
    if t.line == range.start.line && end <= range.start.character {
        return false;
    }
    // On the last line, the token must start before the range end.
    if t.line == range.end.line && t.start >= range.end.character {
        return false;
    }
    true
}

/// Minimal edit list turning `old` into `new`, in the integer-array units the
/// LSP semantic-tokens delta protocol uses (5 integers per token). Strips the
/// common prefix and suffix and splices the changed middle as one edit; an
/// unchanged stream yields no edits.
fn diff_tokens(old: &[SemanticToken], new: &[SemanticToken]) -> Vec<SemanticTokensEdit> {
    let max_prefix = old.len().min(new.len());
    let mut prefix = 0;
    while prefix < max_prefix && old[prefix] == new[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < (old.len() - prefix).min(new.len() - prefix)
        && old[old.len() - 1 - suffix] == new[new.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let removed = old.len() - prefix - suffix;
    let inserted = &new[prefix..new.len() - suffix];
    if removed == 0 && inserted.is_empty() {
        return Vec::new();
    }
    vec![SemanticTokensEdit {
        start: (prefix * 5) as u32,
        delete_count: (removed * 5) as u32,
        data: Some(inserted.to_vec()),
    }]
}

/// Per-document store of the last token stream handed to the client, keyed by
/// the `result_id` it was tagged with, so `…/full/delta` can diff against it.
#[derive(Default)]
pub struct SemanticTokenCache {
    next_id: u64,
    last: HashMap<String, CachedTokens>,
}

struct CachedTokens {
    result_id: String,
    tokens: Vec<SemanticToken>,
}

impl SemanticTokenCache {
    fn fresh_id(&mut self) -> String {
        self.next_id += 1;
        self.next_id.to_string()
    }

    fn record(&mut self, uri: &str, result_id: String, tokens: Vec<SemanticToken>) {
        self.last
            .insert(uri.to_string(), CachedTokens { result_id, tokens });
    }

    /// The tokens last sent for `uri`, but only if they carried
    /// `previous_result_id` (else the client's baseline is stale).
    fn previous(&self, uri: &str, previous_result_id: &str) -> Option<&[SemanticToken]> {
        self.last
            .get(uri)
            .filter(|c| c.result_id == previous_result_id)
            .map(|c| c.tokens.as_slice())
    }

    /// Drop a document's cached tokens (on close).
    pub fn forget(&mut self, uri: &str) {
        self.last.remove(uri);
    }
}

#[derive(Clone)]
struct RawToken {
    line: u32,
    start: u32,
    length: u32,
    token_type: u32,
    modifiers: u32,
}

// ── Token-stream classifier ─────────────────────────────────────────────────

fn classify(
    tokens: &[Token],
    names: &NameSets,
    stdlib_members: &HashSet<&str>,
    snapshot: &Snapshot,
    out: &mut Vec<RawToken>,
) {
    // Set after a declaration keyword (`fn`, `let`, `data`, …); consumed by the
    // immediately following identifier so the declared name carries the right
    // type and the `declaration` modifier.
    let mut pending_decl: Option<(u32, u32)> = None;
    // `import …` / `module …` colour their dotted path as a namespace.
    let mut import_mode = false;
    let mut import_line = 0usize;
    let mut module_mode = false;
    // `@name` annotation: the `@` arms the next identifier as a decorator.
    let mut decorator_pending = false;
    let mut prev: Option<TokenType> = None;

    for (i, tok) in tokens.iter().enumerate() {
        let tt = tok.token_type;

        if tt == TokenType::Eof {
            break;
        }
        // A declaration keyword must be followed immediately by its name;
        // anything else means the buffer is mid-edit — drop the pending decl.
        if pending_decl.is_some() && tt != TokenType::Ident {
            pending_decl = None;
        }
        if import_mode && tok.position.line != import_line {
            import_mode = false;
        }

        match tt {
            // ── declaration keywords ───────────────────────────────────────
            TokenType::Fn => {
                emit(snapshot, tok.span(), TT_KEYWORD, 0, out);
                pending_decl = Some((TT_FUNCTION, MOD_DECLARATION));
            }
            TokenType::Let => {
                emit(snapshot, tok.span(), TT_KEYWORD, 0, out);
                pending_decl = Some((TT_VARIABLE, MOD_DECLARATION | MOD_READONLY));
            }
            TokenType::Data => {
                emit(snapshot, tok.span(), TT_KEYWORD, 0, out);
                pending_decl = Some((TT_ENUM, MOD_DECLARATION));
            }
            TokenType::Effect => {
                emit(snapshot, tok.span(), TT_KEYWORD, 0, out);
                pending_decl = Some((TT_INTERFACE, MOD_DECLARATION));
            }
            TokenType::Class => {
                emit(snapshot, tok.span(), TT_KEYWORD, 0, out);
                pending_decl = Some((TT_CLASS, MOD_DECLARATION));
            }
            TokenType::Type | TokenType::Alias => {
                emit(snapshot, tok.span(), TT_KEYWORD, 0, out);
                pending_decl = Some((TT_TYPE, MOD_DECLARATION));
            }
            TokenType::Module => {
                emit(snapshot, tok.span(), TT_KEYWORD, 0, out);
                module_mode = true;
            }
            TokenType::Import => {
                emit(snapshot, tok.span(), TT_KEYWORD, 0, out);
                import_mode = true;
                import_line = tok.position.line;
            }

            // ── built-in constructors / literals that are keyword tokens ────
            TokenType::Some | TokenType::None | TokenType::Left | TokenType::Right => {
                emit(
                    snapshot,
                    tok.span(),
                    TT_ENUM_MEMBER,
                    MOD_DEFAULT_LIBRARY,
                    out,
                );
            }
            TokenType::True | TokenType::False => {
                emit(snapshot, tok.span(), TT_KEYWORD, 0, out);
            }

            // ── remaining keywords ─────────────────────────────────────────
            TokenType::Do
            | TokenType::Intrinsic
            | TokenType::Primop
            | TokenType::Public
            | TokenType::With
            | TokenType::If
            | TokenType::Else
            | TokenType::Return
            | TokenType::Match
            | TokenType::Select
            | TokenType::Where
            | TokenType::Handle
            | TokenType::Sealing
            | TokenType::Perform
            | TokenType::Instance
            | TokenType::Deriving
            | TokenType::As => {
                emit(snapshot, tok.span(), TT_KEYWORD, 0, out);
            }

            // ── literals ───────────────────────────────────────────────────
            TokenType::Int | TokenType::Float => {
                emit(snapshot, tok.span(), TT_NUMBER, 0, out);
            }
            TokenType::String
            | TokenType::StringEnd
            | TokenType::InterpolationStart
            | TokenType::UnterminatedString => {
                emit(snapshot, tok.span(), TT_STRING, 0, out);
            }
            TokenType::DocComment => {
                emit(snapshot, tok.span(), TT_COMMENT, MOD_DOCUMENTATION, out);
            }

            // ── operators ──────────────────────────────────────────────────
            TokenType::Plus
            | TokenType::Minus
            | TokenType::Asterisk
            | TokenType::Slash
            | TokenType::Percent
            | TokenType::Bang
            | TokenType::Lt
            | TokenType::Gt
            | TokenType::Lte
            | TokenType::Gte
            | TokenType::Eq
            | TokenType::NotEq
            | TokenType::Assign
            | TokenType::And
            | TokenType::Or
            | TokenType::Pipe
            | TokenType::Bar
            | TokenType::Arrow
            | TokenType::FatArrow
            | TokenType::LeftArrow
            | TokenType::Backslash
            | TokenType::DotDotDot => {
                emit(snapshot, tok.span(), TT_OPERATOR, 0, out);
            }

            // ── annotations ────────────────────────────────────────────────
            TokenType::At => {
                emit(snapshot, tok.span(), TT_DECORATOR, 0, out);
                decorator_pending = true;
            }

            // `{` ends a `module Name { … }` path.
            TokenType::LBrace => module_mode = false,

            // ── identifiers ────────────────────────────────────────────────
            TokenType::Ident => {
                let text = tok.literal.as_str();
                let (tt_idx, mods) = if decorator_pending {
                    decorator_pending = false;
                    (TT_DECORATOR, 0)
                } else if import_mode {
                    if text == "exposing" || text == "except" {
                        import_mode = false;
                        (TT_KEYWORD, 0)
                    } else {
                        (TT_NAMESPACE, 0)
                    }
                } else if module_mode {
                    (TT_NAMESPACE, MOD_DECLARATION)
                } else if let Some(decl) = pending_decl.take() {
                    decl
                } else {
                    let next = tokens.get(i + 1).map(|t| t.token_type);
                    let next_paren = next == Some(TokenType::LParen);
                    let next_dot = next == Some(TokenType::Dot);
                    if prev == Some(TokenType::Dot) {
                        classify_member(text, next_paren, names, stdlib_members)
                    } else if first_is_upper(text) {
                        classify_upper(text, next_dot, names, snapshot)
                    } else {
                        classify_lower(text, next_paren, names)
                    }
                };
                emit(snapshot, tok.span(), tt_idx, mods, out);
            }

            // Pure delimiters (parens, braces, brackets, comma, colon, dot,
            // semicolon, hash) carry no semantic colour — the TextMate grammar
            // styles them, and no standard semantic type fits.
            _ => {}
        }

        prev = Some(tt);
    }
}

/// Classify an uppercase reference (a type, constructor, effect, class, or
/// module name used outside a member-access position). `next_is_dot` lets a
/// qualifier like `Array` in `Array.map` read as a namespace even though
/// `Array` is also a built-in type name.
fn classify_upper(
    name: &str,
    next_is_dot: bool,
    names: &NameSets,
    snapshot: &Snapshot,
) -> (u32, u32) {
    if next_is_dot {
        if snapshot.module_short_names.contains(name) {
            return (TT_NAMESPACE, MOD_DEFAULT_LIBRARY);
        }
        if names.modules.contains(name) {
            return (TT_NAMESPACE, 0);
        }
    }
    if BUILTIN_TYPES.contains(&name) {
        (TT_TYPE, MOD_DEFAULT_LIBRARY)
    } else if BUILTIN_VARIANTS.contains(&name) {
        (TT_ENUM_MEMBER, MOD_DEFAULT_LIBRARY)
    } else if names.variants.contains(name) {
        (TT_ENUM_MEMBER, 0)
    } else if names.data_types.contains(name) {
        (TT_ENUM, 0)
    } else if names.effects.contains(name) {
        (TT_INTERFACE, 0)
    } else if names.classes.contains(name) {
        (TT_CLASS, 0)
    } else if names.type_aliases.contains(name) {
        (TT_TYPE, 0)
    } else if snapshot.module_short_names.contains(name) {
        (TT_NAMESPACE, MOD_DEFAULT_LIBRARY)
    } else if names.modules.contains(name) {
        (TT_NAMESPACE, 0)
    } else {
        // Unknown uppercase identifier — treat as a type (matches the grammar's
        // "uppercase starts a type" default).
        (TT_TYPE, 0)
    }
}

/// Classify a lowercase reference (parameter, type variable, function, field,
/// or local variable).
fn classify_lower(name: &str, next_is_paren: bool, names: &NameSets) -> (u32, u32) {
    if names.params.contains(name) {
        (TT_PARAMETER, 0)
    } else if names.type_params.contains(name) {
        (TT_TYPE_PARAMETER, 0)
    } else if names.functions.contains(name) {
        (TT_FUNCTION, 0)
    } else if next_is_paren {
        // Calling something we don't otherwise know — still a function use.
        (TT_FUNCTION, 0)
    } else if names.fields.contains(name) {
        (TT_PROPERTY, 0)
    } else {
        // Everything is immutably bound in Flux, so locals read as `readonly`.
        (TT_VARIABLE, MOD_READONLY)
    }
}

/// Classify the member of a `obj.member` access.
fn classify_member(
    name: &str,
    next_is_paren: bool,
    names: &NameSets,
    stdlib_members: &HashSet<&str>,
) -> (u32, u32) {
    if stdlib_members.contains(name) {
        (TT_METHOD, MOD_DEFAULT_LIBRARY)
    } else if next_is_paren || names.functions.contains(name) {
        (TT_METHOD, 0)
    } else {
        (TT_PROPERTY, 0)
    }
}

fn first_is_upper(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_uppercase())
}

// ── Span → LSP token conversion ─────────────────────────────────────────────

/// Emit `span` as one or more single-line LSP tokens (semantic tokens may not
/// span lines), with the length measured in the negotiated encoding via the
/// position map.
fn emit(
    snapshot: &Snapshot,
    span: FluxSpan,
    token_type: u32,
    modifiers: u32,
    out: &mut Vec<RawToken>,
) {
    let pm = &snapshot.position_map;
    let (Some(start_ts), Some(end_ts)) =
        (pm.flux_to_offset(span.start), pm.flux_to_offset(span.end))
    else {
        return;
    };
    let start = u32::from(start_ts) as usize;
    let bytes = snapshot.text.as_bytes();
    let end = (u32::from(end_ts) as usize).min(bytes.len());
    if end <= start {
        return;
    }

    let mut seg_start = start;
    for (idx, &b) in bytes[start..end].iter().enumerate() {
        if b == b'\n' {
            push_segment(snapshot, seg_start, start + idx, token_type, modifiers, out);
            seg_start = start + idx + 1;
        }
    }
    push_segment(snapshot, seg_start, end, token_type, modifiers, out);
}

fn push_segment(
    snapshot: &Snapshot,
    start: usize,
    end: usize,
    token_type: u32,
    modifiers: u32,
    out: &mut Vec<RawToken>,
) {
    let bytes = snapshot.text.as_bytes();
    // Drop a trailing CR so CRLF line breaks don't pad the token.
    let end = if end > start && bytes[end - 1] == b'\r' {
        end - 1
    } else {
        end
    };
    if end <= start {
        return;
    }
    let pm = &snapshot.position_map;
    let s = pm.offset_to_lsp(TextSize::from(start as u32));
    let e = pm.offset_to_lsp(TextSize::from(end as u32));
    let length = e.character.saturating_sub(s.character);
    if length == 0 {
        return;
    }
    out.push(RawToken {
        line: s.line,
        start: s.character,
        length,
        token_type,
        modifiers,
    });
}

fn delta_encode(sorted: Vec<RawToken>) -> Vec<SemanticToken> {
    let mut result = Vec::with_capacity(sorted.len());
    let mut prev_line = 0u32;
    let mut prev_start = 0u32;
    for tok in sorted {
        let delta_line = tok.line - prev_line;
        let delta_start = if delta_line == 0 {
            tok.start - prev_start
        } else {
            tok.start
        };
        result.push(SemanticToken {
            delta_line,
            delta_start,
            length: tok.length,
            token_type: tok.token_type,
            token_modifiers_bitset: tok.modifiers,
        });
        prev_line = tok.line;
        prev_start = tok.start;
    }
    result
}

// ── AST-derived name sets ────────────────────────────────────────────────────

/// Names harvested from the buffer's AST, keyed by the role they play. The
/// classifier consults these to decide what a bare identifier reference means.
/// Sets are global (unscoped) — a deliberate approximation that keeps colouring
/// cheap and stable; clashes (a parameter named like a top-level function) are
/// resolved by the lookup order in `classify_lower`.
#[derive(Default)]
struct NameSets {
    functions: HashSet<String>,
    data_types: HashSet<String>,
    variants: HashSet<String>,
    effects: HashSet<String>,
    classes: HashSet<String>,
    type_aliases: HashSet<String>,
    type_params: HashSet<String>,
    params: HashSet<String>,
    fields: HashSet<String>,
    modules: HashSet<String>,
}

impl NameSets {
    fn collect(program: &flux::syntax::program::Program, interner: &Interner) -> Self {
        let mut sets = NameSets::default();
        for stmt in &program.statements {
            sets.collect_stmt(stmt, interner);
        }
        sets
    }

    fn insert(set: &mut HashSet<String>, interner: &Interner, id: Identifier) {
        if let Some(name) = interner.try_resolve(id)
            && !name.is_empty()
        {
            set.insert(name.to_string());
        }
    }

    /// Add each dotted segment of a module path (`Flow.List` → `Flow`, `List`).
    fn insert_module_path(&mut self, interner: &Interner, id: Identifier) {
        if let Some(name) = interner.try_resolve(id) {
            for segment in name.split('.') {
                if !segment.is_empty() {
                    self.modules.insert(segment.to_string());
                }
            }
        }
    }

    fn collect_stmt(&mut self, stmt: &Statement, interner: &Interner) {
        match stmt {
            Statement::Function {
                name,
                type_params,
                parameters,
                body,
                ..
            } => {
                Self::insert(&mut self.functions, interner, *name);
                for tp in type_params {
                    Self::insert(&mut self.type_params, interner, tp.name);
                    for c in &tp.constraints {
                        Self::insert(&mut self.classes, interner, *c);
                    }
                }
                for p in parameters {
                    Self::insert(&mut self.params, interner, *p);
                }
                self.collect_block(body, interner);
            }
            Statement::Let { name, value, .. } => {
                Self::insert(&mut self.params, interner, *name);
                self.collect_expr(value, interner);
            }
            Statement::LetDestructure { pattern, value, .. } => {
                self.collect_pattern(pattern, interner);
                self.collect_expr(value, interner);
            }
            Statement::Assign { name, value, .. } => {
                Self::insert(&mut self.params, interner, *name);
                self.collect_expr(value, interner);
            }
            Statement::Return { value: Some(v), .. } => self.collect_expr(v, interner),
            Statement::Expression { expression, .. } => self.collect_expr(expression, interner),
            Statement::Module { name, body, .. } => {
                self.insert_module_path(interner, *name);
                self.collect_block(body, interner);
            }
            Statement::Import { name, alias, .. } => {
                self.insert_module_path(interner, *name);
                if let Some(alias) = alias {
                    Self::insert(&mut self.modules, interner, *alias);
                }
            }
            Statement::Data {
                name,
                type_params,
                variants,
                ..
            } => {
                Self::insert(&mut self.data_types, interner, *name);
                for tp in type_params {
                    Self::insert(&mut self.type_params, interner, *tp);
                }
                for v in variants {
                    Self::insert(&mut self.variants, interner, v.name);
                    if let Some(field_names) = &v.field_names {
                        for f in field_names {
                            Self::insert(&mut self.fields, interner, *f);
                        }
                    }
                }
            }
            Statement::EffectDecl { name, ops, .. } => {
                Self::insert(&mut self.effects, interner, *name);
                for op in ops {
                    Self::insert(&mut self.functions, interner, op.name);
                }
            }
            Statement::EffectAlias { name, .. } => {
                Self::insert(&mut self.effects, interner, *name);
            }
            Statement::TypeAlias(alias) => {
                Self::insert(&mut self.type_aliases, interner, alias.name);
                for p in &alias.params {
                    Self::insert(&mut self.type_params, interner, *p);
                }
            }
            Statement::Class {
                name,
                type_params,
                methods,
                ..
            } => {
                Self::insert(&mut self.classes, interner, *name);
                for tp in type_params {
                    Self::insert(&mut self.type_params, interner, *tp);
                }
                for m in methods {
                    Self::insert(&mut self.functions, interner, m.name);
                    for tp in &m.type_params {
                        Self::insert(&mut self.type_params, interner, *tp);
                    }
                    for p in &m.params {
                        Self::insert(&mut self.params, interner, *p);
                    }
                    if let Some(body) = &m.default_body {
                        self.collect_block(body, interner);
                    }
                }
            }
            Statement::Instance {
                class_name,
                methods,
                ..
            } => {
                Self::insert(&mut self.classes, interner, *class_name);
                for m in methods {
                    Self::insert(&mut self.functions, interner, m.name);
                    for p in &m.params {
                        Self::insert(&mut self.params, interner, *p);
                    }
                    self.collect_block(&m.body, interner);
                }
            }
            _ => {}
        }
    }

    fn collect_block(&mut self, block: &Block, interner: &Interner) {
        for stmt in &block.statements {
            self.collect_stmt(stmt, interner);
        }
    }

    fn collect_expr(&mut self, expr: &Expression, interner: &Interner) {
        match expr {
            Expression::Function {
                parameters, body, ..
            } => {
                for p in parameters {
                    Self::insert(&mut self.params, interner, *p);
                }
                self.collect_block(body, interner);
            }
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                self.collect_expr(function, interner);
                for a in arguments {
                    self.collect_expr(a, interner);
                }
            }
            Expression::Infix { left, right, .. } => {
                self.collect_expr(left, interner);
                self.collect_expr(right, interner);
            }
            Expression::Prefix { right, .. } => self.collect_expr(right, interner),
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                self.collect_expr(condition, interner);
                self.collect_block(consequence, interner);
                if let Some(alt) = alternative {
                    self.collect_block(alt, interner);
                }
            }
            Expression::DoBlock { block, .. } => self.collect_block(block, interner),
            Expression::Match {
                scrutinee, arms, ..
            } => {
                self.collect_expr(scrutinee, interner);
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        self.collect_expr(guard, interner);
                    }
                    self.collect_expr(&arm.body, interner);
                }
            }
            Expression::Handle { expr, arms, .. } => {
                self.collect_expr(expr, interner);
                for arm in arms {
                    Self::insert(&mut self.params, interner, arm.resume_param);
                    for p in &arm.params {
                        Self::insert(&mut self.params, interner, *p);
                    }
                    self.collect_expr(&arm.body, interner);
                }
            }
            Expression::Sealing { expr, .. } => self.collect_expr(expr, interner),
            Expression::ListLiteral { elements, .. }
            | Expression::ArrayLiteral { elements, .. }
            | Expression::TupleLiteral { elements, .. } => {
                for e in elements {
                    self.collect_expr(e, interner);
                }
            }
            Expression::Index { left, index, .. } => {
                self.collect_expr(left, interner);
                self.collect_expr(index, interner);
            }
            Expression::Hash { pairs, .. } => {
                for (k, v) in pairs {
                    self.collect_expr(k, interner);
                    self.collect_expr(v, interner);
                }
            }
            Expression::MemberAccess { object, .. }
            | Expression::TupleFieldAccess { object, .. } => self.collect_expr(object, interner),
            Expression::Some { value, .. }
            | Expression::Left { value, .. }
            | Expression::Right { value, .. } => self.collect_expr(value, interner),
            Expression::Cons { head, tail, .. } => {
                self.collect_expr(head, interner);
                self.collect_expr(tail, interner);
            }
            Expression::Perform { args, .. } => {
                for a in args {
                    self.collect_expr(a, interner);
                }
            }
            Expression::NamedConstructor { fields, .. } => {
                for f in fields {
                    Self::insert(&mut self.fields, interner, f.name);
                    if let Some(v) = &f.value {
                        self.collect_expr(v, interner);
                    }
                }
            }
            Expression::Spread {
                base, overrides, ..
            } => {
                self.collect_expr(base, interner);
                for f in overrides {
                    Self::insert(&mut self.fields, interner, f.name);
                    if let Some(v) = &f.value {
                        self.collect_expr(v, interner);
                    }
                }
            }
            _ => {}
        }
    }

    fn collect_pattern(&mut self, pat: &Pattern, interner: &Interner) {
        match pat {
            Pattern::Identifier { name, .. } => Self::insert(&mut self.params, interner, *name),
            Pattern::Tuple { elements, .. } => {
                for e in elements {
                    self.collect_pattern(e, interner);
                }
            }
            Pattern::Constructor { fields, .. } => {
                for f in fields {
                    self.collect_pattern(f, interner);
                }
            }
            Pattern::NamedConstructor { fields, .. } => {
                for f in fields {
                    if let Some(p) = &f.pattern {
                        self.collect_pattern(p, interner);
                    }
                }
            }
            Pattern::Some { pattern, .. }
            | Pattern::Left { pattern, .. }
            | Pattern::Right { pattern, .. } => self.collect_pattern(pattern, interner),
            Pattern::Cons { head, tail, .. } => {
                self.collect_pattern(head, interner);
                self.collect_pattern(tail, interner);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux::syntax::parser::Parser;
    use lsp_types::Position;

    fn name_sets(src: &str) -> NameSets {
        let mut parser = Parser::new(Lexer::new(src.to_string()));
        let program = parser.parse_program();
        let interner = parser.take_interner();
        NameSets::collect(&program, &interner)
    }

    #[test]
    fn legend_indices_stay_in_range() {
        let types = token_types();
        let mods = token_modifiers();
        // The highest declared index must address a real legend entry.
        assert_eq!(types.len(), 18);
        assert_eq!(TT_DECORATOR as usize, types.len() - 1);
        assert_eq!(mods.len(), 4);
        // Highest modifier bit must be within the declared modifier count.
        assert!((MOD_DOCUMENTATION.trailing_zeros() as usize) < mods.len());
    }

    #[test]
    fn first_is_upper_distinguishes_case() {
        assert!(first_is_upper("Foo"));
        assert!(!first_is_upper("foo"));
        assert!(!first_is_upper("_x"));
        assert!(!first_is_upper(""));
    }

    #[test]
    fn delta_encode_is_relative_to_previous_token() {
        let raw = vec![
            RawToken {
                line: 0,
                start: 4,
                length: 3,
                token_type: TT_FUNCTION,
                modifiers: MOD_DECLARATION,
            },
            RawToken {
                line: 0,
                start: 8,
                length: 1,
                token_type: TT_PARAMETER,
                modifiers: 0,
            },
            RawToken {
                line: 2,
                start: 2,
                length: 5,
                token_type: TT_VARIABLE,
                modifiers: MOD_READONLY,
            },
        ];
        let encoded = delta_encode(raw);
        // Same line → delta_start relative to previous start (8 - 4 = 4).
        assert_eq!(encoded[1].delta_line, 0);
        assert_eq!(encoded[1].delta_start, 4);
        // New line → delta_start is absolute again.
        assert_eq!(encoded[2].delta_line, 2);
        assert_eq!(encoded[2].delta_start, 2);
        assert_eq!(encoded[0].token_modifiers_bitset, MOD_DECLARATION);
    }

    #[test]
    fn name_sets_capture_declaration_roles() {
        let sets = name_sets(
            "import Flow.List as List\n\
             data Color { Red, Green }\n\
             fn shade(x, factor) { x }\n",
        );
        assert!(sets.functions.contains("shade"));
        assert!(sets.params.contains("x"));
        assert!(sets.params.contains("factor"));
        assert!(sets.data_types.contains("Color"));
        assert!(sets.variants.contains("Red"));
        assert!(sets.variants.contains("Green"));
        // Both the dotted path segments and the alias become known modules.
        assert!(sets.modules.contains("Flow"));
        assert!(sets.modules.contains("List"));
    }

    fn tok(delta_line: u32, delta_start: u32, length: u32, ty: u32) -> SemanticToken {
        SemanticToken {
            delta_line,
            delta_start,
            length,
            token_type: ty,
            token_modifiers_bitset: 0,
        }
    }

    #[test]
    fn diff_tokens_identical_yields_no_edits() {
        let a = vec![tok(0, 0, 2, TT_KEYWORD), tok(0, 3, 4, TT_FUNCTION)];
        assert!(diff_tokens(&a, &a).is_empty());
    }

    #[test]
    fn diff_tokens_splices_changed_middle() {
        let old = vec![
            tok(0, 0, 2, TT_KEYWORD),
            tok(0, 3, 4, TT_FUNCTION),
            tok(1, 0, 1, TT_PARAMETER),
        ];
        // Middle token changes type; prefix (1 token) and suffix (1 token) hold.
        let new = vec![
            tok(0, 0, 2, TT_KEYWORD),
            tok(0, 3, 4, TT_VARIABLE),
            tok(1, 0, 1, TT_PARAMETER),
        ];
        let edits = diff_tokens(&old, &new);
        assert_eq!(edits.len(), 1);
        // Units are integers (5 per token): skip 1 token, replace 1 token.
        assert_eq!(edits[0].start, 5);
        assert_eq!(edits[0].delete_count, 5);
        assert_eq!(edits[0].data.as_ref().map(Vec::len), Some(1));
        assert_eq!(edits[0].data.as_ref().unwrap()[0], new[1]);
    }

    #[test]
    fn diff_tokens_handles_pure_insertion() {
        let old = vec![tok(0, 0, 2, TT_KEYWORD)];
        let new = vec![tok(0, 0, 2, TT_KEYWORD), tok(0, 3, 4, TT_FUNCTION)];
        let edits = diff_tokens(&old, &new);
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].start, 5);
        assert_eq!(edits[0].delete_count, 0);
        assert_eq!(edits[0].data.as_ref().map(Vec::len), Some(1));
    }

    #[test]
    fn token_in_range_filters_by_line_and_column() {
        let r = Range {
            start: Position {
                line: 1,
                character: 0,
            },
            end: Position {
                line: 3,
                character: 0,
            },
        };
        let on_line_0 = RawToken {
            line: 0,
            start: 0,
            length: 2,
            token_type: TT_KEYWORD,
            modifiers: 0,
        };
        let in_middle = RawToken {
            line: 2,
            start: 4,
            length: 3,
            token_type: TT_FUNCTION,
            modifiers: 0,
        };
        // The end line is exclusive at character 0 → a token there is excluded.
        let on_end_line = RawToken {
            line: 3,
            start: 0,
            length: 1,
            token_type: TT_PARAMETER,
            modifiers: 0,
        };
        assert!(!token_in_range(&on_line_0, r));
        assert!(token_in_range(&in_middle, r));
        assert!(!token_in_range(&on_end_line, r));
    }

    #[test]
    fn name_sets_recurse_into_nested_functions() {
        let sets = name_sets(
            "fn outer(xs) {\n\
                 fn helper(acc) { acc }\n\
                 helper(xs)\n\
             }\n",
        );
        assert!(sets.functions.contains("outer"));
        assert!(sets.functions.contains("helper"));
        assert!(sets.params.contains("acc"));
    }
}
