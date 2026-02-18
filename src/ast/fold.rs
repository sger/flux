use crate::syntax::{
    Identifier,
    block::Block,
    expression::{Expression, MatchArm, Pattern, StringPart},
    program::Program,
    statement::Statement,
};

/// AST folder (rewriter).
///
/// Every `fold_*` method receives an owned node and returns a (possibly
/// rewritten) owned node. Defaults call the corresponding `fold_*` free
/// function which reconstructs the node after folding its children.
pub trait Folder {
    fn fold_program(&mut self, program: Program) -> Program {
        fold_program(self, program)
    }

    fn fold_block(&mut self, block: Block) -> Block {
        fold_block(self, block)
    }

    fn fold_stmt(&mut self, stmt: Statement) -> Statement {
        fold_stmt(self, stmt)
    }

    fn fold_expr(&mut self, expr: Expression) -> Expression {
        fold_expr(self, expr)
    }

    fn fold_pat(&mut self, pat: Pattern) -> Pattern {
        fold_pat(self, pat)
    }

    fn fold_match_arm(&mut self, arm: MatchArm) -> MatchArm {
        fold_match_arm(self, arm)
    }

    fn fold_string_part(&mut self, part: StringPart) -> StringPart {
        fold_string_part(self, part)
    }

    fn fold_identifier(&mut self, ident: Identifier) -> Identifier {
        ident
    }
}

// ---------------------------------------------------------------------------
// fold_* free functions – exhaustive destructuring so that adding a new
// field or variant causes a compile error until this code is updated.
// ---------------------------------------------------------------------------

pub fn fold_program<F: Folder + ?Sized>(folder: &mut F, program: Program) -> Program {
    let Program { statements, span } = program;
    Program {
        statements: statements
            .into_iter()
            .map(|s| folder.fold_stmt(s))
            .collect(),
        span,
    }
}

pub fn fold_block<F: Folder + ?Sized>(folder: &mut F, block: Block) -> Block {
    let Block { statements, span } = block;
    Block {
        statements: statements
            .into_iter()
            .map(|s| folder.fold_stmt(s))
            .collect(),
        span,
    }
}

pub fn fold_stmt<F: Folder + ?Sized>(folder: &mut F, stmt: Statement) -> Statement {
    match stmt {
        Statement::Let { name, value, span } => Statement::Let {
            name: folder.fold_identifier(name),
            value: folder.fold_expr(value),
            span,
        },
        Statement::LetDestructure {
            pattern,
            value,
            span,
        } => Statement::LetDestructure {
            pattern: folder.fold_pat(pattern),
            value: folder.fold_expr(value),
            span,
        },
        Statement::Return { value, span } => Statement::Return {
            value: value.map(|v| folder.fold_expr(v)),
            span,
        },
        Statement::Expression { expression, span } => Statement::Expression {
            expression: folder.fold_expr(expression),
            span,
        },
        Statement::Function {
            name,
            parameters,
            body,
            span,
        } => Statement::Function {
            name: folder.fold_identifier(name),
            parameters: parameters
                .into_iter()
                .map(|p| folder.fold_identifier(p))
                .collect(),
            body: folder.fold_block(body),
            span,
        },
        Statement::Assign { name, value, span } => Statement::Assign {
            name: folder.fold_identifier(name),
            value: folder.fold_expr(value),
            span,
        },
        Statement::Module { name, body, span } => Statement::Module {
            name: folder.fold_identifier(name),
            body: folder.fold_block(body),
            span,
        },
        Statement::Import { name, alias, span } => Statement::Import {
            name: folder.fold_identifier(name),
            alias: alias.map(|a| folder.fold_identifier(a)),
            span,
        },
    }
}

