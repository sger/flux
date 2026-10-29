use super::*;

fn first_perform_expr(program: &flux::syntax::program::Program) -> Option<&Expression> {
    fn walk_expr(expr: &Expression) -> Option<&Expression> {
        match expr {
            Expression::Perform { .. } => Some(expr),
            Expression::Prefix { right, .. } => walk_expr(right),
            Expression::Infix { left, right, .. } => walk_expr(left).or_else(|| walk_expr(right)),
            Expression::If { condition, consequence, alternative, .. } => walk_expr(condition)
                .or_else(|| walk_block(consequence))
                .or_else(|| alternative.as_ref().and_then(walk_block)),
            Expression::DoBlock { block, .. } => walk_block(block),
            Expression::Function { body, .. } => walk_block(body),
            Expression::Call { function, arguments, .. } => {
                walk_expr(function).or_else(|| arguments.iter().find_map(walk_expr))
            }
            Expression::TupleLiteral { elements, .. }
            | Expression::ListLiteral { elements, .. }
            | Expression::ArrayLiteral { elements, .. } => elements.iter().find_map(walk_expr),
            Expression::Hash { pairs, .. } => {
                pairs.iter().find_map(|(k, v)| walk_expr(k).or_else(|| walk_expr(v)))
            }
            Expression::Index { left, index, .. } => walk_expr(left).or_else(|| walk_expr(index)),
            Expression::MemberAccess { object, .. }
            | Expression::TupleFieldAccess { object, .. } => walk_expr(object),
            Expression::Match { scrutinee, arms, .. } => walk_expr(scrutinee).or_else(|| {
                arms.iter().find_map(|arm| {
                    arm.guard.as_ref().and_then(walk_expr).or_else(|| walk_expr(&arm.body))
                })
            }),
            Expression::Some { value, .. }
            | Expression::Left { value, .. }
            | Expression::Right { value, .. } => walk_expr(value),
            Expression::Cons { head, tail, .. } => walk_expr(head).or_else(|| walk_expr(tail)),
            Expression::Handle { expr, arms, .. } => {
                walk_expr(expr).or_else(|| arms.iter().find_map(|arm| walk_expr(&arm.body)))
            }
            _ => None,
        }
    }

    fn walk_stmt(statement: &Statement) -> Option<&Expression> {
        match statement {
            Statement::Let { value, .. }
            | Statement::LetDestructure { value, .. }
            | Statement::Assign { value, .. } => walk_expr(value),
            Statement::Return { value, .. } => value.as_ref().and_then(walk_expr),
            Statement::Expression { expression, .. } => walk_expr(expression),
            Statement::Function { body, .. } | Statement::Module { body, .. } => walk_block(body),
            _ => None,
        }
    }

    fn walk_block(block: &flux::syntax::block::Block) -> Option<&Expression> {
        block.statements.iter().find_map(walk_stmt)
    }

    program.statements.iter().find_map(walk_stmt)
}

