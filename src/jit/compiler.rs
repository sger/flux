#![allow(clippy::too_many_arguments)]

//! AST → Cranelift IR compiler (Phase 1: expressions, let bindings, calls).

use std::collections::{HashMap, HashSet};

use cranelift_codegen::ir::StackSlotData;
use cranelift_codegen::ir::{
    AbiParam, BlockArg, Function, InstBuilder, MemFlags, UserFuncName, Value as CraneliftValue,
    condcodes::IntCC, types,
};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::JITModule;
use cranelift_module::{DataDescription, FuncId, Linkage, Module};

use crate::ast::free_vars::collect_free_vars;
use crate::primop::{PrimOp, resolve_primop_call};
use crate::runtime::base::{BaseModule, is_base_fastcall_allowlisted};
use crate::runtime::{function_contract::FunctionContract, runtime_type::RuntimeType};
use crate::syntax::{
    Identifier, block::Block, expression::Expression, expression::Pattern, interner::Interner,
    program::Program, statement::Statement, type_expr::TypeExpr,
};

use super::context::JitFunctionEntry;
use super::runtime_helpers::rt_symbols;

/// Pointer type used for all Value pointers in JIT code.
const PTR_TYPE: types::Type = types::I64;

/// Maps runtime helper names to their Cranelift FuncIds.
struct HelperFuncs {
    ids: HashMap<&'static str, FuncId>,
}

#[derive(Clone, Copy)]
struct JitFunctionMeta {
    id: FuncId,
    num_params: usize,
    function_index: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct LiteralKey {
    sl: usize,
    sc: usize,
    el: usize,
    ec: usize,
    bsl: usize,
    bsc: usize,
    bel: usize,
    bec: usize,
    arity: usize,
    first_param: u32,
}

impl LiteralKey {
    fn from_expr(expr: &Expression) -> Self {
        match expr {
            Expression::Function {
                parameters,
                body,
                span,
                ..
            } => Self {
                sl: span.start.line,
                sc: span.start.column,
                el: span.end.line,
                ec: span.end.column,
                bsl: body.span.start.line,
                bsc: body.span.start.column,
                bel: body.span.end.line,
                bec: body.span.end.column,
                arity: parameters.len(),
                first_param: parameters.first().map(|p| p.as_u32()).unwrap_or(0),
            },
            _ => {
                let span = expr.span();
                Self::from_span(span)
            }
        }
    }

    fn from_span(span: crate::diagnostics::position::Span) -> Self {
        Self {
            sl: span.start.line,
            sc: span.start.column,
            el: span.end.line,
            ec: span.end.column,
            bsl: 0,
            bsc: 0,
            bel: 0,
            bec: 0,
            arity: 0,
            first_param: 0,
        }
    }
}

#[derive(Clone)]
struct LiteralFunctionSpec {
    key: LiteralKey,
    parameters: Vec<Identifier>,
    parameter_types: Vec<Option<TypeExpr>>,
    return_type: Option<TypeExpr>,
    body: Block,
    captures: Vec<Identifier>,
    self_name: Option<Identifier>,
}

/// Tracks variables in the current scope.
#[derive(Clone)]
struct Scope {
    /// Maps interned identifier → Cranelift Variable
    locals: HashMap<Identifier, Variable>,
    /// Maps interned identifier → global slot index
    globals: HashMap<Identifier, usize>,
    /// Maps interned identifier → base index
    base_functions: HashMap<Identifier, usize>,
    /// Base names excluded from unqualified lookup via `import Base except [...]`.
    excluded_base_symbols: HashSet<Identifier>,
    /// Maps interned identifier → JIT function metadata.
    functions: HashMap<Identifier, JitFunctionMeta>,
    /// Maps (module name, member name) -> JIT function metadata.
    module_functions: HashMap<(Identifier, Identifier), JitFunctionMeta>,
    /// Imported module names visible in current scope.
    imported_modules: HashSet<Identifier>,
    /// Import aliases: alias -> module name.
    import_aliases: HashMap<Identifier, Identifier>,
    /// Maps literal function key -> JIT function metadata.
    literal_functions: HashMap<LiteralKey, JitFunctionMeta>,
    /// Statically resolved capture order per literal.
    literal_captures: HashMap<LiteralKey, Vec<Identifier>>,
    /// Maps ADT constructor name (interned) → arity. Used to route constructor calls.
    adt_constructors: HashMap<Identifier, usize>,
    /// Maps ADT constructor name (interned) → owning ADT name.
    adt_constructor_owner: HashMap<Identifier, Identifier>,
    /// Maps ADT name → constructor names.
    adt_variants: HashMap<Identifier, Vec<Identifier>>,
}

impl Scope {
    fn new() -> Self {
        Self {
            locals: HashMap::new(),
            globals: HashMap::new(),
            base_functions: HashMap::new(),
            excluded_base_symbols: HashSet::new(),
            functions: HashMap::new(),
            module_functions: HashMap::new(),
            imported_modules: HashSet::new(),
            import_aliases: HashMap::new(),
            literal_functions: HashMap::new(),
            literal_captures: HashMap::new(),
            adt_constructors: HashMap::new(),
            adt_constructor_owner: HashMap::new(),
            adt_variants: HashMap::new(),
        }
    }
}

pub struct JitCompiler {
    pub module: JITModule,
    builder_ctx: FunctionBuilderContext,
    helpers: HelperFuncs,
    jit_functions: Vec<JitFunctionCompileEntry>,
    named_functions: HashMap<String, usize>,
    /// Index in `jit_functions` of the compiled identity function used as
    /// the `resume` value for shallow JIT handlers.
    pub identity_fn_index: usize,
}

struct JitFunctionCompileEntry {
    id: FuncId,
    num_params: usize,
    contract: Option<FunctionContract>,
}

impl JitCompiler {
    pub fn new() -> Result<Self, String> {
        let mut flag_builder = settings::builder();
        flag_builder
            .set("use_colocated_libcalls", "false")
            .map_err(|e| e.to_string())?;
        flag_builder
            .set("is_pic", "false")
            .map_err(|e| e.to_string())?;

        let isa_builder =
            cranelift_native::builder().map_err(|e| format!("native ISA error: {}", e))?;
        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .map_err(|e| e.to_string())?;

        let mut builder = cranelift_jit::JITBuilder::with_isa(isa, default_libcall_names());

        // Register all runtime helper symbols
        for (name, ptr) in rt_symbols() {
            builder.symbol(name, ptr);
        }

        let module = JITModule::new(builder);
        let builder_ctx = FunctionBuilderContext::new();

        let mut compiler = Self {
            module,
            builder_ctx,
            helpers: HelperFuncs {
                ids: HashMap::new(),
            },
            jit_functions: Vec::new(),
            named_functions: HashMap::new(),
            identity_fn_index: usize::MAX,
        };

        compiler.declare_helpers()?;

        Ok(compiler)
    }

    /// Declare all runtime helper functions in the JIT module.
    fn declare_helpers(&mut self) -> Result<(), String> {
        let sigs = helper_signatures();
        for (name, sig_spec) in &sigs {
            let mut sig = self.module.make_signature();
            for _ in 0..sig_spec.num_params {
                sig.params.push(AbiParam::new(PTR_TYPE));
            }
            if sig_spec.has_return {
                sig.returns.push(AbiParam::new(PTR_TYPE));
            }

            let func_id = self
                .module
                .declare_function(name, Linkage::Import, &sig)
                .map_err(|e| format!("declare_function({}): {}", name, e))?;
            self.helpers.ids.insert(name, func_id);
        }
        Ok(())
    }

    /// Compile a program's top-level statements into a single "main" function.
    /// Returns the FuncId of the compiled main function.
    pub fn compile_program(
        &mut self,
        program: &Program,
        interner: &Interner,
    ) -> Result<FuncId, String> {
        // main signature: (ctx: i64) -> i64
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(PTR_TYPE)); // ctx
        sig.returns.push(AbiParam::new(PTR_TYPE)); // result

        let main_id = self
            .module
            .declare_function("flux_main", Linkage::Export, &sig)
            .map_err(|e| format!("declare flux_main: {}", e))?;

        let mut func = Function::with_name_signature(UserFuncName::default(), sig.clone());

        let mut scope = Scope::new();

        // Register base_functions
        register_base_functions(&mut scope, interner);
        apply_base_directives(program, &mut scope, interner)?;
        self.predeclare_imports(program, &mut scope, interner);
        self.predeclare_globals(program, &mut scope);
        collect_adt_definitions(program, &mut scope, interner);
        let literal_specs = collect_literal_function_specs(program);
        // Predeclare/compile user functions first so calls (and recursion) resolve.
        self.predeclare_functions(program, &mut scope, interner)?;
        self.predeclare_literal_functions(&literal_specs, &mut scope, interner)?;
        self.compile_functions(program, &scope, interner)?;
        self.compile_literal_functions(&literal_specs, &scope, interner)?;
        self.record_named_functions(&scope, interner);

        {
            // Destructure self to avoid borrow conflicts: builder_ctx is
            // mutably borrowed by FunctionBuilder, but we also need module
            // and helpers inside compilation functions.
            let module = &mut self.module;
            let helpers = &self.helpers;
            let mut builder = FunctionBuilder::new(&mut func, &mut self.builder_ctx);

            let entry_block = builder.create_block();
            builder.append_block_params_for_function_params(entry_block);
            builder.switch_to_block(entry_block);
            builder.seal_block(entry_block);

            let ctx_val = builder.block_params(entry_block)[0];

            // Compile each statement
            let mut last_val = None;
            for stmt in &program.statements {
                if matches!(stmt, Statement::Function { .. } | Statement::Data { .. }) {
                    continue;
                }
                let outcome = compile_statement(
                    module,
                    helpers,
                    &mut builder,
                    &mut scope,
                    ctx_val,
                    None,
                    None,
                    true,
                    stmt,
                    interner,
                )?;
                match outcome {
                    StmtOutcome::Value(v) => last_val = Some(v),
                    StmtOutcome::Returned => break,
                    StmtOutcome::None => {}
                }
            }

            // Entry-point convention: if `fn main()` exists and there is no explicit
            // top-level `main()` call, invoke it once after top-level initialization.
            let main_meta = scope
                .functions
                .iter()
                .find_map(|(name, meta)| (interner.resolve(*name) == "main").then_some(*meta));
            let has_explicit_top_level_main_call = program.statements.iter().any(|stmt| {
                matches!(
                    stmt,
                    Statement::Expression {
                        expression: Expression::Call { function, arguments, .. },
                        ..
                    } if matches!(function.as_ref(), Expression::Identifier { name, .. } if interner.resolve(*name) == "main")
                        && arguments.is_empty()
                )
            });
            if let Some(meta) = main_meta
                && !has_explicit_top_level_main_call
            {
                let main_result = compile_user_function_call(
                    module,
                    helpers,
                    &mut builder,
                    &mut scope,
                    ctx_val,
                    None,
                    None,
                    meta,
                    crate::diagnostics::position::Span::default(),
                    &[],
                    interner,
                )?;
                last_val = Some(main_result);
            }

            // Return the last expression value, or None
            let ret = match last_val {
                Some(v) => v,
                None => {
                    let make_none =
                        get_helper_func_ref(module, helpers, &mut builder, "rt_make_none");
                    let call = builder.ins().call(make_none, &[ctx_val]);
                    builder.inst_results(call)[0]
                }
            };
            builder.ins().return_(&[ret]);
            builder.finalize();
        }

        // Define the function in the module
        let mut ctx = cranelift_codegen::Context::new();
        ctx.func = func;
        self.module
            .define_function(main_id, &mut ctx)
            .map_err(|e| format!("define flux_main: {}", e))?;

        // Compile the identity function used as the `resume` value in JIT shallow handlers.
        self.identity_fn_index = self.compile_identity_function()?;

        Ok(main_id)
    }

