//! Unified offset → AST node lookup for hover, goto-definition, and
//! completion.
//!
//! Replaces the per-handler walkers that previously each missed different
//! node kinds. Every handler asks `find_at(program, position)` for the
//! deepest enclosing `NodeRef`, then matches on the variant to decide what
//! to render or where to jump.
//!
//! Variants come in three flavors:
//!
//! 1. **Whole AST nodes** (`Expr`, `Pattern`, `Statement`): the cursor is
//!    inside an existing AST node. Renderers reach into compiler state
//!    keyed off `ExprId` / `Span` / `Identifier`.
//! 2. **Sub-positions on AST nodes** (`MemberAccessMember`,
//!    `NamedFieldInitName`, `ImportName`, `DataFieldName`, ...): the
//!    cursor is on a position the AST encodes as a bare `Identifier`
//!    field rather than its own node. The locator emits a synthetic
//!    `NodeRef` whose span is computed from the parent span + the
//!    identifier's text length.
//! 3. **Effect & type sub-trees** (`EffectName`, `TypeExprNamed`, ...):
//!    the default `ast::visit::Visitor` doesn't descend into `TypeExpr`
//!    or `EffectExpr`, so the locator walks those manually.

use flux::diagnostics::position::{Position as FluxPosition, Span as FluxSpan};
use flux::syntax::Identifier;
use flux::syntax::block::Block;
use flux::syntax::data_variant::DataVariant;
use flux::syntax::effect_expr::EffectExpr;
use flux::syntax::effect_ops::EffectOp;
use flux::syntax::expression::{
    ExprId, Expression, HandleArm, MatchArm, NamedFieldInit, Pattern, StringPart,
};
use flux::syntax::interner::Interner;
use flux::syntax::program::Program;
use flux::syntax::statement::Statement;
use flux::syntax::type_class::{ClassMethod, InstanceMethod};
use flux::syntax::type_expr::TypeExpr;

