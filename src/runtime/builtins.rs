use crate::runtime::{builtin_function::BuiltinFunction, object::Object};

fn format_hint(signature: &str) -> String {
    format!("\n\nHint:\n  {}", signature)
}

fn arity_error(name: &str, expected: &str, got: usize, signature: &str) -> String {
    format!(
        "wrong number of arguments\n\n  function: {}/{}\n  expected: {}\n  got: {}{}",
        name,
        expected,
        expected,
        got,
        format_hint(signature)
    )
}

fn type_error(name: &str, label: &str, expected: &str, got: &str, signature: &str) -> String {
    format!(
        "{} expected {} to be {}, got {}{}",
        name,
        label,
        expected,
        got,
        format_hint(signature)
    )
}

fn check_arity(args: &[Object], expected: usize, name: &str, signature: &str) -> Result<(), String> {
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

fn check_arity_range(
    args: &[Object],
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

fn arg_string<'a>(
    args: &'a [Object],
    index: usize,
    name: &str,
    label: &str,
    signature: &str,
) -> Result<&'a str, String> {
    match &args[index] {
        Object::String(s) => Ok(s.as_str()),
        other => Err(type_error(
            name,
            label,
            "String",
            other.type_name(),
            signature,
        )),
    }
}

fn arg_array<'a>(
    args: &'a [Object],
    index: usize,
    name: &str,
    label: &str,
    signature: &str,
) -> Result<&'a Vec<Object>, String> {
    match &args[index] {
        Object::Array(arr) => Ok(arr),
        other => Err(type_error(
            name,
            label,
            "Array",
            other.type_name(),
            signature,
        )),
    }
}

fn arg_int(
    args: &[Object],
    index: usize,
    name: &str,
    label: &str,
    signature: &str,
) -> Result<i64, String> {
    match &args[index] {
        Object::Integer(value) => Ok(*value),
        other => Err(type_error(
            name,
            label,
            "Integer",
            other.type_name(),
            signature,
        )),
    }
}

fn arg_hash<'a>(
    args: &'a [Object],
    index: usize,
    name: &str,
    label: &str,
    signature: &str,
) -> Result<&'a std::collections::HashMap<crate::runtime::hash_key::HashKey, Object>, String> {
    match &args[index] {
        Object::Hash(h) => Ok(h),
        other => Err(type_error(name, label, "Hash", other.type_name(), signature)),
    }
}

/// Convert a HashKey back to an Object
fn hash_key_to_object(key: &crate::runtime::hash_key::HashKey) -> Object {
    use crate::runtime::hash_key::HashKey;
    match key {
        HashKey::Integer(v) => Object::Integer(*v),
        HashKey::Boolean(v) => Object::Boolean(*v),
        HashKey::String(v) => Object::String(v.clone()),
    }
}

/// keys(h) - Return an array of all keys in the hash
fn builtin_keys(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "keys", "keys(h)")?;
    let hash = arg_hash(&args, 0, "keys", "argument", "keys(h)")?;
    let keys: Vec<Object> = hash.keys().map(hash_key_to_object).collect();
    Ok(Object::Array(keys))
}

/// values(h) - Return an array of all values in the hash
fn builtin_values(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "values", "values(h)")?;
    let hash = arg_hash(&args, 0, "values", "argument", "values(h)")?;
    let values: Vec<Object> = hash.values().cloned().collect();
    Ok(Object::Array(values))
}

/// has_key(h, k) - Check if hash contains a key
fn builtin_has_key(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 2, "has_key", "has_key(h, k)")?;
    let hash = arg_hash(&args, 0, "has_key", "first argument", "has_key(h, k)")?;
    let key = args[1].to_hash_key().ok_or_else(|| {
        format!(
            "has_key key must be hashable (String, Int, Bool), got {}{}",
            args[1].type_name(),
            format_hint("has_key(h, k)")
        )
    })?;
    Ok(Object::Boolean(hash.contains_key(&key)))
}

/// merge(h1, h2) - Merge two hashes, with h2 values overwriting h1 on conflict
fn builtin_merge(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 2, "merge", "merge(h1, h2)")?;
    let h1 = arg_hash(&args, 0, "merge", "first argument", "merge(h1, h2)")?;
    let h2 = arg_hash(&args, 1, "merge", "second argument", "merge(h1, h2)")?;
    let mut result = h1.clone();
    for (k, v) in h2.iter() {
        result.insert(k.clone(), v.clone());
    }
    Ok(Object::Hash(result))
}

fn arg_number(
    args: &[Object],
    index: usize,
    name: &str,
    label: &str,
    signature: &str,
) -> Result<f64, String> {
    match &args[index] {
        Object::Integer(v) => Ok(*v as f64),
        Object::Float(v) => Ok(*v),
        other => Err(type_error(name, label, "Number", other.type_name(), signature)),
    }
}

/// abs(n) - Return the absolute value of a number
fn builtin_abs(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "abs", "abs(n)")?;
    match &args[0] {
        Object::Integer(v) => Ok(Object::Integer(v.abs())),
        Object::Float(v) => Ok(Object::Float(v.abs())),
        other => Err(type_error("abs", "argument", "Number", other.type_name(), "abs(n)")),
    }
}

/// min(a, b) - Return the smaller of two numbers
fn builtin_min(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 2, "min", "min(a, b)")?;
    let a = arg_number(&args, 0, "min", "first argument", "min(a, b)")?;
    let b = arg_number(&args, 1, "min", "second argument", "min(a, b)")?;
    let result = a.min(b);
    // Return integer if both inputs were integers and result is whole
    match (&args[0], &args[1]) {
        (Object::Integer(_), Object::Integer(_)) => Ok(Object::Integer(result as i64)),
        _ => Ok(Object::Float(result)),
    }
}

