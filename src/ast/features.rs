//! The language features a program uses, for the support census (proposal
//! 0190 §16). `flux features <file.flx>…` prints one line per file: the path,
//! a tab, and its tags joined by commas.
//!
//! Tags come from a walk of the parsed program, before any desugaring, so they
//! name what the author wrote. A `// feature: a, -b` header line adds `a` and
//! removes `b`, for what the walk cannot see or gets wrong.

use std::collections::BTreeSet;

use crate::ast::visit::{self, Visitor};
use crate::source::interner::Interner;
use crate::syntax::{
    Identifier,
    effect_expr::EffectExpr,
    expression::{Expression, MatchArm, Pattern},
    lexer::Lexer,
    parser::Parser,
    statement::Statement,
};

/// Every tag the walk can produce, so callers can list empty columns too.
pub const TAGS: &[&str] = &[
    "adts",
    "annotations",
    "arrays",
    "associated-types",
    "async",
    "classes",
    "closures",
    "deriving",
    "destructuring",
    "do-blocks",
    "effect-annotations",
    "effect-rows",
    "either",
    "fip",
    "generics",
    "guards",
    "handlers",
    "hashes",
    "imports",
    "indexing",
    "instances",
    "interpolation",
    "intrinsics",
    "lists",
    "match",
    "modules",
    "mutation",
    "nested-patterns",
    "options",
    "parameterized-handlers",
    "records",
    "recursion",
    "sealing",
    "superclasses",
    "tuples",
    "type-aliases",
    "user-effects",
];

/// The tags for `source`, header overrides applied.
pub fn features_of(source: &str) -> BTreeSet<String> {
    let mut parser = Parser::new(Lexer::new(source));
    let program = parser.parse_program();
    let interner = parser.take_interner();
    let mut walk = Walk::default();
    walk.visit_program(&program);
    let mut tags: BTreeSet<String> = walk.tags.iter().map(|tag| (*tag).to_string()).collect();
    if walk.effect_names.iter().any(|&name| resolves_to(&interner, name, "Async"))
        || walk
            .imports
            .iter()
            .any(|&name| name_of(&interner, name).is_some_and(|n| n.starts_with("Flow.Async")))
    {
        tags.insert("async".to_string());
    }
    apply_overrides(source, &mut tags);
    tags
}

fn name_of(interner: &Interner, name: Identifier) -> Option<&str> {
    interner.try_resolve(name)
}

fn resolves_to(interner: &Interner, name: Identifier, expected: &str) -> bool {
    name_of(interner, name) == Some(expected)
}

/// Applies the header's `// feature:` lines to `tags`.
fn apply_overrides(source: &str, tags: &mut BTreeSet<String>) {
    let header =
        source.lines().map(str::trim).take_while(|line| line.is_empty() || line.starts_with("//"));
    for line in header {
        let Some(list) =
            line.strip_prefix("//").and_then(|rest| rest.trim().strip_prefix("feature:"))
        else {
            continue;
        };
        for tag in list.split(',').map(str::trim).filter(|tag| !tag.is_empty()) {
            match tag.strip_prefix('-') {
                Some(removed) => {
                    tags.remove(removed.trim());
                }
                None => {
                    tags.insert(tag.to_string());
                }
            }
        }
    }
}

/// `flux features <file.flx>…`: one `path<TAB>tags` line per file. A file
/// that cannot be read prints `path<TAB>unreadable: <error>`.
pub fn run(args: &[String]) {
    if args.is_empty() {
        eprintln!("Usage: flux features <file.flx>...");
        std::process::exit(1);
    }
    for path in args {
        match std::fs::read_to_string(path) {
            Ok(source) => {
                let tags = features_of(&source).into_iter().collect::<Vec<_>>();
                println!("{path}\t{}", tags.join(","));
            }
            Err(err) => println!("{path}\tunreadable: {err}"),
        }
    }
}

