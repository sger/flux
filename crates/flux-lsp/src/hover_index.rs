use crate::line_index::PositionMap;
use flux::diagnostics::position::{Position as FluxPosition, Span as FluxSpan};
use flux::syntax::Identifier;
use flux::syntax::block::Block;
use flux::syntax::effect_expr::EffectExpr;
use flux::syntax::expression::{ExprId, Expression, HandleArm, MatchArm, Pattern, StringPart};
use flux::syntax::program::Program;
use flux::syntax::statement::Statement;
use flux::syntax::type_expr::TypeExpr;
use lsp_types::Position as LspPosition;

/// What kind of hover information a span covers.
#[derive(Debug, Clone)]
pub enum HoverTarget {
    /// A value expression — hover renders `infer.expr_types[id]`.
    Expr(ExprId),
    /// `let`/`fn` binding — hover renders the resolved scheme via
    /// `resolved_binding_schemes_by_span`, keyed by the statement span.
    DeclName {
        name: Identifier,
        binding_span: FluxSpan,
    },
    /// `IO`, `Time`, ... — hover shows `effect: <name>`.
    EffectName(Identifier),
    /// Effect row variable like `|e` — hover shows `row var: <name>`.
    EffectRowVar(Identifier),
    /// `Int`, `String`, ... in a type annotation — hover shows `type: <name>`.
    TypeName(Identifier),
}

/// Position → innermost hover target index.
///
/// Entries are stored in document order, then sorted so a reverse scan picks
/// the innermost containing span first.
pub struct HoverIndex {
    entries: Vec<Entry>,
}

#[derive(Clone)]
struct Entry {
    span: FluxSpan,
    target: HoverTarget,
}

impl HoverIndex {
    pub fn build(program: &Program) -> Self {
        let mut entries = Vec::new();
        for stmt in &program.statements {
            collect_stmt(stmt, &mut entries);
        }
        entries.sort_by_key(|e| {
            (
                e.span.start.line,
                e.span.start.column,
                std::cmp::Reverse((e.span.end.line, e.span.end.column)),
            )
        });
        Self { entries }
    }

    pub fn target_at(
        &self,
        position: LspPosition,
        position_map: &PositionMap,
    ) -> Option<&HoverTarget> {
        let target = position_map.lsp_to_flux(position)?;
        self.entries
            .iter()
            .rev()
            .find(|e| span_contains(&e.span, target))
            .map(|e| &e.target)
    }
}

fn span_contains(span: &FluxSpan, p: FluxPosition) -> bool {
    let after_start = (span.start.line, span.start.column) <= (p.line, p.column);
    let before_end = (p.line, p.column) <= (span.end.line, span.end.column);
    after_start && before_end
}

// ---------------------------------------------------------------------------
// Walkers — exhaustively descend the AST so future variants surface as compile
// errors via the existing `walk_*` patterns mirrored in `src/ast/visit.rs`.
// ---------------------------------------------------------------------------

fn collect_stmt(stmt: &Statement, out: &mut Vec<Entry>) {
    match stmt {
        Statement::Let {
            name,
            type_annotation,
            value,
            span,
            ..
        } => {
            out.push(Entry {
                span: *span,
                target: HoverTarget::DeclName {
                    name: *name,
                    binding_span: *span,
                },
            });
            if let Some(t) = type_annotation {
                collect_type(t, out);
            }
            collect_expr(value, out);
        }
        Statement::LetDestructure { pattern, value, .. } => {
            collect_pat(pattern, out);
            collect_expr(value, out);
        }
        Statement::Return { value, .. } => {
            if let Some(v) = value {
                collect_expr(v, out);
            }
        }
        Statement::Expression { expression, .. } => collect_expr(expression, out),
        Statement::Function {
            name,
            parameter_types,
            return_type,
            effects,
            body,
            span,
            ..
        } => {
            out.push(Entry {
                span: *span,
                target: HoverTarget::DeclName {
                    name: *name,
                    binding_span: *span,
                },
            });
            for pt in parameter_types.iter().flatten() {
                collect_type(pt, out);
            }
            if let Some(rt) = return_type {
                collect_type(rt, out);
            }
            for e in effects {
                collect_effect(e, out);
            }
            collect_block(body, out);
        }
        Statement::Assign { value, .. } => collect_expr(value, out),
        Statement::Module { body, .. } => collect_block(body, out),
        Statement::Import { .. } => {}
        Statement::Data { variants: _, .. } => {
            // Variants carry TypeExpr fields; walking is best-effort and not
            // strictly required for M2a (no hover info there yet).
        }
        Statement::EffectDecl { ops: _, .. } => {}
        Statement::EffectAlias { expansion, .. } => collect_effect(expansion, out),
        Statement::TypeAlias(alias) => collect_type(&alias.body, out),
        Statement::Class { .. } | Statement::Instance { .. } => {}
    }
}

fn collect_block(block: &Block, out: &mut Vec<Entry>) {
    for stmt in &block.statements {
        collect_stmt(stmt, out);
    }
}

