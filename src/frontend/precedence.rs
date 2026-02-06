use std::sync::LazyLock;

use crate::frontend::token_type::TokenType;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Precedence {
    Lowest,
    Pipe,        // |> lowest precedence for chaining
    LogicalOr,   // || lower precedence than &&
    LogicalAnd,  // && higher precedence than ||
    Equals,      // ==, !=
    LessGreater, // <, >, <=, >=
    Sum,         // +, -
    Product,     // *, /, %
    Prefix,      // -x, !x
    Call,        // fn(x)
    Index,       // array[index]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assoc {
    Left,
    Right,
    Nonassoc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fixity {
    Prefix,
    Infix,
    Postfix,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpInfo {
    pub token: TokenType,
    pub precedence: Precedence,
    pub associativity: Assoc,
    pub fixity: Fixity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InfixInfo {
    pub precedence: Precedence,
    pub associativity: Assoc,
    pub fixity: Fixity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrefixInfo {
    pub precedence: Precedence,
    pub associativity: Assoc,
}

// Single source of truth for operator precedence + associativity.
pub const OPERATOR_TABLE: &[OpInfo] = &[
    // Infix operators
    OpInfo {
        token: TokenType::Pipe,
        precedence: Precedence::Pipe,
        associativity: Assoc::Left,
        fixity: Fixity::Infix,
    },
    OpInfo {
        token: TokenType::Or,
        precedence: Precedence::LogicalOr,
        associativity: Assoc::Left,
        fixity: Fixity::Infix,
    },
    OpInfo {
        token: TokenType::And,
        precedence: Precedence::LogicalAnd,
        associativity: Assoc::Left,
        fixity: Fixity::Infix,
    },
    OpInfo {
        token: TokenType::Eq,
        precedence: Precedence::Equals,
        associativity: Assoc::Left,
        fixity: Fixity::Infix,
    },
    OpInfo {
        token: TokenType::NotEq,
        precedence: Precedence::Equals,
        associativity: Assoc::Left,
        fixity: Fixity::Infix,
    },
    OpInfo {
        token: TokenType::Lt,
        precedence: Precedence::LessGreater,
        associativity: Assoc::Left,
        fixity: Fixity::Infix,
    },
    OpInfo {
        token: TokenType::Gt,
        precedence: Precedence::LessGreater,
        associativity: Assoc::Left,
        fixity: Fixity::Infix,
    },
    OpInfo {
        token: TokenType::Lte,
        precedence: Precedence::LessGreater,
        associativity: Assoc::Left,
        fixity: Fixity::Infix,
    },
    OpInfo {
        token: TokenType::Gte,
        precedence: Precedence::LessGreater,
        associativity: Assoc::Left,
        fixity: Fixity::Infix,
    },
    OpInfo {
        token: TokenType::Plus,
        precedence: Precedence::Sum,
        associativity: Assoc::Left,
        fixity: Fixity::Infix,
    },
    OpInfo {
        token: TokenType::Minus,
        precedence: Precedence::Sum,
        associativity: Assoc::Left,
        fixity: Fixity::Infix,
    },
    OpInfo {
        token: TokenType::Asterisk,
        precedence: Precedence::Product,
        associativity: Assoc::Left,
        fixity: Fixity::Infix,
    },
    OpInfo {
        token: TokenType::Slash,
        precedence: Precedence::Product,
        associativity: Assoc::Left,
        fixity: Fixity::Infix,
    },
    OpInfo {
        token: TokenType::Percent,
        precedence: Precedence::Product,
        associativity: Assoc::Left,
        fixity: Fixity::Infix,
    },
    // Postfix operators handled by Pratt infix dispatch
    OpInfo {
        token: TokenType::LParen,
        precedence: Precedence::Call,
        associativity: Assoc::Left,
        fixity: Fixity::Postfix,
    },
    OpInfo {
        token: TokenType::LBracket,
        precedence: Precedence::Index,
        associativity: Assoc::Left,
        fixity: Fixity::Postfix,
    },
    OpInfo {
        token: TokenType::Dot,
        precedence: Precedence::Index,
        associativity: Assoc::Left,
        fixity: Fixity::Postfix,
    },
    // Prefix operators
    OpInfo {
        token: TokenType::Bang,
        precedence: Precedence::Prefix,
        associativity: Assoc::Right,
        fixity: Fixity::Prefix,
    },
    OpInfo {
        token: TokenType::Minus,
        precedence: Precedence::Prefix,
        associativity: Assoc::Right,
        fixity: Fixity::Prefix,
    },
];

pub static INFIX_TABLE: LazyLock<[Option<InfixInfo>; TokenType::COUNT]> = LazyLock::new(|| {
    let mut table = [None; TokenType::COUNT];
    for info in OPERATOR_TABLE {
        match info.fixity {
            Fixity::Infix | Fixity::Postfix => {
                let index = info.token.as_usize();
                debug_assert!(
                    table[index].is_none(),
                    "duplicate infix/postfix operator table entry for token {:?}",
                    info.token
                );
                table[index] = Some(InfixInfo {
                    precedence: info.precedence,
                    associativity: info.associativity,
                    fixity: info.fixity,
                });
            }
            Fixity::Prefix => {}
        }
    }
    table
});

pub static PREFIX_TABLE: LazyLock<[Option<PrefixInfo>; TokenType::COUNT]> = LazyLock::new(|| {
    let mut table = [None; TokenType::COUNT];
    for info in OPERATOR_TABLE {
        if info.fixity == Fixity::Prefix {
            let index = info.token.as_usize();
            debug_assert!(
                table[index].is_none(),
                "duplicate prefix operator table entry for token {:?}",
                info.token
            );
            table[index] = Some(PrefixInfo {
                precedence: info.precedence,
                associativity: info.associativity,
            });
        }
    }
    table
});

pub fn infix_op(token_type: &TokenType) -> Option<InfixInfo> {
    INFIX_TABLE[token_type.as_usize()]
}

pub fn prefix_op(token_type: &TokenType) -> Option<PrefixInfo> {
    PREFIX_TABLE[token_type.as_usize()]
}

pub fn precedence_of(token_type: &TokenType) -> Option<Precedence> {
    infix_op(token_type).map(|op| op.precedence)
}

pub fn associativity_of(token_type: &TokenType) -> Option<Assoc> {
    infix_op(token_type).map(|op| op.associativity)
}

fn precedence_below(precedence: &Precedence) -> Precedence {
    match precedence {
        Precedence::Lowest => Precedence::Lowest,
        Precedence::Pipe => Precedence::Lowest,
        Precedence::LogicalOr => Precedence::Pipe,
        Precedence::LogicalAnd => Precedence::LogicalOr,
        Precedence::Equals => Precedence::LogicalAnd,
        Precedence::LessGreater => Precedence::Equals,
        Precedence::Sum => Precedence::LessGreater,
        Precedence::Product => Precedence::Sum,
        Precedence::Prefix => Precedence::Product,
        Precedence::Call => Precedence::Prefix,
        Precedence::Index => Precedence::Call,
    }
}

pub fn rhs_precedence_for_infix(token_type: &TokenType) -> Option<Precedence> {
    let op = infix_op(token_type)?;
    Some(match op.associativity {
        Assoc::Left | Assoc::Nonassoc => op.precedence,
        Assoc::Right => precedence_below(&op.precedence),
    })
}

pub fn precedence_or_lowest(token_type: &TokenType) -> Precedence {
    precedence_of(token_type).unwrap_or(Precedence::Lowest)
}

pub fn associativity_or_left(token_type: &TokenType) -> Assoc {
    associativity_of(token_type).unwrap_or(Assoc::Left)
}

pub fn token_precedence(token_type: &TokenType) -> Precedence {
    precedence_or_lowest(token_type)
}
