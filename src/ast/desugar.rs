use crate::ast::fold::{self, Folder};
use crate::ast::visit::{self, Visitor};
use crate::syntax::{expression::Expression, interner::Interner, program::Program};
use crate::types::{infer_type::InferType, type_constructor::TypeConstructor};
use std::borrow::Cow;
use std::collections::HashMap;

/// AST desugaring pass.
///
/// Rewrites syntactic sugar into simpler core constructs. Current rules:
///
/// 1. **Double negation elimination:** `!!x` → `x`
/// 2. **Negated comparison:** `!(a == b)` → `a != b`, `!(a != b)` → `a == b`
///
/// This module is designed to be extended as new syntax sugar is added
/// (e.g., cons syntax from proposal 017, async/await from proposal 026).
struct DesugarPass;

impl Folder for DesugarPass {
    fn fold_expr(&mut self, expr: Expression) -> Expression {
        // Fold children first (bottom-up)
        let expr = fold::fold_expr(self, expr);

        // Only act on `!inner` expressions.
        let is_not = matches!(&expr, Expression::Prefix { operator, .. } if operator == "!");
        if !is_not {
            return expr;
        }

        // Destructure owned expression to avoid cloning sub-trees.
        let Expression::Prefix {
            right, span, id, ..
        } = expr
        else {
            unreachable!()
        };

        match *right {
            // Rule 1: !!x → x
            Expression::Prefix {
                ref operator,
                right: inner,
                ..
            } if operator == "!" => *inner,

            // Rule 2: !(a == b) → a != b, !(a != b) → a == b
            Expression::Infix {
                left,
                ref operator,
                right,
                span: infix_span,
                id: infix_id,
            } if operator == "==" || operator == "!=" => {
                let flipped = if operator == "==" { "!=" } else { "==" };
                Expression::Infix {
                    left,
                    operator: flipped.to_string(),
                    right,
                    span: infix_span,
                    id: infix_id,
                }
            }

            // No simplification; reconstruct the `!inner` expression.
            inner => Expression::Prefix {
                operator: "!".to_string(),
                right: Box::new(inner),
                span,
                id,
            },
        }
    }
}

/// Apply desugaring rules to a program.
pub fn desugar(program: Program) -> Program {
    let mut pass = DesugarPass;
    pass.fold_program(program)
}

struct OperatorDesugarPass<'a> {
    hm_expr_types: &'a HashMap<crate::syntax::expression::ExprId, InferType>,
    interner: &'a mut Interner,
    in_generated_instance_method: bool,
    in_explicit_constraint_context: bool,
}

