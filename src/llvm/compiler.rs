//! Translates `IrProgram` (CFG IR) into LLVM IR and creates a JIT-compiled
//! entry point.
//!
//! Phase 1 covers the minimal subset needed for fibonacci:
//! - Integer/boolean constants, variables, binary arithmetic/comparison
//! - Direct and named function calls (via runtime helpers)
//! - Jump, Branch, Return terminators

use std::collections::HashMap;

use llvm_sys::prelude::*;
use llvm_sys::LLVMIntPredicate;

use crate::cfg::{
    BlockId, IrBinaryOp, IrBlock, IrCallTarget, IrConst, IrExpr, IrFunction, IrInstr,
    IrListTest, IrProgram, IrTagTest, IrTerminator, IrVar,
};
use crate::jit::context::{JIT_TAG_BOOL, JIT_TAG_INT, JIT_TAG_PTR};
use crate::jit::runtime_helpers::rt_symbols;
use crate::syntax::interner::Interner;

use super::context::LlvmCompilerContext;
use super::wrapper::{self, function_type};

/// Compile an IR program into LLVM IR inside the given context.
///
/// After this call, `ctx.finalize()` must be called to create the execution
/// engine, followed by `resolve_all_runtime_symbols()` to bind the `rt_*`
/// helpers.
pub fn compile_program(
    ctx: &mut LlvmCompilerContext,
    program: &IrProgram,
    interner: &Interner,
) -> Result<(), String> {
    // 1. Declare runtime helpers as external functions
    declare_runtime_helpers(ctx);

    // 2. Collect ADT constructor arities from top-level data declarations
    let mut adt_constructors: HashMap<crate::syntax::Identifier, usize> = HashMap::new();
    for item in program.top_level_items.iter() {
        if let crate::cfg::IrTopLevelItem::Data { variants, .. } = item {
            for variant in variants {
                adt_constructors.insert(variant.name, variant.fields.len());
            }
        }
    }

    // 3. Forward-declare all user functions
    declare_user_functions(ctx, program, interner);

    // 3. Compile each function body
    for (idx, function) in program.functions.iter().enumerate() {
        if std::env::var("FLUX_LLVM_DUMP").is_ok() {
            eprintln!("[llvm] compiling function {} ({}) params={} captures={} blocks={}",
                idx,
                function.name.map(|n| interner.resolve(n).to_string()).unwrap_or_else(|| "anon".to_string()),
                function.params.len(),
                function.captures.len(),
                function.blocks.len());
        }
        compile_function(ctx, program, function, idx, interner, &adt_constructors)?;
        if std::env::var("FLUX_LLVM_DUMP").is_ok() {
            eprintln!("[llvm] function {} compiled OK", idx);
        }
    }

    // 4. Create entry wrapper: __flux_entry(ctx) → {i64, i64}
    compile_entry_wrapper(ctx, program, interner)?;
    if std::env::var("FLUX_LLVM_DUMP").is_ok() {
        eprintln!("[llvm] entry wrapper compiled OK");
    }

    // 5. Verify
    if std::env::var("FLUX_LLVM_DUMP").is_ok() {
        eprintln!("=== LLVM IR ===\n{}\n===============", ctx.module.dump_to_string());
    }
    if let Err(err) = ctx.module.verify() {
        if std::env::var("FLUX_LLVM_DUMP").is_ok() {
            eprintln!("[llvm] verification failed: {}", err);
        }
        return Err(err);
    }

    // 7. Finalize (create execution engine)
    ctx.finalize()?;

    // 8. Resolve runtime symbols
    resolve_all_runtime_symbols(ctx);

    Ok(())
}

// ── Runtime helper declaration ───────────────────────────────────────────────

/// Declare all `rt_*` functions as external symbols in the LLVM module.
fn declare_runtime_helpers(ctx: &mut LlvmCompilerContext) {
    let i64_ty = ctx.i64_type;
    let ptr_ty = ctx.ptr_type;
    let tv_ty = ctx.tagged_value_type;
    let void_ty = ctx.void_type;

    // Helper signature descriptors: (name, return_type, param_types)
    let helpers: Vec<(&str, LLVMTypeRef, Vec<LLVMTypeRef>)> = vec![
        // Arithmetic: (ctx, tv, tv) -> tv  — but the C ABI flattens JitTaggedValue
        // rt_add(ctx: ptr, a_tag: i64, a_payload: i64, b_tag: i64, b_payload: i64) -> {i64, i64}
        ("rt_add", tv_ty, vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty]),
        ("rt_sub", tv_ty, vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty]),
        ("rt_mul", tv_ty, vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty]),
        ("rt_div", tv_ty, vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty]),
        ("rt_mod", tv_ty, vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty]),
        // Comparison: same signature
        ("rt_equal", tv_ty, vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty]),
        ("rt_not_equal", tv_ty, vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty]),
        ("rt_greater_than", tv_ty, vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty]),
        ("rt_less_than_or_equal", tv_ty, vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty]),
        ("rt_greater_than_or_equal", tv_ty, vec![ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty]),
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
        ("rt_call_base_function_tagged", ptr_ty, vec![
            ptr_ty, i64_ty, ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty, i64_ty,
        ]),
        // Generic value call: (ctx, callee, args_ptr, nargs, sl, sc, el, ec) -> ptr
        ("rt_call_value", ptr_ty, vec![
            ptr_ty, ptr_ty, ptr_ty, i64_ty, i64_ty, i64_ty, i64_ty, i64_ty,
        ]),
        // Global access
        ("rt_get_global", ptr_ty, vec![ptr_ty, i64_ty]),
        ("rt_set_global", void_ty, vec![ptr_ty, i64_ty, ptr_ty]),
        // Closure creation: (ctx, fn_index, captures_ptr, ncaptures) -> ptr
        ("rt_make_jit_closure", ptr_ty, vec![ptr_ty, i64_ty, ptr_ty, i64_ty]),
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
        ("rt_make_adt", ptr_ty, vec![ptr_ty, ptr_ty, i64_ty, ptr_ty, i64_ty]),
        ("rt_intern_unit_adt", ptr_ty, vec![ptr_ty, ptr_ty, i64_ty]),
        // ADT pattern matching
        ("rt_is_adt_constructor", i64_ty, vec![ptr_ty, ptr_ty, ptr_ty, i64_ty]),
        ("rt_adt_field", ptr_ty, vec![ptr_ty, ptr_ty, i64_ty]),
        ("rt_adt_field_or_none", ptr_ty, vec![ptr_ty, ptr_ty, i64_ty]),
        // Effect handlers
        ("rt_push_handler", void_ty, vec![ptr_ty, i64_ty, ptr_ty, ptr_ty, i64_ty]),
        ("rt_pop_handler", void_ty, vec![ptr_ty]),
        // rt_perform(ctx, effect_id, op_id, args_ptr, nargs, effect_name_ptr, effect_name_len, op_name_ptr, op_name_len, line, column) -> ptr
        ("rt_perform", ptr_ty, vec![ptr_ty, i64_ty, i64_ty, ptr_ty, i64_ty, ptr_ty, i64_ty, ptr_ty, i64_ty, i64_ty, i64_ty]),
    ];

    for (name, ret_ty, param_tys) in helpers {
        let fn_type = function_type(ret_ty, &param_tys, false);
        let func = ctx.module.add_function(name, fn_type);
        ctx.helpers.insert(name, (func, fn_type));
    }
}

/// Bind each declared helper to its actual function pointer.
fn resolve_all_runtime_symbols(ctx: &LlvmCompilerContext) {
    let symbols = rt_symbols();
    let mut resolved = 0;
    for (name, ptr) in &symbols {
        ctx.resolve_symbol(name, *ptr);
        if ctx.helpers.contains_key(name) {
            resolved += 1;
        }
    }
    if std::env::var("FLUX_LLVM_DUMP").is_ok() {
        eprintln!("[llvm] resolved {}/{} runtime symbols ({} declared)", resolved, symbols.len(), ctx.helpers.len());
        let _ = std::io::Write::flush(&mut std::io::stderr());
    }
}

// ── User function declaration ────────────────────────────────────────────────

