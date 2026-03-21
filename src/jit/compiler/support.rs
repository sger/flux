use super::*;

pub(super) fn backend_ir_jit_support_error(
    ir_program: &IrProgram,
    interner: &Interner,
) -> Option<String> {
    if let Some(reason) = backend_ir_top_level_support_error(ir_program.top_level_items()) {
        return Some(reason);
    }

    let mut named_functions: HashSet<_> = ir_program
        .functions()
        .iter()
        .filter_map(|function| function.name)
        .collect();
    collect_backend_top_level_function_names(ir_program.top_level_items(), &mut named_functions);
    let global_names: HashSet<_> = ir_program.globals().iter().copied().collect();
    let mut imported_modules = HashSet::new();
    let mut import_aliases = HashMap::new();
    let mut adt_constructors = HashMap::new();
    collect_backend_top_level_declaration_metadata(
        ir_program.top_level_items(),
        &mut imported_modules,
        &mut import_aliases,
        &mut adt_constructors,
    );

    for function in ir_program.functions() {
        let Some(entry_block) = function
            .blocks
            .iter()
            .find(|block| block.id == function.entry)
        else {
            return Some("backend_ir JIT shape is missing a function entry block".to_string());
        };
        if !entry_block.params.is_empty() {
            return Some("backend_ir JIT shape has entry block parameters".to_string());
        }
        for block in &function.blocks {
            for instr in &block.instrs {
                match instr {
                    BackendIrInstr::Assign { expr, .. } => {
                        match expr {
                            BackendIrExpr::Const(_)
                            | BackendIrExpr::Var(_)
                            | BackendIrExpr::InterpolatedString(_)
                            | BackendIrExpr::Prefix { .. }
                            | BackendIrExpr::MakeTuple(_)
                            | BackendIrExpr::MakeArray(_)
                            | BackendIrExpr::MakeHash(_)
                            | BackendIrExpr::MakeList(_)
                            | BackendIrExpr::MakeAdt(_, _)
                            | BackendIrExpr::MakeClosure(_, _)
                            | BackendIrExpr::EmptyList
                            | BackendIrExpr::Index { .. }
                            | BackendIrExpr::MemberAccess { .. }
                            | BackendIrExpr::TupleFieldAccess { .. }
                            | BackendIrExpr::TupleArityTest { .. }
                            | BackendIrExpr::TagTest { .. }
                            | BackendIrExpr::TagPayload { .. }
                            | BackendIrExpr::ListTest { .. }
                            | BackendIrExpr::ListHead { .. }
                            | BackendIrExpr::ListTail { .. }
                            | BackendIrExpr::AdtTagTest { .. }
                            | BackendIrExpr::AdtField { .. }
                            | BackendIrExpr::None => {}
                            BackendIrExpr::Some(_)
                            | BackendIrExpr::Left(_)
                            | BackendIrExpr::Right(_)
                            | BackendIrExpr::Cons { .. }
                            | BackendIrExpr::Perform { .. }
                            | BackendIrExpr::DropReuse(_)
                            | BackendIrExpr::ReuseCons { .. }
                            | BackendIrExpr::ReuseSome { .. }
                            | BackendIrExpr::ReuseLeft { .. }
                            | BackendIrExpr::ReuseRight { .. }
                            | BackendIrExpr::ReuseAdt { .. }
                            | BackendIrExpr::IsUnique(_) => {}
                            BackendIrExpr::LoadName(name) => {
                                let is_supported_load = named_functions.contains(name)
                                    || global_names.contains(name)
                                    || adt_constructors.get(name).copied() == Some(0)
                                    || resolve_backend_module_name(
                                        &imported_modules,
                                        &import_aliases,
                                        interner,
                                        *name,
                                    )
                                    .is_some()
                                    || crate::runtime::base::get_base_function_index(
                                        interner.resolve(*name),
                                    )
                                    .is_some();
                                if !is_supported_load {
                                    return Some(
                                    "backend_ir JIT shape has an unresolved non-function LoadName"
                                        .to_string(),
                                );
                                }
                            }
                            BackendIrExpr::Binary(op, _, _) => match op {
                                IrBinaryOp::Add
                                | IrBinaryOp::IAdd
                                | IrBinaryOp::Sub
                                | IrBinaryOp::ISub
                                | IrBinaryOp::Mul
                                | IrBinaryOp::IMul
                                | IrBinaryOp::Div
                                | IrBinaryOp::IDiv
                                | IrBinaryOp::Mod
                                | IrBinaryOp::IMod
                                | IrBinaryOp::Eq
                                | IrBinaryOp::NotEq
                                | IrBinaryOp::Gt
                                | IrBinaryOp::Ge
                                | IrBinaryOp::Le
                                | IrBinaryOp::Lt
                                | IrBinaryOp::FAdd
                                | IrBinaryOp::FSub
                                | IrBinaryOp::FMul
                                | IrBinaryOp::FDiv => {}
                                IrBinaryOp::And | IrBinaryOp::Or => {}
                            },
                            _ => return Some(
                                "backend_ir JIT shape contains an unsupported backend expression"
                                    .to_string(),
                            ),
                        }
                    }
                    BackendIrInstr::Call { target, args, .. } => {
                        if let IrCallTarget::Named(name) = target {
                            let is_supported_target = named_functions.contains(name)
                                || global_names.contains(name)
                                || adt_constructors.contains_key(name)
                                || resolve_primop_call(interner.resolve(*name), args.len())
                                    .is_some()
                                || crate::runtime::base::get_base_function_index(
                                    interner.resolve(*name),
                                )
                                .is_some();
                            if !is_supported_target {
                                return Some(format!(
                                    "backend_ir JIT shape has an unresolved named call target {}",
                                    interner.resolve(*name)
                                ));
                            }
                        }
                    }
                    BackendIrInstr::HandleScope {
                        body_entry,
                        body_result,
                        arms,
                        ..
                    } => {
                        if !function.blocks.iter().any(|b| b.id == *body_entry) {
                            return Some(
                                "backend_ir JIT shape references a missing handle-scope body entry block"
                                    .to_string(),
                            );
                        }
                        if !function
                            .blocks
                            .iter()
                            .any(|b| b.params.iter().any(|p| p.var == *body_result))
                        {
                            return Some(
                                "backend_ir JIT shape is missing handle-scope continuation block parameters"
                                    .to_string(),
                            );
                        }
                        for arm in arms {
                            let Some(arm_fn) = ir_program
                                .functions()
                                .iter()
                                .find(|f| f.id == arm.function_id)
                            else {
                                return Some(
                                    "backend_ir JIT shape is missing a handle arm function"
                                        .to_string(),
                                );
                            };
                            if arm_fn.captures.len() != arm.capture_vars.len() {
                                return Some(
                                    "backend_ir JIT shape has inconsistent handle-arm capture metadata"
                                        .to_string(),
                                );
                            }
                        }
                    }
                    BackendIrInstr::AetherDrop { .. } => {
                        // Always supported — no-op hint.
                    }
                }
            }
            match &block.terminator {
                BackendIrTerminator::Return(_, _)
                | BackendIrTerminator::Jump(_, _, _)
                | BackendIrTerminator::Branch { .. }
                | BackendIrTerminator::Unreachable(_) => {}
                BackendIrTerminator::TailCall { .. } => {}
            }
        }
    }

    None
}

