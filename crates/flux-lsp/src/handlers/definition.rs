use flux::diagnostics::position::{Position as FluxPosition, Span as FluxSpan};
use flux::syntax::Identifier;
use flux::syntax::block::Block;
use flux::syntax::effect_expr::EffectExpr;
use flux::syntax::expression::{Expression, Pattern};
use flux::syntax::interner::Interner;
use flux::syntax::program::Program;
use flux::syntax::statement::Statement;
use flux::syntax::type_expr::TypeExpr;
use flux::types::infer_type::InferType;
use flux::types::type_constructor::TypeConstructor;
use lsp_types::{Position, Uri};

use crate::keywords::{is_offset_in_comment_or_string, word_at_offset};
use crate::line_index::PositionMap;
use crate::locator::{NodeRef, find_at, position_in_span};
use crate::navigation_target::NavigationTarget;
use crate::snapshot::Snapshot;
use crate::symbol_index::{Entry, SymbolIndex};

/// Resolve a goto-definition request to a richer [`NavigationTarget`]
/// carrying both `full_range` (whole declaration) and `focus_range`
/// (identifier only). The LSP boundary in
/// [`crate::global_state::GlobalState::handle_definition`] lowers this
/// to `GotoDefinitionResponse::Link(Vec<LocationLink>)`.
pub fn goto_definition(
    snapshot: &Snapshot,
    uri: &Uri,
    position: Position,
) -> Option<NavigationTarget> {
    let target = snapshot.position_map.lsp_to_flux(position)?;

    // Control-flow keyword nav (rust-analyzer-style): F12 on `return`
    // jumps to the enclosing `fn` signature; F12 on `else` to its
    // matching `if`; F12 on `resume` to the enclosing `handle` block.
    // Runs *before* the AST locator because the locator returns the
    // enclosing statement for raw keyword tokens — which gives less
    // useful nav than these dedicated handlers.
    if let Some(offset) = snapshot.position_map.lsp_to_offset(position) {
        let off: usize = offset.into();
        if !is_offset_in_comment_or_string(snapshot.text.as_ref(), off)
            && let Some(word) = word_at_offset(snapshot.text.as_ref(), off)
            && let Some((full, focus)) = control_flow_target(snapshot, word, target)
        {
            return Some(local_target(snapshot, uri, full, focus, word));
        }
    }

    let node = find_at(&snapshot.program, &snapshot.interner, target)?;

    // Cross-module: cursor on the member name of a module-qualified access
    // (e.g. `sqrt` in `Math.sqrt`, or `map` in `A.map` where `A` is an
    // `import Flow.Array as A` alias). Look up the member in the prelude's
    // cached program for that module and jump to its definition there.
    if let NodeRef::MemberAccessMember { object, member, .. } = &node {
        if let Expression::Identifier {
            name: object_id, ..
        } = object
            && let Some(module_short) = resolve_module_short_name(snapshot, *object_id)
            && let Some((mod_program, mod_source, mod_path)) =
                snapshot.module_programs.get(&module_short)
        {
            let mod_index = SymbolIndex::build_for_module_file(mod_program, &snapshot.interner);
            if let Some(entry) = mod_index.lookup_id(*member) {
                let mod_map = PositionMap::new(
                    std::sync::Arc::from(mod_source.as_ref()),
                    snapshot.position_map.encoding(),
                );
                let module_uri = crate::vfs::path_to_uri(mod_path)?;
                return Some(target_from_entry(module_uri, &mod_map, entry));
            }
        }
        // Multi-segment qualified path: `A.B.C.member` where `A.B.C` is a
        // loaded module's full declared name. `resolve_module_short_name`
        // only handles a lone identifier or an `import … as A` alias, so
        // flatten the `object` member-access chain to a dotted string and
        // look it up directly.
        if let Some(module_path) = module_path_string(object, &snapshot.interner)
            && let Some((mod_program, mod_source, mod_path)) =
                snapshot.module_programs.get(&module_path)
        {
            let mod_index = SymbolIndex::build_for_module_file(mod_program, &snapshot.interner);
            if let Some(entry) = mod_index.lookup_id(*member) {
                let mod_map = PositionMap::new(
                    std::sync::Arc::from(mod_source.as_ref()),
                    snapshot.position_map.encoding(),
                );
                let module_uri = crate::vfs::path_to_uri(mod_path)?;
                return Some(target_from_entry(module_uri, &mod_map, entry));
            }
        }
        // Record field: cursor on `.name` in `alice.name`. If `object`'s
        // inferred type is an ADT declared in this buffer, jump to the
        // field's name position in the `data` decl. The variant's
        // declaration span is the full range; the synthesized field-name
        // span is the focus.
        if let Some(adt_sym) = inferred_adt_symbol(snapshot, object)
            && let Some((variant_span, field_span)) =
                find_data_field_spans(&snapshot.program, &snapshot.interner, adt_sym, *member)
        {
            let name = snapshot
                .interner
                .try_resolve(*member)
                .unwrap_or("")
                .to_string();
            return Some(NavigationTarget {
                uri: uri.clone(),
                full_range: snapshot.position_map.flux_span_to_range(variant_span),
                focus_range: snapshot.position_map.flux_span_to_range(field_span),
                name,
            });
        }
    }

    // Import-alias use site: F12 on a bare identifier that's the local
    // alias of an `import X.Y as A` (e.g. cursor on `A` in `A.map(...)`).
    // Jump to the import statement that introduced it. Focus = the alias
    // identifier within the statement; full = the whole import statement.
    if let NodeRef::Expr(Expression::Identifier { name, .. }) = &node
        && let Some((stmt_span, alias_span)) =
            find_import_alias_spans(&snapshot.program, &snapshot.interner, *name)
    {
        let alias_text = snapshot
            .interner
            .try_resolve(*name)
            .unwrap_or("")
            .to_string();
        return Some(NavigationTarget {
            uri: uri.clone(),
            full_range: snapshot.position_map.flux_span_to_range(stmt_span),
            focus_range: snapshot.position_map.flux_span_to_range(alias_span),
            name: alias_text,
        });
    }
    // Cursor on an `import` statement — its module name or its alias.
    // Jump into the imported module's file (to its `module` declaration)
    // when the module is loaded; otherwise fall back to the import
    // statement itself, the locator's precise sub-position as the focus.
    if let NodeRef::ImportAlias {
        alias,
        qualified,
        span,
    } = &node
    {
        if let Some(target) = import_module_target(snapshot, *qualified) {
            return Some(target);
        }
        let stmt_span = find_import_stmt_span_by_alias(&snapshot.program, *alias).unwrap_or(*span);
        let name = snapshot
            .interner
            .try_resolve(*qualified)
            .unwrap_or("")
            .to_string();
        return Some(NavigationTarget {
            uri: uri.clone(),
            full_range: snapshot.position_map.flux_span_to_range(stmt_span),
            focus_range: snapshot.position_map.flux_span_to_range(*span),
            name,
        });
    }
    if let NodeRef::ImportName {
        qualified, span, ..
    } = &node
    {
        if let Some(target) = import_module_target(snapshot, *qualified) {
            return Some(target);
        }
        let stmt_span =
            find_import_stmt_span_by_qualified(&snapshot.program, *qualified).unwrap_or(*span);
        let name = snapshot
            .interner
            .try_resolve(*qualified)
            .unwrap_or("")
            .to_string();
        return Some(NavigationTarget {
            uri: uri.clone(),
            full_range: snapshot.position_map.flux_span_to_range(stmt_span),
            focus_range: snapshot.position_map.flux_span_to_range(*span),
            name,
        });
    }

    // Effect row variable use site (`|e` in a function body's effect row
    // or in a `handle` clause). Jump to the binding occurrence. Focus =
    // the row var token; full = the enclosing function signature.
    if let NodeRef::EffectRowVar { name, span } = &node {
        let name_text = snapshot
            .interner
            .try_resolve(*name)
            .unwrap_or("")
            .to_string();
        if let Some((fn_span, binding_span)) =
            find_row_var_binding_with_fn(&snapshot.program, *name)
        {
            return Some(NavigationTarget {
                uri: uri.clone(),
                full_range: snapshot.position_map.flux_span_to_range(fn_span),
                focus_range: snapshot.position_map.flux_span_to_range(binding_span),
                name: name_text,
            });
        }
        // No earlier binding found — collapse to the cursor's own span.
        let range = snapshot.position_map.flux_span_to_range(*span);
        return Some(NavigationTarget::collapsed(uri.clone(), range, name_text));
    }

    // Class-method dispatch: F12 on `show(x)` where inference resolved
    // the call to `instance Show<Int>`. Look the dispatch up by the
    // function expression's `ExprId` (what the type checker keyed
    // `InferProgramResult::class_method_dispatch` by) and route to the
    // matching `InstanceMethod` arm via the snapshot's instance_index.
    //
    // Runs *before* the `extended_index.lookup_id(def_name)`
    // fall-through so a dispatched call lands on the instance arm
    // rather than the class-method declaration.
    if let NodeRef::Expr(ident_expr @ Expression::Identifier { name, .. }) = &node
        && let Some(infer) = snapshot.infer.as_ref()
        && let Some(dispatch) = infer.class_method_dispatch.get(&ident_expr.expr_id())
        && let Some(entry) = snapshot.instance_index.lookup(
            dispatch.class_name,
            dispatch.head_type_ctor,
            dispatch.method_name,
        )
    {
        let method_name = snapshot
            .interner
            .try_resolve(*name)
            .unwrap_or("")
            .to_string();
        return Some(NavigationTarget {
            uri: uri.clone(),
            full_range: snapshot.position_map.flux_span_to_range(entry.full_span),
            focus_range: snapshot.position_map.flux_span_to_range(entry.focus_span),
            name: method_name,
        });
    }

    let def_name = definition_name(&node)?;

    // Extended index covers top-level names + effect ops + data variants.
    let extended_index = SymbolIndex::build_extended(&snapshot.program, &snapshot.interner);
    if let Some(entry) = extended_index.lookup_id(def_name) {
        return Some(target_from_entry(
            uri.clone(),
            &snapshot.position_map,
            entry,
        ));
    }

    // Local binding walk (let/fn/parameter inside a function body).
    if let Some((full_span, focus_span)) =
        find_local_definition(&snapshot.program, &snapshot.interner, def_name)
    {
        let name = snapshot
            .interner
            .try_resolve(def_name)
            .unwrap_or("")
            .to_string();
        return Some(NavigationTarget {
            uri: uri.clone(),
            full_range: snapshot.position_map.flux_span_to_range(full_span),
            focus_range: snapshot.position_map.flux_span_to_range(focus_span),
            name,
        });
    }

    // Cross-module fallback: an unqualified reference to a member of an
    // imported module — a Flow-prelude function used bare (`len`, `print`),
    // or a sibling user module's export. Nothing in this buffer defines the
    // name, so search every cached module program for a matching declaration.
    module_member_target(snapshot, def_name)
}