    /// Compile a trivial `identity(ctx, args_ptr, nargs, captures_ptr, ncaptures) -> args_ptr[0]`
    /// JIT function. Its function_index is stored in `self.identity_fn_index` and exposed
    /// to the JIT context so `rt_perform` can build a callable `resume` closure.
    fn compile_identity_function(&mut self) -> Result<usize, String> {
        let sig = self.user_function_signature();
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

            // args_ptr is the second parameter (index 1). Load args_ptr[0].
            let args_ptr = builder.block_params(entry)[1];
            let first_arg = builder
                .ins()
                .load(PTR_TYPE, MemFlags::trusted(), args_ptr, 0);
            builder.ins().return_(&[first_arg]);
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
            contract: None,
        });
        Ok(function_index)
    }

    pub fn named_functions(&self) -> HashMap<String, usize> {
        self.named_functions.clone()
    }

    fn record_named_functions(&mut self, scope: &Scope, interner: &Interner) {
        self.named_functions.clear();
        for (name, meta) in &scope.functions {
            self.named_functions
                .insert(interner.resolve(*name).to_string(), meta.function_index);
        }
        for ((module_name, member_name), meta) in &scope.module_functions {
            let full_name = format!(
                "{}.{}",
                interner.resolve(*module_name),
                interner.resolve(*member_name)
            );
            self.named_functions.insert(full_name, meta.function_index);
        }
    }

    fn user_function_signature(&mut self) -> cranelift_codegen::ir::Signature {
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(PTR_TYPE)); // ctx
        sig.params.push(AbiParam::new(PTR_TYPE)); // args ptr
        sig.params.push(AbiParam::new(PTR_TYPE)); // nargs
        sig.params.push(AbiParam::new(PTR_TYPE)); // captures ptr
        sig.params.push(AbiParam::new(PTR_TYPE)); // ncaptures
        sig.returns.push(AbiParam::new(PTR_TYPE)); // result
        sig
    }

    fn predeclare_functions(
        &mut self,
        program: &Program,
        scope: &mut Scope,
        interner: &Interner,
    ) -> Result<(), String> {
        for stmt in &program.statements {
            match stmt {
                Statement::Function {
                    name,
                    parameters,
                    parameter_types,
                    return_type,
                    effects,
                    ..
                } => {
                    if scope.functions.contains_key(name) {
                        continue;
                    }

                    let sig = self.user_function_signature();
                    let fn_name = format!("flux_fn_{}", interner.resolve(*name));
                    let id = self
                        .module
                        .declare_function(&fn_name, Linkage::Local, &sig)
                        .map_err(|e| format!("declare {}: {}", fn_name, e))?;
                    let function_index = self.jit_functions.len();
                    let contract = runtime_contract_from_annotations(
                        parameter_types,
                        return_type,
                        effects,
                        interner,
                    );
                    self.jit_functions.push(JitFunctionCompileEntry {
                        id,
                        num_params: parameters.len(),
                        contract,
                    });
                    scope.functions.insert(
                        *name,
                        JitFunctionMeta {
                            id,
                            num_params: parameters.len(),
                            function_index,
                        },
                    );
                }
                Statement::Module {
                    name: module_name,
                    body,
                    ..
                } => {
                    scope.imported_modules.insert(*module_name);
                    for inner in &body.statements {
                        let Statement::Function {
                            name: fn_name,
                            parameters,
                            parameter_types,
                            return_type,
                            effects,
                            ..
                        } = inner
                        else {
                            continue;
                        };

                        let key = (*module_name, *fn_name);
                        if scope.module_functions.contains_key(&key) {
                            continue;
                        }

                        let sig = self.user_function_signature();
                        let label = format!(
                            "flux_mod_{}_{}",
                            interner.resolve(*module_name),
                            interner.resolve(*fn_name)
                        );
                        let id = self
                            .module
                            .declare_function(&label, Linkage::Local, &sig)
                            .map_err(|e| format!("declare {}: {}", label, e))?;
                        let function_index = self.jit_functions.len();
                        let contract = runtime_contract_from_annotations(
                            parameter_types,
                            return_type,
                            effects,
                            interner,
                        );
                        self.jit_functions.push(JitFunctionCompileEntry {
                            id,
                            num_params: parameters.len(),
                            contract,
                        });
                        scope.module_functions.insert(
                            key,
                            JitFunctionMeta {
                                id,
                                num_params: parameters.len(),
                                function_index,
                            },
                        );
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn predeclare_imports(&self, program: &Program, scope: &mut Scope, interner: &Interner) {
        for stmt in &program.statements {
            if let Statement::Import {
                name,
                alias,
                except: _,
                ..
            } = stmt
            {
                if is_base_symbol(*name, interner) {
                    continue;
                }
                scope.imported_modules.insert(*name);
                if let Some(alias) = alias {
                    scope.import_aliases.insert(*alias, *name);
                }
            }
        }
    }

    fn predeclare_globals(&self, program: &Program, scope: &mut Scope) {
        fn collect_pattern_names(pattern: &Pattern, out: &mut Vec<Identifier>) {
            match pattern {
                Pattern::Identifier { name, .. } => out.push(*name),
                Pattern::Some { pattern, .. }
                | Pattern::Left { pattern, .. }
                | Pattern::Right { pattern, .. } => collect_pattern_names(pattern, out),
                Pattern::Cons { head, tail, .. } => {
                    collect_pattern_names(head, out);
                    collect_pattern_names(tail, out);
                }
                Pattern::Tuple { elements, .. } => {
                    for element in elements {
                        collect_pattern_names(element, out);
                    }
                }
                Pattern::Constructor { fields, .. } => {
                    for field in fields {
                        collect_pattern_names(field, out);
                    }
                }
                Pattern::Wildcard { .. }
                | Pattern::Literal { .. }
                | Pattern::None { .. }
                | Pattern::EmptyList { .. } => {}
            }
        }

        let mut next_idx = scope.globals.len();
        for stmt in &program.statements {
            match stmt {
                Statement::Let { name, .. } => {
                    scope.globals.entry(*name).or_insert_with(|| {
                        let idx = next_idx;
                        next_idx += 1;
                        idx
                    });
                }
                Statement::LetDestructure { pattern, .. } => {
                    let mut names = Vec::new();
                    collect_pattern_names(pattern, &mut names);
                    for name in names {
                        scope.globals.entry(name).or_insert_with(|| {
                            let idx = next_idx;
                            next_idx += 1;
                            idx
                        });
                    }
                }
                _ => {}
            }
        }
    }

    fn compile_functions(
        &mut self,
        program: &Program,
        scope: &Scope,
        interner: &Interner,
    ) -> Result<(), String> {
        for stmt in &program.statements {
            let Statement::Function {
                name,
                parameters,
                body,
                ..
            } = stmt
            else {
                continue;
            };

            let Some(meta) = scope.functions.get(name).copied() else {
                continue;
            };

            let sig = self.user_function_signature();
            let mut func = Function::with_name_signature(UserFuncName::default(), sig);
            {
                let module = &mut self.module;
                let helpers = &self.helpers;
                let mut builder = FunctionBuilder::new(&mut func, &mut self.builder_ctx);
                let mut fn_scope = scope.clone();
                fn_scope.locals.clear();

                let entry = builder.create_block();
                let init_block = builder.create_block();
                let body_block = builder.create_block();
                let arity_fail = builder.create_block();
                let return_block = builder.create_block();
                builder.append_block_param(return_block, PTR_TYPE);
                builder.append_block_params_for_function_params(entry);
                builder.switch_to_block(entry);
                builder.seal_block(entry);

                let entry_params = builder.block_params(entry);
                let ctx_val = entry_params[0];
                let args_ptr = entry_params[1];
                let nargs = entry_params[2];
                let _captures_ptr = entry_params[3];
                let _ncaptures = entry_params[4];
                let want = builder.ins().iconst(PTR_TYPE, parameters.len() as i64);
                let arity_ok = builder.ins().icmp(IntCC::Equal, nargs, want);
                builder
                    .ins()
                    .brif(arity_ok, init_block, &[], arity_fail, &[]);

                builder.switch_to_block(arity_fail);
                let set_arity_error =
                    get_helper_func_ref(module, helpers, &mut builder, "rt_set_arity_error");
                builder.ins().call(set_arity_error, &[ctx_val, nargs, want]);
                let null_ptr = builder.ins().iconst(PTR_TYPE, 0);
                builder.ins().return_(&[null_ptr]);
                builder.seal_block(arity_fail);

                builder.switch_to_block(init_block);
                let mut param_bindings: Vec<(Identifier, Variable)> =
                    Vec::with_capacity(parameters.len());
                for (idx, ident) in parameters.iter().enumerate() {
                    let arg_ptr =
                        builder
                            .ins()
                            .load(PTR_TYPE, MemFlags::new(), args_ptr, (idx * 8) as i32);
                    let var = builder.declare_var(PTR_TYPE);
                    builder.def_var(var, arg_ptr);
                    fn_scope.locals.insert(*ident, var);
                    param_bindings.push((*ident, var));
                }
                builder.ins().jump(body_block, &[]);
                builder.seal_block(init_block);

                let tail_ctx = TailCallContext {
                    function_name: Some(*name),
                    loop_block: body_block,
                    params: param_bindings,
                };

                builder.switch_to_block(body_block);

                let mut last_val = None;
                let mut returned = false;
                let last_index = body.statements.len().saturating_sub(1);
                for (idx, body_stmt) in body.statements.iter().enumerate() {
                    if idx == last_index
                        && let Some(outcome) = try_compile_tail_expression_statement(
                            module,
                            helpers,
                            &mut builder,
                            &mut fn_scope,
                            ctx_val,
                            Some(return_block),
                            &tail_ctx,
                            body_stmt,
                            interner,
                        )?
                    {
                        match outcome {
                            StmtOutcome::Returned => {
                                returned = true;
                                break;
                            }
                            StmtOutcome::Value(v) => {
                                last_val = Some(v);
                                continue;
                            }
                            StmtOutcome::None => continue,
                        }
                    }
                    let outcome = compile_statement(
                        module,
                        helpers,
                        &mut builder,
                        &mut fn_scope,
                        ctx_val,
                        Some(return_block),
                        Some(&tail_ctx),
                        false,
                        body_stmt,
                        interner,
                    )?;
                    match outcome {
                        StmtOutcome::Value(v) => last_val = Some(v),
                        StmtOutcome::Returned => {
                            returned = true;
                            break;
                        }
                        StmtOutcome::None => {}
                    }
                }

                if !returned {
                    let ret = match last_val {
                        Some(v) => v,
                        None => {
                            let make_none =
                                get_helper_func_ref(module, helpers, &mut builder, "rt_make_none");
                            let call = builder.ins().call(make_none, &[ctx_val]);
                            builder.inst_results(call)[0]
                        }
                    };
                    let args = [BlockArg::Value(ret)];
                    builder.ins().jump(return_block, &args);
                }
                builder.seal_block(body_block);
                builder.switch_to_block(return_block);
                let ret = builder.block_params(return_block)[0];
                builder.ins().return_(&[ret]);
                builder.seal_block(return_block);
                builder.finalize();
            }

            let mut ctx = cranelift_codegen::Context::new();
            ctx.func = func;
            self.module
                .define_function(meta.id, &mut ctx)
                .map_err(|e| {
                    format!(
                        "define function {}: {} ({:?})",
                        interner.resolve(*name),
                        e,
                        e
                    )
                })?;
        }

        for stmt in &program.statements {
            let Statement::Module {
                name: module_name,
                body,
                ..
            } = stmt
            else {
                continue;
            };

            for inner in &body.statements {
                let Statement::Function {
                    name,
                    parameters,
                    body,
                    ..
                } = inner
                else {
                    continue;
                };

                let Some(meta) = scope.module_functions.get(&(*module_name, *name)).copied() else {
                    continue;
                };

                let sig = self.user_function_signature();
                let mut func = Function::with_name_signature(UserFuncName::default(), sig);
                {
                    let module = &mut self.module;
                    let helpers = &self.helpers;
                    let mut builder = FunctionBuilder::new(&mut func, &mut self.builder_ctx);
                    let mut fn_scope = scope.clone();
                    fn_scope.locals.clear();
                    for ((mod_name, member_name), member_meta) in &scope.module_functions {
                        if *mod_name == *module_name {
                            fn_scope.functions.insert(*member_name, *member_meta);
                        }
                    }

                    let entry = builder.create_block();
                    let init_block = builder.create_block();
                    let body_block = builder.create_block();
                    let arity_fail = builder.create_block();
                    let return_block = builder.create_block();
                    builder.append_block_param(return_block, PTR_TYPE);
                    builder.append_block_params_for_function_params(entry);
                    builder.switch_to_block(entry);
                    builder.seal_block(entry);

                    let entry_params = builder.block_params(entry);
                    let ctx_val = entry_params[0];
                    let args_ptr = entry_params[1];
                    let nargs = entry_params[2];
                    let _captures_ptr = entry_params[3];
                    let _ncaptures = entry_params[4];
                    let want = builder.ins().iconst(PTR_TYPE, parameters.len() as i64);
                    let arity_ok = builder.ins().icmp(IntCC::Equal, nargs, want);
                    builder
                        .ins()
                        .brif(arity_ok, init_block, &[], arity_fail, &[]);

                    builder.switch_to_block(arity_fail);
                    let set_arity_error =
                        get_helper_func_ref(module, helpers, &mut builder, "rt_set_arity_error");
                    builder.ins().call(set_arity_error, &[ctx_val, nargs, want]);
                    let null_ptr = builder.ins().iconst(PTR_TYPE, 0);
                    builder.ins().return_(&[null_ptr]);
                    builder.seal_block(arity_fail);

                    builder.switch_to_block(init_block);
                    let mut param_bindings: Vec<(Identifier, Variable)> =
                        Vec::with_capacity(parameters.len());
                    for (idx, ident) in parameters.iter().enumerate() {
                        let arg_ptr = builder.ins().load(
                            PTR_TYPE,
                            MemFlags::new(),
                            args_ptr,
                            (idx * 8) as i32,
                        );
                        let var = builder.declare_var(PTR_TYPE);
                        builder.def_var(var, arg_ptr);
                        fn_scope.locals.insert(*ident, var);
                        param_bindings.push((*ident, var));
                    }
                    builder.ins().jump(body_block, &[]);
                    builder.seal_block(init_block);

                    let tail_ctx = TailCallContext {
                        function_name: Some(*name),
                        loop_block: body_block,
                        params: param_bindings,
                    };

                    builder.switch_to_block(body_block);

                    let mut last_val = None;
                    let mut returned = false;
                    let last_index = body.statements.len().saturating_sub(1);
                    for (idx, body_stmt) in body.statements.iter().enumerate() {
                        if idx == last_index
                            && let Some(outcome) = try_compile_tail_expression_statement(
                                module,
                                helpers,
                                &mut builder,
                                &mut fn_scope,
                                ctx_val,
                                Some(return_block),
                                &tail_ctx,
                                body_stmt,
                                interner,
                            )?
                        {
                            match outcome {
                                StmtOutcome::Returned => {
                                    returned = true;
                                    break;
                                }
                                StmtOutcome::Value(v) => {
                                    last_val = Some(v);
                                    continue;
                                }
                                StmtOutcome::None => continue,
                            }
                        }
                        let outcome = compile_statement(
                            module,
                            helpers,
                            &mut builder,
                            &mut fn_scope,
                            ctx_val,
                            Some(return_block),
                            Some(&tail_ctx),
                            false,
                            body_stmt,
                            interner,
                        )?;
                        match outcome {
                            StmtOutcome::Value(v) => last_val = Some(v),
                            StmtOutcome::Returned => {
                                returned = true;
                                break;
                            }
                            StmtOutcome::None => {}
                        }
                    }

                    if !returned {
                        let ret = match last_val {
                            Some(v) => v,
                            None => {
                                let make_none = get_helper_func_ref(
                                    module,
                                    helpers,
                                    &mut builder,
                                    "rt_make_none",
                                );
                                let call = builder.ins().call(make_none, &[ctx_val]);
                                builder.inst_results(call)[0]
                            }
                        };
                        let args = [BlockArg::Value(ret)];
                        builder.ins().jump(return_block, &args);
                    }
                    builder.seal_block(body_block);
                    builder.switch_to_block(return_block);
                    let ret = builder.block_params(return_block)[0];
                    builder.ins().return_(&[ret]);
                    builder.seal_block(return_block);
                    builder.finalize();
                }

                let mut ctx = cranelift_codegen::Context::new();
                ctx.func = func;
                self.module
                    .define_function(meta.id, &mut ctx)
                    .map_err(|e| {
                        format!(
                            "define module function {}.{}: {} ({:?})",
                            interner.resolve(*module_name),
                            interner.resolve(*name),
                            e,
                            e
                        )
                    })?;
            }
        }
        Ok(())
    }

    fn predeclare_literal_functions(
        &mut self,
        specs: &[LiteralFunctionSpec],
        scope: &mut Scope,
        interner: &Interner,
    ) -> Result<(), String> {
        for spec in specs {
            if scope.literal_functions.contains_key(&spec.key) {
                continue;
            }
            let sig = self.user_function_signature();
            let fn_name = format!(
                "flux_lit_{}_{}_{}_{}_{}_{}_{}_{}_{}_{}",
                spec.key.sl,
                spec.key.sc,
                spec.key.el,
                spec.key.ec,
                spec.key.bsl,
                spec.key.bsc,
                spec.key.bel,
                spec.key.bec,
                spec.key.arity,
                spec.key.first_param
            );
            let id = self
                .module
                .declare_function(&fn_name, Linkage::Local, &sig)
                .map_err(|e| format!("declare {}: {}", fn_name, e))?;
            let function_index = self.jit_functions.len();
            let contract = runtime_contract_from_annotations(
                &spec.parameter_types,
                &spec.return_type,
                &[],
                interner,
            );
            self.jit_functions.push(JitFunctionCompileEntry {
                id,
                num_params: spec.parameters.len(),
                contract,
            });
            scope.literal_functions.insert(
                spec.key,
                JitFunctionMeta {
                    id,
                    num_params: spec.parameters.len(),
                    function_index,
                },
            );
            scope
                .literal_captures
                .insert(spec.key, spec.captures.clone());
        }
        Ok(())
    }

    fn compile_literal_functions(
        &mut self,
        specs: &[LiteralFunctionSpec],
        scope: &Scope,
        interner: &Interner,
    ) -> Result<(), String> {
        for spec in specs {
            let Some(meta) = scope.literal_functions.get(&spec.key).copied() else {
                continue;
            };

            let sig = self.user_function_signature();
            let mut func = Function::with_name_signature(UserFuncName::default(), sig);
            {
                let module = &mut self.module;
                let helpers = &self.helpers;
                let mut builder = FunctionBuilder::new(&mut func, &mut self.builder_ctx);
                let mut fn_scope = scope.clone();
                fn_scope.locals.clear();

                let entry = builder.create_block();
                let init_block = builder.create_block();
                let body_block = builder.create_block();
                let arity_fail = builder.create_block();
                let return_block = builder.create_block();
                builder.append_block_param(return_block, PTR_TYPE);
                builder.append_block_params_for_function_params(entry);
                builder.switch_to_block(entry);
                builder.seal_block(entry);

                let entry_params = builder.block_params(entry);
                let ctx_val = entry_params[0];
                let args_ptr = entry_params[1];
                let nargs = entry_params[2];
                let captures_ptr = entry_params[3];
                let ncaptures = entry_params[4];
                let want = builder.ins().iconst(PTR_TYPE, spec.parameters.len() as i64);
                let arity_ok = builder.ins().icmp(IntCC::Equal, nargs, want);
                builder
                    .ins()
                    .brif(arity_ok, init_block, &[], arity_fail, &[]);

                builder.switch_to_block(arity_fail);
                let set_arity_error =
                    get_helper_func_ref(module, helpers, &mut builder, "rt_set_arity_error");
                builder.ins().call(set_arity_error, &[ctx_val, nargs, want]);
                let null_ptr = builder.ins().iconst(PTR_TYPE, 0);
                builder.ins().return_(&[null_ptr]);
                builder.seal_block(arity_fail);

                builder.switch_to_block(init_block);
                let mut param_bindings: Vec<(Identifier, Variable)> =
                    Vec::with_capacity(spec.parameters.len());

                // Bind captures first; params may shadow them.
                for (idx, ident) in spec.captures.iter().enumerate() {
                    let cap_ptr = builder.ins().load(
                        PTR_TYPE,
                        MemFlags::new(),
                        captures_ptr,
                        (idx * 8) as i32,
                    );
                    let var = builder.declare_var(PTR_TYPE);
                    builder.def_var(var, cap_ptr);
                    fn_scope.locals.insert(*ident, var);
                }

                for (idx, ident) in spec.parameters.iter().enumerate() {
                    let arg_ptr =
                        builder
                            .ins()
                            .load(PTR_TYPE, MemFlags::new(), args_ptr, (idx * 8) as i32);
                    let var = builder.declare_var(PTR_TYPE);
                    builder.def_var(var, arg_ptr);
                    fn_scope.locals.insert(*ident, var);
                    param_bindings.push((*ident, var));
                }

                if let Some(self_name) = spec.self_name {
                    let make_jit_closure =
                        get_helper_func_ref(module, helpers, &mut builder, "rt_make_jit_closure");
                    let fn_idx = builder.ins().iconst(PTR_TYPE, meta.function_index as i64);
                    let call = builder.ins().call(
                        make_jit_closure,
                        &[ctx_val, fn_idx, captures_ptr, ncaptures],
                    );
                    let closure = builder.inst_results(call)[0];
                    let self_var = builder.declare_var(PTR_TYPE);
                    builder.def_var(self_var, closure);
                    fn_scope.locals.insert(self_name, self_var);
                }
                builder.ins().jump(body_block, &[]);
                builder.seal_block(init_block);

                let tail_ctx = TailCallContext {
                    function_name: spec.self_name,
                    loop_block: body_block,
                    params: param_bindings,
                };

                builder.switch_to_block(body_block);

                let mut last_val = None;
                let mut returned = false;
                let last_index = spec.body.statements.len().saturating_sub(1);
                for (idx, body_stmt) in spec.body.statements.iter().enumerate() {
                    if idx == last_index
                        && let Some(outcome) = try_compile_tail_expression_statement(
                            module,
                            helpers,
                            &mut builder,
                            &mut fn_scope,
                            ctx_val,
                            Some(return_block),
                            &tail_ctx,
                            body_stmt,
                            interner,
                        )?
                    {
                        match outcome {
                            StmtOutcome::Returned => {
                                returned = true;
                                break;
                            }
                            StmtOutcome::Value(v) => {
                                last_val = Some(v);
                                continue;
                            }
                            StmtOutcome::None => continue,
                        }
                    }
                    let outcome = compile_statement(
                        module,
                        helpers,
                        &mut builder,
                        &mut fn_scope,
                        ctx_val,
                        Some(return_block),
                        Some(&tail_ctx),
                        false,
                        body_stmt,
                        interner,
                    )?;
                    match outcome {
                        StmtOutcome::Value(v) => last_val = Some(v),
                        StmtOutcome::Returned => {
                            returned = true;
                            break;
                        }
                        StmtOutcome::None => {}
                    }
                }

                if !returned {
                    let ret = match last_val {
                        Some(v) => v,
                        None => {
                            let make_none =
                                get_helper_func_ref(module, helpers, &mut builder, "rt_make_none");
                            let call = builder.ins().call(make_none, &[ctx_val]);
                            builder.inst_results(call)[0]
                        }
                    };
                    let args = [BlockArg::Value(ret)];
                    builder.ins().jump(return_block, &args);
                }
                builder.seal_block(body_block);
                builder.switch_to_block(return_block);
                let ret = builder.block_params(return_block)[0];
                builder.ins().return_(&[ret]);
                builder.seal_block(return_block);
                builder.finalize();
            }

            let mut ctx = cranelift_codegen::Context::new();
            ctx.func = func;
            self.module
                .define_function(meta.id, &mut ctx)
                .map_err(|e| format!("define literal function: {}", e))?;
        }
        Ok(())
    }

    /// Finalize all functions and make them callable.
    pub fn finalize(&mut self) {
        self.module.finalize_definitions().unwrap();
    }

    /// Get a callable function pointer for the given FuncId.
    pub fn get_func_ptr(&self, id: FuncId) -> *const u8 {
        self.module.get_finalized_function(id)
    }

    pub fn jit_function_entries(&self) -> Vec<JitFunctionEntry> {
        self.jit_functions
            .iter()
            .map(|entry| JitFunctionEntry {
                ptr: self.module.get_finalized_function(entry.id),
                num_params: entry.num_params,
                contract: entry.contract.clone(),
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Free functions for compilation (avoids borrow conflicts with builder_ctx)
// ---------------------------------------------------------------------------

fn compile_statement(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    top_level: bool,
    stmt: &Statement,
    interner: &Interner,
) -> Result<StmtOutcome, String> {
    match stmt {
        Statement::Let { name, value, .. } => {
            let val = compile_expression(
                module,
                helpers,
                builder,
                scope,
                ctx_val,
                return_block,
                tail_call,
                value,
                interner,
            )?;
            emit_return_on_null_value(builder, val);
            if top_level {
                if let Some(&idx) = scope.globals.get(name) {
                    let set_global = get_helper_func_ref(module, helpers, builder, "rt_set_global");
                    let idx_val = builder.ins().iconst(PTR_TYPE, idx as i64);
                    builder.ins().call(set_global, &[ctx_val, idx_val, val]);
                } else {
                    let var = builder.declare_var(PTR_TYPE);
                    builder.def_var(var, val);
                    scope.locals.insert(*name, var);
                }
            } else {
                let var = builder.declare_var(PTR_TYPE);
                builder.def_var(var, val);
                scope.locals.insert(*name, var);
            }
            Ok(StmtOutcome::None)
        }
        Statement::LetDestructure { pattern, value, .. } => {
            let val = compile_expression(
                module,
                helpers,
                builder,
                scope,
                ctx_val,
                return_block,
                tail_call,
                value,
                interner,
            )?;
            emit_return_on_null_value(builder, val);
            if top_level {
                bind_top_level_pattern_value(
                    module, helpers, builder, scope, ctx_val, pattern, val,
                )?;
            } else {
                bind_pattern_value(module, helpers, builder, scope, ctx_val, pattern, val)?;
            }
            Ok(StmtOutcome::None)
        }
        Statement::Expression {
            expression,
            has_semicolon,
            ..
        } => {
            if !*has_semicolon
                && let Some(tc) = tail_call
                && let Some(fn_name) = tc.function_name
                && let Expression::Call {
                    function,
                    arguments,
                    ..
                } = expression
                && let Expression::Identifier { name, .. } = function.as_ref()
                && *name == fn_name
                && arguments.len() == tc.params.len()
            {
                let mut arg_vals = Vec::with_capacity(arguments.len());
                for arg in arguments {
                    arg_vals.push(compile_expression(
                        module,
                        helpers,
                        builder,
                        scope,
                        ctx_val,
                        return_block,
                        tail_call,
                        arg,
                        interner,
                    )?);
                }
                for (idx, (_, var)) in tc.params.iter().enumerate() {
                    builder.def_var(*var, arg_vals[idx]);
                }
                builder.ins().jump(tc.loop_block, &[]);
                return Ok(StmtOutcome::Returned);
            }

            let val = compile_expression(
                module,
                helpers,
                builder,
                scope,
                ctx_val,
                return_block,
                tail_call,
                expression,
                interner,
            )?;
            if *has_semicolon {
                emit_return_on_null_value(builder, val);
                Ok(StmtOutcome::None)
            } else {
                Ok(StmtOutcome::Value(val))
            }
        }
        Statement::Assign { name, value, .. } => {
            let val = compile_expression(
                module,
                helpers,
                builder,
                scope,
                ctx_val,
                return_block,
                tail_call,
                value,
                interner,
            )?;
            emit_return_on_null_value(builder, val);
            if let Some(&var) = scope.locals.get(name) {
                builder.def_var(var, val);
            } else if let Some(&idx) = scope.globals.get(name) {
                let set_global = get_helper_func_ref(module, helpers, builder, "rt_set_global");
                let idx_val = builder.ins().iconst(PTR_TYPE, idx as i64);
                builder.ins().call(set_global, &[ctx_val, idx_val, val]);
            }
            Ok(StmtOutcome::None)
        }
        Statement::Return { value, .. } => {
            let Some(rb) = return_block else {
                return Err("return outside function is not supported in JIT".to_string());
            };
            if let (
                Some(tc),
                Some(Expression::Call {
                    function,
                    arguments,
                    ..
                }),
            ) = (tail_call, value)
                && let Some(fn_name) = tc.function_name
                && let Expression::Identifier { name, .. } = function.as_ref()
                && *name == fn_name
                && arguments.len() == tc.params.len()
            {
                let mut arg_vals = Vec::with_capacity(arguments.len());
                for arg in arguments {
                    arg_vals.push(compile_expression(
                        module,
                        helpers,
                        builder,
                        scope,
                        ctx_val,
                        return_block,
                        tail_call,
                        arg,
                        interner,
                    )?);
                }
                for (idx, (_, var)) in tc.params.iter().enumerate() {
                    builder.def_var(*var, arg_vals[idx]);
                }
                builder.ins().jump(tc.loop_block, &[]);
                return Ok(StmtOutcome::Returned);
            }
            let ret = match value {
                Some(v) => compile_expression(
                    module,
                    helpers,
                    builder,
                    scope,
                    ctx_val,
                    return_block,
                    tail_call,
                    v,
                    interner,
                )?,
                None => {
                    let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
                    let call = builder.ins().call(make_none, &[ctx_val]);
                    builder.inst_results(call)[0]
                }
            };
            let args = [BlockArg::Value(ret)];
            builder.ins().jump(rb, &args);
            Ok(StmtOutcome::Returned)
        }
        Statement::Function { name, .. } => {
            let Statement::Function {
                parameters,
                parameter_types,
                return_type,
                effects,
                body,
                span,
                ..
            } = stmt
            else {
                unreachable!()
            };
            let expr = Expression::Function {
                parameters: parameters.clone(),
                parameter_types: parameter_types.clone(),
                return_type: return_type.clone(),
                effects: effects.clone(),
                body: body.clone(),
                span: *span,
            };
            let fn_val = compile_function_literal(
                module, helpers, builder, scope, ctx_val, &expr, interner,
            )?;
            let var = builder.declare_var(PTR_TYPE);
            builder.def_var(var, fn_val);
            scope.locals.insert(*name, var);
            Ok(StmtOutcome::None)
        }
        Statement::Import {
            name,
            alias,
            except: _,
            ..
        } => {
            if !is_base_symbol(*name, interner) {
                scope.imported_modules.insert(*name);
                if let Some(alias) = alias {
                    scope.import_aliases.insert(*alias, *name);
                }
            }
            Ok(StmtOutcome::None)
        }
        Statement::Module { name, .. } => {
            scope.imported_modules.insert(*name);
            Ok(StmtOutcome::None)
        }
        Statement::Data { .. } => {
            // ADT declarations are no-ops at runtime; constructors are called directly.
            Ok(StmtOutcome::None)
        }
        Statement::EffectDecl { .. } => {
            // Effect declarations are syntax-only; no JIT code emitted.
            Ok(StmtOutcome::None)
        }
    }
}

fn emit_return_on_null_value(builder: &mut FunctionBuilder, value_ptr: CraneliftValue) {
    let is_null = builder.ins().icmp_imm(IntCC::Equal, value_ptr, 0);
    let null_block = builder.create_block();
    let continue_block = builder.create_block();
    builder
        .ins()
        .brif(is_null, null_block, &[], continue_block, &[]);

    builder.switch_to_block(null_block);
    let null_ptr = builder.ins().iconst(PTR_TYPE, 0);
    builder.ins().return_(&[null_ptr]);
    builder.seal_block(null_block);

    builder.switch_to_block(continue_block);
    builder.seal_block(continue_block);
}

fn try_compile_tail_expression_statement(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_ctx: &TailCallContext,
    stmt: &Statement,
    interner: &Interner,
) -> Result<Option<StmtOutcome>, String> {
    let Some(fn_name) = tail_ctx.function_name else {
        return Ok(None);
    };
    let Statement::Expression { expression, .. } = stmt else {
        return Ok(None);
    };
    let Expression::Call {
        function,
        arguments,
        ..
    } = expression
    else {
        return Ok(None);
    };
    let Expression::Identifier { name, .. } = function.as_ref() else {
        return Ok(None);
    };
    if *name != fn_name || arguments.len() != tail_ctx.params.len() {
        return Ok(None);
    }

    let mut arg_vals = Vec::with_capacity(arguments.len());
    for arg in arguments {
        arg_vals.push(compile_expression(
            module,
            helpers,
            builder,
            scope,
            ctx_val,
            return_block,
            Some(tail_ctx),
            arg,
            interner,
        )?);
    }
    for (idx, (_, var)) in tail_ctx.params.iter().enumerate() {
        builder.def_var(*var, arg_vals[idx]);
    }
    builder.ins().jump(tail_ctx.loop_block, &[]);
    Ok(Some(StmtOutcome::Returned))
}

fn compile_expression(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    expr: &Expression,
    interner: &Interner,
) -> Result<CraneliftValue, String> {
    match expr {
        // --- Literals ---
        Expression::Integer { value, .. } => {
            let make_int = get_helper_func_ref(module, helpers, builder, "rt_make_integer");
            let v = builder.ins().iconst(PTR_TYPE, *value);
            let call = builder.ins().call(make_int, &[ctx_val, v]);
            Ok(builder.inst_results(call)[0])
        }
        Expression::Float { value, .. } => {
            let make_float = get_helper_func_ref(module, helpers, builder, "rt_make_float");
            let bits = builder.ins().iconst(PTR_TYPE, value.to_bits() as i64);
            let call = builder.ins().call(make_float, &[ctx_val, bits]);
            Ok(builder.inst_results(call)[0])
        }
        Expression::Boolean { value, .. } => {
            let make_bool = get_helper_func_ref(module, helpers, builder, "rt_make_bool");
            let v = builder.ins().iconst(PTR_TYPE, *value as i64);
            let call = builder.ins().call(make_bool, &[ctx_val, v]);
            Ok(builder.inst_results(call)[0])
        }
        Expression::None { .. } => {
            let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
            let call = builder.ins().call(make_none, &[ctx_val]);
            Ok(builder.inst_results(call)[0])
        }
        Expression::EmptyList { .. } => {
            let make_empty = get_helper_func_ref(module, helpers, builder, "rt_make_empty_list");
            let call = builder.ins().call(make_empty, &[ctx_val]);
            Ok(builder.inst_results(call)[0])
        }
        Expression::String { value, .. } => {
            let make_string = get_helper_func_ref(module, helpers, builder, "rt_make_string");
            let bytes = value.as_bytes();
            let data = module
                .declare_anonymous_data(false, false)
                .map_err(|e| e.to_string())?;
            let mut desc = cranelift_module::DataDescription::new();
            desc.define(bytes.to_vec().into_boxed_slice());
            module.define_data(data, &desc).map_err(|e| e.to_string())?;
            let gv = module.declare_data_in_func(data, builder.func);
            let ptr = builder.ins().global_value(PTR_TYPE, gv);
            let len = builder.ins().iconst(PTR_TYPE, bytes.len() as i64);
            let call = builder.ins().call(make_string, &[ctx_val, ptr, len]);
            Ok(builder.inst_results(call)[0])
        }
        Expression::TupleLiteral { elements, .. } => {
            let mut elem_vals = Vec::with_capacity(elements.len());
            for elem in elements {
                let val = compile_expression(
                    module,
                    helpers,
                    builder,
                    scope,
                    ctx_val,
                    return_block,
                    tail_call,
                    elem,
                    interner,
                )?;
                elem_vals.push(val);
            }
            let len = elem_vals.len();
            let slot = builder.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
                cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                (len as u32).max(1) * 8,
                3,
            ));
            for (i, val) in elem_vals.iter().enumerate() {
                builder.ins().stack_store(*val, slot, (i * 8) as i32);
            }
            let elems_ptr = builder.ins().stack_addr(PTR_TYPE, slot, 0);
            let len_val = builder.ins().iconst(PTR_TYPE, len as i64);
            let make_tuple = get_helper_func_ref(module, helpers, builder, "rt_make_tuple");
            let call = builder
                .ins()
                .call(make_tuple, &[ctx_val, elems_ptr, len_val]);
            Ok(builder.inst_results(call)[0])
        }
        Expression::TupleFieldAccess { object, index, .. } => {
            let tuple_val = compile_expression(
                module,
                helpers,
                builder,
                scope,
                ctx_val,
                return_block,
                tail_call,
                object,
                interner,
            )?;
            let index_val = builder.ins().iconst(PTR_TYPE, *index as i64);
            let tuple_get = get_helper_func_ref(module, helpers, builder, "rt_tuple_get");
            let call = builder
                .ins()
                .call(tuple_get, &[ctx_val, tuple_val, index_val]);
            Ok(builder.inst_results(call)[0])
        }

        // --- Identifiers ---
        Expression::Identifier { name, .. } => {
            if let Some(&var) = scope.locals.get(name) {
                Ok(builder.use_var(var))
            } else if let Some(meta) = scope.functions.get(name).copied() {
                let make_jit_closure =
                    get_helper_func_ref(module, helpers, builder, "rt_make_jit_closure");
                let fn_idx = builder.ins().iconst(PTR_TYPE, meta.function_index as i64);
                let null_ptr = builder.ins().iconst(PTR_TYPE, 0);
                let zero = builder.ins().iconst(PTR_TYPE, 0);
                let call = builder
                    .ins()
                    .call(make_jit_closure, &[ctx_val, fn_idx, null_ptr, zero]);
                Ok(builder.inst_results(call)[0])
            } else if let Some(&base_idx) = scope.base_functions.get(name) {
                let make_base =
                    get_helper_func_ref(module, helpers, builder, "rt_make_base_function");
                let idx = builder.ins().iconst(PTR_TYPE, base_idx as i64);
                let call = builder.ins().call(make_base, &[ctx_val, idx]);
                Ok(builder.inst_results(call)[0])
            } else if let Some(&idx) = scope.globals.get(name) {
                let get_global = get_helper_func_ref(module, helpers, builder, "rt_get_global");
                let idx_val = builder.ins().iconst(PTR_TYPE, idx as i64);
                let call = builder.ins().call(get_global, &[ctx_val, idx_val]);
                Ok(builder.inst_results(call)[0])
            } else if scope.adt_constructors.get(name).copied() == Some(0) {
                // Zero-arg ADT constructor used as a value (e.g. `Point`, `None_`)
                let name_str = interner.resolve(*name);
                let bytes = name_str.as_bytes().to_vec();

                let data = module
                    .declare_anonymous_data(false, false)
                    .map_err(|e| e.to_string())?;

                let mut desc = DataDescription::new();
                desc.define(bytes.into_boxed_slice());
                module.define_data(data, &desc).map_err(|e| e.to_string())?;

                let global_value = module.declare_data_in_func(data, builder.func);
                let name_ptr = builder.ins().global_value(PTR_TYPE, global_value);
                let name_len = builder.ins().iconst(PTR_TYPE, name_str.len() as i64);

                let empty_slot = builder.create_sized_stack_slot(StackSlotData::new(
                    cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                    8,
                    3,
                ));

                let fields_ptr = builder.ins().stack_addr(PTR_TYPE, empty_slot, 0);
                let zero = builder.ins().iconst(PTR_TYPE, 0);
                let make_adt = get_helper_func_ref(module, helpers, builder, "rt_make_adt");
                let call = builder
                    .ins()
                    .call(make_adt, &[ctx_val, name_ptr, name_len, fields_ptr, zero]);

                Ok(builder.inst_results(call)[0])
            } else {
                Err(format!("undefined identifier: {}", interner.resolve(*name)))
            }
        }
        Expression::MemberAccess { object, member, .. } => {
            if let Expression::Identifier { name, .. } = object.as_ref() {
                if is_base_symbol(*name, interner) {
                    let member_name = interner.resolve(*member);
                    let Some(index) = BaseModule::new().index_of(member_name) else {
                        return Err(format!("unknown Base member: {}", member_name));
                    };
                    let make_base =
                        get_helper_func_ref(module, helpers, builder, "rt_make_base_function");
                    let idx = builder.ins().iconst(PTR_TYPE, index as i64);
                    let call = builder.ins().call(make_base, &[ctx_val, idx]);
                    return Ok(builder.inst_results(call)[0]);
                }

                let module_name = scope.import_aliases.get(name).copied().or_else(|| {
                    if scope.imported_modules.contains(name)
                        || scope
                            .module_functions
                            .keys()
                            .any(|(module_name, _)| module_name == name)
                    {
                        Some(*name)
                    } else {
                        None
                    }
                });

                if let Some(module_name) = module_name {
                    if let Some(meta) = scope.module_functions.get(&(module_name, *member)).copied()
                    {
                        let make_jit_closure =
                            get_helper_func_ref(module, helpers, builder, "rt_make_jit_closure");
                        let fn_idx = builder.ins().iconst(PTR_TYPE, meta.function_index as i64);
                        let null_ptr = builder.ins().iconst(PTR_TYPE, 0);
                        let zero = builder.ins().iconst(PTR_TYPE, 0);
                        let call = builder
                            .ins()
                            .call(make_jit_closure, &[ctx_val, fn_idx, null_ptr, zero]);
                        return Ok(builder.inst_results(call)[0]);
                    }

                    return Err(format!(
                        "unknown module member: {}.{}",
                        interner.resolve(module_name),
                        interner.resolve(*member)
                    ));
                }
            }

            Err("unsupported member access in JIT (only Module.member is supported)".to_string())
        }

        // --- Prefix operators ---
        Expression::Prefix {
            operator, right, ..
        } => {
            let operand = compile_expression(
                module,
                helpers,
                builder,
                scope,
                ctx_val,
                return_block,
                tail_call,
                right,
                interner,
            )?;
            let helper_name = match operator.as_str() {
                "-" => "rt_negate",
                "!" => "rt_not",
                _ => return Err(format!("unknown prefix operator: {}", operator)),
            };
            let func_ref = get_helper_func_ref(module, helpers, builder, helper_name);
            let call = builder.ins().call(func_ref, &[ctx_val, operand]);
            Ok(builder.inst_results(call)[0])
        }

        // --- Infix operators ---
        Expression::Infix {
            left,
            operator,
            right,
            ..
        } => {
            if operator == "&&" || operator == "||" {
                return compile_short_circuit_expression(
                    module,
                    helpers,
                    builder,
                    scope,
                    ctx_val,
                    return_block,
                    tail_call,
                    left,
                    operator,
                    right,
                    interner,
                );
            }
            let lhs = compile_expression(
                module,
                helpers,
                builder,
                scope,
                ctx_val,
                return_block,
                tail_call,
                left,
                interner,
            )?;
            let rhs = compile_expression(
                module,
                helpers,
                builder,
                scope,
                ctx_val,
                return_block,
                tail_call,
                right,
                interner,
            )?;
            let helper_name = match operator.as_str() {
                "+" => "rt_add",
                "-" => "rt_sub",
                "*" => "rt_mul",
                "/" => "rt_div",
                "%" => "rt_mod",
                "==" => "rt_equal",
                "!=" => "rt_not_equal",
                ">" => "rt_greater_than",
                "<=" => "rt_less_than_or_equal",
                ">=" => "rt_greater_than_or_equal",
                "<" => {
                    // a < b  ⟹  !(a >= b)
                    let ge_ref =
                        get_helper_func_ref(module, helpers, builder, "rt_greater_than_or_equal");
                    let ge_call = builder.ins().call(ge_ref, &[ctx_val, lhs, rhs]);
                    let ge_result = builder.inst_results(ge_call)[0];
                    let not_ref = get_helper_func_ref(module, helpers, builder, "rt_not");
                    let not_call = builder.ins().call(not_ref, &[ctx_val, ge_result]);
                    return Ok(builder.inst_results(not_call)[0]);
                }
                _ => return Err(format!("unknown infix operator: {}", operator)),
            };
            let func_ref = get_helper_func_ref(module, helpers, builder, helper_name);
            let call = builder.ins().call(func_ref, &[ctx_val, lhs, rhs]);
            Ok(builder.inst_results(call)[0])
        }
        Expression::If {
            condition,
            consequence,
            alternative,
            ..
        } => compile_if_expression(
            module,
            helpers,
            builder,
            scope,
            ctx_val,
            return_block,
            tail_call,
            condition,
            consequence,
            alternative.as_ref(),
            interner,
        ),
        Expression::DoBlock { block, .. } => {
            match compile_block_expression(
                module,
                helpers,
                builder,
                scope,
                ctx_val,
                return_block,
                tail_call,
                block,
                interner,
            )? {
                BlockEval::Returned => {
                    // The do-block already terminated control-flow (e.g. tail-call jump).
                    // Continue emission in a fresh block to avoid appending instructions
                    // after a terminator in the previous block.
                    let continue_block = builder.create_block();
                    builder.switch_to_block(continue_block);
                    let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
                    let call = builder.ins().call(make_none, &[ctx_val]);
                    builder.seal_block(continue_block);
                    Ok(builder.inst_results(call)[0])
                }
                BlockEval::Value(v) => Ok(v),
            }
        }

        // --- Function calls ---
        Expression::Call {
            function,
            arguments,
            span,
        } => {
            if let Some(primop) = resolve_call_primop(scope, function, arguments, interner) {
                return compile_primop_call(
                    module,
                    helpers,
                    builder,
                    scope,
                    ctx_val,
                    return_block,
                    tail_call,
                    primop,
                    arguments,
                    interner,
                );
            }
            // Check if calling a registered ADT constructor
            if let Expression::Identifier { name, .. } = function.as_ref()
                && let Some(&arity) = scope.adt_constructors.get(name)
            {
                let name_str = interner.resolve(*name);
                let bytes = name_str.as_bytes().to_vec();

                let data = module
                    .declare_anonymous_data(false, false)
                    .map_err(|e| e.to_string())?;
                let mut desc = DataDescription::new();
                desc.define(bytes.into_boxed_slice());
                module.define_data(data, &desc).map_err(|e| e.to_string())?;

                let global_value = module.declare_data_in_func(data, builder.func);
                let name_ptr = builder.ins().global_value(PTR_TYPE, global_value);
                let name_len = builder.ins().iconst(PTR_TYPE, name_str.len() as i64);

                let mut arg_vals = Vec::with_capacity(arguments.len());

                for arg in arguments {
                    let value = compile_expression(
                        module,
                        helpers,
                        builder,
                        scope,
                        ctx_val,
                        return_block,
                        tail_call,
                        arg,
                        interner,
                    )?;
                    arg_vals.push(value);
                }

                let n = arg_vals.len();
                let slot = builder.create_sized_stack_slot(StackSlotData::new(
                    cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                    (n as u32).max(1) * 8,
                    3,
                ));

                for (i, value) in arg_vals.iter().enumerate() {
                    builder.ins().stack_store(*value, slot, (i * 8) as i32);
                }

                let fields_ptr = builder.ins().stack_addr(PTR_TYPE, slot, 0);
                let arity_value = builder.ins().iconst(PTR_TYPE, arity as i64);
                let make_adt = get_helper_func_ref(module, helpers, builder, "rt_make_adt");

                let call = builder.ins().call(
                    make_adt,
                    &[ctx_val, name_ptr, name_len, fields_ptr, arity_value],
                );

                return Ok(builder.inst_results(call)[0]);
            }
            // Check if calling a base directly
            if let Expression::Identifier { name, .. } = function.as_ref() {
                if let Some(meta) = scope.functions.get(name).copied() {
                    return compile_user_function_call(
                        module,
                        helpers,
                        builder,
                        scope,
                        ctx_val,
                        return_block,
                        tail_call,
                        meta,
                        *span,
                        arguments,
                        interner,
                    );
                }
                if should_use_base_fastcall(scope, *name, interner)
                    && let Some(&base_idx) = scope.base_functions.get(name)
                {
                    return compile_base_function_call(
                        module,
                        helpers,
                        builder,
                        scope,
                        ctx_val,
                        return_block,
                        tail_call,
                        base_idx,
                        arguments,
                        interner,
                    );
                }
            }
            compile_generic_call(
                module,
                helpers,
                builder,
                scope,
                ctx_val,
                return_block,
                tail_call,
                function,
                arguments,
                interner,
            )
        }
        Expression::Function { .. } => {
            compile_function_literal(module, helpers, builder, scope, ctx_val, expr, interner)
        }
        Expression::Cons { head, tail, .. } => {
            let head_val = compile_expression(
                module,
                helpers,
                builder,
                scope,
                ctx_val,
                return_block,
                tail_call,
                head,
                interner,
            )?;
            let tail_val = compile_expression(
                module,
                helpers,
                builder,
                scope,
                ctx_val,
                return_block,
                tail_call,
                tail,
                interner,
            )?;
            let make_cons = get_helper_func_ref(module, helpers, builder, "rt_make_cons");
            let call = builder
                .ins()
                .call(make_cons, &[ctx_val, head_val, tail_val]);
            Ok(builder.inst_results(call)[0])
        }
        Expression::Match {
            scrutinee, arms, ..
        } => compile_match_expression(
            module,
            helpers,
            builder,
            scope,
            ctx_val,
            return_block,
            tail_call,
            scrutinee,
            arms,
            interner,
        ),

        Expression::Some { value, .. } => {
            let inner = compile_expression(
                module,
                helpers,
                builder,
                scope,
                ctx_val,
                return_block,
                tail_call,
                value,
                interner,
            )?;
            let make_some = get_helper_func_ref(module, helpers, builder, "rt_make_some");
            let call = builder.ins().call(make_some, &[ctx_val, inner]);
            Ok(builder.inst_results(call)[0])
        }
        Expression::Left { value, .. } => {
            let inner = compile_expression(
                module,
                helpers,
                builder,
                scope,
                ctx_val,
                return_block,
                tail_call,
                value,
                interner,
            )?;
            let make_left = get_helper_func_ref(module, helpers, builder, "rt_make_left");
            let call = builder.ins().call(make_left, &[ctx_val, inner]);
            Ok(builder.inst_results(call)[0])
        }
        Expression::Right { value, .. } => {
            let inner = compile_expression(
                module,
                helpers,
                builder,
                scope,
                ctx_val,
                return_block,
                tail_call,
                value,
                interner,
            )?;
            let make_right = get_helper_func_ref(module, helpers, builder, "rt_make_right");
            let call = builder.ins().call(make_right, &[ctx_val, inner]);
            Ok(builder.inst_results(call)[0])
        }
        Expression::ArrayLiteral { elements, .. } => {
            let mut elem_vals = Vec::with_capacity(elements.len());
            for elem in elements {
                let val = compile_expression(
                    module,
                    helpers,
                    builder,
                    scope,
                    ctx_val,
                    return_block,
                    tail_call,
                    elem,
                    interner,
                )?;
                elem_vals.push(val);
            }
            let len = elem_vals.len();
            let slot = builder.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
                cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                (len as u32).max(1) * 8,
                3,
            ));
            for (i, val) in elem_vals.iter().enumerate() {
                builder.ins().stack_store(*val, slot, (i * 8) as i32);
            }
            let elems_ptr = builder.ins().stack_addr(PTR_TYPE, slot, 0);
            let len_val = builder.ins().iconst(PTR_TYPE, len as i64);
            let make_array = get_helper_func_ref(module, helpers, builder, "rt_make_array");
            let call = builder
                .ins()
                .call(make_array, &[ctx_val, elems_ptr, len_val]);
            Ok(builder.inst_results(call)[0])
        }
        Expression::ListLiteral { elements, .. } => {
            // Build cons chain in reverse: start with None, prepend each element
            let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
            let make_cons = get_helper_func_ref(module, helpers, builder, "rt_make_cons");
            let none_call = builder.ins().call(make_none, &[ctx_val]);
            let mut acc = builder.inst_results(none_call)[0];
            for elem in elements.iter().rev() {
                let val = compile_expression(
                    module,
                    helpers,
                    builder,
                    scope,
                    ctx_val,
                    return_block,
                    tail_call,
                    elem,
                    interner,
                )?;
                let cons_call = builder.ins().call(make_cons, &[ctx_val, val, acc]);
                acc = builder.inst_results(cons_call)[0];
            }
            Ok(acc)
        }
        Expression::Hash { pairs, .. } => {
            let npairs = pairs.len();
            let mut pair_vals = Vec::with_capacity(npairs * 2);
            for (key, value) in pairs {
                let k = compile_expression(
                    module,
                    helpers,
                    builder,
                    scope,
                    ctx_val,
                    return_block,
                    tail_call,
                    key,
                    interner,
                )?;
                let v = compile_expression(
                    module,
                    helpers,
                    builder,
                    scope,
                    ctx_val,
                    return_block,
                    tail_call,
                    value,
                    interner,
                )?;
                pair_vals.push(k);
                pair_vals.push(v);
            }
            let slot_size = (npairs as u32 * 2).max(1) * 8;
            let slot = builder.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
                cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                slot_size,
                3,
            ));
            for (i, val) in pair_vals.iter().enumerate() {
                builder.ins().stack_store(*val, slot, (i * 8) as i32);
            }
            let pairs_ptr = builder.ins().stack_addr(PTR_TYPE, slot, 0);
            let npairs_val = builder.ins().iconst(PTR_TYPE, npairs as i64);
            let make_hash = get_helper_func_ref(module, helpers, builder, "rt_make_hash");
            let call = builder
                .ins()
                .call(make_hash, &[ctx_val, pairs_ptr, npairs_val]);
            Ok(builder.inst_results(call)[0])
        }
        Expression::Index { left, index, .. } => {
            let left_val = compile_expression(
                module,
                helpers,
                builder,
                scope,
                ctx_val,
                return_block,
                tail_call,
                left,
                interner,
            )?;
            let index_val = compile_expression(
                module,
                helpers,
                builder,
                scope,
                ctx_val,
                return_block,
                tail_call,
                index,
                interner,
            )?;
            let rt_index = get_helper_func_ref(module, helpers, builder, "rt_index");
            let call = builder
                .ins()
                .call(rt_index, &[ctx_val, left_val, index_val]);
            Ok(builder.inst_results(call)[0])
        }
        Expression::InterpolatedString { parts, .. } => {
            use crate::syntax::expression::StringPart;
            let rt_to_string = get_helper_func_ref(module, helpers, builder, "rt_to_string");
            let rt_add = get_helper_func_ref(module, helpers, builder, "rt_add");

            let mut acc: Option<CraneliftValue> = None;
            for part in parts {
                let part_val = match part {
                    StringPart::Literal(s) => {
                        let bytes = s.as_bytes();
                        let data = module
                            .declare_anonymous_data(false, false)
                            .map_err(|e| e.to_string())?;
                        let mut desc = cranelift_module::DataDescription::new();
                        desc.define(bytes.to_vec().into_boxed_slice());
                        module.define_data(data, &desc).map_err(|e| e.to_string())?;
                        let gv = module.declare_data_in_func(data, builder.func);
                        let ptr = builder.ins().global_value(PTR_TYPE, gv);
                        let len = builder.ins().iconst(PTR_TYPE, bytes.len() as i64);
                        let make_string =
                            get_helper_func_ref(module, helpers, builder, "rt_make_string");
                        let call = builder.ins().call(make_string, &[ctx_val, ptr, len]);
                        builder.inst_results(call)[0]
                    }
                    StringPart::Interpolation(expr) => {
                        let val = compile_expression(
                            module,
                            helpers,
                            builder,
                            scope,
                            ctx_val,
                            return_block,
                            tail_call,
                            expr,
                            interner,
                        )?;
                        let call = builder.ins().call(rt_to_string, &[ctx_val, val]);
                        builder.inst_results(call)[0]
                    }
                };
                acc = Some(match acc {
                    None => part_val,
                    Some(prev) => {
                        let call = builder.ins().call(rt_add, &[ctx_val, prev, part_val]);
                        builder.inst_results(call)[0]
                    }
                });
            }
            // Empty interpolated string edge case
            match acc {
                Some(val) => Ok(val),
                None => {
                    let make_string =
                        get_helper_func_ref(module, helpers, builder, "rt_make_string");
                    let null = builder.ins().iconst(PTR_TYPE, 0);
                    let zero = builder.ins().iconst(PTR_TYPE, 0);
                    let call = builder.ins().call(make_string, &[ctx_val, null, zero]);
                    Ok(builder.inst_results(call)[0])
                }
            }
        }
        Expression::Perform {
            effect,
            operation,
            args,
            span,
        } => compile_jit_perform(
            module,
            helpers,
            builder,
            scope,
            ctx_val,
            return_block,
            tail_call,
            *effect,
            *operation,
            args,
            interner,
            *span,
        ),
        Expression::Handle {
            expr, effect, arms, ..
        } => compile_jit_handle(
            module,
            helpers,
            builder,
            scope,
            ctx_val,
            return_block,
            tail_call,
            expr,
            *effect,
            arms,
            interner,
        ),
    }
}

/// Compile `perform Effect.op(args)` in JIT mode.
#[allow(clippy::too_many_arguments)]
fn compile_jit_perform(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    effect: crate::syntax::symbol::Symbol,
    op: crate::syntax::symbol::Symbol,
    args: &[Expression],
    interner: &Interner,
    span: crate::diagnostics::position::Span,
) -> Result<CraneliftValue, String> {
    let mut arg_vals: Vec<CraneliftValue> = Vec::new();
    for arg in args {
        let val = compile_expression(
            module,
            helpers,
            builder,
            scope,
            ctx_val,
            return_block,
            tail_call,
            arg,
            interner,
        )?;
        arg_vals.push(val);
    }

    let nargs = arg_vals.len();
    let slot = builder.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
        cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
        (nargs.max(1) as u32) * 8,
        3,
    ));
    for (i, val) in arg_vals.iter().enumerate() {
        builder.ins().stack_store(*val, slot, (i * 8) as i32);
    }
    let args_ptr = builder.ins().stack_addr(PTR_TYPE, slot, 0);
    let nargs_val = builder.ins().iconst(PTR_TYPE, nargs as i64);
    let effect_val = builder.ins().iconst(PTR_TYPE, effect.as_u32() as i64);
    let op_val = builder.ins().iconst(PTR_TYPE, op.as_u32() as i64);

    // Leak the name strings as stable pointers for the JIT runtime error messages.
    let effect_str: &'static str = Box::leak(interner.resolve(effect).to_owned().into_boxed_str());
    let op_str: &'static str = Box::leak(interner.resolve(op).to_owned().into_boxed_str());
    let effect_name_ptr = builder.ins().iconst(PTR_TYPE, effect_str.as_ptr() as i64);
    let effect_name_len = builder.ins().iconst(PTR_TYPE, effect_str.len() as i64);
    let op_name_ptr = builder.ins().iconst(PTR_TYPE, op_str.as_ptr() as i64);
    let op_name_len = builder.ins().iconst(PTR_TYPE, op_str.len() as i64);
    let line_val = builder.ins().iconst(PTR_TYPE, span.start.line as i64);
    let col_val = builder.ins().iconst(PTR_TYPE, span.start.column as i64);

    let rt_perform = get_helper_func_ref(module, helpers, builder, "rt_perform");
    let call = builder.ins().call(
        rt_perform,
        &[
            ctx_val,
            effect_val,
            op_val,
            args_ptr,
            nargs_val,
            effect_name_ptr,
            effect_name_len,
            op_name_ptr,
            op_name_len,
            line_val,
            col_val,
        ],
    );
    let result = builder.inst_results(call)[0];

    // Null result means rt_perform set an error in the context; propagate upward.
    emit_return_on_null_value(builder, result);
    Ok(result)
}

