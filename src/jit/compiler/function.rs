use super::*;

impl JitCompiler {
    pub(super) fn compile_simple_backend_ir_function(
        &mut self,
        function: &BackendIrFunction,
        scope: &Scope,
        backend_function_metas: &HashMap<BackendFunctionId, JitFunctionMeta>,
        backend_function_defs: &HashMap<BackendFunctionId, &BackendIrFunction>,
        global_binding_indices: &HashMap<BackendIrVar, usize>,
        entry_function_id: BackendFunctionId,
        interner: &Interner,
    ) -> Result<(), String> {
        let meta = backend_function_metas
            .get(&function.id)
            .copied()
            .ok_or_else(|| "missing backend function metadata".to_string())?;

        let sig = self.user_function_signature(meta.call_abi);
        let mut func = Function::with_name_signature(UserFuncName::default(), sig);
        {
            let module = &mut self.module;
            let helpers = &self.helpers;
            let mut builder = FunctionBuilder::new(&mut func, &mut self.builder_ctx);
            let prelude = builder.create_block();
            let mut block_map = HashMap::new();
            let block_defs: HashMap<BackendBlockId, &crate::cfg::IrBlock> = function
                .blocks
                .iter()
                .map(|block| (block.id, block))
                .collect();
            let block_order = ordered_backend_blocks(function);
            for block in &function.blocks {
                let cl_block = builder.create_block();
                block_map.insert(block.id, cl_block);
            }
            let entry = block_map[&function.entry];
            builder.append_block_params_for_function_params(prelude);
            for block in &function.blocks {
                if block.id == function.entry {
                    continue;
                }
                let cl_block = block_map[&block.id];
                for _ in &block.params {
                    builder.append_block_param(cl_block, PTR_TYPE);
                }
            }
            builder.switch_to_block(prelude);

            let mut env = HashMap::new();
            let mut module_env = HashMap::new();
            let mut function_env = HashMap::new();
            let mut handler_pop_counts: HashMap<BackendBlockId, usize> = HashMap::new();
            let mut block_envs: HashMap<BackendBlockId, HashMap<BackendIrVar, JitValue>> =
                HashMap::new();
            let mut block_module_envs: HashMap<BackendBlockId, HashMap<BackendIrVar, Identifier>> =
                HashMap::new();
            let mut block_function_envs: HashMap<
                BackendBlockId,
                HashMap<BackendIrVar, JitFunctionMeta>,
            > = HashMap::new();
            let ctx_val = builder.block_params(prelude)[0];
            let params = builder.block_params(prelude).to_vec();
            let args_ptr = if meta.call_abi.uses_array_args() {
                Some(params[1])
            } else {
                None
            };
            let captures_ptr = params[meta.call_abi.captures_param_index()];
            let capture_count = function.captures.len();
            let explicit_arity = function.params.len().saturating_sub(capture_count);
            let init_block = builder.create_block();
            if let Some(args_ptr) = args_ptr {
                let nargs = params[2];
                let want = builder.ins().iconst(PTR_TYPE, explicit_arity as i64);
                let arity_ok = builder.ins().icmp(IntCC::Equal, nargs, want);
                let arity_fail = builder.create_block();
                builder
                    .ins()
                    .brif(arity_ok, init_block, &[], arity_fail, &[]);

                builder.switch_to_block(arity_fail);
                let set_arity_error =
                    get_helper_func_ref(module, helpers, &mut builder, "rt_set_arity_error");
                builder.ins().call(set_arity_error, &[ctx_val, nargs, want]);
                emit_return_null_tagged(&mut builder);
                builder.seal_block(arity_fail);

                builder.switch_to_block(init_block);
                let _ = args_ptr;
            } else {
                builder.ins().jump(init_block, &[]);
                builder.switch_to_block(init_block);
            }
            for (idx, param) in function.params.iter().take(capture_count).enumerate() {
                let cap_tag = builder.ins().load(
                    types::I64,
                    MemFlags::new(),
                    captures_ptr,
                    (idx * 16) as i32,
                );
                let cap_payload = builder.ins().load(
                    PTR_TYPE,
                    MemFlags::new(),
                    captures_ptr,
                    (idx * 16 + 8) as i32,
                );
                env.insert(
                    param.var,
                    JitValue::boxed(boxed_value_from_tagged_parts(
                        module,
                        helpers,
                        &mut builder,
                        ctx_val,
                        cap_tag,
                        cap_payload,
                    )),
                );
                module_env.remove(&param.var);
                function_env.remove(&param.var);
            }
            for (idx, param) in function.params.iter().skip(capture_count).enumerate() {
                let (tag, payload) = match args_ptr {
                    Some(args_ptr) => {
                        let tag = builder.ins().load(
                            types::I64,
                            MemFlags::new(),
                            args_ptr,
                            (idx * 16) as i32,
                        );
                        let payload = builder.ins().load(
                            PTR_TYPE,
                            MemFlags::new(),
                            args_ptr,
                            (idx * 16 + 8) as i32,
                        );
                        (tag, payload)
                    }
                    None => {
                        let base = 1 + idx * 2;
                        (params[base], params[base + 1])
                    }
                };
                env.insert(
                    param.var,
                    JitValue::boxed(boxed_value_from_tagged_parts(
                        module,
                        helpers,
                        &mut builder,
                        ctx_val,
                        tag,
                        payload,
                    )),
                );
                module_env.remove(&param.var);
                function_env.remove(&param.var);
            }
            block_envs.insert(function.entry, env.clone());
            block_module_envs.insert(function.entry, module_env.clone());
            block_function_envs.insert(function.entry, function_env.clone());
            builder.ins().jump(entry, &[]);

            for block in &block_order {
                for instr in &block.instrs {
                    if let BackendIrInstr::HandleScope { body_result, .. } = instr {
                        let Some(cont_block) = function.blocks.iter().find(|candidate| {
                            candidate.params.iter().any(|p| p.var == *body_result)
                        }) else {
                            return Err("backend JIT path missing handle-scope continuation block"
                                .to_string());
                        };
                        *handler_pop_counts.entry(cont_block.id).or_insert(0) += 1;
                    }
                }
            }

            for block in &block_order {
                let cl_block = block_map[&block.id];
                builder.switch_to_block(cl_block);

                let mut env = block_envs.remove(&block.id).unwrap_or_default();
                let mut module_env = block_module_envs.remove(&block.id).unwrap_or_default();
                let mut function_env = block_function_envs.remove(&block.id).unwrap_or_default();

                if block.id != function.entry {
                    let block_params = builder.block_params(cl_block).to_vec();
                    for (idx, param) in block.params.iter().enumerate() {
                        env.insert(param.var, JitValue::boxed(block_params[idx]));
                        module_env.remove(&param.var);
                        function_env.remove(&param.var);
                    }
                }

                if let Some(pop_count) = handler_pop_counts.get(&block.id).copied() {
                    let rt_pop_handler =
                        get_helper_func_ref(module, helpers, &mut builder, "rt_pop_handler");
                    for _ in 0..pop_count {
                        builder.ins().call(rt_pop_handler, &[ctx_val]);
                    }
                }

                for instr in &block.instrs {
                    match instr {
                        BackendIrInstr::Assign {
                            dest,
                            expr,
                            metadata,
                        } => {
                            let value = compile_simple_backend_ir_expr(
                                module,
                                helpers,
                                &mut builder,
                                ctx_val,
                                &env,
                                &module_env,
                                scope,
                                backend_function_metas,
                                backend_function_defs,
                                interner,
                                expr,
                            )?;

                            // After runtime-dispatched binary/prefix ops, check for errors.
                            // IAdd/ISub/IMul/IDiv/IMod are inlined (operands proven Int) — no check needed.
                            if matches!(
                                expr,
                                BackendIrExpr::Binary(
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
                                ) | BackendIrExpr::Prefix { .. }
                            ) && let Some(span) = metadata.span
                            {
                                emit_error_check_and_return(
                                    module,
                                    helpers,
                                    &mut builder,
                                    ctx_val,
                                    span,
                                );
                            }

                            env.insert(*dest, value);
                            match expr {
                                BackendIrExpr::LoadName(name) => {
                                    if let Some(module_name) =
                                        resolve_module_name(scope, interner, *name)
                                    {
                                        module_env.insert(*dest, module_name);
                                    } else {
                                        module_env.remove(dest);
                                    }
                                    if let Some(meta) = scope.functions.get(name).copied() {
                                        function_env.insert(*dest, meta);
                                    } else {
                                        function_env.remove(dest);
                                    }
                                }
                                _ => {
                                    module_env.remove(dest);
                                    function_env.remove(dest);
                                }
                            }
                            if function.id == entry_function_id
                                && let Some(&global_idx) = global_binding_indices.get(dest)
                            {
                                let boxed = box_and_guard_jit_value(
                                    module,
                                    helpers,
                                    &mut builder,
                                    ctx_val,
                                    value,
                                );
                                let set_global = get_helper_func_ref(
                                    module,
                                    helpers,
                                    &mut builder,
                                    "rt_set_global",
                                );
                                let idx_val = builder.ins().iconst(PTR_TYPE, global_idx as i64);
                                builder.ins().call(set_global, &[ctx_val, idx_val, boxed]);
                            }
                        }
                        BackendIrInstr::Call {
                            dest,
                            target,
                            args,
                            metadata,
                        } => {
                            let arg_vals = args
                                .iter()
                                .map(|arg| {
                                    env.get(arg)
                                        .copied()
                                        .ok_or_else(|| format!("missing backend IR var {:?}", arg))
                                })
                                .collect::<Result<Vec<_>, _>>()?;
                            let value = match target {
                                IrCallTarget::Direct(function_id) => {
                                    let callee = backend_function_metas
                                        .get(function_id)
                                        .copied()
                                        .ok_or_else(|| {
                                        "missing direct backend callee metadata".to_string()
                                    })?;
                                    compile_jit_cfg_user_function_call(
                                        module,
                                        helpers,
                                        &mut builder,
                                        ctx_val,
                                        callee,
                                        &arg_vals,
                                        metadata.span.unwrap_or(function.body_span),
                                    )?
                                }
                                IrCallTarget::Named(name) => {
                                    if let Some(callee) = scope.functions.get(name).copied() {
                                        compile_jit_cfg_user_function_call(
                                            module,
                                            helpers,
                                            &mut builder,
                                            ctx_val,
                                            callee,
                                            &arg_vals,
                                            metadata.span.unwrap_or(function.body_span),
                                        )?
                                    } else if let Some(primop) =
                                        resolve_primop_call(interner.resolve(*name), arg_vals.len())
                                    {
                                        compile_jit_cfg_primop_call(
                                            module,
                                            helpers,
                                            &mut builder,
                                            ctx_val,
                                            primop,
                                            &arg_vals,
                                            metadata.span.unwrap_or(function.body_span),
                                        )?
                                    } else if let Some(&base_idx) = scope.base_functions.get(name) {
                                        let boxed_args = arg_vals
                                            .iter()
                                            .map(|value| {
                                                box_and_guard_jit_value(
                                                    module,
                                                    helpers,
                                                    &mut builder,
                                                    ctx_val,
                                                    *value,
                                                )
                                            })
                                            .collect::<Vec<_>>();
                                        compile_jit_cfg_base_function_call(
                                            module,
                                            helpers,
                                            &mut builder,
                                            ctx_val,
                                            base_idx,
                                            &boxed_args,
                                            metadata.span.unwrap_or(function.body_span),
                                        )?
                                    } else if let Some(&global_idx) = scope.globals.get(name) {
                                        let get_global = get_helper_func_ref(
                                            module,
                                            helpers,
                                            &mut builder,
                                            "rt_get_global",
                                        );
                                        let idx_val =
                                            builder.ins().iconst(PTR_TYPE, global_idx as i64);
                                        let call =
                                            builder.ins().call(get_global, &[ctx_val, idx_val]);
                                        let callee = JitValue::boxed(builder.inst_results(call)[0]);
                                        compile_jit_cfg_generic_call(
                                            module,
                                            helpers,
                                            &mut builder,
                                            ctx_val,
                                            callee,
                                            &arg_vals,
                                            metadata.span.unwrap_or(function.body_span),
                                        )?
                                    } else if let Some(&arity) = scope.adt_constructors.get(name) {
                                        if arity != arg_vals.len() {
                                            return Err(format!(
                                                "backend named constructor arity mismatch for {}",
                                                interner.resolve(*name)
                                            ));
                                        }
                                        compile_backend_named_adt_constructor_call(
                                            module,
                                            helpers,
                                            &mut builder,
                                            ctx_val,
                                            *name,
                                            &arg_vals,
                                            interner,
                                        )?
                                    } else {
                                        return Err(format!(
                                            "missing named backend callee metadata for {}",
                                            interner.resolve(*name)
                                        ));
                                    }
                                }
                                IrCallTarget::Var(var) => {
                                    let callee = env.get(var).copied().ok_or_else(|| {
                                        format!("missing backend indirect callee var {:?}", var)
                                    })?;
                                    compile_jit_cfg_generic_call(
                                        module,
                                        helpers,
                                        &mut builder,
                                        ctx_val,
                                        callee,
                                        &arg_vals,
                                        metadata.span.unwrap_or(function.body_span),
                                    )?
                                }
                            };
                            env.insert(*dest, value);
                            module_env.remove(dest);
                            function_env.remove(dest);
                        }
                        BackendIrInstr::AetherDrop { var, .. } => {
                            // Aether early-release: overwrite the arena slot with
                            // Value::None so the old Rc is decremented immediately.
                            // Only act on boxed (heap-pointer) values; primitives
                            // (Int/Float/Bool) have no Rc to drop.
                            if let Some(binding) = env.get(var)
                                && binding.kind == JitValueKind::Boxed
                            {
                                let drop_fn = get_helper_func_ref(
                                    module,
                                    helpers,
                                    &mut builder,
                                    "rt_aether_drop",
                                );
                                builder.ins().call(drop_fn, &[ctx_val, binding.value]);
                            }
                        }
                        BackendIrInstr::HandleScope { effect, arms, .. } => {
                            let mut op_sym_vals = Vec::with_capacity(arms.len());
                            let mut closure_vals = Vec::with_capacity(arms.len());
                            for arm in arms {
                                op_sym_vals.push(
                                    builder
                                        .ins()
                                        .iconst(PTR_TYPE, arm.operation_name.as_u32() as i64),
                                );
                                let arm_fn = backend_function_defs
                                    .get(&arm.function_id)
                                    .copied()
                                    .ok_or_else(|| {
                                    "missing backend handle arm function definition".to_string()
                                })?;
                                let meta = backend_function_metas
                                    .get(&arm.function_id)
                                    .copied()
                                    .ok_or_else(|| {
                                        "missing backend handle arm metadata".to_string()
                                    })?;
                                if arm_fn.captures.len() != arm.capture_vars.len() {
                                    return Err(
                                        "backend JIT path requires explicit handle-arm capture metadata"
                                            .to_string(),
                                    );
                                }
                                let capture_vals = arm
                                    .capture_vars
                                    .iter()
                                    .map(|var| {
                                        env.get(var).copied().ok_or_else(|| {
                                            format!(
                                                "missing backend handle-arm capture var {:?}",
                                                var
                                            )
                                        })
                                    })
                                    .collect::<Result<Vec<_>, _>>()?;
                                let (_, captures_ptr) =
                                    emit_tagged_stack_array(&mut builder, &capture_vals);
                                let ncaptures =
                                    builder.ins().iconst(PTR_TYPE, capture_vals.len() as i64);
                                let make_closure = get_helper_func_ref(
                                    module,
                                    helpers,
                                    &mut builder,
                                    "rt_make_jit_closure",
                                );
                                let fn_idx =
                                    builder.ins().iconst(PTR_TYPE, meta.function_index as i64);
                                let call = builder.ins().call(
                                    make_closure,
                                    &[ctx_val, fn_idx, captures_ptr, ncaptures],
                                );
                                closure_vals.push(builder.inst_results(call)[0]);
                            }
                            let ops_slot = builder.create_sized_stack_slot(StackSlotData::new(
                                cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                                (op_sym_vals.len().max(1) as u32) * 8,
                                3,
                            ));
                            for (i, op) in op_sym_vals.iter().enumerate() {
                                builder.ins().stack_store(*op, ops_slot, (i * 8) as i32);
                            }
                            let ops_ptr = builder.ins().stack_addr(PTR_TYPE, ops_slot, 0);
                            let (_, closures_ptr) =
                                emit_boxed_stack_array(&mut builder, &closure_vals);
                            let effect_val = builder.ins().iconst(PTR_TYPE, effect.as_u32() as i64);
                            let narms_val = builder.ins().iconst(PTR_TYPE, arms.len() as i64);
                            let rt_push_handler = get_helper_func_ref(
                                module,
                                helpers,
                                &mut builder,
                                "rt_push_handler",
                            );
                            builder.ins().call(
                                rt_push_handler,
                                &[ctx_val, effect_val, ops_ptr, closures_ptr, narms_val],
                            );
                        }
                    }
                }

                match &block.terminator {
                    BackendIrTerminator::Return(var, _) => {
                        let value = env
                            .get(var)
                            .copied()
                            .ok_or_else(|| format!("missing backend return var {:?}", var))?;
                        let value_ptr =
                            box_and_guard_jit_value(module, helpers, &mut builder, ctx_val, value);
                        let result_ptr = if meta.has_contract {
                            let fn_index =
                                builder.ins().iconst(PTR_TYPE, meta.function_index as i64);
                            let zero = builder.ins().iconst(PTR_TYPE, 0);
                            let check_ret = get_helper_func_ref(
                                module,
                                helpers,
                                &mut builder,
                                "rt_check_jit_contract_return",
                            );
                            let checked_ret_call = builder.ins().call(
                                check_ret,
                                &[ctx_val, fn_index, value_ptr, zero, zero, zero, zero],
                            );
                            let checked_ret = builder.inst_results(checked_ret_call)[0];
                            emit_return_on_null_value(&mut builder, checked_ret);
                            checked_ret
                        } else {
                            value_ptr
                        };
                        let tag = builder.ins().iconst(types::I64, JIT_TAG_PTR);
                        builder.ins().return_(&[tag, result_ptr]);
                    }
                    BackendIrTerminator::Jump(target, args, _) => {
                        let target_block = block_map[target];
                        if let Some(target_def) = block_defs.get(target).copied() {
                            let target_env = block_envs.entry(*target).or_default();
                            target_env.extend(env.iter().map(|(var, value)| (*var, *value)));
                            let target_module_env = block_module_envs.entry(*target).or_default();
                            target_module_env.extend(
                                module_env
                                    .iter()
                                    .map(|(var, module_name)| (*var, *module_name)),
                            );
                            let target_function_env =
                                block_function_envs.entry(*target).or_default();
                            target_function_env
                                .extend(function_env.iter().map(|(var, meta)| (*var, *meta)));
                            for (param, arg) in target_def.params.iter().zip(args.iter()) {
                                if let Some(value) = env.get(arg).copied() {
                                    target_env.insert(param.var, value);
                                }
                                if let Some(module_name) = module_env.get(arg).copied() {
                                    target_module_env.insert(param.var, module_name);
                                } else {
                                    target_module_env.remove(&param.var);
                                }
                                if let Some(meta) = function_env.get(arg).copied() {
                                    target_function_env.insert(param.var, meta);
                                } else {
                                    target_function_env.remove(&param.var);
                                }
                            }
                        }
                        let block_args = args
                            .iter()
                            .map(|arg| {
                                env.get(arg)
                                    .copied()
                                    .ok_or_else(|| format!("missing backend jump var {:?}", arg))
                                    .map(|value| {
                                        BlockArg::Value(box_and_guard_jit_value(
                                            module,
                                            helpers,
                                            &mut builder,
                                            ctx_val,
                                            value,
                                        ))
                                    })
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        builder.ins().jump(target_block, &block_args);
                    }
                    BackendIrTerminator::Branch {
                        cond,
                        then_block,
                        else_block,
                        ..
                    } => {
                        block_envs
                            .entry(*then_block)
                            .or_default()
                            .extend(env.iter().map(|(var, value)| (*var, *value)));
                        block_module_envs.entry(*then_block).or_default().extend(
                            module_env
                                .iter()
                                .map(|(var, module_name)| (*var, *module_name)),
                        );
                        block_function_envs
                            .entry(*then_block)
                            .or_default()
                            .extend(function_env.iter().map(|(var, meta)| (*var, *meta)));
                        block_envs
                            .entry(*else_block)
                            .or_default()
                            .extend(env.iter().map(|(var, value)| (*var, *value)));
                        block_module_envs.entry(*else_block).or_default().extend(
                            module_env
                                .iter()
                                .map(|(var, module_name)| (*var, *module_name)),
                        );
                        block_function_envs
                            .entry(*else_block)
                            .or_default()
                            .extend(function_env.iter().map(|(var, meta)| (*var, *meta)));
                        let cond_value = env
                            .get(cond)
                            .copied()
                            .ok_or_else(|| format!("missing backend branch var {:?}", cond))?;
                        let cond_bool = compile_simple_backend_ir_truthiness_condition(
                            module,
                            helpers,
                            &mut builder,
                            ctx_val,
                            cond_value,
                        );
                        builder.ins().brif(
                            cond_bool,
                            block_map[then_block],
                            &[],
                            block_map[else_block],
                            &[],
                        );
                    }
                    BackendIrTerminator::TailCall {
                        callee,
                        args,
                        metadata,
                    } => {
                        let arg_vals = args
                            .iter()
                            .map(|arg| {
                                env.get(arg).copied().ok_or_else(|| {
                                    format!("missing backend tailcall var {:?}", arg)
                                })
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        match callee {
                            IrCallTarget::Direct(function_id) => {
                                let callee = backend_function_metas
                                    .get(function_id)
                                    .copied()
                                    .ok_or_else(|| {
                                        "missing direct backend tail callee metadata".to_string()
                                    })?;
                                emit_jit_cfg_user_function_tailcall(
                                    module,
                                    helpers,
                                    &mut builder,
                                    ctx_val,
                                    callee,
                                    &arg_vals,
                                );
                            }
                            IrCallTarget::Named(name) => {
                                if let Some(callee) = scope.functions.get(name).copied() {
                                    emit_jit_cfg_user_function_tailcall(
                                        module,
                                        helpers,
                                        &mut builder,
                                        ctx_val,
                                        callee,
                                        &arg_vals,
                                    );
                                } else if let Some(primop) =
                                    resolve_primop_call(interner.resolve(*name), arg_vals.len())
                                {
                                    let value = compile_jit_cfg_primop_call(
                                        module,
                                        helpers,
                                        &mut builder,
                                        ctx_val,
                                        primop,
                                        &arg_vals,
                                        metadata.span.unwrap_or(function.body_span),
                                    )?;
                                    let (tag, payload) =
                                        jit_value_to_tag_payload(&mut builder, value);
                                    builder.ins().return_(&[tag, payload]);
                                } else if let Some(&base_idx) = scope.base_functions.get(name) {
                                    let boxed_args = arg_vals
                                        .iter()
                                        .map(|value| {
                                            box_and_guard_jit_value(
                                                module,
                                                helpers,
                                                &mut builder,
                                                ctx_val,
                                                *value,
                                            )
                                        })
                                        .collect::<Vec<_>>();
                                    let value = compile_jit_cfg_base_function_call(
                                        module,
                                        helpers,
                                        &mut builder,
                                        ctx_val,
                                        base_idx,
                                        &boxed_args,
                                        metadata.span.unwrap_or(function.body_span),
                                    )?;
                                    let (tag, payload) =
                                        jit_value_to_tag_payload(&mut builder, value);
                                    builder.ins().return_(&[tag, payload]);
                                } else if let Some(&global_idx) = scope.globals.get(name) {
                                    let get_global = get_helper_func_ref(
                                        module,
                                        helpers,
                                        &mut builder,
                                        "rt_get_global",
                                    );
                                    let idx_val = builder.ins().iconst(PTR_TYPE, global_idx as i64);
                                    let call = builder.ins().call(get_global, &[ctx_val, idx_val]);
                                    let callee = JitValue::boxed(builder.inst_results(call)[0]);
                                    let value = compile_jit_cfg_generic_call(
                                        module,
                                        helpers,
                                        &mut builder,
                                        ctx_val,
                                        callee,
                                        &arg_vals,
                                        metadata.span.unwrap_or(function.body_span),
                                    )?;
                                    let (tag, payload) =
                                        jit_value_to_tag_payload(&mut builder, value);
                                    builder.ins().return_(&[tag, payload]);
                                } else if let Some(&arity) = scope.adt_constructors.get(name) {
                                    if arity != arg_vals.len() {
                                        return Err(format!(
                                            "backend named constructor arity mismatch for {}",
                                            interner.resolve(*name)
                                        ));
                                    }
                                    let value = compile_backend_named_adt_constructor_call(
                                        module,
                                        helpers,
                                        &mut builder,
                                        ctx_val,
                                        *name,
                                        &arg_vals,
                                        interner,
                                    )?;
                                    let (tag, payload) =
                                        jit_value_to_tag_payload(&mut builder, value);
                                    builder.ins().return_(&[tag, payload]);
                                } else {
                                    return Err(format!(
                                        "missing named backend tail callee metadata for {}",
                                        interner.resolve(*name)
                                    ));
                                }
                            }
                            IrCallTarget::Var(var) => {
                                if let Some(callee) = function_env.get(var).copied() {
                                    emit_jit_cfg_user_function_tailcall(
                                        module,
                                        helpers,
                                        &mut builder,
                                        ctx_val,
                                        callee,
                                        &arg_vals,
                                    );
                                } else {
                                    let callee = env.get(var).copied().ok_or_else(|| {
                                        format!(
                                            "missing backend indirect tail callee var {:?}",
                                            var
                                        )
                                    })?;
                                    let value = compile_jit_cfg_generic_call(
                                        module,
                                        helpers,
                                        &mut builder,
                                        ctx_val,
                                        callee,
                                        &arg_vals,
                                        metadata.span.unwrap_or(function.body_span),
                                    )?;
                                    let (tag, payload) =
                                        jit_value_to_tag_payload(&mut builder, value);
                                    builder.ins().return_(&[tag, payload]);
                                }
                            }
                        }
                    }
                    BackendIrTerminator::Unreachable(_) => {
                        builder.ins().trap(TrapCode::INTEGER_OVERFLOW);
                    }
                }
            }

            builder.seal_all_blocks();
            builder.finalize();
        }
        let mut ctx = cranelift_codegen::Context::new();
        ctx.func = func;
        self.module
            .define_function(meta.id, &mut ctx)
            .map_err(|e| format!("define backend function: {e:?}"))?;
        Ok(())
    }
}

pub(super) struct FunctionCompiler {
    boxed_array_slot: Option<StackSlot>,
    tagged_array_slot: Option<StackSlot>,
}

#[allow(dead_code)]
impl FunctionCompiler {
    pub(super) fn new(
        builder: &mut FunctionBuilder,
        boxed_array_capacity: usize,
        tagged_array_capacity: usize,
    ) -> Self {
        let boxed_array_slot = (boxed_array_capacity > 0).then(|| {
            builder.create_sized_stack_slot(StackSlotData::new(
                cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                boxed_array_capacity as u32 * 8,
                3,
            ))
        });
        let tagged_array_slot = (tagged_array_capacity > 0).then(|| {
            builder.create_sized_stack_slot(StackSlotData::new(
                cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                tagged_array_capacity as u32 * 16,
                3,
            ))
        });
        Self {
            boxed_array_slot,
            tagged_array_slot,
        }
    }

    pub(super) fn emit_boxed_array(
        &self,
        builder: &mut FunctionBuilder,
        values: &[CraneliftValue],
    ) -> CraneliftValue {
        // Lazily allocate the slot if the capacity calculation missed this call site.
        let slot = match self.boxed_array_slot {
            Some(s) => s,
            None => {
                let capacity = values.len().max(4);
                builder.create_sized_stack_slot(StackSlotData::new(
                    cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                    capacity as u32 * 8,
                    3,
                ))
            }
        };
        for (i, value) in values.iter().enumerate() {
            builder.ins().stack_store(*value, slot, (i * 8) as i32);
        }
        builder.ins().stack_addr(PTR_TYPE, slot, 0)
    }

    pub(super) fn emit_tagged_array(
        &self,
        builder: &mut FunctionBuilder,
        values: &[JitValue],
    ) -> CraneliftValue {
        let slot = match self.tagged_array_slot {
            Some(s) => s,
            None => {
                let capacity = values.len().max(4);
                builder.create_sized_stack_slot(StackSlotData::new(
                    cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                    capacity as u32 * 16,
                    3,
                ))
            }
        };
        for (i, value) in values.iter().enumerate() {
            let (tag, payload) = jit_value_to_tag_payload(builder, *value);
            builder.ins().stack_store(tag, slot, (i * 16) as i32);
            builder
                .ins()
                .stack_store(payload, slot, (i * 16 + 8) as i32);
        }
        builder.ins().stack_addr(PTR_TYPE, slot, 0)
    }
}

#[allow(dead_code)]
pub(super) fn note_boxed_array_usage(current_max: &mut usize, len: usize) {
    *current_max = (*current_max).max(len.max(1));
}
