use crate::runtime::{builtin_function::BuiltinFunction, object::Object};

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
    if args.len() != 1 {
        return Err(format!(
            "wrong number of arguments. got={}, want=1",
            args.len()
        ));
    }
    match &args[0] {
        Object::String(s) => Ok(Object::Integer(s.len() as i64)),
        Object::Array(arr) => Ok(Object::Integer(arr.len() as i64)),
        _ => Err(format!(
            "argument to `len` not supported, got {}",
            args[0].type_name()
        )),
    }
}

fn builtin_first(args: Vec<Object>) -> Result<Object, String> {
    if args.len() != 1 {
        return Err(format!(
            "wrong number of arguments. got={}, want=1",
            args.len()
        ));
    }
    match &args[0] {
        Object::Array(arr) => {
            if arr.is_empty() {
                Ok(Object::None)
            } else {
                Ok(arr[0].clone())
            }
        }
        _ => Err(format!(
            "argument to `first` must be Array, got {}",
            args[0].type_name()
        )),
    }
}

fn builtin_last(args: Vec<Object>) -> Result<Object, String> {
    if args.len() != 1 {
        return Err(format!(
            "wrong number of arguments. got={}, want=1",
            args.len()
        ));
    }
    match &args[0] {
        Object::Array(arr) => {
            if arr.is_empty() {
                Ok(Object::None)
            } else {
                Ok(arr[arr.len() - 1].clone())
            }
        }
        _ => Err(format!(
            "argument to `last` must be Array, got {}",
            args[0].type_name()
        )),
    }
}

fn builtin_rest(args: Vec<Object>) -> Result<Object, String> {
    if args.len() != 1 {
        return Err(format!(
            "wrong number of arguments. got={}, want=1",
            args.len()
        ));
    }
    match &args[0] {
        Object::Array(arr) => {
            if arr.is_empty() {
                Ok(Object::None)
            } else {
                Ok(Object::Array(arr[1..].to_vec()))
            }
        }
        _ => Err(format!(
            "argument to `rest` must be Array, got {}",
            args[0].type_name()
        )),
    }
}

fn builtin_push(args: Vec<Object>) -> Result<Object, String> {
    if args.len() != 2 {
        return Err(format!(
            "wrong number of arguments. got={}, want=2",
            args.len()
        ));
    }
    match &args[0] {
        Object::Array(arr) => {
            let mut new_arr = arr.clone();
            new_arr.push(args[1].clone());
            Ok(Object::Array(new_arr))
        }
        _ => Err(format!(
            "argument to `push` must be Array, got {}",
            args[0].type_name()
        )),
    }
}

fn builtin_to_string(args: Vec<Object>) -> Result<Object, String> {
    if args.len() != 1 {
        return Err(format!(
            "wrong number of arguments. got={}, want=1",
            args.len()
        ));
    }
    Ok(Object::String(args[0].to_string_value()))
}

/// concat(a, b) - Concatenate two arrays into a new array
fn builtin_concat(args: Vec<Object>) -> Result<Object, String> {
    if args.len() != 2 {
        return Err(format!(
            "wrong number of arguments. got={}, want=2",
            args.len()
        ));
    }

    match (&args[0], &args[1]) {
        (Object::Array(a), Object::Array(b)) => {
            let mut result = a.clone();
            result.extend(b.iter().cloned());
            Ok(Object::Array(result))
        }
        (Object::Array(_), other) => Err(format!(
            "second argument to `concat` must be Array, got {}",
            other.type_name(),
        )),
        (other, _) => Err(format!(
            "first argument to `concat` must be Array, got {}",
            other.type_name()
        )),
    }
}

/// reverse(arr) - Return a new array with elements in reverse order
fn builtin_reverse(args: Vec<Object>) -> Result<Object, String> {
    if args.len() != 1 {
        return Err(format!(
            "wrong number of arguments. got={}, want=1",
            args.len()
        ));
    }

    match &args[0] {
        Object::Array(arr) => {
            let mut result = arr.clone();
            result.reverse();
            Ok(Object::Array(result))
        }
        other => Err(format!(
            "argument to `reverse` must be Array, got {}",
            other.type_name()
        )),
    }
}

