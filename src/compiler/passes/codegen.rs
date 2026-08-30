use std::collections::HashMap;

use crate::bytecode::op_code::OpCode;
use crate::cfg::IrProgram;
use crate::core::CoreBinderId;
use crate::diagnostics::DiagnosticPhase;
use crate::syntax::{
    pattern_validate::validate_program_patterns, program::Program, statement::Statement,
};

use super::super::{Compiler, tag_diagnostics};

impl Compiler {
    /// Phase 5: Pattern validation and compile all statements to bytecode.
    pub(in crate::compiler) fn phase_codegen(&mut self, program: &Program, ir_program: &IrProgram) {
        let mut pattern_diags = validate_program_patterns(program, &self.file_path, &self.interner);
        tag_diagnostics(&mut pattern_diags, DiagnosticPhase::Validation);
        self.errors.extend(pattern_diags);

        for statement in &program.statements {
            // Continue compilation even if there are errors
            let compile_result = match statement {
                Statement::Function {
                    name,
                    parameters,
                    parameter_types,
                    return_type,
                    effects,
                    body,
                    intrinsic,
                    span,
                    ..
                } => {
                    let effective_effects: Vec<crate::syntax::effect_expr::EffectExpr> =
                        if effects.is_empty() {
                            self.lookup_unqualified_contract(*name, parameters.len())
                                .map(|contract| contract.effects.clone())
                                .unwrap_or_default()
                        } else {
                            effects.clone()
                        };
                    let ir_function = self.find_ir_function_by_symbol(ir_program, *name);
                    let result = self.compile_function_statement(
                        *name,
                        parameters,
                        parameter_types,
                        return_type,
                        &effective_effects,
                        body,
                        *intrinsic,
                        ir_function,
                        *span,
                    );
                    if self.scope_index == 0 {
                        self.file_scope_symbols.insert(*name);
                    }
                    result
                }
                Statement::Module { name, body, span } => {
                    self.compile_module_statement(*name, body, span.start, Some(ir_program))
                }
                _ => self.compile_statement(statement),
            };
            if let Err(err) = compile_result {
                let mut diag = *err;
                if diag.phase().is_none() {
                    diag.phase = Some(DiagnosticPhase::TypeCheck);
                }
                self.errors.push(diag);
            }
        }

        // Emit dict tuple construction bytecode after all functions are compiled.
        // Dict values must be initialized at module load time before user code
        // can call constrained functions.
        self.emit_dict_globals(ir_program);
    }

    /// Emit bytecode to construct and store dictionary globals.
    ///
    /// A dictionary global has one of two shapes, and both must be stored.
    /// A plain instance lowers to a `MakeTuple` of method references. A
    /// contextual instance (`instance Eq<a> => Eq<List<a>>`) lowers to a
    /// `Lam` taking the context dictionaries and returning that tuple, so it
    /// is stored as the compiled dictionary-constructor function itself.
    ///
    /// Handling only the tuple shape left the contextual global *declared*
    /// (`ir_lowering` registers a slot for every `__dict_*`) but never
    /// *stored*, so the VM read it back as `None` and reported
    /// `E1001 Cannot call non-function value` (Proposal 0179 Stage 2).
    fn emit_dict_globals(&mut self, ir_program: &IrProgram) {
        let Some(core) = ir_program.core.as_ref() else {
            return;
        };

        // Classify first, so the emit loop below borrows `self` mutably
        // without holding a borrow of `core`.
        let dict_entries: Vec<(crate::syntax::Identifier, DictGlobal)> = core
            .defs
            .iter()
            .filter(|def| def.is_dict_def)
            .filter_map(|def| DictGlobal::classify(&def.expr).map(|kind| (def.name, kind)))
            .collect();

        for (dict_name, kind) in &dict_entries {
            let Some(dict_binding) = self.symbol_table.resolve(*dict_name) else {
                continue;
            };

            match kind {
                DictGlobal::MethodTuple { methods } => {
                    // Resolve every method before emitting anything: a partial
                    // load would leave operands stranded on the stack.
                    let bindings: Option<Vec<_>> = methods
                        .iter()
                        .map(|&method| self.symbol_table.resolve(method))
                        .collect();
                    let Some(bindings) = bindings else {
                        continue;
                    };

                    for binding in &bindings {
                        self.load_symbol(binding);
                    }

                    let count = bindings.len();
                    if count <= 255 {
                        self.emit(OpCode::OpTuple, &[count, 0]);
                    } else {
                        self.emit(OpCode::OpTupleLong, &[count]);
                    }
                }
                DictGlobal::ContextualConstructor {
                    context_arity,
                    methods,
                } => {
                    if !self.emit_contextual_dict_constructor(*context_arity, methods) {
                        continue;
                    }
                }
            }

            self.emit(OpCode::OpSetGlobal, &[dict_binding.index]);
        }
    }

