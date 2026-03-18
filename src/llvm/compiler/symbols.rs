//! Symbol collection and declaration: ADT constructors, module functions,
//! runtime helper declarations, symbol resolution, and user function forward declarations.

use llvm_sys::prelude::*;

use crate::cfg::IrProgram;
use crate::runtime::native_helpers::rt_symbols;
use crate::syntax::interner::Interner;

use super::super::context::LlvmCompilerContext;
use super::super::wrapper::function_type;

// ADT constructor and module function collection is shared with the JIT
// backend via `crate::backend_ir::metadata`.

// ── Runtime helper declaration ───────────────────────────────────────────────

/// Declare all `rt_*` functions as external symbols in the LLVM module.
pub(super) fn declare_runtime_helpers(ctx: &mut LlvmCompilerContext) {
    let i64_ty = ctx.i64_type;
    let ptr_ty = ctx.ptr_type;
    let tv_ty = ctx.tagged_value_type;
    let void_ty = ctx.void_type;

    // Helper signature descriptors: (name, return_type, param_types)
    let helpers: Vec<(&str, LLVMTypeRef, Vec<LLVMTypeRef>)> = vec![
        // Arithmetic: (ctx, tv, tv) -> tv  — but the C ABI flattens JitTaggedValue
        // rt_add(ctx: ptr, a_tag: i64, a_payload: i64, b_tag: i64, b_payload: i64) -> {i64, i64}
        (
            "rt_add",
            tv_ty,
            vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty],
        ),
        (
            "rt_sub",
            tv_ty,
            vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty],
        ),
        (
            "rt_mul",
            tv_ty,
            vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty],
        ),
        (
            "rt_div",
            tv_ty,
            vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty],
        ),
        (
            "rt_mod",
            tv_ty,
            vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty],
        ),
        // Comparison: same signature
        (
            "rt_equal",
            tv_ty,
            vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty],
        ),
        (
            "rt_not_equal",
            tv_ty,
            vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty],
        ),
        (
            "rt_greater_than",
            tv_ty,
            vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty],
        ),
        (
            "rt_less_than_or_equal",
            tv_ty,
            vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty],
        ),
        (
            "rt_greater_than_or_equal",
            tv_ty,
            vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty],
        ),
        // Unary
        ("rt_negate", tv_ty, vec![ptr_ty, i64_ty, i64_ty]),
        ("rt_not", tv_ty, vec![ptr_ty, i64_ty, i64_ty]),
        // Truthiness: (ctx, tag, payload) -> i64 (0 or 1)
        ("rt_is_truthy", i64_ty, vec![ptr_ty, i64_ty, i64_ty]),
        // Value constructors
        ("rt_make_integer", tv_ty, vec![ptr_ty, i64_ty]),
        ("rt_make_float", tv_ty, vec![ptr_ty, i64_ty]),
        ("rt_make_bool", tv_ty, vec![ptr_ty, i64_ty]),
        ("rt_make_none", tv_ty, vec![ptr_ty]),
        // Force box: (ctx, tag, payload) -> {i64, i64}
        ("rt_force_boxed", tv_ty, vec![ptr_ty, i64_ty, i64_ty]),
        // String: (ctx, ptr, len) -> *mut Value (returned as ptr)
        ("rt_make_string", ptr_ty, vec![ptr_ty, ptr_ty, i64_ty]),
        // Base function allocation: (ctx, idx) -> *mut Value
        ("rt_make_base_function", ptr_ty, vec![ptr_ty, i64_ty]),
        // Base function call: (ctx, idx, args_ptr, nargs, sl, sc, el, ec) -> ptr
        (
            "rt_call_base_function_tagged",
            ptr_ty,
            vec![
                ptr_ty, i64_ty, ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty, i64_ty,
            ],
        ),
        // Generic value call: (ctx, callee, args_ptr, nargs, sl, sc, el, ec) -> ptr
        (
            "rt_call_value",
            ptr_ty,
            vec![
                ptr_ty, ptr_ty, ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty, i64_ty,
            ],
        ),
        // Global access
        ("rt_get_global", ptr_ty, vec![ptr_ty, i64_ty]),
        ("rt_set_global", void_ty, vec![ptr_ty, i64_ty, ptr_ty]),
        // Closure creation: (ctx, fn_index, captures_ptr, ncaptures) -> ptr
        (
            "rt_make_jit_closure",
            ptr_ty,
            vec![ptr_ty, i64_ty, ptr_ty, i64_ty],
        ),
        // Array/tuple
        ("rt_make_array", ptr_ty, vec![ptr_ty, ptr_ty, i64_ty]),
        ("rt_make_tuple", ptr_ty, vec![ptr_ty, ptr_ty, i64_ty]),
        // String operations
        ("rt_to_string", ptr_ty, vec![ptr_ty, ptr_ty]),
        ("rt_string_concat", ptr_ty, vec![ptr_ty, ptr_ty, ptr_ty]),
        // Hash map
        ("rt_make_hash", ptr_ty, vec![ptr_ty, ptr_ty, i64_ty]),
        // Indexing: (ctx, collection, key) -> ptr
        ("rt_index", ptr_ty, vec![ptr_ty, ptr_ty, ptr_ty]),
        // Cons list
        ("rt_make_cons", ptr_ty, vec![ptr_ty, ptr_ty, ptr_ty]),
        ("rt_make_empty_list", ptr_ty, vec![ptr_ty]),
        ("rt_is_cons", i64_ty, vec![ptr_ty, ptr_ty]),
        ("rt_cons_head", ptr_ty, vec![ptr_ty, ptr_ty]),
        ("rt_cons_tail", ptr_ty, vec![ptr_ty, ptr_ty]),
        // Sum types: Some/Left/Right
        ("rt_make_some", ptr_ty, vec![ptr_ty, ptr_ty]),
        ("rt_make_left", ptr_ty, vec![ptr_ty, ptr_ty]),
        ("rt_make_right", ptr_ty, vec![ptr_ty, ptr_ty]),
        // Pattern matching tests: (ctx, value) -> i64 (0 or 1)
        ("rt_is_some", i64_ty, vec![ptr_ty, ptr_ty]),
        ("rt_is_left", i64_ty, vec![ptr_ty, ptr_ty]),
        ("rt_is_right", i64_ty, vec![ptr_ty, ptr_ty]),
        ("rt_is_none", i64_ty, vec![ptr_ty, ptr_ty]),
        ("rt_is_empty_list", i64_ty, vec![ptr_ty, ptr_ty]),
        // Unwrap: (ctx, value) -> ptr
        ("rt_unwrap_some", ptr_ty, vec![ptr_ty, ptr_ty]),
        ("rt_unwrap_left", ptr_ty, vec![ptr_ty, ptr_ty]),
        ("rt_unwrap_right", ptr_ty, vec![ptr_ty, ptr_ty]),
        // Value equality: (ctx, a, b) -> i64
        ("rt_values_equal", i64_ty, vec![ptr_ty, ptr_ty, ptr_ty]),
        // Tuple ops
        ("rt_is_tuple", i64_ty, vec![ptr_ty, ptr_ty]),
        ("rt_tuple_len_eq", i64_ty, vec![ptr_ty, ptr_ty, i64_ty]),
        ("rt_tuple_get", ptr_ty, vec![ptr_ty, ptr_ty, i64_ty]),
        // ADT construction: (ctx, name_ptr, name_len, fields_ptr, nfields) -> ptr
        (
            "rt_make_adt",
            ptr_ty,
            vec![ptr_ty, ptr_ty, i64_ty, ptr_ty, i64_ty],
        ),
        ("rt_intern_unit_adt", ptr_ty, vec![ptr_ty, ptr_ty, i64_ty]),
        // ADT pattern matching
        (
            "rt_is_adt_constructor",
            i64_ty,
            vec![ptr_ty, ptr_ty, ptr_ty, i64_ty],
        ),
        ("rt_adt_field", ptr_ty, vec![ptr_ty, ptr_ty, i64_ty]),
        ("rt_adt_field_or_none", ptr_ty, vec![ptr_ty, ptr_ty, i64_ty]),
        // Primop call: (ctx, primop_id, args_ptr, nargs, sl, sc, el, ec) -> ptr
        (
            "rt_call_primop",
            ptr_ty,
            vec![
                ptr_ty, i64_ty, ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty, i64_ty,
            ],
        ),
        // JIT function call with contract checking: (ctx, fn_idx, args_ptr, nargs, sl, sc, el, ec) -> ptr
        (
            "rt_call_jit_function",
            ptr_ty,
            vec![
                ptr_ty, i64_ty, ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty, i64_ty,
            ],
        ),
        // Value unboxing: convert *mut Value back to proper {tag, payload}
        ("rt_unbox_to_tagged", tv_ty, vec![ptr_ty, ptr_ty]),
        // Effect handlers
        (
            "rt_push_handler",
            void_ty,
            vec![ptr_ty, i64_ty, ptr_ty, ptr_ty, i64_ty],
        ),
        ("rt_pop_handler", void_ty, vec![ptr_ty]),
        // rt_perform(ctx, effect_id, op_id, args_ptr, nargs, effect_name_ptr, effect_name_len, op_name_ptr, op_name_len, line, column) -> ptr
        (
            "rt_perform",
            ptr_ty,
            vec![
                ptr_ty, i64_ty, i64_ty, ptr_ty, i64_ty, ptr_ty, i64_ty, ptr_ty, i64_ty, i64_ty,
                i64_ty,
            ],
        ),
        // Error checking: (ctx) -> i64 (0 or 1)
        ("rt_has_error", i64_ty, vec![ptr_ty]),
        // Tail call thunk: (ctx, fn_index, args_ptr, nargs) -> {i64, i64} (JIT_TAG_THUNK marker)
        ("rt_set_thunk", tv_ty, vec![ptr_ty, i64_ty, ptr_ty, i64_ty]),
        // Error rendering: (ctx, start_line, start_col, end_line, end_col) -> void
        (
            "rt_render_error_with_span",
            void_ty,
            vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty],
        ),
    ];

    for (name, ret_ty, param_tys) in helpers {
        let fn_type = function_type(ret_ty, &param_tys, false);
        let func = ctx.module.add_function(name, fn_type);
        ctx.helpers.insert(name, (func, fn_type));
    }
}

