//! AST → Cranelift IR compiler (Phase 1: expressions, let bindings, calls).

use std::collections::HashMap;

use cranelift_codegen::ir::{
    types, AbiParam, Function, InstBuilder, UserFuncName,
    Value as CraneliftValue,
};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{FuncId, Linkage, Module};
use cranelift_jit::JITModule;

use crate::syntax::{
    Identifier,
    expression::Expression,
    interner::Interner,
    program::Program,
    statement::Statement,
};

use super::runtime_helpers::rt_symbols;

/// Pointer type used for all Value pointers in JIT code.
const PTR_TYPE: types::Type = types::I64;

/// Maps runtime helper names to their Cranelift FuncIds.
struct HelperFuncs {
    ids: HashMap<&'static str, FuncId>,
}

/// Tracks variables in the current scope.
struct Scope {
    /// Maps interned identifier → Cranelift Variable
    locals: HashMap<Identifier, Variable>,
    /// Maps interned identifier → global slot index
    globals: HashMap<Identifier, usize>,
    next_global: usize,
    /// Maps interned identifier → builtin index
    builtins: HashMap<Identifier, usize>,
}

impl Scope {
    fn new() -> Self {
        Self {
            locals: HashMap::new(),
            globals: HashMap::new(),
            next_global: 0,
            builtins: HashMap::new(),
        }
    }

    fn define_global(&mut self, name: Identifier) -> usize {
        let idx = self.next_global;
        self.next_global += 1;
        self.globals.insert(name, idx);
        idx
    }
}

pub struct JitCompiler {
    pub module: JITModule,
    builder_ctx: FunctionBuilderContext,
    helpers: HelperFuncs,
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

        // Register builtins
        register_builtins(&mut scope, interner);

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
                last_val = compile_statement(
                    module, helpers, &mut builder, &mut scope, ctx_val, stmt, interner,
                )?;
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

        Ok(main_id)
    }

    /// Finalize all functions and make them callable.
    pub fn finalize(&mut self) {
        self.module.finalize_definitions().unwrap();
    }

    /// Get a callable function pointer for the given FuncId.
    pub fn get_func_ptr(&self, id: FuncId) -> *const u8 {
        self.module.get_finalized_function(id)
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
    stmt: &Statement,
    interner: &Interner,
) -> Result<Option<CraneliftValue>, String> {
    match stmt {
        Statement::Let { name, value, .. } => {
            let val = compile_expression(module, helpers, builder, scope, ctx_val, value, interner)?;
            let var = builder.declare_var(PTR_TYPE);
            builder.def_var(var, val);
            scope.locals.insert(*name, var);
            Ok(None)
        }
        Statement::Expression { expression, .. } => {
            let val =
                compile_expression(module, helpers, builder, scope, ctx_val, expression, interner)?;
            Ok(Some(val))
        }
        Statement::Assign { name, value, .. } => {
            let val = compile_expression(module, helpers, builder, scope, ctx_val, value, interner)?;
            if let Some(&var) = scope.locals.get(name) {
                builder.def_var(var, val);
            } else if let Some(&idx) = scope.globals.get(name) {
                let set_global = get_helper_func_ref(module, helpers, builder, "rt_set_global");
                let idx_val = builder.ins().iconst(PTR_TYPE, idx as i64);
                builder.ins().call(set_global, &[ctx_val, idx_val, val]);
            }
            Ok(None)
        }
        Statement::Function { name, .. } => {
            // Phase 1: top-level functions not yet supported, skip
            let _ = name;
            Ok(None)
        }
        _ => {
            // Module, Import, Return — skip for Phase 1
            Ok(None)
        }
    }
}

fn compile_expression(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
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
        Expression::String { value, .. } => {
            let make_string = get_helper_func_ref(module, helpers, builder, "rt_make_string");
            let bytes = value.as_bytes();
            let ptr = builder.ins().iconst(PTR_TYPE, bytes.as_ptr() as i64);
            let len = builder.ins().iconst(PTR_TYPE, bytes.len() as i64);
            let call = builder.ins().call(make_string, &[ctx_val, ptr, len]);
            Ok(builder.inst_results(call)[0])
        }

        // --- Identifiers ---
        Expression::Identifier { name, .. } => {
            if let Some(&var) = scope.locals.get(name) {
                Ok(builder.use_var(var))
            } else if let Some(&builtin_idx) = scope.builtins.get(name) {
                // Builtins are resolved at call sites via rt_call_builtin.
                // Store the builtin index as a negative "tag" (impossible real pointer).
                let tag = builder
                    .ins()
                    .iconst(PTR_TYPE, -(builtin_idx as i64 + 1));
                Ok(tag)
            } else if let Some(&idx) = scope.globals.get(name) {
                let get_global = get_helper_func_ref(module, helpers, builder, "rt_get_global");
                let idx_val = builder.ins().iconst(PTR_TYPE, idx as i64);
                let call = builder.ins().call(get_global, &[ctx_val, idx_val]);
                Ok(builder.inst_results(call)[0])
            } else {
                Err(format!(
                    "undefined identifier: {}",
                    interner.resolve(*name)
                ))
            }
        }

        // --- Prefix operators ---
        Expression::Prefix {
            operator, right, ..
        } => {
            let operand =
                compile_expression(module, helpers, builder, scope, ctx_val, right, interner)?;
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
            let lhs =
                compile_expression(module, helpers, builder, scope, ctx_val, left, interner)?;
            let rhs =
                compile_expression(module, helpers, builder, scope, ctx_val, right, interner)?;
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

        // --- Function calls ---
        Expression::Call {
            function,
            arguments,
            ..
        } => {
            // Check if calling a builtin directly
            if let Expression::Identifier { name, .. } = function.as_ref() {
                if let Some(&builtin_idx) = scope.builtins.get(name) {
                    return compile_builtin_call(
                        module, helpers, builder, scope, ctx_val, builtin_idx, arguments, interner,
                    );
                }
            }

            // General call: not yet supported in Phase 1
            Err("non-builtin function calls not yet supported in JIT".to_string())
        }

        _ => Err(format!(
            "unsupported expression in JIT: {:?}",
            std::mem::discriminant(expr)
        )),
    }
}

