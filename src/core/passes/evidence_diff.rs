//! Compare the evidence emitter with the dictionaries the existing paths chose
//! (0.0.8 plan, step 1c).
//!
//! The emitter runs in shadow mode: it records, per occurrence, the
//! dictionaries the solver's evidence says to pass, and lowering hands that
//! record back as an [`EvidenceShadow`]. The existing answers are spread over
//! AST lowering and dictionary elaboration, so they are read from the Core
//! that comes out of elaboration, where all of them are visible:
//!
//! - a call whose leading arguments are dictionaries — `f(__dict_Num_Int, x)`;
//! - a call dispatched straight to an instance method — `__tc_Show_Int_show(x)`;
//! - a method projected out of a dictionary parameter — `__dict_Ord.0(x, y)`.
//!
//! Core carries spans rather than expression ids, so the two sides meet by
//! span: the occurrence's own, or the span of the call it is the callee of.
//!
//! Enabled by `FLUX_DBG_EVIDENCE_DIFF`; `FLUX_DBG_EVIDENCE_DIFF=all` also lists
//! the sites that agree. Every line other than `agree` is something to explain
//! before evidence becomes the only source (step 1e).

use std::collections::HashMap;

use crate::{
    core::{
        CoreExpr, CoreProgram,
        lower_ast::{EmittedDictArgs, EmittedSite, EvidenceShadow, OccurrenceKind},
    },
    source::position::Span,
    syntax::interner::Interner,
    types::{
        class_env::{dictionary_name, dictionary_prefix, is_generated_instance_method},
        evidence::EvidenceMap,
        translate::DictArg,
    },
};

/// The evidence a compile unit's lowering reads, and the unit it belongs to.
#[derive(Clone, Copy)]
pub struct EvidenceSource<'a> {
    pub map: &'a EvidenceMap,
    pub file_path: &'a str,
    /// Which lowering entry point is reporting. A unit can be lowered by more
    /// than one, and they need not agree.
    pub via: &'static str,
}

impl EvidenceSource<'_> {
    /// Print the comparison if `FLUX_DBG_EVIDENCE_DIFF` asks for it.
    pub fn report_if_enabled(
        &self,
        core: &CoreProgram,
        shadow: &EvidenceShadow,
        interner: &Interner,
    ) {
        if let Ok(mode) = std::env::var("FLUX_DBG_EVIDENCE_DIFF") {
            let label = format!("{} via {}", self.file_path, self.via);
            report_evidence_diff(core, shadow, self.map, interner, &label, mode == "all");
        }
    }
}

/// `Span` is not `Hash`; its four coordinates are.
type SpanKey = (usize, usize, usize, usize);

fn key(span: Span) -> Option<SpanKey> {
    (span != Span::default()).then_some((
        span.start.line,
        span.start.column,
        span.end.line,
        span.end.column,
    ))
}

/// What the existing paths produced at one call.
#[derive(Debug, Clone, PartialEq, Eq)]
enum OldAnswer {
    /// A call, with the dictionaries it passes ahead of its arguments.
    Passed(Vec<String>),
    /// A call dispatched straight to a generated instance method.
    Direct { method: String, passed: Vec<String> },
    /// A method projected out of a dictionary: the dictionary, followed by
    /// the superclass slots walked to reach the method's class.
    Projected(String),
}

/// How the emitter's answer at a site compares with the existing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Verdict {
    /// Both sides pass the same dictionaries.
    Agree,
    /// Both sides passed dictionaries, and they are different ones.
    Differ,
    /// The evidence has a hole, or an answer that cannot be translated.
    Unbuildable,
    /// The evidence says to pass dictionaries; the existing paths built no
    /// call here. A constrained function passed as a value (KI-090).
    Reference,
    /// The evidence says an operator takes a dictionary; the existing paths
    /// lower it to a primitive. Expected today, so only counted.
    Operator,
    /// The solver raised predicates at an expression the emitter never
    /// lowered, so the keying in `types/evidence.rs` missed a case.
    Unreached,
    /// The occurrence's id names a different expression in the evidence map.
    Mismatched,
}

