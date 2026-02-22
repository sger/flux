use crate::runtime::{RuntimeContext, base_function::BaseFunction, value::Value};

mod registry;
pub use registry::{
    BaseModule, get_base_function, get_base_function_by_index, get_base_function_index,
};

mod array_ops;
mod assert_ops;
mod collection_ops;
mod hash_ops;
mod helpers;
mod higher_order_ops;
mod io_ops;
pub(crate) mod list_ops;
mod numeric_ops;
mod string_ops;
mod type_check;

use array_ops::base_sort;
use assert_ops::{
    base_assert_eq, base_assert_false, base_assert_neq, base_assert_throws, base_assert_true,
};
use collection_ops::{
    base_concat, base_contains, base_first, base_last, base_len, base_product, base_push,
    base_range, base_rest, base_reverse, base_slice, base_sum,
};
use hash_ops::{
    base_delete, base_get, base_has_key, base_is_map, base_keys, base_merge, base_put, base_values,
};
use higher_order_ops::{
    base_all, base_any, base_count, base_filter, base_find, base_flat_map, base_flatten, base_fold,
    base_map, base_sort_by, base_zip,
};
use io_ops::{
    base_now_ms, base_parse_int, base_parse_ints, base_read_file, base_read_lines, base_read_stdin,
    base_split_ints, base_time,
};
use list_ops::{base_hd, base_is_list, base_list, base_tl, base_to_array, base_to_list};
use numeric_ops::{base_abs, base_max, base_min};
use string_ops::{
    base_chars, base_ends_with, base_join, base_lower, base_replace, base_split, base_starts_with,
    base_substring, base_to_string, base_trim, base_upper,
};
use type_check::{
    base_is_array, base_is_bool, base_is_float, base_is_hash, base_is_int, base_is_none,
    base_is_some, base_is_string, base_type_of,
};

