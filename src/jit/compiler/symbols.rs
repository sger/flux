use super::*;

pub(super) struct HelperFuncs {
    pub(super) ids: HashMap<&'static str, FuncId>,
}

pub(super) fn get_helper_func_ref(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    name: &str,
) -> cranelift_codegen::ir::FuncRef {
    let func_id = helpers.ids[name];
    module.declare_func_in_func(func_id, builder.func)
}
pub(super) struct HelperSig {
    pub(super) num_params: usize,
    pub(super) num_returns: usize,
}

pub(super) fn helper_signatures() -> Vec<(&'static str, HelperSig)> {
    vec![
        // Value constructors
        (
            "rt_make_integer",
            HelperSig {
                num_params: 2,
                num_returns: 2,
            },
        ),
        (
            "rt_make_float",
            HelperSig {
                num_params: 2,
                num_returns: 2,
            },
        ),
        (
            "rt_make_bool",
            HelperSig {
                num_params: 2,
                num_returns: 2,
            },
        ),
        (
            "rt_division_by_zero",
            HelperSig {
                num_params: 1,
                num_returns: 0,
            },
        ),
        // rt_render_error_with_span(ctx, start_line, start_col, end_line, end_col)
        (
            "rt_render_error_with_span",
            HelperSig {
                num_params: 5,
                num_returns: 0,
            },
        ),
        // rt_has_error(ctx) -> i64 (0 or 1)
        (
            "rt_has_error",
            HelperSig {
                num_params: 1,
                num_returns: 1,
            },
        ),
        (
            "rt_make_none",
            HelperSig {
                num_params: 1,
                num_returns: 2,
            },
        ),
        (
            "rt_force_boxed",
            HelperSig {
                num_params: 3,
                num_returns: 2,
            },
        ),
        (
            "rt_push_gc_roots",
            HelperSig {
                num_params: 3,
                num_returns: 0,
            },
        ),
        (
            "rt_pop_gc_roots",
            HelperSig {
                num_params: 1,
                num_returns: 0,
            },
        ),
        (
            "rt_make_empty_list",
            HelperSig {
                num_params: 1,
                num_returns: 1,
            },
        ),
        (
            "rt_make_string",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        (
            "rt_make_base_function",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        (
            "rt_make_jit_closure",
            HelperSig {
                num_params: 4,
                num_returns: 1,
            },
        ),
        (
            "rt_make_cons",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        // Arithmetic
        (
            "rt_add",
            HelperSig {
                num_params: 5,
                num_returns: 2,
            },
        ),
        (
            "rt_sub",
            HelperSig {
                num_params: 5,
                num_returns: 2,
            },
        ),
        (
            "rt_mul",
            HelperSig {
                num_params: 5,
                num_returns: 2,
            },
        ),
        (
            "rt_div",
            HelperSig {
                num_params: 5,
                num_returns: 2,
            },
        ),
        (
            "rt_mod",
            HelperSig {
                num_params: 5,
                num_returns: 2,
            },
        ),
        // Prefix
        (
            "rt_negate",
            HelperSig {
                num_params: 3,
                num_returns: 2,
            },
        ),
        (
            "rt_not",
            HelperSig {
                num_params: 3,
                num_returns: 2,
            },
        ),
        (
            "rt_is_truthy",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        (
            "rt_bool_value",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        (
            "rt_is_cons",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        (
            "rt_cons_head",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        (
            "rt_cons_tail",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        // Comparisons
        (
            "rt_equal",
            HelperSig {
                num_params: 5,
                num_returns: 2,
            },
        ),
        (
            "rt_not_equal",
            HelperSig {
                num_params: 5,
                num_returns: 2,
            },
        ),
        (
            "rt_greater_than",
            HelperSig {
                num_params: 5,
                num_returns: 2,
            },
        ),
        (
            "rt_less_than_or_equal",
            HelperSig {
                num_params: 5,
                num_returns: 2,
            },
        ),
        (
            "rt_greater_than_or_equal",
            HelperSig {
                num_params: 5,
                num_returns: 2,
            },
        ),
        // rt_set_thunk(ctx, fn_index, args_ptr, nargs) -> JitTaggedValue
        (
            "rt_set_thunk",
            HelperSig {
                num_params: 4,
                num_returns: 2,
            },
        ),
        // BaseFunctions & globals
        // rt_call_base_function_tagged(ctx, idx, tagged_args_ptr, nargs, sl, sc, el, ec) -> *mut Value
        (
            "rt_call_base_function_tagged",
            HelperSig {
                num_params: 8,
                num_returns: 1,
            },
        ),
        (
            "rt_call_base_function",
            HelperSig {
                num_params: 8,
                num_returns: 1,
            },
        ),
        (
            "rt_call_primop",
            HelperSig {
                num_params: 8,
                num_returns: 1,
            },
        ),
        (
            "rt_call_value",
            HelperSig {
                num_params: 8,
                num_returns: 1,
            },
        ),
        (
            "rt_call_jit_function",
            HelperSig {
                num_params: 8,
                num_returns: 1,
            },
        ),
        (
            "rt_get_global",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        (
            "rt_set_global",
            HelperSig {
                num_params: 3,
                num_returns: 0,
            },
        ),
        (
            "rt_set_arity_error",
            HelperSig {
                num_params: 3,
                num_returns: 0,
            },
        ),
        (
            "rt_check_jit_contract_call",
            HelperSig {
                num_params: 8,
                num_returns: 1,
            },
        ),
        (
            "rt_check_jit_contract_call1",
            HelperSig {
                num_params: 7,
                num_returns: 1,
            },
        ),
        (
            "rt_check_jit_contract_call2",
            HelperSig {
                num_params: 8,
                num_returns: 1,
            },
        ),
        (
            "rt_check_jit_contract_call3",
            HelperSig {
                num_params: 9,
                num_returns: 1,
            },
        ),
        (
            "rt_check_jit_contract_call4",
            HelperSig {
                num_params: 10,
                num_returns: 1,
            },
        ),
        (
            "rt_check_jit_contract_return",
            HelperSig {
                num_params: 7,
                num_returns: 1,
            },
        ),
        // Phase 4: value wrappers (ctx, value) -> *mut Value
        (
            "rt_make_some",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        (
            "rt_make_left",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        (
            "rt_make_right",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        // Phase 4: pattern matching checks (ctx, value) -> i64
        (
            "rt_is_some",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        (
            "rt_is_left",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        (
            "rt_is_right",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        (
            "rt_is_none",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        (
            "rt_is_empty_list",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        // Phase 4: unwrap helpers (ctx, value) -> *mut Value
        (
            "rt_unwrap_some",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        (
            "rt_unwrap_left",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        (
            "rt_unwrap_right",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        // Phase 4: structural equality (ctx, a, b) -> i64
        (
            "rt_values_equal",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        // Phase 4: collections
        (
            "rt_make_array",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        (
            "rt_make_tuple",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        (
            "rt_make_hash",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        (
            "rt_index",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        (
            "rt_is_tuple",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        (
            "rt_tuple_len_eq",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        (
            "rt_tuple_get",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        // Phase 4: string ops (ctx, value) -> *mut Value
        (
            "rt_to_string",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        // rt_string_concat(ctx, a_ptr, b_ptr) -> *mut Value
        (
            "rt_string_concat",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        // Phase 5: ADT helpers
        // rt_intern_unit_adt(ctx, constructor_ptr, constructor_len) -> *mut Value
        (
            "rt_intern_unit_adt",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        // rt_make_adt(ctx, constructor_ptr, constructor_len, fields_ptr, arity) -> *mut Value
        (
            "rt_make_adt",
            HelperSig {
                num_params: 5,
                num_returns: 1,
            },
        ),
        // rt_make_adt1(ctx, constructor_ptr, constructor_len, f0) -> *mut Value
        (
            "rt_make_adt1",
            HelperSig {
                num_params: 4,
                num_returns: 1,
            },
        ),
        // rt_make_adt2(ctx, constructor_ptr, constructor_len, f0, f1) -> *mut Value
        (
            "rt_make_adt2",
            HelperSig {
                num_params: 5,
                num_returns: 1,
            },
        ),
        // rt_make_adt3(ctx, constructor_ptr, constructor_len, f0, f1, f2) -> *mut Value
        (
            "rt_make_adt3",
            HelperSig {
                num_params: 6,
                num_returns: 1,
            },
        ),
        // rt_make_adt4(ctx, constructor_ptr, constructor_len, f0, f1, f2, f3) -> *mut Value
        (
            "rt_make_adt4",
            HelperSig {
                num_params: 7,
                num_returns: 1,
            },
        ),
        // rt_make_adt5(ctx, constructor_ptr, constructor_len, f0, f1, f2, f3, f4) -> *mut Value
        (
            "rt_make_adt5",
            HelperSig {
                num_params: 8,
                num_returns: 1,
            },
        ),
        // rt_is_adt_constructor(ctx, value, constructor_ptr, constructor_len) -> i64
        (
            "rt_is_adt_constructor",
            HelperSig {
                num_params: 4,
                num_returns: 1,
            },
        ),
        // rt_adt_field(ctx, value, field_idx) -> *mut Value
        (
            "rt_adt_field",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        // rt_adt_field_or_none(ctx, value, field_idx) -> *mut Value
        (
            "rt_adt_field_or_none",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        // Algebraic effects
        // rt_push_handler(ctx, effect_id, ops_ptr, closures_ptr, narms) -> void
        (
            "rt_push_handler",
            HelperSig {
                num_params: 5,
                num_returns: 0,
            },
        ),
        // rt_pop_handler(ctx) -> void
        (
            "rt_pop_handler",
            HelperSig {
                num_params: 1,
                num_returns: 0,
            },
        ),
        // rt_perform(ctx, effect_id, op_id, args_ptr, nargs,
        //            effect_name_ptr, effect_name_len, op_name_ptr, op_name_len,
        //            line, column) -> *mut Value
        (
            "rt_perform",
            HelperSig {
                num_params: 11,
                num_returns: 1,
            },
        ),
        // Aether: rt_aether_drop(ctx, val_ptr) -> void
        (
            "rt_aether_drop",
            HelperSig {
                num_params: 2,
                num_returns: 0,
            },
        ),
        // Aether Perceus reuse: rt_drop_reuse(ctx, val_ptr) -> *mut Value
        (
            "rt_drop_reuse",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
        // rt_reuse_cons(ctx, token, head, tail) -> *mut Value
        (
            "rt_reuse_cons",
            HelperSig {
                num_params: 4,
                num_returns: 1,
            },
        ),
        // rt_reuse_some(ctx, token, inner) -> *mut Value
        (
            "rt_reuse_some",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        // rt_reuse_left(ctx, token, inner) -> *mut Value
        (
            "rt_reuse_left",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        // rt_reuse_right(ctx, token, inner) -> *mut Value
        (
            "rt_reuse_right",
            HelperSig {
                num_params: 3,
                num_returns: 1,
            },
        ),
        // rt_reuse_adt(ctx, token, name_ptr, name_len, fields_ptr, nfields) -> *mut Value
        (
            "rt_reuse_adt",
            HelperSig {
                num_params: 6,
                num_returns: 1,
            },
        ),
        // rt_reuse_cons_masked(ctx, token, head, tail, field_mask) -> *mut Value
        (
            "rt_reuse_cons_masked",
            HelperSig {
                num_params: 5,
                num_returns: 1,
            },
        ),
        // rt_reuse_adt_masked(ctx, token, name_ptr, name_len, fields_ptr, nfields, field_mask) -> *mut Value
        (
            "rt_reuse_adt_masked",
            HelperSig {
                num_params: 7,
                num_returns: 1,
            },
        ),
        // rt_is_unique(ctx, val) -> i64
        (
            "rt_is_unique",
            HelperSig {
                num_params: 2,
                num_returns: 1,
            },
        ),
    ]
}

pub(super) fn default_libcall_names()
-> Box<dyn Fn(cranelift_codegen::ir::LibCall) -> String + Send + Sync> {
    cranelift_module::default_libcall_names()
}
