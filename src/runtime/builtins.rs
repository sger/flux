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
}