pub(super) fn backend_ir_top_level_support_error(
    items: &[crate::cfg::IrTopLevelItem],
) -> Option<String> {
    for item in items {
        match item {
            crate::cfg::IrTopLevelItem::Function { function_id, .. } => {
                if function_id.is_none() {
                    return Some(
                        "backend_ir JIT shape has a top-level function without a backend function id"
                            .to_string(),
                    );
                }
            }
            crate::cfg::IrTopLevelItem::Module { body, .. } => {
                if let Some(reason) = backend_ir_top_level_support_error(body) {
                    return Some(reason);
                }
            }
            crate::cfg::IrTopLevelItem::Import { .. }
            | crate::cfg::IrTopLevelItem::Data { .. }
            | crate::cfg::IrTopLevelItem::EffectDecl { .. }
            | crate::cfg::IrTopLevelItem::Let { .. }
            | crate::cfg::IrTopLevelItem::LetDestructure { .. }
            | crate::cfg::IrTopLevelItem::Return { .. }
            | crate::cfg::IrTopLevelItem::Expression { .. }
            | crate::cfg::IrTopLevelItem::Assign { .. } => {}
        }
    }

    None
}

fn resolve_backend_module_name(
    imported_modules: &HashSet<Identifier>,
    import_aliases: &HashMap<Identifier, Identifier>,
    interner: &Interner,
    name: Identifier,
) -> Option<Identifier> {
    import_aliases.get(&name).copied().or_else(|| {
        if imported_modules.contains(&name) || interner.resolve(name) == "Base" {
            Some(name)
        } else {
            None
        }
    })
}

