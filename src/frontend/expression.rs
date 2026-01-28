use std::fmt;

use crate::frontend::{Identifier, block::Block};

#[derive(Debug, Clone)]
pub enum Pattern {
    Wildcard,
    Literal(Expression),
    Identifier(Identifier),
    None,
    Some(Box<Pattern>),
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Expression,
}

#[derive(Debug, Clone)]
pub enum Expression {
    Identifier(Identifier),
    Integer(i64),
    Float(f64),
    String(String),
    Boolean(bool),
    Prefix {
        operator: String,
        right: Box<Expression>,
    },
    Infix {
        left: Box<Expression>,
        operator: String,
        right: Box<Expression>,
    },
    If {
        condition: Box<Expression>,
        consequence: Block,
        alternative: Option<Block>,
    },
    Function {
        parameters: Vec<Identifier>,
        body: Block,
    },
    Call {
        function: Box<Expression>,
        arguments: Vec<Expression>,
    },
    Array {
        elements: Vec<Expression>,
    },
    Index {
        left: Box<Expression>,
        index: Box<Expression>,
    },
    Hash {
        pairs: Vec<(Expression, Expression)>,
    },
    MemberAccess {
        object: Box<Expression>,
        member: Identifier,
    },
    Match {
        scrutinee: Box<Expression>,
        arms: Vec<MatchArm>,
    },
    None,
    Some {
        value: Box<Expression>,
    },
}

impl fmt::Display for Expression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expression::Identifier(name) => write!(f, "{}", name),
            Expression::Integer(value) => write!(f, "{}", value),
            Expression::Float(value) => write!(f, "{}", value),
            Expression::String(value) => write!(f, "\"{}\"", value),
            Expression::Boolean(value) => write!(f, "{}", value),
            Expression::Prefix { operator, right } => {
                write!(f, "({}{})", operator, right)
            }
            Expression::Infix {
                left,
                operator,
                right,
            } => {
                write!(f, "({} {} {})", left, operator, right)
            }
            Expression::If {
                condition,
                consequence,
                alternative,
            } => {
                write!(f, "if {} {}", condition, consequence)?;
                if let Some(alt) = alternative {
                    write!(f, " else {}", alt)?;
                }
                Ok(())
            }
            Expression::Function { parameters, body } => {
                write!(f, "fun({}) {}", parameters.join(", "), body)
            }
            Expression::Call {
                function,
                arguments,
            } => {
                let args: Vec<String> = arguments.iter().map(|a| a.to_string()).collect();
                write!(f, "{}({})", function, args.join(", "))
            }
            Expression::Array { elements } => {
                let elems: Vec<String> = elements.iter().map(|e| e.to_string()).collect();
                write!(f, "[{}]", elems.join(", "))
            }
            Expression::Index { left, index } => {
                write!(f, "({}[{}])", left, index)
            }
            Expression::Hash { pairs } => {
                let items: Vec<String> =
                    pairs.iter().map(|(k, v)| format!("{}: {}", k, v)).collect();
                write!(f, "{{{}}}", items.join(", "))
            }
            Expression::MemberAccess { object, member } => {
                write!(f, "{}.{}", object, member)
            }
            Expression::Match { scrutinee, arms } => {
                write!(f, "match {} {{", scrutinee)?;
                for arm in arms {
                    write!(f, " {} -> {};", arm.pattern, arm.body)?;
                }
                write!(f, " }}")
            }
            Expression::None => write!(f, "None"),
            Expression::Some { value } => write!(f, "Some({})", value),
        }
    }
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Pattern::Wildcard => write!(f, "_"),
            Pattern::Literal(expr) => write!(f, "{}", expr),
            Pattern::Identifier(ident) => write!(f, "{}", ident),
            Pattern::None => write!(f, "None"),
            Pattern::Some(pattern) => write!(f, "Some({})", pattern),
        }
    }
}

impl fmt::Display for MatchArm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} -> {}", self.pattern, self.body)
    }
}