/// The deepest AST position covered at an offset. Handlers match on this to
/// decide hover / goto-def / completion behavior.
#[derive(Debug, Clone)]
pub enum NodeRef<'a> {
    /// Cursor inside an expression. Hover renders `infer.expr_types[id]`.
    Expr(&'a Expression),
    /// Cursor inside a pattern. Hover/goto-def look at the pattern shape.
    Pattern(&'a Pattern),
    /// Cursor inside a statement that has no narrower enclosing node (rare
    /// — most statements contain expressions or patterns we'd match first).
    Statement(&'a Statement),
    /// Type-expression identifier in an annotation (`Int` in `let x: Int`).
    TypeExprNamed { name: Identifier, span: FluxSpan },
    /// Effect label in a `with` clause (`IO`, `Async`, ...).
    EffectName { name: Identifier, span: FluxSpan },
    /// Row variable (`|e`) in an effect row.
    EffectRowVar { name: Identifier, span: FluxSpan },
    /// `.member` of `obj.member`. `object` lets the renderer look up the
    /// object's inferred type to find the field's declared type.
    MemberAccessMember {
        object: &'a Expression,
        member: Identifier,
        span: FluxSpan,
    },
    /// Field name in `Person { name: ..., age: ... }`. `parent_constructor`
    /// is the constructor identifier whose `data` variant this field
    /// belongs to; `None` for spread expressions where the constructor
    /// must be inferred from the spread base's type.
    NamedFieldInitName {
        field_name: Identifier,
        parent_constructor: Option<Identifier>,
        span: FluxSpan,
    },
    /// Constructor name `Person` in `Person { ... }`.
    NamedConstructorName {
        name: Identifier,
        expr_id: ExprId,
        span: FluxSpan,
    },
    /// Operation name in `perform op(x)`.
    PerformOpName { name: Identifier, span: FluxSpan },
    /// Operation name in a `handle ... with { op(...) -> ... }` arm.
    HandleArmOpName { name: Identifier, span: FluxSpan },
    /// Imported module name (`Flow.Array` in `import Flow.Array as A`).
    ImportName {
        qualified: Identifier,
        alias: Option<Identifier>,
        span: FluxSpan,
    },
    /// Local alias of an `import ... as X` (`X`).
    ImportAlias {
        alias: Identifier,
        qualified: Identifier,
        span: FluxSpan,
    },
    /// `data` declaration name at the definition site.
    DataName { name: Identifier, span: FluxSpan },
    /// Variant name inside a `data` declaration.
    DataVariantName {
        name: Identifier,
        parent_data: Identifier,
        span: FluxSpan,
    },
    /// Named field inside a `data` variant declaration.
    DataFieldName {
        name: Identifier,
        ty_index: usize,
        parent_variant: Identifier,
        span: FluxSpan,
    },
    /// `effect` declaration name at the definition site.
    EffectDeclName { name: Identifier, span: FluxSpan },
    /// Operation declared inside an `effect` block.
    EffectOpName {
        name: Identifier,
        parent_effect: Identifier,
        span: FluxSpan,
    },
    /// `let`/`fn` declaration name at the definition site. Renderer keys
    /// `resolved_binding_schemes_by_span` by `binding_span` to fetch the
    /// scheme.
    DeclName {
        name: Identifier,
        binding_span: FluxSpan,
    },
    /// Function parameter at its declaration site (in the `fn f(x, y)`
    /// signature).
    FunctionParameter {
        name: Identifier,
        function_span: FluxSpan,
        span: FluxSpan,
    },
    /// Method declared inside a `class { … }` block. `parent_class` is the
    /// class it belongs to, used to render the declared signature.
    ClassMethodName {
        name: Identifier,
        parent_class: Identifier,
        span: FluxSpan,
    },
    /// Method declared inside an `instance { … }` block. `parent_class` is the
    /// class being implemented, used to render the class's declared signature.
    InstanceMethodName {
        name: Identifier,
        parent_class: Identifier,
        span: FluxSpan,
    },
}

impl NodeRef<'_> {
    /// The cursor-containing span the locator matched on. Used by handlers
    /// to compute the response `range`.
    pub fn span(&self) -> FluxSpan {
        match self {
            NodeRef::Expr(e) => e.span(),
            NodeRef::Pattern(p) => p.span(),
            NodeRef::Statement(s) => s.span(),
            NodeRef::TypeExprNamed { span, .. }
            | NodeRef::EffectName { span, .. }
            | NodeRef::EffectRowVar { span, .. }
            | NodeRef::MemberAccessMember { span, .. }
            | NodeRef::NamedFieldInitName { span, .. }
            | NodeRef::NamedConstructorName { span, .. }
            | NodeRef::PerformOpName { span, .. }
            | NodeRef::HandleArmOpName { span, .. }
            | NodeRef::ImportName { span, .. }
            | NodeRef::ImportAlias { span, .. }
            | NodeRef::DataName { span, .. }
            | NodeRef::DataVariantName { span, .. }
            | NodeRef::DataFieldName { span, .. }
            | NodeRef::EffectDeclName { span, .. }
            | NodeRef::EffectOpName { span, .. }
            | NodeRef::DeclName {
                binding_span: span, ..
            }
            | NodeRef::FunctionParameter { span, .. }
            | NodeRef::ClassMethodName { span, .. }
            | NodeRef::InstanceMethodName { span, .. } => *span,
        }
    }
}

/// Find the deepest `NodeRef` whose span contains `target`. Returns `None`
/// if no node covers the position (e.g. cursor is on a blank line).
pub fn find_at<'a>(
    program: &'a Program,
    interner: &Interner,
    target: FluxPosition,
) -> Option<NodeRef<'a>> {
    let mut finder = Finder {
        target,
        interner,
        best: None,
        best_extent: u64::MAX,
    };
    for stmt in &program.statements {
        finder.visit_stmt(stmt);
    }
    finder.best
}

