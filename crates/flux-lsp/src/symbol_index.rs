use std::collections::HashMap;

use flux::diagnostics::position::{Position as FluxPosition, Span as FluxSpan};
use flux::syntax::Identifier;
use flux::syntax::interner::Interner;
use flux::syntax::program::Program;
use flux::syntax::statement::Statement;

use crate::locator::decl_name_start;

/// Index of top-level definitions in one file.
///
/// Maps the identifier symbol to a definition entry carrying both span
/// extents the LSP cares about — `full_span` for the whole declaration
/// (peek-definition's outer highlight) and `focus_span` for the
/// identifier text alone (the inner highlight). Modeled after GHC's
/// `NameAnn`/`EpAnn` split (`compiler/GHC/Parser/Annotation.hs:581-635`):
/// the outer anchor and the inner identifier span are first-class and
/// distinct.
pub struct SymbolIndex {
    by_id: HashMap<Identifier, Entry>,
    by_name: HashMap<String, Entry>,
}

#[derive(Clone)]
pub struct Entry {
    pub name: String,
    /// Whole declaration extent: signature + body for `fn`, the entire
    /// `let x = expr` for `let`, the whole `data ... { ... }` block, etc.
    /// LSP `target_range` uses this so peek-view shows the full context.
    pub full_span: FluxSpan,
    /// Identifier-only sub-span — the position of just `foo` in
    /// `fn foo(x, y) { ... }`. LSP `target_selection_range` uses this so
    /// peek-view highlights only the name when the user lands.
    pub focus_span: FluxSpan,
}

impl SymbolIndex {
    pub fn build(program: &Program, interner: &Interner) -> Self {
        let mut by_id = HashMap::new();
        let mut by_name = HashMap::new();
        for stmt in &program.statements {
            if let Some((sym, full_span, focus_span)) = top_level_definition(stmt, interner)
                && let Some(name) = interner.try_resolve(sym)
                && !name.is_empty()
            {
                let entry = Entry {
                    name: name.to_string(),
                    full_span,
                    focus_span,
                };
                by_id.insert(sym, entry.clone());
                by_name.insert(name.to_string(), entry);
            }
        }
        Self { by_id, by_name }
    }

    pub fn lookup(&self, name: &str) -> Option<&Entry> {
        self.by_name.get(name)
    }

    pub fn lookup_id(&self, id: Identifier) -> Option<&Entry> {
        self.by_id.get(&id)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(|s| s.as_str())
    }
}

/// Top-level declarations whose name we index. Returns
/// `(identifier_sym, full_span, focus_span)`. `focus_span` is synthesized
/// from the statement start by skipping the declaration keyword (and
/// `public ` prefix when present) — same calculation the locator uses
/// when emitting `DataName` / `DeclName` nodes, lifted from
/// `locator::decl_name_start`.
fn top_level_definition(
    stmt: &Statement,
    interner: &Interner,
) -> Option<(Identifier, FluxSpan, FluxSpan)> {
    let (sym, full_span, focus_start) = match stmt {
        Statement::Let {
            is_public,
            name,
            span,
            ..
        } => (*name, *span, decl_name_start(span.start, *is_public, "let")),
        Statement::Function {
            is_public,
            name,
            span,
            ..
        } => (*name, *span, decl_name_start(span.start, *is_public, "fn")),
        Statement::Module { name, span, .. } => {
            // `module` has no `public` prefix in current grammar.
            (*name, *span, decl_name_start(span.start, false, "module"))
        }
        Statement::Data {
            is_public,
            name,
            span,
            ..
        } => (
            *name,
            *span,
            decl_name_start(span.start, *is_public, "data"),
        ),
        Statement::EffectDecl { name, span, .. } => {
            (*name, *span, decl_name_start(span.start, false, "effect"))
        }
        Statement::EffectAlias { name, span, .. } => {
            (*name, *span, decl_name_start(span.start, false, "alias"))
        }
        Statement::Class {
            name,
            span,
            name_span,
            ..
        } => (*name, *span, name_span.start),
        // `Statement::TypeAlias` comes from the `alias` keyword
        // (`alias Foo = Int`), not `type` — so skip 5 chars, not 4.
        Statement::TypeAlias(alias) => (
            alias.name,
            alias.span,
            decl_name_start(alias.span.start, false, "alias"),
        ),
        _ => return None,
    };
    let name_len = interner.try_resolve(sym).map(str::len).unwrap_or(0);
    let focus_span = FluxSpan {
        start: focus_start,
        end: FluxPosition {
            line: focus_start.line,
            column: focus_start.column + name_len,
        },
    };
    Some((sym, full_span, focus_span))
}

