use std::fmt;

use crate::{
    diagnostics::position::Span,
    syntax::{Identifier, block::Block, interner::Interner},
};

#[derive(Debug, Clone)]
pub enum StringPart {
    Literal(String),
    Interpolation(Box<Expression>),
}

#[derive(Debug, Clone)]
pub enum Pattern {
    Wildcard {
        span: Span,
    },
    Literal {
        expression: Expression,
        span: Span,
    },
    Identifier {
        name: Identifier,
        span: Span,
    },
    None {
        span: Span,
    },
    Some {
        pattern: Box<Pattern>,
        span: Span,
    },
    Left {
        pattern: Box<Pattern>,
        span: Span,
    },
    Right {
        pattern: Box<Pattern>,
        span: Span,
    },
    Cons {
        head: Box<Pattern>,
        tail: Box<Pattern>,
        span: Span,
    },
    EmptyList {
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<Expression>,
    pub body: Expression,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Expression {
    Identifier {
        name: Identifier,
        span: Span,
    },
    Integer {
        value: i64,
        span: Span,
    },
    Float {
        value: f64,
        span: Span,
    },
    String {
        value: String,
        span: Span,
    },
    InterpolatedString {
        parts: Vec<StringPart>,
        span: Span,
    },
    Boolean {
        value: bool,
        span: Span,
    },
    Prefix {
        operator: String,
        right: Box<Expression>,
        span: Span,
    },
    Infix {
        left: Box<Expression>,
        operator: String,
        right: Box<Expression>,
        span: Span,
    },
    If {
        condition: Box<Expression>,
        consequence: Block,
        alternative: Option<Block>,
        span: Span,
    },
    Function {
        parameters: Vec<Identifier>,
        body: Block,
        span: Span,
    },
    Call {
        function: Box<Expression>,
        arguments: Vec<Expression>,
        span: Span,
    },
    Array {
        elements: Vec<Expression>,
        span: Span,
    },
    Index {
        left: Box<Expression>,
        index: Box<Expression>,
        span: Span,
    },
    Hash {
        pairs: Vec<(Expression, Expression)>,
        span: Span,
    },
    MemberAccess {
        object: Box<Expression>,
        member: Identifier,
        span: Span,
    },
    Match {
        scrutinee: Box<Expression>,
        arms: Vec<MatchArm>,
        span: Span,
    },
    None {
        span: Span,
    },
    Some {
        value: Box<Expression>,
        span: Span,
    },
    // Either type expressions
    Left {
        value: Box<Expression>,
        span: Span,
    },
    Right {
        value: Box<Expression>,
        span: Span,
    },
    Cons {
        head: Box<Expression>,
        tail: Box<Expression>,
        span: Span,
    },
}

impl fmt::Display for Expression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expression::Identifier { name, .. } => write!(f, "{}", name),
            Expression::Integer { value, .. } => write!(f, "{}", value),
            Expression::Float { value, .. } => write!(f, "{}", value),
            Expression::String { value, .. } => write!(f, "\"{}\"", value),
            Expression::InterpolatedString { parts, .. } => {
                write!(f, "\"")?;
                for part in parts {
                    match part {
                        StringPart::Literal(s) => write!(f, "{}", s)?,
                        StringPart::Interpolation(expr) => write!(f, "#{{{}}}", expr)?,
                    }
                }
                write!(f, "\"")
            }
            Expression::Boolean { value, .. } => write!(f, "{}", value),
            Expression::Prefix {
                operator, right, ..
            } => {
                write!(f, "({}{})", operator, right)
            }
            Expression::Infix {
                left,
                operator,
                right,
                ..
            } => {
                write!(f, "({} {} {})", left, operator, right)
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                write!(f, "if {} {}", condition, consequence)?;
                if let Some(alt) = alternative {
                    write!(f, " else {}", alt)?;
                }
                Ok(())
            }
            Expression::Function {
                parameters, body, ..
            } => {
                let params: Vec<String> = parameters.iter().map(|p| p.to_string()).collect();
                write!(f, "fun({}) {}", params.join(", "), body)
            }
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                let args: Vec<String> = arguments.iter().map(|a| a.to_string()).collect();
                write!(f, "{}({})", function, args.join(", "))
            }
            Expression::Array { elements, .. } => {
                let elems: Vec<String> = elements.iter().map(|e| e.to_string()).collect();
                write!(f, "[{}]", elems.join(", "))
            }
            Expression::Index { left, index, .. } => {
                write!(f, "({}[{}])", left, index)
            }
            Expression::Hash { pairs, .. } => {
                let items: Vec<String> =
                    pairs.iter().map(|(k, v)| format!("{}: {}", k, v)).collect();
                write!(f, "{{{}}}", items.join(", "))
            }
            Expression::MemberAccess { object, member, .. } => {
                write!(f, "{}.{}", object, member)
            }
            Expression::Match {
                scrutinee, arms, ..
            } => {
                write!(f, "match {} {{", scrutinee)?;
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        write!(f, " {} if {} -> {};", arm.pattern, guard, arm.body)?;
                    } else {
                        write!(f, " {} -> {};", arm.pattern, arm.body)?;
                    }
                }
                write!(f, " }}")
            }
            Expression::None { .. } => write!(f, "None"),
            Expression::Some { value, .. } => write!(f, "Some({})", value),
            Expression::Left { value, .. } => write!(f, "Left({})", value),
            Expression::Right { value, .. } => write!(f, "Right({})", value),
            Expression::Cons { head, tail, .. } => write!(f, "[{} | {}]", head, tail),
        }
    }
}