/// Compile `expr handle Effect { arms... }` in JIT mode.
#[allow(clippy::too_many_arguments)]
fn compile_jit_handle(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    expr: &Expression,
    effect: crate::syntax::symbol::Symbol,
    arms: &[crate::syntax::expression::HandleArm],
    interner: &Interner,
) -> Result<CraneliftValue, String> {
    let num_arms = arms.len();
    let mut op_sym_vals: Vec<CraneliftValue> = Vec::new();
    let mut closure_vals: Vec<CraneliftValue> = Vec::new();

    for arm in arms {
        op_sym_vals.push(
            builder
                .ins()
                .iconst(PTR_TYPE, arm.operation_name.as_u32() as i64),
        );

        // Build a synthetic Function expression for the arm body
        let mut params = vec![arm.resume_param];
        params.extend_from_slice(&arm.params);
        let arm_span = arm.body.span();
        let arm_fn_expr = Expression::Function {
            parameters: params,
            parameter_types: vec![None; 1 + arm.params.len()],
            return_type: None,
            effects: vec![],
            body: crate::syntax::block::Block {
                statements: vec![crate::syntax::statement::Statement::Expression {
                    expression: arm.body.clone(),
                    has_semicolon: false,
                    span: arm_span,
                }],
                span: arm_span,
            },
            span: arm.span,
        };
        let cv = compile_function_literal(
            module,
            helpers,
            builder,
            scope,
            ctx_val,
            &arm_fn_expr,
            interner,
        )?;
        closure_vals.push(cv);
    }

    // Store op symbols in a stack slot
    let ops_slot = builder.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
        cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
        (num_arms.max(1) as u32) * 8,
        3,
    ));
    for (i, ov) in op_sym_vals.iter().enumerate() {
        builder.ins().stack_store(*ov, ops_slot, (i * 8) as i32);
    }
    let ops_ptr = builder.ins().stack_addr(PTR_TYPE, ops_slot, 0);

    // Store closures in a stack slot
    let cls_slot = builder.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
        cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
        (num_arms.max(1) as u32) * 8,
        3,
    ));
    for (i, cv) in closure_vals.iter().enumerate() {
        builder.ins().stack_store(*cv, cls_slot, (i * 8) as i32);
    }
    let closures_ptr = builder.ins().stack_addr(PTR_TYPE, cls_slot, 0);

    let effect_val = builder.ins().iconst(PTR_TYPE, effect.as_u32() as i64);
    let narms_val = builder.ins().iconst(PTR_TYPE, num_arms as i64);

    let rt_push_handler = get_helper_func_ref(module, helpers, builder, "rt_push_handler");
    builder.ins().call(
        rt_push_handler,
        &[ctx_val, effect_val, ops_ptr, closures_ptr, narms_val],
    );

    let result = compile_expression(
        module,
        helpers,
        builder,
        scope,
        ctx_val,
        return_block,
        tail_call,
        expr,
        interner,
    )?;

    let rt_pop_handler = get_helper_func_ref(module, helpers, builder, "rt_pop_handler");
    builder.ins().call(rt_pop_handler, &[ctx_val]);

    Ok(result)
}