/// Resolve `textDocument/typeDefinition`: from an expression (or a `let`/
/// pattern binding) jump to the declaration of its inferred type's ADT or
/// alias. rust-analyzer's "Go to Type Definition" — the Flux analogue resolves
/// the cursor's inferred type to its `data`/`type` declaration, same-file first
/// then any cached module (a Flow-prelude or sibling-module type).
pub fn goto_type_definition(
    snapshot: &Snapshot,
    uri: &Uri,
    position: Position,
) -> Option<NavigationTarget> {
    let target = snapshot.position_map.lsp_to_flux(position)?;
    let node = find_at(&snapshot.program, &snapshot.interner, target)?;
    let adt_sym = node_type_adt_symbol(snapshot, &node)?;

    let extended_index = SymbolIndex::build_extended(&snapshot.program, &snapshot.interner);
    if let Some(entry) = extended_index.lookup_id(adt_sym) {
        return Some(target_from_entry(
            uri.clone(),
            &snapshot.position_map,
            entry,
        ));
    }
    module_member_target(snapshot, adt_sym)
}

/// The ADT/alias symbol of the type a node carries: an expression's inferred
/// type, or a `let`/pattern binding's scheme. `None` when the node has no
/// resolvable type or its type isn't a user ADT (a function, a built-in like
/// `Int`, or a bare type variable).
fn node_type_adt_symbol(snapshot: &Snapshot, node: &NodeRef) -> Option<Identifier> {
    let infer = snapshot.infer.as_ref()?;
    let ty = match node {
        NodeRef::Expr(expr) => infer.expr_types.get(&expr.expr_id())?,
        NodeRef::Pattern(Pattern::Identifier { name, .. }) => {
            &infer.resolved_binding_schemes.get(name)?.infer_type
        }
        NodeRef::DeclName { binding_span, .. } => {
            let key = (
                binding_span.start.line,
                binding_span.start.column,
                binding_span.end.line,
                binding_span.end.column,
            );
            &infer.resolved_binding_schemes_by_span.get(&key)?.infer_type
        }
        _ => return None,
    };
    adt_symbol_of_infer_type(ty)
}

