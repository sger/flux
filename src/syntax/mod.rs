#![allow(dead_code)]

pub mod block;
pub mod byte_span;
pub mod data_variant;
pub mod effect_expr;
pub mod effect_ops;
pub mod entry;
pub mod expression;
pub mod formatter;
pub mod interner;
pub mod lexeme;
pub mod lexer;
pub mod linter;
pub mod module_graph;
pub mod parser;
pub mod pattern_validate;
pub mod precedence;
pub mod program;
pub mod statement;
pub mod symbol;
pub mod token;
pub mod token_type;
pub mod type_class;
pub mod type_expr;

pub type Identifier = symbol::Symbol;