/// Find the innermost `Expression::Call` whose argument list contains `target`.
///
/// Returns `(call_expr, active_param_index)` where `active_param_index` is the
/// zero-based index of the argument slot the cursor is in.
pub fn find_enclosing_call<'a>(
    program: &'a Program,
    target: FluxPosition,
) -> Option<(&'a Expression, usize)> {
    let mut best: Option<(&'a Expression, usize, u64)> = None;
    for stmt in &program.statements {
        enclosing_call_stmt(stmt, target, &mut best);
    }
    best.map(|(e, idx, _)| (e, idx))
}

fn enclosing_call_stmt<'a>(
    stmt: &'a Statement,
    target: FluxPosition,
    best: &mut Option<(&'a Expression, usize, u64)>,
) {
    match stmt {
        Statement::Let { value, .. }
        | Statement::Assign { value, .. }
        | Statement::LetDestructure { value, .. } => {
            enclosing_call_expr(value, target, best);
        }
        Statement::Return { value: Some(v), .. } => enclosing_call_expr(v, target, best),
        Statement::Return { value: None, .. } => {}
        Statement::Expression { expression, .. } => enclosing_call_expr(expression, target, best),
        Statement::Function { body, .. } | Statement::Module { body, .. } => {
            for s in &body.statements {
                enclosing_call_stmt(s, target, best);
            }
        }
        _ => {}
    }
}

fn enclosing_call_expr<'a>(
    expr: &'a Expression,
    target: FluxPosition,
    best: &mut Option<(&'a Expression, usize, u64)>,
) {
    if let Expression::Call {
        function,
        arguments,
        span,
        ..
    } = expr
    {
        // Cursor must be inside the call span but outside the function sub-expression.
        if position_in_span(target, *span) && !position_in_span(target, function.span()) {
            // Determine which argument slot the cursor is in.
            let param_idx = arguments
                .iter()
                .position(|arg| position_in_span(target, arg.span()))
                .unwrap_or_else(|| {
                    // Cursor is between or after args — count args ending before target.
                    arguments
                        .iter()
                        .filter(|arg| {
                            let s = arg.span();
                            (s.end.line, s.end.column) <= (target.line, target.column)
                        })
                        .count()
                });
            let extent = span_extent(*span);
            if best.as_ref().map(|(_, _, e)| extent <= *e).unwrap_or(true) {
                *best = Some((expr, param_idx, extent));
            }
        }
        // Always recurse into sub-expressions.
        enclosing_call_expr(function, target, best);
        for arg in arguments {
            enclosing_call_expr(arg, target, best);
        }
    } else {
        match expr {
            Expression::Infix { left, right, .. } => {
                enclosing_call_expr(left, target, best);
                enclosing_call_expr(right, target, best);
            }
            Expression::Prefix { right, .. } => enclosing_call_expr(right, target, best),
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                enclosing_call_expr(condition, target, best);
                for s in &consequence.statements {
                    enclosing_call_stmt(s, target, best);
                }
                if let Some(alt) = alternative {
                    for s in &alt.statements {
                        enclosing_call_stmt(s, target, best);
                    }
                }
            }
            Expression::Match {
                scrutinee, arms, ..
            } => {
                enclosing_call_expr(scrutinee, target, best);
                for arm in arms {
                    enclosing_call_expr(&arm.body, target, best);
                }
            }
            Expression::DoBlock { block, .. } => {
                for s in &block.statements {
                    enclosing_call_stmt(s, target, best);
                }
            }
            Expression::Function { body, .. } => {
                for s in &body.statements {
                    enclosing_call_stmt(s, target, best);
                }
            }
            Expression::Handle { expr, arms, .. } => {
                enclosing_call_expr(expr, target, best);
                for arm in arms {
                    enclosing_call_expr(&arm.body, target, best);
                }
            }
            _ => {}
        }
    }
}

struct Finder<'ast, 'i> {
    target: FluxPosition,
    interner: &'i Interner,
    best: Option<NodeRef<'ast>>,
    /// Cached "smaller is better" score for the current `best` candidate.
    /// We pick the node with the smallest containing span so inner matches
    /// always beat outer ones, regardless of visit order.
    best_extent: u64,
}