pub fn fold_expr<F: Folder + ?Sized>(folder: &mut F, expr: Expression) -> Expression {
    match expr {
        Expression::Identifier { name, span } => Expression::Identifier {
            name: folder.fold_identifier(name),
            span,
        },
        Expression::Integer { value, span } => Expression::Integer { value, span },
        Expression::Float { value, span } => Expression::Float { value, span },
        Expression::String { value, span } => Expression::String { value, span },
        Expression::InterpolatedString { parts, span } => Expression::InterpolatedString {
            parts: parts
                .into_iter()
                .map(|p| folder.fold_string_part(p))
                .collect(),
            span,
        },
        Expression::Boolean { value, span } => Expression::Boolean { value, span },
        Expression::Prefix {
            operator,
            right,
            span,
        } => Expression::Prefix {
            operator,
            right: Box::new(folder.fold_expr(*right)),
            span,
        },
        Expression::Infix {
            left,
            operator,
            right,
            span,
        } => Expression::Infix {
            left: Box::new(folder.fold_expr(*left)),
            operator,
            right: Box::new(folder.fold_expr(*right)),
            span,
        },
        Expression::If {
            condition,
            consequence,
            alternative,
            span,
        } => Expression::If {
            condition: Box::new(folder.fold_expr(*condition)),
            consequence: folder.fold_block(consequence),
            alternative: alternative.map(|a| folder.fold_block(a)),
            span,
        },
        Expression::Function {
            parameters,
            body,
            span,
        } => Expression::Function {
            parameters: parameters
                .into_iter()
                .map(|p| folder.fold_identifier(p))
                .collect(),
            body: folder.fold_block(body),
            span,
        },
        Expression::Call {
            function,
            arguments,
            span,
        } => Expression::Call {
            function: Box::new(folder.fold_expr(*function)),
            arguments: arguments.into_iter().map(|a| folder.fold_expr(a)).collect(),
            span,
        },
        Expression::ListLiteral { elements, span } => Expression::ListLiteral {
            elements: elements.into_iter().map(|e| folder.fold_expr(e)).collect(),
            span,
        },
        Expression::ArrayLiteral { elements, span } => Expression::ArrayLiteral {
            elements: elements.into_iter().map(|e| folder.fold_expr(e)).collect(),
            span,
        },
        Expression::TupleLiteral { elements, span } => Expression::TupleLiteral {
            elements: elements.into_iter().map(|e| folder.fold_expr(e)).collect(),
            span,
        },
        Expression::EmptyList { span } => Expression::EmptyList { span },
        Expression::Index { left, index, span } => Expression::Index {
            left: Box::new(folder.fold_expr(*left)),
            index: Box::new(folder.fold_expr(*index)),
            span,
        },
        Expression::Hash { pairs, span } => Expression::Hash {
            pairs: pairs
                .into_iter()
                .map(|(k, v)| (folder.fold_expr(k), folder.fold_expr(v)))
                .collect(),
            span,
        },
        Expression::MemberAccess {
            object,
            member,
            span,
        } => Expression::MemberAccess {
            object: Box::new(folder.fold_expr(*object)),
            member: folder.fold_identifier(member),
            span,
        },
        Expression::TupleFieldAccess {
            object,
            index,
            span,
        } => Expression::TupleFieldAccess {
            object: Box::new(folder.fold_expr(*object)),
            index,
            span,
        },
        Expression::Match {
            scrutinee,
            arms,
            span,
        } => Expression::Match {
            scrutinee: Box::new(folder.fold_expr(*scrutinee)),
            arms: arms.into_iter().map(|a| folder.fold_match_arm(a)).collect(),
            span,
        },
        Expression::None { span } => Expression::None { span },
        Expression::Some { value, span } => Expression::Some {
            value: Box::new(folder.fold_expr(*value)),
            span,
        },
        Expression::Left { value, span } => Expression::Left {
            value: Box::new(folder.fold_expr(*value)),
            span,
        },
        Expression::Right { value, span } => Expression::Right {
            value: Box::new(folder.fold_expr(*value)),
            span,
        },
        Expression::Cons { head, tail, span } => Expression::Cons {
            head: Box::new(folder.fold_expr(*head)),
            tail: Box::new(folder.fold_expr(*tail)),
            span,
        },
    }
}

pub fn fold_pat<F: Folder + ?Sized>(folder: &mut F, pat: Pattern) -> Pattern {
    match pat {
        Pattern::Wildcard { span } => Pattern::Wildcard { span },
        Pattern::Literal { expression, span } => Pattern::Literal {
            expression: folder.fold_expr(expression),
            span,
        },
        Pattern::Identifier { name, span } => Pattern::Identifier {
            name: folder.fold_identifier(name),
            span,
        },
        Pattern::None { span } => Pattern::None { span },
        Pattern::Some { pattern, span } => Pattern::Some {
            pattern: Box::new(folder.fold_pat(*pattern)),
            span,
        },
        Pattern::Left { pattern, span } => Pattern::Left {
            pattern: Box::new(folder.fold_pat(*pattern)),
            span,
        },
        Pattern::Right { pattern, span } => Pattern::Right {
            pattern: Box::new(folder.fold_pat(*pattern)),
            span,
        },
        Pattern::Cons { head, tail, span } => Pattern::Cons {
            head: Box::new(folder.fold_pat(*head)),
            tail: Box::new(folder.fold_pat(*tail)),
            span,
        },
        Pattern::EmptyList { span } => Pattern::EmptyList { span },
        Pattern::Tuple { elements, span } => Pattern::Tuple {
            elements: elements.into_iter().map(|p| folder.fold_pat(p)).collect(),
            span,
        },
    }
}

pub fn fold_match_arm<F: Folder + ?Sized>(folder: &mut F, arm: MatchArm) -> MatchArm {
    let MatchArm {
        pattern,
        guard,
        body,
        span,
    } = arm;
    MatchArm {
        pattern: folder.fold_pat(pattern),
        guard: guard.map(|g| folder.fold_expr(g)),
        body: folder.fold_expr(body),
        span,
    }
}

pub fn fold_string_part<F: Folder + ?Sized>(folder: &mut F, part: StringPart) -> StringPart {
    match part {
        StringPart::Literal(s) => StringPart::Literal(s),
        StringPart::Interpolation(expr) => {
            StringPart::Interpolation(Box::new(folder.fold_expr(*expr)))
        }
    }
}