/// max(a, b) - Return the larger of two numbers
fn builtin_max(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 2, "max", "max(a, b)")?;
    let a = arg_number(&args, 0, "max", "first argument", "max(a, b)")?;
    let b = arg_number(&args, 1, "max", "second argument", "max(a, b)")?;
    let result = a.max(b);
    // Return integer if both inputs were integers and result is whole
    match (&args[0], &args[1]) {
        (Object::Integer(_), Object::Integer(_)) => Ok(Object::Integer(result as i64)),
        _ => Ok(Object::Float(result)),
    }
}

/// type_of(x) - Return the type name of a value as a string
fn builtin_type_of(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "type_of", "type_of(x)")?;
    Ok(Object::String(args[0].type_name().to_string()))
}

/// is_int(x) - Check if value is an integer
fn builtin_is_int(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "is_int", "is_int(x)")?;
    Ok(Object::Boolean(matches!(args[0], Object::Integer(_))))
}

/// is_float(x) - Check if value is a float
fn builtin_is_float(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "is_float", "is_float(x)")?;
    Ok(Object::Boolean(matches!(args[0], Object::Float(_))))
}

/// is_string(x) - Check if value is a string
fn builtin_is_string(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "is_string", "is_string(x)")?;
    Ok(Object::Boolean(matches!(args[0], Object::String(_))))
}

/// is_bool(x) - Check if value is a boolean
fn builtin_is_bool(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "is_bool", "is_bool(x)")?;
    Ok(Object::Boolean(matches!(args[0], Object::Boolean(_))))
}

/// is_array(x) - Check if value is an array
fn builtin_is_array(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "is_array", "is_array(x)")?;
    Ok(Object::Boolean(matches!(args[0], Object::Array(_))))
}

/// is_hash(x) - Check if value is a hash
fn builtin_is_hash(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "is_hash", "is_hash(x)")?;
    Ok(Object::Boolean(matches!(args[0], Object::Hash(_))))
}

/// is_none(x) - Check if value is None
fn builtin_is_none(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "is_none", "is_none(x)")?;
    Ok(Object::Boolean(matches!(args[0], Object::None)))
}

/// is_some(x) - Check if value is Some
fn builtin_is_some(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "is_some", "is_some(x)")?;
    Ok(Object::Boolean(matches!(args[0], Object::Some(_))))
}

fn builtin_print(args: Vec<Object>) -> Result<Object, String> {
    for arg in args {
        match &arg {
            Object::String(s) => println!("{}", s), // Raw string
            _ => println!("{}", arg),
        }
    }
    Ok(Object::None)
}

fn builtin_len(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "len", "len(value)")?;
    match &args[0] {
        Object::String(s) => Ok(Object::Integer(s.len() as i64)),
        Object::Array(arr) => Ok(Object::Integer(arr.len() as i64)),
        other => Err(type_error(
            "len",
            "argument",
            "String or Array",
            other.type_name(),
            "len(value)",
        )),
    }
}

fn builtin_first(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "first", "first(arr)")?;
    let arr = arg_array(&args, 0, "first", "argument", "first(arr)")?;
    if arr.is_empty() {
        Ok(Object::None)
    } else {
        Ok(arr[0].clone())
    }
}

fn builtin_last(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "last", "last(arr)")?;
    let arr = arg_array(&args, 0, "last", "argument", "last(arr)")?;
    if arr.is_empty() {
        Ok(Object::None)
    } else {
        Ok(arr[arr.len() - 1].clone())
    }
}

fn builtin_rest(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "rest", "rest(arr)")?;
    let arr = arg_array(&args, 0, "rest", "argument", "rest(arr)")?;
    if arr.is_empty() {
        Ok(Object::None)
    } else {
        Ok(Object::Array(arr[1..].to_vec()))
    }
}

fn builtin_push(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 2, "push", "push(arr, elem)")?;
    let arr = arg_array(&args, 0, "push", "first argument", "push(arr, elem)")?;
    let mut new_arr = arr.clone();
    new_arr.push(args[1].clone());
    Ok(Object::Array(new_arr))
}

fn builtin_to_string(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "to_string", "to_string(value)")?;
    Ok(Object::String(args[0].to_string_value()))
}

/// concat(a, b) - Concatenate two arrays into a new array
fn builtin_concat(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 2, "concat", "concat(a, b)")?;
    let a = arg_array(&args, 0, "concat", "first argument", "concat(a, b)")?;
    let b = arg_array(&args, 1, "concat", "second argument", "concat(a, b)")?;
    let mut result = a.clone();
    result.extend(b.iter().cloned());
    Ok(Object::Array(result))
}

/// reverse(arr) - Return a new array with elements in reverse order
fn builtin_reverse(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "reverse", "reverse(arr)")?;
    let arr = arg_array(&args, 0, "reverse", "argument", "reverse(arr)")?;
    let mut result = arr.clone();
    result.reverse();
    Ok(Object::Array(result))
}

/// contains(arr, elem) - Check if array contains an element
fn builtin_contains(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 2, "contains", "contains(arr, elem)")?;
    let arr = arg_array(&args, 0, "contains", "first argument", "contains(arr, elem)")?;
    let elem = &args[1];
    let found = arr.iter().any(|item| item == elem);
    Ok(Object::Boolean(found))
}

/// slice(arr, start, end) - Return a slice of the array from start to end (exclusive)
fn builtin_slice(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 3, "slice", "slice(arr, start, end)")?;
    let arr = arg_array(&args, 0, "slice", "first argument", "slice(arr, start, end)")?;
    let start = arg_int(&args, 1, "slice", "second argument", "slice(arr, start, end)")?;
    let end = arg_int(&args, 2, "slice", "third argument", "slice(arr, start, end)")?;
    let len = arr.len() as i64;
    let start = if start < 0 { 0 } else { start as usize };
    let end = if end > len { len as usize } else { end as usize };
    if start >= end || start >= arr.len() {
        Ok(Object::Array(vec![]))
    } else {
        Ok(Object::Array(arr[start..end].to_vec()))
    }
}

