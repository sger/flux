use crate::runtime::object::Object;

use super::helpers::check_arity;

pub(super) fn builtin_type_of(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "type_of", "type_of(x)")?;
    Ok(Object::String(args[0].type_name().to_string()))
}

pub(super) fn builtin_is_int(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "is_int", "is_int(x)")?;
    Ok(Object::Boolean(matches!(args[0], Object::Integer(_))))
}

pub(super) fn builtin_is_float(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "is_float", "is_float(x)")?;
    Ok(Object::Boolean(matches!(args[0], Object::Float(_))))
}

pub(super) fn builtin_is_string(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "is_string", "is_string(x)")?;
    Ok(Object::Boolean(matches!(args[0], Object::String(_))))
}

pub(super) fn builtin_is_bool(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "is_bool", "is_bool(x)")?;
    Ok(Object::Boolean(matches!(args[0], Object::Boolean(_))))
}

pub(super) fn builtin_is_array(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "is_array", "is_array(x)")?;
    Ok(Object::Boolean(matches!(args[0], Object::Array(_))))
}

pub(super) fn builtin_is_hash(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "is_hash", "is_hash(x)")?;
    Ok(Object::Boolean(matches!(args[0], Object::Hash(_))))
}

pub(super) fn builtin_is_none(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "is_none", "is_none(x)")?;
    Ok(Object::Boolean(matches!(args[0], Object::None)))
}

pub(super) fn builtin_is_some(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "is_some", "is_some(x)")?;
    Ok(Object::Boolean(matches!(args[0], Object::Some(_))))
}