#[derive(Default)]
struct Walk {
    tags: BTreeSet<&'static str>,
    /// Named functions being walked, innermost last, to see recursion.
    enclosing: Vec<Identifier>,
    /// How deep the walk is inside a pattern.
    pattern_depth: usize,
    effect_names: Vec<Identifier>,
    imports: Vec<Identifier>,
}

impl Walk {
    fn tag(&mut self, tag: &'static str) {
        self.tags.insert(tag);
    }

    fn effects(&mut self, effects: &[EffectExpr]) {
        if !effects.is_empty() {
            self.tag("effect-annotations");
        }
        for effect in effects {
            self.effect(effect);
        }
    }

    fn effect(&mut self, effect: &EffectExpr) {
        match effect {
            EffectExpr::Named { name, .. } => self.effect_names.push(*name),
            EffectExpr::RowVar { .. } => self.tag("effect-rows"),
            EffectExpr::Add { left, right, .. } | EffectExpr::Subtract { left, right, .. } => {
                self.effect(left);
                self.effect(right);
            }
        }
    }

    fn signature(
        &mut self,
        parameter_types: &[Option<crate::syntax::type_expr::TypeExpr>],
        return_type: Option<&crate::syntax::type_expr::TypeExpr>,
    ) {
        if parameter_types.iter().any(Option::is_some) || return_type.is_some() {
            self.tag("annotations");
        }
    }
}

