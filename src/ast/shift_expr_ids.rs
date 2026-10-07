//! Move every expression id in a program by a fixed offset.
//!
//! Each module is parsed on its own and numbers its expressions from one, so
//! two modules' ids overlap. That is harmless while each is compiled alone.
//! It is not when they are concatenated into one program and inferred as a
//! whole: the type table is keyed by `ExprId`, and two expressions sharing an
//! id get one type. Shifting each module past the ones before it keeps every
//! id in the merged program distinct, and keeps a module's own relative order.

use crate::{
    ast::fold::{self, Folder},
    syntax::{
        expression::{ExprId, Expression},
        program::Program,
        statement::Statement,
        type_class::{ClassMethod, InstanceMethod},
    },
};

/// Add `offset` to every expression id in `program`.
///
/// [`ExprId::UNSET`] is left alone: it marks a node made outside the parser,
/// and is a sentinel rather than a position in the numbering. Class default
/// bodies and instance method bodies are shifted too; the generic folder
/// passes them through untouched.
pub fn shift_expr_ids(program: Program, offset: u32) -> Program {
    if offset == 0 {
        return program;
    }
    ShiftExprIds { offset }.fold_program(program)
}

struct ShiftExprIds {
    offset: u32,
}

impl Folder for ShiftExprIds {
    fn fold_expr(&mut self, expr: Expression) -> Expression {
        let mut expr = fold::fold_expr(self, expr);
        let id = expr.expr_id_mut();
        if *id != ExprId::UNSET {
            *id = ExprId(id.0 + self.offset);
        }
        expr
    }

    fn fold_stmt(&mut self, stmt: Statement) -> Statement {
        match fold::fold_stmt(self, stmt) {
            Statement::Class {
                is_public,
                name,
                type_params,
                superclasses,
                methods,
                associated_types,
                span,
                name_span,
            } => Statement::Class {
                is_public,
                name,
                type_params,
                superclasses,
                methods: methods
                    .into_iter()
                    .map(|method| ClassMethod {
                        default_body: method.default_body.map(|body| self.fold_block(body)),
                        ..method
                    })
                    .collect(),
                associated_types,
                span,
                name_span,
            },
            Statement::Instance {
                is_public,
                class_name,
                type_args,
                context,
                methods,
                associated_types,
                span,
                name_span,
            } => Statement::Instance {
                is_public,
                class_name,
                type_args,
                context,
                methods: methods
                    .into_iter()
                    .map(|method| InstanceMethod {
                        body: self.fold_block(method.body),
                        ..method
                    })
                    .collect(),
                associated_types,
                span,
                name_span,
            },
            other => other,
        }
    }
}