/// Search every cached module program (`module_programs` carries the Flow
/// prelude modules plus, in a cross-file component, the sibling user modules)
/// for a top-level — or `module`-block-nested — definition of `def_name`,
/// returning a `NavigationTarget` into that module's source file.
///
/// Iteration order over the `module_programs` map is unspecified, so if two
/// modules export the same name the winner is arbitrary. That is acceptable
/// for a last-resort fallback that only runs once every in-buffer lookup has
/// failed.
fn module_member_target(snapshot: &Snapshot, def_name: Identifier) -> Option<NavigationTarget> {
    for (program, source, path) in snapshot.module_programs.values() {
        let index = SymbolIndex::build_for_module_file(program, &snapshot.interner);
        if let Some(entry) = index.lookup_id(def_name) {
            let module_map = PositionMap::new(
                std::sync::Arc::from(source.as_ref()),
                snapshot.position_map.encoding(),
            );
            let module_uri = crate::vfs::path_to_uri(path)?;
            return Some(target_from_entry(module_uri, &module_map, entry));
        }
    }
    None
}

/// Build a `NavigationTarget` from a `SymbolIndex::Entry`, converting
/// the entry's Flux spans to LSP ranges against the destination file's
/// position map. Used for both same-file (passing `snapshot.position_map`)
/// and cross-module (passing a freshly-constructed map from the module
/// source) lookups — the same code path produces correct ranges either
/// way because the converter is the parameter.
fn target_from_entry(uri: Uri, dest_map: &PositionMap, entry: &Entry) -> NavigationTarget {
    NavigationTarget {
        uri,
        full_range: dest_map.flux_span_to_range(entry.full_span),
        focus_range: dest_map.flux_span_to_range(entry.focus_span),
        name: entry.name.clone(),
    }
}

