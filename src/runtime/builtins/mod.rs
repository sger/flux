use crate::runtime::{RuntimeContext, builtin_function::BuiltinFunction, value::Value};

mod array_ops;
mod hash_ops;
mod helpers;
pub(crate) mod list_ops;
mod numeric_ops;
mod string_ops;
mod type_check;

use array_ops::{
    builtin_concat, builtin_contains, builtin_filter, builtin_first, builtin_fold, builtin_last,
    builtin_len, builtin_map, builtin_push, builtin_rest, builtin_reverse, builtin_slice,
    builtin_sort,
};
use hash_ops::{
    builtin_delete, builtin_get, builtin_has_key, builtin_is_map, builtin_keys, builtin_merge,
    builtin_put, builtin_values,
};
use list_ops::{
    builtin_hd, builtin_is_list, builtin_list, builtin_tl, builtin_to_array, builtin_to_list,
};
use numeric_ops::{builtin_abs, builtin_max, builtin_min};
use string_ops::{
    builtin_chars, builtin_ends_with, builtin_join, builtin_lower, builtin_replace, builtin_split,
    builtin_starts_with, builtin_substring, builtin_to_string, builtin_trim, builtin_upper,
};
use type_check::{
    builtin_is_array, builtin_is_bool, builtin_is_float, builtin_is_hash, builtin_is_int,
    builtin_is_none, builtin_is_some, builtin_is_string, builtin_type_of,
};

fn builtin_print(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            print!(" ");
        }
        match arg {
            Value::String(s) => print!("{}", s), // Raw string
            Value::Gc(_) => print!("{}", list_ops::format_value(ctx, arg)),
            _ => print!("{}", arg),
        }
    }
    println!();
    Ok(Value::None)
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
        name: "starts_with",
        func: builtin_starts_with,
    },
    BuiltinFunction {
        name: "ends_with",
        func: builtin_ends_with,
    },
    BuiltinFunction {
        name: "replace",
        func: builtin_replace,
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
        name: "delete",
        func: builtin_delete,
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
    BuiltinFunction {
        name: "map",
        func: builtin_map,
    },
    BuiltinFunction {
        name: "filter",
        func: builtin_filter,
    },
    BuiltinFunction {
        name: "fold",
        func: builtin_fold,
    },
    // List builtins (persistent cons-cell lists)
    BuiltinFunction {
        name: "hd",
        func: builtin_hd,
    },
    BuiltinFunction {
        name: "tl",
        func: builtin_tl,
    },
    BuiltinFunction {
        name: "is_list",
        func: builtin_is_list,
    },
    BuiltinFunction {
        name: "to_list",
        func: builtin_to_list,
    },
    BuiltinFunction {
        name: "to_array",
        func: builtin_to_array,
    },
    // Map builtins (persistent HAMT maps)
    BuiltinFunction {
        name: "put",
        func: builtin_put,
    },
    BuiltinFunction {
        name: "get",
        func: builtin_get,
    },
    BuiltinFunction {
        name: "is_map",
        func: builtin_is_map,
    },
    BuiltinFunction {
        name: "list",
        func: builtin_list,
    },
];

pub fn get_builtin(name: &str) -> Option<&'static BuiltinFunction> {
    BUILTINS.iter().find(|b| b.name == name)
}

pub fn get_builtin_by_index(index: usize) -> Option<&'static BuiltinFunction> {
    BUILTINS.get(index)
}

#[cfg(test)]
mod array_ops_test;
#[cfg(test)]
mod hash_ops_test;
#[cfg(test)]
mod helpers_test;
#[cfg(test)]
mod numeric_ops_test;
#[cfg(test)]
mod string_ops_test;
#[cfg(test)]
mod type_check_test;