    /// Push a closure implementing a contextual instance's dictionary
    /// constructor, returning `false` if a method symbol was unresolvable.
    ///
    /// The Core shape being reproduced is
    /// `λctx. MakeTuple(λargs. __tc_Class_Type_method(ctx, args))`: the outer
    /// function takes the context dictionaries, and each method slot is a
    /// closure capturing them and forwarding to the already-compiled mangled
    /// method, which takes those dictionaries as leading parameters.
    ///
    /// This is assembled directly rather than compiled from Core, because the
    /// definition is synthesised by a Core pass and so never exists as an AST
    /// function that `phase_codegen` could lower.
    fn emit_contextual_dict_constructor(
        &mut self,
        context_arity: usize,
        methods: &[ContextualDictMethod],
    ) -> bool {
        use crate::runtime::compiled_function::CompiledFunction;
        use crate::runtime::value::Value;
        use std::sync::Arc;

        // Resolve every method up front: emitting a partial constructor would
        // leave a malformed closure in the dictionary slot.
        let Some(globals): Option<Vec<_>> = methods
            .iter()
            .map(|method| {
                let binding = self.symbol_table.resolve(method.mangled)?;
                matches!(
                    binding.symbol_scope,
                    crate::compiler::symbol_scope::SymbolScope::Global
                )
                .then_some((binding.index, method.arity))
            })
            .collect()
        else {
            return false;
        };

        let mut body = Vec::new();
        for (slot, (global_index, arity)) in globals.iter().enumerate() {
            // Each slot: OpClosure over a forwarder that captures the context
            // dictionaries as free variables.
            let forwarder = Self::contextual_dict_forwarder(*global_index, context_arity, *arity);
            let forwarder_index = self.add_constant(Value::Function(Arc::new(forwarder)));

            for ctx in 0..context_arity {
                push_operand(&mut body, OpCode::OpGetLocal, ctx);
            }
            push_closure(&mut body, forwarder_index, context_arity);
            debug_assert!(slot < methods.len());
        }
        push_tuple(&mut body, methods.len());
        body.push(OpCode::OpReturnValue as u8);

        let constructor = CompiledFunction::new(body, context_arity, context_arity, None);
        let constructor_index = self.add_constant(Value::Function(Arc::new(constructor)));
        self.emit_closure_index(constructor_index, 0);
        true
    }

    /// Build the per-method forwarder `λargs. method(ctx.., args..)`.
    ///
    /// The context dictionaries arrive as free variables (captured by the
    /// enclosing constructor) and are passed ahead of the caller's arguments,
    /// matching the leading dictionary parameters that dictionary elaboration
    /// gave the mangled method.
    fn contextual_dict_forwarder(
        global_index: usize,
        context_arity: usize,
        arity: usize,
    ) -> crate::runtime::compiled_function::CompiledFunction {
        let mut code = Vec::new();
        push_operand(&mut code, OpCode::OpGetGlobal, global_index);
        for ctx in 0..context_arity {
            push_operand(&mut code, OpCode::OpGetFree, ctx);
        }
        for arg in 0..arity {
            push_operand(&mut code, OpCode::OpGetLocal, arg);
        }
        code.push(OpCode::OpCall as u8);
        code.push((context_arity + arity) as u8);
        code.push(OpCode::OpReturnValue as u8);

        crate::runtime::compiled_function::CompiledFunction::new(code, arity, arity, None)
    }
}

/// One method slot of a contextual instance's dictionary.
struct ContextualDictMethod {
    /// The mangled `__tc_{Class}_{Type}_{method}` symbol.
    mangled: crate::syntax::Identifier,
    /// How many arguments the caller supplies, excluding the leading context
    /// dictionaries.
    arity: usize,
}

