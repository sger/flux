use crate::syntax::{
    Identifier,
    block::Block,
    expression::{Expression, MatchArm, Pattern, StringPart},
    program::Program,
    statement::Statement,
};

/// Read-only AST visitor.
///
/// Every `visit_*` method has a default that calls the corresponding `walk_*`
/// free function, which recurses into child nodes. Override a method to
/// intercept a node; call `walk_*` from within your override to continue
/// the traversal.
pub trait Visitor<'ast> {
    fn visit_program(&mut self, program: &'ast Program) {
        walk_program(self, program);
    }

    fn visit_block(&mut self, block: &'ast Block) {
        walk_block(self, block);
    }

    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'ast Expression) {
        walk_expr(self, expr);
    }

    fn visit_pat(&mut self, pat: &'ast Pattern) {
        walk_pat(self, pat);
    }

    fn visit_match_arm(&mut self, arm: &'ast MatchArm) {
        walk_match_arm(self, arm);
    }

    fn visit_string_part(&mut self, part: &'ast StringPart) {
        walk_string_part(self, part);
    }

    fn visit_identifier(&mut self, _ident: &'ast Identifier) {}
}

// ---------------------------------------------------------------------------
// walk_* free functions – exhaustive destructuring so that adding a new
// field or variant causes a compile error until this code is updated.
// ---------------------------------------------------------------------------

pub fn walk_program<'ast, V: Visitor<'ast> + ?Sized>(visitor: &mut V, program: &'ast Program) {
    let Program {
        statements,
        span: _,
    } = program;
    for stmt in statements {
        visitor.visit_stmt(stmt);
    }
}

pub fn walk_block<'ast, V: Visitor<'ast> + ?Sized>(visitor: &mut V, block: &'ast Block) {
    let Block {
        statements,
        span: _,
    } = block;
    for stmt in statements {
        visitor.visit_stmt(stmt);
    }
}

pub fn walk_stmt<'ast, V: Visitor<'ast> + ?Sized>(visitor: &mut V, stmt: &'ast Statement) {
    match stmt {
        Statement::Let {
            name,
            value,
            span: _,
        } => {
            visitor.visit_identifier(name);
            visitor.visit_expr(value);
        }
        Statement::LetDestructure {
            pattern,
            value,
            span: _,
        } => {
            visitor.visit_pat(pattern);
            visitor.visit_expr(value);
        }
        Statement::Return { value, span: _ } => {
            if let Some(expr) = value {
                visitor.visit_expr(expr);
            }
        }
        Statement::Expression {
            expression,
            span: _,
        } => {
            visitor.visit_expr(expression);
        }
        Statement::Function {
            name,
            parameters,
            body,
            span: _,
        } => {
            visitor.visit_identifier(name);
            for param in parameters {
                visitor.visit_identifier(param);
            }
            visitor.visit_block(body);
        }
        Statement::Assign {
            name,
            value,
            span: _,
        } => {
            visitor.visit_identifier(name);
            visitor.visit_expr(value);
        }
        Statement::Module {
            name,
            body,
            span: _,
        } => {
            visitor.visit_identifier(name);
            visitor.visit_block(body);
        }
        Statement::Import {
            name,
            alias,
            span: _,
        } => {
            visitor.visit_identifier(name);
            if let Some(alias_ident) = alias {
                visitor.visit_identifier(alias_ident);
            }
        }
    }
}