impl Expression {
    pub fn span(&self) -> Span {
        match self {
            Expression::Identifier { span, .. }
            | Expression::Integer { span, .. }
            | Expression::Float { span, .. }
            | Expression::String { span, .. }
            | Expression::InterpolatedString { span, .. }
            | Expression::Boolean { span, .. }
            | Expression::Prefix { span, .. }
            | Expression::Infix { span, .. }
            | Expression::If { span, .. }
            | Expression::Function { span, .. }
            | Expression::Call { span, .. }
            | Expression::Array { span, .. }
            | Expression::Index { span, .. }
            | Expression::Hash { span, .. }
            | Expression::MemberAccess { span, .. }
            | Expression::Match { span, .. }
            | Expression::None { span, .. }
            | Expression::Some { span, .. } => *span,
            // Either type expressions
            Expression::Left { span, .. } | Expression::Right { span, .. } => *span,
            Expression::Cons { span, .. } => *span,
        }
    }
}

impl Expression {
    /// Formats this expression using the interner to resolve identifier names.
    pub fn display_with(&self, interner: &Interner) -> String {
        match self {
            Expression::Identifier { name, .. } => interner.resolve(*name).to_string(),
            Expression::Integer { value, .. } => format!("{}", value),
            Expression::Float { value, .. } => format!("{}", value),
            Expression::String { value, .. } => format!("\"{}\"", value),
            Expression::InterpolatedString { parts, .. } => {
                let mut out = String::from("\"");
                for part in parts {
                    match part {
                        StringPart::Literal(s) => out.push_str(s),
                        StringPart::Interpolation(expr) => {
                            out.push_str(&format!("#{{{}}}", expr.display_with(interner)));
                        }
                    }
                }
                out.push('"');
                out
            }
            Expression::Boolean { value, .. } => format!("{}", value),
            Expression::Prefix {
                operator, right, ..
            } => format!("({}{})", operator, right.display_with(interner)),
            Expression::Infix {
                left,
                operator,
                right,
                ..
            } => format!(
                "({} {} {})",
                left.display_with(interner),
                operator,
                right.display_with(interner)
            ),
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                let mut out = format!("if {} {}", condition.display_with(interner), consequence);
                if let Some(alt) = alternative {
                    out.push_str(&format!(" else {}", alt));
                }
                out
            }
            Expression::Function {
                parameters, body, ..
            } => {
                let params: Vec<&str> = parameters.iter().map(|p| interner.resolve(*p)).collect();
                format!("fun({}) {}", params.join(", "), body)
            }
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                let args: Vec<String> =
                    arguments.iter().map(|a| a.display_with(interner)).collect();
                format!("{}({})", function.display_with(interner), args.join(", "))
            }
            Expression::Array { elements, .. } => {
                let elems: Vec<String> =
                    elements.iter().map(|e| e.display_with(interner)).collect();
                format!("[{}]", elems.join(", "))
            }
            Expression::Index { left, index, .. } => {
                format!(
                    "({}[{}])",
                    left.display_with(interner),
                    index.display_with(interner)
                )
            }
            Expression::Hash { pairs, .. } => {
                let items: Vec<String> = pairs
                    .iter()
                    .map(|(k, v)| {
                        format!("{}: {}", k.display_with(interner), v.display_with(interner))
                    })
                    .collect();
                format!("{{{}}}", items.join(", "))
            }
            Expression::MemberAccess { object, member, .. } => {
                format!(
                    "{}.{}",
                    object.display_with(interner),
                    interner.resolve(*member)
                )
            }
            Expression::Match {
                scrutinee, arms, ..
            } => {
                let mut out = format!("match {} {{", scrutinee.display_with(interner));
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        out.push_str(&format!(
                            " {} if {} -> {};",
                            arm.pattern.display_with(interner),
                            guard.display_with(interner),
                            arm.body.display_with(interner)
                        ));
                    } else {
                        out.push_str(&format!(
                            " {} -> {};",
                            arm.pattern.display_with(interner),
                            arm.body.display_with(interner)
                        ));
                    }
                }
                out.push_str(" }}");
                out
            }
            Expression::None { .. } => "None".to_string(),
            Expression::Some { value, .. } => {
                format!("Some({})", value.display_with(interner))
            }
            Expression::Left { value, .. } => {
                format!("Left({})", value.display_with(interner))
            }
            Expression::Right { value, .. } => {
                format!("Right({})", value.display_with(interner))
            }
            Expression::Cons { head, tail, .. } => {
                format!(
                    "[{} | {}]",
                    head.display_with(interner),
                    tail.display_with(interner)
                )
            }
        }
    }
}

