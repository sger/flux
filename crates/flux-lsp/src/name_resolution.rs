//! Unresolved-name diagnostics (`E004`).
//!
//! HM inference is deliberately *not* the name-resolution stage: an unknown
//! identifier silently becomes a fresh type variable, because builtins,
//! constructors, class methods, and module members are resolved through side
//! tables (and undefined-variable reporting proper happens later, in the VM
//! `compiler/` pass with its symbol table — which the LSP never runs). This
//! pass fills that gap for the editor: a dedicated lexical-scope walk over the
//! AST that flags value identifiers nothing binds, and members a known module
//! does not export.
//!
//! It is intentionally conservative to keep false positives near zero:
//!
//! - Only **lowercase** value identifiers are flagged. Uppercase names (types,
//!   constructors, module qualifiers) have a murkier resolution story — a bare
//!   `Modules.Math.square` leads with `Modules`, which binds nothing on its own
//!   — so they are left to the type checker.
//! - A name counts as resolved if it is bound in any lexical scope, is a
//!   top-level / module-level declaration, a class method, an `exposing` import,
//!   or one of the prelude/builtin names inference was seeded with
//!   (`Snapshot::base_names`).
//! - Module members are only checked for an **aliased** import whose full member
//!   list is known (the Flow stdlib, keyed by short name); anything less certain
//!   is skipped.

use std::collections::HashSet;

use flux::compiler::suggestions::find_similar_names;
use flux::diagnostics::position::Span as FluxSpan;
use flux::diagnostics::{Diagnostic as FluxDiagnostic, UNDEFINED_VARIABLE, UNKNOWN_MODULE_MEMBER};
use flux::syntax::block::Block;
use flux::syntax::expression::{Expression, Pattern, StringPart};
use flux::syntax::interner::Interner;
use flux::syntax::statement::Statement;

use crate::snapshot::Snapshot;

/// Walk `snapshot`'s AST and return an `E004` diagnostic for every lowercase
/// value identifier that resolves to nothing, plus every member access on a
/// known aliased module that the module does not export.
pub fn unresolved_name_diagnostics(snapshot: &Snapshot) -> Vec<FluxDiagnostic> {
    let interner = &snapshot.interner;
    let mut globals: HashSet<String> = snapshot
        .base_names
        .iter()
        .filter_map(|id| interner.try_resolve(*id))
        .map(str::to_string)
        .collect();
    collect_global_decls(&snapshot.program.statements, interner, &mut globals);

    // `import X exposing (..)` brings every member of X into scope unqualified.
    // (The `exposing (a, b)` form is handled name-by-name in `collect_global_decls`.)
    // Members come from the Flow-stdlib index (`module_members`, keyed by short
    // name) or, for a sibling/subdir `module`, the workspace symbol index
    // (`user_module_members`, keyed by module name).
    for stmt in &snapshot.program.statements {
        if let Statement::Import {
            name,
            exposing: flux::syntax::statement::ImportExposing::All,
            ..
        } = stmt
            && let Some(full) = interner.try_resolve(*name)
        {
            if let Some(members) = snapshot.module_members.get(last_segment(full)) {
                globals.extend(members.iter().cloned());
            }
            if let Some(members) = snapshot
                .user_module_members
                .get(full)
                .or_else(|| snapshot.user_module_members.get(last_segment(full)))
            {
                globals.extend(members.iter().cloned());
            }
        }
    }

    let module_members = module_member_sets(snapshot);
    let module_bindings: HashSet<String> = module_members.keys().cloned().collect();

    let mut resolver = Resolver {
        interner,
        globals,
        module_members,
        module_bindings,
        scopes: Vec::new(),
        diagnostics: Vec::new(),
    };
    for stmt in &snapshot.program.statements {
        resolver.check_stmt(stmt);
    }
    resolver.diagnostics
}

