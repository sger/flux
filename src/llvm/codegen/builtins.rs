//! Built-in function support for llvm.
//!
//! Maps Flux built-in function names to C runtime function declarations.
//! Only functions with a direct C runtime equivalent are supported;
//! others require closure wrapping (future work).

use crate::llvm::Linkage;
use crate::llvm::{CallConv, GlobalId, LlvmDecl, LlvmFunctionSig, LlvmModule, LlvmType};

/// Describes a built-in function's C runtime mapping.
pub struct BuiltinMapping {
    /// The Flux built-in function name (e.g., "print").
    pub flux_name: &'static str,
    /// The C runtime function name (e.g., "flux_println").
    pub c_name: &'static str,
    /// Number of parameters.
    pub arity: usize,
    /// Whether the function returns a value (i64) or void.
    pub returns_value: bool,
}

/// Known built-in function → C runtime mappings.
static BUILTIN_MAPPINGS: &[BuiltinMapping] = &[
    // I/O
    BuiltinMapping {
        flux_name: "print",
        c_name: "flux_print",
        arity: 1,
        returns_value: false,
    },
    BuiltinMapping {
        flux_name: "println",
        c_name: "flux_println",
        arity: 1,
        returns_value: false,
    },
    // String conversion
    BuiltinMapping {
        flux_name: "to_string",
        c_name: "flux_to_string",
        arity: 1,
        returns_value: true,
    },
    // String operations
    BuiltinMapping {
        flux_name: "str_concat",
        c_name: "flux_string_concat",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "str_length",
        c_name: "flux_string_length",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "str_slice",
        c_name: "flux_string_slice",
        arity: 3,
        returns_value: true,
    },
    // HAMT
    BuiltinMapping {
        flux_name: "hamt_empty",
        c_name: "flux_hamt_empty",
        arity: 0,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "hamt_get",
        c_name: "flux_hamt_get",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "hamt_set",
        c_name: "flux_hamt_set",
        arity: 3,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "hamt_delete",
        c_name: "flux_hamt_delete",
        arity: 2,
        returns_value: true,
    },
    // I/O (file)
    BuiltinMapping {
        flux_name: "read_file",
        c_name: "flux_read_file",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "read_stdin",
        c_name: "flux_read_line",
        arity: 0,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "read_lines",
        c_name: "flux_read_lines",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "write_file",
        c_name: "flux_write_file",
        arity: 2,
        returns_value: true,
    },
    // Numeric
    BuiltinMapping {
        flux_name: "abs",
        c_name: "flux_abs",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "sqrt",
        c_name: "flux_sqrt",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "sin",
        c_name: "flux_sin",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "cos",
        c_name: "flux_cos",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "exp",
        c_name: "flux_exp",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "log",
        c_name: "flux_log",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "floor",
        c_name: "flux_floor",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "ceil",
        c_name: "flux_ceil",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "round",
        c_name: "flux_round",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "min",
        c_name: "flux_min",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "max",
        c_name: "flux_max",
        arity: 2,
        returns_value: true,
    },
    // Type inspection
    BuiltinMapping {
        flux_name: "type_of",
        c_name: "flux_type_of",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "is_int",
        c_name: "flux_is_int",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "is_float",
        c_name: "flux_is_float",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "is_string",
        c_name: "flux_is_string",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "is_bool",
        c_name: "flux_is_bool",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "is_none",
        c_name: "flux_is_none",
        arity: 1,
        returns_value: true,
    },
    // String operations
    BuiltinMapping {
        flux_name: "trim",
        c_name: "flux_trim",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "upper",
        c_name: "flux_upper",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "lower",
        c_name: "flux_lower",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "replace",
        c_name: "flux_replace",
        arity: 3,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "chars",
        c_name: "flux_chars",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "str_contains",
        c_name: "flux_str_contains",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "substring",
        c_name: "flux_substring",
        arity: 3,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "parse_int",
        c_name: "flux_parse_int",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "parse_ints",
        c_name: "flux_parse_ints",
        arity: 1,
        returns_value: true,
    },
    // Array operations
    BuiltinMapping {
        flux_name: "len",
        c_name: "flux_rt_len",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "push",
        c_name: "flux_array_push",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "concat",
        c_name: "flux_array_concat",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "reverse",
        c_name: "flux_array_reverse",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "slice",
        c_name: "flux_array_slice",
        arity: 3,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "contains",
        c_name: "flux_array_contains",
        arity: 2,
        returns_value: true,
    },
    // HAMT extended
    BuiltinMapping {
        flux_name: "put",
        c_name: "flux_hamt_set",
        arity: 3,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "get",
        c_name: "flux_hamt_get_option",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "has_key",
        c_name: "flux_hamt_contains",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "delete",
        c_name: "flux_hamt_delete",
        arity: 2,
        returns_value: true,
    },
    // Control
    BuiltinMapping {
        flux_name: "panic",
        c_name: "flux_panic",
        arity: 1,
        returns_value: false,
    },
    BuiltinMapping {
        flux_name: "now_ms",
        c_name: "flux_clock_now",
        arity: 0,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "time",
        c_name: "flux_clock_now",
        arity: 0,
        returns_value: true,
    },
    // Collection helpers
    BuiltinMapping {
        flux_name: "keys",
        c_name: "flux_hamt_keys",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "split",
        c_name: "flux_split",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "to_list",
        c_name: "flux_to_list",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "is_array",
        c_name: "flux_is_array",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "is_map",
        c_name: "flux_is_map",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "is_hash",
        c_name: "flux_is_map",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "values",
        c_name: "flux_hamt_values",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "len",
        c_name: "flux_rt_len",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "length",
        c_name: "flux_rt_len",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "size",
        c_name: "flux_hamt_size",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "contains",
        c_name: "flux_array_contains",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "push",
        c_name: "flux_array_push",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "reverse",
        c_name: "flux_array_reverse",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "concat",
        c_name: "flux_array_concat",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "trim",
        c_name: "flux_trim",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "upper",
        c_name: "flux_upper",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "lower",
        c_name: "flux_lower",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "replace",
        c_name: "flux_replace",
        arity: 3,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "chars",
        c_name: "flux_chars",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "str_contains",
        c_name: "flux_str_contains",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "join",
        c_name: "flux_join",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "parse_int",
        c_name: "flux_parse_int",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "int_to_string",
        c_name: "flux_int_to_string",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "is_list",
        c_name: "flux_is_list",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "is_some",
        c_name: "flux_is_some",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "is_none",
        c_name: "flux_is_none",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "merge",
        c_name: "flux_hamt_merge",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "unwrap",
        c_name: "flux_unwrap",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "safe_div",
        c_name: "flux_safe_div",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "safe_mod",
        c_name: "flux_safe_mod",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "unwrap_or",
        c_name: "flux_unwrap_or",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "to_array",
        c_name: "flux_to_array",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "type_of",
        c_name: "flux_type_of",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "abs",
        c_name: "flux_abs",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "sqrt",
        c_name: "flux_sqrt",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "sin",
        c_name: "flux_sin",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "cos",
        c_name: "flux_cos",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "exp",
        c_name: "flux_exp",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "log",
        c_name: "flux_log",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "floor",
        c_name: "flux_floor",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "ceil",
        c_name: "flux_ceil",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "round",
        c_name: "flux_round",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "min",
        c_name: "flux_min",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "max",
        c_name: "flux_max",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "array_len",
        c_name: "flux_array_len",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "array_push",
        c_name: "flux_array_push",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "array_get",
        c_name: "flux_array_get",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "array_set",
        c_name: "flux_array_set",
        arity: 3,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "array_slice",
        c_name: "flux_array_slice",
        arity: 3,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "string_length",
        c_name: "flux_string_length",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "string_len",
        c_name: "flux_string_length",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "substring",
        c_name: "flux_substring",
        arity: 3,
        returns_value: true,
    },
    // Higher-order functions (call closures via flux_call_closure_c trampoline)
    BuiltinMapping {
        flux_name: "map",
        c_name: "flux_ho_map",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "filter",
        c_name: "flux_ho_filter",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "any",
        c_name: "flux_ho_any",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "all",
        c_name: "flux_ho_all",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "fold",
        c_name: "flux_ho_fold",
        arity: 3,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "each",
        c_name: "flux_ho_each",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "find",
        c_name: "flux_ho_find",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "sort_by",
        c_name: "flux_ho_sort_by",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "zip",
        c_name: "flux_zip",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "sum",
        c_name: "flux_sum",
        arity: 1,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "starts_with",
        c_name: "flux_starts_with",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "ends_with",
        c_name: "flux_ends_with",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "split_ints",
        c_name: "flux_split_ints",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "count",
        c_name: "flux_ho_count",
        arity: 2,
        returns_value: true,
    },
    // Deep structural comparison (used by Flow.Assert)
    BuiltinMapping {
        flux_name: "cmp_eq",
        c_name: "flux_rt_eq",
        arity: 2,
        returns_value: true,
    },
    BuiltinMapping {
        flux_name: "cmp_ne",
        c_name: "flux_rt_neq",
        arity: 2,
        returns_value: true,
    },
];