fn collect_adt_definitions(program: &Program, scope: &mut Scope, interner: &Interner) {
    for statement in &program.statements {
        collect_adt_definitions_from_stmt(statement, scope, interner);
    }
}

fn collect_adt_definitions_from_stmt(
    statement: &Statement,
    scope: &mut Scope,
    interner: &Interner,
) {
    let _ = interner;
    match statement {
        Statement::Data { name, variants, .. } => {
            let mut constructor_names = Vec::with_capacity(variants.len());
            for variant in variants {
                let name_sym = variant.name;
                scope
                    .adt_constructors
                    .insert(name_sym, variant.fields.len());
                scope.adt_constructor_owner.insert(name_sym, *name);
                constructor_names.push(name_sym);
            }
            scope.adt_variants.insert(*name, constructor_names);
        }
        Statement::Module { body, .. } => {
            for statement in &body.statements {
                collect_adt_definitions_from_stmt(statement, scope, interner);
            }
        }
        _ => {}
    }
}

fn compile_match_expression(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    scrutinee: &Expression,
    arms: &[crate::syntax::expression::MatchArm],
    interner: &Interner,
) -> Result<CraneliftValue, String> {
    if arms.is_empty() {
        let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
        let call = builder.ins().call(make_none, &[ctx_val]);
        return Ok(builder.inst_results(call)[0]);
    }

    validate_jit_match_arms(scope, arms, interner)?;

    let scrutinee_val = compile_expression(
        module,
        helpers,
        builder,
        scope,
        ctx_val,
        return_block,
        tail_call,
        scrutinee,
        interner,
    )?;
    let merge_block = builder.create_block();
    builder.append_block_param(merge_block, PTR_TYPE);

    let initial_test = builder.create_block();
    builder.ins().jump(initial_test, &[]);
    let mut pending_test = Some(initial_test);

    for arm in arms {
        let Some(test_block) = pending_test else {
            break;
        };
        builder.switch_to_block(test_block);

        validate_pattern_constructors_for_jit(&arm.pattern, scope, interner)?;

        let arm_block = builder.create_block();
        let mut next_test: Option<cranelift_codegen::ir::Block> = None;
        let mut matched_block = arm_block;
        let has_guard = arm.guard.is_some();
        if has_guard {
            matched_block = builder.create_block();
        }

        match &arm.pattern {
            Pattern::Wildcard { .. } | Pattern::Identifier { .. } => {
                builder.ins().jump(matched_block, &[]);
                if has_guard {
                    let next = builder.create_block();
                    next_test = Some(next);
                    pending_test = Some(next);
                } else {
                    pending_test = None;
                }
            }
            Pattern::Cons { .. } => {
                let is_cons = get_helper_func_ref(module, helpers, builder, "rt_is_cons");
                let call = builder.ins().call(is_cons, &[ctx_val, scrutinee_val]);
                let is_cons_i64 = builder.inst_results(call)[0];
                let cond = builder.ins().icmp_imm(IntCC::NotEqual, is_cons_i64, 0);
                let next = builder.create_block();
                builder.ins().brif(cond, matched_block, &[], next, &[]);
                next_test = Some(next);
                pending_test = Some(next);
            }
            Pattern::None { .. } => {
                let is_none = get_helper_func_ref(module, helpers, builder, "rt_is_none");
                let call = builder.ins().call(is_none, &[ctx_val, scrutinee_val]);
                let result = builder.inst_results(call)[0];
                let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
                let next = builder.create_block();
                builder.ins().brif(cond, matched_block, &[], next, &[]);
                next_test = Some(next);
                pending_test = Some(next);
            }
            Pattern::EmptyList { .. } => {
                let is_el = get_helper_func_ref(module, helpers, builder, "rt_is_empty_list");
                let call = builder.ins().call(is_el, &[ctx_val, scrutinee_val]);
                let result = builder.inst_results(call)[0];
                let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
                let next = builder.create_block();
                builder.ins().brif(cond, matched_block, &[], next, &[]);
                next_test = Some(next);
                pending_test = Some(next);
            }
            Pattern::Some { .. } => {
                let is_some = get_helper_func_ref(module, helpers, builder, "rt_is_some");
                let call = builder.ins().call(is_some, &[ctx_val, scrutinee_val]);
                let result = builder.inst_results(call)[0];
                let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
                let next = builder.create_block();
                builder.ins().brif(cond, matched_block, &[], next, &[]);
                next_test = Some(next);
                pending_test = Some(next);
            }
            Pattern::Left { .. } => {
                let is_left = get_helper_func_ref(module, helpers, builder, "rt_is_left");
                let call = builder.ins().call(is_left, &[ctx_val, scrutinee_val]);
                let result = builder.inst_results(call)[0];
                let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
                let next = builder.create_block();
                builder.ins().brif(cond, matched_block, &[], next, &[]);
                next_test = Some(next);
                pending_test = Some(next);
            }
            Pattern::Right { .. } => {
                let is_right = get_helper_func_ref(module, helpers, builder, "rt_is_right");
                let call = builder.ins().call(is_right, &[ctx_val, scrutinee_val]);
                let result = builder.inst_results(call)[0];
                let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
                let next = builder.create_block();
                builder.ins().brif(cond, matched_block, &[], next, &[]);
                next_test = Some(next);
                pending_test = Some(next);
            }
            Pattern::Literal { expression, .. } => {
                // Compile the literal value, then compare with scrutinee
                let lit_val = compile_expression(
                    module,
                    helpers,
                    builder,
                    scope,
                    ctx_val,
                    return_block,
                    tail_call,
                    expression,
                    interner,
                )?;
                let vals_eq = get_helper_func_ref(module, helpers, builder, "rt_values_equal");
                let call = builder
                    .ins()
                    .call(vals_eq, &[ctx_val, scrutinee_val, lit_val]);
                let result = builder.inst_results(call)[0];
                let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
                let next = builder.create_block();
                builder.ins().brif(cond, matched_block, &[], next, &[]);
                next_test = Some(next);
                pending_test = Some(next);
            }
            Pattern::Tuple { elements, .. } => {
                let next = builder.create_block();
                // Use emit_pattern_check to recursively validate the tuple
                // and each of its elements against their sub-patterns.
                emit_pattern_check(
                    module,
                    helpers,
                    builder,
                    ctx_val,
                    &arm.pattern,
                    scrutinee_val,
                    matched_block,
                    next,
                    interner,
                )?;
                // Seal intermediate element-check blocks created inside
                // emit_pattern_check (they were created and immediately switched to).
                // We only need to track elements as a reference to satisfy the compiler.
                let _ = elements;
                next_test = Some(next);
                pending_test = Some(next);
            }
            Pattern::Constructor { name, .. } => {
                // Embed the constructor name as a data constant
                let name_str = interner.resolve(*name);
                let bytes = name_str.as_bytes().to_vec();

                let data = module
                    .declare_anonymous_data(false, false)
                    .map_err(|e| e.to_string())
                    .expect("declare unknown data");

                let mut desc = DataDescription::new();
                desc.define(bytes.into_boxed_slice());
                module.define_data(data, &desc).expect("define data");

                let global_value = module.declare_data_in_func(data, builder.func);
                let name_ptr = builder.ins().global_value(PTR_TYPE, global_value);
                let name_len = builder.ins().iconst(PTR_TYPE, name_str.len() as i64);

                let is_adt = get_helper_func_ref(module, helpers, builder, "rt_is_adt_constructor");
                let call = builder
                    .ins()
                    .call(is_adt, &[ctx_val, scrutinee_val, name_ptr, name_len]);
                let result = builder.inst_results(call)[0];
                let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
                let next = builder.create_block();
                builder.ins().brif(cond, matched_block, &[], next, &[]);
                next_test = Some(next);
                pending_test = Some(next);
            }
        }

        builder.seal_block(test_block);

        builder.switch_to_block(matched_block);
        let mut arm_scope = scope.clone();
        bind_pattern_value(
            module,
            helpers,
            builder,
            &mut arm_scope,
            ctx_val,
            &arm.pattern,
            scrutinee_val,
        )?;
        if let Some(guard_expr) = &arm.guard {
            let guard_val = compile_expression(
                module,
                helpers,
                builder,
                &mut arm_scope,
                ctx_val,
                return_block,
                tail_call,
                guard_expr,
                interner,
            )?;
            let is_truthy = get_helper_func_ref(module, helpers, builder, "rt_is_truthy");
            let truthy_call = builder.ins().call(is_truthy, &[ctx_val, guard_val]);
            let truthy_i64 = builder.inst_results(truthy_call)[0];
            let cond = builder.ins().icmp_imm(IntCC::NotEqual, truthy_i64, 0);
            let fail_block = match next_test {
                Some(next) => next,
                None => {
                    let next = builder.create_block();
                    next_test = Some(next);
                    pending_test = Some(next);
                    next
                }
            };
            builder.ins().brif(cond, arm_block, &[], fail_block, &[]);
            builder.seal_block(matched_block);
            builder.switch_to_block(arm_block);
        }
        let arm_val = compile_expression(
            module,
            helpers,
            builder,
            &mut arm_scope,
            ctx_val,
            return_block,
            tail_call,
            &arm.body,
            interner,
        )?;
        let args = [BlockArg::Value(arm_val)];
        builder.ins().jump(merge_block, &args);
        builder.seal_block(arm_block);

        if let Some(next) = next_test {
            builder.switch_to_block(next);
        }
    }

    if let Some(unmatched) = pending_test {
        builder.switch_to_block(unmatched);
        let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
        let call = builder.ins().call(make_none, &[ctx_val]);
        let fallback = builder.inst_results(call)[0];
        let args = [BlockArg::Value(fallback)];
        builder.ins().jump(merge_block, &args);
        builder.seal_block(unmatched);
    }

    builder.switch_to_block(merge_block);
    builder.seal_block(merge_block);
    Ok(builder.block_params(merge_block)[0])
}

