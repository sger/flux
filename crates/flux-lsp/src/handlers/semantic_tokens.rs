use flux::diagnostics::position::Position as FluxPosition;
use flux::syntax::Identifier;
use flux::syntax::block::Block;
use flux::syntax::expression::{Expression, Pattern};
use flux::syntax::statement::Statement;
use lsp_types::{SemanticToken, SemanticTokenType, SemanticTokens, SemanticTokensLegend};

use crate::snapshot::Snapshot;

// Token type indices — order matches TOKEN_TYPES below.
const TT_NAMESPACE: u32 = 0;
const TT_TYPE: u32 = 1;
const TT_STRUCT: u32 = 2;
const TT_CLASS: u32 = 3;
const TT_INTERFACE: u32 = 4;
const TT_FUNCTION: u32 = 5;
const TT_VARIABLE: u32 = 6;
const TT_PARAMETER: u32 = 7;
const TT_PROPERTY: u32 = 8;

const TOKEN_TYPES: &[&str] = &[
    "namespace", // 0
    "type",      // 1
    "struct",    // 2
    "class",     // 3
    "interface", // 4
    "function",  // 5
    "variable",  // 6
    "parameter", // 7
    "property",  // 8
];

pub fn semantic_tokens_legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: TOKEN_TYPES
            .iter()
            .map(|s| SemanticTokenType::new(s))
            .collect(),
        token_modifiers: vec![],
    }
}

pub fn semantic_tokens(snapshot: &Snapshot) -> SemanticTokens {
    let mut raw: Vec<RawToken> = Vec::new();
    for stmt in &snapshot.program.statements {
        collect_stmt(stmt, snapshot, &mut raw);
    }
    raw.sort_by_key(|t| (t.line, t.start));
    raw.dedup_by_key(|t| (t.line, t.start));
    SemanticTokens {
        result_id: None,
        data: delta_encode(raw),
    }
}

#[derive(Clone)]
struct RawToken {
    line: u32,
    start: u32,
    length: u32,
    token_type: u32,
}

fn push_ident(
    snapshot: &Snapshot,
    flux_pos: FluxPosition,
    name: Identifier,
    token_type: u32,
    out: &mut Vec<RawToken>,
) {
    let Some(text) = snapshot.interner.try_resolve(name) else {
        return;
    };
    let lsp_pos = snapshot.position_map.flux_to_lsp(flux_pos);
    out.push(RawToken {
        line: lsp_pos.line,
        start: lsp_pos.character,
        length: text.len() as u32,
        token_type,
    });
}

/// Compute the name start position for keyword-prefixed declarations.
/// Mirrors `decl_name_start` in locator.rs.
fn name_start(stmt_start: FluxPosition, is_public: bool, keyword: &str) -> FluxPosition {
    let mut col = stmt_start.column;
    if is_public {
        col += "public ".len();
    }
    col += keyword.len() + 1;
    FluxPosition {
        line: stmt_start.line,
        column: col,
    }
}