/// Look up a built-in function's C runtime mapping by Flux name.
pub fn find_builtin(name: &str) -> Option<&'static BuiltinMapping> {
    BUILTIN_MAPPINGS.iter().find(|m| m.flux_name == name)
}

/// Ensure the C runtime declaration for a builtin exists in the module.
pub fn ensure_builtin_declared(module: &mut LlvmModule, mapping: &BuiltinMapping) {
    let name = mapping.c_name;
    // Check if already declared.
    if module.declarations.iter().any(|d| d.name.0 == name)
        || module.functions.iter().any(|f| f.name.0 == name)
    {
        return;
    }

    let params = vec![LlvmType::i64(); mapping.arity];
    let ret = if mapping.returns_value {
        LlvmType::i64()
    } else {
        LlvmType::Void
    };

    module.declarations.push(LlvmDecl {
        linkage: Linkage::External,
        name: GlobalId(name.into()),
        sig: LlvmFunctionSig {
            ret,
            params,
            varargs: false,
            call_conv: CallConv::Ccc,
        },
        attrs: vec!["nounwind".into()],
    });
}

/// Check if a name (resolved via interner) is a known built-in function.
#[allow(dead_code)]
pub fn is_known_builtin(name: &str) -> bool {
    find_builtin(name).is_some()
}