/// Bind each declared helper to its actual function pointer.
pub(super) fn resolve_all_runtime_symbols(ctx: &LlvmCompilerContext) {
    let symbols = rt_symbols();
    let mut resolved = 0;
    for (name, ptr) in &symbols {
        ctx.resolve_symbol(name, *ptr);
        if ctx.helpers.contains_key(name) {
            resolved += 1;
        }
    }
    if std::env::var("FLUX_LLVM_DUMP").is_ok() {
        eprintln!(
            "[llvm] resolved {}/{} runtime symbols ({} declared)",
            resolved,
            symbols.len(),
            ctx.helpers.len()
        );
        let _ = std::io::Write::flush(&mut std::io::stderr());
    }
}

// ── User function declaration ────────────────────────────────────────────────

pub(super) fn declare_user_functions(
    ctx: &mut LlvmCompilerContext,
    program: &IrProgram,
    interner: &Interner,
) {
    let fn_type = ctx.user_function_type();
    for (idx, function) in program.functions.iter().enumerate() {
        let name = match function.name {
            Some(id) => format!("flux_{}", interner.resolve(id)),
            None => format!("flux_anon_{}", idx),
        };
        let func = ctx.module.add_function(&name, fn_type);
        ctx.functions.insert(idx, (func, fn_type));
    }
}
