use crate::ast::visit::{self, Visitor};
use crate::diagnostics::position::Span;
use crate::syntax::{block::Block, expression::Expression, program::Program, statement::Statement};

/// A call expression found in tail position.
#[derive(Debug, Clone)]
pub struct TailCall {
    /// Source span of the tail call expression.
    pub span: Span,
}

/// Finds all `Expression::Call` nodes that occur in tail position.
///
/// Tail position rules mirror the bytecode compiler:
/// - The last Expression/Return statement in a function body is in tail position
/// - Both branches of an `if` expression inherit tail position from their parent
/// - All match arm bodies inherit tail position from their parent
/// - A Call expression in tail position is a tail call
struct TailPositionAnalyzer {
    in_tail: bool,
    tail_calls: Vec<TailCall>,
}

impl TailPositionAnalyzer {
    fn new() -> Self {
        Self {
            in_tail: false,
            tail_calls: Vec::new(),
        }
    }

    /// Walk a block with tail-position awareness on the last statement.
    /// Mirrors `compile_block_with_tail` in the bytecode compiler.
    fn visit_block_with_tail(&mut self, block: &Block) {
        let len = block.statements.len();
        for (i, stmt) in block.statements.iter().enumerate() {
            let is_last = i == len - 1;
            let tail_eligible = matches!(
                stmt,
                Statement::Expression { .. } | Statement::Return { .. }
            );

            if is_last && tail_eligible {
                let was_tail = self.in_tail;
                self.in_tail = true;
                self.visit_stmt(stmt);
                self.in_tail = was_tail;
            } else {
                let was_tail = self.in_tail;
                self.in_tail = false;
                self.visit_stmt(stmt);
                self.in_tail = was_tail;
            }
        }
    }
}

impl<'ast> Visitor<'ast> for TailPositionAnalyzer {
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::Return { value, span: _ } => {
                if let Some(expr) = value {
                    let was_tail = self.in_tail;
                    self.in_tail = true;
                    self.visit_expr(expr);
                    self.in_tail = was_tail;
                }
            }
            Statement::Function {
                name: _,
                parameters: _,
                body,
                span: _,
            } => {
                // Enter a new tail context for the function body
                let was_tail = self.in_tail;
                self.in_tail = true;
                self.visit_block_with_tail(body);
                self.in_tail = was_tail;
            }
            _ => {
                visit::walk_stmt(self, stmt);
            }
        }
    }

    fn visit_expr(&mut self, expr: &'ast Expression) {
        match expr {
            Expression::Call {
                function,
                arguments,
                span,
            } => {
                if self.in_tail {
                    self.tail_calls.push(TailCall { span: *span });
                }
                // Function and arguments are NOT in tail position
                let was_tail = self.in_tail;
                self.in_tail = false;
                self.visit_expr(function);
                for arg in arguments {
                    self.visit_expr(arg);
                }
                self.in_tail = was_tail;
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                span: _,
            } => {
                // Condition is NOT in tail position
                let was_tail = self.in_tail;
                self.in_tail = false;
                self.visit_expr(condition);
                self.in_tail = was_tail;

                // Both branches inherit tail position from parent
                if self.in_tail {
                    self.visit_block_with_tail(consequence);
                } else {
                    self.visit_block(consequence);
                }
                if let Some(alt) = alternative {
                    if self.in_tail {
                        self.visit_block_with_tail(alt);
                    } else {
                        self.visit_block(alt);
                    }
                }
            }
            Expression::Match {
                scrutinee,
                arms,
                span: _,
            } => {
                // Scrutinee is NOT in tail position
                let was_tail = self.in_tail;
                self.in_tail = false;
                self.visit_expr(scrutinee);
                self.in_tail = was_tail;

                // Arm bodies inherit tail position from parent
                for arm in arms {
                    visit::walk_pat(self, &arm.pattern);
                    if let Some(guard) = &arm.guard {
                        let was_tail = self.in_tail;
                        self.in_tail = false;
                        self.visit_expr(guard);
                        self.in_tail = was_tail;
                    }
                    // Arm body inherits tail position
                    self.visit_expr(&arm.body);
                }
            }
            Expression::Function {
                parameters: _,
                body,
                span: _,
            } => {
                // Enter a new tail context for the lambda body
                let was_tail = self.in_tail;
                self.in_tail = true;
                self.visit_block_with_tail(body);
                self.in_tail = was_tail;
            }
            _ => {
                // For all other expressions, children are NOT in tail position
                let was_tail = self.in_tail;
                self.in_tail = false;
                visit::walk_expr(self, expr);
                self.in_tail = was_tail;
            }
        }
    }
}

/// Find all call expressions in tail position within a program.
pub fn find_tail_calls(program: &Program) -> Vec<TailCall> {
    let mut analyzer = TailPositionAnalyzer::new();
    analyzer.visit_program(program);
    analyzer.tail_calls
}