/// contains(arr, elem) - Check if array contains an element
fn builtin_contains(args: Vec<Object>) -> Result<Object, String> {
    if args.len() != 2 {
        return Err(format!(
            "wrong number of arguments. got={}, want=2",
            args.len()
        ));
    }

    match &args[0] {
        Object::Array(arr) => {
            let elem = &args[1];
            let found = arr.iter().any(|item| item == elem);
            Ok(Object::Boolean(found))
        }
        other => Err(format!(
            "first argument to `contains` must be Array, got {}",
            other.type_name()
        )),
    }
}

/// slice(arr, start, end) - Return a slice of the array from start to end (exclusive)
fn builtin_slice(args: Vec<Object>) -> Result<Object, String> {
    if args.len() != 3 {
        return Err(format!(
            "wrong number of arguments. got={}, want=3",
            args.len()
        ));
    }

    match (&args[0], &args[1], &args[2]) {
        (Object::Array(arr), Object::Integer(start), Object::Integer(end)) => {
            let len = arr.len() as i64;
            let start = if *start < 0 { 0 } else { *start as usize };
            let end = if *end > len {
                len as usize
            } else {
                *end as usize
            };
            if start >= end || start >= arr.len() {
                Ok(Object::Array(vec![]))
            } else {
                Ok(Object::Array(arr[start..end].to_vec()))
            }
        }
        (Object::Array(_), Object::Integer(_), other) => Err(format!(
            "third argument to `slice` must be Integer, got {}",
            other.type_name()
        )),
        (Object::Array(_), other, _) => Err(format!(
            "second argument to `slice` must be Integer, got {}",
            other.type_name()
        )),
        (other, _, _) => Err(format!(
            "first argument to `slice` must be Array, got {}",
            other.type_name()
        )),
    }
}

/// sort(arr) or sort(arr, order) - Return a new sorted array
/// order: "asc" (default) or "desc"
/// Only works with integers/floats
fn builtin_sort(args: Vec<Object>) -> Result<Object, String> {
    if args.is_empty() || args.len() > 2 {
        return Err(format!(
            "wrong number of arguments. got={}, want=1 or 2",
            args.len()
        ));
    }

    // Determine sort order (default: ascending)
    let descending = if args.len() == 2 {
        match &args[1] {
            Object::String(s) => match s.as_str() {
                "asc" => false,
                "desc" => true,
                _ => return Err(format!("sort order must be \"asc\" or \"desc\", got \"{}\"", s)),
            },
            other => {
                return Err(format!(
                    "second argument to `sort` must be String, got {}",
                    other.type_name()
                ))
            }
        }
    } else {
        false
    };

    match &args[0] {
        Object::Array(arr) => {
            // Check if all elements are comparable (integers or floats)
            let all_numeric = arr
                .iter()
                .all(|item| matches!(item, Object::Integer(_) | Object::Float(_)));

            if !all_numeric && !arr.is_empty() {
                return Err("sort only supports arrays of integers or floats".to_string());
            }

            let mut result = arr.clone();

            result.sort_by(|a, b| {
                use std::cmp::Ordering;
                // Smart comparison: avoid f64 conversion when both are same type
                let cmp = match (a, b) {
                    (Object::Integer(i1), Object::Integer(i2)) => i1.cmp(i2),
                    (Object::Float(f1), Object::Float(f2)) => {
                        f1.partial_cmp(f2).unwrap_or(Ordering::Equal)
                    }
                    (Object::Integer(i), Object::Float(f)) => {
                        (*i as f64).partial_cmp(f).unwrap_or(Ordering::Equal)
                    }
                    (Object::Float(f), Object::Integer(i)) => {
                        f.partial_cmp(&(*i as f64)).unwrap_or(Ordering::Equal)
                    }
                    _ => Ordering::Equal,
                };
                if descending {
                    cmp.reverse()
                } else {
                    cmp
                }
            });
            Ok(Object::Array(result))
        }
        other => Err(format!(
            "first argument to `sort` must be Array, got {}",
            other.type_name()
        )),
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
}