/// Build a `NavigationTarget` pointing into the current buffer from two
/// Flux spans plus the focus identifier text.
fn local_target(
    snapshot: &Snapshot,
    uri: &Uri,
    full: FluxSpan,
    focus: FluxSpan,
    name: &str,
) -> NavigationTarget {
    NavigationTarget {
        uri: uri.clone(),
        full_range: snapshot.position_map.flux_span_to_range(full),
        focus_range: snapshot.position_map.flux_span_to_range(focus),
        name: name.to_string(),
    }
}

/// Route control-flow keywords to their syntactic anchor and return
/// `(full_span, focus_span)`:
/// - `return` → enclosing `fn` signature; focus collapses to the same
///   span since the function statement only carries the signature span.
/// - `else`   → matching `if` expression; focus collapses to the same.
/// - `resume` → enclosing `handle` expression; focus collapses to the same.
///
/// Returns `None` for any other word so the caller can fall through to
/// ordinary identifier resolution. We collapse focus = full here because
/// the existing parser data doesn't carry sub-spans for these
/// constructs; a future enrichment could narrow focus to just the
/// `fn`/`if`/`handle` keyword position.
fn control_flow_target(
    snapshot: &Snapshot,
    word: &str,
    target: FluxPosition,
) -> Option<(FluxSpan, FluxSpan)> {
    let span = match word {
        "return" => enclosing_function_span(&snapshot.program, target)?,
        "else" => enclosing_if_span(&snapshot.program, target)?,
        "resume" => enclosing_handle_span(&snapshot.program, target)?,
        _ => return None,
    };
    Some((span, span))
}

fn enclosing_function_span(program: &Program, target: FluxPosition) -> Option<FluxSpan> {
    for stmt in &program.statements {
        if let Statement::Function { span, body, .. } = stmt {
            // The function statement's `span` covers only the signature;
            // the body has its own block span. The "return" keyword sits
            // inside the body — so the cursor will be in the body's
            // statement spans, not in `span`. Walk both.
            if position_in_span(target, *span)
                || body
                    .statements
                    .iter()
                    .any(|s| position_in_span(target, s.span()))
            {
                return Some(*span);
            }
        }
    }
    None
}

fn enclosing_if_span(program: &Program, target: FluxPosition) -> Option<FluxSpan> {
    for stmt in &program.statements {
        if let Some(span) = enclosing_if_in_stmt(stmt, target) {
            return Some(span);
        }
    }
    None
}