impl Verdict {
    fn label(self) -> &'static str {
        match self {
            Verdict::Agree => "agree",
            Verdict::Differ => "differ",
            Verdict::Unbuildable => "unbuildable",
            Verdict::Reference => "reference",
            Verdict::Operator => "operator",
            Verdict::Unreached => "unreached",
            Verdict::Mismatched => "mismatched",
        }
    }

    /// Whether a site with this verdict is listed by default.
    fn listed(self) -> bool {
        !matches!(self, Verdict::Agree | Verdict::Operator)
    }
}

/// Print the comparison for one program to stderr.
pub fn report_evidence_diff(
    core: &CoreProgram,
    shadow: &EvidenceShadow,
    evidence: &EvidenceMap,
    interner: &Interner,
    file_path: &str,
    list_all: bool,
) {
    let mut old = HashMap::new();
    for def in &core.defs {
        collect_old_answers(&def.expr, interner, &mut old);
    }

    let mut lines: Vec<(Span, Verdict, String)> = Vec::new();
    for (id, site) in &shadow.sites {
        let found = key(site.span).and_then(|k| old.get(&k)).or_else(|| {
            shadow
                .call_spans
                .get(id)
                .and_then(|span| key(*span))
                .and_then(|k| old.get(&k))
        });
        let (verdict, detail) = compare(site, found, evidence, interner);
        lines.push((site.span, verdict, detail));
    }
    for (id, raised) in evidence.raised_sites() {
        if shadow.sites.contains_key(&id) {
            continue;
        }
        let predicate = evidence.predicate(&crate::types::evidence::EvidenceSite::new(id, 0));
        let span = predicate.map(|p| p.span).unwrap_or_default();
        let detail = match predicate {
            Some(p) => format!(
                "{} {:?}, raised={raised}",
                interner.resolve(p.class_name),
                p.origin
            ),
            None => format!("raised={raised}"),
        };
        lines.push((span, Verdict::Unreached, detail));
    }

    lines.sort_by_key(|(span, verdict, detail)| {
        (span.start.line, span.start.column, *verdict, detail.clone())
    });
    let mut counts: HashMap<Verdict, usize> = HashMap::new();
    for (_, verdict, _) in &lines {
        *counts.entry(*verdict).or_default() += 1;
    }
    let summary = [
        Verdict::Agree,
        Verdict::Differ,
        Verdict::Unbuildable,
        Verdict::Reference,
        Verdict::Operator,
        Verdict::Unreached,
        Verdict::Mismatched,
    ]
    .iter()
    .map(|v| format!("{} {}", v.label(), counts.get(v).copied().unwrap_or(0)))
    .collect::<Vec<_>>()
    .join(", ");
    eprintln!(
        "EVIDENCE DIFF for {file_path}: {} sites — {summary}",
        lines.len()
    );
    for (span, verdict, detail) in &lines {
        if list_all || verdict.listed() {
            eprintln!(
                "  {:<11} {}:{}  {detail}",
                verdict.label(),
                span.start.line,
                span.start.column
            );
        }
    }
}

