use crate::runtime::{RuntimeContext, base_function::BaseFunction, value::Value};

mod registry;
pub use registry::{
    BaseModule, get_base_function, get_base_function_by_index, get_base_function_index,
};

mod array_ops;
mod assert_ops;
mod hash_ops;
mod helpers;
mod io_ops;
pub(crate) mod list_ops;
mod numeric_ops;
mod string_ops;
mod type_check;

use array_ops::{
    builtin_all, builtin_any, builtin_concat, builtin_contains, builtin_count, builtin_filter,
    builtin_find, builtin_first, builtin_flat_map, builtin_flatten, builtin_fold, builtin_last,
    builtin_len, builtin_map, builtin_product, builtin_push, builtin_range, builtin_rest,
    builtin_reverse, builtin_slice, builtin_sort, builtin_sort_by, builtin_sum, builtin_zip,
};
use assert_ops::{
    builtin_assert_eq, builtin_assert_false, builtin_assert_neq, builtin_assert_throws,
    builtin_assert_true,
};
use hash_ops::{
    builtin_delete, builtin_get, builtin_has_key, builtin_is_map, builtin_keys, builtin_merge,
    builtin_put, builtin_values,
};
use io_ops::{
    builtin_now_ms, builtin_parse_int, builtin_parse_ints, builtin_read_file, builtin_read_lines,
    builtin_read_stdin, builtin_split_ints, builtin_time,
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
            Value::Gc(_) | Value::Tuple(_) | Value::Array(_) => {
                print!("{}", list_ops::format_value(ctx, arg))
            }
            _ => print!("{}", arg),
        }
    }
    println!();
    Ok(Value::None)
}

