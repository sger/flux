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
    base_assert_eq, base_assert_eq_borrowed, base_assert_false, base_assert_false_borrowed,
    base_assert_neq, base_assert_neq_borrowed, base_assert_throws, base_assert_throws_borrowed,
    base_assert_true, base_assert_true_borrowed,
};
use collection_ops::{
    base_concat, base_contains, base_contains_borrowed, base_first, base_first_borrowed, base_last,
    base_last_borrowed, base_len, base_len_borrowed, base_product, base_product_borrowed,
    base_push, base_range, base_range_borrowed, base_rest, base_rest_borrowed, base_reverse,
    base_reverse_borrowed, base_slice, base_slice_borrowed, base_sum, base_sum_borrowed,
};
use hash_ops::{
    base_delete, base_delete_borrowed, base_get, base_get_borrowed, base_has_key,
    base_has_key_borrowed, base_is_map, base_is_map_borrowed, base_keys, base_keys_borrowed,
    base_merge, base_merge_borrowed, base_put, base_put_borrowed, base_values,
    base_values_borrowed,
};
use higher_order_ops::{
    base_all, base_any, base_count, base_filter, base_find, base_flat_map, base_flatten, base_fold,
    base_map, base_sort_by, base_zip,
};
use io_ops::{
    base_now_ms, base_now_ms_borrowed, base_parse_int, base_parse_int_borrowed, base_parse_ints,
    base_parse_ints_borrowed, base_read_file, base_read_file_borrowed, base_read_lines,
    base_read_lines_borrowed, base_read_stdin, base_read_stdin_borrowed, base_split_ints,
    base_split_ints_borrowed, base_time, base_time_borrowed,
};
use list_ops::{
    base_hd, base_hd_borrowed, base_is_list, base_is_list_borrowed, base_list, base_tl,
    base_tl_borrowed, base_to_array, base_to_array_borrowed, base_to_list, base_to_list_borrowed,
};
use numeric_ops::{
    base_abs, base_abs_borrowed, base_max, base_max_borrowed, base_min, base_min_borrowed,
};
use string_ops::{
    base_chars, base_chars_borrowed, base_ends_with, base_ends_with_borrowed, base_join,
    base_join_borrowed, base_lower, base_lower_borrowed, base_replace, base_replace_borrowed,
    base_split, base_split_borrowed, base_starts_with, base_starts_with_borrowed, base_substring,
    base_substring_borrowed, base_to_string, base_to_string_borrowed, base_trim,
    base_trim_borrowed, base_upper, base_upper_borrowed,
};
use type_check::{
    base_is_array, base_is_array_borrowed, base_is_bool, base_is_bool_borrowed, base_is_float,
    base_is_float_borrowed, base_is_hash, base_is_hash_borrowed, base_is_int, base_is_int_borrowed,
    base_is_none, base_is_none_borrowed, base_is_some, base_is_some_borrowed, base_is_string,
    base_is_string_borrowed, base_type_of, base_type_of_borrowed,
};

fn base_print(ctx: &mut dyn RuntimeContext, args: Vec<Value>) -> Result<Value, String> {
    let borrowed: Vec<&Value> = args.iter().collect();
    base_print_borrowed(ctx, &borrowed)
}

