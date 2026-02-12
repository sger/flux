use flux::ast::visit::{self, Visitor};
use flux::syntax::{
    Identifier, expression::Expression, lexer::Lexer, parser::Parser, program::Program,
    statement::Statement,
};

/// A visitor that counts expressions, statements, and identifier references.
struct NodeCounter {
    exprs: usize,
    stmts: usize,
    idents: usize,
}

impl NodeCounter {
    fn new() -> Self {
        Self {
            exprs: 0,
            stmts: 0,
            idents: 0,
        }
    }
}

impl<'ast> Visitor<'ast> for NodeCounter {
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        self.stmts += 1;
        visit::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'ast Expression) {
        self.exprs += 1;
        visit::walk_expr(self, expr);
    }

    fn visit_identifier(&mut self, _ident: &'ast Identifier) {
        self.idents += 1;
    }
}

fn parse(input: &str) -> Program {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "Parser errors: {:?}",
        parser.errors
    );
    program
}

#[test]
fn counts_simple_let() {
    // `let x = 1;` → 1 stmt, 1 expr (Integer), 2 idents (x binding + x in Let)
    let program = parse("let x = 1;");
    let mut counter = NodeCounter::new();
    counter.visit_program(&program);

    assert_eq!(counter.stmts, 1);
    assert_eq!(counter.exprs, 1); // the integer literal
    assert_eq!(counter.idents, 1); // `x` in let binding
}

#[test]
fn counts_infix_expression() {
    // `1 + 2;` → 1 stmt, 3 exprs (Infix, Integer 1, Integer 2), 0 idents
    let program = parse("1 + 2;");
    let mut counter = NodeCounter::new();
    counter.visit_program(&program);

    assert_eq!(counter.stmts, 1);
    assert_eq!(counter.exprs, 3); // Infix + two Integer leaves
    assert_eq!(counter.idents, 0);
}

#[test]
fn counts_function_call() {
    // `fun add(a, b) { return a + b; }` and `add(1, 2);`
    let program = parse("fun add(a, b) { return a + b; }\nadd(1, 2);");
    let mut counter = NodeCounter::new();
    counter.visit_program(&program);

    // Stmts: Function decl, Return (inside body), Expression stmt (call)
    assert_eq!(counter.stmts, 3);
    // Exprs: (return) Infix(Ident(a), Ident(b)), Call(Ident(add), Int(1), Int(2))
    //   return value: Infix=1, a=1, b=1 → 3
    //   call stmt: Call=1, add=1, 1=1, 2=1 → 4
    assert_eq!(counter.exprs, 7);
}

#[test]
fn counts_multiple_statements() {
    let program = parse("let a = 1;\nlet b = 2;\nlet c = 3;");
    let mut counter = NodeCounter::new();
    counter.visit_program(&program);

    assert_eq!(counter.stmts, 3);
    assert_eq!(counter.exprs, 3); // three integer literals
    assert_eq!(counter.idents, 3); // a, b, c
}

#[test]
fn counts_nested_if() {
    // if (true) { 1; } else { 2; }
    let program = parse("if (true) { 1; } else { 2; };");
    let mut counter = NodeCounter::new();
    counter.visit_program(&program);

    // 1 top-level expression stmt
    // Inside if: 1 stmt in consequence, 1 stmt in alternative
    assert_eq!(counter.stmts, 3);
    // If expr, Boolean true, Integer 1, Integer 2
    assert_eq!(counter.exprs, 4);
}

#[test]
fn counts_array_elements() {
    let program = parse("[1, 2, 3];");
    let mut counter = NodeCounter::new();
    counter.visit_program(&program);

    assert_eq!(counter.stmts, 1);
    // Array expr + 3 integer elements
    assert_eq!(counter.exprs, 4);
}