impl<'ast> Finder<'ast, '_> {
    fn record(&mut self, span: FluxSpan, node: NodeRef<'ast>) {
        if !position_in_span(self.target, span) {
            return;
        }
        let extent = span_extent(span);
        if extent <= self.best_extent {
            self.best_extent = extent;
            self.best = Some(node);
        }
    }

    fn record_name_at(
        &mut self,
        start: FluxPosition,
        name: Identifier,
        build: impl FnOnce(FluxSpan) -> NodeRef<'ast>,
    ) {
        if let Some(text) = self.interner.try_resolve(name) {
            let span = FluxSpan {
                start,
                end: FluxPosition {
                    line: start.line,
                    column: start.column + text.len(),
                },
            };
            self.record(span, build(span));
        }
    }

    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::Let {
                name,
                type_annotation,
                value,
                span,
                ..
            } => {
                self.record(
                    *span,
                    NodeRef::DeclName {
                        name: *name,
                        binding_span: *span,
                    },
                );
                if let Some(t) = type_annotation {
                    self.visit_type(t);
                }
                self.visit_expr(value);
            }
            Statement::LetDestructure { pattern, value, .. } => {
                self.visit_pattern(pattern);
                self.visit_expr(value);
            }
            Statement::Return { value, .. } => {
                if let Some(v) = value {
                    self.visit_expr(v);
                }
            }
            Statement::Expression { expression, .. } => self.visit_expr(expression),
            Statement::Function {
                name,
                parameters,
                parameter_types,
                return_type,
                effects,
                body,
                span,
                ..
            } => {
                self.record(
                    *span,
                    NodeRef::DeclName {
                        name: *name,
                        binding_span: *span,
                    },
                );
                // Parameter names live as bare `Identifier`s on the function
                // statement. The parser doesn't store per-parameter name
                // spans, so we approximate from the function span start —
                // good enough for goto-def to come back to the function
                // signature when hovering on a param use isn't yet wired.
                for param in parameters {
                    self.record_name_at(span.start, *param, |s| NodeRef::FunctionParameter {
                        name: *param,
                        function_span: *span,
                        span: s,
                    });
                }
                for pt in parameter_types.iter().flatten() {
                    self.visit_type(pt);
                }
                if let Some(rt) = return_type {
                    self.visit_type(rt);
                }
                for e in effects {
                    self.visit_effect(e);
                }
                self.visit_block(body);
            }
            Statement::Assign { value, .. } => self.visit_expr(value),
            Statement::Module { body, .. } => self.visit_block(body),
            Statement::Import {
                name,
                alias,
                span,
                exposing: _,
                except: _,
            } => {
                // `import <Name>` — name starts after the `import ` keyword
                // (7 chars). The parser doesn't record a separate name span.
                let name_start = FluxPosition {
                    line: span.start.line,
                    column: span.start.column + "import ".len(),
                };
                self.record_name_at(name_start, *name, |s| NodeRef::ImportName {
                    qualified: *name,
                    alias: *alias,
                    span: s,
                });
                if let Some(a) = alias {
                    // We don't know exactly where `as <alias>` sits inside
                    // the span without rewalking the source; emit a covering
                    // entry on the statement span so hover at least
                    // resolves to the alias variant. A precise span is a
                    // follow-up.
                    self.record(
                        *span,
                        NodeRef::ImportAlias {
                            alias: *a,
                            qualified: *name,
                            span: *span,
                        },
                    );
                }
            }
            Statement::Data {
                is_public,
                name,
                variants,
                span,
                ..
            } => {
                let name_start = decl_name_start(span.start, *is_public, "data");
                self.record_name_at(name_start, *name, |s| NodeRef::DataName {
                    name: *name,
                    span: s,
                });
                for variant in variants {
                    self.visit_data_variant(variant, *name);
                }
            }
            Statement::EffectDecl { name, ops, span } => {
                let name_start = decl_name_start(span.start, false, "effect");
                self.record_name_at(name_start, *name, |s| NodeRef::EffectDeclName {
                    name: *name,
                    span: s,
                });
                for op in ops {
                    self.visit_effect_op(op, *name);
                }
            }
            Statement::EffectAlias {
                name,
                expansion,
                span,
            } => {
                let name_start = decl_name_start(span.start, false, "alias");
                self.record_name_at(name_start, *name, |s| NodeRef::EffectDeclName {
                    name: *name,
                    span: s,
                });
                self.visit_effect(expansion);
            }
            Statement::TypeAlias(alias) => {
                let name_start = decl_name_start(alias.span.start, alias.is_public, "alias");
                self.record_name_at(name_start, alias.name, |s| NodeRef::DataName {
                    name: alias.name,
                    span: s,
                });
                self.visit_type(&alias.body);
            }
            Statement::Class {
                name,
                name_span,
                methods,
                ..
            } => {
                // The parser records the class name's own span — correct even
                // with a superclass constraint (`class Eq<a> => Ord<a>`), where
                // the name sits after `=>`, not right after the keyword.
                self.record_name_at(name_span.start, *name, |s| NodeRef::DataName {
                    name: *name,
                    span: s,
                });
                for m in methods {
                    self.visit_class_method(m, *name);
                }
            }
            Statement::Instance {
                class_name,
                methods,
                name_span,
                ..
            } => {
                // The parser records the head class name's own span — correct
                // even with a context constraint (`instance Eq<a> => Eq<List<a>>`),
                // where the head sits after `=>`, not right after the keyword.
                self.record_name_at(name_span.start, *class_name, |s| NodeRef::DataName {
                    name: *class_name,
                    span: s,
                });
                for m in methods {
                    self.visit_instance_method(m, *class_name);
                }
            }
        }
    }

    /// A `class` method: record its name, then descend into its signature
    /// types and any default body so hover works throughout the block.
    fn visit_class_method(&mut self, m: &'ast ClassMethod, parent_class: Identifier) {
        self.record_name_at(decl_name_start(m.span.start, false, "fn"), m.name, |s| {
            NodeRef::ClassMethodName {
                name: m.name,
                parent_class,
                span: s,
            }
        });
        for pt in &m.param_types {
            self.visit_type(pt);
        }
        self.visit_type(&m.return_type);
        for e in &m.effects {
            self.visit_effect(e);
        }
        if let Some(body) = &m.default_body {
            self.visit_block(body);
        }
    }

    /// An `instance` method: record its name, then descend into its effects
    /// and body. Instance methods carry no type annotations, so hover renders
    /// the class's declared signature instead.
    fn visit_instance_method(&mut self, m: &'ast InstanceMethod, parent_class: Identifier) {
        self.record_name_at(decl_name_start(m.span.start, false, "fn"), m.name, |s| {
            NodeRef::InstanceMethodName {
                name: m.name,
                parent_class,
                span: s,
            }
        });
        for e in &m.effects {
            self.visit_effect(e);
        }
        self.visit_block(&m.body);
    }

    fn visit_data_variant(&mut self, variant: &'ast DataVariant, parent_data: Identifier) {
        self.record_name_at(variant.span.start, variant.name, |s| {
            NodeRef::DataVariantName {
                name: variant.name,
                parent_data,
                span: s,
            }
        });
        // Field names (when present) sit at the start of each field's
        // type span — the parser doesn't record per-field-name spans,
        // so we approximate using the type's start position.
        if let Some(names) = &variant.field_names {
            for (i, (field_name, ty)) in names.iter().zip(variant.fields.iter()).enumerate() {
                let ty_span = type_span(ty);
                // Field name appears just before the type; we synthesize
                // a name span ending at the type's start. Best-effort —
                // hovers and goto-def land on the field name area.
                self.record_name_at(
                    FluxPosition {
                        line: ty_span.start.line,
                        column: ty_span
                            .start
                            .column
                            .saturating_sub(field_name_text_len(self.interner, *field_name) + 2),
                    },
                    *field_name,
                    |s| NodeRef::DataFieldName {
                        name: *field_name,
                        ty_index: i,
                        parent_variant: variant.name,
                        span: s,
                    },
                );
                self.visit_type(ty);
            }
        } else {
            for ty in &variant.fields {
                self.visit_type(ty);
            }
        }
    }

    fn visit_effect_op(&mut self, op: &'ast EffectOp, parent_effect: Identifier) {
        self.record_name_at(op.span.start, op.name, |s| NodeRef::EffectOpName {
            name: op.name,
            parent_effect,
            span: s,
        });
        self.visit_type(&op.type_expr);
    }

    fn visit_block(&mut self, block: &'ast Block) {
        for stmt in &block.statements {
            self.visit_stmt(stmt);
        }
    }

    fn visit_expr(&mut self, expr: &'ast Expression) {
        // Always record the expression itself; deeper matches will replace
        // it via the smaller-extent comparison.
        self.record(expr.span(), NodeRef::Expr(expr));

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
                        self.visit_expr(e);
                    }
                }
            }
            Expression::Prefix { right, .. } => self.visit_expr(right),
            Expression::Infix { left, right, .. } => {
                self.visit_expr(left);
                self.visit_expr(right);
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                self.visit_expr(condition);
                self.visit_block(consequence);
                if let Some(b) = alternative {
                    self.visit_block(b);
                }
            }
            Expression::DoBlock { block, .. } => self.visit_block(block),
            Expression::Function {
                parameter_types,
                return_type,
                effects,
                body,
                ..
            } => {
                for pt in parameter_types.iter().flatten() {
                    self.visit_type(pt);
                }
                if let Some(rt) = return_type {
                    self.visit_type(rt);
                }
                for e in effects {
                    self.visit_effect(e);
                }
                self.visit_block(body);
            }
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                self.visit_expr(function);
                for a in arguments {
                    self.visit_expr(a);
                }
            }
            Expression::ListLiteral { elements, .. }
            | Expression::ArrayLiteral { elements, .. }
            | Expression::TupleLiteral { elements, .. } => {
                for e in elements {
                    self.visit_expr(e);
                }
            }
            Expression::Index { left, index, .. } => {
                self.visit_expr(left);
                self.visit_expr(index);
            }
            Expression::Hash { pairs, .. } => {
                for (k, v) in pairs {
                    self.visit_expr(k);
                    self.visit_expr(v);
                }
            }
            Expression::MemberAccess {
                object,
                member,
                span,
                ..
            } => {
                // The member name sits at the end of the access span. We
                // approximate its span as the trailing `name.len()` chars
                // of the access span (since `.member` is always at the
                // tail of `expr.member`).
                if let Some(text) = self.interner.try_resolve(*member) {
                    let member_start = FluxPosition {
                        line: span.end.line,
                        column: span.end.column.saturating_sub(text.len()),
                    };
                    let member_span = FluxSpan {
                        start: member_start,
                        end: span.end,
                    };
                    self.record(
                        member_span,
                        NodeRef::MemberAccessMember {
                            object,
                            member: *member,
                            span: member_span,
                        },
                    );
                }
                self.visit_expr(object);
            }
            Expression::TupleFieldAccess { object, .. } => self.visit_expr(object),
            Expression::Match {
                scrutinee, arms, ..
            } => {
                self.visit_expr(scrutinee);
                for arm in arms {
                    self.visit_match_arm(arm);
                }
            }
            Expression::Some { value, .. }
            | Expression::Left { value, .. }
            | Expression::Right { value, .. } => self.visit_expr(value),
            Expression::Cons { head, tail, .. } => {
                self.visit_expr(head);
                self.visit_expr(tail);
            }
            Expression::Perform {
                operation,
                args,
                span,
                ..
            } => {
                self.record_name_at(span.start, *operation, |s| NodeRef::PerformOpName {
                    name: *operation,
                    span: s,
                });
                for a in args {
                    self.visit_expr(a);
                }
            }
            Expression::Handle {
                expr,
                parameter,
                arms,
                ..
            } => {
                self.visit_expr(expr);
                if let Some(p) = parameter {
                    self.visit_expr(p);
                }
                for arm in arms {
                    self.visit_handle_arm(arm);
                }
            }
            Expression::Sealing { expr, allowed, .. } => {
                self.visit_expr(expr);
                for e in allowed {
                    self.visit_effect(e);
                }
            }
            Expression::NamedConstructor {
                name,
                fields,
                span,
                id,
            } => {
                self.record_name_at(span.start, *name, |s| NodeRef::NamedConstructorName {
                    name: *name,
                    expr_id: *id,
                    span: s,
                });
                for field in fields {
                    self.visit_named_field_init(field, Some(*name));
                }
            }
            Expression::Spread {
                base, overrides, ..
            } => {
                self.visit_expr(base);
                for field in overrides {
                    self.visit_named_field_init(field, None);
                }
            }
        }
    }

    fn visit_named_field_init(
        &mut self,
        field: &'ast NamedFieldInit,
        parent_constructor: Option<Identifier>,
    ) {
        self.record_name_at(field.span.start, field.name, |s| {
            NodeRef::NamedFieldInitName {
                field_name: field.name,
                parent_constructor,
                span: s,
            }
        });
        if let Some(v) = &field.value {
            self.visit_expr(v);
        }
    }

    fn visit_match_arm(&mut self, arm: &'ast MatchArm) {
        self.visit_pattern(&arm.pattern);
        if let Some(g) = &arm.guard {
            self.visit_expr(g);
        }
        self.visit_expr(&arm.body);
    }

    fn visit_handle_arm(&mut self, arm: &'ast HandleArm) {
        // `operation_name` on HandleArm has no separate span; emit a
        // best-effort entry covering the arm body's start.
        self.record_name_at(arm.body.span().start, arm.operation_name, |s| {
            NodeRef::HandleArmOpName {
                name: arm.operation_name,
                span: s,
            }
        });
        self.visit_expr(&arm.body);
    }

    fn visit_pattern(&mut self, pat: &'ast Pattern) {
        self.record(pat.span(), NodeRef::Pattern(pat));
        match pat {
            Pattern::Tuple { elements, .. } => {
                for e in elements {
                    self.visit_pattern(e);
                }
            }
            Pattern::Constructor { fields, .. } => {
                for f in fields {
                    self.visit_pattern(f);
                }
            }
            Pattern::NamedConstructor { fields, .. } => {
                for f in fields {
                    if let Some(inner) = &f.pattern {
                        self.visit_pattern(inner);
                    }
                }
            }
            Pattern::Cons { head, tail, .. } => {
                self.visit_pattern(head);
                self.visit_pattern(tail);
            }
            Pattern::Some { pattern, .. }
            | Pattern::Left { pattern, .. }
            | Pattern::Right { pattern, .. } => self.visit_pattern(pattern),
            _ => {}
        }
    }

    fn visit_type(&mut self, ty: &'ast TypeExpr) {
        match ty {
            TypeExpr::Named { name, args, span } => {
                self.record(
                    *span,
                    NodeRef::TypeExprNamed {
                        name: *name,
                        span: *span,
                    },
                );
                for a in args {
                    self.visit_type(a);
                }
            }
            TypeExpr::Tuple { elements, .. } => {
                for e in elements {
                    self.visit_type(e);
                }
            }
            TypeExpr::Function {
                params,
                ret,
                effects,
                ..
            } => {
                for p in params {
                    self.visit_type(p);
                }
                self.visit_type(ret);
                for e in effects {
                    self.visit_effect(e);
                }
            }
        }
    }

    fn visit_effect(&mut self, eff: &'ast EffectExpr) {
        match eff {
            EffectExpr::Named { name, span } => self.record(
                *span,
                NodeRef::EffectName {
                    name: *name,
                    span: *span,
                },
            ),
            EffectExpr::RowVar { name, span } => self.record(
                *span,
                NodeRef::EffectRowVar {
                    name: *name,
                    span: *span,
                },
            ),
            EffectExpr::Add { left, right, .. } | EffectExpr::Subtract { left, right, .. } => {
                self.visit_effect(left);
                self.visit_effect(right);
            }
        }
    }
}