pub fn walk_expr<'ast, V: Visitor<'ast> + ?Sized>(visitor: &mut V, expr: &'ast Expression) {
    match expr {
        Expression::Identifier { name, span: _ } => {
            visitor.visit_identifier(name);
        }
        Expression::Integer { value: _, span: _ } => {}
        Expression::Float { value: _, span: _ } => {}
        Expression::String { value: _, span: _ } => {}
        Expression::InterpolatedString { parts, span: _ } => {
            for part in parts {
                visitor.visit_string_part(part);
            }
        }
        Expression::Boolean { value: _, span: _ } => {}
        Expression::Prefix {
            operator: _,
            right,
            span: _,
        } => {
            visitor.visit_expr(right);
        }
        Expression::Infix {
            left,
            operator: _,
            right,
            span: _,
        } => {
            visitor.visit_expr(left);
            visitor.visit_expr(right);
        }
        Expression::If {
            condition,
            consequence,
            alternative,
            span: _,
        } => {
            visitor.visit_expr(condition);
            visitor.visit_block(consequence);
            if let Some(alt) = alternative {
                visitor.visit_block(alt);
            }
        }
        Expression::Function {
            parameters,
            body,
            span: _,
        } => {
            for param in parameters {
                visitor.visit_identifier(param);
            }
            visitor.visit_block(body);
        }
        Expression::Call {
            function,
            arguments,
            span: _,
        } => {
            visitor.visit_expr(function);
            for arg in arguments {
                visitor.visit_expr(arg);
            }
        }
        Expression::ListLiteral { elements, span: _ }
        | Expression::ArrayLiteral { elements, span: _ }
        | Expression::TupleLiteral { elements, span: _ } => {
            for elem in elements {
                visitor.visit_expr(elem);
            }
        }
        Expression::EmptyList { span: _ } => {}
        Expression::Index {
            left,
            index,
            span: _,
        } => {
            visitor.visit_expr(left);
            visitor.visit_expr(index);
        }
        Expression::Hash { pairs, span: _ } => {
            for (key, value) in pairs {
                visitor.visit_expr(key);
                visitor.visit_expr(value);
            }
        }
        Expression::MemberAccess {
            object,
            member,
            span: _,
        } => {
            visitor.visit_expr(object);
            visitor.visit_identifier(member);
        }
        Expression::TupleFieldAccess {
            object,
            index: _,
            span: _,
        } => {
            visitor.visit_expr(object);
        }
        Expression::Match {
            scrutinee,
            arms,
            span: _,
        } => {
            visitor.visit_expr(scrutinee);
            for arm in arms {
                visitor.visit_match_arm(arm);
            }
        }
        Expression::None { span: _ } => {}
        Expression::Some { value, span: _ } => {
            visitor.visit_expr(value);
        }
        Expression::Left { value, span: _ } => {
            visitor.visit_expr(value);
        }
        Expression::Right { value, span: _ } => {
            visitor.visit_expr(value);
        }
        Expression::Cons { head, tail, .. } => {
            visitor.visit_expr(head);
            visitor.visit_expr(tail);
        }
    }
}

pub fn walk_pat<'ast, V: Visitor<'ast> + ?Sized>(visitor: &mut V, pat: &'ast Pattern) {
    match pat {
        Pattern::Wildcard { span: _ } => {}
        Pattern::Literal {
            expression,
            span: _,
        } => {
            visitor.visit_expr(expression);
        }
        Pattern::Identifier { name, span: _ } => {
            visitor.visit_identifier(name);
        }
        Pattern::None { span: _ } => {}
        Pattern::Some { pattern, span: _ } => {
            visitor.visit_pat(pattern);
        }
        Pattern::Left { pattern, span: _ } => {
            visitor.visit_pat(pattern);
        }
        Pattern::Right { pattern, span: _ } => {
            visitor.visit_pat(pattern);
        }
        Pattern::Cons { head, tail, .. } => {
            visitor.visit_pat(head);
            visitor.visit_pat(tail);
        }
        Pattern::EmptyList { .. } => {}
        Pattern::Tuple { elements, .. } => {
            for element in elements {
                visitor.visit_pat(element);
            }
        }
    }
}

pub fn walk_match_arm<'ast, V: Visitor<'ast> + ?Sized>(visitor: &mut V, arm: &'ast MatchArm) {
    let MatchArm {
        pattern,
        guard,
        body,
        span: _,
    } = arm;
    visitor.visit_pat(pattern);
    if let Some(guard_expr) = guard {
        visitor.visit_expr(guard_expr);
    }
    visitor.visit_expr(body);
}

pub fn walk_string_part<'ast, V: Visitor<'ast> + ?Sized>(visitor: &mut V, part: &'ast StringPart) {
    match part {
        StringPart::Literal(_) => {}
        StringPart::Interpolation(expr) => {
            visitor.visit_expr(expr);
        }
    }
}
