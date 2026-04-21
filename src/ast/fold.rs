use crate::syntax::{
    Identifier,
    block::Block,
    expression::{Expression, HandleArm, MatchArm, Pattern, StringPart},
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
        Statement::Let {
            is_public,
            name,
            type_annotation,
            value,
            span,
        } => Statement::Let {
            is_public,
            name: folder.fold_identifier(name),
            type_annotation,
            value: folder.fold_expr(value),
            span,
        },
        Statement::LetDestructure {
            is_public,
            pattern,
            value,
            span,
        } => Statement::LetDestructure {
            is_public,
            pattern: folder.fold_pat(pattern),
            value: folder.fold_expr(value),
            span,
        },
        Statement::Return { value, span } => Statement::Return {
            value: value.map(|v| folder.fold_expr(v)),
            span,
        },
        Statement::Expression {
            expression,
            has_semicolon,
            span,
        } => Statement::Expression {
            expression: folder.fold_expr(expression),
            has_semicolon,
            span,
        },
        Statement::Function {
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
            intrinsic,
        } => Statement::Function {
            is_public,
            name: folder.fold_identifier(name),
            type_params,
            parameters: parameters
                .into_iter()
                .map(|p| folder.fold_identifier(p))
                .collect(),
            parameter_types,
            return_type,
            effects,
            body: folder.fold_block(body),
            span,
            fip,
            intrinsic,
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
        Statement::Import {
            name,
            alias,
            except,
            exposing,
            span,
        } => Statement::Import {
            name: folder.fold_identifier(name),
            alias: alias.map(|a| folder.fold_identifier(a)),
            except: except
                .into_iter()
                .map(|name| folder.fold_identifier(name))
                .collect(),
            exposing: match exposing {
                crate::syntax::statement::ImportExposing::Names(names) => {
                    crate::syntax::statement::ImportExposing::Names(
                        names
                            .into_iter()
                            .map(|n| folder.fold_identifier(n))
                            .collect(),
                    )
                }
                other => other,
            },
            span,
        },
        Statement::Data {
            is_public,
            name,
            type_params,
            variants,
            span,
            deriving,
        } => Statement::Data {
            is_public,
            name,
            type_params,
            variants,
            span,
            deriving,
        },
        Statement::EffectDecl { name, ops, span } => Statement::EffectDecl { name, ops, span },
        Statement::Class {
            is_public,
            name,
            type_params,
            superclasses,
            methods,
            span,
        } => Statement::Class {
            is_public,
            name,
            type_params,
            superclasses,
            methods,
            span,
        },
        Statement::Instance {
            is_public,
            class_name,
            type_args,
            context,
            methods,
            span,
        } => Statement::Instance {
            is_public,
            class_name,
            type_args,
            context,
            methods,
            span,
        },
    }
}