/// Synthesize the declaration-name start position for `data`, `class`, etc.
/// statements: skip the keyword and the single space before the name.
///
/// The parser consumes a leading `public` modifier *before* it records the
/// statement's span (see `parse_statement`'s `public` dispatch arms, which
/// `next_token()` past `public` first), so `stmt_start` already points at the
/// keyword — there is no `public ` prefix to skip. `_is_public` is kept only so
/// call sites can pass the declaration's visibility flag without each having to
/// know this; it does not affect the column.
pub(crate) fn decl_name_start(
    stmt_start: FluxPosition,
    _is_public: bool,
    keyword: &str,
) -> FluxPosition {
    FluxPosition {
        line: stmt_start.line,
        column: stmt_start.column + keyword.len() + 1,
    }
}

pub(crate) fn position_in_span(target: FluxPosition, span: FluxSpan) -> bool {
    let after_start = (span.start.line, span.start.column) <= (target.line, target.column);
    let before_end = (target.line, target.column) <= (span.end.line, span.end.column);
    after_start && before_end
}

/// Smaller is "more inner". We encode line × very-large + column so deeper
/// matches across different lines still compare correctly.
fn span_extent(span: FluxSpan) -> u64 {
    let line_delta = span.end.line.saturating_sub(span.start.line) as u64;
    let col_delta = if span.end.line == span.start.line {
        span.end.column.saturating_sub(span.start.column) as u64
    } else {
        // Multi-line spans always lose to single-line ones at the same
        // line-delta count; this is a stable tiebreaker.
        u64::from(u32::MAX)
    };
    line_delta
        .saturating_mul(1_000_000)
        .saturating_add(col_delta)
}