impl OperatorDesugarPass<'_> {
    fn operand_type(&self, expr: &Expression) -> Option<&InferType> {
        self.hm_expr_types.get(&expr.expr_id())
    }

    fn is_dynamic_operand(&self, expr: &Expression) -> bool {
        matches!(
            self.operand_type(expr),
            None | Some(InferType::Con(TypeConstructor::Any))
        )
    }

    fn is_type_var_operand(&self, expr: &Expression) -> bool {
        matches!(self.operand_type(expr), Some(InferType::Var(_)))
    }

    fn is_concrete_non_any_operand(&self, expr: &Expression) -> bool {
        self.operand_type(expr)
            .is_some_and(|ty| ty.is_concrete() && !ty.contains_any())
    }

    fn concrete_numeric_operands(
        &self,
        left: &Expression,
        right: &Expression,
    ) -> (bool, bool, bool) {
        let left_ty = self.operand_type(left);
        let right_ty = self.operand_type(right);
        let ints = matches!(left_ty, Some(InferType::Con(TypeConstructor::Int)))
            && matches!(right_ty, Some(InferType::Con(TypeConstructor::Int)));
        let floats = matches!(left_ty, Some(InferType::Con(TypeConstructor::Float)))
            && matches!(right_ty, Some(InferType::Con(TypeConstructor::Float)));
        let numeric = matches!(
            left_ty,
            Some(InferType::Con(
                TypeConstructor::Int | TypeConstructor::Float
            ))
        ) && matches!(
            right_ty,
            Some(InferType::Con(
                TypeConstructor::Int | TypeConstructor::Float
            ))
        );
        (ints, floats, numeric)
    }

    fn concrete_string_operands(&self, left: &Expression, right: &Expression) -> bool {
        matches!(
            self.operand_type(left),
            Some(InferType::Con(TypeConstructor::String))
        ) && matches!(
            self.operand_type(right),
            Some(InferType::Con(TypeConstructor::String))
        )
    }

    fn operator_method_name(&self, operator: &str) -> Option<&'static str> {
        match operator {
            "+" => Some("add"),
            "-" => Some("sub"),
            "*" => Some("mul"),
            "/" => Some("div"),
            "==" => Some("eq"),
            "!=" => Some("neq"),
            "<" => Some("lt"),
            "<=" => Some("lte"),
            ">" => Some("gt"),
            ">=" => Some("gte"),
            "++" => Some("append"),
            _ => None,
        }
    }

    // Decision order matters here:
    // 1. Never rewrite non-overloadable operators.
    // 2. Preserve infix when either operand is dynamic (`Any` / missing HM type),
    //    because downstream runtime semantics still own those cases.
    // 3. Outside an explicit class-constraint context, keep arithmetic,
    //    comparisons, and type-variable operands infix so unconstrained code
    //    does not pick up synthetic class-method calls.
    // 4. Preserve clearly concrete non-`Any` pairs, then use the narrower
    //    Int/Float/String fast-path checks below to keep primitive lowering for
    //    the operator/type combinations we specialize.
    // 5. Everything else rewrites to the corresponding class method.
    fn should_keep_infix(&self, left: &Expression, operator: &str, right: &Expression) -> bool {
        if matches!(operator, "&&" | "||" | "|>" | "%") {
            return true;
        }
        if self.is_dynamic_operand(left) || self.is_dynamic_operand(right) {
            return true;
        }
        if !self.in_explicit_constraint_context && matches!(operator, "+" | "-" | "*" | "/") {
            return true;
        }
        if !self.in_explicit_constraint_context
            && matches!(operator, "==" | "!=" | "<" | "<=" | ">" | ">=")
        {
            return true;
        }
        if operator != "++"
            && self.is_concrete_non_any_operand(left)
            && self.is_concrete_non_any_operand(right)
        {
            return true;
        }
        if !self.in_explicit_constraint_context
            && (self.is_type_var_operand(left) || self.is_type_var_operand(right))
        {
            return true;
        }
        let (_, _, numeric) = self.concrete_numeric_operands(left, right);
        let strings = self.concrete_string_operands(left, right);
        match operator {
            "+" => numeric || strings,
            "-" | "*" | "/" => numeric,
            "==" | "!=" | "<" | "<=" | ">" | ">=" => numeric,
            _ => false,
        }
    }
}

