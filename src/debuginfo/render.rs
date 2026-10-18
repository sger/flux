//! The readable `--dump-debug-info` rendering of a [`ProgramDebugInfo`].

use std::fmt::Write;

use crate::diagnostics::position::Span;

use super::model::{DebugFunction, DebugType, DebugTypeId, FnKind, ProgramDebugInfo, VarKind};

/// Renders `info` as text. The output is deterministic: functions, variables
/// and declarations appear in the order the module defines them.
pub fn render_program_debug_info(info: &ProgramDebugInfo) -> String {
    let mut out = String::new();
    for file in &info.files {
        let _ = writeln!(
            out,
            "debug info: {}",
            crate::shared::paths::without_verbatim_prefix(&file.path)
        );
    }
    for function in &info.functions {
        render_function(&mut out, info, function);
    }
    for ty in &info.types {
        if let DebugType::Adt { name, ctors, .. } = ty
            && !ctors.is_empty()
        {
            let _ = writeln!(out, "data {name}");
            for ctor in ctors {
                let fields = ctor
                    .fields
                    .iter()
                    .map(|field| match &field.name {
                        Some(name) => format!("{name}: {}", field.ty),
                        None => field.ty.clone(),
                    })
                    .collect::<Vec<_>>();
                if fields.is_empty() {
                    let _ = writeln!(out, "  {}", ctor.name);
                } else {
                    let _ = writeln!(out, "  {}({})", ctor.name, fields.join(", "));
                }
            }
        }
    }
    for effect in &info.effects {
        let _ = writeln!(
            out,
            "effect {} @{}: {}",
            effect.name,
            position(effect.span),
            effect.ops.join(", ")
        );
    }
    out
}

fn render_function(out: &mut String, info: &ProgramDebugInfo, function: &DebugFunction) {
    let kind = match function.kind {
        FnKind::User => "fn".to_string(),
        FnKind::Lambda { parent } => {
            format!(
                "lambda in {}",
                info.functions[parent.0 as usize].source_name
            )
        }
        FnKind::DictDef => "dictionary".to_string(),
        FnKind::DictMethod => "instance method".to_string(),
    };
    let artificial = if function.artificial {
        ", artificial"
    } else {
        ""
    };
    let _ = writeln!(
        out,
        "{} [{kind}{artificial}] @{}",
        function.source_name,
        position(function.decl_span)
    );
    for var in &function.vars {
        let kind = match var.kind {
            VarKind::Param => "param",
            VarKind::Let => "let",
            VarKind::PatternBound => "pattern",
            VarKind::HandlerParam => "handler",
            VarKind::Evidence => "evidence",
        };
        let artificial = if var.artificial { " (artificial)" } else { "" };
        let _ = writeln!(
            out,
            "  {kind} {}: {} @{}{artificial}",
            var.name,
            render_type(info, var.ty),
            position(var.decl_span)
        );
    }
}

/// `line:column`, both 1-based, as editors show them; `?` for code the
/// compiler made, which has no position.
fn position(span: Span) -> String {
    if span.start.line == 0 {
        return "?".to_string();
    }
    format!("{}:{}", span.start.line, span.start.column + 1)
}

fn render_type(info: &ProgramDebugInfo, id: DebugTypeId) -> String {
    let list = |ids: &[DebugTypeId]| {
        ids.iter()
            .map(|id| render_type(info, *id))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match info.ty(id) {
        DebugType::Int => "Int".into(),
        DebugType::Float => "Float".into(),
        DebugType::Bool => "Bool".into(),
        DebugType::String => "String".into(),
        DebugType::Unit => "Unit".into(),
        DebugType::Never => "Never".into(),
        DebugType::List(elem) => format!("List<{}>", render_type(info, *elem)),
        DebugType::Array(elem) => format!("Array<{}>", render_type(info, *elem)),
        DebugType::Option(inner) => format!("Option<{}>", render_type(info, *inner)),
        DebugType::Either(left, right) => format!(
            "Either<{}, {}>",
            render_type(info, *left),
            render_type(info, *right)
        ),
        DebugType::Map(key, value) => format!(
            "Map<{}, {}>",
            render_type(info, *key),
            render_type(info, *value)
        ),
        DebugType::Tuple(elems) => format!("({})", list(elems)),
        DebugType::Function(params, result) => {
            format!("({}) -> {}", list(params), render_type(info, *result))
        }
        DebugType::Adt { name, args, .. } if args.is_empty() => name.clone(),
        DebugType::Adt { name, args, .. } => format!("{name}<{}>", list(args)),
        DebugType::Dynamic => "?".into(),
    }
}