fn declare_user_functions(
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

// ── Function compilation ─────────────────────────────────────────────────────

fn compile_function(
    ctx: &LlvmCompilerContext,
    program: &IrProgram,
    function: &IrFunction,
    fn_index: usize,
    interner: &Interner,
    adt_constructors: &HashMap<crate::syntax::Identifier, usize>,
) -> Result<(), String> {
    // Build global binding map for the entry function.
    // Assignments to global-bound vars emit rt_set_global after the value is computed.
    let global_binding_indices: HashMap<IrVar, usize> = if function.id == program.entry {
        let map: HashMap<IrVar, usize> = program
            .global_bindings
            .iter()
            .filter_map(|binding| {
                program
                    .globals
                    .iter()
                    .position(|g| *g == binding.name)
                    .map(|idx| (binding.var, idx))
            })
            .collect();
        if std::env::var("FLUX_LLVM_DUMP").is_ok() {
            eprintln!("[llvm] entry fn: {} global bindings, {} globals", program.global_bindings.len(), program.globals.len());
            for (var, idx) in &map {
                eprintln!("[llvm]   v{} -> global[{}]", var.0, idx);
            }
            let _ = std::io::Write::flush(&mut std::io::stderr());
        }
        map
    } else {
        HashMap::new()
    };
    let (func_ref, _) = ctx.functions[&fn_index];

    // Get function parameters (Array ABI)
    let ctx_val = wrapper::get_param(func_ref, 0); // *mut JitContext
    let args_ptr = wrapper::get_param(func_ref, 1); // *const JitTaggedValue
    let _nargs = wrapper::get_param(func_ref, 2); // i64
    let captures_ptr = wrapper::get_param(func_ref, 3); // *const JitTaggedValue
    let _ncaptures = wrapper::get_param(func_ref, 4); // i64

    // Create LLVM basic blocks for each IR block
    let mut block_map: HashMap<BlockId, LLVMBasicBlockRef> = HashMap::new();
    for block in &function.blocks {
        let llvm_block = ctx
            .llvm_ctx
            .append_basic_block(func_ref, &format!("b{}", block.id.0));
        block_map.insert(block.id, llvm_block);
    }

    // Variable environment: IrVar → LLVMValueRef (each is a {i64, i64} tagged value)
    let mut env: HashMap<IrVar, LLVMValueRef> = HashMap::new();

    // Pre-pass: create PHI instructions for block parameters.
    // Each block param becomes a PHI node in the target LLVM block.
    // phi_map: (BlockId, param_index) → PHI LLVMValueRef
    let mut phi_map: HashMap<(BlockId, usize), LLVMValueRef> = HashMap::new();
    for block in &function.blocks {
        if !block.params.is_empty() {
            let llvm_block = block_map[&block.id];
            ctx.builder.position_at_end(llvm_block);
            for (param_idx, _param) in block.params.iter().enumerate() {
                let phi = ctx
                    .builder
                    .build_phi(ctx.tagged_value_type, &format!("phi_b{}_p{}", block.id.0, param_idx));
                phi_map.insert((block.id, param_idx), phi);
            }
        }
    }

    // Load function parameters into env.
    // In the Array ABI, captures occupy the first N params, then explicit args follow.
    let capture_count = function.captures.len();
    let entry_block = block_map[&function.entry];
    ctx.builder.position_at_end(entry_block);

    // Load captures from captures_ptr (first capture_count params)
    for (i, param) in function.params.iter().take(capture_count).enumerate() {
        let tagged = load_tagged_from_ptr(ctx, captures_ptr, i, "cap");
        env.insert(param.var, tagged);
    }

    // Load explicit args from args_ptr (remaining params)
    for (i, param) in function.params.iter().skip(capture_count).enumerate() {
        let tagged = load_tagged_from_ptr(ctx, args_ptr, i, "arg");
        env.insert(param.var, tagged);
    }

    // Pre-compute handler pop counts: for each HandleScope, find the
    // continuation block (where body_result becomes a param) and increment
    // its pop count. At block entry, emit that many rt_pop_handler calls.
    let mut handler_pop_counts: HashMap<BlockId, usize> = HashMap::new();
    for block in &function.blocks {
        for instr in &block.instrs {
            if let IrInstr::HandleScope { body_result, .. } = instr {
                // Find the continuation block: the block whose params include body_result
                if let Some(cont_block) = function.blocks.iter().find(|b| {
                    b.params.iter().any(|p| p.var == *body_result)
                }) {
                    *handler_pop_counts.entry(cont_block.id).or_insert(0) += 1;
                }
            }
        }
    }

    // Compile each block
    for block in &function.blocks {
        if std::env::var("FLUX_LLVM_DUMP").is_ok() {
            eprintln!("[llvm]  block {} (entry={}) instrs={}", block.id.0, block.id == function.entry, block.instrs.len());
            let _ = std::io::Write::flush(&mut std::io::stderr());
        }
        let llvm_block = block_map[&block.id];
        // Only position if this isn't the entry block (already positioned)
        if block.id != function.entry {
            ctx.builder.position_at_end(llvm_block);
        }

        // Emit rt_pop_handler calls for continuation blocks
        if let Some(&pop_count) = handler_pop_counts.get(&block.id) {
            let (pop_handler, pop_handler_ty) = get_helper(ctx, "rt_pop_handler")
                .map_err(|e| format!("handler pop: {}", e))?;
            for _ in 0..pop_count {
                ctx.builder.build_call(pop_handler_ty, pop_handler, &mut [ctx_val], "");
            }
        }

        // Load block params from PHI nodes into env
        for (param_idx, param) in block.params.iter().enumerate() {
            if let Some(&phi) = phi_map.get(&(block.id, param_idx)) {
                env.insert(param.var, phi);
            }
        }

        compile_block(ctx, program, function, block, &block_map, &mut env, ctx_val, interner, &phi_map, &global_binding_indices, adt_constructors)?;
    }

    Ok(())
}

fn compile_block(
    ctx: &LlvmCompilerContext,
    program: &IrProgram,
    function: &IrFunction,
    block: &IrBlock,
    block_map: &HashMap<BlockId, LLVMBasicBlockRef>,
    env: &mut HashMap<IrVar, LLVMValueRef>,
    ctx_val: LLVMValueRef,
    interner: &Interner,
    phi_map: &HashMap<(BlockId, usize), LLVMValueRef>,
    global_binding_indices: &HashMap<IrVar, usize>,
    adt_constructors: &HashMap<crate::syntax::Identifier, usize>,
) -> Result<(), String> {
    // Compile instructions
    for (instr_idx, instr) in block.instrs.iter().enumerate() {
        if std::env::var("FLUX_LLVM_DUMP").is_ok() {
            match instr {
                IrInstr::Assign { dest, expr, .. } => {
                    eprintln!("[llvm]   instr {}: Assign v{} = {:?}", instr_idx, dest.0, std::mem::discriminant(expr));
                    let _ = std::io::Write::flush(&mut std::io::stderr());
                }
                IrInstr::Call { dest, target, args, .. } => {
                    eprintln!("[llvm]   instr {}: Call v{} target={:?} nargs={}", instr_idx, dest.0, target, args.len());
                    let _ = std::io::Write::flush(&mut std::io::stderr());
                }
                IrInstr::HandleScope { .. } => eprintln!("[llvm]   instr {}: HandleScope", instr_idx),
            }
        }
        match instr {
            IrInstr::Assign { dest, expr, .. } => {
                let value = compile_expr(ctx, program, expr, env, ctx_val, interner, adt_constructors)
                    .map_err(|e| format!("in assign v{}: {}", dest.0, e))?;
                env.insert(*dest, value);
                emit_set_global_if_bound(ctx, *dest, value, ctx_val, global_binding_indices)?;
            }
            IrInstr::Call {
                dest,
                target,
                args,
                ..
            } => {
                let value =
                    compile_call(ctx, program, function, target, args, env, ctx_val, interner, adt_constructors)?;
                env.insert(*dest, value);
                emit_set_global_if_bound(ctx, *dest, value, ctx_val, global_binding_indices)?;
            }
            IrInstr::HandleScope {
                effect,
                arms,
                body_entry: _,
                body_result: _,
                dest: _,
                ..
            } => {
                // Build operation symbol ID array on the stack
                let narms = arms.len();
                if narms == 0 {
                    // Empty handler — just push an empty handler frame
                    let (push_handler, push_handler_ty) = get_helper(ctx, "rt_push_handler")?;
                    let effect_val = wrapper::const_i64(ctx.i64_type, effect.as_u32() as i64);
                    let null_ptr = wrapper::const_null(ctx.ptr_type);
                    let zero = wrapper::const_i64(ctx.i64_type, 0);
                    ctx.builder.build_call(
                        push_handler_ty,
                        push_handler,
                        &mut [ctx_val, effect_val, null_ptr, null_ptr, zero],
                        "",
                    );
                } else {
                    // Build ops array: [op0_id, op1_id, ...]
                    let ops_array_ty = unsafe {
                        llvm_sys::core::LLVMArrayType2(ctx.i64_type, narms as u64)
                    };
                    let ops_alloca = ctx.builder.build_alloca(ops_array_ty, "handler_ops");
                    for (i, arm) in arms.iter().enumerate() {
                        let op_id = wrapper::const_i64(ctx.i64_type, arm.operation_name.as_u32() as i64);
                        let slot = unsafe {
                            llvm_sys::core::LLVMBuildGEP2(
                                ctx.builder.raw_ptr(),
                                ops_array_ty,
                                ops_alloca,
                                [wrapper::const_i64(ctx.i64_type, 0), wrapper::const_i64(ctx.i64_type, i as i64)].as_mut_ptr(),
                                2,
                                c"op_slot".as_ptr(),
                            )
                        };
                        ctx.builder.build_store(op_id, slot);
                    }

                    // Build closures array: [closure0_ptr, closure1_ptr, ...]
                    let closures_array_ty = unsafe {
                        llvm_sys::core::LLVMArrayType2(ctx.ptr_type, narms as u64)
                    };
                    let closures_alloca = ctx.builder.build_alloca(closures_array_ty, "handler_closures");
                    let (make_closure, make_closure_ty) = get_helper(ctx, "rt_make_jit_closure")?;

                    for (i, arm) in arms.iter().enumerate() {
                        // Find the function index for this arm's function
                        let arm_fn_index = program
                            .functions
                            .iter()
                            .position(|f| f.id == arm.function_id)
                            .ok_or_else(|| format!("missing handler arm function {:?}", arm.function_id))?;

                        // Build captures array for this arm
                        let closure_ptr = if arm.capture_vars.is_empty() {
                            let fn_idx_val = wrapper::const_i64(ctx.i64_type, arm_fn_index as i64);
                            let null_ptr = wrapper::const_null(ctx.ptr_type);
                            let zero = wrapper::const_i64(ctx.i64_type, 0);
                            ctx.builder.build_call(
                                make_closure_ty,
                                make_closure,
                                &mut [ctx_val, fn_idx_val, null_ptr, zero],
                                "arm_closure",
                            )
                        } else {
                            // Build captures as tagged value array
                            let cap_args: Vec<IrVar> = arm.capture_vars.clone();
                            let captures_buf = build_tagged_args_array(ctx, &cap_args, env)?;
                            let ncaptures = wrapper::const_i64(ctx.i64_type, arm.capture_vars.len() as i64);
                            let fn_idx_val = wrapper::const_i64(ctx.i64_type, arm_fn_index as i64);
                            ctx.builder.build_call(
                                make_closure_ty,
                                make_closure,
                                &mut [ctx_val, fn_idx_val, captures_buf, ncaptures],
                                "arm_closure",
                            )
                        };

                        let slot = unsafe {
                            llvm_sys::core::LLVMBuildGEP2(
                                ctx.builder.raw_ptr(),
                                closures_array_ty,
                                closures_alloca,
                                [wrapper::const_i64(ctx.i64_type, 0), wrapper::const_i64(ctx.i64_type, i as i64)].as_mut_ptr(),
                                2,
                                c"closure_slot".as_ptr(),
                            )
                        };
                        ctx.builder.build_store(closure_ptr, slot);
                    }

                    // Call rt_push_handler(ctx, effect_id, ops_ptr, closures_ptr, narms)
                    let (push_handler, push_handler_ty) = get_helper(ctx, "rt_push_handler")?;
                    let effect_val = wrapper::const_i64(ctx.i64_type, effect.as_u32() as i64);
                    let narms_val = wrapper::const_i64(ctx.i64_type, narms as i64);
                    ctx.builder.build_call(
                        push_handler_ty,
                        push_handler,
                        &mut [ctx_val, effect_val, ops_alloca, closures_alloca, narms_val],
                        "",
                    );
                }
                // Control flow continues into the body blocks (handled by the
                // normal terminator — typically Jump to body_entry).
            }
        }
    }

    // Compile terminator
    if std::env::var("FLUX_LLVM_DUMP").is_ok() {
        eprintln!("[llvm]    terminator: {:?}", std::mem::discriminant(&block.terminator));
        let _ = std::io::Write::flush(&mut std::io::stderr());
    }
    match &block.terminator {
        IrTerminator::Return(var, _) => {
            if std::env::var("FLUX_LLVM_DUMP").is_ok() {
                eprintln!("[llvm]    return v{}, in_env={}", var.0, env.contains_key(var));
                let _ = std::io::Write::flush(&mut std::io::stderr());
            }
            let value = get_var(env, *var)?;
            if std::env::var("FLUX_LLVM_DUMP").is_ok() {
                eprintln!("[llvm]    return value found, building ret");
                let _ = std::io::Write::flush(&mut std::io::stderr());
            }
            ctx.builder.build_ret(value);
        }
        IrTerminator::Jump(target, args, _) => {
            // Add incoming edges to target block's PHI nodes
            let current_llvm_block = block_map[&block.id];
            for (param_idx, arg) in args.iter().enumerate() {
                let val = get_var(env, *arg)?;
                if let Some(&phi) = phi_map.get(&(*target, param_idx)) {
                    wrapper::add_incoming(phi, &[val], &[current_llvm_block]);
                }
            }
            let llvm_target = block_map[target];
            ctx.builder.build_br(llvm_target);
        }
        IrTerminator::Branch {
            cond,
            then_block,
            else_block,
            ..
        } => {
            let cond_val = get_var(env, *cond)?;
            // Extract the payload (boolean value) and compare to 0
            let payload = ctx
                .builder
                .build_extract_value(cond_val, 1, "cond_payload");
            let zero = wrapper::const_i64(ctx.i64_type, 0);
            let cond_bool = ctx.builder.build_icmp(
                LLVMIntPredicate::LLVMIntNE,
                payload,
                zero,
                "cond_bool",
            );
            let llvm_then = block_map[then_block];
            let llvm_else = block_map[else_block];
            ctx.builder.build_cond_br(cond_bool, llvm_then, llvm_else);
        }
        IrTerminator::TailCall {
            callee, args, ..
        } => {
            // Phase 1: compile as a regular call + return
            let value = compile_call(ctx, program, function, callee, args, env, ctx_val, interner, adt_constructors)?;
            ctx.builder.build_ret(value);
        }
        IrTerminator::Unreachable(_) => {
            // Emit unreachable
            unsafe {
                llvm_sys::core::LLVMBuildUnreachable(ctx.builder.raw_ptr());
            }
        }
    }

    Ok(())
}

// ── Expression compilation ───────────────────────────────────────────────────

fn compile_expr(
    ctx: &LlvmCompilerContext,
    program: &IrProgram,
    expr: &IrExpr,
    env: &mut HashMap<IrVar, LLVMValueRef>,
    ctx_val: LLVMValueRef,
    interner: &Interner,
    adt_constructors: &HashMap<crate::syntax::Identifier, usize>,
) -> Result<LLVMValueRef, String> {
    match expr {
        IrExpr::Const(IrConst::Int(n)) => Ok(build_int_tagged(ctx, *n)),
        IrExpr::Const(IrConst::Bool(b)) => Ok(build_bool_tagged(ctx, *b)),
        IrExpr::Const(IrConst::Unit) => {
            // Unit is represented as None
            let (func, fn_ty) = get_helper(ctx, "rt_make_none")?;
            let result = ctx
                .builder
                .build_call(fn_ty, func, &mut [ctx_val], "unit");
            Ok(result)
        }
        IrExpr::Const(IrConst::Float(f)) => {
            let (func, fn_ty) = get_helper(ctx, "rt_make_float")?;
            let bits = wrapper::const_i64(ctx.i64_type, f.to_bits() as i64);
            let result = ctx
                .builder
                .build_call(fn_ty, func, &mut [ctx_val, bits], "float");
            Ok(result)
        }
        IrExpr::Const(IrConst::String(s)) => {
            // Embed string bytes as a global constant, then call rt_make_string
            let bytes = s.as_bytes();
            let global_name = format!(".str.{}", s.len());
            let global = wrapper::create_global_string(
                &ctx.module,
                &ctx.llvm_ctx,
                &global_name,
                bytes,
            );
            let (make_string, make_string_ty) = get_helper(ctx, "rt_make_string")?;
            let len_val = wrapper::const_i64(ctx.i64_type, bytes.len() as i64);
            let ptr_result = ctx.builder.build_call(
                make_string_ty,
                make_string,
                &mut [ctx_val, global, len_val],
                "str",
            );
            Ok(build_ptr_tagged(ctx, ptr_result))
        }
        IrExpr::Var(var) => get_var(env, *var),
        IrExpr::Binary(op, lhs, rhs) => {
            let lhs_val = get_var(env, *lhs)?;
            let rhs_val = get_var(env, *rhs)?;
            compile_binary(ctx, *op, lhs_val, rhs_val, ctx_val)
        }
        IrExpr::None => {
            let (func, fn_ty) = get_helper(ctx, "rt_make_none")?;
            let result = ctx
                .builder
                .build_call(fn_ty, func, &mut [ctx_val], "none");
            Ok(result)
        }
        IrExpr::LoadName(name) => {
            let name_str = interner.resolve(*name);

            // 1. Check if it's a user function → create a JitClosure
            if let Some(fn_index) = program.functions.iter().position(|f| f.name == Some(*name)) {
                let (make_closure, make_closure_ty) = get_helper(ctx, "rt_make_jit_closure")?;
                let fn_idx_val = wrapper::const_i64(ctx.i64_type, fn_index as i64);
                let null_ptr = wrapper::const_null(ctx.ptr_type);
                let zero = wrapper::const_i64(ctx.i64_type, 0);
                let ptr_result = ctx.builder.build_call(
                    make_closure_ty,
                    make_closure,
                    &mut [ctx_val, fn_idx_val, null_ptr, zero],
                    "user_fn",
                );
                return Ok(build_ptr_tagged(ctx, ptr_result));
            }

            // 2. Check if it's a base function
            if let Some(idx) = crate::runtime::base::get_base_function_index(name_str) {
                let (make_base_fn, make_base_fn_ty) = get_helper(ctx, "rt_make_base_function")?;
                let idx_val = wrapper::const_i64(ctx.i64_type, idx as i64);
                let ptr_result = ctx.builder.build_call(
                    make_base_fn_ty,
                    make_base_fn,
                    &mut [ctx_val, idx_val],
                    "base_fn",
                );
                return Ok(build_ptr_tagged(ctx, ptr_result));
            }

            // 3. Check if it's a unit ADT constructor (0-arity)
            if let Some(&arity) = adt_constructors.get(name) {
                if arity == 0 {
                    let (intern_adt, intern_adt_ty) = get_helper(ctx, "rt_intern_unit_adt")?;
                    let name_bytes = name_str.as_bytes();
                    let global = wrapper::create_global_string(
                        &ctx.module, &ctx.llvm_ctx,
                        &format!(".adt.{}", name_str), name_bytes,
                    );
                    let len = wrapper::const_i64(ctx.i64_type, name_bytes.len() as i64);
                    let ptr_result = ctx.builder.build_call(
                        intern_adt_ty, intern_adt,
                        &mut [ctx_val, global, len], "unit_adt",
                    );
                    return Ok(build_ptr_tagged(ctx, ptr_result));
                }
                // Non-zero arity constructors are handled via Named calls / MakeAdt
            }

            // 4. Check if it's a global variable
            if let Some(idx) = program.globals.iter().position(|g| *g == *name) {
                let (func, fn_ty) = get_helper(ctx, "rt_get_global")?;
                let idx_val = wrapper::const_i64(ctx.i64_type, idx as i64);
                let ptr_result = ctx
                    .builder
                    .build_call(fn_ty, func, &mut [ctx_val, idx_val], "global_ptr");
                return Ok(build_ptr_tagged(ctx, ptr_result));
            }

            Err(format!("LLVM backend: unresolved name '{}'", name_str))
        }
        IrExpr::MakeClosure(fn_id, captures) => {
            let fn_index = program
                .functions
                .iter()
                .position(|f| f.id == *fn_id)
                .ok_or_else(|| format!("missing function {:?}", fn_id))?;

            let (func, fn_ty) = get_helper(ctx, "rt_make_jit_closure")?;
            let fn_idx_val = wrapper::const_i64(ctx.i64_type, fn_index as i64);

            if captures.is_empty() {
                let null_ptr = wrapper::const_null(ctx.ptr_type);
                let zero = wrapper::const_i64(ctx.i64_type, 0);
                let result = ctx.builder.build_call(
                    fn_ty,
                    func,
                    &mut [ctx_val, fn_idx_val, null_ptr, zero],
                    "closure",
                );
                Ok(build_ptr_tagged(ctx, result))
            } else {
                // Build captures as a tagged value array (consecutive {tag, payload} i64 pairs)
                let captures_buf = build_tagged_args_array(ctx, captures, env)?;
                let ncaptures = wrapper::const_i64(ctx.i64_type, captures.len() as i64);
                let result = ctx.builder.build_call(
                    fn_ty,
                    func,
                    &mut [ctx_val, fn_idx_val, captures_buf, ncaptures],
                    "closure",
                );
                Ok(build_ptr_tagged(ctx, result))
            }
        }
        IrExpr::Prefix { operator, right } => {
            let right_val = get_var(env, *right)?;
            let helper_name = match operator.as_str() {
                "-" => "rt_negate",
                "!" => "rt_not",
                _ => return Err(format!("LLVM backend: unsupported prefix operator '{}'", operator)),
            };
            let (func, fn_ty) = get_helper(ctx, helper_name)?;
            let tag = ctx.builder.build_extract_value(right_val, 0, "pfx_tag");
            let payload = ctx.builder.build_extract_value(right_val, 1, "pfx_payload");
            let result = ctx.builder.build_call(
                fn_ty,
                func,
                &mut [ctx_val, tag, payload],
                "prefix",
            );
            Ok(result)
        }
        IrExpr::InterpolatedString(parts) => {
            // Build each part as a string, then concatenate via rt_string_concat.
            // For now, convert each part to a boxed Value and concatenate.
            let (make_string, make_string_ty) = get_helper(ctx, "rt_make_string")?;
            let (force_boxed, force_boxed_ty) = get_helper(ctx, "rt_force_boxed")?;

            // Helper: rt_to_string and rt_string_concat
            let to_string = ctx.helpers.get("rt_to_string");
            let string_concat = ctx.helpers.get("rt_string_concat");

            if to_string.is_none() || string_concat.is_none() {
                return Err("LLVM backend: rt_to_string/rt_string_concat not declared".to_string());
            }
            let (to_string_fn, to_string_ty) = *to_string.unwrap();
            let (concat_fn, concat_ty) = *string_concat.unwrap();

            // Start with empty string
            let empty_global = wrapper::create_global_string(&ctx.module, &ctx.llvm_ctx, ".str.empty", b"");
            let zero_len = wrapper::const_i64(ctx.i64_type, 0);
            let mut accum = ctx.builder.build_call(
                make_string_ty,
                make_string,
                &mut [ctx_val, empty_global, zero_len],
                "interp_base",
            );

            for part in parts {
                let part_ptr = match part {
                    crate::cfg::IrStringPart::Literal(s) => {
                        let bytes = s.as_bytes();
                        let global = wrapper::create_global_string(
                            &ctx.module, &ctx.llvm_ctx,
                            &format!(".str.interp.{}", bytes.len()), bytes,
                        );
                        let len = wrapper::const_i64(ctx.i64_type, bytes.len() as i64);
                        ctx.builder.build_call(make_string_ty, make_string, &mut [ctx_val, global, len], "lit_part")
                    }
                    crate::cfg::IrStringPart::Interpolation(var) => {
                        let val = get_var(env, *var)?;
                        let tag = ctx.builder.build_extract_value(val, 0, "interp_tag");
                        let payload = ctx.builder.build_extract_value(val, 1, "interp_payload");
                        let boxed = ctx.builder.build_call(
                            force_boxed_ty, force_boxed,
                            &mut [ctx_val, tag, payload], "interp_boxed",
                        );
                        let ptr_int = ctx.builder.build_extract_value(boxed, 1, "interp_ptr_int");
                        let ptr = ctx.builder.build_int_to_ptr(ptr_int, ctx.ptr_type, "interp_ptr");
                        // Convert to string
                        ctx.builder.build_call(to_string_ty, to_string_fn, &mut [ctx_val, ptr], "interp_str")
                    }
                };
                // Concatenate
                accum = ctx.builder.build_call(concat_ty, concat_fn, &mut [ctx_val, accum, part_ptr], "interp_cat");
            }

            Ok(build_ptr_tagged(ctx, accum))
        }
        IrExpr::EmptyList => {
            let (func, fn_ty) = get_helper(ctx, "rt_make_empty_list")?;
            let result = ctx.builder.build_call(fn_ty, func, &mut [ctx_val], "empty_list");
            Ok(build_ptr_tagged(ctx, result))
        }
        IrExpr::MakeArray(vars) => {
            let (func, fn_ty) = get_helper(ctx, "rt_make_array")?;
            let args_buf = build_tagged_args_array(ctx, vars, env)?;
            let len = wrapper::const_i64(ctx.i64_type, vars.len() as i64);
            let result = ctx.builder.build_call(fn_ty, func, &mut [ctx_val, args_buf, len], "array");
            Ok(build_ptr_tagged(ctx, result))
        }
        IrExpr::MakeTuple(vars) => {
            let (func, fn_ty) = get_helper(ctx, "rt_make_tuple")?;
            let args_buf = build_tagged_args_array(ctx, vars, env)?;
            let len = wrapper::const_i64(ctx.i64_type, vars.len() as i64);
            let result = ctx.builder.build_call(fn_ty, func, &mut [ctx_val, args_buf, len], "tuple");
            Ok(build_ptr_tagged(ctx, result))
        }
        IrExpr::MakeHash(pairs) => {
            // rt_make_hash expects interleaved [k0, v0, k1, v1, ...] tagged values
            let (func, fn_ty) = get_helper(ctx, "rt_make_hash")?;
            let flat: Vec<IrVar> = pairs.iter().flat_map(|(k, v)| [*k, *v]).collect();
            let args_buf = build_tagged_args_array(ctx, &flat, env)?;
            let npairs = wrapper::const_i64(ctx.i64_type, pairs.len() as i64);
            let result = ctx.builder.build_call(fn_ty, func, &mut [ctx_val, args_buf, npairs], "hash");
            Ok(build_ptr_tagged(ctx, result))
        }
        IrExpr::MakeList(vars) => {
            // Build a cons list right-to-left: cons(last, cons(... cons(first, empty)))
            let (make_cons, make_cons_ty) = get_helper(ctx, "rt_make_cons")?;
            let (make_empty, make_empty_ty) = get_helper(ctx, "rt_make_empty_list")?;
            let (force_boxed, force_boxed_ty) = get_helper(ctx, "rt_force_boxed")?;
            let mut tail = ctx.builder.build_call(make_empty_ty, make_empty, &mut [ctx_val], "list_tail");
            for var in vars.iter().rev() {
                let val = get_var(env, *var)?;
                let tag = ctx.builder.build_extract_value(val, 0, "le_tag");
                let payload = ctx.builder.build_extract_value(val, 1, "le_payload");
                let boxed = ctx.builder.build_call(force_boxed_ty, force_boxed, &mut [ctx_val, tag, payload], "le_boxed");
                let ptr_int = ctx.builder.build_extract_value(boxed, 1, "le_ptr_int");
                let head = ctx.builder.build_int_to_ptr(ptr_int, ctx.ptr_type, "le_ptr");
                tail = ctx.builder.build_call(make_cons_ty, make_cons, &mut [ctx_val, head, tail], "list_cons");
            }
            Ok(build_ptr_tagged(ctx, tail))
        }
        IrExpr::Index { left, index } => {
            let (func, fn_ty) = get_helper(ctx, "rt_index")?;
            let left_ptr = force_box_to_ptr(ctx, env, *left, ctx_val)?;
            let idx_ptr = force_box_to_ptr(ctx, env, *index, ctx_val)?;
            let result = ctx.builder.build_call(fn_ty, func, &mut [ctx_val, left_ptr, idx_ptr], "index");
            Ok(build_ptr_tagged(ctx, result))
        }
        IrExpr::TupleFieldAccess { object, index } => {
            let (func, fn_ty) = get_helper(ctx, "rt_tuple_get")?;
            let obj_ptr = force_box_to_ptr(ctx, env, *object, ctx_val)?;
            let idx_val = wrapper::const_i64(ctx.i64_type, *index as i64);
            let result = ctx.builder.build_call(fn_ty, func, &mut [ctx_val, obj_ptr, idx_val], "tuple_get");
            Ok(build_ptr_tagged(ctx, result))
        }
        IrExpr::TupleArityTest { value, arity } => {
            let (func, fn_ty) = get_helper(ctx, "rt_tuple_len_eq")?;
            let val_ptr = force_box_to_ptr(ctx, env, *value, ctx_val)?;
            let arity_val = wrapper::const_i64(ctx.i64_type, *arity as i64);
            let result = ctx.builder.build_call(fn_ty, func, &mut [ctx_val, val_ptr, arity_val], "tuple_arity");
            Ok(build_bool_tagged(ctx, result))
        }
        IrExpr::MakeAdt(constructor, fields) => {
            let name_str = interner.resolve(*constructor);
            let name_bytes = name_str.as_bytes();
            if fields.is_empty() {
                // Unit ADT — use rt_intern_unit_adt for deduplication
                let (func, fn_ty) = get_helper(ctx, "rt_intern_unit_adt")?;
                let global = wrapper::create_global_string(&ctx.module, &ctx.llvm_ctx, &format!(".adt.{}", name_str), name_bytes);
                let len = wrapper::const_i64(ctx.i64_type, name_bytes.len() as i64);
                let result = ctx.builder.build_call(fn_ty, func, &mut [ctx_val, global, len], "unit_adt");
                Ok(build_ptr_tagged(ctx, result))
            } else {
                let (func, fn_ty) = get_helper(ctx, "rt_make_adt")?;
                let global = wrapper::create_global_string(&ctx.module, &ctx.llvm_ctx, &format!(".adt.{}", name_str), name_bytes);
                let name_len = wrapper::const_i64(ctx.i64_type, name_bytes.len() as i64);
                let fields_buf = build_tagged_args_array(ctx, fields, env)?;
                let nfields = wrapper::const_i64(ctx.i64_type, fields.len() as i64);
                let result = ctx.builder.build_call(fn_ty, func, &mut [ctx_val, global, name_len, fields_buf, nfields], "adt");
                Ok(build_ptr_tagged(ctx, result))
            }
        }
        IrExpr::AdtTagTest { value, constructor } => {
            let (func, fn_ty) = get_helper(ctx, "rt_is_adt_constructor")?;
            let val_ptr = force_box_to_ptr(ctx, env, *value, ctx_val)?;
            let name_str = interner.resolve(*constructor);
            let name_bytes = name_str.as_bytes();
            let global = wrapper::create_global_string(&ctx.module, &ctx.llvm_ctx, &format!(".adt.{}", name_str), name_bytes);
            let len = wrapper::const_i64(ctx.i64_type, name_bytes.len() as i64);
            let result = ctx.builder.build_call(fn_ty, func, &mut [ctx_val, val_ptr, global, len], "adt_test");
            Ok(build_bool_tagged(ctx, result))
        }
        IrExpr::AdtField { value, index } => {
            let (func, fn_ty) = get_helper(ctx, "rt_adt_field")?;
            let val_ptr = force_box_to_ptr(ctx, env, *value, ctx_val)?;
            let idx_val = wrapper::const_i64(ctx.i64_type, *index as i64);
            let result = ctx.builder.build_call(fn_ty, func, &mut [ctx_val, val_ptr, idx_val], "adt_field");
            Ok(build_ptr_tagged(ctx, result))
        }
        IrExpr::TagTest { value, tag } => {
            let helper_name = match tag {
                IrTagTest::None => "rt_is_none",
                IrTagTest::Some => "rt_is_some",
                IrTagTest::Left => "rt_is_left",
                IrTagTest::Right => "rt_is_right",
            };
            let (func, fn_ty) = get_helper(ctx, helper_name)?;
            let val_ptr = force_box_to_ptr(ctx, env, *value, ctx_val)?;
            let result = ctx.builder.build_call(fn_ty, func, &mut [ctx_val, val_ptr], "tag_test");
            Ok(build_bool_tagged(ctx, result))
        }
        IrExpr::TagPayload { value, tag } => {
            let helper_name = match tag {
                IrTagTest::Some => "rt_unwrap_some",
                IrTagTest::Left => "rt_unwrap_left",
                IrTagTest::Right => "rt_unwrap_right",
                IrTagTest::None => return Err("LLVM backend: cannot unwrap None".to_string()),
            };
            let (func, fn_ty) = get_helper(ctx, helper_name)?;
            let val_ptr = force_box_to_ptr(ctx, env, *value, ctx_val)?;
            let result = ctx.builder.build_call(fn_ty, func, &mut [ctx_val, val_ptr], "tag_payload");
            Ok(build_ptr_tagged(ctx, result))
        }
        IrExpr::ListTest { value, tag } => {
            let helper_name = match tag {
                IrListTest::Empty => "rt_is_empty_list",
                IrListTest::Cons => "rt_is_cons",
            };
            let (func, fn_ty) = get_helper(ctx, helper_name)?;
            let val_ptr = force_box_to_ptr(ctx, env, *value, ctx_val)?;
            let result = ctx.builder.build_call(fn_ty, func, &mut [ctx_val, val_ptr], "list_test");
            Ok(build_bool_tagged(ctx, result))
        }
        IrExpr::ListHead { value } => {
            let (func, fn_ty) = get_helper(ctx, "rt_cons_head")?;
            let val_ptr = force_box_to_ptr(ctx, env, *value, ctx_val)?;
            let result = ctx.builder.build_call(fn_ty, func, &mut [ctx_val, val_ptr], "list_head");
            Ok(build_ptr_tagged(ctx, result))
        }
        IrExpr::ListTail { value } => {
            let (func, fn_ty) = get_helper(ctx, "rt_cons_tail")?;
            let val_ptr = force_box_to_ptr(ctx, env, *value, ctx_val)?;
            let result = ctx.builder.build_call(fn_ty, func, &mut [ctx_val, val_ptr], "list_tail");
            Ok(build_ptr_tagged(ctx, result))
        }
        IrExpr::Some(var) => {
            let (func, fn_ty) = get_helper(ctx, "rt_make_some")?;
            let ptr = force_box_to_ptr(ctx, env, *var, ctx_val)?;
            let result = ctx.builder.build_call(fn_ty, func, &mut [ctx_val, ptr], "some");
            Ok(build_ptr_tagged(ctx, result))
        }
        IrExpr::Left(var) => {
            let (func, fn_ty) = get_helper(ctx, "rt_make_left")?;
            let ptr = force_box_to_ptr(ctx, env, *var, ctx_val)?;
            let result = ctx.builder.build_call(fn_ty, func, &mut [ctx_val, ptr], "left");
            Ok(build_ptr_tagged(ctx, result))
        }
        IrExpr::Right(var) => {
            let (func, fn_ty) = get_helper(ctx, "rt_make_right")?;
            let ptr = force_box_to_ptr(ctx, env, *var, ctx_val)?;
            let result = ctx.builder.build_call(fn_ty, func, &mut [ctx_val, ptr], "right");
            Ok(build_ptr_tagged(ctx, result))
        }
        IrExpr::Cons { head, tail } => {
            let (func, fn_ty) = get_helper(ctx, "rt_make_cons")?;
            let head_ptr = force_box_to_ptr(ctx, env, *head, ctx_val)?;
            let tail_ptr = force_box_to_ptr(ctx, env, *tail, ctx_val)?;
            let result = ctx.builder.build_call(fn_ty, func, &mut [ctx_val, head_ptr, tail_ptr], "cons");
            Ok(build_ptr_tagged(ctx, result))
        }
        IrExpr::MemberAccess { object, member, module_name: _ } => {
            // Module member access — force-box the object and look up member
            // For now, treat as a global lookup on the member name
            let _obj_val = get_var(env, *object)?;
            let name_str = interner.resolve(*member);
            // Check base functions first
            if let Some(idx) = crate::runtime::base::get_base_function_index(name_str) {
                let (make_base_fn, make_base_fn_ty) = get_helper(ctx, "rt_make_base_function")?;
                let idx_val = wrapper::const_i64(ctx.i64_type, idx as i64);
                let ptr_result = ctx.builder.build_call(make_base_fn_ty, make_base_fn, &mut [ctx_val, idx_val], "member_base_fn");
                return Ok(build_ptr_tagged(ctx, ptr_result));
            }
            Err(format!("LLVM backend: unsupported member access '{}'", name_str))
        }
        IrExpr::Perform { effect, operation, args } => {
            let (func, fn_ty) = get_helper(ctx, "rt_perform")?;
            let (force_boxed, force_boxed_ty) = get_helper(ctx, "rt_force_boxed")?;

            let effect_id = wrapper::const_i64(ctx.i64_type, effect.as_u32() as i64);
            let op_id = wrapper::const_i64(ctx.i64_type, operation.as_u32() as i64);

            // Build boxed args array (*mut Value pointers)
            let args_ptr = if args.is_empty() {
                wrapper::const_null(ctx.ptr_type)
            } else {
                let array_ty = unsafe {
                    llvm_sys::core::LLVMArrayType2(ctx.ptr_type, args.len() as u64)
                };
                let alloca = ctx.builder.build_alloca(array_ty, "perform_args");
                for (i, arg) in args.iter().enumerate() {
                    let val = get_var(env, *arg)?;
                    let tag = ctx.builder.build_extract_value(val, 0, "pa_tag");
                    let payload = ctx.builder.build_extract_value(val, 1, "pa_payload");
                    let boxed = ctx.builder.build_call(
                        force_boxed_ty, force_boxed,
                        &mut [ctx_val, tag, payload], "pa_boxed",
                    );
                    let ptr_int = ctx.builder.build_extract_value(boxed, 1, "pa_ptr_int");
                    let ptr = ctx.builder.build_int_to_ptr(ptr_int, ctx.ptr_type, "pa_ptr");
                    let slot = unsafe {
                        llvm_sys::core::LLVMBuildGEP2(
                            ctx.builder.raw_ptr(), array_ty, alloca,
                            [wrapper::const_i64(ctx.i64_type, 0), wrapper::const_i64(ctx.i64_type, i as i64)].as_mut_ptr(),
                            2, c"pa_slot".as_ptr(),
                        )
                    };
                    ctx.builder.build_store(ptr, slot);
                }
                alloca
            };
            let nargs = wrapper::const_i64(ctx.i64_type, args.len() as i64);

            // Effect and operation name strings for error messages
            let effect_name = interner.resolve(*effect);
            let op_name = interner.resolve(*operation);
            let effect_global = wrapper::create_global_string(
                &ctx.module, &ctx.llvm_ctx,
                &format!(".effect.{}", effect_name), effect_name.as_bytes(),
            );
            let op_global = wrapper::create_global_string(
                &ctx.module, &ctx.llvm_ctx,
                &format!(".op.{}", op_name), op_name.as_bytes(),
            );
            let effect_len = wrapper::const_i64(ctx.i64_type, effect_name.len() as i64);
            let op_len = wrapper::const_i64(ctx.i64_type, op_name.len() as i64);
            let zero = wrapper::const_i64(ctx.i64_type, 0);

            let result = ctx.builder.build_call(
                fn_ty, func,
                &mut [ctx_val, effect_id, op_id, args_ptr, nargs,
                      effect_global, effect_len, op_global, op_len, zero, zero],
                "perform",
            );
            Ok(build_ptr_tagged(ctx, result))
        }
        IrExpr::Handle { .. } => {
            Err("LLVM backend: Handle expression not supported (use HandleScope instruction)".to_string())
        }
    }
}

// ── Binary operations ────────────────────────────────────────────────────────

fn compile_binary(
    ctx: &LlvmCompilerContext,
    op: IrBinaryOp,
    lhs: LLVMValueRef,
    rhs: LLVMValueRef,
    ctx_val: LLVMValueRef,
) -> Result<LLVMValueRef, String> {
    // For typed integer operations, inline the arithmetic
    match op {
        IrBinaryOp::IAdd | IrBinaryOp::ISub | IrBinaryOp::IMul | IrBinaryOp::IDiv | IrBinaryOp::IMod => {
            let lhs_payload = ctx.builder.build_extract_value(lhs, 1, "lhs_p");
            let rhs_payload = ctx.builder.build_extract_value(rhs, 1, "rhs_p");
            let result = match op {
                IrBinaryOp::IAdd => ctx.builder.build_add(lhs_payload, rhs_payload, "iadd"),
                IrBinaryOp::ISub => ctx.builder.build_sub(lhs_payload, rhs_payload, "isub"),
                IrBinaryOp::IMul => ctx.builder.build_mul(lhs_payload, rhs_payload, "imul"),
                IrBinaryOp::IDiv => ctx.builder.build_sdiv(lhs_payload, rhs_payload, "idiv"),
                IrBinaryOp::IMod => ctx.builder.build_srem(lhs_payload, rhs_payload, "imod"),
                _ => unreachable!(),
            };
            Ok(build_int_tagged(ctx, result))
        }
        IrBinaryOp::Lt | IrBinaryOp::Gt | IrBinaryOp::Le | IrBinaryOp::Ge | IrBinaryOp::Eq | IrBinaryOp::NotEq => {
            // Use runtime helpers for polymorphic comparisons.
            // Note: there is no rt_less_than; for Lt we swap args and use rt_greater_than.
            let (helper_name, swap_args) = match op {
                IrBinaryOp::Lt => ("rt_greater_than", true), // a < b  ≡  b > a
                IrBinaryOp::Le => ("rt_less_than_or_equal", false),
                IrBinaryOp::Gt => ("rt_greater_than", false),
                IrBinaryOp::Ge => ("rt_greater_than_or_equal", false),
                IrBinaryOp::Eq => ("rt_equal", false),
                IrBinaryOp::NotEq => ("rt_not_equal", false),
                _ => unreachable!(),
            };
            let (func, fn_ty) = get_helper(ctx, helper_name)?;
            let (a, b) = if swap_args { (rhs, lhs) } else { (lhs, rhs) };
            let a_tag = ctx.builder.build_extract_value(a, 0, "a_tag");
            let a_payload = ctx.builder.build_extract_value(a, 1, "a_payload");
            let b_tag = ctx.builder.build_extract_value(b, 0, "b_tag");
            let b_payload = ctx.builder.build_extract_value(b, 1, "b_payload");
            let result = ctx.builder.build_call(
                fn_ty,
                func,
                &mut [ctx_val, a_tag, a_payload, b_tag, b_payload],
                "cmp",
            );
            Ok(result)
        }
        // Polymorphic / float arithmetic — delegate to runtime
        IrBinaryOp::Add | IrBinaryOp::Sub | IrBinaryOp::Mul | IrBinaryOp::Div | IrBinaryOp::Mod
        | IrBinaryOp::FAdd | IrBinaryOp::FSub | IrBinaryOp::FMul | IrBinaryOp::FDiv
        | IrBinaryOp::And | IrBinaryOp::Or => {
            let helper_name = match op {
                IrBinaryOp::Add | IrBinaryOp::FAdd => "rt_add",
                IrBinaryOp::Sub | IrBinaryOp::FSub => "rt_sub",
                IrBinaryOp::Mul | IrBinaryOp::FMul => "rt_mul",
                IrBinaryOp::Div | IrBinaryOp::FDiv => "rt_div",
                IrBinaryOp::Mod => "rt_mod",
                // And/Or at the IR level are already non-short-circuit (operands evaluated).
                // The runtime rt_add just returns its argument for bools — use rt_equal hack?
                // Actually, And/Or should be implemented as boolean operations.
                // For And: both must be truthy → (a != 0) & (b != 0)
                // For Or: either truthy → (a != 0) | (b != 0)
                // But the runtime helpers don't have rt_and/rt_or.
                // Route through the generic Add path which handles bool+bool correctly.
                IrBinaryOp::And => "rt_mul", // bool AND = multiply (0*x=0, 1*1=1)
                IrBinaryOp::Or => "rt_add",  // bool OR  = add clamped (0+0=0, 1+0=1, 1+1=2→truthy)
                _ => unreachable!(),
            };
            let (func, fn_ty) = get_helper(ctx, helper_name)?;
            let l_tag = ctx.builder.build_extract_value(lhs, 0, "l_tag");
            let l_payload = ctx.builder.build_extract_value(lhs, 1, "l_payload");
            let r_tag = ctx.builder.build_extract_value(rhs, 0, "r_tag");
            let r_payload = ctx.builder.build_extract_value(rhs, 1, "r_payload");
            let result = ctx.builder.build_call(
                fn_ty,
                func,
                &mut [ctx_val, l_tag, l_payload, r_tag, r_payload],
                "binop",
            );
            Ok(result)
        }
        // All IrBinaryOp variants are covered above.
    }
}

// ── Call compilation ─────────────────────────────────────────────────────────

fn compile_call(
    ctx: &LlvmCompilerContext,
    program: &IrProgram,
    _function: &IrFunction,
    target: &IrCallTarget,
    args: &[IrVar],
    env: &mut HashMap<IrVar, LLVMValueRef>,
    ctx_val: LLVMValueRef,
    interner: &Interner,
    adt_constructors: &HashMap<crate::syntax::Identifier, usize>,
) -> Result<LLVMValueRef, String> {
    if std::env::var("FLUX_LLVM_DUMP").is_ok() {
        eprintln!("[llvm]     compile_call target={:?}", target);
    }
    match target {
        IrCallTarget::Direct(fn_id) => {
            let fn_index = program
                .functions
                .iter()
                .position(|f| f.id == *fn_id)
                .ok_or_else(|| format!("missing direct callee {:?}", fn_id))?;
            let (callee, callee_ty) = ctx.functions[&fn_index];

            // Build args array on the stack as consecutive (tag, payload) i64s
            let args_array = build_tagged_args_array(ctx, args, env)?;
            let nargs = wrapper::const_i64(ctx.i64_type, args.len() as i64);
            let null_captures = wrapper::const_null(ctx.ptr_type);
            let zero_captures = wrapper::const_i64(ctx.i64_type, 0);

            let result = ctx.builder.build_call(
                callee_ty,
                callee,
                &mut [ctx_val, args_array, nargs, null_captures, zero_captures],
                "direct_call",
            );
            Ok(result)
        }
        IrCallTarget::Named(name) => {
            let name_str = interner.resolve(*name);

            // Check if it's a user function
            if let Some(fn_index) = program
                .functions
                .iter()
                .position(|f| f.name == Some(*name))
            {
                let (callee, callee_ty) = ctx.functions[&fn_index];
                let args_array = build_tagged_args_array(ctx, args, env)?;
                let nargs = wrapper::const_i64(ctx.i64_type, args.len() as i64);
                let null_captures = wrapper::const_null(ctx.ptr_type);
                let zero_captures = wrapper::const_i64(ctx.i64_type, 0);

                let result = ctx.builder.build_call(
                    callee_ty,
                    callee,
                    &mut [ctx_val, args_array, nargs, null_captures, zero_captures],
                    "named_call",
                );
                return Ok(result);
            }

            // Check if it's a base function
            if let Some(base_idx) = crate::runtime::base::get_base_function_index(name_str) {
                let (func, fn_ty) = get_helper(ctx, "rt_call_base_function_tagged")?;
                let idx_val = wrapper::const_i64(ctx.i64_type, base_idx as i64);
                let args_array = build_tagged_args_array(ctx, args, env)?;
                let nargs = wrapper::const_i64(ctx.i64_type, args.len() as i64);
                let zero = wrapper::const_i64(ctx.i64_type, 0);

                let result = ctx.builder.build_call(
                    fn_ty,
                    func,
                    &mut [ctx_val, idx_val, args_array, nargs, zero, zero, zero, zero],
                    "base_call",
                );
                // Result is *mut Value; wrap as PTR tag
                return Ok(build_ptr_tagged(ctx, result));
            }

            // Check if it's an ADT constructor
            if let Some(&arity) = adt_constructors.get(name) {
                if arity == args.len() {
                    // Build ADT via rt_make_adt
                    let (func, fn_ty) = get_helper(ctx, "rt_make_adt")?;
                    let name_bytes = name_str.as_bytes();
                    let global = wrapper::create_global_string(
                        &ctx.module, &ctx.llvm_ctx,
                        &format!(".adt.{}", name_str), name_bytes,
                    );
                    let name_len = wrapper::const_i64(ctx.i64_type, name_bytes.len() as i64);
                    let fields_buf = build_tagged_args_array(ctx, args, env)?;
                    let nfields = wrapper::const_i64(ctx.i64_type, args.len() as i64);
                    let result = ctx.builder.build_call(
                        fn_ty, func,
                        &mut [ctx_val, global, name_len, fields_buf, nfields],
                        "adt_ctor",
                    );
                    return Ok(build_ptr_tagged(ctx, result));
                }
            }

            Err(format!(
                "LLVM backend: unresolved named callee '{}'",
                name_str
            ))
        }
        IrCallTarget::Var(var) => {
            // Indirect call: the callee is a value in the env (closure, base fn, etc.)
            // Force-box it and call via rt_call_value
            let callee_val = get_var(env, *var)?;
            let callee_tag = ctx.builder.build_extract_value(callee_val, 0, "callee_tag");
            let callee_payload = ctx.builder.build_extract_value(callee_val, 1, "callee_payload");

            // rt_force_boxed converts a tagged value to *mut Value
            let (force_boxed, force_boxed_ty) = get_helper(ctx, "rt_force_boxed")?;
            let boxed_tv = ctx.builder.build_call(
                force_boxed_ty,
                force_boxed,
                &mut [ctx_val, callee_tag, callee_payload],
                "callee_boxed",
            );
            // The result is {tag=JIT_TAG_PTR, payload=ptr_as_i64}
            let callee_ptr_int = ctx.builder.build_extract_value(boxed_tv, 1, "callee_ptr_int");
            let callee_ptr = ctx.builder.build_int_to_ptr(callee_ptr_int, ctx.ptr_type, "callee_ptr");

            // Build args array and call rt_call_value
            let (rt_call_value, rt_call_value_ty) = get_helper(ctx, "rt_call_value")?;

            // rt_call_value expects *mut Value args, not JitTaggedValue args.
            // For now, force-box each arg individually.
            // TODO: optimize this for hot paths
            let args_ptrs = if args.is_empty() {
                wrapper::const_null(ctx.ptr_type)
            } else {
                let array_ty = unsafe {
                    llvm_sys::core::LLVMArrayType2(ctx.ptr_type, args.len() as u64)
                };
                let alloca = ctx.builder.build_alloca(array_ty, "var_args_buf");
                for (i, arg) in args.iter().enumerate() {
                    let arg_val = get_var(env, *arg)?;
                    let arg_tag = ctx.builder.build_extract_value(arg_val, 0, "varg_tag");
                    let arg_payload = ctx.builder.build_extract_value(arg_val, 1, "varg_payload");
                    let boxed = ctx.builder.build_call(
                        force_boxed_ty,
                        force_boxed,
                        &mut [ctx_val, arg_tag, arg_payload],
                        "varg_boxed",
                    );
                    let ptr_int = ctx.builder.build_extract_value(boxed, 1, "varg_ptr_int");
                    let ptr = ctx.builder.build_int_to_ptr(ptr_int, ctx.ptr_type, "varg_ptr");
                    let slot_ptr = unsafe {
                        llvm_sys::core::LLVMBuildGEP2(
                            ctx.builder.raw_ptr(),
                            array_ty,
                            alloca,
                            [wrapper::const_i64(ctx.i64_type, 0), wrapper::const_i64(ctx.i64_type, i as i64)].as_mut_ptr(),
                            2,
                            c"varg_slot".as_ptr(),
                        )
                    };
                    ctx.builder.build_store(ptr, slot_ptr);
                }
                alloca
            };

            let nargs = wrapper::const_i64(ctx.i64_type, args.len() as i64);
            let zero = wrapper::const_i64(ctx.i64_type, 0);
            let result = ctx.builder.build_call(
                rt_call_value_ty,
                rt_call_value,
                &mut [ctx_val, callee_ptr, args_ptrs, nargs, zero, zero, zero, zero],
                "var_call",
            );
            // Result is *mut Value; wrap as PTR tag
            Ok(build_ptr_tagged(ctx, result))
        }
    }
}

// ── Entry wrapper ────────────────────────────────────────────────────────────

fn compile_entry_wrapper(
    ctx: &mut LlvmCompilerContext,
    program: &IrProgram,
    interner: &Interner,
) -> Result<(), String> {
    let entry_fn_type = ctx.entry_function_type();
    let entry_wrapper = ctx.module.add_function("__flux_entry", entry_fn_type);

    let block = ctx.llvm_ctx.append_basic_block(entry_wrapper, "entry");
    ctx.builder.position_at_end(block);

    let ctx_param = wrapper::get_param(entry_wrapper, 0);
    let null_args = wrapper::const_null(ctx.ptr_type);
    let zero = wrapper::const_i64(ctx.i64_type, 0);

    // 1. Call the IR entry function (sets up globals, creates closures)
    let entry_idx = program
        .functions
        .iter()
        .position(|f| f.id == program.entry)
        .ok_or_else(|| "missing entry function".to_string())?;

    let (entry_func, entry_func_ty) = ctx.functions[&entry_idx];
    let entry_result = ctx.builder.build_call(
        entry_func_ty,
        entry_func,
        &mut [ctx_param, null_args, zero, null_args, zero],
        "entry_result",
    );

    // 2. If there's a user-defined `main` function, call it too
    //    (mirrors the Cranelift JIT's entry wrapper pattern)
    let main_idx = program
        .functions
        .iter()
        .position(|f| f.name.is_some() && interner.resolve(f.name.unwrap()) == "main");

    let result = if let Some(main_idx) = main_idx {
        if main_idx != entry_idx {
            let (main_func, main_func_ty) = ctx.functions[&main_idx];
            ctx.builder.build_call(
                main_func_ty,
                main_func,
                &mut [ctx_param, null_args, zero, null_args, zero],
                "main_result",
            )
        } else {
            entry_result
        }
    } else {
        entry_result
    };

    ctx.builder.build_ret(result);

    Ok(())
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Load a `{i64, i64}` tagged value from a pointer to an array of i64 pairs.
/// Index `i` means the pair at `ptr[i*2]` (tag) and `ptr[i*2+1]` (payload).
fn load_tagged_from_ptr(
    ctx: &LlvmCompilerContext,
    base_ptr: LLVMValueRef,
    index: usize,
    prefix: &str,
) -> LLVMValueRef {
    let tag_offset = wrapper::const_i64(ctx.i64_type, (index * 2) as i64);
    let tag_ptr = unsafe {
        llvm_sys::core::LLVMBuildGEP2(
            ctx.builder.raw_ptr(),
            ctx.i64_type,
            base_ptr,
            [tag_offset].as_mut_ptr(),
            1,
            c"tag_ptr".as_ptr(),
        )
    };
    let tag = ctx
        .builder
        .build_load(ctx.i64_type, tag_ptr, &format!("{}_tag", prefix));

    let payload_offset = wrapper::const_i64(ctx.i64_type, (index * 2 + 1) as i64);
    let payload_ptr = unsafe {
        llvm_sys::core::LLVMBuildGEP2(
            ctx.builder.raw_ptr(),
            ctx.i64_type,
            base_ptr,
            [payload_offset].as_mut_ptr(),
            1,
            c"payload_ptr".as_ptr(),
        )
    };
    let payload = ctx
        .builder
        .build_load(ctx.i64_type, payload_ptr, &format!("{}_payload", prefix));

    build_tagged_value(ctx, tag, payload)
}

/// If the destination variable is a global binding, emit `rt_set_global` to persist
/// the value in the JitContext globals array.
fn emit_set_global_if_bound(
    ctx: &LlvmCompilerContext,
    dest: IrVar,
    value: LLVMValueRef,
    ctx_val: LLVMValueRef,
    global_binding_indices: &HashMap<IrVar, usize>,
) -> Result<(), String> {
    if let Some(&global_idx) = global_binding_indices.get(&dest) {
        // Force-box the tagged value to get a *mut Value
        let (force_boxed, force_boxed_ty) = get_helper(ctx, "rt_force_boxed")?;
        let tag = ctx.builder.build_extract_value(value, 0, "gb_tag");
        let payload = ctx.builder.build_extract_value(value, 1, "gb_payload");
        let boxed = ctx.builder.build_call(
            force_boxed_ty,
            force_boxed,
            &mut [ctx_val, tag, payload],
            "gb_boxed",
        );
        let ptr_int = ctx.builder.build_extract_value(boxed, 1, "gb_ptr_int");
        let ptr = ctx.builder.build_int_to_ptr(ptr_int, ctx.ptr_type, "gb_ptr");

        let (set_global, set_global_ty) = get_helper(ctx, "rt_set_global")?;
        let idx_val = wrapper::const_i64(ctx.i64_type, global_idx as i64);
        ctx.builder.build_call(
            set_global_ty,
            set_global,
            &mut [ctx_val, idx_val, ptr],
            "",
        );
    }
    Ok(())
}

/// Force-box a tagged value to get a `*mut Value` pointer.
/// Calls `rt_force_boxed` and extracts the pointer from the result.
fn force_box_to_ptr(
    ctx: &LlvmCompilerContext,
    env: &HashMap<IrVar, LLVMValueRef>,
    var: IrVar,
    ctx_val: LLVMValueRef,
) -> Result<LLVMValueRef, String> {
    let val = get_var(env, var)?;
    let (force_boxed, force_boxed_ty) = get_helper(ctx, "rt_force_boxed")?;
    let tag = ctx.builder.build_extract_value(val, 0, "fb_tag");
    let payload = ctx.builder.build_extract_value(val, 1, "fb_payload");
    let boxed = ctx.builder.build_call(force_boxed_ty, force_boxed, &mut [ctx_val, tag, payload], "fb_boxed");
    let ptr_int = ctx.builder.build_extract_value(boxed, 1, "fb_ptr_int");
    Ok(ctx.builder.build_int_to_ptr(ptr_int, ctx.ptr_type, "fb_ptr"))
}

fn get_var(env: &HashMap<IrVar, LLVMValueRef>, var: IrVar) -> Result<LLVMValueRef, String> {
    env.get(&var)
        .copied()
        .ok_or_else(|| format!("LLVM backend: undefined variable {:?}", var))
}

fn get_helper(
    ctx: &LlvmCompilerContext,
    name: &str,
) -> Result<(LLVMValueRef, LLVMTypeRef), String> {
    ctx.helpers
        .get(name)
        .copied()
        .ok_or_else(|| format!("LLVM backend: undeclared helper '{}'", name))
}

/// Build a `{i64, i64}` tagged value from tag and payload.
fn build_tagged_value(
    ctx: &LlvmCompilerContext,
    tag: LLVMValueRef,
    payload: LLVMValueRef,
) -> LLVMValueRef {
    let undef = wrapper::get_undef(ctx.tagged_value_type);
    let v1 = ctx.builder.build_insert_value(undef, tag, 0, "tv_tag");
    ctx.builder.build_insert_value(v1, payload, 1, "tv")
}

/// Build a tagged integer constant.
fn build_int_tagged(ctx: &LlvmCompilerContext, value: impl IntoI64OrValue) -> LLVMValueRef {
    let tag = wrapper::const_i64(ctx.i64_type, JIT_TAG_INT);
    let payload = value.to_llvm_value(ctx.i64_type);
    build_tagged_value(ctx, tag, payload)
}

/// Build a tagged boolean constant.
fn build_bool_tagged(ctx: &LlvmCompilerContext, value: impl IntoBoolOrValue) -> LLVMValueRef {
    let tag = wrapper::const_i64(ctx.i64_type, JIT_TAG_BOOL);
    let payload = value.to_bool_payload(ctx.i64_type);
    build_tagged_value(ctx, tag, payload)
}

trait IntoBoolOrValue {
    fn to_bool_payload(self, i64_type: LLVMTypeRef) -> LLVMValueRef;
}

impl IntoBoolOrValue for bool {
    fn to_bool_payload(self, i64_type: LLVMTypeRef) -> LLVMValueRef {
        wrapper::const_i64(i64_type, self as i64)
    }
}

impl IntoBoolOrValue for LLVMValueRef {
    fn to_bool_payload(self, _i64_type: LLVMTypeRef) -> LLVMValueRef {
        self // already an i64 value (0 or 1)
    }
}

/// Build a tagged PTR value from a pointer.
fn build_ptr_tagged(ctx: &LlvmCompilerContext, ptr: LLVMValueRef) -> LLVMValueRef {
    let tag = wrapper::const_i64(ctx.i64_type, JIT_TAG_PTR);
    let payload = ctx
        .builder
        .build_ptr_to_int(ptr, ctx.i64_type, "ptr_as_int");
    build_tagged_value(ctx, tag, payload)
}

/// Build an array of tagged values on the stack for passing to function calls.
/// Returns a pointer to the first element (or null if empty).
fn build_tagged_args_array(
    ctx: &LlvmCompilerContext,
    args: &[IrVar],
    env: &HashMap<IrVar, LLVMValueRef>,
) -> Result<LLVMValueRef, String> {
    if args.is_empty() {
        return Ok(wrapper::const_null(ctx.ptr_type));
    }

    // Allocate space for args.len() * 2 i64s on the stack as a flat i64 array.
    let total_slots = args.len() * 2;
    let array_ty = unsafe {
        llvm_sys::core::LLVMArrayType2(ctx.i64_type, total_slots as u64)
    };
    let alloca = ctx.builder.build_alloca(array_ty, "args_buf");

    for (i, arg) in args.iter().enumerate() {
        let val = get_var(env, *arg)?;
        let tag = ctx.builder.build_extract_value(val, 0, "tag");
        let payload = ctx.builder.build_extract_value(val, 1, "payload");

        // GEP with [array_ty, 0, offset] to index into [N x i64] array
        let tag_offset = wrapper::const_i64(ctx.i64_type, (i * 2) as i64);
        let tag_ptr = unsafe {
            llvm_sys::core::LLVMBuildGEP2(
                ctx.builder.raw_ptr(),
                array_ty,
                alloca,
                [wrapper::const_i64(ctx.i64_type, 0), tag_offset].as_mut_ptr(),
                2,
                c"arg_tag_slot".as_ptr(),
            )
        };
        ctx.builder.build_store(tag, tag_ptr);

        let payload_offset = wrapper::const_i64(ctx.i64_type, (i * 2 + 1) as i64);
        let payload_ptr = unsafe {
            llvm_sys::core::LLVMBuildGEP2(
                ctx.builder.raw_ptr(),
                array_ty,
                alloca,
                [wrapper::const_i64(ctx.i64_type, 0), payload_offset].as_mut_ptr(),
                2,
                c"arg_payload_slot".as_ptr(),
            )
        };
        ctx.builder.build_store(payload, payload_ptr);
    }

    Ok(alloca)
}

/// Trait to accept both i64 constants and LLVM values for `build_int_tagged`.
trait IntoI64OrValue {
    fn to_llvm_value(self, i64_type: LLVMTypeRef) -> LLVMValueRef;
}

impl IntoI64OrValue for i64 {
    fn to_llvm_value(self, i64_type: LLVMTypeRef) -> LLVMValueRef {
        wrapper::const_i64(i64_type, self)
    }
}

impl IntoI64OrValue for LLVMValueRef {
    fn to_llvm_value(self, _i64_type: LLVMTypeRef) -> LLVMValueRef {
        self
    }
}