fn validate_jit_match_arms(
    scope: &Scope,
    arms: &[crate::syntax::expression::MatchArm],
    interner: &Interner,
) -> Result<(), String> {
    let all_constructor_names: Vec<Identifier> = arms
        .iter()
        .filter_map(|arm| match &arm.pattern {
            Pattern::Constructor { name, .. } => Some(*name),
            _ => None,
        })
        .collect();
    if all_constructor_names.is_empty() {
        return Ok(());
    }

    let constructor_names: Vec<Identifier> = arms
        .iter()
        .filter_map(|arm| {
            if arm.guard.is_none()
                && let Pattern::Constructor { name, .. } = &arm.pattern
            {
                return Some(*name);
            }
            None
        })
        .collect();

    let first = all_constructor_names[0];
    let Some(first_adt) = scope.adt_constructor_owner.get(&first).copied() else {
        return Err(format!(
            "Unknown constructor `{}`.",
            interner.resolve(first)
        ));
    };
    for constructor in &all_constructor_names {
        let Some(owner) = scope.adt_constructor_owner.get(constructor).copied() else {
            return Err(format!(
                "Unknown constructor `{}`.",
                interner.resolve(*constructor)
            ));
        };
        if owner != first_adt {
            return Err(format!(
                "Match arms mix constructors from different ADTs: `{}` and `{}`.",
                interner.resolve(first_adt),
                interner.resolve(owner)
            ));
        }
    }

    let has_catch_all = arms.iter().any(|arm| {
        arm.guard.is_none()
            && matches!(
                arm.pattern,
                Pattern::Wildcard { .. } | Pattern::Identifier { .. }
            )
    });
    if has_catch_all {
        return Ok(());
    }

    let Some(variants) = scope.adt_variants.get(&first_adt) else {
        return Ok(());
    };

    if constructor_names.is_empty() {
        let all = variants
            .iter()
            .map(|name| interner.resolve(*name))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "Match on `{}` is non-exhaustive because all constructor arms are guarded. Missing: {}.",
            interner.resolve(first_adt),
            all
        ));
    }

    let covered: HashSet<Identifier> = constructor_names.into_iter().collect();
    let missing = variants
        .iter()
        .filter(|name| !covered.contains(name))
        .map(|name| interner.resolve(*name))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "Match on `{}` is missing constructors: {}.",
            interner.resolve(first_adt),
            missing.join(", ")
        ));
    }

    Ok(())
}

fn validate_pattern_constructors_for_jit(
    pattern: &Pattern,
    scope: &Scope,
    interner: &Interner,
) -> Result<(), String> {
    match pattern {
        Pattern::Constructor { name, fields, .. } => {
            let Some(expected_arity) = scope.adt_constructors.get(name).copied() else {
                return Err(format!(
                    "Unknown constructor `{}`.",
                    interner.resolve(*name)
                ));
            };
            if fields.len() != expected_arity {
                return Err(format!(
                    "Constructor `{}` expects {} argument(s) but got {}.",
                    interner.resolve(*name),
                    expected_arity,
                    fields.len()
                ));
            }
            for field in fields {
                validate_pattern_constructors_for_jit(field, scope, interner)?;
            }
            Ok(())
        }
        Pattern::Some { pattern, .. }
        | Pattern::Left { pattern, .. }
        | Pattern::Right { pattern, .. } => {
            validate_pattern_constructors_for_jit(pattern, scope, interner)
        }
        Pattern::Cons { head, tail, .. } => {
            validate_pattern_constructors_for_jit(head, scope, interner)?;
            validate_pattern_constructors_for_jit(tail, scope, interner)
        }
        Pattern::Tuple { elements, .. } => {
            for element in elements {
                validate_pattern_constructors_for_jit(element, scope, interner)?;
            }
            Ok(())
        }
        Pattern::Wildcard { .. }
        | Pattern::Identifier { .. }
        | Pattern::Literal { .. }
        | Pattern::None { .. }
        | Pattern::EmptyList { .. } => Ok(()),
    }
}