/// sort(arr) or sort(arr, order) - Return a new sorted array
/// order: "asc" (default) or "desc"
/// Only works with integers/floats
fn builtin_sort(args: Vec<Object>) -> Result<Object, String> {
    check_arity_range(&args, 1, 2, "sort", "sort(arr, order)")?;
    let arr = arg_array(&args, 0, "sort", "first argument", "sort(arr, order)")?;
    // Determine sort order (default: ascending)
    let descending = if args.len() == 2 {
        match arg_string(&args, 1, "sort", "second argument", "sort(arr, order)")? {
            "asc" => false,
            "desc" => true,
            other => {
                return Err(format!(
                    "sort order must be \"asc\" or \"desc\", got \"{}\"{}",
                    other,
                    format_hint("sort(arr, order)")
                ));
            }
        }
    } else {
        false
    };

    // Check if all elements are comparable (integers or floats)
    let all_numeric = arr
        .iter()
        .all(|item| matches!(item, Object::Integer(_) | Object::Float(_)));

    if !all_numeric && !arr.is_empty() {
        return Err(format!(
            "sort only supports arrays of Integers or Floats{}",
            format_hint("sort(arr, order)")
        ));
    }

    let mut result = arr.clone();

    result.sort_by(|a, b| {
        use std::cmp::Ordering;
        // Smart comparison: avoid f64 conversion when both are same type
        let cmp = match (a, b) {
            (Object::Integer(i1), Object::Integer(i2)) => i1.cmp(i2),
            (Object::Float(f1), Object::Float(f2)) => f1.partial_cmp(f2).unwrap_or(Ordering::Equal),
            (Object::Integer(i), Object::Float(f)) => {
                (*i as f64).partial_cmp(f).unwrap_or(Ordering::Equal)
            }
            (Object::Float(f), Object::Integer(i)) => {
                f.partial_cmp(&(*i as f64)).unwrap_or(Ordering::Equal)
            }
            _ => Ordering::Equal,
        };
        if descending { cmp.reverse() } else { cmp }
    });
    Ok(Object::Array(result))
}

/// split(s, delim) - Split a string by delimiter into an array of strings
fn builtin_split(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 2, "split", "split(s, delim)")?;
    let s = arg_string(&args, 0, "split", "first argument", "split(s, delim)")?;
    let delim = arg_string(&args, 1, "split", "second argument", "split(s, delim)")?;
    let parts: Vec<Object> = if delim.is_empty() {
        // Match test expectation: split into characters without empty ends.
        s.chars().map(|ch| Object::String(ch.to_string())).collect()
    } else {
        s.split(delim)
            .map(|part| Object::String(part.to_string()))
            .collect()
    };
    Ok(Object::Array(parts))
}

/// join(arr, delim) - Join an array of strings with a delimiter
fn builtin_join(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 2, "join", "join(arr, delim)")?;
    let arr = arg_array(&args, 0, "join", "first argument", "join(arr, delim)")?;
    let delim = arg_string(&args, 1, "join", "second argument", "join(arr, delim)")?;
    let strings: Result<Vec<String>, String> = arr
        .iter()
        .map(|item| match item {
            Object::String(s) => Ok(s.clone()),
            other => Err(format!(
                "join expected array elements to be String, got {}{}",
                other.type_name(),
                format_hint("join(arr, delim)")
            )),
        })
        .collect();
    Ok(Object::String(strings?.join(delim)))
}

/// trim(s) - Remove leading and trailing whitespace
fn builtin_trim(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "trim", "trim(s)")?;
    let s = arg_string(&args, 0, "trim", "argument", "trim(s)")?;
    Ok(Object::String(s.trim().to_string()))
}

/// upper(s) - Convert string to uppercase
fn builtin_upper(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "upper", "upper(s)")?;
    let s = arg_string(&args, 0, "upper", "argument", "upper(s)")?;
    Ok(Object::String(s.to_uppercase()))
}

/// lower(s) - Convert string to lowercase
fn builtin_lower(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "lower", "lower(s)")?;
    let s = arg_string(&args, 0, "lower", "argument", "lower(s)")?;
    Ok(Object::String(s.to_lowercase()))
}

/// chars(s) - Convert string to array of single-character strings
fn builtin_chars(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 1, "chars", "chars(s)")?;
    let s = arg_string(&args, 0, "chars", "argument", "chars(s)")?;
    let chars: Vec<Object> = s.chars().map(|c| Object::String(c.to_string())).collect();
    Ok(Object::Array(chars))
}

/// substring(s, start, end) - Extract a substring (start inclusive, end exclusive)
fn builtin_substring(args: Vec<Object>) -> Result<Object, String> {
    check_arity(&args, 3, "substring", "substring(s, start, end)")?;
    let s = arg_string(&args, 0, "substring", "first argument", "substring(s, start, end)")?;
    let start =
        arg_int(&args, 1, "substring", "second argument", "substring(s, start, end)")?;
    let end = arg_int(&args, 2, "substring", "third argument", "substring(s, start, end)")?;
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len() as i64;
    let start = if start < 0 { 0 } else { start as usize };
    let end = if end > len { len as usize } else { end as usize };
    if start >= end || start >= chars.len() {
        Ok(Object::String(String::new()))
    } else {
        Ok(Object::String(chars[start..end].iter().collect()))
    }
}

