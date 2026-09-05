use std::collections::HashMap;

use crate::bytecode::op_code::OpCode;
use crate::cfg::IrProgram;
use crate::core::CoreBinderId;
use crate::diagnostics::DiagnosticPhase;
use crate::syntax::{
    pattern_validate::validate_program_patterns, program::Program, statement::Statement,
};

use super::super::{Compiler, tag_diagnostics};

/// Whether compiling `statement` emits code that runs when the module loads.
///
/// A declaration binds a name and evaluates nothing; these evaluate. The
/// distinction is what decides where the dictionary globals are stored — see
/// `phase_codegen`.
fn runs_at_load_time(statement: &Statement) -> bool {
    match statement {
        Statement::Let { .. }
        | Statement::LetDestructure { .. }
        | Statement::Assign { .. }
        | Statement::Expression { .. }
        | Statement::Return { .. } => true,
        Statement::Function { .. }
        | Statement::Module { .. }
        | Statement::Import { .. }
        | Statement::Data { .. }
        | Statement::EffectDecl { .. }
        | Statement::EffectAlias { .. }
        | Statement::Class { .. }
        | Statement::Instance { .. }
        | Statement::TypeAlias(_) => false,
    }
}

impl Compiler {
    /// Phase 5: Pattern validation and compile all statements to bytecode.
    pub(in crate::compiler) fn phase_codegen(&mut self, program: &Program, ir_program: &IrProgram) {
        let mut pattern_diags = validate_program_patterns(program, &self.file_path, &self.interner);
        tag_diagnostics(&mut pattern_diags, DiagnosticPhase::Validation);
        self.errors.extend(pattern_diags);

        // The dictionary globals are stored once before the first statement
        // that can *run*, and again after the whole program is compiled.
        //
        // A top-level value definition's initializer executes at module load
        // time, and if it calls a constrained function it reads a `__dict_*`
        // slot. Stored only at the end, that slot is still `None` when the
        // initializer runs, and the call fails as `E1001 Cannot call
        // non-function value` — naming the dictionary constructor rather than
        // the user's function, which is what made this look like a problem
        // with the callee (KI-083).
        //
        // Not stored first either: a dictionary is built from its instance's
        // compiled methods, so those globals must already hold their closures.
        // The boundary satisfies both — every declaration ahead of the first
        // executable statement has been compiled, and nothing has executed
        // yet. An instance declared *after* a value definition that needs it
        // stays out of reach, which is the rule the top level already follows:
        // a value definition cannot call a function declared below it either.
        let mut dictionaries_stored = false;
        for statement in &program.statements {
            if !dictionaries_stored && runs_at_load_time(statement) {
                self.store_dictionary_globals(ir_program);
                dictionaries_stored = true;
            }
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

        // Stored again, unconditionally: the boundary above can only build a
        // dictionary from the methods compiled before it, so an instance
        // declared after the first executable statement would be left holding
        // unassigned slots. Re-storing makes the boundary a strict addition
        // rather than a move, and the two agree wherever both can build a
        // dictionary at all.
        self.store_dictionary_globals(ir_program);

        // The aliases are emitted here and *only* here. Each copies a
        // module-qualified instance method into its canonical name, so run
        // before the module has initialized its functions they would copy
        // `None` over the canonical binding — the very binding a dictionary
        // constructor reads when it is finally called. That is what the
        // boundary store above would otherwise walk straight into.
        self.emit_instance_method_aliases(ir_program);
    }

    /// Store the dictionary globals: the ones this unit defines, and the ones
    /// it imports.
    ///
    /// Both are values built from already-compiled functions, and both are
    /// read by any constrained call. The instance-method aliases are
    /// deliberately not here — see the call site.
    fn store_dictionary_globals(&mut self, ir_program: &IrProgram) {
        self.emit_dict_globals(ir_program);
        self.emit_imported_dict_globals(ir_program);
    }

    /// Store the dictionary globals of instances this unit imports.
    ///
    /// `emit_dict_globals` can only store what this unit's Core defines, and a
    /// cached dependency contributes no Core at all. Its `__dict_*` symbol is
    /// still declared here (`preload_imported_instance_schemes`) because a
    /// constrained call across the module boundary names it, so without this
    /// the global reads back as `None` and the call fails with
    /// `E1001 Cannot call non-function value` (KI-061).
    ///
    /// The layout rebuilt here is [`ClassEnv::dictionary_layout`]'s, the same
    /// one `emit_dict_globals` reproduces from Core.
    ///
    /// [`ClassEnv::dictionary_layout`]: crate::types::class_env::ClassEnv::dictionary_layout
    fn emit_imported_dict_globals(&mut self, ir_program: &IrProgram) {
        let locally_defined: std::collections::HashSet<crate::syntax::Identifier> = ir_program
            .core
            .as_ref()
            .map(|core| {
                core.defs
                    .iter()
                    .filter(|def| def.is_dict_def)
                    .map(|def| def.name)
                    .collect()
            })
            .unwrap_or_default();

        // Classify first: building the plan borrows `class_env`, emitting it
        // borrows `self` mutably.
        let plans = self.imported_dict_global_plans(&locally_defined);
        for (dict_binding, context_arity, slots) in plans {
            let emitted = if context_arity == 0 {
                self.emit_plain_dict_tuple(&slots)
            } else {
                self.emit_contextual_dict_constructor(context_arity, &slots)
            };
            if emitted {
                self.emit(OpCode::OpSetGlobal, &[dict_binding]);
            }
        }
    }

    /// The dictionary globals this unit must store on behalf of its imports,
    /// as `(global index, context arity, slots)`.
    fn imported_dict_global_plans(
        &mut self,
        locally_defined: &std::collections::HashSet<crate::syntax::Identifier>,
    ) -> Vec<(usize, usize, Vec<ContextualDictSlot>)> {
        let instances = self.class_env.instances.clone();
        let mut plans = Vec::new();
        for instance in &instances {
            let Some(class_def) = self
                .class_env
                .lookup_class_by_id(instance.class_id)
                .cloned()
            else {
                continue;
            };
            let type_key = instance
                .type_args
                .iter()
                .map(|arg| arg.display_with(&self.interner))
                .collect::<Vec<_>>()
                .join("_");
            let dict_str = crate::types::class_env::dictionary_name(
                instance.class_id,
                &type_key,
                &self.interner,
            );
            let Some(dict_name) = self.interner.lookup(&dict_str) else {
                continue;
            };
            if locally_defined.contains(&dict_name) {
                continue;
            }
            let Some(dict_binding) = self.symbol_table.resolve(dict_name) else {
                continue;
            };
            if dict_binding.symbol_scope != crate::compiler::symbol_scope::SymbolScope::Global {
                continue;
            }

            // Context dictionaries become this constructor's parameters, in
            // declaration order — the same order `resolve_dictionary_ref_by_id`
            // produces its `context_args` in.
            let context_arity = instance
                .context_class_ids
                .iter()
                .take(instance.context.len())
                .count();

            let mut slots = Vec::new();
            let mut complete = true;
            for &superclass in &class_def.superclass_class_ids {
                let from_context = instance
                    .context_class_ids
                    .iter()
                    .position(|&context_id| context_id == superclass);
                match from_context {
                    Some(index) => slots.push(ContextualDictSlot::ContextEvidence { index }),
                    None => {
                        let evidence = crate::types::class_env::dictionary_name(
                            superclass,
                            &type_key,
                            &self.interner,
                        );
                        match self.interner.lookup(&evidence) {
                            Some(global) => slots.push(ContextualDictSlot::Evidence { global }),
                            None => complete = false,
                        }
                    }
                }
            }
            for method in &class_def.methods {
                let mangled = crate::types::class_env::mangled_method_name(
                    instance.class_id,
                    &type_key,
                    self.interner.resolve(method.name),
                    &self.interner,
                );
                match self.interner.lookup(&mangled) {
                    Some(mangled) => slots.push(ContextualDictSlot::Method {
                        mangled,
                        arity: method.arity + context_arity,
                    }),
                    None => complete = false,
                }
            }
            // A marker class (`Sendable`) has no superclasses and no methods,
            // so its dictionary holds nothing and no call can project from it.
            // Storing one would be an `OpTuple 0` in every program.
            if complete && !slots.is_empty() {
                plans.push((dict_binding.index, context_arity, slots));
            }
        }
        plans
    }

    /// Push a plain instance's dictionary: a tuple of its evidence and method
    /// globals, with no context parameters to close over.
    fn emit_plain_dict_tuple(&mut self, slots: &[ContextualDictSlot]) -> bool {
        let Some(bindings): Option<Vec<_>> = slots
            .iter()
            .map(|slot| {
                let symbol = match slot {
                    ContextualDictSlot::ContextEvidence { .. } => return None,
                    ContextualDictSlot::Evidence { global } => *global,
                    ContextualDictSlot::Method { mangled, .. } => *mangled,
                };
                let binding = self.symbol_table.resolve(symbol)?;
                matches!(
                    binding.symbol_scope,
                    crate::compiler::symbol_scope::SymbolScope::Global
                )
                .then_some(binding)
            })
            .collect()
        else {
            return false;
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
        true
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
                    slots,
                } => {
                    if !self.emit_contextual_dict_constructor(*context_arity, slots) {
                        continue;
                    }
                }
            }

            self.emit(OpCode::OpSetGlobal, &[dict_binding.index]);
        }
    }

    /// Module bodies qualify their generated instance methods (for example
    /// `Flow.Json.__tc_Encode_Int_encode`), while typed dispatch refers to the
    /// canonical hidden name (`__tc_Encode_Int_encode`).  Install the latter
    /// as an alias after the module has initialized its functions.  Interface
    /// preloading already creates these canonical bindings; this also covers
    /// no-cache builds, where the dependency AST is compiled through the same
    /// compiler and no serialized interface supplies the alias.
    fn emit_instance_method_aliases(&mut self, ir_program: &IrProgram) {
        if ir_program.core.is_none() {
            return;
        }
        let aliases: Vec<(usize, usize)> = self
            .symbol_table
            .global_bindings()
            .into_iter()
            .filter_map(|source| {
                let qualified = self.interner.resolve(source.name);
                let (_, suffix) = qualified.rsplit_once('.')?;
                if !crate::types::class_env::is_generated_instance_method(suffix) {
                    return None;
                }
                let alias = self.interner.lookup(suffix)?;
                let target = self.symbol_table.resolve(alias)?;
                Some((source.index, target.index))
            })
            .collect();

        for (source, target) in aliases {
            self.emit(OpCode::OpGetGlobal, &[source]);
            self.emit(OpCode::OpSetGlobal, &[target]);
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
        slots: &[ContextualDictSlot],
    ) -> bool {
        use crate::runtime::compiled_function::CompiledFunction;
        use crate::runtime::value::Value;
        use std::sync::Arc;

        // Resolve every slot's global up front: emitting a partial constructor
        // would leave a malformed closure in the dictionary, and a slot short
        // of the layout would shift every slot after it.
        let Some(resolved): Option<Vec<_>> = slots
            .iter()
            .map(|slot| {
                let symbol = match slot {
                    // Reads a parameter, so there is no global to resolve.
                    ContextualDictSlot::ContextEvidence { .. } => return Some((0, slot)),
                    ContextualDictSlot::Evidence { global } => *global,
                    ContextualDictSlot::Method { mangled, .. } => *mangled,
                };
                let binding = self.symbol_table.resolve(symbol)?;
                matches!(
                    binding.symbol_scope,
                    crate::compiler::symbol_scope::SymbolScope::Global
                )
                .then_some((binding.index, slot))
            })
            .collect()
        else {
            return false;
        };

        let mut body = Vec::new();
        for (global_index, slot) in resolved {
            match slot {
                // Superclass evidence already in hand: one of the context
                // dictionaries this constructor was called with.
                ContextualDictSlot::ContextEvidence { index } => {
                    push_operand(&mut body, OpCode::OpGetLocal, *index);
                }
                // Superclass evidence from another instance's global.
                ContextualDictSlot::Evidence { .. } => {
                    push_operand(&mut body, OpCode::OpGetGlobal, global_index);
                }
                // Method: a closure over a forwarder that captures the context
                // dictionaries as free variables.
                ContextualDictSlot::Method { arity, .. } => {
                    let forwarder =
                        Self::contextual_dict_forwarder(global_index, context_arity, *arity);
                    let forwarder_index = self.add_constant(Value::Function(Arc::new(forwarder)));

                    for ctx in 0..context_arity {
                        push_operand(&mut body, OpCode::OpGetLocal, ctx);
                    }
                    push_closure(&mut body, forwarder_index, context_arity);
                }
            }
        }
        push_tuple(&mut body, slots.len());
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

/// One slot of a contextual instance's dictionary.
///
/// The slot kinds and their order are [`ClassEnv::dictionary_layout`]'s;
/// this enum only records what the Core definition was found to hold, so the
/// bytecode can reproduce it.
///
/// [`ClassEnv::dictionary_layout`]: crate::types::class_env::ClassEnv::dictionary_layout
enum ContextualDictSlot {
    /// Superclass evidence taken straight from one of the context
    /// dictionaries this constructor was handed.
    ///
    /// `instance Middle<Int> => Top<Int>` receives the `Middle<Int>`
    /// dictionary that `Top`'s superclass slot needs, so the slot is that
    /// parameter rather than a global.
    ContextEvidence {
        /// Which context parameter, by position.
        index: usize,
    },
    /// Superclass evidence read from another instance's dictionary global.
    Evidence {
        /// The `__dict_{Class}_{Type}` symbol.
        global: crate::syntax::Identifier,
    },
    /// A method closure capturing the context dictionaries.
    Method {
        /// The mangled `__tc_{Class}_{Type}_{method}` symbol.
        mangled: crate::syntax::Identifier,
        /// How many arguments the caller supplies, excluding the leading
        /// context dictionaries.
        arity: usize,
    },
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
        slots: Vec<ContextualDictSlot>,
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
            CoreExpr::Lam { params, body, .. } => Some(Self::ContextualConstructor {
                context_arity: params.len(),
                slots: Self::contextual_slots(
                    body,
                    &params.iter().map(|param| param.id).collect::<Vec<_>>(),
                )?,
            }),
            _ => None,
        }
    }

    /// Extract the method slots from a contextual dictionary body,
    /// `MakeTuple(λargs. __tc_*(ctx.., args..))`.
    ///
    /// Core lowering hoists each method closure into a `let`, so the tuple
    /// elements are usually variables referring to those bindings rather than
    /// inline lambdas. Both forms are accepted.
    fn contextual_slots(
        body: &crate::core::CoreExpr,
        context: &[CoreBinderId],
    ) -> Option<Vec<ContextualDictSlot>> {
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
            .map(|slot| Self::contextual_slot(slot, &bound, context))
            .collect()
    }

    /// Read one dictionary slot.
    ///
    /// A method slot is a lambda forwarding to a mangled method, either inline
    /// or reached through a `let` binding. A superclass evidence slot is a
    /// reference to another dictionary global, applied to this instance's
    /// context dictionaries when that superclass instance is contextual too.
    fn contextual_slot(
        slot: &crate::core::CoreExpr,
        bound: &HashMap<CoreBinderId, &crate::core::CoreExpr>,
        context: &[CoreBinderId],
    ) -> Option<ContextualDictSlot> {
        use crate::core::CoreExpr;

        let slot = match slot {
            CoreExpr::Var { var, .. } => var
                .binder
                .and_then(|binder| bound.get(&binder).copied())
                .unwrap_or(slot),
            other => other,
        };

        // The three shapes are distinguishable without reading any name: a
        // method slot is a closure, and superclass evidence is a reference to
        // a dictionary global — bare when that instance is plain, applied to
        // this instance's context dictionaries when it is contextual.
        match slot {
            CoreExpr::Var { var, .. } => match var
                .binder
                .and_then(|binder| context.iter().position(|param| *param == binder))
            {
                Some(index) => Some(ContextualDictSlot::ContextEvidence { index }),
                None => Some(ContextualDictSlot::Evidence { global: var.name }),
            },
            CoreExpr::Lam { params, body, .. } => {
                let CoreExpr::App { func, .. } = body.as_ref() else {
                    return None;
                };
                let CoreExpr::Var { var, .. } = func.as_ref() else {
                    return None;
                };
                Some(ContextualDictSlot::Method {
                    mangled: var.name,
                    arity: params.len(),
                })
            }
            _ => None,
        }
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