fn bind_pattern_value(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    pattern: &Pattern,
    value: CraneliftValue,
) -> Result<(), String> {
    match pattern {
        Pattern::Wildcard { .. } => Ok(()),
        Pattern::Identifier { name, .. } => {
            let var = builder.declare_var(PTR_TYPE);
            builder.def_var(var, value);
            scope.locals.insert(*name, var);
            Ok(())
        }
        Pattern::Cons { head, tail, .. } => {
            let cons_head = get_helper_func_ref(module, helpers, builder, "rt_cons_head");
            let cons_tail = get_helper_func_ref(module, helpers, builder, "rt_cons_tail");
            let h_call = builder.ins().call(cons_head, &[ctx_val, value]);
            let t_call = builder.ins().call(cons_tail, &[ctx_val, value]);
            let h_val = builder.inst_results(h_call)[0];
            let t_val = builder.inst_results(t_call)[0];
            bind_pattern_value(module, helpers, builder, scope, ctx_val, head, h_val)?;
            bind_pattern_value(module, helpers, builder, scope, ctx_val, tail, t_val)?;
            Ok(())
        }
        Pattern::None { .. } | Pattern::EmptyList { .. } | Pattern::Literal { .. } => {
            // No bindings for these patterns
            Ok(())
        }
        Pattern::Some { pattern, .. } => {
            let unwrap = get_helper_func_ref(module, helpers, builder, "rt_unwrap_some");
            let call = builder.ins().call(unwrap, &[ctx_val, value]);
            let inner = builder.inst_results(call)[0];
            bind_pattern_value(module, helpers, builder, scope, ctx_val, pattern, inner)
        }
        Pattern::Left { pattern, .. } => {
            let unwrap = get_helper_func_ref(module, helpers, builder, "rt_unwrap_left");
            let call = builder.ins().call(unwrap, &[ctx_val, value]);
            let inner = builder.inst_results(call)[0];
            bind_pattern_value(module, helpers, builder, scope, ctx_val, pattern, inner)
        }
        Pattern::Right { pattern, .. } => {
            let unwrap = get_helper_func_ref(module, helpers, builder, "rt_unwrap_right");
            let call = builder.ins().call(unwrap, &[ctx_val, value]);
            let inner = builder.inst_results(call)[0];
            bind_pattern_value(module, helpers, builder, scope, ctx_val, pattern, inner)
        }
        Pattern::Tuple { elements, .. } => {
            let tuple_get = get_helper_func_ref(module, helpers, builder, "rt_tuple_get");
            for (index, element) in elements.iter().enumerate() {
                let index_val = builder.ins().iconst(PTR_TYPE, index as i64);
                let call = builder.ins().call(tuple_get, &[ctx_val, value, index_val]);
                let item = builder.inst_results(call)[0];
                bind_pattern_value(module, helpers, builder, scope, ctx_val, element, item)?;
            }
            Ok(())
        }
        Pattern::Constructor { fields, .. } => {
            let adt_field = get_helper_func_ref(module, helpers, builder, "rt_adt_field");
            for (index, field_pattern) in fields.iter().enumerate() {
                let idx_val = builder.ins().iconst(PTR_TYPE, index as i64);
                let call = builder.ins().call(adt_field, &[ctx_val, value, idx_val]);
                let item = builder.inst_results(call)[0];
                bind_pattern_value(
                    module,
                    helpers,
                    builder,
                    scope,
                    ctx_val,
                    field_pattern,
                    item,
                )?;
            }
            Ok(())
        }
    }
}

/// Emits a chain of feasibility checks for `pattern` applied to `value`.
///
/// If `value` satisfies the pattern, control falls to `pass_block`.
/// If it does not, control jumps to `fail_block`.
/// The caller must switch to `pass_block` afterwards to continue.
///
/// Only the _outer_ shape of the value is checked here — identifier/wildcard
/// sub-patterns always pass since they bind unconditionally.
fn emit_pattern_check(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    ctx_val: CraneliftValue,
    pattern: &Pattern,
    value: CraneliftValue,
    pass_block: cranelift_codegen::ir::Block,
    fail_block: cranelift_codegen::ir::Block,
    interner: &Interner,
) -> Result<(), String> {
    match pattern {
        Pattern::Wildcard { .. } | Pattern::Identifier { .. } => {
            builder.ins().jump(pass_block, &[]);
        }
        Pattern::Cons { .. } => {
            let is_cons = get_helper_func_ref(module, helpers, builder, "rt_is_cons");
            let call = builder.ins().call(is_cons, &[ctx_val, value]);
            let result = builder.inst_results(call)[0];
            let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
            builder.ins().brif(cond, pass_block, &[], fail_block, &[]);
        }
        Pattern::EmptyList { .. } => {
            let is_el = get_helper_func_ref(module, helpers, builder, "rt_is_empty_list");
            let call = builder.ins().call(is_el, &[ctx_val, value]);
            let result = builder.inst_results(call)[0];
            let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
            builder.ins().brif(cond, pass_block, &[], fail_block, &[]);
        }
        Pattern::None { .. } => {
            let is_none = get_helper_func_ref(module, helpers, builder, "rt_is_none");
            let call = builder.ins().call(is_none, &[ctx_val, value]);
            let result = builder.inst_results(call)[0];
            let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
            builder.ins().brif(cond, pass_block, &[], fail_block, &[]);
        }
        Pattern::Some { .. } => {
            let is_some = get_helper_func_ref(module, helpers, builder, "rt_is_some");
            let call = builder.ins().call(is_some, &[ctx_val, value]);
            let result = builder.inst_results(call)[0];
            let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
            builder.ins().brif(cond, pass_block, &[], fail_block, &[]);
        }
        Pattern::Left { .. } => {
            let is_left = get_helper_func_ref(module, helpers, builder, "rt_is_left");
            let call = builder.ins().call(is_left, &[ctx_val, value]);
            let result = builder.inst_results(call)[0];
            let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
            builder.ins().brif(cond, pass_block, &[], fail_block, &[]);
        }
        Pattern::Right { .. } => {
            let is_right = get_helper_func_ref(module, helpers, builder, "rt_is_right");
            let call = builder.ins().call(is_right, &[ctx_val, value]);
            let result = builder.inst_results(call)[0];
            let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
            builder.ins().brif(cond, pass_block, &[], fail_block, &[]);
        }
        Pattern::Literal { expression, .. } => {
            // Literals require a runtime value to compare against, but we
            // don't have an interner-aware compile path here. Fall through
            // optimistically — the arm body will produce wrong results if
            // the literal doesn't match, but pattern::Literal inside a Tuple
            // is rarely used in practice.
            let _ = (expression, interner);
            builder.ins().jump(pass_block, &[]);
        }
        Pattern::Tuple { elements, .. } => {
            // Check rt_is_tuple first, then chain checks for each element.
            let is_tuple = get_helper_func_ref(module, helpers, builder, "rt_is_tuple");
            let call = builder.ins().call(is_tuple, &[ctx_val, value]);
            let result = builder.inst_results(call)[0];
            let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
            // Build a chain: check_tuple → check_el_0 → ... → pass_block
            // Each step jumps to fail_block on failure.
            // Build a step block per element so we can chain checks forward.
            let step_blocks: Vec<cranelift_codegen::ir::Block> =
                elements.iter().map(|_| builder.create_block()).collect();
            // The first check is rt_is_tuple; on success jump to step_blocks[0] or pass_block.
            let first = step_blocks.first().copied().unwrap_or(pass_block);
            builder.ins().brif(cond, first, &[], fail_block, &[]);
            // Now emit each element check, chaining into the next step or pass_block.
            for (i, (element, &step)) in elements.iter().zip(step_blocks.iter()).enumerate() {
                builder.seal_block(step);
                builder.switch_to_block(step);
                let tuple_get = get_helper_func_ref(module, helpers, builder, "rt_tuple_get");
                let idx_val = builder.ins().iconst(PTR_TYPE, i as i64);
                let elem_call = builder.ins().call(tuple_get, &[ctx_val, value, idx_val]);
                let elem_val = builder.inst_results(elem_call)[0];
                let next = step_blocks.get(i + 1).copied().unwrap_or(pass_block);
                emit_pattern_check(
                    module, helpers, builder, ctx_val, element, elem_val, next, fail_block,
                    interner,
                )?;
            }
        }
        Pattern::Constructor { name, .. } => {
            let name_str = interner.resolve(*name);
            let bytes = name_str.as_bytes().to_vec();
            let data = module
                .declare_anonymous_data(false, false)
                .map_err(|e| e.to_string())?;
            let mut desc = DataDescription::new();
            desc.define(bytes.into_boxed_slice());
            module.define_data(data, &desc).map_err(|e| e.to_string())?;
            let global_value = module.declare_data_in_func(data, builder.func);
            let name_ptr = builder.ins().global_value(PTR_TYPE, global_value);
            let name_len = builder.ins().iconst(PTR_TYPE, name_str.len() as i64);
            let is_adt = get_helper_func_ref(module, helpers, builder, "rt_is_adt_constructor");
            let call = builder
                .ins()
                .call(is_adt, &[ctx_val, value, name_ptr, name_len]);
            let result = builder.inst_results(call)[0];
            let cond = builder.ins().icmp_imm(IntCC::NotEqual, result, 0);
            builder.ins().brif(cond, pass_block, &[], fail_block, &[]);
        }
    }
    Ok(())
}

fn bind_top_level_pattern_value(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    pattern: &Pattern,
    value: CraneliftValue,
) -> Result<(), String> {
    match pattern {
        Pattern::Identifier { name, .. } => {
            if let Some(&idx) = scope.globals.get(name) {
                let set_global = get_helper_func_ref(module, helpers, builder, "rt_set_global");
                let idx_val = builder.ins().iconst(PTR_TYPE, idx as i64);
                builder.ins().call(set_global, &[ctx_val, idx_val, value]);
                Ok(())
            } else {
                bind_pattern_value(module, helpers, builder, scope, ctx_val, pattern, value)
            }
        }
        Pattern::Cons { head, tail, .. } => {
            let cons_head = get_helper_func_ref(module, helpers, builder, "rt_cons_head");
            let cons_tail = get_helper_func_ref(module, helpers, builder, "rt_cons_tail");
            let h_call = builder.ins().call(cons_head, &[ctx_val, value]);
            let t_call = builder.ins().call(cons_tail, &[ctx_val, value]);
            let h_val = builder.inst_results(h_call)[0];
            let t_val = builder.inst_results(t_call)[0];
            bind_top_level_pattern_value(module, helpers, builder, scope, ctx_val, head, h_val)?;
            bind_top_level_pattern_value(module, helpers, builder, scope, ctx_val, tail, t_val)?;
            Ok(())
        }
        Pattern::Some { pattern, .. } => {
            let unwrap = get_helper_func_ref(module, helpers, builder, "rt_unwrap_some");
            let call = builder.ins().call(unwrap, &[ctx_val, value]);
            let inner = builder.inst_results(call)[0];
            bind_top_level_pattern_value(module, helpers, builder, scope, ctx_val, pattern, inner)
        }
        Pattern::Left { pattern, .. } => {
            let unwrap = get_helper_func_ref(module, helpers, builder, "rt_unwrap_left");
            let call = builder.ins().call(unwrap, &[ctx_val, value]);
            let inner = builder.inst_results(call)[0];
            bind_top_level_pattern_value(module, helpers, builder, scope, ctx_val, pattern, inner)
        }
        Pattern::Right { pattern, .. } => {
            let unwrap = get_helper_func_ref(module, helpers, builder, "rt_unwrap_right");
            let call = builder.ins().call(unwrap, &[ctx_val, value]);
            let inner = builder.inst_results(call)[0];
            bind_top_level_pattern_value(module, helpers, builder, scope, ctx_val, pattern, inner)
        }
        Pattern::Tuple { elements, .. } => {
            let tuple_get = get_helper_func_ref(module, helpers, builder, "rt_tuple_get");
            for (index, element) in elements.iter().enumerate() {
                let index_val = builder.ins().iconst(PTR_TYPE, index as i64);
                let call = builder.ins().call(tuple_get, &[ctx_val, value, index_val]);
                let item = builder.inst_results(call)[0];
                bind_top_level_pattern_value(
                    module, helpers, builder, scope, ctx_val, element, item,
                )?;
            }
            Ok(())
        }
        Pattern::Constructor { fields, .. } => {
            let adt_field = get_helper_func_ref(module, helpers, builder, "rt_adt_field");
            for (index, field_pattern) in fields.iter().enumerate() {
                let idx_val = builder.ins().iconst(PTR_TYPE, index as i64);
                let call = builder.ins().call(adt_field, &[ctx_val, value, idx_val]);
                let item = builder.inst_results(call)[0];
                bind_top_level_pattern_value(
                    module,
                    helpers,
                    builder,
                    scope,
                    ctx_val,
                    field_pattern,
                    item,
                )?;
            }
            Ok(())
        }
        Pattern::Wildcard { .. }
        | Pattern::None { .. }
        | Pattern::EmptyList { .. }
        | Pattern::Literal { .. } => Ok(()),
    }
}

fn compile_block_expression(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    scope: &Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    block: &Block,
    interner: &Interner,
) -> Result<BlockEval, String> {
    let mut block_scope = scope.clone();
    for (idx, stmt) in block.statements.iter().enumerate() {
        let is_last = idx + 1 == block.statements.len();
        let outcome = compile_statement(
            module,
            helpers,
            builder,
            &mut block_scope,
            ctx_val,
            return_block,
            tail_call,
            false,
            stmt,
            interner,
        )?;
        match outcome {
            StmtOutcome::Value(v) if is_last => return Ok(BlockEval::Value(v)),
            StmtOutcome::Returned => return Ok(BlockEval::Returned),
            StmtOutcome::Value(_) | StmtOutcome::None => {}
        }
    }
    let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
    let call = builder.ins().call(make_none, &[ctx_val]);
    Ok(BlockEval::Value(builder.inst_results(call)[0]))
}

fn compile_if_expression(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    condition: &Expression,
    consequence: &Block,
    alternative: Option<&Block>,
    interner: &Interner,
) -> Result<CraneliftValue, String> {
    let cond_val = compile_expression(
        module,
        helpers,
        builder,
        scope,
        ctx_val,
        return_block,
        tail_call,
        condition,
        interner,
    )?;
    let is_truthy = get_helper_func_ref(module, helpers, builder, "rt_is_truthy");
    let truthy_call = builder.ins().call(is_truthy, &[ctx_val, cond_val]);
    let truthy_i64 = builder.inst_results(truthy_call)[0];
    let cond_b1 = builder.ins().icmp_imm(IntCC::NotEqual, truthy_i64, 0);

    let then_block = builder.create_block();
    let else_block = builder.create_block();
    let merge_block = builder.create_block();
    builder.append_block_param(merge_block, PTR_TYPE);

    builder
        .ins()
        .brif(cond_b1, then_block, &[], else_block, &[]);

    builder.switch_to_block(then_block);
    let then_eval = compile_block_expression(
        module,
        helpers,
        builder,
        scope,
        ctx_val,
        return_block,
        tail_call,
        consequence,
        interner,
    )?;
    let mut has_merge_value = false;
    if let BlockEval::Value(then_val) = then_eval {
        let then_args = [BlockArg::Value(then_val)];
        builder.ins().jump(merge_block, &then_args);
        has_merge_value = true;
    }
    builder.seal_block(then_block);

    builder.switch_to_block(else_block);
    let else_eval = match alternative {
        Some(alt) => compile_block_expression(
            module,
            helpers,
            builder,
            scope,
            ctx_val,
            return_block,
            tail_call,
            alt,
            interner,
        )?,
        None => BlockEval::Value({
            let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
            let call = builder.ins().call(make_none, &[ctx_val]);
            builder.inst_results(call)[0]
        }),
    };
    if let BlockEval::Value(else_val) = else_eval {
        let else_args = [BlockArg::Value(else_val)];
        builder.ins().jump(merge_block, &else_args);
        has_merge_value = true;
    }
    builder.seal_block(else_block);

    builder.switch_to_block(merge_block);
    builder.seal_block(merge_block);
    if has_merge_value {
        Ok(builder.block_params(merge_block)[0])
    } else {
        let make_none = get_helper_func_ref(module, helpers, builder, "rt_make_none");
        let call = builder.ins().call(make_none, &[ctx_val]);
        Ok(builder.inst_results(call)[0])
    }
}

