use flux::ast::complexity::analyze_complexity;
use flux::syntax::{lexer::Lexer, parser::Parser, program::Program};

fn parse(input: &str) -> (Program, flux::syntax::interner::Interner) {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "Parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    (program, interner)
}

#[test]
fn simple_function_complexity_one() {
    let (program, _interner) = parse("fn f(x) { x; }");
    let metrics = analyze_complexity(&program);
    assert_eq!(metrics.len(), 1);
    assert_eq!(metrics[0].cyclomatic_complexity, 1);
    assert_eq!(metrics[0].max_nesting_depth, 0);
    assert_eq!(metrics[0].parameter_count, 1);
}

#[test]
fn single_if_adds_one_branch() {
    let (program, _) = parse(
        r#"
        fn f(x) {
            if x > 0 {
                1;
            } else {
                0;
            };
        }
    "#,
    );
    let metrics = analyze_complexity(&program);
    assert_eq!(metrics.len(), 1);
    assert_eq!(metrics[0].cyclomatic_complexity, 2); // 1 branch + 1
    assert_eq!(metrics[0].max_nesting_depth, 1);
}

#[test]
fn nested_if_increases_depth() {
    let (program, _) = parse(
        r#"
        fn f(x) {
            if x > 0 {
                if x > 10 {
                    2;
                } else {
                    1;
                };
            } else {
                0;
            };
        }
    "#,
    );
    let metrics = analyze_complexity(&program);
    assert_eq!(metrics.len(), 1);
    assert_eq!(metrics[0].cyclomatic_complexity, 3); // 2 branches + 1
    assert_eq!(metrics[0].max_nesting_depth, 2);
}

#[test]
fn match_with_three_arms() {
    let (program, _) = parse(
        r#"
        fn f(x) {
            match x {
                1 -> "one",
                2 -> "two",
                _ -> "other",
            }
        }
    "#,
    );
    let metrics = analyze_complexity(&program);
    assert_eq!(metrics.len(), 1);
    assert_eq!(metrics[0].cyclomatic_complexity, 3); // (3 - 1) branches + 1
    assert_eq!(metrics[0].match_arm_count, 3);
    assert_eq!(metrics[0].max_nesting_depth, 1);
}

#[test]
fn nested_functions_measured_independently() {
    let (program, interner) = parse(
        r#"
        fn outer(x) {
            let inner = \y -> if y > 0 { y; } else { 0; };
            if x > 0 {
                inner(x);
            } else {
                0;
            };
        }
    "#,
    );
    let metrics = analyze_complexity(&program);
    assert_eq!(metrics.len(), 2);

    // Find outer by name
    let outer = metrics
        .iter()
        .find(|m| m.name.is_some() && interner.resolve(m.name.unwrap()) == "outer")
        .expect("should find outer");
    // Outer has 1 if branch, the lambda is NOT counted inside outer
    assert_eq!(outer.cyclomatic_complexity, 2);

    // Find the anonymous lambda
    let lambda = metrics
        .iter()
        .find(|m| m.name.is_none())
        .expect("should find lambda");
    assert_eq!(lambda.cyclomatic_complexity, 2); // 1 if + 1
    assert_eq!(lambda.parameter_count, 1);
}

#[test]
fn no_functions_returns_empty() {
    let (program, _) = parse("let x = 1;");
    let metrics = analyze_complexity(&program);
    assert!(metrics.is_empty());
}

#[test]
fn function_with_many_params() {
    let (program, _) = parse("fn f(a, b, c, d, e, g) { a; }");
    let metrics = analyze_complexity(&program);
    assert_eq!(metrics.len(), 1);
    assert_eq!(metrics[0].parameter_count, 6);
}

#[test]
fn mixed_if_and_match() {
    let (program, _) = parse(
        r#"
        fn f(x) {
            if x > 0 {
                match x {
                    1 -> "one",
                    _ -> "other",
                }
            } else {
                "negative";
            };
        }
    "#,
    );
    let metrics = analyze_complexity(&program);
    assert_eq!(metrics.len(), 1);
    // 1 if + 1 match arm (2 - 1) = 2 branches → complexity 3
    assert_eq!(metrics[0].cyclomatic_complexity, 3);
    assert_eq!(metrics[0].max_nesting_depth, 2); // if > match
    assert_eq!(metrics[0].match_arm_count, 2);
}