impl<'ast> Visitor<'ast> for Walk {
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::Function {
                name,
                fip,
                intrinsic,
                type_params,
                parameter_types,
                return_type,
                effects,
                ..
            } => {
                if fip.is_some() {
                    self.tag("fip");
                }
                if intrinsic.is_some() {
                    self.tag("intrinsics");
                }
                if !type_params.is_empty() {
                    self.tag("generics");
                }
                self.signature(parameter_types, return_type.as_ref());
                self.effects(effects);
                self.enclosing.push(*name);
                visit::walk_stmt(self, stmt);
                self.enclosing.pop();
                return;
            }
            Statement::LetDestructure { .. } => self.tag("destructuring"),
            Statement::Assign { .. } => self.tag("mutation"),
            Statement::Module { .. } => self.tag("modules"),
            Statement::Import { name, .. } => {
                self.tag("imports");
                self.imports.push(*name);
            }
            Statement::Data { deriving, .. } => {
                self.tag("adts");
                if !deriving.is_empty() {
                    self.tag("deriving");
                }
            }
            Statement::EffectDecl { .. } | Statement::EffectAlias { .. } => {
                self.tag("user-effects");
            }
            Statement::TypeAlias(_) => self.tag("type-aliases"),
            Statement::Class { superclasses, associated_types, .. } => {
                self.tag("classes");
                if !superclasses.is_empty() {
                    self.tag("superclasses");
                }
                if !associated_types.is_empty() {
                    self.tag("associated-types");
                }
            }
            Statement::Instance { .. } => self.tag("instances"),
            _ => {}
        }
        visit::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'ast Expression) {
        match expr {
            Expression::InterpolatedString { .. } => self.tag("interpolation"),
            Expression::DoBlock { .. } => self.tag("do-blocks"),
            Expression::Function { parameter_types, return_type, effects, .. } => {
                self.tag("closures");
                self.signature(parameter_types, return_type.as_ref());
                self.effects(effects);
            }
            Expression::Call { function, .. } => {
                if let Expression::Identifier { name, .. } = function.as_ref()
                    && self.enclosing.contains(name)
                {
                    self.tag("recursion");
                }
            }
            Expression::ListLiteral { .. }
            | Expression::EmptyList { .. }
            | Expression::Cons { .. } => self.tag("lists"),
            Expression::ArrayLiteral { .. } => self.tag("arrays"),
            Expression::TupleLiteral { .. } | Expression::TupleFieldAccess { .. } => {
                self.tag("tuples");
            }
            Expression::Index { .. } => self.tag("indexing"),
            Expression::Hash { .. } => self.tag("hashes"),
            Expression::Match { .. } => self.tag("match"),
            Expression::Some { .. } | Expression::None { .. } => self.tag("options"),
            Expression::Left { .. } | Expression::Right { .. } => self.tag("either"),
            Expression::Perform { .. } => self.tag("user-effects"),
            Expression::Handle { parameter, .. } => {
                self.tag("handlers");
                if parameter.is_some() {
                    self.tag("parameterized-handlers");
                }
            }
            Expression::Sealing { .. } => self.tag("sealing"),
            Expression::NamedConstructor { .. } | Expression::Spread { .. } => self.tag("records"),
            _ => {}
        }
        visit::walk_expr(self, expr);
    }

    fn visit_match_arm(&mut self, arm: &'ast MatchArm) {
        if arm.guard.is_some() {
            self.tag("guards");
        }
        visit::walk_match_arm(self, arm);
    }

    fn visit_pat(&mut self, pat: &'ast Pattern) {
        let structured = !matches!(
            pat,
            Pattern::Wildcard { .. } | Pattern::Identifier { .. } | Pattern::Literal { .. }
        );
        if structured && self.pattern_depth > 0 {
            self.tag("nested-patterns");
        }
        match pat {
            Pattern::Cons { .. } | Pattern::EmptyList { .. } => self.tag("lists"),
            Pattern::Tuple { .. } => self.tag("tuples"),
            Pattern::Some { .. } | Pattern::None { .. } => self.tag("options"),
            Pattern::Left { .. } | Pattern::Right { .. } => self.tag("either"),
            Pattern::NamedConstructor { .. } => self.tag("records"),
            _ => {}
        }
        self.pattern_depth += 1;
        visit::walk_pat(self, pat);
        self.pattern_depth -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has(source: &str, tag: &str) -> bool {
        features_of(source).contains(tag)
    }

    #[test]
    fn each_construct_gives_its_tag() {
        let cases = [
            ("fn f(n) { if n <= 0 { 0 } else { f(n - 1) } }", "recursion"),
            ("fn main() with IO { print(\"#{1}\") }", "interpolation"),
            ("fn main() with IO { print(1) }", "effect-annotations"),
            ("effect Ask { ask: () -> Int }", "user-effects"),
            ("data Shape { Circle(Int) } deriving (Eq)", "deriving"),
            ("class Size<a> { fn size(x: a) -> Int }", "classes"),
            ("fn f(x: Int) -> Int { x }", "annotations"),
            ("fn f() { let g = \\x -> x; g(1) }", "closures"),
            ("fn f(xs) { match xs { [h | t] -> h, _ -> 0 } }", "lists"),
            ("fn f(p) { match p { Some(Some(x)) -> x, _ -> 0 } }", "nested-patterns"),
            ("fn f(x) { match x { n if n > 0 -> n, _ -> 0 } }", "guards"),
            ("import Flow.List", "imports"),
            ("fn f() { (1, 2) }", "tuples"),
        ];
        for (source, tag) in cases {
            assert!(has(source, tag), "{tag} in {source:?}: {:?}", features_of(source));
        }
    }

    #[test]
    fn a_call_to_another_function_is_not_recursion() {
        assert!(!has("fn g(n) { n }\nfn f(n) { g(n) }", "recursion"));
        assert!(!has("fn f(p) { match p { Some(x) -> x, _ -> 0 } }", "nested-patterns"));
    }

    #[test]
    fn async_is_seen_from_the_effect_row() {
        assert!(has("fn main() with Async { 1 }", "async"));
    }

    #[test]
    fn a_header_adds_and_removes_tags() {
        let source = "// feature: async, -closures\nfn f() { let g = \\x -> x; g(1) }";
        let tags = features_of(source);
        assert!(tags.contains("async"));
        assert!(!tags.contains("closures"));
    }

    #[test]
    fn every_tag_the_walk_gives_is_listed() {
        let source = "fn f(n) { if n <= 0 { 0 } else { f(n - 1) } }\n\
                      fn main() with IO { print(\"#{[1]}\"); match Some((1, 2)) { Some((a, b)) -> a, _ -> 0 } }";
        for tag in features_of(source) {
            assert!(TAGS.contains(&tag.as_str()), "{tag} is not in TAGS");
        }
    }
}