impl Pattern {
    /// Formats this pattern using the interner to resolve identifier names.
    pub fn display_with(&self, interner: &Interner) -> String {
        match self {
            Pattern::Wildcard { .. } => "_".to_string(),
            Pattern::Literal { expression, .. } => expression.display_with(interner),
            Pattern::Identifier { name, .. } => interner.resolve(*name).to_string(),
            Pattern::None { .. } => "None".to_string(),
            Pattern::Some { pattern, .. } => {
                format!("Some({})", pattern.display_with(interner))
            }
            Pattern::Left { pattern, .. } => {
                format!("Left({})", pattern.display_with(interner))
            }
            Pattern::Right { pattern, .. } => {
                format!("Right({})", pattern.display_with(interner))
            }
            Pattern::Cons { head, tail, .. } => {
                format!(
                    "[{} | {}]",
                    head.display_with(interner),
                    tail.display_with(interner)
                )
            }
            Pattern::EmptyList { .. } => "[]".to_string(),
        }
    }
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Pattern::Wildcard { .. } => write!(f, "_"),
            Pattern::Literal { expression, .. } => write!(f, "{}", expression),
            Pattern::Identifier { name, .. } => write!(f, "{}", name),
            Pattern::None { .. } => write!(f, "None"),
            Pattern::Some { pattern, .. } => write!(f, "Some({})", pattern),
            Pattern::Left { pattern, .. } => write!(f, "Left({})", pattern),
            Pattern::Right { pattern, .. } => write!(f, "Right({})", pattern),
            Pattern::Cons { head, tail, .. } => write!(f, "[{} | {}]", head, tail),
            Pattern::EmptyList { .. } => write!(f, "[]"),
        }
    }
}

impl Pattern {
    pub fn span(&self) -> Span {
        match self {
            Pattern::Wildcard { span }
            | Pattern::Literal { span, .. }
            | Pattern::Identifier { span, .. }
            | Pattern::None { span }
            | Pattern::Some { span, .. }
            | Pattern::Left { span, .. }
            | Pattern::Right { span, .. } => *span,
            Pattern::Cons { span, .. } | Pattern::EmptyList { span, .. } => *span,
        }
    }
}

impl fmt::Display for MatchArm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(guard) = &self.guard {
            write!(f, "{} if {} -> {}", self.pattern, guard, self.body)
        } else {
            write!(f, "{} -> {}", self.pattern, self.body)
        }
    }
}

impl MatchArm {
    pub fn span(&self) -> Span {
        self.span
    }
}