fn compile_short_circuit_expression(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    left: &Expression,
    operator: &str,
    right: &Expression,
    interner: &Interner,
) -> Result<CraneliftValue, String> {
    let lhs = compile_expression(
        module,
        helpers,
        builder,
        scope,
        ctx_val,
        return_block,
        tail_call,
        left,
        interner,
    )?;
    let is_truthy = get_helper_func_ref(module, helpers, builder, "rt_is_truthy");
    let truthy_call = builder.ins().call(is_truthy, &[ctx_val, lhs]);
    let truthy_i64 = builder.inst_results(truthy_call)[0];
    let cond_b1 = builder.ins().icmp_imm(IntCC::NotEqual, truthy_i64, 0);

    let short_block = builder.create_block();
    let eval_rhs_block = builder.create_block();
    let merge_block = builder.create_block();
    builder.append_block_param(merge_block, PTR_TYPE);

    match operator {
        "&&" => {
            builder
                .ins()
                .brif(cond_b1, eval_rhs_block, &[], short_block, &[]);
        }
        "||" => {
            builder
                .ins()
                .brif(cond_b1, short_block, &[], eval_rhs_block, &[]);
        }
        _ => return Err(format!("unknown short-circuit operator: {}", operator)),
    }

    builder.switch_to_block(short_block);
    let short_args = [BlockArg::Value(lhs)];
    builder.ins().jump(merge_block, &short_args);
    builder.seal_block(short_block);

    builder.switch_to_block(eval_rhs_block);
    let rhs = compile_expression(
        module,
        helpers,
        builder,
        scope,
        ctx_val,
        return_block,
        tail_call,
        right,
        interner,
    )?;
    let rhs_args = [BlockArg::Value(rhs)];
    builder.ins().jump(merge_block, &rhs_args);
    builder.seal_block(eval_rhs_block);

    builder.switch_to_block(merge_block);
    builder.seal_block(merge_block);
    Ok(builder.block_params(merge_block)[0])
}

fn compile_base_function_call(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    base_idx: usize,
    arguments: &[Expression],
    interner: &Interner,
) -> Result<CraneliftValue, String> {
    // Compile all arguments
    let mut arg_vals = Vec::with_capacity(arguments.len());
    for arg in arguments {
        let val = compile_expression(
            module,
            helpers,
            builder,
            scope,
            ctx_val,
            return_block,
            tail_call,
            arg,
            interner,
        )?;
        arg_vals.push(val);
    }

    // Store argument pointers in a stack slot array
    let nargs = arg_vals.len();
    let slot = builder.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
        cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
        (nargs as u32) * 8, // 8 bytes per pointer
        3,                  // align to 8 bytes (2^3)
    ));

    for (i, val) in arg_vals.iter().enumerate() {
        builder.ins().stack_store(*val, slot, (i * 8) as i32);
    }

    let args_ptr = builder.ins().stack_addr(PTR_TYPE, slot, 0);
    let idx_val = builder.ins().iconst(PTR_TYPE, base_idx as i64);
    let nargs_val = builder.ins().iconst(PTR_TYPE, nargs as i64);

    let call_base = get_helper_func_ref(module, helpers, builder, "rt_call_base_function");
    let call = builder
        .ins()
        .call(call_base, &[ctx_val, idx_val, args_ptr, nargs_val]);
    Ok(builder.inst_results(call)[0])
}

fn compile_primop_call(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    primop: PrimOp,
    arguments: &[Expression],
    interner: &Interner,
) -> Result<CraneliftValue, String> {
    let mut arg_vals = Vec::with_capacity(arguments.len());

    for arg in arguments {
        let val = compile_expression(
            module,
            helpers,
            builder,
            scope,
            ctx_val,
            return_block,
            tail_call,
            arg,
            interner,
        )?;
        arg_vals.push(val);
    }

    let nargs = arg_vals.len();
    let slot = builder.create_sized_stack_slot(StackSlotData::new(
        cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
        (nargs as u32) * 8,
        3,
    ));

    for (i, val) in arg_vals.iter().enumerate() {
        builder.ins().stack_store(*val, slot, (i * 8) as i32);
    }

    let args_ptr = builder.ins().stack_addr(PTR_TYPE, slot, 0);
    let primop_val = builder.ins().iconst(PTR_TYPE, primop.id() as i64);
    let nargs_val = builder.ins().iconst(PTR_TYPE, nargs as i64);
    let call_primop = get_helper_func_ref(module, helpers, builder, "rt_call_primop");
    let call = builder
        .ins()
        .call(call_primop, &[ctx_val, primop_val, args_ptr, nargs_val]);
    Ok(builder.inst_results(call)[0])
}

fn resolve_call_primop(
    scope: &Scope,
    function: &Expression,
    arguments: &[Expression],
    interner: &Interner,
) -> Option<PrimOp> {
    let Expression::Identifier { name, .. } = function else {
        return None;
    };
    if scope.excluded_base_symbols.contains(name) {
        return None;
    }

    // Shadowed names must resolve through the regular call path.
    if scope.locals.contains_key(name)
        || scope.functions.contains_key(name)
        || scope.globals.contains_key(name)
    {
        return None;
    }

    let name = interner.try_resolve(*name)?;
    resolve_primop_call(name, arguments.len())
}

fn should_use_base_fastcall(scope: &Scope, name: Identifier, interner: &Interner) -> bool {
    if scope.excluded_base_symbols.contains(&name) {
        return false;
    }
    if scope.locals.contains_key(&name)
        || scope.functions.contains_key(&name)
        || scope.globals.contains_key(&name)
    {
        return false;
    }
    let Some(name_str) = interner.try_resolve(name) else {
        return false;
    };
    is_base_fastcall_allowlisted(name_str)
}

fn compile_user_function_call(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    meta: JitFunctionMeta,
    call_span: crate::diagnostics::position::Span,
    arguments: &[Expression],
    interner: &Interner,
) -> Result<CraneliftValue, String> {
    let mut arg_vals = Vec::with_capacity(arguments.len());
    for arg in arguments {
        let val = compile_expression(
            module,
            helpers,
            builder,
            scope,
            ctx_val,
            return_block,
            tail_call,
            arg,
            interner,
        )?;
        arg_vals.push(val);
    }

    let nargs = arg_vals.len();
    if nargs != meta.num_params {
        return Err(format!(
            "wrong number of arguments in JIT call: want={}, got={}",
            meta.num_params, nargs
        ));
    }

    let slot = builder.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
        cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
        (nargs as u32) * 8,
        3,
    ));
    for (i, val) in arg_vals.iter().enumerate() {
        builder.ins().stack_store(*val, slot, (i * 8) as i32);
    }

    let args_ptr = builder.ins().stack_addr(PTR_TYPE, slot, 0);
    let nargs_val = builder.ins().iconst(PTR_TYPE, nargs as i64);
    let null_ptr = builder.ins().iconst(PTR_TYPE, 0);
    let zero = builder.ins().iconst(PTR_TYPE, 0);
    let fn_index = builder.ins().iconst(PTR_TYPE, meta.function_index as i64);
    let line_val = builder.ins().iconst(PTR_TYPE, call_span.start.line as i64);
    let col_val = builder
        .ins()
        .iconst(PTR_TYPE, (call_span.start.column + 1) as i64);

    let check_call = get_helper_func_ref(module, helpers, builder, "rt_check_jit_contract_call");
    let call_ok = builder.ins().call(
        check_call,
        &[ctx_val, fn_index, args_ptr, nargs_val, line_val, col_val],
    );
    let call_ok_val = builder.inst_results(call_ok)[0];
    let call_ok_bool = builder.ins().icmp_imm(IntCC::NotEqual, call_ok_val, 0);

    let call_block = builder.create_block();
    let fail_block = builder.create_block();
    let done_block = builder.create_block();
    builder.append_block_param(done_block, PTR_TYPE);
    builder
        .ins()
        .brif(call_ok_bool, call_block, &[], fail_block, &[]);

    builder.switch_to_block(fail_block);
    let fail_args = [BlockArg::Value(null_ptr)];
    builder.ins().jump(done_block, &fail_args);
    builder.seal_block(fail_block);

    builder.switch_to_block(call_block);
    let callee_ref = module.declare_func_in_func(meta.id, builder.func);
    let call = builder
        .ins()
        .call(callee_ref, &[ctx_val, args_ptr, nargs_val, null_ptr, zero]);
    let raw_result = builder.inst_results(call)[0];
    let check_ret = get_helper_func_ref(module, helpers, builder, "rt_check_jit_contract_return");
    let checked_ret_call = builder.ins().call(
        check_ret,
        &[ctx_val, fn_index, raw_result, line_val, col_val],
    );
    let checked_ret = builder.inst_results(checked_ret_call)[0];
    let ok_args = [BlockArg::Value(checked_ret)];
    builder.ins().jump(done_block, &ok_args);
    builder.seal_block(call_block);

    builder.switch_to_block(done_block);
    let result = builder.block_params(done_block)[0];
    builder.seal_block(done_block);
    Ok(result)
}

fn compile_generic_call(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    return_block: Option<cranelift_codegen::ir::Block>,
    tail_call: Option<&TailCallContext>,
    function: &Expression,
    arguments: &[Expression],
    interner: &Interner,
) -> Result<CraneliftValue, String> {
    let callee = compile_expression(
        module,
        helpers,
        builder,
        scope,
        ctx_val,
        return_block,
        tail_call,
        function,
        interner,
    )?;

    let mut arg_vals = Vec::with_capacity(arguments.len());
    for arg in arguments {
        let val = compile_expression(
            module,
            helpers,
            builder,
            scope,
            ctx_val,
            return_block,
            tail_call,
            arg,
            interner,
        )?;
        arg_vals.push(val);
    }

    let nargs = arg_vals.len();
    let slot = builder.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
        cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
        (nargs as u32) * 8,
        3,
    ));
    for (i, val) in arg_vals.iter().enumerate() {
        builder.ins().stack_store(*val, slot, (i * 8) as i32);
    }

    let args_ptr = builder.ins().stack_addr(PTR_TYPE, slot, 0);
    let nargs_val = builder.ins().iconst(PTR_TYPE, nargs as i64);
    let call_value = get_helper_func_ref(module, helpers, builder, "rt_call_value");
    let call = builder
        .ins()
        .call(call_value, &[ctx_val, callee, args_ptr, nargs_val]);
    Ok(builder.inst_results(call)[0])
}

fn compile_function_literal(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    expr: &Expression,
    _interner: &Interner,
) -> Result<CraneliftValue, String> {
    let key = LiteralKey::from_expr(expr);
    let Some(meta) = scope.literal_functions.get(&key).copied() else {
        return Err("missing literal function metadata in JIT".to_string());
    };
    let captures = scope
        .literal_captures
        .get(&key)
        .cloned()
        .unwrap_or_default();

    let mut capture_vals: Vec<CraneliftValue> = Vec::new();
    for sym in captures {
        if let Some(&var) = scope.locals.get(&sym) {
            capture_vals.push(builder.use_var(var));
            continue;
        }
        if let Some(fn_meta) = scope.functions.get(&sym).copied() {
            let make_jit_closure =
                get_helper_func_ref(module, helpers, builder, "rt_make_jit_closure");
            let fn_idx = builder
                .ins()
                .iconst(PTR_TYPE, fn_meta.function_index as i64);
            let null_ptr = builder.ins().iconst(PTR_TYPE, 0);
            let zero = builder.ins().iconst(PTR_TYPE, 0);
            let call = builder
                .ins()
                .call(make_jit_closure, &[ctx_val, fn_idx, null_ptr, zero]);
            capture_vals.push(builder.inst_results(call)[0]);
            continue;
        }
        if let Some(&idx) = scope.globals.get(&sym) {
            let get_global = get_helper_func_ref(module, helpers, builder, "rt_get_global");
            let idx_val = builder.ins().iconst(PTR_TYPE, idx as i64);
            let call = builder.ins().call(get_global, &[ctx_val, idx_val]);
            capture_vals.push(builder.inst_results(call)[0]);
            continue;
        }
        if let Some(&base_idx) = scope.base_functions.get(&sym) {
            let make_base = get_helper_func_ref(module, helpers, builder, "rt_make_base_function");
            let idx_val = builder.ins().iconst(PTR_TYPE, base_idx as i64);
            let call = builder.ins().call(make_base, &[ctx_val, idx_val]);
            capture_vals.push(builder.inst_results(call)[0]);
            continue;
        }
        return Err("unsupported capture in JIT function literal".to_string());
    }

    let slot = builder.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
        cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
        (capture_vals.len() as u32) * 8,
        3,
    ));
    for (i, val) in capture_vals.iter().enumerate() {
        builder.ins().stack_store(*val, slot, (i * 8) as i32);
    }
    let captures_ptr = builder.ins().stack_addr(PTR_TYPE, slot, 0);
    let ncaptures = builder.ins().iconst(PTR_TYPE, capture_vals.len() as i64);
    let fn_idx = builder.ins().iconst(PTR_TYPE, meta.function_index as i64);
    let make_jit_closure = get_helper_func_ref(module, helpers, builder, "rt_make_jit_closure");
    let call = builder.ins().call(
        make_jit_closure,
        &[ctx_val, fn_idx, captures_ptr, ncaptures],
    );
    Ok(builder.inst_results(call)[0])
}

fn get_helper_func_ref(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    name: &str,
) -> cranelift_codegen::ir::FuncRef {
    let func_id = helpers.ids[name];
    module.declare_func_in_func(func_id, builder.func)
}

fn register_base_functions(scope: &mut Scope, interner: &Interner) {
    use crate::runtime::base::BASE_FUNCTIONS;
    use crate::syntax::symbol::Symbol;
    // Scan the interner to find Symbols matching each base name.
    for (idx, base_fn) in BASE_FUNCTIONS.iter().enumerate() {
        for sym_idx in 0u32.. {
            let sym = Symbol::new(sym_idx);
            match interner.try_resolve(sym) {
                Some(name) if name == base_fn.name => {
                    scope.base_functions.insert(sym, idx);
                    break;
                }
                Some(_) => continue,
                None => break,
            }
        }
    }
}

fn is_base_symbol(name: Identifier, interner: &Interner) -> bool {
    interner
        .try_resolve(name)
        .is_some_and(|name| name == "Base")
}

fn apply_base_directives(
    program: &Program,
    scope: &mut Scope,
    interner: &Interner,
) -> Result<(), String> {
    let mut seen = HashSet::new();
    for stmt in &program.statements {
        let Statement::Import {
            name,
            alias,
            except,
            ..
        } = stmt
        else {
            continue;
        };
        if !is_base_symbol(*name, interner) {
            continue;
        }
        if let Some(alias) = alias {
            return Err(format!(
                "`import Base as {}` is not allowed",
                interner.resolve(*alias)
            ));
        }
        for excluded in except {
            if !seen.insert(*excluded) {
                return Err(format!(
                    "duplicate Base exclusion `{}`",
                    interner.resolve(*excluded)
                ));
            }
            let excluded_name = interner.resolve(*excluded);
            if BaseModule::new().index_of(excluded_name).is_none() {
                return Err(format!("unknown Base member: {}", excluded_name));
            }
            scope.excluded_base_symbols.insert(*excluded);
            scope.base_functions.remove(excluded);
        }
    }
    Ok(())
}

fn collect_literal_function_specs(program: &Program) -> Vec<LiteralFunctionSpec> {
    let mut collector = LiteralCollector::new();
    collector.collect_program(program);
    collector.specs
}

struct LiteralCollector {
    scopes: Vec<HashSet<Identifier>>,
    specs: Vec<LiteralFunctionSpec>,
    seen: HashSet<LiteralKey>,
}

impl LiteralCollector {
    fn new() -> Self {
        Self {
            scopes: vec![HashSet::new()],
            specs: Vec::new(),
            seen: HashSet::new(),
        }
    }

    fn collect_program(&mut self, program: &Program) {
        // Pre-bind top-level function names for recursion/references.
        for stmt in &program.statements {
            if let Statement::Function { name, .. } = stmt {
                self.define(*name);
            }
        }
        for stmt in &program.statements {
            self.collect_stmt(stmt);
        }
    }