/// All built-in functions in order (index matters for OpGetBuiltin)
pub static BUILTINS: &[BuiltinFunction] = &[
    BuiltinFunction {
        name: "print",
        func: builtin_print,
    },
    BuiltinFunction {
        name: "len",
        func: builtin_len,
    },
    BuiltinFunction {
        name: "first",
        func: builtin_first,
    },
    BuiltinFunction {
        name: "last",
        func: builtin_last,
    },
    BuiltinFunction {
        name: "rest",
        func: builtin_rest,
    },
    BuiltinFunction {
        name: "push",
        func: builtin_push,
    },
    BuiltinFunction {
        name: "to_string",
        func: builtin_to_string,
    },
    BuiltinFunction {
        name: "concat",
        func: builtin_concat,
    },
    BuiltinFunction {
        name: "reverse",
        func: builtin_reverse,
    },
    BuiltinFunction {
        name: "contains",
        func: builtin_contains,
    },
    BuiltinFunction {
        name: "slice",
        func: builtin_slice,
    },
    BuiltinFunction {
        name: "sort",
        func: builtin_sort,
    },
    BuiltinFunction {
        name: "split",
        func: builtin_split,
    },
    BuiltinFunction {
        name: "join",
        func: builtin_join,
    },
    BuiltinFunction {
        name: "trim",
        func: builtin_trim,
    },
    BuiltinFunction {
        name: "upper",
        func: builtin_upper,
    },
    BuiltinFunction {
        name: "lower",
        func: builtin_lower,
    },
    BuiltinFunction {
        name: "chars",
        func: builtin_chars,
    },
    BuiltinFunction {
        name: "substring",
        func: builtin_substring,
    },
    BuiltinFunction {
        name: "keys",
        func: builtin_keys,
    },
    BuiltinFunction {
        name: "values",
        func: builtin_values,
    },
    BuiltinFunction {
        name: "has_key",
        func: builtin_has_key,
    },
    BuiltinFunction {
        name: "merge",
        func: builtin_merge,
    },
    BuiltinFunction {
        name: "abs",
        func: builtin_abs,
    },
    BuiltinFunction {
        name: "min",
        func: builtin_min,
    },
    BuiltinFunction {
        name: "max",
        func: builtin_max,
    },
    BuiltinFunction {
        name: "type_of",
        func: builtin_type_of,
    },
    BuiltinFunction {
        name: "is_int",
        func: builtin_is_int,
    },
    BuiltinFunction {
        name: "is_float",
        func: builtin_is_float,
    },
    BuiltinFunction {
        name: "is_string",
        func: builtin_is_string,
    },
    BuiltinFunction {
        name: "is_bool",
        func: builtin_is_bool,
    },
    BuiltinFunction {
        name: "is_array",
        func: builtin_is_array,
    },
    BuiltinFunction {
        name: "is_hash",
        func: builtin_is_hash,
    },
    BuiltinFunction {
        name: "is_none",
        func: builtin_is_none,
    },
    BuiltinFunction {
        name: "is_some",
        func: builtin_is_some,
    },
];

pub fn get_builtin(name: &str) -> Option<&'static BuiltinFunction> {
    BUILTINS.iter().find(|b| b.name == name)
}