fn enclosing_if_in_stmt(stmt: &Statement, target: FluxPosition) -> Option<FluxSpan> {
    match stmt {
        Statement::Let { value, .. }
        | Statement::Assign { value, .. }
        | Statement::LetDestructure { value, .. } => enclosing_if_in_expr(value, target),
        Statement::Return { value: Some(v), .. } => enclosing_if_in_expr(v, target),
        Statement::Expression { expression, .. } => enclosing_if_in_expr(expression, target),
        Statement::Function { body, .. } | Statement::Module { body, .. } => {
            for s in &body.statements {
                if let Some(span) = enclosing_if_in_stmt(s, target) {
                    return Some(span);
                }
            }
            None
        }
        _ => None,
    }
}

fn enclosing_if_in_expr(expr: &Expression, target: FluxPosition) -> Option<FluxSpan> {
    if let Expression::If {
        condition,
        consequence,
        alternative,
        span,
        ..
    } = expr
    {
        // If the cursor is anywhere inside the alternative block, this
        // is the matching `if`. Return the if-expression's overall span
        // (start of `if`, just before the `else` keyword position).
        if let Some(alt) = alternative
            && alt
                .statements
                .iter()
                .any(|s| position_in_span(target, s.span()))
        {
            return Some(*span);
        }
        // Recurse so chained `else if` finds the innermost match.
        if let Some(inner) = enclosing_if_in_expr(condition, target) {
            return Some(inner);
        }
        for s in &consequence.statements {
            if let Some(inner) = enclosing_if_in_stmt(s, target) {
                return Some(inner);
            }
        }
        if let Some(alt) = alternative {
            for s in &alt.statements {
                if let Some(inner) = enclosing_if_in_stmt(s, target) {
                    return Some(inner);
                }
            }
        }
    }
    None
}

fn enclosing_handle_span(program: &Program, target: FluxPosition) -> Option<FluxSpan> {
    for stmt in &program.statements {
        if let Some(span) = enclosing_handle_in_stmt(stmt, target) {
            return Some(span);
        }
    }
    None
}

fn enclosing_handle_in_stmt(stmt: &Statement, target: FluxPosition) -> Option<FluxSpan> {
    match stmt {
        Statement::Let { value, .. }
        | Statement::Assign { value, .. }
        | Statement::LetDestructure { value, .. } => enclosing_handle_in_expr(value, target),
        Statement::Return { value: Some(v), .. } => enclosing_handle_in_expr(v, target),
        Statement::Expression { expression, .. } => enclosing_handle_in_expr(expression, target),
        Statement::Function { body, .. } | Statement::Module { body, .. } => {
            for s in &body.statements {
                if let Some(span) = enclosing_handle_in_stmt(s, target) {
                    return Some(span);
                }
            }
            None
        }
        _ => None,
    }
}

fn enclosing_handle_in_expr(expr: &Expression, target: FluxPosition) -> Option<FluxSpan> {
    if let Expression::Handle { span, arms, .. } = expr
        && (position_in_span(target, *span)
            || arms.iter().any(|a| position_in_span(target, a.body.span())))
    {
        return Some(*span);
    }
    None
}

/// Pull the ADT identifier out of `expr`'s inferred type, if any. The
/// hover handler uses the same shape — both `Con(Adt(s))` and `App(Adt(s),
/// _)` carry the data-type symbol.
fn inferred_adt_symbol(snapshot: &Snapshot, expr: &Expression) -> Option<Identifier> {
    let infer = snapshot.infer.as_ref()?;
    adt_symbol_of_infer_type(infer.expr_types.get(&expr.expr_id())?)
}

/// The ADT symbol carried by an inferred type, if it is one. Handles both the
/// nullary `Con(Adt(s))` and applied `App(Adt(s), _)` forms.
pub(crate) fn adt_symbol_of_infer_type(ty: &InferType) -> Option<Identifier> {
    match ty {
        InferType::Con(TypeConstructor::Adt(s)) | InferType::App(TypeConstructor::Adt(s), _) => {
            Some(*s)
        }
        _ => None,
    }
}