/// How a `__dict_*` global is materialised at module load time.
enum DictGlobal {
    /// A plain instance: a tuple of method references, in class-declaration
    /// order.
    MethodTuple {
        methods: Vec<crate::syntax::Identifier>,
    },
    /// A contextual instance: a function from context dictionaries to the
    /// method tuple.
    ContextualConstructor {
        /// Number of context dictionaries the constructor takes.
        context_arity: usize,
        methods: Vec<ContextualDictMethod>,
    },
}

impl DictGlobal {
    /// Classify a dictionary definition's body, or `None` when it has neither
    /// recognised shape.
    fn classify(expr: &crate::core::CoreExpr) -> Option<Self> {
        use crate::core::{CoreExpr, CorePrimOp};

        match expr {
            CoreExpr::PrimOp {
                op: CorePrimOp::MakeTuple,
                args,
                ..
            } => {
                let methods: Option<Vec<_>> = args
                    .iter()
                    .map(|arg| match arg {
                        CoreExpr::Var { var, .. } => Some(var.name),
                        _ => None,
                    })
                    .collect();
                methods
                    .filter(|methods| !methods.is_empty())
                    .map(|methods| Self::MethodTuple { methods })
            }
            CoreExpr::Lam { params, body, .. } => {
                let methods = Self::contextual_methods(body)?;
                Some(Self::ContextualConstructor {
                    context_arity: params.len(),
                    methods,
                })
            }
            _ => None,
        }
    }

    /// Extract the method slots from a contextual dictionary body,
    /// `MakeTuple(λargs. __tc_*(ctx.., args..))`.
    ///
    /// Core lowering hoists each method closure into a `let`, so the tuple
    /// elements are usually variables referring to those bindings rather than
    /// inline lambdas. Both forms are accepted.
    fn contextual_methods(body: &crate::core::CoreExpr) -> Option<Vec<ContextualDictMethod>> {
        use crate::core::{CoreExpr, CorePrimOp};

        let mut bound: HashMap<CoreBinderId, &CoreExpr> = HashMap::new();
        let mut cursor = body;
        while let CoreExpr::Let { var, rhs, body, .. } = cursor {
            bound.insert(var.id, rhs.as_ref());
            cursor = body;
        }

        let CoreExpr::PrimOp {
            op: CorePrimOp::MakeTuple,
            args,
            ..
        } = cursor
        else {
            return None;
        };

        args.iter()
            .map(|slot| Self::contextual_method(slot, &bound))
            .collect()
    }

    /// Read one method slot: a lambda forwarding to a mangled method, either
    /// inline or reached through a `let` binding.
    fn contextual_method(
        slot: &crate::core::CoreExpr,
        bound: &HashMap<CoreBinderId, &crate::core::CoreExpr>,
    ) -> Option<ContextualDictMethod> {
        use crate::core::CoreExpr;

        let slot = match slot {
            CoreExpr::Var { var, .. } => var
                .binder
                .and_then(|binder| bound.get(&binder).copied())
                .unwrap_or(slot),
            other => other,
        };

        let CoreExpr::Lam { params, body, .. } = slot else {
            return None;
        };
        let CoreExpr::App { func, .. } = body.as_ref() else {
            return None;
        };
        let CoreExpr::Var { var, .. } = func.as_ref() else {
            return None;
        };
        Some(ContextualDictMethod {
            mangled: var.name,
            arity: params.len(),
        })
    }
}

/// Append `op` with a single two-byte operand.
fn push_operand(code: &mut Vec<u8>, op: OpCode, operand: usize) {
    code.push(op as u8);
    match op {
        OpCode::OpGetLocal | OpCode::OpGetFree => code.push(operand as u8),
        _ => code.extend_from_slice(&(operand as u16).to_be_bytes()),
    }
}

fn push_closure(code: &mut Vec<u8>, const_index: usize, num_free: usize) {
    code.push(OpCode::OpClosure as u8);
    code.extend_from_slice(&(const_index as u16).to_be_bytes());
    code.push(num_free as u8);
}

fn push_tuple(code: &mut Vec<u8>, count: usize) {
    code.push(OpCode::OpTuple as u8);
    code.extend_from_slice(&(count as u16).to_be_bytes());
}
