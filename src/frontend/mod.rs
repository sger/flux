#![allow(dead_code)]

pub mod block;
pub mod diagnostic;
pub mod expression;
pub mod formatter;
pub mod lexer;
pub mod linter;
pub mod module_graph;
pub mod parser;
pub mod position;
pub mod precedence;
pub mod program;
pub mod statement;
pub mod token;
pub mod token_type;

pub type Identifier = String;