/// Walk `program` for the `data` decl that declares `adt_sym` and find
/// the named field `field`. Returns the synthesized span of the field
/// name in the declaration source. Mirrors the locator's
/// `DataFieldName` span synthesis so goto-def lands in the same place
/// hover and document-symbols use.
///
/// `adt_sym` may name either the data-type or a specific variant (the
/// snapshot's `variant_fields` is keyed both ways for the common
/// `data X { X { ... } }` pattern).
/// Return `(variant_span, field_name_span)` for the `field` declaration
/// inside the variant declaring `adt_sym`. The variant span is the
/// `full_range` (covers the whole `Variant { name: Type, ... }` block);
/// the field-name span is the `focus_range`, synthesized the same way
/// the locator does for `DataFieldName`.
fn find_data_field_spans(
    program: &Program,
    interner: &Interner,
    adt_sym: Identifier,
    field: Identifier,
) -> Option<(FluxSpan, FluxSpan)> {
    for stmt in &program.statements {
        let Statement::Data {
            name: data_name,
            variants,
            ..
        } = stmt
        else {
            continue;
        };
        for variant in variants {
            if variant.name != adt_sym && *data_name != adt_sym {
                continue;
            }
            let Some(names) = &variant.field_names else {
                continue;
            };
            for (field_name, ty) in names.iter().zip(variant.fields.iter()) {
                if *field_name != field {
                    continue;
                }
                let name_text = interner.try_resolve(*field_name)?;
                let ty_span = type_expr_span(ty);
                // Same approximation the locator uses: field name sits
                // `name_text.len() + 2` chars before the type span starts
                // (the `+ 2` accounts for `: ` between name and type).
                let start_col = ty_span.start.column.saturating_sub(name_text.len() + 2);
                let focus = FluxSpan {
                    start: FluxPosition {
                        line: ty_span.start.line,
                        column: start_col,
                    },
                    end: FluxPosition {
                        line: ty_span.start.line,
                        column: start_col + name_text.len(),
                    },
                };
                return Some((variant.span, focus));
            }
        }
    }
    None
}

/// Walk top-level functions for the first effect-row-variable binding of
/// `target` (`fn f() with E, |e`). Returns
/// `(function_signature_span, row_var_span)`. Picks the earliest such
/// binding in source order — `EffectExpr::RowVar` only appears at
/// row-tail positions, so multiple occurrences within one signature all
/// refer to the same binder.
fn find_row_var_binding_with_fn(
    program: &Program,
    target: Identifier,
) -> Option<(FluxSpan, FluxSpan)> {
    for stmt in &program.statements {
        if let Statement::Function { span, effects, .. } = stmt {
            for eff in effects {
                if let Some(row_span) = row_var_span(eff, target) {
                    return Some((*span, row_span));
                }
            }
        }
    }
    None
}

fn row_var_span(eff: &EffectExpr, target: Identifier) -> Option<FluxSpan> {
    match eff {
        EffectExpr::RowVar { name, span } if *name == target => Some(*span),
        EffectExpr::Add { left, right, .. } | EffectExpr::Subtract { left, right, .. } => {
            row_var_span(left, target).or_else(|| row_var_span(right, target))
        }
        _ => None,
    }
}

/// `TypeExpr` doesn't expose `span()` directly; pull it via the variant.
fn type_expr_span(ty: &TypeExpr) -> FluxSpan {
    match ty {
        TypeExpr::Named { span, .. }
        | TypeExpr::Tuple { span, .. }
        | TypeExpr::Function { span, .. } => *span,
    }
}

/// Translate the identifier on the left of a member access into the
/// `module_programs` lookup key. Two cases produce a hit:
///
/// 1. `object_id` directly resolves to a loaded module's short name
///    (e.g. user wrote `Math.sqrt` and `Math` is loaded).
/// 2. `object_id` is the alias of an `import X.Y as A` statement — return
///    the qualified module's short final segment (e.g. alias `A` →
///    `"Array"` for `import Flow.Array as A`).
fn resolve_module_short_name(snapshot: &Snapshot, object_id: Identifier) -> Option<String> {
    let direct = snapshot.interner.try_resolve(object_id)?;
    if snapshot.module_programs.contains_key(direct) {
        return Some(direct.to_string());
    }
    for stmt in &snapshot.program.statements {
        if let Statement::Import {
            name,
            alias: Some(a),
            ..
        } = stmt
            && *a == object_id
        {
            let qualified = snapshot.interner.try_resolve(*name)?;
            let short = qualified.rsplit('.').next().unwrap_or(qualified);
            return Some(short.to_string());
        }
    }
    None
}