pub fn fold_expr<F: Folder + ?Sized>(folder: &mut F, expr: Expression) -> Expression {
    match expr {
        Expression::Identifier { name, span, id } => Expression::Identifier {
            name: folder.fold_identifier(name),
            span,
            id,
        },
        Expression::Integer { value, span, id } => Expression::Integer { value, span, id },
        Expression::Float { value, span, id } => Expression::Float { value, span, id },
        Expression::String { value, span, id } => Expression::String { value, span, id },
        Expression::InterpolatedString { parts, span, id } => Expression::InterpolatedString {
            parts: parts
                .into_iter()
                .map(|p| folder.fold_string_part(p))
                .collect(),
            span,
            id,
        },
        Expression::Boolean { value, span, id } => Expression::Boolean { value, span, id },
        Expression::Prefix {
            operator,
            right,
            span,
            id,
        } => Expression::Prefix {
            operator,
            right: Box::new(folder.fold_expr(*right)),
            span,
            id,
        },
        Expression::Infix {
            left,
            operator,
            right,
            span,
            id,
        } => Expression::Infix {
            left: Box::new(folder.fold_expr(*left)),
            operator,
            right: Box::new(folder.fold_expr(*right)),
            span,
            id,
        },
        Expression::If {
            condition,
            consequence,
            alternative,
            span,
            id,
        } => Expression::If {
            condition: Box::new(folder.fold_expr(*condition)),
            consequence: folder.fold_block(consequence),
            alternative: alternative.map(|a| folder.fold_block(a)),
            span,
            id,
        },
        Expression::DoBlock { block, span, id } => Expression::DoBlock {
            block: folder.fold_block(block),
            span,
            id,
        },
        Expression::Function {
            parameters,
            parameter_types,
            return_type,
            effects,
            body,
            span,
            id,
        } => Expression::Function {
            parameters: parameters
                .into_iter()
                .map(|p| folder.fold_identifier(p))
                .collect(),
            parameter_types,
            return_type,
            effects,
            body: folder.fold_block(body),
            span,
            id,
        },
        Expression::Call {
            function,
            arguments,
            span,
            id,
        } => Expression::Call {
            function: Box::new(folder.fold_expr(*function)),
            arguments: arguments.into_iter().map(|a| folder.fold_expr(a)).collect(),
            span,
            id,
        },
        Expression::ListLiteral { elements, span, id } => Expression::ListLiteral {
            elements: elements.into_iter().map(|e| folder.fold_expr(e)).collect(),
            span,
            id,
        },
        Expression::ArrayLiteral { elements, span, id } => Expression::ArrayLiteral {
            elements: elements.into_iter().map(|e| folder.fold_expr(e)).collect(),
            span,
            id,
        },
        Expression::TupleLiteral { elements, span, id } => Expression::TupleLiteral {
            elements: elements.into_iter().map(|e| folder.fold_expr(e)).collect(),
            span,
            id,
        },
        Expression::EmptyList { span, id } => Expression::EmptyList { span, id },
        Expression::Index {
            left,
            index,
            span,
            id,
        } => Expression::Index {
            left: Box::new(folder.fold_expr(*left)),
            index: Box::new(folder.fold_expr(*index)),
            span,
            id,
        },
        Expression::Hash { pairs, span, id } => Expression::Hash {
            pairs: pairs
                .into_iter()
                .map(|(k, v)| (folder.fold_expr(k), folder.fold_expr(v)))
                .collect(),
            span,
            id,
        },
        Expression::MemberAccess {
            object,
            member,
            span,
            id,
        } => Expression::MemberAccess {
            object: Box::new(folder.fold_expr(*object)),
            member: folder.fold_identifier(member),
            span,
            id,
        },
        Expression::TupleFieldAccess {
            object,
            index,
            span,
            id,
        } => Expression::TupleFieldAccess {
            object: Box::new(folder.fold_expr(*object)),
            index,
            span,
            id,
        },
        Expression::Match {
            scrutinee,
            arms,
            span,
            id,
        } => Expression::Match {
            scrutinee: Box::new(folder.fold_expr(*scrutinee)),
            arms: arms.into_iter().map(|a| folder.fold_match_arm(a)).collect(),
            span,
            id,
        },
        Expression::None { span, id } => Expression::None { span, id },
        Expression::Some { value, span, id } => Expression::Some {
            value: Box::new(folder.fold_expr(*value)),
            span,
            id,
        },
        Expression::Left { value, span, id } => Expression::Left {
            value: Box::new(folder.fold_expr(*value)),
            span,
            id,
        },
        Expression::Right { value, span, id } => Expression::Right {
            value: Box::new(folder.fold_expr(*value)),
            span,
            id,
        },
        Expression::Cons {
            head,
            tail,
            span,
            id,
        } => Expression::Cons {
            head: Box::new(folder.fold_expr(*head)),
            tail: Box::new(folder.fold_expr(*tail)),
            span,
            id,
        },
        Expression::Perform {
            effect,
            operation,
            args,
            span,
            id,
        } => Expression::Perform {
            effect: folder.fold_identifier(effect),
            operation: folder.fold_identifier(operation),
            args: args.into_iter().map(|a| folder.fold_expr(a)).collect(),
            span,
            id,
        },
        Expression::Handle {
            expr,
            effect,
            arms,
            span,
            id,
        } => Expression::Handle {
            expr: Box::new(folder.fold_expr(*expr)),
            effect: folder.fold_identifier(effect),
            arms: arms
                .into_iter()
                .map(|a| HandleArm {
                    operation_name: folder.fold_identifier(a.operation_name),
                    resume_param: folder.fold_identifier(a.resume_param),
                    params: a
                        .params
                        .into_iter()
                        .map(|p| folder.fold_identifier(p))
                        .collect(),
                    body: folder.fold_expr(a.body),
                    span: a.span,
                })
                .collect(),
            span,
            id,
        },
        Expression::NamedConstructor {
            name,
            fields,
            span,
            id,
        } => Expression::NamedConstructor {
            name: folder.fold_identifier(name),
            fields: fields
                .into_iter()
                .map(|f| crate::syntax::expression::NamedFieldInit {
                    name: f.name,
                    value: f.value.map(|v| Box::new(folder.fold_expr(*v))),
                    span: f.span,
                })
                .collect(),
            span,
            id,
        },
        Expression::Spread {
            base,
            overrides,
            span,
            id,
        } => Expression::Spread {
            base: Box::new(folder.fold_expr(*base)),
            overrides: overrides
                .into_iter()
                .map(|f| crate::syntax::expression::NamedFieldInit {
                    name: f.name,
                    value: f.value.map(|v| Box::new(folder.fold_expr(*v))),
                    span: f.span,
                })
                .collect(),
            span,
            id,
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
        Pattern::Constructor { name, fields, span } => Pattern::Constructor {
            name,
            fields: fields.into_iter().map(|p| folder.fold_pat(p)).collect(),
            span,
        },
        Pattern::NamedConstructor {
            name,
            fields,
            rest,
            span,
        } => Pattern::NamedConstructor {
            name,
            fields: fields
                .into_iter()
                .map(|f| crate::syntax::expression::NamedFieldPattern {
                    name: f.name,
                    pattern: f.pattern.map(|p| folder.fold_pat(p)),
                    span: f.span,
                })
                .collect(),
            rest,
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