struct Resolver<'a> {
    interner: &'a Interner,
    /// Names resolvable everywhere: prelude/builtins, top-level and
    /// module-level declarations, class methods, `exposing` imports.
    globals: HashSet<String>,
    /// Aliased-module binding name → its full set of exported members. Only
    /// modules whose member list is known with confidence appear here.
    module_members: std::collections::HashMap<String, HashSet<String>>,
    /// Every name that denotes a module (so `Mod.member` is treated as module
    /// access, never a record field access). Superset of `module_members`'s keys.
    module_bindings: HashSet<String>,
    /// Lexical scopes, innermost last. Each holds names bound by params, `let`s,
    /// lambda params, and pattern bindings.
    scopes: Vec<HashSet<String>>,
    diagnostics: Vec<FluxDiagnostic>,
}

impl Resolver<'_> {
    fn resolve(&self, id: flux::syntax::Identifier) -> Option<&str> {
        self.interner.try_resolve(id)
    }

    fn is_bound(&self, name: &str) -> bool {
        self.globals.contains(name) || self.scopes.iter().any(|s| s.contains(name))
    }

    /// Flag `name` as undefined unless it is bound, uppercase, `_`-prefixed, or a
    /// globally-available builtin primop spelling (`cmp_eq`, `len`, …) the
    /// surface language recognizes without a declaration.
    fn check_value_name(&mut self, name: &str, span: FluxSpan) {
        if !flaggable(name)
            || self.is_bound(name)
            || self.module_bindings.contains(name)
            || flux::core::CorePrimOp::is_builtin_helper_name(name)
        {
            return;
        }
        let mut candidates: Vec<String> = self.globals.iter().cloned().collect();
        for scope in &self.scopes {
            candidates.extend(scope.iter().cloned());
        }
        let mut message = format!("I can't find a value named `{name}`.");
        if let Some(suggestion) = find_similar_names(name, &candidates, 1).into_iter().next() {
            message.push_str(&format!(" Did you mean `{suggestion}`?"));
        }
        self.emit(message, span);
    }

    fn emit(&mut self, message: String, span: FluxSpan) {
        self.diagnostics.push(FluxDiagnostic::make_error_dynamic(
            UNDEFINED_VARIABLE.code,
            UNDEFINED_VARIABLE.title,
            UNDEFINED_VARIABLE.error_type,
            message,
            None,
            "",
            span,
        ));
    }

    fn push_scope(&mut self, names: HashSet<String>) {
        self.scopes.push(names);
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn check_stmt(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Function {
                parameters,
                body,
                intrinsic,
                ..
            } => {
                // An `intrinsic fn … = primop P` has a compiler-generated body
                // (a call to P's helper, e.g. `string_concat_builtin`) — nothing
                // the user wrote, so there is nothing to resolve.
                if intrinsic.is_some() {
                    return;
                }
                let mut scope = self.idents_to_strings(parameters);
                self.hoist_block(body, &mut scope);
                self.push_scope(scope);
                self.check_block(body);
                self.pop_scope();
            }
            Statement::Let { value, .. } => self.check_expr(value),
            Statement::LetDestructure { value, .. } => self.check_expr(value),
            Statement::Assign { value, .. } => self.check_expr(value),
            Statement::Return { value: Some(v), .. } => self.check_expr(v),
            Statement::Expression { expression, .. } => self.check_expr(expression),
            Statement::Module { body, .. } => {
                // A module's members are visible to each other unqualified.
                let mut scope = HashSet::new();
                self.hoist_block(body, &mut scope);
                collect_global_decls(&body.statements, self.interner, &mut scope);
                self.push_scope(scope);
                self.check_block(body);
                self.pop_scope();
            }
            Statement::Class { methods, .. } => {
                for method in methods {
                    if let Some(body) = &method.default_body {
                        let mut scope = self.idents_to_strings(&method.params);
                        self.hoist_block(body, &mut scope);
                        self.push_scope(scope);
                        self.check_block(body);
                        self.pop_scope();
                    }
                }
            }
            Statement::Instance { methods, .. } => {
                for method in methods {
                    let mut scope = self.idents_to_strings(&method.params);
                    self.hoist_block(&method.body, &mut scope);
                    self.push_scope(scope);
                    self.check_block(&method.body);
                    self.pop_scope();
                }
            }
            _ => {}
        }
    }

    fn check_block(&mut self, block: &Block) {
        for stmt in &block.statements {
            self.check_stmt(stmt);
        }
    }

    fn check_expr(&mut self, expr: &Expression) {
        match expr {
            Expression::Identifier { name, span, .. } => {
                if let Some(text) = self.resolve(*name) {
                    let text = text.to_string();
                    self.check_value_name(&text, *span);
                }
            }
            Expression::MemberAccess {
                object,
                member,
                span,
                ..
            } => self.check_member_access(object, *member, *span),
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                self.check_expr(function);
                for arg in arguments {
                    self.check_expr(arg);
                }
            }
            Expression::Function {
                parameters, body, ..
            } => {
                let mut scope = self.idents_to_strings(parameters);
                self.hoist_block(body, &mut scope);
                self.push_scope(scope);
                self.check_block(body);
                self.pop_scope();
            }
            Expression::DoBlock { block, .. } => {
                let mut scope = HashSet::new();
                self.hoist_block(block, &mut scope);
                self.push_scope(scope);
                self.check_block(block);
                self.pop_scope();
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                self.check_expr(condition);
                self.check_branch(consequence);
                if let Some(alt) = alternative {
                    self.check_branch(alt);
                }
            }
            Expression::Match {
                scrutinee, arms, ..
            } => {
                self.check_expr(scrutinee);
                for arm in arms {
                    let mut scope = HashSet::new();
                    pattern_bindings(&arm.pattern, self.interner, &mut scope);
                    self.push_scope(scope);
                    if let Some(guard) = &arm.guard {
                        self.check_expr(guard);
                    }
                    self.check_expr(&arm.body);
                    self.pop_scope();
                }
            }
            Expression::Handle {
                expr,
                parameter,
                arms,
                ..
            } => {
                self.check_expr(expr);
                if let Some(p) = parameter {
                    self.check_expr(p);
                }
                for arm in arms {
                    let mut scope = HashSet::new();
                    if let Some(name) = self.resolve(arm.resume_param) {
                        scope.insert(name.to_string());
                    }
                    for p in &arm.params {
                        if let Some(name) = self.resolve(*p) {
                            scope.insert(name.to_string());
                        }
                    }
                    self.push_scope(scope);
                    self.check_expr(&arm.body);
                    self.pop_scope();
                }
            }
            Expression::Infix { left, right, .. } => {
                self.check_expr(left);
                self.check_expr(right);
            }
            Expression::Prefix { right, .. } => self.check_expr(right),
            Expression::Index { left, index, .. } => {
                self.check_expr(left);
                self.check_expr(index);
            }
            Expression::Cons { head, tail, .. } => {
                self.check_expr(head);
                self.check_expr(tail);
            }
            Expression::Some { value, .. }
            | Expression::Left { value, .. }
            | Expression::Right { value, .. } => self.check_expr(value),
            Expression::Sealing { expr, .. } => self.check_expr(expr),
            Expression::TupleFieldAccess { object, .. } => self.check_expr(object),
            Expression::ListLiteral { elements, .. }
            | Expression::ArrayLiteral { elements, .. }
            | Expression::TupleLiteral { elements, .. } => {
                for e in elements {
                    self.check_expr(e);
                }
            }
            Expression::Hash { pairs, .. } => {
                for (k, v) in pairs {
                    self.check_expr(k);
                    self.check_expr(v);
                }
            }
            Expression::Perform { args, .. } => {
                for arg in args {
                    self.check_expr(arg);
                }
            }
            Expression::NamedConstructor { fields, .. } => {
                for field in fields {
                    self.check_field_init(field);
                }
            }
            Expression::Spread {
                base, overrides, ..
            } => {
                self.check_expr(base);
                for field in overrides {
                    self.check_field_init(field);
                }
            }
            Expression::InterpolatedString { parts, .. } => {
                for part in parts {
                    if let StringPart::Interpolation(e) = part {
                        self.check_expr(e);
                    }
                }
            }
            _ => {}
        }
    }

    /// A record field initializer: `Point { x: expr }` checks `expr`; the pun
    /// `Point { x }` references a value named `x` in scope.
    fn check_field_init(&mut self, field: &flux::syntax::expression::NamedFieldInit) {
        match &field.value {
            Some(value) => self.check_expr(value),
            None => {
                if let Some(text) = self.resolve(field.name) {
                    let text = text.to_string();
                    self.check_value_name(&text, field.span);
                }
            }
        }
    }

    /// `object.member`: when `object` names a module, treat `member` as a module
    /// member (flag it if the module is known not to export it); otherwise it is
    /// a value/field access — recurse into `object` and leave `member` alone.
    fn check_member_access(
        &mut self,
        object: &Expression,
        member: flux::syntax::Identifier,
        span: FluxSpan,
    ) {
        if let Expression::Identifier { name, .. } = object {
            let Some(object_name) = self.resolve(*name).map(str::to_string) else {
                return;
            };
            if self.module_bindings.contains(&object_name) {
                let member_name = self.resolve(member).map(str::to_string);
                // Compute everything before `emit` so no `module_members` borrow
                // is held across the mutable diagnostic push.
                let missing = match (self.module_members.get(&object_name), &member_name) {
                    (Some(members), Some(m)) => !members.contains(m),
                    _ => false,
                };
                if missing {
                    let member_name = member_name.unwrap_or_default();
                    self.diagnostics.push(FluxDiagnostic::make_error(
                        &UNKNOWN_MODULE_MEMBER,
                        &[&object_name, &member_name],
                        "",
                        span,
                    ));
                }
                return;
            }
            // Not a module — `object` is a value (e.g. a record); check it.
            self.check_value_name(&object_name, object.span());
            return;
        }
        self.check_expr(object);
    }

    fn idents_to_strings(&self, ids: &[flux::syntax::Identifier]) -> HashSet<String> {
        ids.iter()
            .filter_map(|id| self.resolve(*id))
            .map(str::to_string)
            .collect()
    }

    /// Pre-seed a block's scope with every `fn`/`let` name it declares, so
    /// forward references and mutual recursion never read as undefined.
    fn hoist_block(&self, block: &Block, scope: &mut HashSet<String>) {
        for stmt in &block.statements {
            match stmt {
                Statement::Function { name, .. } | Statement::Let { name, .. } => {
                    if let Some(text) = self.resolve(*name) {
                        scope.insert(text.to_string());
                    }
                }
                Statement::LetDestructure { pattern, .. } => {
                    pattern_bindings(pattern, self.interner, scope);
                }
                _ => {}
            }
        }
    }

    fn check_branch(&mut self, block: &Block) {
        let mut scope = HashSet::new();
        self.hoist_block(block, &mut scope);
        self.push_scope(scope);
        self.check_block(block);
        self.pop_scope();
    }
}