pub(super) fn ordered_backend_blocks(function: &BackendIrFunction) -> Vec<&crate::cfg::IrBlock> {
    let block_defs: HashMap<BackendBlockId, &crate::cfg::IrBlock> = function
        .blocks
        .iter()
        .map(|block| (block.id, block))
        .collect();
    let mut ordered = Vec::with_capacity(function.blocks.len());
    let mut seen = HashSet::new();
    let mut stack = vec![function.entry];

    while let Some(block_id) = stack.pop() {
        if !seen.insert(block_id) {
            continue;
        }
        let Some(block) = block_defs.get(&block_id).copied() else {
            continue;
        };
        ordered.push(block);
        for succ in backend_terminator_successors(&block.terminator)
            .into_iter()
            .rev()
        {
            stack.push(succ);
        }
    }

    for block in &function.blocks {
        if seen.insert(block.id) {
            ordered.push(block);
        }
    }

    ordered
}

pub(super) fn backend_terminator_successors(
    terminator: &BackendIrTerminator,
) -> Vec<BackendBlockId> {
    let mut succs = Vec::with_capacity(2);
    match terminator {
        BackendIrTerminator::Jump(target, ..) => succs.push(*target),
        BackendIrTerminator::Branch {
            then_block,
            else_block,
            ..
        } => {
            succs.push(*then_block);
            succs.push(*else_block);
        }
        BackendIrTerminator::Return(..)
        | BackendIrTerminator::TailCall { .. }
        | BackendIrTerminator::Unreachable(..) => {}
    }
    succs
}

pub(super) fn collect_backend_top_level_declaration_metadata(
    items: &[crate::cfg::IrTopLevelItem],
    imported_modules: &mut HashSet<Identifier>,
    import_aliases: &mut HashMap<Identifier, Identifier>,
    adt_constructors: &mut HashMap<Identifier, usize>,
) {
    // ADT constructors and module metadata use shared utilities
    crate::cfg::metadata::collect_adt_constructors(items, adt_constructors);
    let mut module_names: Vec<Identifier> = Vec::new();
    crate::cfg::metadata::collect_module_metadata(items, &mut module_names, import_aliases);
    for name in module_names {
        imported_modules.insert(name);
    }
}

pub(super) fn register_backend_top_level_module_functions(
    items: &[crate::cfg::IrTopLevelItem],
    backend_function_metas: &HashMap<BackendFunctionId, JitFunctionMeta>,
    import_aliases: &HashMap<Identifier, Identifier>,
    scope: &mut Scope,
) {
    // Use shared generic collection, parameterized on JitFunctionMeta
    crate::cfg::metadata::collect_module_functions(
        items,
        None,
        &|fn_id| backend_function_metas.get(&fn_id).copied(),
        &mut scope.module_functions,
    );
    crate::cfg::metadata::apply_import_aliases(&mut scope.module_functions, import_aliases);

    // JIT-specific: track which ADT constructor belongs to which data type
    collect_adt_constructor_owners(items, scope);
}

pub(super) fn collect_adt_constructor_owners(
    items: &[crate::cfg::IrTopLevelItem],
    scope: &mut Scope,
) {
    for item in items {
        match item {
            crate::cfg::IrTopLevelItem::Data { name, variants, .. } => {
                for variant in variants {
                    scope.adt_constructor_owner.insert(variant.name, *name);
                }
            }
            crate::cfg::IrTopLevelItem::Module { body, .. } => {
                collect_adt_constructor_owners(body, scope);
            }
            _ => {}
        }
    }
}

pub(super) fn collect_backend_top_level_function_names(
    items: &[crate::cfg::IrTopLevelItem],
    names: &mut HashSet<Identifier>,
) {
    for item in items {
        match item {
            crate::cfg::IrTopLevelItem::Function {
                name,
                function_id: Some(_),
                ..
            } => {
                names.insert(*name);
            }
            crate::cfg::IrTopLevelItem::Module { body, .. } => {
                collect_backend_top_level_function_names(body, names);
            }
            _ => {}
        }
    }
}

pub(super) fn register_backend_top_level_named_functions(
    items: &[crate::cfg::IrTopLevelItem],
    backend_function_metas: &HashMap<BackendFunctionId, JitFunctionMeta>,
    scope: &mut Scope,
) {
    for item in items {
        match item {
            crate::cfg::IrTopLevelItem::Function {
                name,
                function_id: Some(function_id),
                ..
            } => {
                if let Some(meta) = backend_function_metas.get(function_id).copied() {
                    scope.functions.insert(*name, meta);
                }
            }
            crate::cfg::IrTopLevelItem::Module { body, .. } => {
                register_backend_top_level_named_functions(body, backend_function_metas, scope);
            }
            _ => {}
        }
    }
}

pub(super) fn register_base_functions(scope: &mut Scope, interner: &Interner) {
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

#[allow(dead_code)]
fn is_base_symbol(name: Identifier, interner: &Interner) -> bool {
    interner
        .try_resolve(name)
        .is_some_and(|name| name == "Base")
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------
