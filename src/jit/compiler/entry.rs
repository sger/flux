use super::*;

impl JitCompiler {
    fn compile_identity_function(&mut self) -> Result<usize, String> {
        let sig = self.user_function_signature(JitCallAbi::from_arity(1));
        let func_id = self
            .module
            .declare_function("__flux_identity", cranelift_module::Linkage::Local, &sig)
            .map_err(|e| format!("declare __flux_identity: {}", e))?;

        let mut func = Function::with_name_signature(UserFuncName::default(), sig);
        {
            let mut builder = FunctionBuilder::new(&mut func, &mut self.builder_ctx);
            let entry = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            builder.seal_block(entry);

            let entry_params = builder.block_params(entry).to_vec();
            let tag = entry_params[1];
            let payload = entry_params[2];
            builder.ins().return_(&[tag, payload]);
            builder.finalize();
        }

        let mut ctx = cranelift_codegen::Context::new();
        ctx.func = func;
        self.module
            .define_function(func_id, &mut ctx)
            .map_err(|e| format!("define __flux_identity: {}", e))?;

        let function_index = self.jit_functions.len();
        self.jit_functions.push(JitFunctionCompileEntry {
            id: func_id,
            num_params: 1,
            call_abi: JitCallAbi::Reg1,
            contract: None,
            return_span: None,
        });
        Ok(function_index)
    }

    pub(super) fn compile_simple_backend_ir_program(
        &mut self,
        ir_program: &IrProgram,
        interner: &Interner,
    ) -> Result<FuncId, String> {
        let mut scope = Scope::new(Rc::clone(&self.hm_expr_types));
        register_base_functions(&mut scope, interner);
        let mut backend_function_metas = HashMap::new();
        let backend_function_defs: HashMap<BackendFunctionId, &BackendIrFunction> =
            ir_program.functions().iter().map(|f| (f.id, f)).collect();
        for (idx, name) in ir_program.globals().iter().enumerate() {
            scope.globals.insert(*name, idx);
        }
        let mut imported_modules = HashSet::new();
        let mut import_aliases = HashMap::new();
        let mut adt_constructors = HashMap::new();
        let global_binding_indices: HashMap<BackendIrVar, usize> = ir_program
            .global_bindings()
            .iter()
            .filter_map(|binding| {
                scope
                    .globals
                    .get(&binding.name)
                    .copied()
                    .map(|idx| (binding.var, idx))
            })
            .collect();

        for function in ir_program.functions() {
            let explicit_arity = function
                .params
                .len()
                .saturating_sub(function.captures.len());
            let call_abi = JitCallAbi::from_arity(explicit_arity);
            let sig = self.user_function_signature(call_abi);
            let function_name = function
                .name
                .map(|name| format!("flux_backend_{}", interner.resolve(name)))
                .unwrap_or_else(|| format!("flux_backend_fn{}", function.id.0));
            let id = self
                .module
                .declare_function(&function_name, Linkage::Local, &sig)
                .map_err(|e| format!("declare {}: {}", function_name, e))?;
            let function_index = self.jit_functions.len();
            let contract = crate::runtime::function_contract::runtime_contract_from_annotations(
                &function.parameter_types,
                &function.return_type_annotation,
                &function.effects,
                interner,
            );
            let has_contract = contract.is_some();
            self.jit_functions.push(JitFunctionCompileEntry {
                id,
                num_params: explicit_arity,
                call_abi,
                contract,
                return_span: function
                    .return_type_annotation
                    .as_ref()
                    .map(|_| function.body_span),
            });
            let meta = JitFunctionMeta {
                id,
                call_abi,
                function_index,
                has_contract,
            };
            backend_function_metas.insert(function.id, meta);
            if let Some(name) = function.name {
                scope.functions.insert(name, meta);
            }
        }
        collect_backend_top_level_declaration_metadata(
            ir_program.top_level_items(),
            &mut imported_modules,
            &mut import_aliases,
            &mut adt_constructors,
        );
        scope.imported_modules.extend(imported_modules);
        scope.import_aliases.extend(import_aliases);
        scope.adt_constructors.extend(adt_constructors);
        register_backend_top_level_named_functions(
            ir_program.top_level_items(),
            &backend_function_metas,
            &mut scope,
        );
        let import_aliases = scope.import_aliases.clone();
        register_backend_top_level_module_functions(
            ir_program.top_level_items(),
            &backend_function_metas,
            &import_aliases,
            &mut scope,
        );

        for function in ir_program.functions() {
            self.compile_simple_backend_ir_function(
                function,
                &scope,
                &backend_function_metas,
                &backend_function_defs,
                &global_binding_indices,
                ir_program.entry(),
                interner,
            )?;
        }
        self.record_named_functions(&scope, interner);

        let entry_function = ir_program
            .functions()
            .iter()
            .find(|function| function.id == ir_program.entry())
            .ok_or_else(|| format!("missing backend entry function {:?}", ir_program.entry()))?;
        let entry_meta = backend_function_metas
            .get(&entry_function.id)
            .copied()
            .ok_or_else(|| "missing backend entry metadata".to_string())?;
        let main_meta = scope
            .functions
            .iter()
            .find_map(|(name, meta)| (interner.resolve(*name) == "main").then_some(*meta));

        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(PTR_TYPE));
        sig.returns.push(AbiParam::new(PTR_TYPE));
        sig.returns.push(AbiParam::new(PTR_TYPE));
        let main_id = self
            .module
            .declare_function("flux_main", Linkage::Export, &sig)
            .map_err(|e| format!("declare flux_main: {}", e))?;
        let mut func = Function::with_name_signature(UserFuncName::default(), sig);
        {
            let module = &mut self.module;
            let helpers = &self.helpers;
            let mut builder = FunctionBuilder::new(&mut func, &mut self.builder_ctx);
            let entry = builder.create_block();
            builder.append_block_params_for_function_params(entry);
            builder.switch_to_block(entry);
            builder.seal_block(entry);
            let ctx_val = builder.block_params(entry)[0];
            let entry_result = compile_jit_cfg_user_function_call(
                module,
                helpers,
                &mut builder,
                ctx_val,
                entry_meta,
                &[],
                entry_function.body_span,
            )?;
            let result = if let Some(main_meta) = main_meta {
                compile_jit_cfg_user_function_call(
                    module,
                    helpers,
                    &mut builder,
                    ctx_val,
                    main_meta,
                    &[],
                    entry_function.body_span,
                )?
            } else {
                entry_result
            };
            let (tag, payload) = jit_value_to_tag_payload(&mut builder, result);
            builder.ins().return_(&[tag, payload]);
            builder.finalize();
        }
        let mut ctx = cranelift_codegen::Context::new();
        ctx.func = func;
        self.module
            .define_function(main_id, &mut ctx)
            .map_err(|e| format!("define flux_main: {}", e))?;
        self.identity_fn_index = self.compile_identity_function()?;
        Ok(main_id)
    }
}