fn collect_expr(expr: &Expression, out: &mut Vec<Entry>) {
    out.push(Entry {
        span: expr.span(),
        target: HoverTarget::Expr(expr.expr_id()),
    });
    match expr {
        Expression::Identifier { .. }
        | Expression::Integer { .. }
        | Expression::Float { .. }
        | Expression::String { .. }
        | Expression::Boolean { .. }
        | Expression::EmptyList { .. }
        | Expression::None { .. } => {}
        Expression::InterpolatedString { parts, .. } => {
            for part in parts {
                if let StringPart::Interpolation(e) = part {
                    collect_expr(e, out);
                }
            }
        }
        Expression::Prefix { right, .. } => collect_expr(right, out),
        Expression::Infix { left, right, .. } => {
            collect_expr(left, out);
            collect_expr(right, out);
        }
        Expression::If {
            condition,
            consequence,
            alternative,
            ..
        } => {
            collect_expr(condition, out);
            collect_block(consequence, out);
            if let Some(b) = alternative {
                collect_block(b, out);
            }
        }
        Expression::DoBlock { block, .. } => collect_block(block, out),
        Expression::Function {
            parameter_types,
            return_type,
            effects,
            body,
            ..
        } => {
            for pt in parameter_types.iter().flatten() {
                collect_type(pt, out);
            }
            if let Some(rt) = return_type {
                collect_type(rt, out);
            }
            for e in effects {
                collect_effect(e, out);
            }
            collect_block(body, out);
        }
        Expression::Call {
            function,
            arguments,
            ..
        } => {
            collect_expr(function, out);
            for a in arguments {
                collect_expr(a, out);
            }
        }
        Expression::ListLiteral { elements, .. } | Expression::ArrayLiteral { elements, .. } => {
            for e in elements {
                collect_expr(e, out);
            }
        }
        Expression::TupleLiteral { elements, .. } => {
            for e in elements {
                collect_expr(e, out);
            }
        }
        Expression::Index { left, index, .. } => {
            collect_expr(left, out);
            collect_expr(index, out);
        }
        Expression::Hash { pairs, .. } => {
            for (k, v) in pairs {
                collect_expr(k, out);
                collect_expr(v, out);
            }
        }
        Expression::MemberAccess { object, .. } => collect_expr(object, out),
        Expression::TupleFieldAccess { object, .. } => collect_expr(object, out),
        Expression::Match {
            scrutinee, arms, ..
        } => {
            collect_expr(scrutinee, out);
            for arm in arms {
                collect_match_arm(arm, out);
            }
        }
        Expression::Some { value, .. }
        | Expression::Left { value, .. }
        | Expression::Right { value, .. } => collect_expr(value, out),
        Expression::Cons { head, tail, .. } => {
            collect_expr(head, out);
            collect_expr(tail, out);
        }
        Expression::Perform { args, .. } => {
            for a in args {
                collect_expr(a, out);
            }
        }
        Expression::Handle {
            expr,
            parameter,
            arms,
            ..
        } => {
            collect_expr(expr, out);
            if let Some(p) = parameter {
                collect_expr(p, out);
            }
            for arm in arms {
                collect_handle_arm(arm, out);
            }
        }
        Expression::Sealing { expr, allowed, .. } => {
            collect_expr(expr, out);
            for e in allowed {
                collect_effect(e, out);
            }
        }
        Expression::NamedConstructor { fields, .. } => {
            for field in fields {
                if let Some(v) = &field.value {
                    collect_expr(v, out);
                }
            }
        }
        Expression::Spread {
            base, overrides, ..
        } => {
            collect_expr(base, out);
            for field in overrides {
                if let Some(v) = &field.value {
                    collect_expr(v, out);
                }
            }
        }
    }
}

fn collect_handle_arm(arm: &HandleArm, out: &mut Vec<Entry>) {
    collect_expr(&arm.body, out);
}

fn collect_match_arm(arm: &MatchArm, out: &mut Vec<Entry>) {
    collect_pat(&arm.pattern, out);
    if let Some(g) = &arm.guard {
        collect_expr(g, out);
    }
    collect_expr(&arm.body, out);
}

fn collect_pat(_pat: &Pattern, _out: &mut Vec<Entry>) {
    // Patterns can carry identifiers and nested patterns. Hover on pattern
    // bindings is a follow-up; leaving the walker as a no-op for now keeps
    // M2a focused on the user's three reported gaps.
}

fn collect_type(ty: &TypeExpr, out: &mut Vec<Entry>) {
    match ty {
        TypeExpr::Named { name, args, span } => {
            out.push(Entry {
                span: *span,
                target: HoverTarget::TypeName(*name),
            });
            for a in args {
                collect_type(a, out);
            }
        }
        TypeExpr::Tuple { elements, .. } => {
            for e in elements {
                collect_type(e, out);
            }
        }
        TypeExpr::Function {
            params,
            ret,
            effects,
            ..
        } => {
            for p in params {
                collect_type(p, out);
            }
            collect_type(ret, out);
            for e in effects {
                collect_effect(e, out);
            }
        }
    }
}

fn collect_effect(eff: &EffectExpr, out: &mut Vec<Entry>) {
    match eff {
        EffectExpr::Named { name, span } => out.push(Entry {
            span: *span,
            target: HoverTarget::EffectName(*name),
        }),
        EffectExpr::RowVar { name, span } => out.push(Entry {
            span: *span,
            target: HoverTarget::EffectRowVar(*name),
        }),
        EffectExpr::Add { left, right, .. } | EffectExpr::Subtract { left, right, .. } => {
            collect_effect(left, out);
            collect_effect(right, out);
        }
    }
}
