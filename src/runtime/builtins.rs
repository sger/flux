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
}