fn flaggable(name: &str) -> bool {
    matches!(name.chars().next(), Some(c) if c.is_ascii_lowercase())
}

/// Collect declaration names that should resolve unqualified within their
/// enclosing scope: functions, `let`s, data type names and constructors, effect
/// and class names (and class methods), type aliases, module names.
fn collect_global_decls(stmts: &[Statement], interner: &Interner, out: &mut HashSet<String>) {
    let add = |out: &mut HashSet<String>, id: flux::syntax::Identifier| {
        if let Some(text) = interner.try_resolve(id) {
            out.insert(text.to_string());
        }
    };
    for stmt in stmts {
        match stmt {
            Statement::Function { name, .. }
            | Statement::Let { name, .. }
            | Statement::EffectDecl { name, .. }
            | Statement::EffectAlias { name, .. } => add(out, *name),
            Statement::Data { name, variants, .. } => {
                add(out, *name);
                for variant in variants {
                    add(out, variant.name);
                }
            }
            Statement::Class { name, methods, .. } => {
                add(out, *name);
                for method in methods {
                    add(out, method.name);
                }
            }
            Statement::TypeAlias(alias) => add(out, alias.name),
            Statement::Module { name, .. } => {
                add(out, *name);
                if let Some(text) = interner.try_resolve(*name) {
                    out.insert(last_segment(text).to_string());
                }
            }
            Statement::Import {
                name,
                alias,
                exposing,
                ..
            } => {
                // The binding name (alias, or the full path) so the module is in
                // scope; plus any `exposing (...)` members made unqualified.
                if let Some(alias) = alias {
                    add(out, *alias);
                } else if let Some(text) = interner.try_resolve(*name) {
                    out.insert(text.to_string());
                }
                if let flux::syntax::statement::ImportExposing::Names(names) = exposing {
                    for n in names {
                        add(out, *n);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Build `binding name → exported member set` for imports whose members are
/// known with confidence: aliased Flow stdlib modules (keyed by short name in
/// `Snapshot::module_members`) and buffer-local `module` declarations.
fn module_member_sets(snapshot: &Snapshot) -> std::collections::HashMap<String, HashSet<String>> {
    let interner = &snapshot.interner;
    let mut map: std::collections::HashMap<String, HashSet<String>> =
        std::collections::HashMap::new();

    for stmt in &snapshot.program.statements {
        match stmt {
            Statement::Import { name, alias, .. } => {
                let Some(full) = interner.try_resolve(*name) else {
                    continue;
                };
                let short = last_segment(full);
                // Prefer the Flow-stdlib member set (keyed by short name); fall
                // back to a sibling/subdir `module`'s members from the workspace
                // symbol index (keyed by full or short name).
                let members: HashSet<String> = if let Some(m) = snapshot.module_members.get(short) {
                    m.iter().cloned().collect()
                } else if let Some(m) = snapshot
                    .user_module_members
                    .get(full)
                    .or_else(|| snapshot.user_module_members.get(short))
                {
                    m.clone()
                } else {
                    // Members of this module aren't known with confidence — skip
                    // the membership check (but still treat it as a binding).
                    continue;
                };
                let binding = alias
                    .and_then(|a| interner.try_resolve(a))
                    .unwrap_or(full)
                    .to_string();
                map.insert(binding, members);
            }
            Statement::Module { name, body, .. } => {
                let mut members = HashSet::new();
                for inner in &body.statements {
                    match inner {
                        Statement::Function { name, .. } | Statement::Let { name, .. } => {
                            if let Some(text) = interner.try_resolve(*name) {
                                members.insert(text.to_string());
                            }
                        }
                        _ => {}
                    }
                }
                if let Some(text) = interner.try_resolve(*name) {
                    map.insert(text.to_string(), members.clone());
                    map.insert(last_segment(text).to_string(), members);
                }
            }
            _ => {}
        }
    }
    map
}

/// Add every variable a pattern binds (recursing into tuples, constructors, and
/// list-cons forms) to `out`.
fn pattern_bindings(pattern: &Pattern, interner: &Interner, out: &mut HashSet<String>) {
    match pattern {
        Pattern::Identifier { name, .. } => {
            if let Some(text) = interner.try_resolve(*name) {
                out.insert(text.to_string());
            }
        }
        Pattern::Tuple { elements, .. } => {
            for e in elements {
                pattern_bindings(e, interner, out);
            }
        }
        Pattern::Constructor { fields, .. } => {
            for f in fields {
                pattern_bindings(f, interner, out);
            }
        }
        Pattern::NamedConstructor { fields, .. } => {
            for f in fields {
                if let Some(p) = &f.pattern {
                    pattern_bindings(p, interner, out);
                } else if let Some(text) = interner.try_resolve(f.name) {
                    // Field pun in a pattern binds a variable named after the field.
                    out.insert(text.to_string());
                }
            }
        }
        Pattern::Some { pattern, .. }
        | Pattern::Left { pattern, .. }
        | Pattern::Right { pattern, .. } => pattern_bindings(pattern, interner, out),
        Pattern::Cons { head, tail, .. } => {
            pattern_bindings(head, interner, out);
            pattern_bindings(tail, interner, out);
        }
        _ => {}
    }
}

fn last_segment(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_lowercase_initial_names_are_flaggable() {
        // Lowercase values can be undefined; uppercase (types, constructors,
        // module qualifiers) and `_`-prefixed names are left to the type checker.
        assert!(flaggable("foo"));
        assert!(flaggable("amount"));
        assert!(!flaggable("Foo"));
        assert!(!flaggable("List"));
        assert!(!flaggable("_unused"));
        assert!(!flaggable(""));
    }

    #[test]
    fn last_segment_takes_final_dotted_part() {
        assert_eq!(last_segment("Flow.List"), "List");
        assert_eq!(last_segment("Modules.App.Main"), "Main");
        assert_eq!(last_segment("Bare"), "Bare");
    }
}