fn type_span(ty: &TypeExpr) -> FluxSpan {
    match ty {
        TypeExpr::Named { span, .. } => *span,
        TypeExpr::Tuple { span, .. } => *span,
        TypeExpr::Function { span, .. } => *span,
    }
}

fn field_name_text_len(interner: &Interner, name: Identifier) -> usize {
    interner.try_resolve(name).map(|s| s.len()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux::syntax::lexer::Lexer;
    use flux::syntax::parser::Parser;

    fn parse(src: &str) -> (Program, Interner) {
        let lexer = Lexer::new(src.to_string());
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        (program, parser.take_interner())
    }

    fn pos(line: usize, column: usize) -> FluxPosition {
        FluxPosition { line, column }
    }

    #[test]
    fn finds_decl_name_on_let_binding() {
        let (program, interner) = parse("let xyz = 42\n");
        let node = find_at(&program, &interner, pos(1, 5)).expect("decl name");
        match node {
            NodeRef::DeclName { .. } | NodeRef::Expr(_) => {}
            other => panic!("expected DeclName or Expr, got {other:?}"),
        }
    }

    #[test]
    fn finds_integer_literal_as_expr() {
        let (program, interner) = parse("let x = 42\n");
        let node = find_at(&program, &interner, pos(1, 10)).expect("integer expr");
        match node {
            NodeRef::Expr(Expression::Integer { .. }) => {}
            other => panic!("expected Integer expr, got {other:?}"),
        }
    }

    #[test]
    fn finds_member_access_member() {
        let (program, interner) = parse("let v = obj.name\n");
        // `obj.name` — cursor on `name` (column ~14).
        let node = find_at(&program, &interner, pos(1, 14)).expect("member");
        match node {
            NodeRef::MemberAccessMember { .. } => {}
            other => panic!("expected MemberAccessMember, got {other:?}"),
        }
    }

    #[test]
    fn finds_import_name() {
        let (program, interner) = parse("import Flow.Async exposing (..)\n");
        // Cursor on `Async` (column ~13).
        let node = find_at(&program, &interner, pos(1, 13)).expect("import");
        match node {
            NodeRef::ImportName { .. } => {}
            other => panic!("expected ImportName, got {other:?}"),
        }
    }

    #[test]
    fn finds_named_constructor_name() {
        let (program, interner) = parse(
            "data Person { Person { name: String, age: Int } }\nlet p = Person { name: \"A\", age: 1 }\n",
        );
        // `Person` on line 2 starts at column 9.
        let node = find_at(&program, &interner, pos(2, 10)).expect("constructor");
        match node {
            NodeRef::NamedConstructorName { .. } => {}
            other => panic!("expected NamedConstructorName, got {other:?}"),
        }
    }

    #[test]
    fn finds_data_name() {
        let (program, interner) = parse("data Person { Person { name: String, age: Int } }\n");
        // `Person` starts at column 6.
        let node = find_at(&program, &interner, pos(1, 7)).expect("data name");
        match node {
            NodeRef::DataName { .. } => {}
            other => panic!("expected DataName, got {other:?}"),
        }
    }

    #[test]
    fn returns_none_for_blank_position() {
        let (program, interner) = parse("let x = 1\n");
        // Way past end of source.
        assert!(find_at(&program, &interner, pos(99, 99)).is_none());
    }

    #[test]
    fn instance_head_name_resolves_after_fat_arrow() {
        let (program, interner) = parse(
            "class Eq<a> { fn eq(x: a, y: a) -> Bool }\n\
             instance Eq<a> => Eq<List<a>> { fn eq(x, y) { true } }\n",
        );
        // The head class name `Eq` sits after `=>` (column 18 on line 2), not at
        // the context constraint right after `instance ` (column 9).
        let node = find_at(&program, &interner, pos(2, 18)).expect("instance head name");
        match node {
            NodeRef::DataName { name, .. } => {
                assert_eq!(interner.try_resolve(name), Some("Eq"));
            }
            other => panic!("expected DataName for the instance head, got {other:?}"),
        }
        // The old (buggy) position — the context constraint — no longer carries
        // the instance head node.
        assert!(
            !matches!(
                find_at(&program, &interner, pos(2, 9)),
                Some(NodeRef::DataName { .. })
            ),
            "the context constraint position must not resolve to the head name"
        );
    }
}
