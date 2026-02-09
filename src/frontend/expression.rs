use std::fmt;

use crate::frontend::{Identifier, block::Block, position::Span};

#[derive(Debug, Clone)]
pub enum StringPart {
    Literal(String),
    Interpolation(Box<Expression>),
}

#[derive(Debug, Clone)]
pub enum Pattern {
    Wildcard { span: Span },
    Literal { expression: Expression, span: Span },
    Identifier { name: Identifier, span: Span },
    None { span: Span },
    Some { pattern: Box<Pattern>, span: Span },
    Left { pattern: Box<Pattern>, span: Span },
    Right { pattern: Box<Pattern>, span: Span },
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