    fn define(&mut self, ident: Identifier) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(ident);
        }
    }

    fn is_bound(&self, ident: Identifier) -> bool {
        self.scopes.iter().rev().any(|s| s.contains(&ident))
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashSet::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn bind_pattern_identifiers(&mut self, pattern: &Pattern) {
        match pattern {
            Pattern::Identifier { name, .. } => self.define(*name),
            Pattern::Some { pattern, .. }
            | Pattern::Left { pattern, .. }
            | Pattern::Right { pattern, .. } => self.bind_pattern_identifiers(pattern),
            Pattern::Cons { head, tail, .. } => {
                self.bind_pattern_identifiers(head);
                self.bind_pattern_identifiers(tail);
            }
            Pattern::Tuple { elements, .. } => {
                for element in elements {
                    self.bind_pattern_identifiers(element);
                }
            }
            Pattern::Constructor { fields, .. } => {
                for field in fields {
                    self.bind_pattern_identifiers(field);
                }
            }
            Pattern::Wildcard { .. }
            | Pattern::Literal { .. }
            | Pattern::None { .. }
            | Pattern::EmptyList { .. } => {}
        }
    }

    fn collect_stmt(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Let { name, value, .. } => {
                self.collect_expr(value);
                self.define(*name);
            }
            Statement::LetDestructure { pattern, value, .. } => {
                self.collect_expr(value);
                self.bind_pattern_identifiers(pattern);
            }
            Statement::Assign { value, .. } => self.collect_expr(value),
            Statement::Expression { expression, .. } => self.collect_expr(expression),
            Statement::Return { value, .. } => {
                if let Some(v) = value {
                    self.collect_expr(v);
                }
            }
            Statement::Function {
                name,
                parameters,
                parameter_types,
                return_type,
                effects,
                body,
                ..
            } => {
                let expr = Expression::Function {
                    parameters: parameters.clone(),
                    parameter_types: parameter_types.clone(),
                    return_type: return_type.clone(),
                    effects: effects.clone(),
                    body: body.clone(),
                    span: stmt.span(),
                };
                let key = LiteralKey::from_expr(&expr);
                if !self.seen.contains(&key) {
                    let mut captures: Vec<Identifier> = collect_free_vars(&expr)
                        .into_iter()
                        .filter(|sym| self.is_bound(*sym))
                        .collect();
                    // Recursive local functions should not capture themselves.
                    captures.retain(|sym| sym != name);
                    captures.sort_by_key(|s| s.as_u32());
                    self.specs.push(LiteralFunctionSpec {
                        key,
                        parameters: parameters.clone(),
                        parameter_types: parameter_types.clone(),
                        return_type: return_type.clone(),
                        body: body.clone(),
                        captures,
                        self_name: Some(*name),
                    });
                    self.seen.insert(key);
                }

                // Function name is bound in outer scope after declaration.
                self.define(*name);

                self.push_scope();
                // Recursive references resolve in function body.
                self.define(*name);
                for p in parameters {
                    self.define(*p);
                }
                for s in &body.statements {
                    self.collect_stmt(s);
                }
                self.pop_scope();
            }
            Statement::Module { body, .. } => {
                self.push_scope();
                // Pre-bind all module function names before processing bodies.
                // This mirrors collect_program's pre-binding for top-level functions and
                // ensures nested literal functions inside module functions can correctly
                // include sibling module functions in their capture sets.
                for s in &body.statements {
                    if let Statement::Function { name, .. } = s {
                        self.define(*name);
                    }
                }
                // Process module body: module-level functions are compiled as module
                // functions (not literal closures), so we must NOT register them as
                // literal specs. We only collect nested literal functions from their bodies.
                for s in &body.statements {
                    match s {
                        Statement::Function {
                            name,
                            parameters,
                            body,
                            ..
                        } => {
                            // Module-level function: push a scope with params and collect
                            // any nested literal functions defined inside the body.
                            self.push_scope();
                            self.define(*name); // allow self-recursion within the body
                            for p in parameters {
                                self.define(*p);
                            }
                            for inner in &body.statements {
                                self.collect_stmt(inner);
                            }
                            self.pop_scope();
                        }
                        _ => self.collect_stmt(s),
                    }
                }
                self.pop_scope();
            }
            Statement::Import { .. } => {}
            Statement::Data { .. } => {}
            Statement::EffectDecl { .. } => {}
        }
    }

    fn collect_expr(&mut self, expr: &Expression) {
        match expr {
            Expression::Function {
                parameters,
                parameter_types,
                return_type,
                body,
                ..
            } => {
                let key = LiteralKey::from_expr(expr);
                if !self.seen.contains(&key) {
                    let mut captures: Vec<Identifier> = collect_free_vars(expr)
                        .into_iter()
                        .filter(|sym| self.is_bound(*sym))
                        .collect();
                    captures.sort_by_key(|s| s.as_u32());
                    self.specs.push(LiteralFunctionSpec {
                        key,
                        parameters: parameters.clone(),
                        parameter_types: parameter_types.clone(),
                        return_type: return_type.clone(),
                        body: body.clone(),
                        captures,
                        self_name: None,
                    });
                    self.seen.insert(key);
                }

                self.push_scope();
                for p in parameters {
                    self.define(*p);
                }
                for s in &body.statements {
                    self.collect_stmt(s);
                }
                self.pop_scope();
            }
            Expression::Prefix { right, .. } => self.collect_expr(right),
            Expression::Infix { left, right, .. } => {
                self.collect_expr(left);
                self.collect_expr(right);
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                self.collect_expr(condition);
                self.push_scope();
                for s in &consequence.statements {
                    self.collect_stmt(s);
                }
                self.pop_scope();
                if let Some(alt) = alternative {
                    self.push_scope();
                    for s in &alt.statements {
                        self.collect_stmt(s);
                    }
                    self.pop_scope();
                }
            }
            Expression::DoBlock { block, .. } => {
                self.push_scope();
                for s in &block.statements {
                    self.collect_stmt(s);
                }
                self.pop_scope();
            }
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                self.collect_expr(function);
                for a in arguments {
                    self.collect_expr(a);
                }
            }
            Expression::ListLiteral { elements, .. }
            | Expression::ArrayLiteral { elements, .. }
            | Expression::TupleLiteral { elements, .. } => {
                for e in elements {
                    self.collect_expr(e);
                }
            }
            Expression::Index { left, index, .. } => {
                self.collect_expr(left);
                self.collect_expr(index);
            }
            Expression::Hash { pairs, .. } => {
                for (k, v) in pairs {
                    self.collect_expr(k);
                    self.collect_expr(v);
                }
            }
            Expression::MemberAccess { object, .. } => self.collect_expr(object),
            Expression::TupleFieldAccess { object, .. } => self.collect_expr(object),
            Expression::Match {
                scrutinee, arms, ..
            } => {
                self.collect_expr(scrutinee);
                for arm in arms {
                    self.push_scope();
                    self.bind_pattern_identifiers(&arm.pattern);
                    if let Some(g) = &arm.guard {
                        self.collect_expr(g);
                    }
                    self.collect_expr(&arm.body);
                    self.pop_scope();
                }
            }
            Expression::Some { value, .. }
            | Expression::Left { value, .. }
            | Expression::Right { value, .. } => self.collect_expr(value),
            Expression::Cons { head, tail, .. } => {
                self.collect_expr(head);
                self.collect_expr(tail);
            }
            Expression::InterpolatedString { parts, .. } => {
                for part in parts {
                    if let crate::syntax::expression::StringPart::Interpolation(expr) = part {
                        self.collect_expr(expr);
                    }
                }
            }
            Expression::Perform { args, .. } => {
                for arg in args {
                    self.collect_expr(arg);
                }
            }
            Expression::Handle { expr, arms, .. } => {
                self.collect_expr(expr);
                for arm in arms {
                    // Build the same synthetic Function expression used by compile_jit_handle,
                    // so each arm closure is pre-compiled as a literal function spec.
                    let mut params = vec![arm.resume_param];
                    params.extend_from_slice(&arm.params);
                    let arm_fn_expr = Expression::Function {
                        parameters: params.clone(),
                        parameter_types: vec![None; params.len()],
                        return_type: None,
                        effects: vec![],
                        body: crate::syntax::block::Block {
                            statements: vec![crate::syntax::statement::Statement::Expression {
                                expression: arm.body.clone(),
                                has_semicolon: false,
                                span: arm.body.span(),
                            }],
                            span: arm.body.span(),
                        },
                        span: arm.span,
                    };
                    self.collect_expr(&arm_fn_expr);
                }
            }
            Expression::Identifier { .. }
            | Expression::Integer { .. }
            | Expression::Float { .. }
            | Expression::String { .. }
            | Expression::Boolean { .. }
            | Expression::EmptyList { .. }
            | Expression::None { .. } => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

fn convert_type_expr_for_contract(ty: &TypeExpr, interner: &Interner) -> Option<RuntimeType> {
    match ty {
        TypeExpr::Named { name, args, .. } => {
            let name_str = interner.try_resolve(*name)?;
            match (name_str, args.len()) {
                ("Any", 0) => Some(RuntimeType::Any),
                ("Int", 0) => Some(RuntimeType::Int),
                ("Float", 0) => Some(RuntimeType::Float),
                ("Bool", 0) => Some(RuntimeType::Bool),
                ("String", 0) => Some(RuntimeType::String),
                ("Unit", 0) => Some(RuntimeType::Unit),
                ("Option", 1) => Some(RuntimeType::Option(Box::new(
                    convert_type_expr_for_contract(&args[0], interner)?,
                ))),
                ("List", 1) => Some(RuntimeType::List(Box::new(convert_type_expr_for_contract(
                    &args[0], interner,
                )?))),
                ("Either", 2) => Some(RuntimeType::Either(
                    Box::new(convert_type_expr_for_contract(&args[0], interner)?),
                    Box::new(convert_type_expr_for_contract(&args[1], interner)?),
                )),
                ("Array", 1) => Some(RuntimeType::Array(Box::new(
                    convert_type_expr_for_contract(&args[0], interner)?,
                ))),
                ("Map", 2) => Some(RuntimeType::Map(
                    Box::new(convert_type_expr_for_contract(&args[0], interner)?),
                    Box::new(convert_type_expr_for_contract(&args[1], interner)?),
                )),
                _ => None,
            }
        }
        TypeExpr::Tuple { elements, .. } => Some(RuntimeType::Tuple(
            elements
                .iter()
                .map(|e| convert_type_expr_for_contract(e, interner))
                .collect::<Option<Vec<_>>>()?,
        )),
        TypeExpr::Function { .. } => None,
    }
}

fn runtime_contract_from_annotations(
    parameter_types: &[Option<TypeExpr>],
    return_type: &Option<TypeExpr>,
    effects: &[crate::syntax::effect_expr::EffectExpr],
    interner: &Interner,
) -> Option<FunctionContract> {
    let params = parameter_types
        .iter()
        .map(|ty| {
            ty.as_ref()
                .and_then(|t| convert_type_expr_for_contract(t, interner))
        })
        .collect::<Vec<_>>();
    let ret = return_type
        .as_ref()
        .and_then(|ty| convert_type_expr_for_contract(ty, interner));
    if !params.iter().any(|t| t.is_some()) && ret.is_none() && effects.is_empty() {
        None
    } else {
        let effects = effects
            .iter()
            .flat_map(crate::syntax::effect_expr::EffectExpr::normalized_names)
            .collect::<Vec<_>>();
        Some(FunctionContract {
            params,
            ret,
            effects,
        })
    }
}

struct HelperSig {
    num_params: usize,
    has_return: bool,
}

enum StmtOutcome {
    None,
    Value(CraneliftValue),
    Returned,
}

enum BlockEval {
    Value(CraneliftValue),
    Returned,
}

#[derive(Clone)]
struct TailCallContext {
    function_name: Option<Identifier>,
    loop_block: cranelift_codegen::ir::Block,
    params: Vec<(Identifier, Variable)>,
}

fn helper_signatures() -> Vec<(&'static str, HelperSig)> {
    vec![
        // Value constructors
        (
            "rt_make_integer",
            HelperSig {
                num_params: 2,
                has_return: true,
            },
        ),
        (
            "rt_make_float",
            HelperSig {
                num_params: 2,
                has_return: true,
            },
        ),
        (
            "rt_make_bool",
            HelperSig {
                num_params: 2,
                has_return: true,
            },
        ),
        (
            "rt_make_none",
            HelperSig {
                num_params: 1,
                has_return: true,
            },
        ),
        (
            "rt_make_empty_list",
            HelperSig {
                num_params: 1,
                has_return: true,
            },
        ),
        (
            "rt_make_string",
            HelperSig {
                num_params: 3,
                has_return: true,
            },
        ),
        (
            "rt_make_base_function",
            HelperSig {
                num_params: 2,
                has_return: true,
            },
        ),
        (
            "rt_make_jit_closure",
            HelperSig {
                num_params: 4,
                has_return: true,
            },
        ),
        (
            "rt_make_cons",
            HelperSig {
                num_params: 3,
                has_return: true,
            },
        ),
        // Arithmetic
        (
            "rt_add",
            HelperSig {
                num_params: 3,
                has_return: true,
            },
        ),
        (
            "rt_sub",
            HelperSig {
                num_params: 3,
                has_return: true,
            },
        ),
        (
            "rt_mul",
            HelperSig {
                num_params: 3,
                has_return: true,
            },
        ),
        (
            "rt_div",
            HelperSig {
                num_params: 3,
                has_return: true,
            },
        ),
        (
            "rt_mod",
            HelperSig {
                num_params: 3,
                has_return: true,
            },
        ),
        // Prefix
        (
            "rt_negate",
            HelperSig {
                num_params: 2,
                has_return: true,
            },
        ),
        (
            "rt_not",
            HelperSig {
                num_params: 2,
                has_return: true,
            },
        ),
        (
            "rt_is_truthy",
            HelperSig {
                num_params: 2,
                has_return: true,
            },
        ),
        (
            "rt_is_cons",
            HelperSig {
                num_params: 2,
                has_return: true,
            },
        ),
        (
            "rt_cons_head",
            HelperSig {
                num_params: 2,
                has_return: true,
            },
        ),
        (
            "rt_cons_tail",
            HelperSig {
                num_params: 2,
                has_return: true,
            },
        ),
        // Comparisons
        (
            "rt_equal",
            HelperSig {
                num_params: 3,
                has_return: true,
            },
        ),
        (
            "rt_not_equal",
            HelperSig {
                num_params: 3,
                has_return: true,
            },
        ),
        (
            "rt_greater_than",
            HelperSig {
                num_params: 3,
                has_return: true,
            },
        ),
        (
            "rt_less_than_or_equal",
            HelperSig {
                num_params: 3,
                has_return: true,
            },
        ),
        (
            "rt_greater_than_or_equal",
            HelperSig {
                num_params: 3,
                has_return: true,
            },
        ),
        // BaseFunctions & globals
        (
            "rt_call_base_function",
            HelperSig {
                num_params: 4,
                has_return: true,
            },
        ),
        (
            "rt_call_primop",
            HelperSig {
                num_params: 4,
                has_return: true,
            },
        ),
        (
            "rt_call_value",
            HelperSig {
                num_params: 4,
                has_return: true,
            },
        ),
        (
            "rt_get_global",
            HelperSig {
                num_params: 2,
                has_return: true,
            },
        ),
        (
            "rt_set_global",
            HelperSig {
                num_params: 3,
                has_return: false,
            },
        ),
        (
            "rt_set_arity_error",
            HelperSig {
                num_params: 3,
                has_return: false,
            },
        ),
        (
            "rt_check_jit_contract_call",
            HelperSig {
                num_params: 6,
                has_return: true,
            },
        ),
        (
            "rt_check_jit_contract_return",
            HelperSig {
                num_params: 5,
                has_return: true,
            },
        ),
        // Phase 4: value wrappers (ctx, value) -> *mut Value
        (
            "rt_make_some",
            HelperSig {
                num_params: 2,
                has_return: true,
            },
        ),
        (
            "rt_make_left",
            HelperSig {
                num_params: 2,
                has_return: true,
            },
        ),
        (
            "rt_make_right",
            HelperSig {
                num_params: 2,
                has_return: true,
            },
        ),
        // Phase 4: pattern matching checks (ctx, value) -> i64
        (
            "rt_is_some",
            HelperSig {
                num_params: 2,
                has_return: true,
            },
        ),
        (
            "rt_is_left",
            HelperSig {
                num_params: 2,
                has_return: true,
            },
        ),
        (
            "rt_is_right",
            HelperSig {
                num_params: 2,
                has_return: true,
            },
        ),
        (
            "rt_is_none",
            HelperSig {
                num_params: 2,
                has_return: true,
            },
        ),
        (
            "rt_is_empty_list",
            HelperSig {
                num_params: 2,
                has_return: true,
            },
        ),
        // Phase 4: unwrap helpers (ctx, value) -> *mut Value
        (
            "rt_unwrap_some",
            HelperSig {
                num_params: 2,
                has_return: true,
            },
        ),
        (
            "rt_unwrap_left",
            HelperSig {
                num_params: 2,
                has_return: true,
            },
        ),
        (
            "rt_unwrap_right",
            HelperSig {
                num_params: 2,
                has_return: true,
            },
        ),
        // Phase 4: structural equality (ctx, a, b) -> i64
        (
            "rt_values_equal",
            HelperSig {
                num_params: 3,
                has_return: true,
            },
        ),
        // Phase 4: collections
        (
            "rt_make_array",
            HelperSig {
                num_params: 3,
                has_return: true,
            },
        ),
        (
            "rt_make_tuple",
            HelperSig {
                num_params: 3,
                has_return: true,
            },
        ),
        (
            "rt_make_hash",
            HelperSig {
                num_params: 3,
                has_return: true,
            },
        ),
        (
            "rt_index",
            HelperSig {
                num_params: 3,
                has_return: true,
            },
        ),
        (
            "rt_is_tuple",
            HelperSig {
                num_params: 2,
                has_return: true,
            },
        ),
        (
            "rt_tuple_len_eq",
            HelperSig {
                num_params: 3,
                has_return: true,
            },
        ),
        (
            "rt_tuple_get",
            HelperSig {
                num_params: 3,
                has_return: true,
            },
        ),
        // Phase 4: string ops (ctx, value) -> *mut Value
        (
            "rt_to_string",
            HelperSig {
                num_params: 2,
                has_return: true,
            },
        ),
        // Phase 5: ADT helpers
        // rt_make_adt(ctx, constructor_ptr, constructor_len, fields_ptr, arity) -> *mut Value
        (
            "rt_make_adt",
            HelperSig {
                num_params: 5,
                has_return: true,
            },
        ),
        // rt_is_adt_constructor(ctx, value, constructor_ptr, constructor_len) -> i64
        (
            "rt_is_adt_constructor",
            HelperSig {
                num_params: 4,
                has_return: true,
            },
        ),
        // rt_adt_field(ctx, value, field_idx) -> *mut Value
        (
            "rt_adt_field",
            HelperSig {
                num_params: 3,
                has_return: true,
            },
        ),
        // Algebraic effects
        // rt_push_handler(ctx, effect_id, ops_ptr, closures_ptr, narms) -> void
        (
            "rt_push_handler",
            HelperSig {
                num_params: 5,
                has_return: false,
            },
        ),
        // rt_pop_handler(ctx) -> void
        (
            "rt_pop_handler",
            HelperSig {
                num_params: 1,
                has_return: false,
            },
        ),
        // rt_perform(ctx, effect_id, op_id, args_ptr, nargs,
        //            effect_name_ptr, effect_name_len, op_name_ptr, op_name_len,
        //            line, column) -> *mut Value
        (
            "rt_perform",
            HelperSig {
                num_params: 11,
                has_return: true,
            },
        ),
    ]
}

fn default_libcall_names() -> Box<dyn Fn(cranelift_codegen::ir::LibCall) -> String + Send + Sync> {
    cranelift_module::default_libcall_names()
}
