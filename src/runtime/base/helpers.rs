use std::collections::HashMap;

use crate::{
    runtime::{
        base::{
            base_hm_effect_row::BaseHmEffectRow, base_hm_signature::BaseHmSignature,
            base_hm_signature_id::BaseHmSignatureId, base_hm_type::BaseHmType,
        },
        value::Value,
    },
    syntax::{Identifier, interner::Interner},
    types::{
        TypeVarId, infer_effect_row::InferEffectRow, infer_type::InferType, scheme::Scheme,
        type_constructor::TypeConstructor,
    },
};

pub(super) fn format_hint(signature: &str) -> String {
    format!("\n\nHint:\n  {}", signature)
}

pub(super) fn arity_error(name: &str, expected: &str, got: usize, signature: &str) -> String {
    format!(
        "wrong number of arguments\n\n  function: {}/{}\n  expected: {}\n  got: {}{}",
        name,
        expected,
        expected,
        got,
        format_hint(signature)
    )
}

pub(super) fn type_error(
    name: &str,
    label: &str,
    expected: &str,
    got: &str,
    signature: &str,
) -> String {
    format!(
        "{} expected {} to be {}, got {}{}",
        name,
        label,
        expected,
        got,
        format_hint(signature)
    )
}

pub(super) fn check_arity(
    args: &[Value],
    expected: usize,
    name: &str,
    signature: &str,
) -> Result<(), String> {
    if args.len() != expected {
        return Err(arity_error(
            name,
            &expected.to_string(),
            args.len(),
            signature,
        ));
    }
    Ok(())
}

pub(super) fn check_arity_range(
    args: &[Value],
    min: usize,
    max: usize,
    name: &str,
    signature: &str,
) -> Result<(), String> {
    if args.len() < min || args.len() > max {
        return Err(arity_error(
            name,
            &format!("{}..{}", min, max),
            args.len(),
            signature,
        ));
    }
    Ok(())
}

pub(super) fn arg_string<'a>(
    args: &'a [Value],
    index: usize,
    name: &str,
    label: &str,
    signature: &str,
) -> Result<&'a str, String> {
    match &args[index] {
        Value::String(s) => Ok(s.as_ref()),
        other => Err(type_error(
            name,
            label,
            "String",
            other.type_name(),
            signature,
        )),
    }
}

pub(super) fn arg_array<'a>(
    args: &'a [Value],
    index: usize,
    name: &str,
    label: &str,
    signature: &str,
) -> Result<&'a Vec<Value>, String> {
    match &args[index] {
        Value::Array(arr) => Ok(arr),
        other => Err(type_error(
            name,
            label,
            "Array",
            other.type_name(),
            signature,
        )),
    }
}

pub(super) fn arg_int(
    args: &[Value],
    index: usize,
    name: &str,
    label: &str,
    signature: &str,
) -> Result<i64, String> {
    match &args[index] {
        Value::Integer(value) => Ok(*value),
        other => Err(type_error(
            name,
            label,
            "Integer",
            other.type_name(),
            signature,
        )),
    }
}

pub(super) fn arg_number(
    args: &[Value],
    index: usize,
    name: &str,
    label: &str,
    signature: &str,
) -> Result<f64, String> {
    match &args[index] {
        Value::Integer(v) => Ok(*v as f64),
        Value::Float(v) => Ok(*v),
        other => Err(type_error(
            name,
            label,
            "Number",
            other.type_name(),
            signature,
        )),
    }
}