fn first_handle_expr(program: &flux::syntax::program::Program) -> Option<&Expression> {
    fn walk_expr(expr: &Expression) -> Option<&Expression> {
        match expr {
            Expression::Handle { .. } => Some(expr),
            Expression::Prefix { right, .. } => walk_expr(right),
            Expression::Infix { left, right, .. } => walk_expr(left).or_else(|| walk_expr(right)),
            Expression::If { condition, consequence, alternative, .. } => walk_expr(condition)
                .or_else(|| walk_block(consequence))
                .or_else(|| alternative.as_ref().and_then(walk_block)),
            Expression::DoBlock { block, .. } => walk_block(block),
            Expression::Function { body, .. } => walk_block(body),
            Expression::Call { function, arguments, .. } => {
                walk_expr(function).or_else(|| arguments.iter().find_map(walk_expr))
            }
            Expression::TupleLiteral { elements, .. }
            | Expression::ListLiteral { elements, .. }
            | Expression::ArrayLiteral { elements, .. } => elements.iter().find_map(walk_expr),
            Expression::Hash { pairs, .. } => {
                pairs.iter().find_map(|(k, v)| walk_expr(k).or_else(|| walk_expr(v)))
            }
            Expression::Index { left, index, .. } => walk_expr(left).or_else(|| walk_expr(index)),
            Expression::MemberAccess { object, .. }
            | Expression::TupleFieldAccess { object, .. } => walk_expr(object),
            Expression::Match { scrutinee, arms, .. } => walk_expr(scrutinee).or_else(|| {
                arms.iter().find_map(|arm| {
                    arm.guard.as_ref().and_then(walk_expr).or_else(|| walk_expr(&arm.body))
                })
            }),
            Expression::Some { value, .. }
            | Expression::Left { value, .. }
            | Expression::Right { value, .. } => walk_expr(value),
            Expression::Cons { head, tail, .. } => walk_expr(head).or_else(|| walk_expr(tail)),
            _ => None,
        }
    }

    fn walk_stmt(statement: &Statement) -> Option<&Expression> {
        match statement {
            Statement::Let { value, .. }
            | Statement::LetDestructure { value, .. }
            | Statement::Assign { value, .. } => walk_expr(value),
            Statement::Return { value, .. } => value.as_ref().and_then(walk_expr),
            Statement::Expression { expression, .. } => walk_expr(expression),
            Statement::Function { body, .. } | Statement::Module { body, .. } => walk_block(body),
            _ => None,
        }
    }

    fn walk_block(block: &flux::syntax::block::Block) -> Option<&Expression> {
        block.statements.iter().find_map(walk_stmt)
    }

    program.statements.iter().find_map(walk_stmt)
}

#[test]
fn infer_perform_returns_declared_op_return_type() {
    let (result, program) = infer_program_from_source(
        r#"
effect Console {
    read: String -> Int
}
fn main() -> Unit with Console {
    let x = perform Console.read("name")
}
"#,
    );
    let perform = first_perform_expr(&program).expect("expected perform expression");
    let ty = result
        .expr_types
        .get(&perform.expr_id())
        .expect("expected inferred type for perform expression");
    assert_eq!(*ty, int());
}

#[test]
fn infer_perform_arg_type_mismatch_reports_e300() {
    let (result, _program) = infer_program_from_source(
        r#"
effect Console {
    print: String -> Unit
}
fn main() -> Unit with Console {
    perform Console.print(1)
}
"#,
    );
    assert!(
        result.diagnostics.iter().any(|d| d.code().is_some_and(|code| code == "E300")),
        "expected E300 diagnostics for perform arg mismatch, got: {:?}",
        result.diagnostics.iter().map(|d| d.code().unwrap_or("")).collect::<Vec<_>>()
    );
}