impl Folder for OperatorDesugarPass<'_> {
    fn fold_stmt(
        &mut self,
        stmt: crate::syntax::statement::Statement,
    ) -> crate::syntax::statement::Statement {
        match stmt {
            crate::syntax::statement::Statement::Function {
                is_public,
                name,
                type_params,
                parameters,
                parameter_types,
                return_type,
                effects,
                body,
                span,
                fip,
            } => {
                let prev_generated = self.in_generated_instance_method;
                let prev_constraint_context = self.in_explicit_constraint_context;
                self.in_generated_instance_method =
                    self.interner.resolve(name).starts_with("__tc_");
                self.in_explicit_constraint_context = type_params
                    .iter()
                    .any(|param| !param.constraints.is_empty());
                let body = self.fold_block(body);
                self.in_generated_instance_method = prev_generated;
                self.in_explicit_constraint_context = prev_constraint_context;
                crate::syntax::statement::Statement::Function {
                    is_public,
                    name,
                    type_params,
                    parameters,
                    parameter_types,
                    return_type,
                    effects,
                    body,
                    span,
                    fip,
                }
            }
            other => fold::fold_stmt(self, other),
        }
    }

    fn fold_expr(&mut self, expr: Expression) -> Expression {
        let expr = fold::fold_expr(self, expr);
        if self.in_generated_instance_method {
            return expr;
        }
        let Expression::Infix {
            left,
            operator,
            right,
            span,
            id,
        } = expr
        else {
            return expr;
        };

        if self.should_keep_infix(&left, &operator, &right) {
            return Expression::Infix {
                left,
                operator,
                right,
                span,
                id,
            };
        }

        let Some(method_name) = self.operator_method_name(&operator) else {
            return Expression::Infix {
                left,
                operator,
                right,
                span,
                id,
            };
        };

        Expression::Call {
            function: Box::new(Expression::Identifier {
                name: self.interner.intern(method_name),
                span,
                id: crate::syntax::expression::ExprId::UNSET,
            }),
            arguments: vec![*left, *right],
            span,
            id,
        }
    }
}

struct OperatorDesugarDetector<'a> {
    hm_expr_types: &'a HashMap<crate::syntax::expression::ExprId, InferType>,
    interner: &'a Interner,
    in_generated_instance_method: bool,
    in_explicit_constraint_context: bool,
    found_rewrite: bool,
}

impl OperatorDesugarDetector<'_> {
    fn operand_type(&self, expr: &Expression) -> Option<&InferType> {
        self.hm_expr_types.get(&expr.expr_id())
    }

    fn is_dynamic_operand(&self, expr: &Expression) -> bool {
        matches!(
            self.operand_type(expr),
            None | Some(InferType::Con(TypeConstructor::Any))
        )
    }

    fn is_type_var_operand(&self, expr: &Expression) -> bool {
        matches!(self.operand_type(expr), Some(InferType::Var(_)))
    }

    fn is_concrete_non_any_operand(&self, expr: &Expression) -> bool {
        self.operand_type(expr)
            .is_some_and(|ty| ty.is_concrete() && !ty.contains_any())
    }

    fn concrete_numeric_operands(
        &self,
        left: &Expression,
        right: &Expression,
    ) -> (bool, bool, bool) {
        let left_ty = self.operand_type(left);
        let right_ty = self.operand_type(right);
        let ints = matches!(left_ty, Some(InferType::Con(TypeConstructor::Int)))
            && matches!(right_ty, Some(InferType::Con(TypeConstructor::Int)));
        let floats = matches!(left_ty, Some(InferType::Con(TypeConstructor::Float)))
            && matches!(right_ty, Some(InferType::Con(TypeConstructor::Float)));
        let numeric = matches!(
            left_ty,
            Some(InferType::Con(
                TypeConstructor::Int | TypeConstructor::Float
            ))
        ) && matches!(
            right_ty,
            Some(InferType::Con(
                TypeConstructor::Int | TypeConstructor::Float
            ))
        );
        (ints, floats, numeric)
    }

    fn concrete_string_operands(&self, left: &Expression, right: &Expression) -> bool {
        matches!(
            self.operand_type(left),
            Some(InferType::Con(TypeConstructor::String))
        ) && matches!(
            self.operand_type(right),
            Some(InferType::Con(TypeConstructor::String))
        )
    }

    fn operator_method_name(&self, operator: &str) -> Option<&'static str> {
        match operator {
            "+" => Some("add"),
            "-" => Some("sub"),
            "*" => Some("mul"),
            "/" => Some("div"),
            "==" => Some("eq"),
            "!=" => Some("neq"),
            "<" => Some("lt"),
            "<=" => Some("lte"),
            ">" => Some("gt"),
            ">=" => Some("gte"),
            "++" => Some("append"),
            _ => None,
        }
    }

    fn should_keep_infix(&self, left: &Expression, operator: &str, right: &Expression) -> bool {
        if matches!(operator, "&&" | "||" | "|>" | "%") {
            return true;
        }
        if self.is_dynamic_operand(left) || self.is_dynamic_operand(right) {
            return true;
        }
        if !self.in_explicit_constraint_context && matches!(operator, "+" | "-" | "*" | "/") {
            return true;
        }
        if !self.in_explicit_constraint_context
            && matches!(operator, "==" | "!=" | "<" | "<=" | ">" | ">=")
        {
            return true;
        }
        if operator != "++"
            && self.is_concrete_non_any_operand(left)
            && self.is_concrete_non_any_operand(right)
        {
            return true;
        }
        if !self.in_explicit_constraint_context
            && (self.is_type_var_operand(left) || self.is_type_var_operand(right))
        {
            return true;
        }
        let (_, _, numeric) = self.concrete_numeric_operands(left, right);
        let strings = self.concrete_string_operands(left, right);
        match operator {
            "+" => numeric || strings,
            "-" | "*" | "/" => numeric,
            "==" | "!=" | "<" | "<=" | ">" | ">=" => numeric,
            _ => false,
        }
    }
}