fn collect_stmt(stmt: &Statement, snapshot: &Snapshot, out: &mut Vec<RawToken>) {
    match stmt {
        Statement::Let {
            name,
            value,
            span,
            is_public,
            ..
        } => {
            let pos = name_start(span.start, *is_public, "let");
            push_ident(snapshot, pos, *name, TT_VARIABLE, out);
            collect_expr(value, snapshot, out);
        }
        Statement::LetDestructure { pattern, value, .. } => {
            collect_pattern(pattern, snapshot, out);
            collect_expr(value, snapshot, out);
        }
        Statement::Function {
            name,
            parameters,
            body,
            span,
            is_public,
            ..
        } => {
            let pos = name_start(span.start, *is_public, "fn");
            push_ident(snapshot, pos, *name, TT_FUNCTION, out);
            // Parameters: approximated from function span start + "fn <name>(" offset.
            // The parser does not record individual parameter name positions,
            // so we use the same approximation as locator.rs's FunctionParameter arm.
            let name_text = snapshot.interner.try_resolve(*name).unwrap_or("");
            let paren_col = pos.column + name_text.len() + 1; // +1 for "("
            let mut param_col = paren_col;
            for param in parameters {
                let param_text = snapshot.interner.try_resolve(*param).unwrap_or("");
                push_ident(
                    snapshot,
                    FluxPosition {
                        line: span.start.line,
                        column: param_col,
                    },
                    *param,
                    TT_PARAMETER,
                    out,
                );
                param_col += param_text.len() + 2; // +2 for ", "
            }
            for s in &body.statements {
                collect_stmt(s, snapshot, out);
            }
        }
        Statement::Data {
            name,
            variants,
            span,
            is_public,
            ..
        } => {
            let pos = name_start(span.start, *is_public, "data");
            push_ident(snapshot, pos, *name, TT_STRUCT, out);
            for variant in variants {
                push_ident(snapshot, variant.span.start, variant.name, TT_CLASS, out);
                if let Some(field_names) = &variant.field_names {
                    for field_name in field_names {
                        push_ident(snapshot, variant.span.start, *field_name, TT_PROPERTY, out);
                    }
                }
            }
        }
        Statement::EffectDecl {
            name, ops, span, ..
        } => {
            let pos = name_start(span.start, false, "effect");
            push_ident(snapshot, pos, *name, TT_INTERFACE, out);
            for op in ops {
                push_ident(snapshot, op.span.start, op.name, TT_INTERFACE, out);
            }
        }
        Statement::TypeAlias(alias) => {
            let pos = name_start(alias.span.start, alias.is_public, "alias");
            push_ident(snapshot, pos, alias.name, TT_TYPE, out);
        }
        Statement::Module {
            name, body, span, ..
        } => {
            let pos = name_start(span.start, false, "module");
            push_ident(snapshot, pos, *name, TT_NAMESPACE, out);
            collect_block(body, snapshot, out);
        }
        Statement::Import { name, span, .. } => {
            // "import " is 7 bytes.
            let pos = FluxPosition {
                line: span.start.line,
                column: span.start.column + "import ".len(),
            };
            push_ident(snapshot, pos, *name, TT_NAMESPACE, out);
        }
        Statement::Expression { expression, .. } => collect_expr(expression, snapshot, out),
        Statement::Return { value: Some(v), .. } => collect_expr(v, snapshot, out),
        Statement::Assign {
            name, value, span, ..
        } => {
            push_ident(snapshot, span.start, *name, TT_VARIABLE, out);
            collect_expr(value, snapshot, out);
        }
        _ => {}
    }
}