impl SymbolIndex {
    /// Build an extended index that also maps effect operation names and data
    /// variant names to their declaration spans. Used for cross-declaration
    /// goto-definition (e.g. F12 on `perform Emit(x)` → jumps to the `Emit`
    /// op inside its `effect` block).
    ///
    /// For ops and variants the parser already records a name-only span on
    /// the `EffectOp` / `DataVariant` nodes, so `focus_span` and `full_span`
    /// coincide — the locator's `EffectOpName` / `DataVariantName` emission
    /// uses the same span.
    pub fn build_extended(program: &Program, interner: &Interner) -> Self {
        let mut idx = Self::build(program, interner);
        for stmt in &program.statements {
            match stmt {
                Statement::EffectDecl { ops, .. } => {
                    for op in ops {
                        if let Some(name) = interner.try_resolve(op.name) {
                            let entry = Entry {
                                name: name.to_string(),
                                full_span: op.span,
                                focus_span: op.span,
                            };
                            idx.by_id.insert(op.name, entry.clone());
                            idx.by_name.insert(name.to_string(), entry);
                        }
                    }
                }
                Statement::Data { variants, .. } => {
                    for v in variants {
                        if let Some(name) = interner.try_resolve(v.name) {
                            let entry = Entry {
                                name: name.to_string(),
                                full_span: v.span,
                                focus_span: v.span,
                            };
                            idx.by_id.insert(v.name, entry.clone());
                            idx.by_name.insert(name.to_string(), entry);
                        }
                    }
                }
                _ => {}
            }
        }
        idx
    }

    /// Like [`SymbolIndex::build_extended`] but also indexes declarations
    /// nested inside top-level `module { ... }` blocks. A user module file
    /// wraps its `public fn`s in a `module Name { ... }` block, so its
    /// members are not top-level — cross-file goto-definition needs them.
    pub fn build_for_module_file(program: &Program, interner: &Interner) -> Self {
        let mut idx = Self::build_extended(program, interner);
        for stmt in &program.statements {
            if let Statement::Module { body, .. } = stmt {
                for inner in &body.statements {
                    if let Some((sym, full_span, focus_span)) =
                        top_level_definition(inner, interner)
                        && let Some(name) = interner.try_resolve(sym)
                        && !name.is_empty()
                    {
                        let entry = Entry {
                            name: name.to_string(),
                            full_span,
                            focus_span,
                        };
                        idx.by_id.insert(sym, entry.clone());
                        idx.by_name.insert(name.to_string(), entry);
                    }
                }
            }
        }
        idx
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flux::syntax::lexer::Lexer;
    use flux::syntax::parser::Parser;

    fn parse(src: &str) -> (Program, Interner) {
        let mut parser = Parser::new(Lexer::new(src.to_string()));
        let program = parser.parse_program();
        (program, parser.take_interner())
    }

    #[test]
    fn alias_focus_span_covers_the_name() {
        // `alias Foo = Int` — the name starts at column 6 (`alias ` = 6 chars),
        // not 5. Regression for the keyword length used to synthesize the span.
        let (program, interner) = parse("alias Foo = Int\n");
        let idx = SymbolIndex::build(&program, &interner);
        let entry = idx.lookup("Foo").expect("alias indexed");
        assert_eq!(entry.focus_span.start.column, 6);
        assert_eq!(entry.focus_span.end.column, 9);
    }
}