impl<'ast> Visitor<'ast> for OperatorDesugarDetector<'_> {
    fn visit_stmt(&mut self, stmt: &'ast crate::syntax::statement::Statement) {
        if self.found_rewrite {
            return;
        }
        match stmt {
            crate::syntax::statement::Statement::Function {
                name,
                type_params,
                body,
                ..
            } => {
                let prev_generated = self.in_generated_instance_method;
                let prev_constraint_context = self.in_explicit_constraint_context;
                self.in_generated_instance_method =
                    self.interner.resolve(*name).starts_with("__tc_");
                self.in_explicit_constraint_context = type_params
                    .iter()
                    .any(|param| !param.constraints.is_empty());
                self.visit_block(body);
                self.in_generated_instance_method = prev_generated;
                self.in_explicit_constraint_context = prev_constraint_context;
            }
            _ => visit::walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &'ast Expression) {
        if self.found_rewrite {
            return;
        }
        if self.in_generated_instance_method {
            visit::walk_expr(self, expr);
            return;
        }
        if let Expression::Infix {
            left,
            operator,
            right,
            ..
        } = expr
            && self.operator_method_name(operator).is_some()
            && !self.should_keep_infix(left, operator, right)
        {
            self.found_rewrite = true;
            return;
        }

        visit::walk_expr(self, expr);
    }
}

/// Desugar overloadable operators to ordinary function calls using HM types to
/// preserve concrete Int/Float fast paths.
pub fn desugar_operators(
    program: Program,
    hm_expr_types: &HashMap<crate::syntax::expression::ExprId, InferType>,
    interner: &mut Interner,
) -> Program {
    let mut pass = OperatorDesugarPass {
        hm_expr_types,
        interner,
        in_generated_instance_method: false,
        in_explicit_constraint_context: false,
    };
    pass.fold_program(program)
}

pub fn operator_desugaring_needed(
    program: &Program,
    hm_expr_types: &HashMap<crate::syntax::expression::ExprId, InferType>,
    interner: &Interner,
) -> bool {
    let mut detector = OperatorDesugarDetector {
        hm_expr_types,
        interner,
        in_generated_instance_method: false,
        in_explicit_constraint_context: false,
        found_rewrite: false,
    };
    detector.visit_program(program);
    detector.found_rewrite
}

pub fn desugar_operators_if_needed<'a>(
    program: Cow<'a, Program>,
    hm_expr_types: &HashMap<crate::syntax::expression::ExprId, InferType>,
    interner: &mut Interner,
) -> Cow<'a, Program> {
    let needs_desugar = operator_desugaring_needed(program.as_ref(), hm_expr_types, interner);
    if needs_desugar {
        Cow::Owned(desugar_operators(program.into_owned(), hm_expr_types, interner))
    } else {
        program
    }
}