fn base_print(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
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
        func: base_print,
    },
    BaseFunction {
        name: "len",
        func: base_len,
    },
    BaseFunction {
        name: "first",
        func: base_first,
    },
    BaseFunction {
        name: "last",
        func: base_last,
    },
    BaseFunction {
        name: "rest",
        func: base_rest,
    },
    BaseFunction {
        name: "push",
        func: base_push,
    },
    BaseFunction {
        name: "to_string",
        func: base_to_string,
    },
    BaseFunction {
        name: "concat",
        func: base_concat,
    },
    BaseFunction {
        name: "reverse",
        func: base_reverse,
    },
    BaseFunction {
        name: "contains",
        func: base_contains,
    },
    BaseFunction {
        name: "slice",
        func: base_slice,
    },
    BaseFunction {
        name: "sort",
        func: base_sort,
    },
    BaseFunction {
        name: "split",
        func: base_split,
    },
    BaseFunction {
        name: "join",
        func: base_join,
    },
    BaseFunction {
        name: "trim",
        func: base_trim,
    },
    BaseFunction {
        name: "upper",
        func: base_upper,
    },
    BaseFunction {
        name: "lower",
        func: base_lower,
    },
    BaseFunction {
        name: "starts_with",
        func: base_starts_with,
    },
    BaseFunction {
        name: "ends_with",
        func: base_ends_with,
    },
    BaseFunction {
        name: "replace",
        func: base_replace,
    },
    BaseFunction {
        name: "chars",
        func: base_chars,
    },
    BaseFunction {
        name: "substring",
        func: base_substring,
    },
    BaseFunction {
        name: "keys",
        func: base_keys,
    },
    BaseFunction {
        name: "values",
        func: base_values,
    },
    BaseFunction {
        name: "has_key",
        func: base_has_key,
    },
    BaseFunction {
        name: "merge",
        func: base_merge,
    },
    BaseFunction {
        name: "delete",
        func: base_delete,
    },
    BaseFunction {
        name: "abs",
        func: base_abs,
    },
    BaseFunction {
        name: "min",
        func: base_min,
    },
    BaseFunction {
        name: "max",
        func: base_max,
    },
    BaseFunction {
        name: "type_of",
        func: base_type_of,
    },
    BaseFunction {
        name: "is_int",
        func: base_is_int,
    },
    BaseFunction {
        name: "is_float",
        func: base_is_float,
    },
    BaseFunction {
        name: "is_string",
        func: base_is_string,
    },
    BaseFunction {
        name: "is_bool",
        func: base_is_bool,
    },
    BaseFunction {
        name: "is_array",
        func: base_is_array,
    },
    BaseFunction {
        name: "is_hash",
        func: base_is_hash,
    },
    BaseFunction {
        name: "is_none",
        func: base_is_none,
    },
    BaseFunction {
        name: "is_some",
        func: base_is_some,
    },
    BaseFunction {
        name: "map",
        func: base_map,
    },
    BaseFunction {
        name: "filter",
        func: base_filter,
    },
    BaseFunction {
        name: "fold",
        func: base_fold,
    },
    // List base_functions (persistent cons-cell lists)
    BaseFunction {
        name: "hd",
        func: base_hd,
    },
    BaseFunction {
        name: "tl",
        func: base_tl,
    },
    BaseFunction {
        name: "is_list",
        func: base_is_list,
    },
    BaseFunction {
        name: "to_list",
        func: base_to_list,
    },
    BaseFunction {
        name: "to_array",
        func: base_to_array,
    },
    // Map base_functions (persistent HAMT maps)
    BaseFunction {
        name: "put",
        func: base_put,
    },
    BaseFunction {
        name: "get",
        func: base_get,
    },
    BaseFunction {
        name: "is_map",
        func: base_is_map,
    },
    BaseFunction {
        name: "list",
        func: base_list,
    },
    BaseFunction {
        name: "read_file",
        func: base_read_file,
    },
    BaseFunction {
        name: "read_lines",
        func: base_read_lines,
    },
    BaseFunction {
        name: "read_stdin",
        func: base_read_stdin,
    },
    BaseFunction {
        name: "parse_int",
        func: base_parse_int,
    },
    BaseFunction {
        name: "now_ms",
        func: base_now_ms,
    },
    BaseFunction {
        name: "time",
        func: base_time,
    },
    BaseFunction {
        name: "range",
        func: base_range,
    },
    BaseFunction {
        name: "sum",
        func: base_sum,
    },
    BaseFunction {
        name: "product",
        func: base_product,
    },
    BaseFunction {
        name: "parse_ints",
        func: base_parse_ints,
    },
    BaseFunction {
        name: "split_ints",
        func: base_split_ints,
    },
    BaseFunction {
        name: "flat_map",
        func: base_flat_map,
    },
    // Higher-order search and sort base_functions
    BaseFunction {
        name: "any",
        func: base_any,
    },
    BaseFunction {
        name: "all",
        func: base_all,
    },
    BaseFunction {
        name: "find",
        func: base_find,
    },
    BaseFunction {
        name: "sort_by",
        func: base_sort_by,
    },
    BaseFunction {
        name: "zip",
        func: base_zip,
    },
    BaseFunction {
        name: "flatten",
        func: base_flatten,
    },
    BaseFunction {
        name: "count",
        func: base_count,
    },
    // Assert base_functions (test framework)
    BaseFunction {
        name: "assert_eq",
        func: base_assert_eq,
    },
    BaseFunction {
        name: "assert_neq",
        func: base_assert_neq,
    },
    BaseFunction {
        name: "assert_true",
        func: base_assert_true,
    },
    BaseFunction {
        name: "assert_false",
        func: base_assert_false,
    },
    BaseFunction {
        name: "assert_throws",
        func: base_assert_throws,
    },
];

#[cfg(test)]
mod collection_ops_test;
#[cfg(test)]
mod hash_ops_test;
#[cfg(test)]
mod helpers_test;
#[cfg(test)]
mod higher_order_ops_test;
#[cfg(test)]
mod numeric_ops_test;
#[cfg(test)]
mod string_ops_test;
#[cfg(test)]
mod type_check_test;
