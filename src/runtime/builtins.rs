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