fn base_print_borrowed(ctx: &mut dyn RuntimeContext, args: &[&Value]) -> Result<Value, String> {
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            print!(" ");
        }
        match arg {
            Value::String(s) => print!("{}", s), // Raw string
            Value::Gc(_) | Value::GcAdt(_) | Value::Tuple(_) | Value::Array(_) | Value::Adt(_) => {
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
    BaseFunction::preferred(
        "print",
        BaseHmSignatureId::Print,
        base_print_borrowed,
        base_print,
    ),
    BaseFunction::preferred("len", BaseHmSignatureId::Len, base_len_borrowed, base_len),
    BaseFunction::preferred(
        "first",
        BaseHmSignatureId::First,
        base_first_borrowed,
        base_first,
    ),
    BaseFunction::preferred(
        "last",
        BaseHmSignatureId::Last,
        base_last_borrowed,
        base_last,
    ),
    BaseFunction::preferred(
        "rest",
        BaseHmSignatureId::Rest,
        base_rest_borrowed,
        base_rest,
    ),
    BaseFunction::owned("push", BaseHmSignatureId::Push, base_push),
    BaseFunction::preferred(
        "to_string",
        BaseHmSignatureId::ToString,
        base_to_string_borrowed,
        base_to_string,
    ),
    BaseFunction::owned("concat", BaseHmSignatureId::Concat, base_concat),
    BaseFunction::preferred(
        "reverse",
        BaseHmSignatureId::Reverse,
        base_reverse_borrowed,
        base_reverse,
    ),
    BaseFunction::preferred(
        "contains",
        BaseHmSignatureId::Contains,
        base_contains_borrowed,
        base_contains,
    ),
    BaseFunction::preferred(
        "slice",
        BaseHmSignatureId::Slice,
        base_slice_borrowed,
        base_slice,
    ),
    BaseFunction::owned("sort", BaseHmSignatureId::Sort, base_sort),
    BaseFunction::preferred(
        "split",
        BaseHmSignatureId::Split,
        base_split_borrowed,
        base_split,
    ),
    BaseFunction::preferred(
        "join",
        BaseHmSignatureId::Join,
        base_join_borrowed,
        base_join,
    ),
    BaseFunction::preferred(
        "trim",
        BaseHmSignatureId::Trim,
        base_trim_borrowed,
        base_trim,
    ),
    BaseFunction::preferred(
        "upper",
        BaseHmSignatureId::Upper,
        base_upper_borrowed,
        base_upper,
    ),
    BaseFunction::preferred(
        "lower",
        BaseHmSignatureId::Lower,
        base_lower_borrowed,
        base_lower,
    ),
    BaseFunction::preferred(
        "starts_with",
        BaseHmSignatureId::StartsWith,
        base_starts_with_borrowed,
        base_starts_with,
    ),
    BaseFunction::preferred(
        "ends_with",
        BaseHmSignatureId::EndsWith,
        base_ends_with_borrowed,
        base_ends_with,
    ),
    BaseFunction::preferred(
        "replace",
        BaseHmSignatureId::Replace,
        base_replace_borrowed,
        base_replace,
    ),
    BaseFunction::preferred(
        "chars",
        BaseHmSignatureId::Chars,
        base_chars_borrowed,
        base_chars,
    ),
    BaseFunction::preferred(
        "substring",
        BaseHmSignatureId::Substring,
        base_substring_borrowed,
        base_substring,
    ),
    BaseFunction::preferred(
        "keys",
        BaseHmSignatureId::Keys,
        base_keys_borrowed,
        base_keys,
    ),
    BaseFunction::preferred(
        "values",
        BaseHmSignatureId::Values,
        base_values_borrowed,
        base_values,
    ),
    BaseFunction::preferred(
        "has_key",
        BaseHmSignatureId::HasKey,
        base_has_key_borrowed,
        base_has_key,
    ),
    BaseFunction::preferred(
        "merge",
        BaseHmSignatureId::Merge,
        base_merge_borrowed,
        base_merge,
    ),
    BaseFunction::preferred(
        "delete",
        BaseHmSignatureId::Delete,
        base_delete_borrowed,
        base_delete,
    ),
    BaseFunction::preferred("abs", BaseHmSignatureId::Abs, base_abs_borrowed, base_abs),
    BaseFunction::preferred("min", BaseHmSignatureId::Min, base_min_borrowed, base_min),
    BaseFunction::preferred("max", BaseHmSignatureId::Max, base_max_borrowed, base_max),
    BaseFunction::preferred(
        "type_of",
        BaseHmSignatureId::TypeOf,
        base_type_of_borrowed,
        base_type_of,
    ),
    BaseFunction::preferred(
        "is_int",
        BaseHmSignatureId::IsInt,
        base_is_int_borrowed,
        base_is_int,
    ),
    BaseFunction::preferred(
        "is_float",
        BaseHmSignatureId::IsFloat,
        base_is_float_borrowed,
        base_is_float,
    ),
    BaseFunction::preferred(
        "is_string",
        BaseHmSignatureId::IsString,
        base_is_string_borrowed,
        base_is_string,
    ),
    BaseFunction::preferred(
        "is_bool",
        BaseHmSignatureId::IsBool,
        base_is_bool_borrowed,
        base_is_bool,
    ),
    BaseFunction::preferred(
        "is_array",
        BaseHmSignatureId::IsArray,
        base_is_array_borrowed,
        base_is_array,
    ),
    BaseFunction::preferred(
        "is_hash",
        BaseHmSignatureId::IsHash,
        base_is_hash_borrowed,
        base_is_hash,
    ),
    BaseFunction::preferred(
        "is_none",
        BaseHmSignatureId::IsNone,
        base_is_none_borrowed,
        base_is_none,
    ),
    BaseFunction::preferred(
        "is_some",
        BaseHmSignatureId::IsSome,
        base_is_some_borrowed,
        base_is_some,
    ),
    BaseFunction::owned("map", BaseHmSignatureId::Map, base_map),
    BaseFunction::owned("filter", BaseHmSignatureId::Filter, base_filter),
    BaseFunction::owned("fold", BaseHmSignatureId::Fold, base_fold),
    // List base_functions (persistent cons-cell lists)
    BaseFunction::preferred("hd", BaseHmSignatureId::Hd, base_hd_borrowed, base_hd),
    BaseFunction::preferred("tl", BaseHmSignatureId::Tl, base_tl_borrowed, base_tl),
    BaseFunction::preferred(
        "is_list",
        BaseHmSignatureId::IsList,
        base_is_list_borrowed,
        base_is_list,
    ),
    BaseFunction::preferred(
        "to_list",
        BaseHmSignatureId::ToList,
        base_to_list_borrowed,
        base_to_list,
    ),
    BaseFunction::preferred(
        "to_array",
        BaseHmSignatureId::ToArray,
        base_to_array_borrowed,
        base_to_array,
    ),
    // Map base_functions (persistent HAMT maps)
    BaseFunction::preferred("put", BaseHmSignatureId::Put, base_put_borrowed, base_put),
    BaseFunction::preferred("get", BaseHmSignatureId::Get, base_get_borrowed, base_get),
    BaseFunction::preferred(
        "is_map",
        BaseHmSignatureId::IsMap,
        base_is_map_borrowed,
        base_is_map,
    ),
    BaseFunction::owned("list", BaseHmSignatureId::List, base_list),
    BaseFunction::preferred(
        "read_file",
        BaseHmSignatureId::ReadFile,
        base_read_file_borrowed,
        base_read_file,
    ),
    BaseFunction::preferred(
        "read_lines",
        BaseHmSignatureId::ReadLines,
        base_read_lines_borrowed,
        base_read_lines,
    ),
    BaseFunction::preferred(
        "read_stdin",
        BaseHmSignatureId::ReadStdin,
        base_read_stdin_borrowed,
        base_read_stdin,
    ),
    BaseFunction::preferred(
        "parse_int",
        BaseHmSignatureId::ParseInt,
        base_parse_int_borrowed,
        base_parse_int,
    ),
    BaseFunction::preferred(
        "now_ms",
        BaseHmSignatureId::NowMs,
        base_now_ms_borrowed,
        base_now_ms,
    ),
    BaseFunction::preferred(
        "time",
        BaseHmSignatureId::Time,
        base_time_borrowed,
        base_time,
    ),
    BaseFunction::preferred(
        "range",
        BaseHmSignatureId::Range,
        base_range_borrowed,
        base_range,
    ),
    BaseFunction::preferred("sum", BaseHmSignatureId::Sum, base_sum_borrowed, base_sum),
    BaseFunction::preferred(
        "product",
        BaseHmSignatureId::Product,
        base_product_borrowed,
        base_product,
    ),
    BaseFunction::preferred(
        "parse_ints",
        BaseHmSignatureId::ParseInts,
        base_parse_ints_borrowed,
        base_parse_ints,
    ),
    BaseFunction::preferred(
        "split_ints",
        BaseHmSignatureId::SplitInts,
        base_split_ints_borrowed,
        base_split_ints,
    ),
    BaseFunction::owned("flat_map", BaseHmSignatureId::FlatMap, base_flat_map),
    // Higher-order search and sort base_functions
    BaseFunction::owned("any", BaseHmSignatureId::Any, base_any),
    BaseFunction::owned("all", BaseHmSignatureId::All, base_all),
    BaseFunction::owned("find", BaseHmSignatureId::Find, base_find),
    BaseFunction::owned("sort_by", BaseHmSignatureId::SortBy, base_sort_by),
    BaseFunction::owned("zip", BaseHmSignatureId::Zip, base_zip),
    BaseFunction::owned("flatten", BaseHmSignatureId::Flatten, base_flatten),
    BaseFunction::owned("count", BaseHmSignatureId::Count, base_count),
    // Assert base_functions (test framework)
    BaseFunction::preferred(
        "assert_eq",
        BaseHmSignatureId::AssertEq,
        base_assert_eq_borrowed,
        base_assert_eq,
    ),
    BaseFunction::preferred(
        "assert_neq",
        BaseHmSignatureId::AssertNeq,
        base_assert_neq_borrowed,
        base_assert_neq,
    ),
    BaseFunction::preferred(
        "assert_true",
        BaseHmSignatureId::AssertTrue,
        base_assert_true_borrowed,
        base_assert_true,
    ),
    BaseFunction::preferred(
        "assert_false",
        BaseHmSignatureId::AssertFalse,
        base_assert_false_borrowed,
        base_assert_false,
    ),
    BaseFunction::preferred(
        "assert_throws",
        BaseHmSignatureId::AssertThrows,
        base_assert_throws_borrowed,
        base_assert_throws,
    ),
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