/// All Base functions in deterministic index order.
pub static BASE_FUNCTIONS: &[BaseFunction] = &[
    BaseFunction {
        name: "print",
        func: builtin_print,
    },
    BaseFunction {
        name: "len",
        func: builtin_len,
    },
    BaseFunction {
        name: "first",
        func: builtin_first,
    },
    BaseFunction {
        name: "last",
        func: builtin_last,
    },
    BaseFunction {
        name: "rest",
        func: builtin_rest,
    },
    BaseFunction {
        name: "push",
        func: builtin_push,
    },
    BaseFunction {
        name: "to_string",
        func: builtin_to_string,
    },
    BaseFunction {
        name: "concat",
        func: builtin_concat,
    },
    BaseFunction {
        name: "reverse",
        func: builtin_reverse,
    },
    BaseFunction {
        name: "contains",
        func: builtin_contains,
    },
    BaseFunction {
        name: "slice",
        func: builtin_slice,
    },
    BaseFunction {
        name: "sort",
        func: builtin_sort,
    },
    BaseFunction {
        name: "split",
        func: builtin_split,
    },
    BaseFunction {
        name: "join",
        func: builtin_join,
    },
    BaseFunction {
        name: "trim",
        func: builtin_trim,
    },
    BaseFunction {
        name: "upper",
        func: builtin_upper,
    },
    BaseFunction {
        name: "lower",
        func: builtin_lower,
    },
    BaseFunction {
        name: "starts_with",
        func: builtin_starts_with,
    },
    BaseFunction {
        name: "ends_with",
        func: builtin_ends_with,
    },
    BaseFunction {
        name: "replace",
        func: builtin_replace,
    },
    BaseFunction {
        name: "chars",
        func: builtin_chars,
    },
    BaseFunction {
        name: "substring",
        func: builtin_substring,
    },
    BaseFunction {
        name: "keys",
        func: builtin_keys,
    },
    BaseFunction {
        name: "values",
        func: builtin_values,
    },
    BaseFunction {
        name: "has_key",
        func: builtin_has_key,
    },
    BaseFunction {
        name: "merge",
        func: builtin_merge,
    },
    BaseFunction {
        name: "delete",
        func: builtin_delete,
    },
    BaseFunction {
        name: "abs",
        func: builtin_abs,
    },
    BaseFunction {
        name: "min",
        func: builtin_min,
    },
    BaseFunction {
        name: "max",
        func: builtin_max,
    },
    BaseFunction {
        name: "type_of",
        func: builtin_type_of,
    },
    BaseFunction {
        name: "is_int",
        func: builtin_is_int,
    },
    BaseFunction {
        name: "is_float",
        func: builtin_is_float,
    },
    BaseFunction {
        name: "is_string",
        func: builtin_is_string,
    },
    BaseFunction {
        name: "is_bool",
        func: builtin_is_bool,
    },
    BaseFunction {
        name: "is_array",
        func: builtin_is_array,
    },
    BaseFunction {
        name: "is_hash",
        func: builtin_is_hash,
    },
    BaseFunction {
        name: "is_none",
        func: builtin_is_none,
    },
    BaseFunction {
        name: "is_some",
        func: builtin_is_some,
    },
    BaseFunction {
        name: "map",
        func: builtin_map,
    },
    BaseFunction {
        name: "filter",
        func: builtin_filter,
    },
    BaseFunction {
        name: "fold",
        func: builtin_fold,
    },
    // List base_functions (persistent cons-cell lists)
    BaseFunction {
        name: "hd",
        func: builtin_hd,
    },
    BaseFunction {
        name: "tl",
        func: builtin_tl,
    },
    BaseFunction {
        name: "is_list",
        func: builtin_is_list,
    },
    BaseFunction {
        name: "to_list",
        func: builtin_to_list,
    },
    BaseFunction {
        name: "to_array",
        func: builtin_to_array,
    },
    // Map base_functions (persistent HAMT maps)
    BaseFunction {
        name: "put",
        func: builtin_put,
    },
    BaseFunction {
        name: "get",
        func: builtin_get,
    },
    BaseFunction {
        name: "is_map",
        func: builtin_is_map,
    },
    BaseFunction {
        name: "list",
        func: builtin_list,
    },
    BaseFunction {
        name: "read_file",
        func: builtin_read_file,
    },
    BaseFunction {
        name: "read_lines",
        func: builtin_read_lines,
    },
    BaseFunction {
        name: "read_stdin",
        func: builtin_read_stdin,
    },
    BaseFunction {
        name: "parse_int",
        func: builtin_parse_int,
    },
    BaseFunction {
        name: "now_ms",
        func: builtin_now_ms,
    },
    BaseFunction {
        name: "time",
        func: builtin_time,
    },
    BaseFunction {
        name: "range",
        func: builtin_range,
    },
    BaseFunction {
        name: "sum",
        func: builtin_sum,
    },
    BaseFunction {
        name: "product",
        func: builtin_product,
    },
    BaseFunction {
        name: "parse_ints",
        func: builtin_parse_ints,
    },
    BaseFunction {
        name: "split_ints",
        func: builtin_split_ints,
    },
    BaseFunction {
        name: "flat_map",
        func: builtin_flat_map,
    },
    // Higher-order search and sort base_functions
    BaseFunction {
        name: "any",
        func: builtin_any,
    },
    BaseFunction {
        name: "all",
        func: builtin_all,
    },
    BaseFunction {
        name: "find",
        func: builtin_find,
    },
    BaseFunction {
        name: "sort_by",
        func: builtin_sort_by,
    },
    BaseFunction {
        name: "zip",
        func: builtin_zip,
    },
    BaseFunction {
        name: "flatten",
        func: builtin_flatten,
    },
    BaseFunction {
        name: "count",
        func: builtin_count,
    },
    // Assert base_functions (test framework)
    BaseFunction {
        name: "assert_eq",
        func: builtin_assert_eq,
    },
    BaseFunction {
        name: "assert_neq",
        func: builtin_assert_neq,
    },
    BaseFunction {
        name: "assert_true",
        func: builtin_assert_true,
    },
    BaseFunction {
        name: "assert_false",
        func: builtin_assert_false,
    },
    BaseFunction {
        name: "assert_throws",
        func: builtin_assert_throws,
    },
];

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