fn collect_expr(expr: &Expression, snapshot: &Snapshot, out: &mut Vec<RawToken>) {
    match expr {
        Expression::Call {
            function,
            arguments,
            ..
        } => {
            collect_expr(function, snapshot, out);
            for a in arguments {
                collect_expr(a, snapshot, out);
            }
        }
        Expression::MemberAccess {
            object,
            member,
            span,
            ..
        } => {
            collect_expr(object, snapshot, out);
            // Member is at end of span; compute its start from the identifier text.
            let member_text = snapshot.interner.try_resolve(*member).unwrap_or("");
            let member_col = span.end.column.saturating_sub(member_text.len());
            push_ident(
                snapshot,
                FluxPosition {
                    line: span.end.line,
                    column: member_col,
                },
                *member,
                TT_PROPERTY,
                out,
            );
        }
        Expression::NamedConstructor {
            name, fields, span, ..
        } => {
            push_ident(snapshot, span.start, *name, TT_CLASS, out);
            for f in fields {
                push_ident(snapshot, f.span.start, f.name, TT_PROPERTY, out);
                if let Some(v) = &f.value {
                    collect_expr(v, snapshot, out);
                }
            }
        }
        Expression::Perform {
            operation,
            args,
            span,
            ..
        } => {
            push_ident(snapshot, span.start, *operation, TT_INTERFACE, out);
            for a in args {
                collect_expr(a, snapshot, out);
            }
        }
        Expression::Function { body, .. } => collect_block(body, snapshot, out),
        Expression::DoBlock { block, .. } => collect_block(block, snapshot, out),
        Expression::If {
            condition,
            consequence,
            alternative,
            ..
        } => {
            collect_expr(condition, snapshot, out);
            collect_block(consequence, snapshot, out);
            if let Some(b) = alternative {
                collect_block(b, snapshot, out);
            }
        }
        Expression::Match {
            scrutinee, arms, ..
        } => {
            collect_expr(scrutinee, snapshot, out);
            for arm in arms {
                collect_expr(&arm.body, snapshot, out);
            }
        }
        Expression::Infix { left, right, .. } => {
            collect_expr(left, snapshot, out);
            collect_expr(right, snapshot, out);
        }
        Expression::Prefix { right, .. } => collect_expr(right, snapshot, out),
        Expression::Handle { expr, arms, .. } => {
            collect_expr(expr, snapshot, out);
            for arm in arms {
                collect_expr(&arm.body, snapshot, out);
            }
        }
        Expression::ListLiteral { elements, .. } | Expression::ArrayLiteral { elements, .. } => {
            for e in elements {
                collect_expr(e, snapshot, out);
            }
        }
        Expression::TupleLiteral { elements, .. } => {
            for e in elements {
                collect_expr(e, snapshot, out);
            }
        }
        Expression::Spread {
            base, overrides, ..
        } => {
            collect_expr(base, snapshot, out);
            for f in overrides {
                push_ident(snapshot, f.span.start, f.name, TT_PROPERTY, out);
                if let Some(v) = &f.value {
                    collect_expr(v, snapshot, out);
                }
            }
        }
        Expression::Index { left, index, .. } => {
            collect_expr(left, snapshot, out);
            collect_expr(index, snapshot, out);
        }
        Expression::Cons { head, tail, .. } => {
            collect_expr(head, snapshot, out);
            collect_expr(tail, snapshot, out);
        }
        Expression::Some { value, .. }
        | Expression::Left { value, .. }
        | Expression::Right { value, .. } => collect_expr(value, snapshot, out),
        _ => {}
    }
}

fn collect_pattern(pat: &Pattern, snapshot: &Snapshot, out: &mut Vec<RawToken>) {
    match pat {
        Pattern::Identifier { name, span } => {
            push_ident(snapshot, span.start, *name, TT_VARIABLE, out);
        }
        Pattern::Tuple { elements, .. } => {
            for e in elements {
                collect_pattern(e, snapshot, out);
            }
        }
        Pattern::Constructor { fields, .. } => {
            for f in fields {
                collect_pattern(f, snapshot, out);
            }
        }
        Pattern::NamedConstructor {
            name, fields, span, ..
        } => {
            push_ident(snapshot, span.start, *name, TT_CLASS, out);
            for f in fields {
                if let Some(p) = &f.pattern {
                    collect_pattern(p, snapshot, out);
                }
            }
        }
        Pattern::Some { pattern, .. }
        | Pattern::Left { pattern, .. }
        | Pattern::Right { pattern, .. } => {
            collect_pattern(pattern, snapshot, out);
        }
        Pattern::Cons { head, tail, .. } => {
            collect_pattern(head, snapshot, out);
            collect_pattern(tail, snapshot, out);
        }
        _ => {}
    }
}

fn collect_block(block: &Block, snapshot: &Snapshot, out: &mut Vec<RawToken>) {
    for s in &block.statements {
        collect_stmt(s, snapshot, out);
    }
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
            token_modifiers_bitset: 0,
        });
        prev_line = tok.line;
        prev_start = tok.start;
    }
    result
}
