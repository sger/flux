use std::collections::HashMap;

use crate::ast::fold::Folder;
use crate::syntax::{Identifier, expression::Expression, program::Program, symbol::Symbol};

/// Systematic identifier renaming.
///
/// Replaces identifiers according to a `Symbol → Symbol` map. Applies to all
/// identifier positions: let bindings, function names, parameters, variable
/// references, member names, and pattern bindings.
struct Renamer {
    map: HashMap<Symbol, Symbol>,
}

impl Folder for Renamer {
    fn fold_identifier(&mut self, ident: Identifier) -> Identifier {
        self.map.get(&ident).copied().unwrap_or(ident)
    }
}

/// Rename identifiers in a program according to the given map.
pub fn rename(program: Program, map: HashMap<Symbol, Symbol>) -> Program {
    let mut renamer = Renamer { map };
    renamer.fold_program(program)
}

/// Rename identifiers in a single expression.
pub fn rename_expr(expr: Expression, map: HashMap<Symbol, Symbol>) -> Expression {
    let mut renamer = Renamer { map };
    renamer.fold_expr(expr)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::ast::{rename, rename_expr};
    use crate::syntax::{expression::Expression, interner::Interner, lexer::Lexer, parser::Parser};

    fn parse_program(source: &str) -> (crate::syntax::program::Program, Interner) {
        let lexer = Lexer::new(source);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );
        let interner = parser.take_interner();
        (program, interner)
    }

    #[test]
    fn renames_identifiers_in_program() {
        let (program, mut interner) = parse_program("let x = x;");
        let x = interner.intern("x");
        let y = interner.intern("y");
        let mut map = HashMap::new();
        map.insert(x, y);

        let renamed = rename(program, map);
        assert_eq!(renamed.display_with(&interner), "let y = y;");
    }

    #[test]
    fn renames_identifiers_in_expression() {
        let mut interner = Interner::new();
        let x = interner.intern("x");
        let y = interner.intern("y");

        let expr = Expression::Identifier {
            name: x,
            span: Default::default(),
        };
        let mut map = HashMap::new();
        map.insert(x, y);

        let renamed = rename_expr(expr, map);
        match renamed {
            Expression::Identifier { name, .. } => assert_eq!(name, y),
            other => panic!("expected identifier, got {:?}", other),
        }
    }
}
