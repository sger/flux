//! `textDocument/signatureHelp` — the parameter hint popup shown while typing
//! a call's arguments.
//!
//! The signature label is built from the callee's inferred function type and
//! enriched with the real parameter names and `///` doc comment from the
//! callee's declaration: searched in this buffer for a direct call `foo(..)`,
//! or — for a qualified `M.foo(..)` — in the imported module's cached program
//! (`Snapshot::module_programs`, no workspace round-trip), so cross-module
//! callees get names too. It falls back to types only when the declaration
//! can't be found (e.g. a function imported unqualified via `exposing`). Each
//! parameter's span within the label is reported as `[start, end)` offsets (not
//! a substring) so the client highlights the *exact* active parameter even when
//! two share a type string, and the active index is clamped into range.

use flux::ast::type_infer::display_infer_type;
use flux::diagnostics::position::Span as FluxSpan;
use flux::syntax::Identifier;
use flux::syntax::expression::Expression;
use flux::syntax::interner::Interner;
use flux::syntax::program::Program;
use flux::syntax::statement::{ImportExposing, Statement};
use flux::types::infer_type::InferType;
use lsp_types::{
    Documentation, MarkupContent, MarkupKind, ParameterInformation, ParameterLabel, Position,
    SignatureHelp, SignatureInformation,
};

use crate::doc_comments::doc_comment_above;
use crate::handlers::hover::module_key_for;
use crate::locator::find_enclosing_call;
use crate::snapshot::Snapshot;

pub fn signature_help(snapshot: &Snapshot, position: Position) -> Option<SignatureHelp> {
    let target = snapshot.position_map.lsp_to_flux(position)?;
    let infer = snapshot.infer.as_ref()?;

    let (call_expr, active_param) = find_enclosing_call(&snapshot.program, target)?;

    let Expression::Call { function, .. } = call_expr else {
        return None;
    };

    let fn_ty = infer.expr_types.get(&function.expr_id())?;
    let InferType::Fun(param_types, ret_type, _) = fn_ty else {
        return None;
    };

    let name = callee_name(function, &snapshot.interner);
    // Parameter names + doc comment from the callee's declaration. `None` for
    // computed callees, or functions whose declaration we can't locate (e.g.
    // imported unqualified via `exposing`) — those show types only.
    let decl = name
        .as_deref()
        .and_then(|n| resolve_callee_decl(snapshot, function, n, param_types.len()));

    // Build the label, recording each parameter's [start, end) offset within it
    // in UTF-16 code units (the unit the LSP signature-help spec mandates).
    let mut label = String::new();
    if let Some(n) = &name {
        label.push_str(n);
    }
    label.push('(');
    let mut offsets: Vec<[u32; 2]> = Vec::with_capacity(param_types.len());
    for (idx, ty) in param_types.iter().enumerate() {
        if idx > 0 {
            label.push_str(", ");
        }
        let start = utf16_len(&label);
        if let Some(d) = &decl {
            label.push_str(&d.param_names[idx]);
            label.push_str(": ");
        }
        label.push_str(&display_infer_type(ty, &snapshot.interner));
        offsets.push([start, utf16_len(&label)]);
    }
    label.push_str(") -> ");
    label.push_str(&display_infer_type(ret_type, &snapshot.interner));

    let parameters: Vec<ParameterInformation> = offsets
        .iter()
        .map(|&offset| ParameterInformation {
            label: ParameterLabel::LabelOffsets(offset),
            documentation: None,
        })
        .collect();

    // Clamp the active parameter into range — a trailing comma or surplus
    // argument keeps the last parameter highlighted; a no-arg signature has
    // none.
    let active = param_types
        .len()
        .checked_sub(1)
        .map(|last| active_param.min(last) as u32);

    let documentation = decl.and_then(|d| d.doc).map(|doc| {
        Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: doc,
        })
    });

    Some(SignatureHelp {
        signatures: vec![SignatureInformation {
            label,
            documentation,
            parameters: Some(parameters),
            active_parameter: active,
        }],
        active_signature: Some(0),
        active_parameter: active,
    })
}

/// Parameter names and `///` doc comment recovered from a callee's declaration.
struct CalleeDecl {
    param_names: Vec<String>,
    doc: Option<String>,
}