fn compare(
    site: &EmittedSite,
    found: Option<&OldAnswer>,
    evidence: &EvidenceMap,
    interner: &Interner,
) -> (Verdict, String) {
    let what = match site.kind {
        OccurrenceKind::Identifier(name) => format!("`{}`", interner.resolve(name)),
        OccurrenceKind::Operator => "operator".to_string(),
    };
    let args = match &site.args {
        EmittedDictArgs::Built(args) => args,
        EmittedDictArgs::Unbuildable => {
            let old = found.map(describe_old).unwrap_or_else(|| "none".into());
            return (Verdict::Unbuildable, format!("{what}  old={old}"));
        }
        EmittedDictArgs::Mismatched { raised_at } => {
            return (
                Verdict::Mismatched,
                format!(
                    "{what}  id's predicates were raised at {}:{}",
                    raised_at.start.line, raised_at.start.column
                ),
            );
        }
    };
    let rendered = args
        .iter()
        .map(|arg| render_dict_arg(arg, evidence, interner))
        .collect::<Vec<_>>();
    let built = format!("[{}]", rendered.join(", "));

    let Some(found) = found else {
        let verdict = match (rendered.is_empty(), site.kind) {
            (true, _) => Verdict::Agree,
            (false, OccurrenceKind::Operator) => Verdict::Operator,
            (false, OccurrenceKind::Identifier(_)) => Verdict::Reference,
        };
        return (verdict, format!("{what}  evidence={built} old=none"));
    };

    let agree = match found {
        OldAnswer::Passed(passed) => *passed == rendered,
        OldAnswer::Direct { method, passed } => match (args.as_slice(), site.kind) {
            ([DictArg::Global { instance }], OccurrenceKind::Identifier(name))
            | ([DictArg::Applied { instance, .. }], OccurrenceKind::Identifier(name)) => {
                let expected = crate::types::class_env::mangled_method_name(
                    instance.class_id,
                    &instance.dict_type_key,
                    interner.resolve(name),
                    interner,
                );
                let context = match &args[0] {
                    DictArg::Applied { context, .. } => context
                        .iter()
                        .map(|arg| render_dict_arg(arg, evidence, interner))
                        .collect(),
                    _ => Vec::new(),
                };
                *method == expected && (passed.is_empty() || *passed == context)
            }
            _ => false,
        },
        OldAnswer::Projected(dictionary) => rendered.as_slice() == [dictionary.clone()],
    };
    let verdict = if agree {
        Verdict::Agree
    } else {
        Verdict::Differ
    };
    (
        verdict,
        format!("{what}  evidence={built} old={}", describe_old(found)),
    )
}

fn describe_old(answer: &OldAnswer) -> String {
    match answer {
        OldAnswer::Passed(passed) => format!("[{}]", passed.join(", ")),
        OldAnswer::Direct { method, passed } if passed.is_empty() => format!("direct {method}"),
        OldAnswer::Direct { method, passed } => {
            format!("direct {method} [{}]", passed.join(", "))
        }
        OldAnswer::Projected(dictionary) => format!("projected from {dictionary}"),
    }
}