fn compile_builtin_call(
    module: &mut JITModule,
    helpers: &HelperFuncs,
    builder: &mut FunctionBuilder,
    scope: &mut Scope,
    ctx_val: CraneliftValue,
    builtin_idx: usize,
    arguments: &[Expression],
    interner: &Interner,
) -> Result<CraneliftValue, String> {
    // Compile all arguments
    let mut arg_vals = Vec::with_capacity(arguments.len());
    for arg in arguments {
        let val = compile_expression(module, helpers, builder, scope, ctx_val, arg, interner)?;
        arg_vals.push(val);
    }

    // Store argument pointers in a stack slot array
    let nargs = arg_vals.len();
    let slot = builder.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
        cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
        (nargs as u32) * 8, // 8 bytes per pointer
        3,                   // align to 8 bytes (2^3)
    ));

    for (i, val) in arg_vals.iter().enumerate() {
        builder
            .ins()
            .stack_store(*val, slot, (i * 8) as i32);
    }

    let args_ptr = builder.ins().stack_addr(PTR_TYPE, slot, 0);
    let idx_val = builder.ins().iconst(PTR_TYPE, builtin_idx as i64);
    let nargs_val = builder.ins().iconst(PTR_TYPE, nargs as i64);

    let call_builtin = get_helper_func_ref(module, helpers, builder, "rt_call_builtin");
    let call = builder
        .ins()
        .call(call_builtin, &[ctx_val, idx_val, args_ptr, nargs_val]);
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

fn register_builtins(scope: &mut Scope, interner: &Interner) {
    use crate::runtime::builtins::BUILTINS;
    use crate::syntax::symbol::Symbol;
    // Scan the interner to find Symbols matching each builtin name.
    for (idx, builtin) in BUILTINS.iter().enumerate() {
        for sym_idx in 0u32.. {
            let sym = Symbol::new(sym_idx);
            match interner.try_resolve(sym) {
                Some(name) if name == builtin.name => {
                    scope.builtins.insert(sym, idx);
                    break;
                }
                Some(_) => continue,
                None => break,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

struct HelperSig {
    num_params: usize,
    has_return: bool,
}

fn helper_signatures() -> Vec<(&'static str, HelperSig)> {
    vec![
        // Value constructors
        ("rt_make_integer", HelperSig { num_params: 2, has_return: true }),
        ("rt_make_float", HelperSig { num_params: 2, has_return: true }),
        ("rt_make_bool", HelperSig { num_params: 2, has_return: true }),
        ("rt_make_none", HelperSig { num_params: 1, has_return: true }),
        ("rt_make_string", HelperSig { num_params: 3, has_return: true }),
        // Arithmetic
        ("rt_add", HelperSig { num_params: 3, has_return: true }),
        ("rt_sub", HelperSig { num_params: 3, has_return: true }),
        ("rt_mul", HelperSig { num_params: 3, has_return: true }),
        ("rt_div", HelperSig { num_params: 3, has_return: true }),
        ("rt_mod", HelperSig { num_params: 3, has_return: true }),
        // Prefix
        ("rt_negate", HelperSig { num_params: 2, has_return: true }),
        ("rt_not", HelperSig { num_params: 2, has_return: true }),
        // Comparisons
        ("rt_equal", HelperSig { num_params: 3, has_return: true }),
        ("rt_not_equal", HelperSig { num_params: 3, has_return: true }),
        ("rt_greater_than", HelperSig { num_params: 3, has_return: true }),
        ("rt_less_than_or_equal", HelperSig { num_params: 3, has_return: true }),
        ("rt_greater_than_or_equal", HelperSig { num_params: 3, has_return: true }),
        // Builtins & globals
        ("rt_call_builtin", HelperSig { num_params: 4, has_return: true }),
        ("rt_get_global", HelperSig { num_params: 2, has_return: true }),
        ("rt_set_global", HelperSig { num_params: 3, has_return: false }),
    ]
}

fn default_libcall_names() -> Box<dyn Fn(cranelift_codegen::ir::LibCall) -> String + Send + Sync> {
    cranelift_module::default_libcall_names()
}