/// Resolve the callee's declaration to its parameter names and doc comment.
///
/// A direct call `foo(..)` is looked up in this buffer, then in any module that
/// exposes `foo` unqualified (`import M exposing (foo)` / `(..)`); a qualified
/// call `M.foo(..)` is looked up in module `M`. All module lookups use the
/// cached program and source (`Snapshot::module_programs`) — already in hand, so
/// no workspace round-trip on a per-keystroke request. `None` when the
/// declaration can't be found.
fn resolve_callee_decl(
    snapshot: &Snapshot,
    function: &Expression,
    name: &str,
    arity: usize,
) -> Option<CalleeDecl> {
    let encoding = snapshot.position_map.encoding();
    if let Expression::MemberAccess { object, .. } = function {
        // Qualified `M.foo` — search the imported module's cached program.
        let key = module_key_for(snapshot, object)?;
        let (program, source, _) = snapshot.module_programs.get(&key)?;
        let (param_names, span) = function_decl(program, &snapshot.interner, name, arity)?;
        return Some(CalleeDecl {
            param_names,
            doc: doc_comment_above(source, span, encoding),
        });
    }
    // Direct `foo` — search this buffer first.
    if let Some((param_names, span)) =
        function_decl(&snapshot.program, &snapshot.interner, name, arity)
    {
        return Some(CalleeDecl {
            param_names,
            doc: doc_comment_above(snapshot.text.as_ref(), span, encoding),
        });
    }
    // Then any module that exposes `foo` unqualified.
    resolve_exposed_decl(snapshot, name, arity, encoding)
}

/// Find `name`'s declaration in a module that exposes it unqualified via an
/// `import M exposing (name)` or `import M exposing (..)` in this buffer.
fn resolve_exposed_decl(
    snapshot: &Snapshot,
    name: &str,
    arity: usize,
    encoding: crate::line_index::PositionEncoding,
) -> Option<CalleeDecl> {
    for stmt in &snapshot.program.statements {
        let Statement::Import {
            name: module_name,
            exposing,
            except,
            ..
        } = stmt
        else {
            continue;
        };
        let exposed = match exposing {
            ImportExposing::None => false,
            ImportExposing::All => !except
                .iter()
                .any(|e| snapshot.interner.try_resolve(*e) == Some(name)),
            ImportExposing::Names(names) => names
                .iter()
                .any(|n| snapshot.interner.try_resolve(*n) == Some(name)),
        };
        if !exposed {
            continue;
        }
        let Some(key) = module_program_key(snapshot, *module_name) else {
            continue;
        };
        let Some((program, source, _)) = snapshot.module_programs.get(&key) else {
            continue;
        };
        if let Some((param_names, span)) = function_decl(program, &snapshot.interner, name, arity) {
            return Some(CalleeDecl {
                param_names,
                doc: doc_comment_above(source, span, encoding),
            });
        }
    }
    None
}

/// The `module_programs` key for an `import` statement's module name: the full
/// declared name, or its short final segment. `None` if the module isn't loaded.
fn module_program_key(snapshot: &Snapshot, module_name: Identifier) -> Option<String> {
    let full = snapshot.interner.try_resolve(module_name)?;
    if snapshot.module_programs.contains_key(full) {
        return Some(full.to_string());
    }
    let short = full.rsplit('.').next().unwrap_or(full);
    if snapshot.module_programs.contains_key(short) {
        return Some(short.to_string());
    }
    None
}

/// The textual name of a call's callee: a direct `foo(..)` or the member of a
/// qualified `M.foo(..)`. `None` for a computed callee (e.g. `getFn()()`).
fn callee_name(function: &Expression, interner: &Interner) -> Option<String> {
    match function {
        Expression::Identifier { name, .. } => interner.try_resolve(*name).map(str::to_string),
        Expression::MemberAccess { member, .. } => {
            interner.try_resolve(*member).map(str::to_string)
        }
        _ => None,
    }
}

/// Parameter names and declaration span of the function `name` with `arity`
/// parameters, searched in this buffer (top-level or directly inside a
/// `module {}`). `None` when the name/arity don't match a buffer declaration.
fn function_decl(
    program: &Program,
    interner: &Interner,
    name: &str,
    arity: usize,
) -> Option<(Vec<String>, FluxSpan)> {
    fn matches(
        stmt: &Statement,
        interner: &Interner,
        name: &str,
        arity: usize,
    ) -> Option<(Vec<String>, FluxSpan)> {
        if let Statement::Function {
            name: fname,
            parameters,
            span,
            ..
        } = stmt
            && interner.try_resolve(*fname) == Some(name)
            && parameters.len() == arity
        {
            let names = parameters
                .iter()
                .map(|p| interner.try_resolve(*p).unwrap_or("_").to_string())
                .collect();
            return Some((names, *span));
        }
        None
    }

    for stmt in &program.statements {
        if let Some(found) = matches(stmt, interner, name, arity) {
            return Some(found);
        }
        if let Statement::Module { body, .. } = stmt {
            for inner in &body.statements {
                if let Some(found) = matches(inner, interner, name, arity) {
                    return Some(found);
                }
            }
        }
    }
    None
}

/// Length of `s` in UTF-16 code units — the offset unit the LSP signature-help
/// `ParameterLabel::LabelOffsets` form is measured in.
fn utf16_len(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}