pub(super) fn lower_type(
    ty: &BaseHmType,
    type_params: &mut HashMap<&'static str, TypeVarId>,
    row_params: &mut HashMap<&'static str, TypeVarId>,
    next_var: &mut TypeVarId,
    interner: &mut Interner,
) -> Result<InferType, String> {
    let out = match ty {
        BaseHmType::Any => InferType::Con(TypeConstructor::Any),
        BaseHmType::Int => InferType::Con(TypeConstructor::Int),
        BaseHmType::Float => InferType::Con(TypeConstructor::Float),
        BaseHmType::Bool => InferType::Con(TypeConstructor::Bool),
        BaseHmType::String => InferType::Con(TypeConstructor::String),
        BaseHmType::Unit => InferType::Con(TypeConstructor::Unit),
        BaseHmType::TypeVar(name) => {
            let id = *type_params.entry(name).or_insert_with(|| {
                let var = *next_var;
                *next_var = next_var.saturating_add(1);
                var
            });
            InferType::Var(id)
        }
        BaseHmType::Option(inner) => InferType::App(
            TypeConstructor::Option,
            vec![lower_type(
                inner,
                type_params,
                row_params,
                next_var,
                interner,
            )?],
        ),
        BaseHmType::List(inner) => InferType::App(
            TypeConstructor::List,
            vec![lower_type(
                inner,
                type_params,
                row_params,
                next_var,
                interner,
            )?],
        ),
        BaseHmType::Array(inner) => InferType::App(
            TypeConstructor::Array,
            vec![lower_type(
                inner,
                type_params,
                row_params,
                next_var,
                interner,
            )?],
        ),
        BaseHmType::Map(k, v) => InferType::App(
            TypeConstructor::Map,
            vec![
                lower_type(k, type_params, row_params, next_var, interner)?,
                lower_type(v, type_params, row_params, next_var, interner)?,
            ],
        ),
        BaseHmType::Either(l, r) => InferType::App(
            TypeConstructor::Either,
            vec![
                lower_type(l, type_params, row_params, next_var, interner)?,
                lower_type(r, type_params, row_params, next_var, interner)?,
            ],
        ),
        BaseHmType::Tuple(elements) => InferType::Tuple(
            elements
                .iter()
                .map(|e| lower_type(e, type_params, row_params, next_var, interner))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        BaseHmType::Fun {
            params,
            ret,
            effects,
        } => InferType::Fun(
            params
                .iter()
                .map(|p| lower_type(p, type_params, row_params, next_var, interner))
                .collect::<Result<Vec<_>, _>>()?,
            Box::new(lower_type(
                ret,
                type_params,
                row_params,
                next_var,
                interner,
            )?),
            lower_effect_row(effects, row_params, next_var, interner)?,
        ),
    };
    Ok(out)
}

pub(super) fn lower_effect_row(
    row: &BaseHmEffectRow,
    row_params: &mut HashMap<&'static str, TypeVarId>,
    next_var: &mut TypeVarId,
    interner: &mut Interner,
) -> Result<InferEffectRow, String> {
    let concrete: Vec<Identifier> = row.concrete.iter().map(|n| interner.intern(n)).collect();
    let tail = row.tail.map(|name| {
        *row_params.entry(name).or_insert_with(|| {
            let var = *next_var;
            *next_var = next_var.saturating_add(1);
            var
        })
    });
    match tail {
        Some(tail_var) => Ok(InferEffectRow::open_from_symbols(concrete, tail_var)),
        None => Ok(InferEffectRow::closed_from_symbols(concrete)),
    }
}

pub(crate) fn scheme_for_signature_id(
    id: BaseHmSignatureId,
    interner: &mut Interner,
) -> Result<Scheme, String> {
    signature_for_id(id).to_scheme(interner)
}

pub fn signature_for_id(id: BaseHmSignatureId) -> BaseHmSignature {
    use BaseHmSignatureId as Id;
    match id {
        Id::Print => sig(vec![t_any()], t_unit(), row(vec!["IO"], None)),
        Id::Len => sig(vec![t_any()], t_int(), row(vec![], None)),
        Id::First => sig(vec![t_any()], t_option(t_any()), row(vec![], None)),
        Id::Last => sig(vec![t_any()], t_option(t_any()), row(vec![], None)),
        Id::Rest => sig(vec![t_any()], t_any(), row(vec![], None)),
        Id::Push => sig(vec![t_any(), t_any()], t_any(), row(vec![], None)),
        Id::ToString => sig(vec![t_any()], t_string(), row(vec![], None)),
        Id::Concat => sig(vec![t_any(), t_any()], t_any(), row(vec![], None)),
        Id::Reverse => sig(vec![t_any()], t_any(), row(vec![], None)),
        Id::Contains => sig(vec![t_any(), t_any()], t_bool(), row(vec![], None)),
        Id::Slice => sig(vec![t_any(), t_int(), t_int()], t_any(), row(vec![], None)),
        Id::Sort => sig(vec![t_any()], t_any(), row(vec![], None)),
        Id::Split => sig(vec![t_string(), t_string()], t_any(), row(vec![], None)),
        Id::Join => sig(vec![t_any(), t_string()], t_string(), row(vec![], None)),
        Id::Trim => sig(vec![t_string()], t_string(), row(vec![], None)),
        Id::Upper => sig(vec![t_string()], t_string(), row(vec![], None)),
        Id::Lower => sig(vec![t_string()], t_string(), row(vec![], None)),
        Id::StartsWith => sig(vec![t_string(), t_string()], t_bool(), row(vec![], None)),
        Id::EndsWith => sig(vec![t_string(), t_string()], t_bool(), row(vec![], None)),
        Id::Replace => sig(
            vec![t_string(), t_string(), t_string()],
            t_string(),
            row(vec![], None),
        ),
        Id::Chars => sig(vec![t_string()], t_any(), row(vec![], None)),
        Id::Substring => sig(
            vec![t_string(), t_int(), t_int()],
            t_string(),
            row(vec![], None),
        ),
        Id::Keys => sig(vec![t_any()], t_any(), row(vec![], None)),
        Id::Values => sig(vec![t_any()], t_any(), row(vec![], None)),
        Id::HasKey => sig(vec![t_any(), t_any()], t_bool(), row(vec![], None)),
        Id::Merge => sig(vec![t_any(), t_any()], t_any(), row(vec![], None)),
        Id::Delete => sig(vec![t_any(), t_any()], t_any(), row(vec![], None)),
        Id::Abs => sig(vec![t_any()], t_any(), row(vec![], None)),
        Id::Min => sig(vec![t_any(), t_any()], t_any(), row(vec![], None)),
        Id::Max => sig(vec![t_any(), t_any()], t_any(), row(vec![], None)),
        Id::TypeOf => sig(vec![t_any()], t_string(), row(vec![], None)),
        Id::IsInt => sig(vec![t_any()], t_bool(), row(vec![], None)),
        Id::IsFloat => sig(vec![t_any()], t_bool(), row(vec![], None)),
        Id::IsString => sig(vec![t_any()], t_bool(), row(vec![], None)),
        Id::IsBool => sig(vec![t_any()], t_bool(), row(vec![], None)),
        Id::IsArray => sig(vec![t_any()], t_bool(), row(vec![], None)),
        Id::IsHash => sig(vec![t_any()], t_bool(), row(vec![], None)),
        Id::IsNone => sig(vec![t_any()], t_bool(), row(vec![], None)),
        Id::IsSome => sig(vec![t_any()], t_bool(), row(vec![], None)),
        Id::Map => sig_with_row_params(
            vec![],
            vec!["e"],
            vec![
                t_any(),
                t_fun(vec![t_any()], t_any(), row(vec![], Some("e"))),
            ],
            t_any(),
            row(vec![], Some("e")),
        ),
        Id::Filter => sig_with_row_params(
            vec![],
            vec!["e"],
            vec![
                t_any(),
                t_fun(vec![t_any()], t_bool(), row(vec![], Some("e"))),
            ],
            t_any(),
            row(vec![], Some("e")),
        ),
        Id::Fold => sig_with_row_params(
            vec![],
            vec!["e"],
            vec![
                t_any(),
                t_any(),
                t_fun(vec![t_any(), t_any()], t_any(), row(vec![], Some("e"))),
            ],
            t_any(),
            row(vec![], Some("e")),
        ),
        Id::Hd => sig(vec![t_any()], t_option(t_any()), row(vec![], None)),
        Id::Tl => sig(vec![t_any()], t_any(), row(vec![], None)),
        Id::IsList => sig(vec![t_any()], t_bool(), row(vec![], None)),
        Id::ToList => sig(vec![t_any()], t_any(), row(vec![], None)),
        Id::ToArray => sig(vec![t_any()], t_any(), row(vec![], None)),
        Id::Put => sig(vec![t_any(), t_any(), t_any()], t_any(), row(vec![], None)),
        Id::Get => sig(vec![t_any(), t_any()], t_option(t_any()), row(vec![], None)),
        Id::IsMap => sig(vec![t_any()], t_bool(), row(vec![], None)),
        Id::List => sig(vec![t_any()], t_any(), row(vec![], None)),
        Id::ReadFile => sig(vec![t_string()], t_string(), row(vec!["IO"], None)),
        Id::ReadLines => sig(vec![t_string()], t_any(), row(vec!["IO"], None)),
        Id::ReadStdin => sig(vec![], t_string(), row(vec!["IO"], None)),
        Id::ParseInt => sig(vec![t_string()], t_option(t_int()), row(vec![], None)),
        Id::NowMs => sig(vec![], t_int(), row(vec!["Time"], None)),
        Id::Time => sig(vec![], t_int(), row(vec!["Time"], None)),
        Id::Range => sig(vec![t_int(), t_int()], t_any(), row(vec![], None)),
        Id::Sum => sig(vec![t_any()], t_any(), row(vec![], None)),
        Id::Product => sig(vec![t_any()], t_any(), row(vec![], None)),
        Id::ParseInts => sig(
            vec![BaseHmType::Array(Box::new(t_string()))],
            t_any(),
            row(vec![], None),
        ),
        Id::SplitInts => sig(vec![t_string(), t_string()], t_any(), row(vec![], None)),
        Id::FlatMap => sig_with_row_params(
            vec![],
            vec!["e"],
            vec![
                t_any(),
                t_fun(vec![t_any()], t_any(), row(vec![], Some("e"))),
            ],
            t_any(),
            row(vec![], Some("e")),
        ),
        Id::Any => sig_with_row_params(
            vec![],
            vec!["e"],
            vec![
                t_any(),
                t_fun(vec![t_any()], t_bool(), row(vec![], Some("e"))),
            ],
            t_bool(),
            row(vec![], Some("e")),
        ),
        Id::All => sig_with_row_params(
            vec![],
            vec!["e"],
            vec![
                t_any(),
                t_fun(vec![t_any()], t_bool(), row(vec![], Some("e"))),
            ],
            t_bool(),
            row(vec![], Some("e")),
        ),
        Id::Find => sig_with_row_params(
            vec![],
            vec!["e"],
            vec![
                t_any(),
                t_fun(vec![t_any()], t_bool(), row(vec![], Some("e"))),
            ],
            t_option(t_any()),
            row(vec![], Some("e")),
        ),
        Id::SortBy => sig_with_row_params(
            vec![],
            vec!["e"],
            vec![
                t_any(),
                t_fun(vec![t_any()], t_any(), row(vec![], Some("e"))),
            ],
            t_any(),
            row(vec![], Some("e")),
        ),
        Id::Zip => sig(vec![t_any(), t_any()], t_any(), row(vec![], None)),
        Id::Flatten => sig(vec![t_any()], t_any(), row(vec![], None)),
        Id::Count => sig_with_row_params(
            vec![],
            vec!["e"],
            vec![
                t_any(),
                t_fun(vec![t_any()], t_bool(), row(vec![], Some("e"))),
            ],
            t_int(),
            row(vec![], Some("e")),
        ),
        Id::AssertEq => sig(vec![t_any(), t_any()], t_unit(), row(vec![], None)),
        Id::AssertNeq => sig(vec![t_any(), t_any()], t_unit(), row(vec![], None)),
        Id::AssertTrue => sig(vec![t_bool()], t_unit(), row(vec![], None)),
        Id::AssertFalse => sig(vec![t_bool()], t_unit(), row(vec![], None)),
        Id::AssertThrows => sig_with_row_params(
            vec![],
            vec!["e"],
            vec![t_fun(vec![], t_any(), row(vec![], Some("e")))],
            t_unit(),
            row(vec![], Some("e")),
        ),
    }
}

fn sig(params: Vec<BaseHmType>, ret: BaseHmType, effects: BaseHmEffectRow) -> BaseHmSignature {
    BaseHmSignature {
        type_params: vec![],
        row_params: vec![],
        params,
        ret,
        effects,
    }
}

fn sig_with_row_params(
    type_params: Vec<&'static str>,
    row_params: Vec<&'static str>,
    params: Vec<BaseHmType>,
    ret: BaseHmType,
    effects: BaseHmEffectRow,
) -> BaseHmSignature {
    BaseHmSignature {
        type_params,
        row_params,
        params,
        ret,
        effects,
    }
}

fn row(concrete: Vec<&'static str>, tail: Option<&'static str>) -> BaseHmEffectRow {
    BaseHmEffectRow { concrete, tail }
}

fn t_any() -> BaseHmType {
    BaseHmType::Any
}

fn t_int() -> BaseHmType {
    BaseHmType::Int
}

fn t_bool() -> BaseHmType {
    BaseHmType::Bool
}

fn t_string() -> BaseHmType {
    BaseHmType::String
}

fn t_unit() -> BaseHmType {
    BaseHmType::Unit
}

fn t_option(inner: BaseHmType) -> BaseHmType {
    BaseHmType::Option(Box::new(inner))
}

fn t_fun(params: Vec<BaseHmType>, ret: BaseHmType, effects: BaseHmEffectRow) -> BaseHmType {
    BaseHmType::Fun {
        params,
        ret: Box::new(ret),
        effects,
    }
}

#[cfg(test)]
mod tests {
    use super::{BaseHmSignatureId, scheme_for_signature_id};
    use crate::syntax::interner::Interner;

    #[test]
    fn map_signature_lowers_with_row_tail_quantified() {
        let mut interner = Interner::new();
        let scheme = scheme_for_signature_id(BaseHmSignatureId::Map, &mut interner)
            .expect("map signature must lower");
        assert!(!scheme.forall.is_empty());
    }
}