/// Flatten a pure member-access chain (`A.B.C`, built only from
/// `Identifier`s and `MemberAccess`es) into its dotted string. Returns
/// `None` for any expression that is not a plain qualified path — a call,
/// index, or literal somewhere in the chain disqualifies it. Used to match
/// a deeply-qualified module reference against the `module_programs` key,
/// which is the module's full declared name.
fn module_path_string(expr: &Expression, interner: &Interner) -> Option<String> {
    match expr {
        Expression::Identifier { name, .. } => interner.try_resolve(*name).map(str::to_string),
        Expression::MemberAccess { object, member, .. } => {
            let base = module_path_string(object, interner)?;
            let segment = interner.try_resolve(*member)?;
            Some(format!("{base}.{segment}"))
        }
        _ => None,
    }
}

/// Goto-definition target for an `import` statement: the imported module's
/// `module` declaration in its own file. Returns `None` when the module is
/// not loaded into `module_programs` (an unresolved import), so the caller
/// falls back to the in-file import statement.
fn import_module_target(snapshot: &Snapshot, qualified: Identifier) -> Option<NavigationTarget> {
    let module_name = snapshot.interner.try_resolve(qualified)?;
    // `module_programs` keys user modules by their full declared name and
    // Flow stdlib modules by their short final segment — try both.
    let (mod_program, mod_source, mod_path) =
        snapshot.module_programs.get(module_name).or_else(|| {
            let short = module_name.rsplit('.').next().unwrap_or(module_name);
            snapshot.module_programs.get(short)
        })?;
    let module_uri = crate::vfs::path_to_uri(mod_path)?;
    let mod_map = PositionMap::new(
        std::sync::Arc::from(mod_source.as_ref()),
        snapshot.position_map.encoding(),
    );
    let mod_index = SymbolIndex::build_for_module_file(mod_program, &snapshot.interner);
    if let Some(entry) = mod_index.lookup_id(qualified) {
        return Some(target_from_entry(module_uri, &mod_map, entry));
    }
    // No `module` declaration matched (e.g. a plain-script module file) —
    // open the file at its top.
    Some(NavigationTarget::collapsed(
        module_uri,
        lsp_types::Range::default(),
        module_name.to_string(),
    ))
}

/// Walk top-level imports for one whose alias equals `target`. Returns
/// `(import_statement_span, alias_identifier_span)`. The alias span is
/// synthesized as the trailing `alias_text.len()` characters of the
/// statement — `import Foo as A` ends with the alias text.
fn find_import_alias_spans(
    program: &Program,
    interner: &Interner,
    target: Identifier,
) -> Option<(FluxSpan, FluxSpan)> {
    for stmt in &program.statements {
        if let Statement::Import {
            alias: Some(a),
            span,
            ..
        } = stmt
            && *a == target
        {
            let alias_text = interner.try_resolve(*a)?;
            let alias_len = alias_text.len();
            let alias_start = FluxPosition {
                line: span.end.line,
                column: span.end.column.saturating_sub(alias_len),
            };
            let alias_span = FluxSpan {
                start: alias_start,
                end: span.end,
            };
            return Some((*span, alias_span));
        }
    }
    None
}

/// Find the import statement whose alias identifier equals `target`,
/// returning the statement's span. Used by the `ImportAlias` NodeRef
/// branch to compute the `full_range`.
fn find_import_stmt_span_by_alias(program: &Program, target: Identifier) -> Option<FluxSpan> {
    for stmt in &program.statements {
        if let Statement::Import {
            alias: Some(a),
            span,
            ..
        } = stmt
            && *a == target
        {
            return Some(*span);
        }
    }
    None
}

/// Find the import statement whose qualified module name equals `target`,
/// returning the statement's span. Used by the `ImportName` NodeRef
/// branch to compute the `full_range`.
fn find_import_stmt_span_by_qualified(program: &Program, target: Identifier) -> Option<FluxSpan> {
    for stmt in &program.statements {
        if let Statement::Import { name, span, .. } = stmt
            && *name == target
        {
            return Some(*span);
        }
    }
    None
}

/// Walk the program for a non-top-level binding (let/fn/param inside a
/// function body) of `target`. Returns `(full_span, focus_span)` —
/// `full_span` is the entire statement, `focus_span` is the identifier
/// position computed the same way the locator emits `DeclName` /
/// `FunctionParameter` synthesized spans.
fn find_local_definition(
    program: &Program,
    interner: &Interner,
    target: Identifier,
) -> Option<(FluxSpan, FluxSpan)> {
    for stmt in &program.statements {
        if let Some(pair) = find_in_stmt(stmt, interner, target) {
            return Some(pair);
        }
    }
    None
}

