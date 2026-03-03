use crate::runtime::{RuntimeContext, base_function::BaseFunction, value::Value};

mod registry;
pub use registry::{
    BaseModule, get_base_function, get_base_function_by_index, get_base_function_index,
    is_base_fastcall_allowlisted,
};

mod array_ops;
mod assert_ops;
mod base_hm_effect_row;
mod base_hm_signature;
mod base_hm_signature_id;
mod base_hm_type;
mod collection_ops;
mod hash_ops;
mod helpers;
pub(crate) use base_hm_signature_id::BaseHmSignatureId;
pub(crate) use helpers::scheme_for_signature_id;
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
        hm_signature: BaseHmSignatureId::Print,
        func: base_print,
    },
    BaseFunction {
        name: "len",
        hm_signature: BaseHmSignatureId::Len,
        func: base_len,
    },
    BaseFunction {
        name: "first",
        hm_signature: BaseHmSignatureId::First,
        func: base_first,
    },
    BaseFunction {
        name: "last",
        hm_signature: BaseHmSignatureId::Last,
        func: base_last,
    },
    BaseFunction {
        name: "rest",
        hm_signature: BaseHmSignatureId::Rest,
        func: base_rest,
    },
    BaseFunction {
        name: "push",
        hm_signature: BaseHmSignatureId::Push,
        func: base_push,
    },
    BaseFunction {
        name: "to_string",
        hm_signature: BaseHmSignatureId::ToString,
        func: base_to_string,
    },
    BaseFunction {
        name: "concat",
        hm_signature: BaseHmSignatureId::Concat,
        func: base_concat,
    },
    BaseFunction {
        name: "reverse",
        hm_signature: BaseHmSignatureId::Reverse,
        func: base_reverse,
    },
    BaseFunction {
        name: "contains",
        hm_signature: BaseHmSignatureId::Contains,
        func: base_contains,
    },
    BaseFunction {
        name: "slice",
        hm_signature: BaseHmSignatureId::Slice,
        func: base_slice,
    },
    BaseFunction {
        name: "sort",
        hm_signature: BaseHmSignatureId::Sort,
        func: base_sort,
    },
    BaseFunction {
        name: "split",
        hm_signature: BaseHmSignatureId::Split,
        func: base_split,
    },
    BaseFunction {
        name: "join",
        hm_signature: BaseHmSignatureId::Join,
        func: base_join,
    },
    BaseFunction {
        name: "trim",
        hm_signature: BaseHmSignatureId::Trim,
        func: base_trim,
    },
    BaseFunction {
        name: "upper",
        hm_signature: BaseHmSignatureId::Upper,
        func: base_upper,
    },
    BaseFunction {
        name: "lower",
        hm_signature: BaseHmSignatureId::Lower,
        func: base_lower,
    },
    BaseFunction {
        name: "starts_with",
        hm_signature: BaseHmSignatureId::StartsWith,
        func: base_starts_with,
    },
    BaseFunction {
        name: "ends_with",
        hm_signature: BaseHmSignatureId::EndsWith,
        func: base_ends_with,
    },
    BaseFunction {
        name: "replace",
        hm_signature: BaseHmSignatureId::Replace,
        func: base_replace,
    },
    BaseFunction {
        name: "chars",
        hm_signature: BaseHmSignatureId::Chars,
        func: base_chars,
    },
    BaseFunction {
        name: "substring",
        hm_signature: BaseHmSignatureId::Substring,
        func: base_substring,
    },
    BaseFunction {
        name: "keys",
        hm_signature: BaseHmSignatureId::Keys,
        func: base_keys,
    },
    BaseFunction {
        name: "values",
        hm_signature: BaseHmSignatureId::Values,
        func: base_values,
    },
    BaseFunction {
        name: "has_key",
        hm_signature: BaseHmSignatureId::HasKey,
        func: base_has_key,
    },
    BaseFunction {
        name: "merge",
        hm_signature: BaseHmSignatureId::Merge,
        func: base_merge,
    },
    BaseFunction {
        name: "delete",
        hm_signature: BaseHmSignatureId::Delete,
        func: base_delete,
    },
    BaseFunction {
        name: "abs",
        hm_signature: BaseHmSignatureId::Abs,
        func: base_abs,
    },
    BaseFunction {
        name: "min",
        hm_signature: BaseHmSignatureId::Min,
        func: base_min,
    },
    BaseFunction {
        name: "max",
        hm_signature: BaseHmSignatureId::Max,
        func: base_max,
    },
    BaseFunction {
        name: "type_of",
        hm_signature: BaseHmSignatureId::TypeOf,
        func: base_type_of,
    },
    BaseFunction {
        name: "is_int",
        hm_signature: BaseHmSignatureId::IsInt,
        func: base_is_int,
    },
    BaseFunction {
        name: "is_float",
        hm_signature: BaseHmSignatureId::IsFloat,
        func: base_is_float,
    },
    BaseFunction {
        name: "is_string",
        hm_signature: BaseHmSignatureId::IsString,
        func: base_is_string,
    },
    BaseFunction {
        name: "is_bool",
        hm_signature: BaseHmSignatureId::IsBool,
        func: base_is_bool,
    },
    BaseFunction {
        name: "is_array",
        hm_signature: BaseHmSignatureId::IsArray,
        func: base_is_array,
    },
    BaseFunction {
        name: "is_hash",
        hm_signature: BaseHmSignatureId::IsHash,
        func: base_is_hash,
    },
    BaseFunction {
        name: "is_none",
        hm_signature: BaseHmSignatureId::IsNone,
        func: base_is_none,
    },
    BaseFunction {
        name: "is_some",
        hm_signature: BaseHmSignatureId::IsSome,
        func: base_is_some,
    },
    BaseFunction {
        name: "map",
        hm_signature: BaseHmSignatureId::Map,
        func: base_map,
    },
    BaseFunction {
        name: "filter",
        hm_signature: BaseHmSignatureId::Filter,
        func: base_filter,
    },
    BaseFunction {
        name: "fold",
        hm_signature: BaseHmSignatureId::Fold,
        func: base_fold,
    },
    // List base_functions (persistent cons-cell lists)
    BaseFunction {
        name: "hd",
        hm_signature: BaseHmSignatureId::Hd,
        func: base_hd,
    },
    BaseFunction {
        name: "tl",
        hm_signature: BaseHmSignatureId::Tl,
        func: base_tl,
    },
    BaseFunction {
        name: "is_list",
        hm_signature: BaseHmSignatureId::IsList,
        func: base_is_list,
    },
    BaseFunction {
        name: "to_list",
        hm_signature: BaseHmSignatureId::ToList,
        func: base_to_list,
    },
    BaseFunction {
        name: "to_array",
        hm_signature: BaseHmSignatureId::ToArray,
        func: base_to_array,
    },
    // Map base_functions (persistent HAMT maps)
    BaseFunction {
        name: "put",
        hm_signature: BaseHmSignatureId::Put,
        func: base_put,
    },
    BaseFunction {
        name: "get",
        hm_signature: BaseHmSignatureId::Get,
        func: base_get,
    },
    BaseFunction {
        name: "is_map",
        hm_signature: BaseHmSignatureId::IsMap,
        func: base_is_map,
    },
    BaseFunction {
        name: "list",
        hm_signature: BaseHmSignatureId::List,
        func: base_list,
    },
    BaseFunction {
        name: "read_file",
        hm_signature: BaseHmSignatureId::ReadFile,
        func: base_read_file,
    },
    BaseFunction {
        name: "read_lines",
        hm_signature: BaseHmSignatureId::ReadLines,
        func: base_read_lines,
    },
    BaseFunction {
        name: "read_stdin",
        hm_signature: BaseHmSignatureId::ReadStdin,
        func: base_read_stdin,
    },
    BaseFunction {
        name: "parse_int",
        hm_signature: BaseHmSignatureId::ParseInt,
        func: base_parse_int,
    },
    BaseFunction {
        name: "now_ms",
        hm_signature: BaseHmSignatureId::NowMs,
        func: base_now_ms,
    },
    BaseFunction {
        name: "time",
        hm_signature: BaseHmSignatureId::Time,
        func: base_time,
    },
    BaseFunction {
        name: "range",
        hm_signature: BaseHmSignatureId::Range,
        func: base_range,
    },
    BaseFunction {
        name: "sum",
        hm_signature: BaseHmSignatureId::Sum,
        func: base_sum,
    },
    BaseFunction {
        name: "product",
        hm_signature: BaseHmSignatureId::Product,
        func: base_product,
    },
    BaseFunction {
        name: "parse_ints",
        hm_signature: BaseHmSignatureId::ParseInts,
        func: base_parse_ints,
    },
    BaseFunction {
        name: "split_ints",
        hm_signature: BaseHmSignatureId::SplitInts,
        func: base_split_ints,
    },
    BaseFunction {
        name: "flat_map",
        hm_signature: BaseHmSignatureId::FlatMap,
        func: base_flat_map,
    },
    // Higher-order search and sort base_functions
    BaseFunction {
        name: "any",
        hm_signature: BaseHmSignatureId::Any,
        func: base_any,
    },
    BaseFunction {
        name: "all",
        hm_signature: BaseHmSignatureId::All,
        func: base_all,
    },
    BaseFunction {
        name: "find",
        hm_signature: BaseHmSignatureId::Find,
        func: base_find,
    },
    BaseFunction {
        name: "sort_by",
        hm_signature: BaseHmSignatureId::SortBy,
        func: base_sort_by,
    },
    BaseFunction {
        name: "zip",
        hm_signature: BaseHmSignatureId::Zip,
        func: base_zip,
    },
    BaseFunction {
        name: "flatten",
        hm_signature: BaseHmSignatureId::Flatten,
        func: base_flatten,
    },
    BaseFunction {
        name: "count",
        hm_signature: BaseHmSignatureId::Count,
        func: base_count,
    },
    // Assert base_functions (test framework)
    BaseFunction {
        name: "assert_eq",
        hm_signature: BaseHmSignatureId::AssertEq,
        func: base_assert_eq,
    },
    BaseFunction {
        name: "assert_neq",
        hm_signature: BaseHmSignatureId::AssertNeq,
        func: base_assert_neq,
    },
    BaseFunction {
        name: "assert_true",
        hm_signature: BaseHmSignatureId::AssertTrue,
        func: base_assert_true,
    },
    BaseFunction {
        name: "assert_false",
        hm_signature: BaseHmSignatureId::AssertFalse,
        func: base_assert_false,
    },
    BaseFunction {
        name: "assert_throws",
        hm_signature: BaseHmSignatureId::AssertThrows,
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
