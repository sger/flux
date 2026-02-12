use crate::ast::visit::{self, Visitor};
use crate::diagnostics::position::Span;
use crate::syntax::{
    expression::Expression, program::Program, statement::Statement, symbol::Symbol,
};

/// Per-function complexity metrics.
#[derive(Debug, Clone)]
pub struct FunctionMetrics {
    /// Function name (`None` for anonymous lambdas).
    pub name: Option<Symbol>,
    /// Source span of the function.
    pub span: Span,
    /// McCabe cyclomatic complexity (branches + 1).
    pub cyclomatic_complexity: usize,
    /// Maximum nesting depth of branching expressions.
    pub max_nesting_depth: usize,
    /// Total number of match arms across all match expressions.
    pub match_arm_count: usize,
    /// Number of parameters.
    pub parameter_count: usize,
}

/// Inner analyzer that measures a single function body.
/// Does NOT descend into nested functions — those are measured separately.
struct FunctionAnalyzer {
    branches: usize,
    depth: usize,
    max_depth: usize,
    match_arms: usize,
}

impl FunctionAnalyzer {
    fn new() -> Self {
        Self {
            branches: 0,
            depth: 0,
            max_depth: 0,
            match_arms: 0,
        }
    }
}

impl<'ast> Visitor<'ast> for FunctionAnalyzer {
    fn visit_expr(&mut self, expr: &'ast Expression) {
        match expr {
            Expression::If { .. } => {
                self.branches += 1;
                self.depth += 1;
                self.max_depth = self.max_depth.max(self.depth);
                visit::walk_expr(self, expr);
                self.depth -= 1;
            }
            Expression::Match { arms, .. } => {
                self.branches += arms.len().saturating_sub(1);
                self.match_arms += arms.len();
                self.depth += 1;
                self.max_depth = self.max_depth.max(self.depth);
                visit::walk_expr(self, expr);
                self.depth -= 1;
            }
            // Skip nested functions — they get their own metrics entry
            Expression::Function { .. } => {}
            _ => visit::walk_expr(self, expr),
        }
    }
}

/// Top-level collector that finds functions and measures each one.
struct ComplexityCollector {
    metrics: Vec<FunctionMetrics>,
}

impl ComplexityCollector {
    fn new() -> Self {
        Self {
            metrics: Vec::new(),
        }
    }

    fn analyze_function(
        &mut self,
        name: Option<Symbol>,
        parameters: usize,
        body: &crate::syntax::block::Block,
        span: Span,
    ) {
        let mut analyzer = FunctionAnalyzer::new();
        analyzer.visit_block(body);

        self.metrics.push(FunctionMetrics {
            name,
            span,
            cyclomatic_complexity: analyzer.branches + 1,
            max_nesting_depth: analyzer.max_depth,
            match_arm_count: analyzer.match_arms,
            parameter_count: parameters,
        });
    }
}

impl<'ast> Visitor<'ast> for ComplexityCollector {
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        if let Statement::Function {
            name,
            parameters,
            body,
            span,
        } = stmt
        {
            self.analyze_function(Some(*name), parameters.len(), body, *span);
            // Continue walking to find nested functions inside the body
            visit::walk_stmt(self, stmt);
        } else {
            visit::walk_stmt(self, stmt);
        }
    }

    fn visit_expr(&mut self, expr: &'ast Expression) {
        if let Expression::Function {
            parameters,
            body,
            span,
        } = expr
        {
            self.analyze_function(None, parameters.len(), body, *span);
            // Continue walking to find nested functions inside the body
            visit::walk_expr(self, expr);
        } else {
            visit::walk_expr(self, expr);
        }
    }
}

/// Analyze complexity of all functions in a program.
///
/// Returns one `FunctionMetrics` per function/lambda, in visitation order.
pub fn analyze_complexity(program: &Program) -> Vec<FunctionMetrics> {
    let mut collector = ComplexityCollector::new();
    collector.visit_program(program);
    collector.metrics
}
