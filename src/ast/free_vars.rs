use std::collections::HashSet;

use crate::ast::visit::{self, Visitor};
use crate::syntax::{
    expression::{Expression, Pattern},
    program::Program,
    statement::Statement,
    symbol::Symbol,
};

/// Collects free variables — identifiers referenced but not bound in scope.
struct FreeVarCollector {
    scopes: Vec<HashSet<Symbol>>,
    free: HashSet<Symbol>,
}

impl FreeVarCollector {
    fn new() -> Self {
        Self {
            scopes: vec![HashSet::new()],
            free: HashSet::new(),
        }
    }

    fn define(&mut self, name: Symbol) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name);
        }
    }

    fn is_bound(&self, name: Symbol) -> bool {
        self.scopes.iter().rev().any(|s| s.contains(&name))
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashSet::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn extract_pattern_bindings(&mut self, pattern: &Pattern) {
        match pattern {
            Pattern::Identifier { name, .. } => {
                self.define(*name);
            }
            Pattern::Some { pattern, .. }
            | Pattern::Left { pattern, .. }
            | Pattern::Right { pattern, .. } => {
                self.extract_pattern_bindings(pattern);
            }
            Pattern::Cons { head, tail, .. } => {
                self.extract_pattern_bindings(head);
                self.extract_pattern_bindings(tail);
            }
            Pattern::Wildcard { .. }
            | Pattern::Literal { .. }
            | Pattern::None { .. }
            | Pattern::EmptyList { .. } => {}
        }
    }
}

impl<'ast> Visitor<'ast> for FreeVarCollector {
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::Let {
                name,
                value,
                span: _,
            } => {
                // Visit value before defining the binding (value can't reference itself).
                self.visit_expr(value);
                self.define(*name);
            }
            Statement::Function {
                name,
                parameters,
                body,
                span: _,
            } => {
                // Define function in outer scope first to support recursion.
                self.define(*name);
                self.push_scope();
                for param in parameters {
                    self.define(*param);
                }
                self.visit_block(body);
                self.pop_scope();
            }
            Statement::Assign {
                name,
                value,
                span: _,
            } => {
                if !self.is_bound(*name) {
                    self.free.insert(*name);
                }
                self.visit_expr(value);
            }
            _ => visit::walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &'ast Expression) {
        match expr {
            Expression::Identifier { name, .. } => {
                if !self.is_bound(*name) {
                    self.free.insert(*name);
                }
            }
            Expression::Function {
                parameters,
                body,
                span: _,
            } => {
                self.push_scope();
                for param in parameters {
                    self.define(*param);
                }
                self.visit_block(body);
                self.pop_scope();
            }
            Expression::Match {
                scrutinee, arms, ..
            } => {
                self.visit_expr(scrutinee);
                for arm in arms {
                    self.push_scope();
                    self.extract_pattern_bindings(&arm.pattern);
                    if let Some(guard) = &arm.guard {
                        self.visit_expr(guard);
                    }
                    self.visit_expr(&arm.body);
                    self.pop_scope();
                }
            }
            _ => visit::walk_expr(self, expr),
        }
    }
}

/// Collect free variables in an expression.
pub fn collect_free_vars(expr: &Expression) -> HashSet<Symbol> {
    let mut collector = FreeVarCollector::new();
    collector.visit_expr(expr);
    collector.free
}

/// Collect free variables across an entire program.
pub fn collect_free_vars_in_program(program: &Program) -> HashSet<Symbol> {
    let mut collector = FreeVarCollector::new();
    collector.visit_program(program);
    collector.free
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::ast::free_vars::collect_free_vars_in_program;
    use crate::syntax::{lexer::Lexer, parser::Parser};

    fn free_var_names(source: &str) -> HashSet<String> {
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();

        collect_free_vars_in_program(&program)
            .into_iter()
            .map(|sym| interner.resolve(sym).to_string())
            .collect()
    }

    #[test]
    fn collects_unbound_names_but_not_outer_bound_names() {
        let free = free_var_names(
            r#"
let x = 1;
let f = fn() { x + y; };
"#,
        );
        assert!(free.contains("y"));
        assert!(!free.contains("x"));
    }

    #[test]
    fn recursive_function_name_is_not_free() {
        let free = free_var_names(
            r#"
fn fact(n) {
    if n == 0 { 1; } else { fact(n - 1); }
}
"#,
        );
        assert!(free.is_empty(), "expected no free vars, got {free:?}");
    }

    #[test]
    fn match_pattern_bindings_are_not_free() {
        let free = free_var_names(
            r#"
let value = Some(1);
match value {
    Some(v) -> v,
    _ -> missing
};
"#,
        );
        assert!(free.contains("missing"));
        assert!(!free.contains("value"));
        assert!(!free.contains("v"));
    }
}
