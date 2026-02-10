use crate::runtime::value::Value;

use super::helpers::check_arity;

pub(super) fn builtin_type_of(args: &[Value]) -> Result<Value, String> {
    check_arity(&args, 1, "type_of", "type_of(x)")?;
    Ok(Value::String(args[0].type_name().to_string().into()))
}

pub(super) fn builtin_is_int(args: &[Value]) -> Result<Value, String> {
    check_arity(&args, 1, "is_int", "is_int(x)")?;
    Ok(Value::Boolean(matches!(args[0], Value::Integer(_))))
}

pub(super) fn builtin_is_float(args: &[Value]) -> Result<Value, String> {
    check_arity(&args, 1, "is_float", "is_float(x)")?;
    Ok(Value::Boolean(matches!(args[0], Value::Float(_))))
}

pub(super) fn builtin_is_string(args: &[Value]) -> Result<Value, String> {
    check_arity(&args, 1, "is_string", "is_string(x)")?;
    Ok(Value::Boolean(matches!(args[0], Value::String(_))))
}

pub(super) fn builtin_is_bool(args: &[Value]) -> Result<Value, String> {
    check_arity(&args, 1, "is_bool", "is_bool(x)")?;
    Ok(Value::Boolean(matches!(args[0], Value::Boolean(_))))
}

pub(super) fn builtin_is_array(args: &[Value]) -> Result<Value, String> {
    check_arity(&args, 1, "is_array", "is_array(x)")?;
    Ok(Value::Boolean(matches!(args[0], Value::Array(_))))
}

pub(super) fn builtin_is_hash(args: &[Value]) -> Result<Value, String> {
    check_arity(&args, 1, "is_hash", "is_hash(x)")?;
    Ok(Value::Boolean(matches!(args[0], Value::Hash(_))))
}

pub(super) fn builtin_is_none(args: &[Value]) -> Result<Value, String> {
    check_arity(&args, 1, "is_none", "is_none(x)")?;
    Ok(Value::Boolean(matches!(args[0], Value::None)))
}

pub(super) fn builtin_is_some(args: &[Value]) -> Result<Value, String> {
    check_arity(&args, 1, "is_some", "is_some(x)")?;
    Ok(Value::Boolean(matches!(args[0], Value::Some(_))))
}