#[test]
fn infer_parameterized_state_handler_is_hm_clean() {
    let (result, _program) = infer_program_from_source(
        r#"
effect State {
    get: () -> Int,
    set: Int -> Unit
}

fn program() -> Int with State {
    perform State.set(10)
    let x = perform State.get()
    perform State.set(x + 5)
    perform State.get()
}

fn main() -> Unit {
    let _ = program() handle State(0) {
        get(resume, state) -> resume(state, state),
        set(resume, value, _state) -> resume((), value)
    }
}
"#,
    );
    assert!(
        result.diagnostics.is_empty(),
        "expected parameterized State handler to be HM-clean, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn infer_parameterized_reader_handler_is_hm_clean() {
    let (result, _program) = infer_program_from_source(
        r#"
effect Config {
    ask: () -> String
}

fn describe() -> String with Config {
    perform Config.ask()
}

fn main() -> Unit {
    let _ = describe() handle Config("flux-server") {
        ask(resume, env) -> resume(env, env)
    }
}
"#,
    );
    assert!(
        result.diagnostics.is_empty(),
        "expected parameterized Reader handler to be HM-clean, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn infer_parameterized_handler_missing_state_binder_reports_e428() {
    let (result, _program) = infer_program_from_source(
        r#"
effect State {
    get: () -> Int
}

fn program() -> Int with State {
    perform State.get()
}

fn main() -> Unit {
    let _ = program() handle State(0) {
        get(resume) -> resume(0, 0)
    }
}
"#,
    );
    assert!(
        has_diagnostic_code(&result, "E428"),
        "expected E428 for missing state binder, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn infer_parameterized_handler_one_arg_resume_reports_e428() {
    let (result, _program) = infer_program_from_source(
        r#"
effect State {
    get: () -> Int
}

fn program() -> Int with State {
    perform State.get()
}

fn main() -> Unit {
    let _ = program() handle State(0) {
        get(resume, state) -> resume(state)
    }
}
"#,
    );
    assert!(
        has_diagnostic_code(&result, "E428"),
        "expected E428 for one-argument resume in parameterized handler, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn infer_non_parameterized_handler_two_arg_resume_reports_e428() {
    let (result, _program) = infer_program_from_source(
        r#"
effect State {
    get: () -> Int
}

fn program() -> Int with State {
    perform State.get()
}

fn main() -> Unit {
    let _ = program() handle State {
        get(resume) -> resume(0, 0)
    }
}
"#,
    );
    assert!(
        has_diagnostic_code(&result, "E428"),
        "expected E428 for two-argument resume in non-parameterized handler, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn infer_parameterized_handler_fallthrough_is_hm_clean() {
    let (result, _program) = infer_program_from_source(
        r#"
effect Pick {
    pick: () -> Int
}

fn choose() -> Int with Pick {
    perform Pick.pick()
}

fn main() -> Unit {
    let _ = choose() handle Pick(41) {
        pick(_resume, state) -> state
    }
}
"#,
    );
    assert!(
        result.diagnostics.is_empty(),
        "expected parameterized handler fallthrough to remain HM-clean, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn infer_polymorphic_effect_op_instantiates_per_perform_site() {
    let (result, _program) = infer_program_from_source(
        r#"
effect Log {
    emit: a -> Unit
}
fn main() -> Unit with Log {
    perform Log.emit(1)
    perform Log.emit("hi")
}
"#,
    );
    assert!(
        result.diagnostics.is_empty(),
        "expected no diagnostics for polymorphic effect op reuse, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn infer_polymorphic_effect_op_inside_polymorphic_function_is_hm_clean() {
    let (result, _program) = infer_program_from_source(
        r#"
effect Log {
    emit: a -> Unit
}
fn forward<a>(x: a) -> a with Log {
    perform Log.emit(x)
    x
}
fn main() -> Unit with Log {
    let _ = forward(1)
    let _ = forward("hi")
}
"#,
    );
    assert!(
        result.diagnostics.is_empty(),
        "expected no diagnostics for polymorphic function performing polymorphic op, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn infer_polymorphic_effect_op_deliberate_mismatch_still_reports_e300() {
    let (result, _program) = infer_program_from_source(
        r#"
effect Id {
    bounce: a -> a
}
fn main() -> Unit with Id {
    let _: Int = perform Id.bounce("oops")
}
"#,
    );
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected E300 for mismatched instantiated polymorphic effect op, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn infer_effect_row_order_equivalence_for_function_params() {
    let (result, _program) = infer_program_from_source(
        r#"
fn call_swapped_effects(f: ((Int) -> Int with Time, IO), x: Int) -> Int with IO, Time {
    f(x)
}

fn add_one(x: Int) -> Int with IO, Time {
    x + 1
}

fn main() -> Unit {
    let _ = call_swapped_effects(add_one, 1)
}
"#,
    );
    assert!(
        result.diagnostics.is_empty(),
        "expected no diagnostics for order-equivalent effect rows, got: {:?}",
        result.diagnostics.iter().map(|d| d.code().unwrap_or("")).collect::<Vec<_>>()
    );
}

#[test]
fn infer_effect_row_subtract_var_signature_stays_hm_clean() {
    let (result, _program) = infer_program_from_source(
        r#"
effect Console {
    print: String -> ()
}
fn run_filtered(f: (() -> Int with |e - Console)) -> Int with |e - Console {
    f()
}
fn io_work() -> Int with IO {
    print("work")
    10
}
fn main() -> Unit {
    let _ = run_filtered(io_work)
}
"#,
    );
    assert!(
        result.diagnostics.is_empty(),
        "expected no HM diagnostics for satisfiable row-variable subtraction signature, got: {:?}",
        result.diagnostics.iter().map(|d| d.code().unwrap_or("")).collect::<Vec<_>>()
    );
}

#[test]
fn infer_effect_row_subset_callback_requirement_emits_e300() {
    let (result, _program) = infer_program_from_source(
        r#"
fn run_needs_io_time(f: (() -> Int with IO, Time)) -> Int with IO, Time {
    f()
}
fn only_io() -> Int with IO {
    1
}
fn main() -> Unit {
    let _ = run_needs_io_time(only_io)
}
"#,
    );
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected HM effect-row mismatch diagnostics, got: {:?}",
        result.diagnostics.iter().map(|d| d.code().unwrap_or("")).collect::<Vec<_>>()
    );
}

#[test]
fn infer_effect_row_absent_ordering_linked_vars_stays_hm_clean() {
    let (result, _program) = infer_program_from_source(
        r#"
fn needs_linked_absence(
    first: (() -> Int with |e - IO),
    second: (() -> Int with |e)
) -> Int with |e - IO {
    first() + second()
}
fn row_var_worker() -> Int with |e {
    1
}
fn time_worker() -> Int with Time {
    now_ms()
    2
}
fn main() -> Unit with Time {
    let _ = needs_linked_absence(row_var_worker, time_worker)
}
"#,
    );
    assert!(
        result.diagnostics.is_empty(),
        "expected HM to remain diagnostics-clean for linked row-var absence flow; compiler enforces call-site row constraints, got: {:?}",
        result.diagnostics.iter().map(|d| d.code().unwrap_or("")).collect::<Vec<_>>()
    );
}

#[test]
fn infer_handle_result_matches_handled_expression_type() {
    let (result, program) = infer_program_from_source(
        r#"
effect Console {
    print: String -> Int
}
fn main() -> Unit with Console {
    let x = (perform Console.print("x")) handle Console {
        print(resume, _msg) -> 1
    }
}
"#,
    );
    let handle = first_handle_expr(&program).expect("expected handle expression");
    let ty = result
        .expr_types
        .get(&handle.expr_id())
        .expect("expected inferred type for handle expression");
    assert_eq!(*ty, int());
}

#[test]
fn infer_handle_arms_bind_declared_param_types() {
    let (result, _program) = infer_program_from_source(
        r#"
effect Console {
    print: String -> Int
}
fn main() -> Unit with Console {
    let _ = (perform Console.print("x")) handle Console {
        print(resume, msg) -> msg + 1
    }
}
"#,
    );
    assert!(
        result.diagnostics.iter().any(|d| d.code().is_some_and(|code| code == "E300")),
        "expected E300 diagnostics for handler param type usage mismatch"
    );
}

#[test]
fn infer_polymorphic_handle_arm_supports_multiple_perform_instantiations() {
    let (result, _program) = infer_program_from_source(
        r#"
effect Id {
    bounce: a -> a
}
fn run() -> Int with Id {
    let x = perform Id.bounce(1)
    let _y = perform Id.bounce("hi")
    x
}
fn main() -> Unit with Id {
    let _ = run() handle Id {
        bounce(resume, v) -> resume(v)
    }
}
"#,
    );
    assert!(
        result.diagnostics.is_empty(),
        "expected no diagnostics for polymorphic handler arm across perform instantiations, got: {:?}",
        result.diagnostics
    );
}