fn find_in_stmt(
    stmt: &Statement,
    interner: &Interner,
    target: Identifier,
) -> Option<(FluxSpan, FluxSpan)> {
    match stmt {
        Statement::Let {
            is_public,
            name,
            span,
            ..
        } if *name == target => {
            let focus = name_span_after_keyword(*span, *is_public, "let", interner, *name);
            Some((*span, focus))
        }
        Statement::LetDestructure { pattern, span, .. } => {
            find_in_pattern(pattern, target, *span).map(|focus| (*span, focus))
        }
        Statement::Function {
            is_public,
            name,
            span,
            parameters,
            body,
            ..
        } => {
            if *name == target {
                let focus = name_span_after_keyword(*span, *is_public, "fn", interner, *name);
                return Some((*span, focus));
            }
            if parameters.contains(&target) {
                // The parser doesn't record per-parameter name spans —
                // collapse focus = full (the function signature) and let
                // the user land on the signature line.
                return Some((*span, *span));
            }
            find_in_block(body, interner, target)
        }
        Statement::Module { body, .. } => find_in_block(body, interner, target),
        _ => None,
    }
}

/// Synthesize an identifier-name span for a `let`/`fn` declaration by
/// reusing `decl_name_start` logic. The full statement span gives the
/// keyword start; the focus span covers just the identifier text.
fn name_span_after_keyword(
    stmt_span: FluxSpan,
    is_public: bool,
    keyword: &str,
    interner: &Interner,
    name: Identifier,
) -> FluxSpan {
    let start = crate::locator::decl_name_start(stmt_span.start, is_public, keyword);
    let len = interner.try_resolve(name).map(str::len).unwrap_or(0);
    FluxSpan {
        start,
        end: FluxPosition {
            line: start.line,
            column: start.column + len,
        },
    }
}

fn find_in_pattern(pat: &Pattern, target: Identifier, binding_span: FluxSpan) -> Option<FluxSpan> {
    match pat {
        Pattern::Identifier { name, .. } if *name == target => Some(binding_span),
        Pattern::Tuple { elements, .. } => elements
            .iter()
            .find_map(|e| find_in_pattern(e, target, binding_span)),
        Pattern::Constructor { fields, .. } => fields
            .iter()
            .find_map(|f| find_in_pattern(f, target, binding_span)),
        Pattern::NamedConstructor { fields, .. } => fields.iter().find_map(|f| {
            f.pattern
                .as_ref()
                .and_then(|p| find_in_pattern(p, target, binding_span))
        }),
        Pattern::Some { pattern, .. }
        | Pattern::Left { pattern, .. }
        | Pattern::Right { pattern, .. } => find_in_pattern(pattern, target, binding_span),
        Pattern::Cons { head, tail, .. } => find_in_pattern(head, target, binding_span)
            .or_else(|| find_in_pattern(tail, target, binding_span)),
        _ => None,
    }
}

fn find_in_block(
    block: &Block,
    interner: &Interner,
    target: Identifier,
) -> Option<(FluxSpan, FluxSpan)> {
    for stmt in &block.statements {
        if let Some(pair) = find_in_stmt(stmt, interner, target) {
            return Some(pair);
        }
    }
    None
}

/// Map a `NodeRef` to the identifier whose definition F12 should jump to.
/// Returns `None` for nodes that don't represent a navigable reference.
fn definition_name(node: &NodeRef) -> Option<Identifier> {
    match node {
        NodeRef::Expr(Expression::Identifier { name, .. }) => Some(*name),
        NodeRef::Pattern(Pattern::Identifier { name, .. }) => Some(*name),
        NodeRef::NamedConstructorName { name, .. } => Some(*name),
        NodeRef::TypeExprNamed { name, .. } => Some(*name),
        NodeRef::EffectName { name, .. } => Some(*name),
        // Effect operation references — jump to op declaration in effect block.
        NodeRef::PerformOpName { name, .. }
        | NodeRef::HandleArmOpName { name, .. }
        | NodeRef::EffectOpName { name, .. } => Some(*name),
        // Data variant references — jump to variant declaration in data block.
        NodeRef::DataVariantName { name, .. } => Some(*name),
        // Decl-site nodes: F12 stays on the same identifier (its own
        // definition is itself).
        NodeRef::DataName { name, .. }
        | NodeRef::EffectDeclName { name, .. }
        | NodeRef::DeclName { name, .. } => Some(*name),
        _ => None,
    }
}