/// Render a dictionary argument the way the existing paths name it in Core.
///
/// A parameter is named as dictionary elaboration names it: the class's
/// dictionary prefix, with a suffix for the second and later dictionary of
/// one class in *its owner's* parameters (`__dict_Enc`, `__dict_Enc_1`).
fn render_dict_arg(arg: &DictArg, evidence: &EvidenceMap, interner: &Interner) -> String {
    match arg {
        DictArg::Global { instance } => {
            dictionary_name(instance.class_id, &instance.dict_type_key, interner)
        }
        DictArg::Applied { instance, context } => format!(
            "{}({})",
            dictionary_name(instance.class_id, &instance.dict_type_key, interner),
            context
                .iter()
                .map(|arg| render_dict_arg(arg, evidence, interner))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        DictArg::Param { owner, index, path } => {
            let Some(params) = evidence.definition(*owner).map(|d| &d.params) else {
                return format!("<param {owner:?}#{index}>");
            };
            let Some(given) = params.get(*index) else {
                return format!("<param {owner:?}#{index} of {}>", params.len());
            };
            let occurrence = params[..*index]
                .iter()
                .filter(|earlier| earlier.class_id == given.class_id)
                .count();
            let mut name = dictionary_prefix(given.class_id, interner);
            if occurrence > 0 {
                name.push_str(&format!("_{occurrence}"));
            }
            for slot in path {
                name.push_str(&format!(".{slot}"));
            }
            name
        }
        DictArg::None => "<none>".to_string(),
    }
}

/// Render a Core expression as a dictionary, or `None` if it is not one.
fn render_old_dict(expr: &CoreExpr, interner: &Interner) -> Option<String> {
    match expr {
        CoreExpr::Var { var, .. } => {
            let name = interner.try_resolve(var.name)?;
            name.starts_with("__dict_").then(|| name.to_string())
        }
        CoreExpr::App { func, args, .. } => {
            let head = render_old_dict(func, interner)?;
            let args = args
                .iter()
                .map(|arg| render_old_dict(arg, interner))
                .collect::<Option<Vec<_>>>()?;
            Some(format!("{head}({})", args.join(", ")))
        }
        CoreExpr::TupleField { object, index, .. } => {
            Some(format!("{}.{index}", render_old_dict(object, interner)?))
        }
        _ => None,
    }
}

/// Record what every call in `expr` passes, keyed by the callee's span and by
/// the call's.
fn collect_old_answers(
    expr: &CoreExpr,
    interner: &Interner,
    out: &mut HashMap<SpanKey, OldAnswer>,
) {
    if let CoreExpr::App { func, args, span } = expr
        && let Some(answer) = old_answer(func, args, interner)
    {
        let func_span = match func.as_ref() {
            CoreExpr::Var { span, .. } | CoreExpr::MemberAccess { span, .. } => Some(*span),
            _ => None,
        };
        for k in [func_span, Some(*span)]
            .into_iter()
            .flatten()
            .filter_map(key)
        {
            out.entry(k).or_insert_with(|| answer.clone());
        }
    }
    for child in children(expr) {
        collect_old_answers(child, interner, out);
    }
}

fn old_answer(func: &CoreExpr, args: &[CoreExpr], interner: &Interner) -> Option<OldAnswer> {
    let passed = || {
        args.iter()
            .map_while(|arg| render_old_dict(arg, interner))
            .collect::<Vec<_>>()
    };
    match func {
        CoreExpr::Var { var, .. } => {
            let name = interner.try_resolve(var.name)?;
            if name.starts_with("__dict_") {
                // A contextual dictionary being applied, not a call.
                return None;
            }
            if is_generated_instance_method(name) {
                return Some(OldAnswer::Direct {
                    method: name.to_string(),
                    passed: passed(),
                });
            }
            Some(OldAnswer::Passed(passed()))
        }
        // A module-qualified call: `Array.sort(d, xs)`.
        CoreExpr::MemberAccess { .. } => Some(OldAnswer::Passed(passed())),
        CoreExpr::TupleField { object, .. } => {
            // The last slot is the method; the ones before it walk superclass
            // evidence, which is what a `DictArg::Param` path names.
            let dictionary = render_old_dict(object, interner)?;
            Some(OldAnswer::Projected(dictionary))
        }
        _ => None,
    }
}

fn children(expr: &CoreExpr) -> Vec<&CoreExpr> {
    match expr {
        CoreExpr::Var { .. } | CoreExpr::Lit(..) => Vec::new(),
        CoreExpr::Lam { body, .. } | CoreExpr::Return { value: body, .. } => vec![body],
        CoreExpr::App { func, args, .. } => {
            let mut out = vec![func.as_ref()];
            out.extend(args);
            out
        }
        CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => vec![rhs, body],
        CoreExpr::LetRecGroup { bindings, body, .. } => {
            let mut out: Vec<&CoreExpr> = bindings.iter().map(|(_, rhs)| rhs.as_ref()).collect();
            out.push(body);
            out
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            let mut out = vec![scrutinee.as_ref()];
            for alt in alts {
                out.extend(alt.guard.as_ref());
                out.push(&alt.rhs);
            }
            out
        }
        CoreExpr::Con { fields, .. } | CoreExpr::PrimOp { args: fields, .. } => {
            fields.iter().collect()
        }
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            vec![object]
        }
        CoreExpr::Perform { args, .. } => args.iter().collect(),
        CoreExpr::Handle {
            body,
            parameter,
            handlers,
            ..
        } => {
            let mut out = vec![body.as_ref()];
            out.extend(parameter.as_deref());
            out.extend(handlers.iter().map(|handler| &handler.body));
            out
        }
    }
}