pub fn get_builtin_by_index(index: usize) -> Option<&'static BuiltinFunction> {
    BUILTINS.get(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_len_string() {
        let result = builtin_len(vec![Object::String("hello".to_string())]).unwrap();
        assert_eq!(result, Object::Integer(5));
    }

    #[test]
    fn test_builtin_len_array() {
        let result = builtin_len(vec![Object::Array(vec![
            Object::Integer(1),
            Object::Integer(2),
            Object::Integer(3),
        ])])
        .unwrap();
        assert_eq!(result, Object::Integer(3));
    }

    #[test]
    fn test_builtin_first() {
        let arr = Object::Array(vec![Object::Integer(1), Object::Integer(2)]);
        let result = builtin_first(vec![arr]).unwrap();
        assert_eq!(result, Object::Integer(1));
    }

    #[test]
    fn test_builtin_last() {
        let arr = Object::Array(vec![Object::Integer(1), Object::Integer(2)]);
        let result = builtin_last(vec![arr]).unwrap();
        assert_eq!(result, Object::Integer(2));
    }

    #[test]
    fn test_builtin_rest() {
        let arr = Object::Array(vec![
            Object::Integer(1),
            Object::Integer(2),
            Object::Integer(3),
        ]);
        let result = builtin_rest(vec![arr]).unwrap();
        assert_eq!(
            result,
            Object::Array(vec![Object::Integer(2), Object::Integer(3)])
        );
    }

    #[test]
    fn test_builtin_push() {
        let arr = Object::Array(vec![Object::Integer(1)]);
        let result = builtin_push(vec![arr, Object::Integer(2)]).unwrap();
        assert_eq!(
            result,
            Object::Array(vec![Object::Integer(1), Object::Integer(2)])
        );
    }

    #[test]
    fn test_get_builtin() {
        assert!(get_builtin("print").is_some());
        assert!(get_builtin("len").is_some());
        assert!(get_builtin("nonexistent").is_none());
    }

    #[test]
    fn test_builtin_concat() {
        let a = Object::Array(vec![Object::Integer(1), Object::Integer(2)]);
        let b = Object::Array(vec![Object::Integer(3), Object::Integer(4)]);
        let result = builtin_concat(vec![a, b]).unwrap();
        assert_eq!(
            result,
            Object::Array(vec![
                Object::Integer(1),
                Object::Integer(2),
                Object::Integer(3),
                Object::Integer(4)
            ])
        );
    }

    #[test]
    fn test_builtin_concat_empty() {
        let a = Object::Array(vec![Object::Integer(1)]);
        let b = Object::Array(vec![]);
        let result = builtin_concat(vec![a, b]).unwrap();
        assert_eq!(result, Object::Array(vec![Object::Integer(1)]));
    }

    #[test]
    fn test_builtin_reverse() {
        let arr = Object::Array(vec![
            Object::Integer(1),
            Object::Integer(2),
            Object::Integer(3),
        ]);
        let result = builtin_reverse(vec![arr]).unwrap();
        assert_eq!(
            result,
            Object::Array(vec![
                Object::Integer(3),
                Object::Integer(2),
                Object::Integer(1)
            ])
        );
    }

    #[test]
    fn test_builtin_reverse_empty() {
        let arr = Object::Array(vec![]);
        let result = builtin_reverse(vec![arr]).unwrap();
        assert_eq!(result, Object::Array(vec![]));
    }

    #[test]
    fn test_builtin_contains_found() {
        let arr = Object::Array(vec![
            Object::Integer(1),
            Object::Integer(2),
            Object::Integer(3),
        ]);
        let result = builtin_contains(vec![arr, Object::Integer(2)]).unwrap();
        assert_eq!(result, Object::Boolean(true));
    }

    #[test]
    fn test_builtin_contains_not_found() {
        let arr = Object::Array(vec![
            Object::Integer(1),
            Object::Integer(2),
            Object::Integer(3),
        ]);
        let result = builtin_contains(vec![arr, Object::Integer(5)]).unwrap();
        assert_eq!(result, Object::Boolean(false));
    }

    #[test]
    fn test_builtin_slice() {
        let arr = Object::Array(vec![
            Object::Integer(1),
            Object::Integer(2),
            Object::Integer(3),
            Object::Integer(4),
            Object::Integer(5),
        ]);
        let result = builtin_slice(vec![arr, Object::Integer(1), Object::Integer(4)]).unwrap();
        assert_eq!(
            result,
            Object::Array(vec![
                Object::Integer(2),
                Object::Integer(3),
                Object::Integer(4)
            ])
        );
    }

    #[test]
    fn test_builtin_slice_out_of_bounds() {
        let arr = Object::Array(vec![Object::Integer(1), Object::Integer(2)]);
        let result = builtin_slice(vec![arr, Object::Integer(0), Object::Integer(10)]).unwrap();
        assert_eq!(
            result,
            Object::Array(vec![Object::Integer(1), Object::Integer(2)])
        );
    }

    #[test]
    fn test_builtin_sort() {
        let arr = Object::Array(vec![
            Object::Integer(3),
            Object::Integer(1),
            Object::Integer(4),
            Object::Integer(1),
            Object::Integer(5),
        ]);
        let result = builtin_sort(vec![arr]).unwrap();
        assert_eq!(
            result,
            Object::Array(vec![
                Object::Integer(1),
                Object::Integer(1),
                Object::Integer(3),
                Object::Integer(4),
                Object::Integer(5)
            ])
        );
    }

    #[test]
    fn test_builtin_sort_floats() {
        let arr = Object::Array(vec![
            Object::Float(3.14),
            Object::Float(1.0),
            Object::Float(2.71),
        ]);
        let result = builtin_sort(vec![arr]).unwrap();
        assert_eq!(
            result,
            Object::Array(vec![
                Object::Float(1.0),
                Object::Float(2.71),
                Object::Float(3.14)
            ])
        );
    }

    #[test]
    fn test_builtin_sort_mixed_numeric() {
        let arr = Object::Array(vec![
            Object::Integer(3),
            Object::Float(1.5),
            Object::Integer(2),
        ]);
        let result = builtin_sort(vec![arr]).unwrap();
        assert_eq!(
            result,
            Object::Array(vec![
                Object::Float(1.5),
                Object::Integer(2),
                Object::Integer(3)
            ])
        );
    }

    #[test]
    fn test_builtin_sort_asc_explicit() {
        let arr = Object::Array(vec![
            Object::Integer(3),
            Object::Integer(1),
            Object::Integer(2),
        ]);
        let result = builtin_sort(vec![arr, Object::String("asc".to_string())]).unwrap();
        assert_eq!(
            result,
            Object::Array(vec![
                Object::Integer(1),
                Object::Integer(2),
                Object::Integer(3)
            ])
        );
    }

    #[test]
    fn test_builtin_sort_desc() {
        let arr = Object::Array(vec![
            Object::Integer(3),
            Object::Integer(1),
            Object::Integer(5),
            Object::Integer(2),
        ]);
        let result = builtin_sort(vec![arr, Object::String("desc".to_string())]).unwrap();
        assert_eq!(
            result,
            Object::Array(vec![
                Object::Integer(5),
                Object::Integer(3),
                Object::Integer(2),
                Object::Integer(1)
            ])
        );
    }

    #[test]
    fn test_builtin_sort_desc_floats() {
        let arr = Object::Array(vec![
            Object::Float(1.0),
            Object::Float(3.14),
            Object::Float(2.71),
        ]);
        let result = builtin_sort(vec![arr, Object::String("desc".to_string())]).unwrap();
        assert_eq!(
            result,
            Object::Array(vec![
                Object::Float(3.14),
                Object::Float(2.71),
                Object::Float(1.0)
            ])
        );
    }

    #[test]
    fn test_builtin_sort_invalid_order() {
        let arr = Object::Array(vec![Object::Integer(1), Object::Integer(2)]);
        let result = builtin_sort(vec![arr, Object::String("invalid".to_string())]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must be \"asc\" or \"desc\""));
    }

    #[test]
    fn test_builtin_split() {
        let result = builtin_split(vec![
            Object::String("a,b,c".to_string()),
            Object::String(",".to_string()),
        ])
        .unwrap();
        assert_eq!(
            result,
            Object::Array(vec![
                Object::String("a".to_string()),
                Object::String("b".to_string()),
                Object::String("c".to_string())
            ])
        );
    }

    #[test]
    fn test_builtin_split_empty() {
        let result = builtin_split(vec![
            Object::String("hello".to_string()),
            Object::String("".to_string()),
        ])
        .unwrap();
        // Split by empty string gives each character
        assert_eq!(
            result,
            Object::Array(vec![
                Object::String("h".to_string()),
                Object::String("e".to_string()),
                Object::String("l".to_string()),
                Object::String("l".to_string()),
                Object::String("o".to_string())
            ])
        );
    }

    #[test]
    fn test_builtin_join() {
        let arr = Object::Array(vec![
            Object::String("a".to_string()),
            Object::String("b".to_string()),
            Object::String("c".to_string()),
        ]);
        let result = builtin_join(vec![arr, Object::String(",".to_string())]).unwrap();
        assert_eq!(result, Object::String("a,b,c".to_string()));
    }

    #[test]
    fn test_builtin_join_empty_delim() {
        let arr = Object::Array(vec![
            Object::String("a".to_string()),
            Object::String("b".to_string()),
        ]);
        let result = builtin_join(vec![arr, Object::String("".to_string())]).unwrap();
        assert_eq!(result, Object::String("ab".to_string()));
    }

    #[test]
    fn test_builtin_trim() {
        let result = builtin_trim(vec![Object::String("  hello world  ".to_string())]).unwrap();
        assert_eq!(result, Object::String("hello world".to_string()));
    }

    #[test]
    fn test_builtin_trim_no_whitespace() {
        let result = builtin_trim(vec![Object::String("hello".to_string())]).unwrap();
        assert_eq!(result, Object::String("hello".to_string()));
    }

    #[test]
    fn test_builtin_upper() {
        let result = builtin_upper(vec![Object::String("hello".to_string())]).unwrap();
        assert_eq!(result, Object::String("HELLO".to_string()));
    }

    #[test]
    fn test_builtin_lower() {
        let result = builtin_lower(vec![Object::String("HELLO".to_string())]).unwrap();
        assert_eq!(result, Object::String("hello".to_string()));
    }

    #[test]
    fn test_builtin_chars() {
        let result = builtin_chars(vec![Object::String("abc".to_string())]).unwrap();
        assert_eq!(
            result,
            Object::Array(vec![
                Object::String("a".to_string()),
                Object::String("b".to_string()),
                Object::String("c".to_string())
            ])
        );
    }

    #[test]
    fn test_builtin_chars_empty() {
        let result = builtin_chars(vec![Object::String("".to_string())]).unwrap();
        assert_eq!(result, Object::Array(vec![]));
    }

    #[test]
    fn test_builtin_substring() {
        let result = builtin_substring(vec![
            Object::String("hello world".to_string()),
            Object::Integer(0),
            Object::Integer(5),
        ])
        .unwrap();
        assert_eq!(result, Object::String("hello".to_string()));
    }

    #[test]
    fn test_builtin_substring_middle() {
        let result = builtin_substring(vec![
            Object::String("hello world".to_string()),
            Object::Integer(6),
            Object::Integer(11),
        ])
        .unwrap();
        assert_eq!(result, Object::String("world".to_string()));
    }

    #[test]
    fn test_builtin_substring_out_of_bounds() {
        let result = builtin_substring(vec![
            Object::String("hello".to_string()),
            Object::Integer(0),
            Object::Integer(100),
        ])
        .unwrap();
        assert_eq!(result, Object::String("hello".to_string()));
    }

    // =============================================================================
    // Hash Builtins Tests (5.3)
    // =============================================================================

    fn make_test_hash() -> Object {
        use crate::runtime::hash_key::HashKey;
        let mut hash = std::collections::HashMap::new();
        hash.insert(HashKey::String("name".to_string()), Object::String("Alice".to_string()));
        hash.insert(HashKey::Integer(42), Object::Integer(100));
        hash.insert(HashKey::Boolean(true), Object::String("yes".to_string()));
        Object::Hash(hash)
    }

    #[test]
    fn test_builtin_keys() {
        let hash = make_test_hash();
        let result = builtin_keys(vec![hash]).unwrap();
        match result {
            Object::Array(keys) => {
                assert_eq!(keys.len(), 3);
                // Check that all expected keys are present (order is not guaranteed)
                let has_name = keys.contains(&Object::String("name".to_string()));
                let has_42 = keys.contains(&Object::Integer(42));
                let has_true = keys.contains(&Object::Boolean(true));
                assert!(has_name, "missing 'name' key");
                assert!(has_42, "missing 42 key");
                assert!(has_true, "missing true key");
            }
            _ => panic!("expected Array"),
        }
    }

    #[test]
    fn test_builtin_keys_empty() {
        let hash = Object::Hash(std::collections::HashMap::new());
        let result = builtin_keys(vec![hash]).unwrap();
        assert_eq!(result, Object::Array(vec![]));
    }

    #[test]
    fn test_builtin_values() {
        let hash = make_test_hash();
        let result = builtin_values(vec![hash]).unwrap();
        match result {
            Object::Array(values) => {
                assert_eq!(values.len(), 3);
                // Check that all expected values are present (order is not guaranteed)
                let has_alice = values.contains(&Object::String("Alice".to_string()));
                let has_100 = values.contains(&Object::Integer(100));
                let has_yes = values.contains(&Object::String("yes".to_string()));
                assert!(has_alice, "missing 'Alice' value");
                assert!(has_100, "missing 100 value");
                assert!(has_yes, "missing 'yes' value");
            }
            _ => panic!("expected Array"),
        }
    }

    #[test]
    fn test_builtin_values_empty() {
        let hash = Object::Hash(std::collections::HashMap::new());
        let result = builtin_values(vec![hash]).unwrap();
        assert_eq!(result, Object::Array(vec![]));
    }

    #[test]
    fn test_builtin_has_key_found() {
        let hash = make_test_hash();
        let result = builtin_has_key(vec![hash, Object::String("name".to_string())]).unwrap();
        assert_eq!(result, Object::Boolean(true));
    }

    #[test]
    fn test_builtin_has_key_not_found() {
        let hash = make_test_hash();
        let result = builtin_has_key(vec![hash, Object::String("email".to_string())]).unwrap();
        assert_eq!(result, Object::Boolean(false));
    }

    #[test]
    fn test_builtin_has_key_integer_key() {
        let hash = make_test_hash();
        let result = builtin_has_key(vec![hash, Object::Integer(42)]).unwrap();
        assert_eq!(result, Object::Boolean(true));
    }

    #[test]
    fn test_builtin_has_key_boolean_key() {
        let hash = make_test_hash();
        let result = builtin_has_key(vec![hash, Object::Boolean(true)]).unwrap();
        assert_eq!(result, Object::Boolean(true));
    }

    #[test]
    fn test_builtin_has_key_unhashable() {
        let hash = make_test_hash();
        let result = builtin_has_key(vec![hash, Object::Array(vec![])]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must be hashable"));
    }

    #[test]
    fn test_builtin_merge() {
        use crate::runtime::hash_key::HashKey;
        let mut h1 = std::collections::HashMap::new();
        h1.insert(HashKey::String("a".to_string()), Object::Integer(1));
        h1.insert(HashKey::String("b".to_string()), Object::Integer(2));

        let mut h2 = std::collections::HashMap::new();
        h2.insert(HashKey::String("b".to_string()), Object::Integer(20)); // overwrites
        h2.insert(HashKey::String("c".to_string()), Object::Integer(3));

        let result = builtin_merge(vec![Object::Hash(h1), Object::Hash(h2)]).unwrap();
        match result {
            Object::Hash(merged) => {
                assert_eq!(merged.len(), 3);
                assert_eq!(merged.get(&HashKey::String("a".to_string())), Some(&Object::Integer(1)));
                assert_eq!(merged.get(&HashKey::String("b".to_string())), Some(&Object::Integer(20))); // overwritten
                assert_eq!(merged.get(&HashKey::String("c".to_string())), Some(&Object::Integer(3)));
            }
            _ => panic!("expected Hash"),
        }
    }

    #[test]
    fn test_builtin_merge_empty() {
        use crate::runtime::hash_key::HashKey;
        let mut h1 = std::collections::HashMap::new();
        h1.insert(HashKey::String("a".to_string()), Object::Integer(1));

        let h2 = std::collections::HashMap::new();

        let result = builtin_merge(vec![Object::Hash(h1.clone()), Object::Hash(h2)]).unwrap();
        match result {
            Object::Hash(merged) => {
                assert_eq!(merged.len(), 1);
                assert_eq!(merged.get(&HashKey::String("a".to_string())), Some(&Object::Integer(1)));
            }
            _ => panic!("expected Hash"),
        }
    }

    #[test]
    fn test_builtin_merge_into_empty() {
        use crate::runtime::hash_key::HashKey;
        let h1 = std::collections::HashMap::new();

        let mut h2 = std::collections::HashMap::new();
        h2.insert(HashKey::String("a".to_string()), Object::Integer(1));

        let result = builtin_merge(vec![Object::Hash(h1), Object::Hash(h2)]).unwrap();
        match result {
            Object::Hash(merged) => {
                assert_eq!(merged.len(), 1);
                assert_eq!(merged.get(&HashKey::String("a".to_string())), Some(&Object::Integer(1)));
            }
            _ => panic!("expected Hash"),
        }
    }

    // =============================================================================
    // Math Builtins Tests (5.4)
    // =============================================================================

    #[test]
    fn test_builtin_abs_integer_positive() {
        let result = builtin_abs(vec![Object::Integer(5)]).unwrap();
        assert_eq!(result, Object::Integer(5));
    }

    #[test]
    fn test_builtin_abs_integer_negative() {
        let result = builtin_abs(vec![Object::Integer(-5)]).unwrap();
        assert_eq!(result, Object::Integer(5));
    }

    #[test]
    fn test_builtin_abs_integer_zero() {
        let result = builtin_abs(vec![Object::Integer(0)]).unwrap();
        assert_eq!(result, Object::Integer(0));
    }

    #[test]
    fn test_builtin_abs_float_positive() {
        let result = builtin_abs(vec![Object::Float(3.14)]).unwrap();
        assert_eq!(result, Object::Float(3.14));
    }

    #[test]
    fn test_builtin_abs_float_negative() {
        let result = builtin_abs(vec![Object::Float(-3.14)]).unwrap();
        assert_eq!(result, Object::Float(3.14));
    }

    #[test]
    fn test_builtin_abs_type_error() {
        let result = builtin_abs(vec![Object::String("hello".to_string())]);
        assert!(result.is_err());
    }

    #[test]
    fn test_builtin_min_integers() {
        let result = builtin_min(vec![Object::Integer(3), Object::Integer(7)]).unwrap();
        assert_eq!(result, Object::Integer(3));
    }

    #[test]
    fn test_builtin_min_integers_reversed() {
        let result = builtin_min(vec![Object::Integer(10), Object::Integer(2)]).unwrap();
        assert_eq!(result, Object::Integer(2));
    }

    #[test]
    fn test_builtin_min_floats() {
        let result = builtin_min(vec![Object::Float(3.5), Object::Float(2.1)]).unwrap();
        assert_eq!(result, Object::Float(2.1));
    }

    #[test]
    fn test_builtin_min_mixed() {
        let result = builtin_min(vec![Object::Integer(3), Object::Float(2.5)]).unwrap();
        assert_eq!(result, Object::Float(2.5));
    }

    #[test]
    fn test_builtin_min_negative() {
        let result = builtin_min(vec![Object::Integer(-5), Object::Integer(-10)]).unwrap();
        assert_eq!(result, Object::Integer(-10));
    }

    #[test]
    fn test_builtin_max_integers() {
        let result = builtin_max(vec![Object::Integer(3), Object::Integer(7)]).unwrap();
        assert_eq!(result, Object::Integer(7));
    }

    #[test]
    fn test_builtin_max_integers_reversed() {
        let result = builtin_max(vec![Object::Integer(10), Object::Integer(2)]).unwrap();
        assert_eq!(result, Object::Integer(10));
    }

    #[test]
    fn test_builtin_max_floats() {
        let result = builtin_max(vec![Object::Float(3.5), Object::Float(2.1)]).unwrap();
        assert_eq!(result, Object::Float(3.5));
    }

    #[test]
    fn test_builtin_max_mixed() {
        let result = builtin_max(vec![Object::Integer(3), Object::Float(3.5)]).unwrap();
        assert_eq!(result, Object::Float(3.5));
    }

    #[test]
    fn test_builtin_max_negative() {
        let result = builtin_max(vec![Object::Integer(-5), Object::Integer(-10)]).unwrap();
        assert_eq!(result, Object::Integer(-5));
    }

    #[test]
    fn test_builtin_min_type_error() {
        let result = builtin_min(vec![Object::String("a".to_string()), Object::Integer(1)]);
        assert!(result.is_err());
    }

    #[test]
    fn test_builtin_max_type_error() {
        let result = builtin_max(vec![Object::Integer(1), Object::String("a".to_string())]);
        assert!(result.is_err());
    }

    // =============================================================================
    // Type Checking Builtins Tests (5.5)
    // =============================================================================

    #[test]
    fn test_builtin_type_of_int() {
        let result = builtin_type_of(vec![Object::Integer(42)]).unwrap();
        assert_eq!(result, Object::String("Int".to_string()));
    }

    #[test]
    fn test_builtin_type_of_float() {
        let result = builtin_type_of(vec![Object::Float(3.14)]).unwrap();
        assert_eq!(result, Object::String("Float".to_string()));
    }

    #[test]
    fn test_builtin_type_of_string() {
        let result = builtin_type_of(vec![Object::String("hello".to_string())]).unwrap();
        assert_eq!(result, Object::String("String".to_string()));
    }

    #[test]
    fn test_builtin_type_of_bool() {
        let result = builtin_type_of(vec![Object::Boolean(true)]).unwrap();
        assert_eq!(result, Object::String("Bool".to_string()));
    }

    #[test]
    fn test_builtin_type_of_array() {
        let result = builtin_type_of(vec![Object::Array(vec![Object::Integer(1)])]).unwrap();
        assert_eq!(result, Object::String("Array".to_string()));
    }

    #[test]
    fn test_builtin_type_of_hash() {
        let result = builtin_type_of(vec![Object::Hash(std::collections::HashMap::new())]).unwrap();
        assert_eq!(result, Object::String("Hash".to_string()));
    }

    #[test]
    fn test_builtin_type_of_none() {
        let result = builtin_type_of(vec![Object::None]).unwrap();
        assert_eq!(result, Object::String("None".to_string()));
    }

    #[test]
    fn test_builtin_type_of_some() {
        let result = builtin_type_of(vec![Object::Some(Box::new(Object::Integer(42)))]).unwrap();
        assert_eq!(result, Object::String("Some".to_string()));
    }

    #[test]
    fn test_builtin_is_int_true() {
        let result = builtin_is_int(vec![Object::Integer(42)]).unwrap();
        assert_eq!(result, Object::Boolean(true));
    }

    #[test]
    fn test_builtin_is_int_false() {
        let result = builtin_is_int(vec![Object::Float(3.14)]).unwrap();
        assert_eq!(result, Object::Boolean(false));
    }

    #[test]
    fn test_builtin_is_float_true() {
        let result = builtin_is_float(vec![Object::Float(3.14)]).unwrap();
        assert_eq!(result, Object::Boolean(true));
    }

    #[test]
    fn test_builtin_is_float_false() {
        let result = builtin_is_float(vec![Object::Integer(42)]).unwrap();
        assert_eq!(result, Object::Boolean(false));
    }

    #[test]
    fn test_builtin_is_string_true() {
        let result = builtin_is_string(vec![Object::String("hello".to_string())]).unwrap();
        assert_eq!(result, Object::Boolean(true));
    }

    #[test]
    fn test_builtin_is_string_false() {
        let result = builtin_is_string(vec![Object::Integer(42)]).unwrap();
        assert_eq!(result, Object::Boolean(false));
    }

    #[test]
    fn test_builtin_is_bool_true() {
        let result = builtin_is_bool(vec![Object::Boolean(true)]).unwrap();
        assert_eq!(result, Object::Boolean(true));
    }

    #[test]
    fn test_builtin_is_bool_false() {
        let result = builtin_is_bool(vec![Object::Integer(0)]).unwrap();
        assert_eq!(result, Object::Boolean(false));
    }

    #[test]
    fn test_builtin_is_array_true() {
        let result = builtin_is_array(vec![Object::Array(vec![])]).unwrap();
        assert_eq!(result, Object::Boolean(true));
    }

    #[test]
    fn test_builtin_is_array_false() {
        let result = builtin_is_array(vec![Object::String("hello".to_string())]).unwrap();
        assert_eq!(result, Object::Boolean(false));
    }

    #[test]
    fn test_builtin_is_hash_true() {
        let result = builtin_is_hash(vec![Object::Hash(std::collections::HashMap::new())]).unwrap();
        assert_eq!(result, Object::Boolean(true));
    }

    #[test]
    fn test_builtin_is_hash_false() {
        let result = builtin_is_hash(vec![Object::Array(vec![])]).unwrap();
        assert_eq!(result, Object::Boolean(false));
    }

    #[test]
    fn test_builtin_is_none_true() {
        let result = builtin_is_none(vec![Object::None]).unwrap();
        assert_eq!(result, Object::Boolean(true));
    }

    #[test]
    fn test_builtin_is_none_false() {
        let result = builtin_is_none(vec![Object::Integer(42)]).unwrap();
        assert_eq!(result, Object::Boolean(false));
    }

    #[test]
    fn test_builtin_is_some_true() {
        let result = builtin_is_some(vec![Object::Some(Box::new(Object::Integer(42)))]).unwrap();
        assert_eq!(result, Object::Boolean(true));
    }

    #[test]
    fn test_builtin_is_some_false() {
        let result = builtin_is_some(vec![Object::None]).unwrap();
        assert_eq!(result, Object::Boolean(false));
    }
}
