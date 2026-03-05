use std::collections::{HashMap, HashSet};

use crate::ast::visit::{self, Visitor};
use crate::syntax::{
    expression::{Expression, Pattern},
    program::Program,
    statement::Statement,
    symbol::Symbol,
};

/// Collects free variables — identifiers referenced but not bound in scope.
///
/// Uses a flat `HashMap<Symbol, usize>` where the value is the number of
/// active scopes that bind the symbol. This gives O(1) lookup instead of
/// O(depth) linear scan through a scope stack.
struct FreeVarCollector {
    bound: HashMap<Symbol, usize>,
    free: HashSet<Symbol>,
}

impl FreeVarCollector {
    fn new() -> Self {
        Self {
            bound: HashMap::new(),
            free: HashSet::new(),
        }
    }

    fn define(&mut self, name: Symbol) {
        *self.bound.entry(name).or_insert(0) += 1;
    }

    fn undefine(&mut self, name: Symbol) {
        if let Some(count) = self.bound.get_mut(&name) {
            *count -= 1;
            if *count == 0 {
                self.bound.remove(&name);
            }
        }
    }

    fn is_bound(&self, name: Symbol) -> bool {
        self.bound.contains_key(&name)
    }

    /// Execute `f` inside a scope where `names` are defined, then clean up.
    fn with_scope<F>(&mut self, names: &[Symbol], f: F)
    where
        F: FnOnce(&mut Self),
    {
        for &name in names {
            self.define(name);
        }
        f(self);
        for &name in names {
            self.undefine(name);
        }
    }

    fn define_pattern_bindings(&mut self, pattern: &Pattern) {
        match pattern {
            Pattern::Identifier { name, .. } => {
                self.define(*name);
            }
            Pattern::Some { pattern, .. }
            | Pattern::Left { pattern, .. }
            | Pattern::Right { pattern, .. } => {
                self.define_pattern_bindings(pattern);
            }
            Pattern::Cons { head, tail, .. } => {
                self.define_pattern_bindings(head);
                self.define_pattern_bindings(tail);
            }
            Pattern::Tuple { elements, .. } => {
                for element in elements {
                    self.define_pattern_bindings(element);
                }
            }
            Pattern::Constructor { fields, .. } => {
                for field in fields {
                    self.define_pattern_bindings(field);
                }
            }
            Pattern::Wildcard { .. }
            | Pattern::Literal { .. }
            | Pattern::None { .. }
            | Pattern::EmptyList { .. } => {}
        }
    }

    /// Collect all symbol names bound by a pattern (for scope cleanup).
    fn collect_pattern_names(&self, pattern: &Pattern) -> Vec<Symbol> {
        let mut names = Vec::new();
        Self::collect_pattern_names_into(pattern, &mut names);
        names
    }

    fn collect_pattern_names_into(pattern: &Pattern, names: &mut Vec<Symbol>) {
        match pattern {
            Pattern::Identifier { name, .. } => names.push(*name),
            Pattern::Some { pattern, .. }
            | Pattern::Left { pattern, .. }
            | Pattern::Right { pattern, .. } => Self::collect_pattern_names_into(pattern, names),
            Pattern::Cons { head, tail, .. } => {
                Self::collect_pattern_names_into(head, names);
                Self::collect_pattern_names_into(tail, names);
            }
            Pattern::Tuple { elements, .. } | Pattern::Constructor { fields: elements, .. } => {
                for element in elements {
                    Self::collect_pattern_names_into(element, names);
                }
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
                ..
            } => {
                // Visit value before defining the binding (value can't reference itself).
                self.visit_expr(value);
                self.define(*name);
            }
            Statement::LetDestructure {
                pattern,
                value,
                span: _,
            } => {
                self.visit_expr(value);
                self.define_pattern_bindings(pattern);
            }
            Statement::Function {
                name,
                parameters,
                body,
                span: _,
                ..
            } => {
                // Define function in outer scope first to support recursion.
                self.define(*name);
                let params: Vec<Symbol> = parameters.to_vec();
                self.with_scope(&params, |this| {
                    this.visit_block(body);
                });
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
                ..
            } => {
                let params: Vec<Symbol> = parameters.to_vec();
                self.with_scope(&params, |this| {
                    this.visit_block(body);
                });
            }
            Expression::Match {
                scrutinee, arms, ..
            } => {
                self.visit_expr(scrutinee);
                for arm in arms {
                    let names = self.collect_pattern_names(&arm.pattern);
                    self.define_pattern_bindings(&arm.pattern);
                    if let Some(guard) = &arm.guard {
                        self.visit_expr(guard);
                    }
                    self.visit_expr(&arm.body);
                    for name in &names {
                        self.undefine(*name);
                    }
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
