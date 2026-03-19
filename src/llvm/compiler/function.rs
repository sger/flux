//! Function and block compilation: translates `IrFunction` and `IrBlock` into LLVM IR.

use std::collections::HashMap;

use llvm_sys::LLVMIntPredicate;
use llvm_sys::prelude::*;

use crate::cfg::{
    BlockId, IrBinaryOp, IrBlock, IrCallTarget, IrExpr, IrFunction, IrInstr, IrProgram,
    IrTerminator, IrVar,
};
use crate::syntax::interner::Interner;

use super::super::context::LlvmCompilerContext;
use super::super::wrapper;
use super::calls::compile_call;
use super::expressions::compile_expr;
use super::helpers::{
    build_tagged_args_array, emit_error_check_and_return, emit_set_global_if_bound, get_helper,
    get_var, load_tagged_from_ptr,
};

// ── Function compilation ─────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub(super) fn compile_function(
    ctx: &LlvmCompilerContext,
    program: &IrProgram,
    function: &IrFunction,
    fn_index: usize,
    interner: &Interner,
    adt_constructors: &HashMap<crate::syntax::Identifier, usize>,
    module_functions: &HashMap<(crate::syntax::Identifier, crate::syntax::Identifier), usize>,
    module_names: &[crate::syntax::Identifier],
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
            eprintln!(
                "[llvm] entry fn: {} global bindings, {} globals",
                program.global_bindings.len(),
                program.globals.len()
            );
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
                let phi = ctx.builder.build_phi(
                    ctx.tagged_value_type,
                    &format!("phi_b{}_p{}", block.id.0, param_idx),
                );
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
                if let Some(cont_block) = function
                    .blocks
                    .iter()
                    .find(|b| b.params.iter().any(|p| p.var == *body_result))
                {
                    *handler_pop_counts.entry(cont_block.id).or_insert(0) += 1;
                }
            }
        }
    }

    // Track which IrVars hold known user function closures (no captures).
    let mut var_fn_map: HashMap<IrVar, usize> = HashMap::new();
    // Track which IrVars hold module references (for MemberAccess resolution).
    let mut module_env: HashMap<IrVar, crate::syntax::Identifier> = HashMap::new();

    // Compile each block
    for block in &function.blocks {
        if std::env::var("FLUX_LLVM_DUMP").is_ok() {
            eprintln!(
                "[llvm]  block {} (entry={}) instrs={}",
                block.id.0,
                block.id == function.entry,
                block.instrs.len()
            );
            let _ = std::io::Write::flush(&mut std::io::stderr());
        }
        let llvm_block = block_map[&block.id];
        // Only position if this isn't the entry block (already positioned)
        if block.id != function.entry {
            ctx.builder.position_at_end(llvm_block);
        }

        // Emit rt_pop_handler calls for continuation blocks
        if let Some(&pop_count) = handler_pop_counts.get(&block.id) {
            let (pop_handler, pop_handler_ty) =
                get_helper(ctx, "rt_pop_handler").map_err(|e| format!("handler pop: {}", e))?;
            for _ in 0..pop_count {
                ctx.builder
                    .build_call(pop_handler_ty, pop_handler, &mut [ctx_val], "");
            }
        }

        // Load block params from PHI nodes into env
        for (param_idx, param) in block.params.iter().enumerate() {
            if let Some(&phi) = phi_map.get(&(block.id, param_idx)) {
                env.insert(param.var, phi);
            }
        }

        compile_block(
            ctx,
            program,
            function,
            fn_index,
            block,
            &block_map,
            &mut env,
            ctx_val,
            func_ref,
            interner,
            &phi_map,
            &global_binding_indices,
            adt_constructors,
            &mut var_fn_map,
            module_functions,
            module_names,
            &mut module_env,
        )?;
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn compile_block(
    ctx: &LlvmCompilerContext,
    program: &IrProgram,
    function: &IrFunction,
    current_fn_index: usize,
    block: &IrBlock,
    block_map: &HashMap<BlockId, LLVMBasicBlockRef>,
    env: &mut HashMap<IrVar, LLVMValueRef>,
    ctx_val: LLVMValueRef,
    func_ref: LLVMValueRef,
    interner: &Interner,
    phi_map: &HashMap<(BlockId, usize), LLVMValueRef>,
    global_binding_indices: &HashMap<IrVar, usize>,
    adt_constructors: &HashMap<crate::syntax::Identifier, usize>,
    var_fn_map: &mut HashMap<IrVar, usize>,
    module_functions: &HashMap<(crate::syntax::Identifier, crate::syntax::Identifier), usize>,
    module_names: &[crate::syntax::Identifier],
    module_env: &mut HashMap<IrVar, crate::syntax::Identifier>,
) -> Result<(), String> {
    // Compile instructions
    for (instr_idx, instr) in block.instrs.iter().enumerate() {
        if std::env::var("FLUX_LLVM_DUMP").is_ok() {
            match instr {
                IrInstr::Assign { dest, expr, .. } => {
                    eprintln!(
                        "[llvm]   instr {}: Assign v{} = {:?}",
                        instr_idx,
                        dest.0,
                        std::mem::discriminant(expr)
                    );
                    let _ = std::io::Write::flush(&mut std::io::stderr());
                }
                IrInstr::Call {
                    dest, target, args, ..
                } => {
                    eprintln!(
                        "[llvm]   instr {}: Call v{} target={:?} nargs={}",
                        instr_idx,
                        dest.0,
                        target,
                        args.len()
                    );
                    let _ = std::io::Write::flush(&mut std::io::stderr());
                }
                IrInstr::HandleScope { .. } => {
                    eprintln!("[llvm]   instr {}: HandleScope", instr_idx)
                }
                IrInstr::AetherDrop { var, .. } => {
                    eprintln!("[llvm]   instr {}: AetherDrop v{}", instr_idx, var.0)
                }
            }
        }
        match instr {
            IrInstr::Assign {
                dest,
                expr,
                metadata,
            } => {
                // Track if this variable holds a known user function (for TCO)
                match expr {
                    IrExpr::MakeClosure(fn_id, captures) if captures.is_empty() => {
                        if let Some(idx) = program.functions.iter().position(|f| f.id == *fn_id) {
                            var_fn_map.insert(*dest, idx);
                        }
                    }
                    IrExpr::LoadName(name) => {
                        if let Some(idx) =
                            program.functions.iter().position(|f| f.name == Some(*name))
                        {
                            var_fn_map.insert(*dest, idx);
                        }
                        // Track if this var holds a module reference
                        let is_mod = interner.resolve(*name) == "Base"
                            || module_names.contains(name)
                            || module_names.iter().any(|m| {
                                interner
                                    .resolve(*name)
                                    .starts_with(&format!("{}.", interner.resolve(*m)))
                            });
                        if is_mod {
                            module_env.insert(*dest, *name);
                        }
                    }
                    _ => {}
                }
                let value = compile_expr(
                    ctx,
                    program,
                    expr,
                    env,
                    ctx_val,
                    func_ref,
                    interner,
                    adt_constructors,
                    module_functions,
                    module_names,
                    module_env,
                )
                .map_err(|e| format!("in assign v{}: {}", dest.0, e))?;

                // After runtime-dispatched binary ops, check for errors.
                // IAdd/ISub/IMul/IDiv/IMod are inlined (operands proven Int) — no check needed.
                // All others call rt_* helpers that may set ctx.error on type mismatches.
                if (matches!(
                    expr,
                    IrExpr::Binary(
                        IrBinaryOp::Add
                            | IrBinaryOp::Sub
                            | IrBinaryOp::Mul
                            | IrBinaryOp::Div
                            | IrBinaryOp::Mod
                            | IrBinaryOp::FAdd
                            | IrBinaryOp::FSub
                            | IrBinaryOp::FMul
                            | IrBinaryOp::FDiv
                            | IrBinaryOp::Gt
                            | IrBinaryOp::Ge
                            | IrBinaryOp::Le
                            | IrBinaryOp::Lt
                            | IrBinaryOp::Eq
                            | IrBinaryOp::NotEq,
                        _,
                        _
                    )
                ) || matches!(expr, IrExpr::Prefix { .. }))
                    && let Some(span) = &metadata.span
                {
                    emit_error_check_and_return(ctx, ctx_val, func_ref, span)?;
                }

                env.insert(*dest, value);
                emit_set_global_if_bound(ctx, *dest, value, ctx_val, global_binding_indices)?;
            }
            IrInstr::Call {
                dest,
                target,
                args,
                metadata,
            } => {
                let value = compile_call(
                    ctx,
                    program,
                    function,
                    target,
                    args,
                    env,
                    ctx_val,
                    func_ref,
                    interner,
                    adt_constructors,
                    metadata.span,
                )?;
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
                    let ops_array_ty =
                        unsafe { llvm_sys::core::LLVMArrayType2(ctx.i64_type, narms as u64) };
                    let ops_alloca = ctx.builder.build_alloca(ops_array_ty, "handler_ops");
                    for (i, arm) in arms.iter().enumerate() {
                        let op_id =
                            wrapper::const_i64(ctx.i64_type, arm.operation_name.as_u32() as i64);
                        let slot = unsafe {
                            llvm_sys::core::LLVMBuildGEP2(
                                ctx.builder.raw_ptr(),
                                ops_array_ty,
                                ops_alloca,
                                [
                                    wrapper::const_i64(ctx.i64_type, 0),
                                    wrapper::const_i64(ctx.i64_type, i as i64),
                                ]
                                .as_mut_ptr(),
                                2,
                                c"op_slot".as_ptr(),
                            )
                        };
                        let s = ctx.builder.build_store(op_id, slot);
                        wrapper::set_tbaa(s, ctx.tbaa_args);
                    }

                    // Build closures array: [closure0_ptr, closure1_ptr, ...]
                    let closures_array_ty =
                        unsafe { llvm_sys::core::LLVMArrayType2(ctx.ptr_type, narms as u64) };
                    let closures_alloca = ctx
                        .builder
                        .build_alloca(closures_array_ty, "handler_closures");
                    let (make_closure, make_closure_ty) = get_helper(ctx, "rt_make_jit_closure")?;

                    for (i, arm) in arms.iter().enumerate() {
                        // Find the function index for this arm's function
                        let arm_fn_index = program
                            .functions
                            .iter()
                            .position(|f| f.id == arm.function_id)
                            .ok_or_else(|| {
                                format!("missing handler arm function {:?}", arm.function_id)
                            })?;

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
                            let ncaptures =
                                wrapper::const_i64(ctx.i64_type, arm.capture_vars.len() as i64);
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
                                [
                                    wrapper::const_i64(ctx.i64_type, 0),
                                    wrapper::const_i64(ctx.i64_type, i as i64),
                                ]
                                .as_mut_ptr(),
                                2,
                                c"closure_slot".as_ptr(),
                            )
                        };
                        let s = ctx.builder.build_store(closure_ptr, slot);
                        wrapper::set_tbaa(s, ctx.tbaa_heap);
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
            IrInstr::AetherDrop { var, .. } => {
                // Aether early-release: overwrite the arena slot with Value::None
                // so the old Rc is decremented immediately.  Only act when the
                // tagged value has tag == JIT_TAG_PTR (heap pointer); primitives
                // (Int/Float/Bool) carry no Rc and need no action.
                if let Ok(tagged) = get_var(env, *var) {
                    let tag = ctx.builder.build_extract_value(tagged, 0, "ad_tag");
                    let ptr_tag = wrapper::const_i64(
                        ctx.i64_type,
                        crate::runtime::native_context::JIT_TAG_PTR,
                    );
                    let is_ptr = ctx.builder.build_icmp(
                        LLVMIntPredicate::LLVMIntEQ,
                        tag,
                        ptr_tag,
                        "ad_is_ptr",
                    );
                    let drop_bb = ctx.llvm_ctx.append_basic_block(func_ref, "aether_drop");
                    let cont_bb = ctx.llvm_ctx.append_basic_block(func_ref, "aether_cont");
                    ctx.builder.build_cond_br(is_ptr, drop_bb, cont_bb);

                    ctx.builder.position_at_end(drop_bb);
                    let payload = ctx.builder.build_extract_value(tagged, 1, "ad_payload");
                    let ptr_val = ctx
                        .builder
                        .build_int_to_ptr(payload, ctx.ptr_type, "ad_ptr");
                    let (drop_fn, drop_ty) = get_helper(ctx, "rt_aether_drop")?;
                    ctx.builder
                        .build_call(drop_ty, drop_fn, &mut [ctx_val, ptr_val], "");
                    ctx.builder.build_br(cont_bb);

                    ctx.builder.position_at_end(cont_bb);
                }
            }
        }
    }

    // Compile terminator
    if std::env::var("FLUX_LLVM_DUMP").is_ok() {
        eprintln!(
            "[llvm]    terminator: {:?}",
            std::mem::discriminant(&block.terminator)
        );
        let _ = std::io::Write::flush(&mut std::io::stderr());
    }
    match &block.terminator {
        IrTerminator::Return(var, _) => {
            if std::env::var("FLUX_LLVM_DUMP").is_ok() {
                eprintln!(
                    "[llvm]    return v{}, in_env={}",
                    var.0,
                    env.contains_key(var)
                );
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
            // Add incoming edges to target block's PHI nodes.
            // Use get_insert_block() instead of block_map — after null-check
            // insertions, the builder may be in a different (continue) block.
            let current_llvm_block = ctx.builder.get_insert_block();
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
            let payload = ctx.builder.build_extract_value(cond_val, 1, "cond_payload");
            let zero = wrapper::const_i64(ctx.i64_type, 0);
            let cond_bool =
                ctx.builder
                    .build_icmp(LLVMIntPredicate::LLVMIntNE, payload, zero, "cond_bool");
            let llvm_then = block_map[then_block];
            let llvm_else = block_map[else_block];
            ctx.builder.build_cond_br(cond_bool, llvm_then, llvm_else);
        }
        IrTerminator::TailCall { callee, args, .. } => {
            // Thunk-based tail call optimization: instead of a direct tail call
            // (which can't use musttail due to stack-allocated args buffers),
            // call rt_set_thunk to copy args to the heap and return a JIT_TAG_THUNK
            // marker. The trampoline loop in llvm_execute re-invokes the target.
            let resolved_fn_index = match callee {
                IrCallTarget::Direct(fn_id) => Some(
                    program
                        .functions
                        .iter()
                        .position(|f| f.id == *fn_id)
                        .ok_or_else(|| format!("missing tail callee {:?}", fn_id))?,
                ),
                IrCallTarget::Named(name) => {
                    program.functions.iter().position(|f| f.name == Some(*name))
                }
                IrCallTarget::Var(var) => var_fn_map.get(var).copied(),
            };

            if let Some(fn_index) = resolved_fn_index {
                if fn_index == current_fn_index {
                    // Self-recursive tail call: emit direct call + ret.
                    // LLVM's tailcallelim pass will convert this to a loop.
                    let (callee_fn, callee_ty) = ctx.functions[&fn_index];
                    let args_array = build_tagged_args_array(ctx, args, env)?;
                    let nargs = wrapper::const_i64(ctx.i64_type, args.len() as i64);
                    let null_captures = wrapper::const_null(ctx.ptr_type);
                    let zero_captures = wrapper::const_i64(ctx.i64_type, 0);
                    let call_inst = ctx.builder.build_call(
                        callee_ty,
                        callee_fn,
                        &mut [ctx_val, args_array, nargs, null_captures, zero_captures],
                        "self_tail_call",
                    );
                    unsafe {
                        llvm_sys::core::LLVMSetTailCallKind(
                            call_inst,
                            llvm_sys::LLVMTailCallKind::LLVMTailCallKindTail,
                        );
                    }
                    ctx.builder.build_ret(call_inst);
                } else {
                    // Mutual recursion: use thunk trampoline
                    let args_array = build_tagged_args_array(ctx, args, env)?;
                    let nargs = wrapper::const_i64(ctx.i64_type, args.len() as i64);
                    let fn_idx_val = wrapper::const_i64(ctx.i64_type, fn_index as i64);
                    let (set_thunk, set_thunk_ty) = get_helper(ctx, "rt_set_thunk")?;
                    let result = ctx.builder.build_call(
                        set_thunk_ty,
                        set_thunk,
                        &mut [ctx_val, fn_idx_val, args_array, nargs],
                        "set_thunk",
                    );
                    ctx.builder.build_ret(result);
                }
            } else {
                // Unknown callee — fall back to regular call+return
                let value = compile_call(
                    ctx,
                    program,
                    function,
                    callee,
                    args,
                    env,
                    ctx_val,
                    func_ref,
                    interner,
                    adt_constructors,
                    None,
                )?;
                ctx.builder.build_ret(value);
            }
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
