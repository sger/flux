use std::{collections::{HashMap, HashSet}, fmt, sync::Arc};

use crate::{
    core::{CoreBinder, CoreBinderId, CoreDef, CoreExpr, CoreProgram},
    core_to_llvm::{
        CallConv, GlobalId, LabelId, Linkage, LlvmBlock, LlvmFunction, LlvmFunctionSig, LlvmInstr,
        LlvmLocal, LlvmModule, LlvmOperand, LlvmTerminator, LlvmType, LlvmValueKind,
        emit_adt_support, emit_closure_support, emit_prelude_and_arith,
    },
    syntax::{Identifier, interner::Interner},
};

use super::{adt::AdtMetadata, closure::common_closure_load_instrs, expr::FunctionLowering};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreToLlvmError {
    Unsupported {
        feature: &'static str,
        context: String,
    },
    Malformed {
        message: String,
    },
    MissingSymbol {
        message: String,
    },
}

impl fmt::Display for CoreToLlvmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CoreToLlvmError::Unsupported { feature, context } => {
                write!(f, "unsupported CoreToLlvm feature `{feature}`: {context}")
            }
            CoreToLlvmError::Malformed { message } => {
                write!(f, "malformed Core lowering: {message}")
            }
            CoreToLlvmError::MissingSymbol { message } => {
                write!(f, "missing CoreToLlvm symbol: {message}")
            }
        }
    }
}

impl std::error::Error for CoreToLlvmError {}

#[derive(Debug, Clone)]
pub(super) struct TopLevelFunctionInfo {
    pub symbol: GlobalId,
    pub arity: usize,
    pub name: Identifier,
}

pub(super) struct ProgramState<'a> {
    pub interner: Option<&'a Interner>,
    pub top_level: HashMap<CoreBinderId, TopLevelFunctionInfo>,
    pub adt_metadata: AdtMetadata,
    pub generated_functions: Vec<LlvmFunction>,
    pub top_level_wrappers: HashMap<CoreBinderId, GlobalId>,
    pub next_lambda_id: u32,
    /// Builtin C runtime functions referenced during codegen.
    pub needed_builtins: Vec<&'static super::builtins::BuiltinMapping>,
    /// String literal globals to emit (name, content).
    pub generated_string_globals: Vec<(GlobalId, String)>,
    /// C runtime declarations needed (name, param types, return type).
    pub needed_c_decls: Vec<(String, Vec<LlvmType>, LlvmType)>,
    /// Mutual tail-recursion groups (Phase 2 TCO).
    pub mutual_rec_groups: HashMap<CoreBinderId, Arc<MutualRecGroup>>,
}

impl<'a> ProgramState<'a> {
    fn new(
        top_level: HashMap<CoreBinderId, TopLevelFunctionInfo>,
        adt_metadata: AdtMetadata,
        interner: Option<&'a Interner>,
    ) -> Self {
        Self {
            interner,
            top_level,
            adt_metadata,
            generated_functions: Vec::new(),
            top_level_wrappers: HashMap::new(),
            next_lambda_id: 0,
            needed_builtins: Vec::new(),
            generated_string_globals: Vec::new(),
            needed_c_decls: Vec::new(),
            mutual_rec_groups: HashMap::new(),
        }
    }

    /// Track a C runtime function declaration needed by the codegen.
    pub fn ensure_c_decl(&mut self, name: &str, params: &[LlvmType], ret: LlvmType) {
        if !self.needed_c_decls.iter().any(|(n, _, _)| n == name) {
            self.needed_c_decls
                .push((name.to_string(), params.to_vec(), ret));
        }
    }

    pub fn register_builtin(&mut self, mapping: &'static super::builtins::BuiltinMapping) {
        if !self
            .needed_builtins
            .iter()
            .any(|m| m.c_name == mapping.c_name)
        {
            self.needed_builtins.push(mapping);
        }
    }

    pub fn fresh_lambda_symbol(&mut self, hint: &str) -> GlobalId {
        let id = self.next_lambda_id;
        self.next_lambda_id += 1;
        GlobalId(format!("{}.lambda.{id}", sanitize_symbol_fragment(hint)))
    }

    pub fn push_generated_function(&mut self, function: LlvmFunction) {
        self.generated_functions.push(function);
    }

    pub fn top_level_info(&self, binder: CoreBinderId) -> Option<&TopLevelFunctionInfo> {
        self.top_level.get(&binder)
    }

    /// Look up a top-level function by its Identifier (name), for MemberAccess resolution.
    /// Returns (CoreBinderId, &TopLevelFunctionInfo) so the caller can use ensure_top_level_wrapper.
    pub fn top_level_by_name_with_binder(
        &self,
        name: Identifier,
    ) -> Option<(CoreBinderId, TopLevelFunctionInfo)> {
        self.top_level
            .iter()
            .find(|(_, info)| info.name == name)
            .map(|(k, v)| (*k, v.clone()))
    }

    pub fn ensure_top_level_wrapper(
        &mut self,
        binder: CoreBinderId,
    ) -> Result<GlobalId, CoreToLlvmError> {
        if let Some(symbol) = self.top_level_wrappers.get(&binder) {
            return Ok(symbol.clone());
        }
        let info =
            self.top_level
                .get(&binder)
                .cloned()
                .ok_or_else(|| CoreToLlvmError::MissingSymbol {
                    message: format!("missing wrapper target for binder {:?}", binder),
                })?;
        let wrapper = GlobalId(format!(
            "{}.closure_wrapper",
            sanitize_symbol_name(info.name, self.interner)
        ));
        let function = build_top_level_wrapper(&wrapper, &info.symbol, info.arity);
        self.generated_functions.push(function);
        self.top_level_wrappers.insert(binder, wrapper.clone());
        Ok(wrapper)
    }
}

// ── Non-tail self-recursion detection (Phase 3 CPS) ──────────────────────────

/// Check if a function has any non-tail recursive calls to itself.
/// Returns true if the function body contains at least one `App`/`AetherCall`
/// to the function's own binder in a non-tail position.
pub(super) fn has_nontail_self_recursion(def: &CoreDef) -> bool {
    let CoreExpr::Lam { body, .. } = &def.expr else {
        return false;
    };
    has_nontail_self_call(body, def.binder.id, true)
}

/// Recursively check if `expr` contains a non-tail call to `self_id`.
fn has_nontail_self_call(expr: &CoreExpr, self_id: CoreBinderId, in_tail: bool) -> bool {
    match expr {
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => false,
        CoreExpr::Lam { body, .. } => {
            // Lambda body is a new function scope — NOT tail of the enclosing one.
            has_nontail_self_call(body, self_id, false)
        }
        CoreExpr::App { func, args, .. } | CoreExpr::AetherCall { func, args, .. } => {
            // If this is a call to self and we're NOT in tail position → found it.
            if !in_tail
                && let CoreExpr::Var { var, .. } = func.as_ref()
                && var.binder == Some(self_id)
            {
                return true;
            }
            // Check subexpressions (all non-tail).
            has_nontail_self_call(func, self_id, false)
                || args.iter().any(|a| has_nontail_self_call(a, self_id, false))
        }
        CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
            has_nontail_self_call(rhs, self_id, false)
                || has_nontail_self_call(body, self_id, in_tail)
        }
        CoreExpr::Case { scrutinee, alts, .. } => {
            has_nontail_self_call(scrutinee, self_id, false)
                || alts.iter().any(|alt| {
                    alt.guard
                        .as_ref()
                        .is_some_and(|g| has_nontail_self_call(g, self_id, false))
                        || has_nontail_self_call(&alt.rhs, self_id, in_tail)
                })
        }
        CoreExpr::Con { fields, .. } | CoreExpr::PrimOp { args: fields, .. } => {
            fields.iter().any(|f| has_nontail_self_call(f, self_id, false))
        }
        CoreExpr::Return { value, .. } => has_nontail_self_call(value, self_id, in_tail),
        CoreExpr::Perform { args, .. } => {
            args.iter().any(|a| has_nontail_self_call(a, self_id, false))
        }
        CoreExpr::Handle { body, handlers, .. } => {
            has_nontail_self_call(body, self_id, false)
                || handlers
                    .iter()
                    .any(|h| has_nontail_self_call(&h.body, self_id, false))
        }
        CoreExpr::Dup { body, .. } | CoreExpr::Drop { body, .. } => {
            has_nontail_self_call(body, self_id, in_tail)
        }
        CoreExpr::Reuse { fields, .. } => {
            fields.iter().any(|f| has_nontail_self_call(f, self_id, false))
        }
        CoreExpr::DropSpecialized {
            unique_body,
            shared_body,
            ..
        } => {
            has_nontail_self_call(unique_body, self_id, in_tail)
                || has_nontail_self_call(shared_body, self_id, in_tail)
        }
    }
}

// ── Mutual tail-call recursion detection ─────────────────────────────────────

/// A group of mutually tail-recursive functions (SCC of size >= 2 on the
/// tail-call graph).  Each member gets a `fn_index` used by the trampoline.
#[derive(Debug, Clone)]
pub(super) struct MutualRecGroup {
    /// Symbol names of group members, indexed by `fn_index`.
    pub members: Vec<CoreBinderId>,
    /// Binder → fn_index mapping.
    pub member_index: HashMap<CoreBinderId, u8>,
    /// A descriptive name for the trampoline function (e.g., "isEven_isOdd").
    pub trampoline_name: String,
}

/// Collect only tail-call edges from a Core expression.
/// Unlike `collect_local_callees` (which collects ALL callees), this only
/// records calls that are in tail position relative to the enclosing function.
fn collect_tail_callees(
    expr: &CoreExpr,
    def_ids: &HashSet<CoreBinderId>,
    out: &mut HashSet<CoreBinderId>,
    in_tail: bool,
) {
    match expr {
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => {}
        CoreExpr::Lam { body, .. } => {
            // Lambda body starts a new function — not tail of the enclosing one.
            collect_tail_callees(body, def_ids, out, false);
        }
        CoreExpr::App { func, args, .. } | CoreExpr::AetherCall { func, args, .. } => {
            if in_tail
                && let CoreExpr::Var { var, .. } = func.as_ref()
                && let Some(binder) = var.binder
                && def_ids.contains(&binder)
            {
                out.insert(binder);
            }
            // Subexpressions of a call are never in tail position.
            collect_tail_callees(func, def_ids, out, false);
            for arg in args {
                collect_tail_callees(arg, def_ids, out, false);
            }
        }
        CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
            collect_tail_callees(rhs, def_ids, out, false);
            collect_tail_callees(body, def_ids, out, in_tail); // body inherits tail
        }
        CoreExpr::Case { scrutinee, alts, .. } => {
            collect_tail_callees(scrutinee, def_ids, out, false);
            for alt in alts {
                if let Some(guard) = &alt.guard {
                    collect_tail_callees(guard, def_ids, out, false);
                }
                collect_tail_callees(&alt.rhs, def_ids, out, in_tail); // arms inherit tail
            }
        }
        CoreExpr::Con { fields, .. } | CoreExpr::PrimOp { args: fields, .. } => {
            for field in fields {
                collect_tail_callees(field, def_ids, out, false);
            }
        }
        CoreExpr::Return { value, .. } => {
            collect_tail_callees(value, def_ids, out, in_tail);
        }
        CoreExpr::Perform { args, .. } => {
            for arg in args {
                collect_tail_callees(arg, def_ids, out, false);
            }
        }
        CoreExpr::Handle { body, handlers, .. } => {
            collect_tail_callees(body, def_ids, out, false);
            for handler in handlers {
                collect_tail_callees(&handler.body, def_ids, out, false);
            }
        }
        CoreExpr::Dup { body, .. } | CoreExpr::Drop { body, .. } => {
            collect_tail_callees(body, def_ids, out, in_tail); // transparent for tail
        }
        CoreExpr::Reuse { fields, .. } => {
            for field in fields {
                collect_tail_callees(field, def_ids, out, false);
            }
        }
        CoreExpr::DropSpecialized {
            unique_body,
            shared_body,
            ..
        } => {
            collect_tail_callees(unique_body, def_ids, out, in_tail);
            collect_tail_callees(shared_body, def_ids, out, in_tail);
        }
    }
}

/// Compute mutual tail-recursion groups using Tarjan's SCC on the tail-call
/// graph.  Returns a map from each member's `CoreBinderId` to its group.
/// Only groups of size >= 2 are included (self-recursive = Phase 1).
fn compute_mutual_rec_groups(
    core: &CoreProgram,
    interner: Option<&Interner>,
) -> HashMap<CoreBinderId, Arc<MutualRecGroup>> {
    let def_ids: HashSet<_> = core.defs.iter().map(|def| def.binder.id).collect();

    // Build tail-call adjacency graph.
    let adjacency: HashMap<CoreBinderId, Vec<CoreBinderId>> = core
        .defs
        .iter()
        .map(|def| {
            let mut callees = HashSet::new();
            // The body of a top-level Lam starts in tail position.
            if let CoreExpr::Lam { body, .. } = &def.expr {
                collect_tail_callees(body, &def_ids, &mut callees, true);
            }
            // Remove self-edges (handled by Phase 1).
            callees.remove(&def.binder.id);
            (def.binder.id, callees.into_iter().collect())
        })
        .collect();

    // Tarjan's SCC algorithm.
    let mut index = 0usize;
    let mut stack = Vec::new();
    let mut on_stack = HashSet::new();
    let mut indices = HashMap::<CoreBinderId, usize>::new();
    let mut lowlinks = HashMap::<CoreBinderId, usize>::new();
    let mut components = Vec::new();

    #[allow(clippy::too_many_arguments)]
    fn strongconnect(
        v: CoreBinderId,
        adjacency: &HashMap<CoreBinderId, Vec<CoreBinderId>>,
        index: &mut usize,
        stack: &mut Vec<CoreBinderId>,
        on_stack: &mut HashSet<CoreBinderId>,
        indices: &mut HashMap<CoreBinderId, usize>,
        lowlinks: &mut HashMap<CoreBinderId, usize>,
        components: &mut Vec<Vec<CoreBinderId>>,
    ) {
        indices.insert(v, *index);
        lowlinks.insert(v, *index);
        *index += 1;
        stack.push(v);
        on_stack.insert(v);

        for w in adjacency.get(&v).into_iter().flatten().copied() {
            if !indices.contains_key(&w) {
                strongconnect(w, adjacency, index, stack, on_stack, indices, lowlinks, components);
                let low_v = lowlinks[&v];
                let low_w = lowlinks[&w];
                lowlinks.insert(v, low_v.min(low_w));
            } else if on_stack.contains(&w) {
                let low_v = lowlinks[&v];
                let idx_w = indices[&w];
                lowlinks.insert(v, low_v.min(idx_w));
            }
        }

        if indices[&v] == lowlinks[&v] {
            let mut component = Vec::new();
            while let Some(w) = stack.pop() {
                on_stack.remove(&w);
                component.push(w);
                if w == v {
                    break;
                }
            }
            components.push(component);
        }
    }

    for def in &core.defs {
        if !indices.contains_key(&def.binder.id) {
            strongconnect(
                def.binder.id, &adjacency, &mut index, &mut stack, &mut on_stack,
                &mut indices, &mut lowlinks, &mut components,
            );
        }
    }

    // Build MutualRecGroup for SCCs with >= 2 members.
    let mut result = HashMap::new();
    for component in components {
        if component.len() < 2 {
            continue;
        }
        let member_index: HashMap<CoreBinderId, u8> = component
            .iter()
            .enumerate()
            .map(|(i, &id)| (id, i as u8))
            .collect();

        // Build a descriptive trampoline name from member function names.
        let name_parts: Vec<String> = component
            .iter()
            .map(|id| {
                core.defs
                    .iter()
                    .find(|d| d.binder.id == *id)
                    .map(|d| display_ident(d.name, interner))
                    .unwrap_or_else(|| format!("{:?}", id))
            })
            .collect();
        let trampoline_name = format!("trampoline.{}", name_parts.join("_"));

        let group = Arc::new(MutualRecGroup {
            members: component,
            member_index,
            trampoline_name,
        });
        for &member in &group.members {
            result.insert(member, Arc::clone(&group));
        }
    }
    result
}

pub fn compile_program(core: &CoreProgram) -> Result<LlvmModule, CoreToLlvmError> {
    compile_program_with_interner(core, None)
}

pub fn compile_program_with_interner(
    core: &CoreProgram,
    interner: Option<&Interner>,
) -> Result<LlvmModule, CoreToLlvmError> {
    let mut module = LlvmModule::new();
    emit_prelude_and_arith(&mut module);
    emit_closure_support(&mut module);
    let adt_metadata = AdtMetadata::collect(core, interner)?;
    emit_adt_support(&mut module, &adt_metadata);

    let mut top_level = HashMap::new();
    for def in &core.defs {
        let CoreExpr::Lam { params, .. } = &def.expr else {
            return Err(CoreToLlvmError::Unsupported {
                feature: "top-level value definitions",
                context: format!(
                    "definition `{}` is not a lambda",
                    display_ident(def.name, interner)
                ),
            });
        };
        let raw_name = sanitize_symbol_name(def.name, interner);
        // Rename user's `main` to `flux_main` so the C runtime can call it.
        let symbol_name = if raw_name == "main" {
            "flux_main".to_string()
        } else {
            raw_name
        };
        top_level.insert(
            def.binder.id,
            TopLevelFunctionInfo {
                symbol: GlobalId(symbol_name),
                arity: params.len(),
                name: def.name,
            },
        );
    }

    let mut program = ProgramState::new(top_level, adt_metadata, interner);
    program.mutual_rec_groups = compute_mutual_rec_groups(core, interner);

    // Track which groups we've already generated trampolines for.
    let mut generated_trampolines: HashSet<String> = HashSet::new();

    for def in &core.defs {
        let info = program
            .top_level
            .get(&def.binder.id)
            .cloned()
            .ok_or_else(|| CoreToLlvmError::MissingSymbol {
                message: format!(
                    "missing top-level symbol for `{}`",
                    display_ident(def.name, interner)
                ),
            })?;

        let mutual_group = program.mutual_rec_groups.get(&def.binder.id).cloned();

        if has_nontail_self_recursion(def) {
            // Phase 3: CPS driver loop for non-tail self-recursion.
            let function = lower_top_level_function_cps(
                def,
                info.symbol.clone(),
                &mut program,
            )?;
            module.functions.push(function);
        } else if let Some(ref group) = mutual_group {
            // Phase 2: mutual tail-call trampoline.
            let impl_symbol = GlobalId(format!("{}.impl", info.symbol.0));
            let function = lower_top_level_function_with_mutual(
                def,
                impl_symbol.clone(),
                def.is_recursive,
                def.binder.id,
                group.clone(),
                &mut program,
            )?;
            module.functions.push(function);

            let wrapper = build_trampoline_entry_wrapper(
                &info.symbol,
                &impl_symbol,
                info.arity,
                group,
                def.binder.id,
                &program,
            );
            module.functions.push(wrapper);

            if generated_trampolines.insert(group.trampoline_name.clone()) {
                let trampoline = build_trampoline_function(group, &program);
                module.functions.push(trampoline);
            }
        } else {
            // Phase 1 or normal lowering.
            let function = lower_top_level_function(
                def,
                info.symbol.clone(),
                def.is_recursive,
                &mut program,
            )?;
            module.functions.push(function);
        }
    }
    module.functions.extend(program.generated_functions);

    // Make flux_main externally visible so the C runtime's main() can call it.
    // Also use ccc (C calling convention) for the entry point.
    for func in &mut module.functions {
        if func.name.0 == "flux_main" {
            func.linkage = crate::core_to_llvm::Linkage::External;
            func.sig.call_conv = crate::core_to_llvm::CallConv::Ccc;
        }
    }

    // Add C runtime declarations for any builtin functions referenced.
    for mapping in &program.needed_builtins {
        super::builtins::ensure_builtin_declared(&mut module, mapping);
    }

    // Add C runtime declarations requested by codegen.
    for (name, params, ret) in &program.needed_c_decls {
        if !module.declarations.iter().any(|d| d.name.0 == *name)
            && !module.functions.iter().any(|f| f.name.0 == *name)
        {
            module.declarations.push(crate::core_to_llvm::LlvmDecl {
                linkage: crate::core_to_llvm::Linkage::External,
                name: GlobalId(name.clone()),
                sig: crate::core_to_llvm::LlvmFunctionSig {
                    ret: ret.clone(),
                    params: params.clone(),
                    varargs: false,
                    call_conv: crate::core_to_llvm::CallConv::Ccc,
                },
                attrs: vec!["nounwind".into()],
            });
        }
    }

    // Emit string literal globals.
    for (name, content) in &program.generated_string_globals {
        module.globals.push(crate::core_to_llvm::LlvmGlobal {
            linkage: crate::core_to_llvm::Linkage::Private,
            name: name.clone(),
            ty: LlvmType::Array {
                len: content.len() as u64,
                element: Box::new(LlvmType::i8()),
            },
            is_constant: true,
            value: crate::core_to_llvm::LlvmConst::Array {
                element_ty: LlvmType::i8(),
                elements: content
                    .bytes()
                    .map(|b| crate::core_to_llvm::LlvmConst::Int {
                        bits: 8,
                        value: b as i128,
                    })
                    .collect(),
            },
            attrs: vec![],
        });
    }

    // Ensure flux_string_new is declared with its actual signature (ptr, i32) → i64.
    if !program.generated_string_globals.is_empty() {
        let name = "flux_string_new";
        if !module.declarations.iter().any(|d| d.name.0 == name)
            && !module.functions.iter().any(|f| f.name.0 == name)
        {
            module.declarations.push(crate::core_to_llvm::LlvmDecl {
                linkage: crate::core_to_llvm::Linkage::External,
                name: GlobalId(name.into()),
                sig: crate::core_to_llvm::LlvmFunctionSig {
                    ret: LlvmType::i64(),
                    params: vec![LlvmType::ptr(), LlvmType::i32()],
                    varargs: false,
                    call_conv: crate::core_to_llvm::CallConv::Ccc,
                },
                attrs: vec!["nounwind".into()],
            });
        }
    }

    Ok(module)
}

pub(super) fn display_ident(ident: Identifier, interner: Option<&Interner>) -> String {
    interner
        .map(|it| it.resolve(ident).to_string())
        .unwrap_or_else(|| ident.to_string())
}

pub(super) fn sanitize_symbol_name(ident: Identifier, interner: Option<&Interner>) -> String {
    sanitize_symbol_fragment(&display_ident(ident, interner))
}

pub(super) fn sanitize_symbol_fragment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() { "_anon".into() } else { out }
}

fn lower_top_level_function(
    def: &CoreDef,
    symbol: GlobalId,
    is_recursive: bool,
    program: &mut ProgramState<'_>,
) -> Result<LlvmFunction, CoreToLlvmError> {
    let CoreExpr::Lam { params, body, .. } = &def.expr else {
        return Err(CoreToLlvmError::Malformed {
            message: format!(
                "top-level function `{}` was not lowered as Lam",
                display_ident(def.name, program.interner)
            ),
        });
    };

    let mut lowering = FunctionLowering::new_top_level(symbol.clone(), params, program);
    if is_recursive {
        lowering.setup_tco_loop();
    }
    let result = lowering.lower_expr(body)?;
    lowering.finish_with_return(result)
}

/// Lower a function with non-tail self-recursion using the Phase 3 CPS
/// driver loop (explicit continuation stack).
fn lower_top_level_function_cps(
    def: &CoreDef,
    symbol: GlobalId,
    program: &mut ProgramState<'_>,
) -> Result<LlvmFunction, CoreToLlvmError> {
    let CoreExpr::Lam { params, body, .. } = &def.expr else {
        return Err(CoreToLlvmError::Malformed {
            message: format!(
                "top-level function `{}` was not lowered as Lam",
                display_ident(def.name, program.interner)
            ),
        });
    };

    let mut lowering = FunctionLowering::new_top_level(symbol.clone(), params, program);
    lowering.setup_cps_driver(def.binder.id);
    let body_result = lowering.lower_expr(body)?;
    let final_result = lowering.finalize_cps(body_result)?;
    lowering.finish_with_return(final_result)
}

/// Like `lower_top_level_function`, but sets up mutual group info so that
/// cross-function tail calls within the group emit thunk returns.
fn lower_top_level_function_with_mutual(
    def: &CoreDef,
    symbol: GlobalId,
    is_recursive: bool,
    binder: CoreBinderId,
    group: Arc<MutualRecGroup>,
    program: &mut ProgramState<'_>,
) -> Result<LlvmFunction, CoreToLlvmError> {
    let CoreExpr::Lam { params, body, .. } = &def.expr else {
        return Err(CoreToLlvmError::Malformed {
            message: format!(
                "top-level function `{}` was not lowered as Lam",
                display_ident(def.name, program.interner)
            ),
        });
    };

    let mut lowering = FunctionLowering::new_top_level(symbol.clone(), params, program);
    lowering.mutual_group = Some((binder, group));
    if is_recursive {
        lowering.setup_tco_loop();
    }
    let result = lowering.lower_expr(body)?;
    lowering.finish_with_return(result)
}

/// Build a trampoline function for a mutual recursion group.
///
/// ```llvm
/// define fastcc i64 @trampoline.NAME(i8 %fn_index, ptr %args, i32 %nargs) {
///   loop: switch on fn_index → call each member's .impl
///   check: if is_thunk → unpack, loop; else → ret
/// }
/// ```
fn build_trampoline_function(
    group: &MutualRecGroup,
    program: &ProgramState<'_>,
) -> LlvmFunction {
    use crate::core_to_llvm::LlvmConst;
    use super::prelude::flux_prelude_symbol;

    let trampoline_sym = GlobalId(group.trampoline_name.clone());

    let mut blocks = Vec::new();

    // entry: br label %loop
    blocks.push(LlvmBlock {
        label: LabelId("entry".into()),
        instrs: vec![],
        term: LlvmTerminator::Br {
            target: LabelId("loop".into()),
        },
    });

    // loop: phi nodes for fn_index, args, nargs
    //   switch on fn_index → call.0, call.1, ...
    let loop_instrs = vec![
        LlvmInstr::Phi {
            dst: LlvmLocal("cur.fn".into()),
            ty: LlvmType::i8(),
            incoming: vec![
                (LlvmOperand::Local(LlvmLocal("fn_index".into())), LabelId("entry".into())),
                (LlvmOperand::Local(LlvmLocal("next.fn".into())), LabelId("continue".into())),
            ],
        },
        LlvmInstr::Phi {
            dst: LlvmLocal("cur.args".into()),
            ty: LlvmType::ptr(),
            incoming: vec![
                (LlvmOperand::Local(LlvmLocal("args".into())), LabelId("entry".into())),
                (LlvmOperand::Local(LlvmLocal("next.args".into())), LabelId("continue".into())),
            ],
        },
        LlvmInstr::Phi {
            dst: LlvmLocal("cur.nargs".into()),
            ty: LlvmType::i32(),
            incoming: vec![
                (LlvmOperand::Local(LlvmLocal("nargs".into())), LabelId("entry".into())),
                (LlvmOperand::Local(LlvmLocal("next.nargs".into())), LabelId("continue".into())),
            ],
        },
    ];

    // Build switch cases
    let mut switch_cases = Vec::new();
    for (i, _member_id) in group.members.iter().enumerate() {
        let label = LabelId(format!("call.{i}"));
        switch_cases.push((
            LlvmConst::Int { bits: 8, value: i as i128 },
            label,
        ));
    }

    blocks.push(LlvmBlock {
        label: LabelId("loop".into()),
        instrs: loop_instrs,
        term: LlvmTerminator::Switch {
            ty: LlvmType::i8(),
            scrutinee: LlvmOperand::Local(LlvmLocal("cur.fn".into())),
            default: LabelId("unreachable".into()),
            cases: switch_cases,
        },
    });

    // Generate call blocks: each loads args and calls the .impl function
    let mut check_incoming = Vec::new();
    for (i, &member_id) in group.members.iter().enumerate() {
        let info = program.top_level.get(&member_id).expect("mutual group member in top_level");
        let impl_symbol = GlobalId(format!("{}.impl", info.symbol.0));
        let arity = info.arity;
        let label = LabelId(format!("call.{i}"));

        let mut instrs = Vec::new();

        // Load each argument from cur.args
        let mut call_args = Vec::new();
        for j in 0..arity {
            let slot = LlvmLocal(format!("call.{i}.arg.ptr.{j}"));
            instrs.push(LlvmInstr::GetElementPtr {
                dst: slot.clone(),
                inbounds: true,
                element_ty: LlvmType::i64(),
                base: LlvmOperand::Local(LlvmLocal("cur.args".into())),
                indices: vec![(LlvmType::i32(), LlvmOperand::Const(LlvmConst::Int { bits: 32, value: j as i128 }))],
            });
            let arg = LlvmLocal(format!("call.{i}.arg.{j}"));
            instrs.push(LlvmInstr::Load {
                dst: arg.clone(),
                ty: LlvmType::i64(),
                ptr: LlvmOperand::Local(slot),
                align: Some(8),
            });
            call_args.push((LlvmType::i64(), LlvmOperand::Local(arg)));
        }

        // Call the .impl function
        let result = LlvmLocal(format!("call.{i}.result"));
        instrs.push(LlvmInstr::Call {
            dst: Some(result.clone()),
            tail: false,
            call_conv: Some(CallConv::Fastcc),
            ret_ty: LlvmType::i64(),
            callee: LlvmOperand::Global(impl_symbol),
            args: call_args,
            attrs: vec![],
        });

        check_incoming.push((LlvmOperand::Local(result), label.clone()));

        blocks.push(LlvmBlock {
            label,
            instrs,
            term: LlvmTerminator::Br {
                target: LabelId("check".into()),
            },
        });
    }

    // check: phi result, test is_thunk, branch to continue or done
    blocks.push(LlvmBlock {
        label: LabelId("check".into()),
        instrs: vec![
            LlvmInstr::Phi {
                dst: LlvmLocal("result".into()),
                ty: LlvmType::i64(),
                incoming: check_incoming,
            },
            LlvmInstr::Call {
                dst: Some(LlvmLocal("is_thunk".into())),
                tail: false,
                call_conv: Some(CallConv::Fastcc),
                ret_ty: LlvmType::i1(),
                callee: LlvmOperand::Global(flux_prelude_symbol("flux_is_thunk")),
                args: vec![(LlvmType::i64(), LlvmOperand::Local(LlvmLocal("result".into())))],
                attrs: vec![],
            },
        ],
        term: LlvmTerminator::CondBr {
            cond_ty: LlvmType::i1(),
            cond: LlvmOperand::Local(LlvmLocal("is_thunk".into())),
            then_label: LabelId("continue".into()),
            else_label: LabelId("done".into()),
        },
    });

    // continue: unpack thunk → next.fn, next.args, next.nargs, branch to loop
    blocks.push(LlvmBlock {
        label: LabelId("continue".into()),
        instrs: vec![
            // Untag the thunk pointer
            LlvmInstr::Call {
                dst: Some(LlvmLocal("thunk.ptr".into())),
                tail: false,
                call_conv: Some(CallConv::Fastcc),
                ret_ty: LlvmType::ptr(),
                callee: LlvmOperand::Global(flux_prelude_symbol("flux_untag_thunk_ptr")),
                args: vec![(LlvmType::i64(), LlvmOperand::Local(LlvmLocal("result".into())))],
                attrs: vec![],
            },
            // Load fn_index (i8 at offset 0)
            LlvmInstr::Load {
                dst: LlvmLocal("next.fn".into()),
                ty: LlvmType::i8(),
                ptr: LlvmOperand::Local(LlvmLocal("thunk.ptr".into())),
                align: Some(1),
            },
            // Load nargs (i32 at offset 4)
            LlvmInstr::GetElementPtr {
                dst: LlvmLocal("thunk.nargs.ptr".into()),
                inbounds: true,
                element_ty: LlvmType::i32(),
                base: LlvmOperand::Local(LlvmLocal("thunk.ptr".into())),
                indices: vec![(LlvmType::i32(), LlvmOperand::Const(LlvmConst::Int { bits: 32, value: 1 }))],
            },
            LlvmInstr::Load {
                dst: LlvmLocal("next.nargs".into()),
                ty: LlvmType::i32(),
                ptr: LlvmOperand::Local(LlvmLocal("thunk.nargs.ptr".into())),
                align: Some(4),
            },
            // Args pointer (i64* at offset 8)
            LlvmInstr::GetElementPtr {
                dst: LlvmLocal("next.args".into()),
                inbounds: true,
                element_ty: LlvmType::i64(),
                base: LlvmOperand::Local(LlvmLocal("thunk.ptr".into())),
                indices: vec![(LlvmType::i32(), LlvmOperand::Const(LlvmConst::Int { bits: 32, value: 1 }))],
            },
        ],
        term: LlvmTerminator::Br {
            target: LabelId("loop".into()),
        },
    });

    // done: return the result
    blocks.push(LlvmBlock {
        label: LabelId("done".into()),
        instrs: vec![],
        term: LlvmTerminator::Ret {
            ty: LlvmType::i64(),
            value: LlvmOperand::Local(LlvmLocal("result".into())),
        },
    });

    // unreachable
    blocks.push(LlvmBlock {
        label: LabelId("unreachable".into()),
        instrs: vec![],
        term: LlvmTerminator::Unreachable,
    });

    LlvmFunction {
        linkage: Linkage::Internal,
        name: trampoline_sym,
        sig: LlvmFunctionSig {
            ret: LlvmType::i64(),
            params: vec![LlvmType::i8(), LlvmType::ptr(), LlvmType::i32()],
            varargs: false,
            call_conv: CallConv::Fastcc,
        },
        params: vec![
            LlvmLocal("fn_index".into()),
            LlvmLocal("args".into()),
            LlvmLocal("nargs".into()),
        ],
        attrs: vec![],
        blocks,
    }
}

/// Build an entry wrapper that calls the trampoline with the appropriate fn_index.
///
/// ```llvm
/// define fastcc i64 @isEven(i64 %arg0) {
///   %args = alloca [1 x i64]
///   store %arg0 into args[0]
///   %result = call fastcc i64 @trampoline.GROUP(i8 0, ptr %args, i32 1)
///   ret i64 %result
/// }
/// ```
fn build_trampoline_entry_wrapper(
    public_symbol: &GlobalId,
    _impl_symbol: &GlobalId,
    arity: usize,
    group: &MutualRecGroup,
    binder: CoreBinderId,
    _program: &ProgramState<'_>,
) -> LlvmFunction {
    use crate::core_to_llvm::LlvmConst;

    let fn_index = group.member_index[&binder];
    let trampoline_sym = GlobalId(group.trampoline_name.clone());

    let params: Vec<LlvmLocal> = (0..arity)
        .map(|i| LlvmLocal(format!("arg{i}")))
        .collect();
    let param_types: Vec<LlvmType> = (0..arity).map(|_| LlvmType::i64()).collect();

    let mut instrs = Vec::new();

    // Allocate args array on stack
    let args_ptr = LlvmLocal("entry.args".into());
    let count = arity.max(1) as i32;
    instrs.push(LlvmInstr::Alloca {
        dst: args_ptr.clone(),
        ty: LlvmType::i64(),
        count: Some((LlvmType::i32(), LlvmOperand::Const(LlvmConst::Int { bits: 32, value: count as i128 }))),
        align: Some(8),
    });

    // Store each arg
    for (i, param) in params.iter().enumerate() {
        let slot = LlvmLocal(format!("entry.args.slot.{i}"));
        instrs.push(LlvmInstr::GetElementPtr {
            dst: slot.clone(),
            inbounds: true,
            element_ty: LlvmType::i64(),
            base: LlvmOperand::Local(args_ptr.clone()),
            indices: vec![(LlvmType::i32(), LlvmOperand::Const(LlvmConst::Int { bits: 32, value: i as i128 }))],
        });
        instrs.push(LlvmInstr::Store {
            ty: LlvmType::i64(),
            value: LlvmOperand::Local(param.clone()),
            ptr: LlvmOperand::Local(slot),
            align: Some(8),
        });
    }

    // Call trampoline
    let result = LlvmLocal("entry.result".into());
    instrs.push(LlvmInstr::Call {
        dst: Some(result.clone()),
        tail: false,
        call_conv: Some(CallConv::Fastcc),
        ret_ty: LlvmType::i64(),
        callee: LlvmOperand::Global(trampoline_sym),
        args: vec![
            (LlvmType::i8(), LlvmOperand::Const(LlvmConst::Int { bits: 8, value: fn_index as i128 })),
            (LlvmType::ptr(), LlvmOperand::Local(args_ptr)),
            (LlvmType::i32(), LlvmOperand::Const(LlvmConst::Int { bits: 32, value: arity as i128 })),
        ],
        attrs: vec![],
    });

    LlvmFunction {
        linkage: Linkage::Internal,
        name: public_symbol.clone(),
        sig: LlvmFunctionSig {
            ret: LlvmType::i64(),
            params: param_types,
            varargs: false,
            call_conv: CallConv::Fastcc,
        },
        params,
        attrs: vec![],
        blocks: vec![LlvmBlock {
            label: LabelId("entry".into()),
            instrs,
            term: LlvmTerminator::Ret {
                ty: LlvmType::i64(),
                value: LlvmOperand::Local(result),
            },
        }],
    }
}

fn build_top_level_wrapper(wrapper: &GlobalId, target: &GlobalId, arity: usize) -> LlvmFunction {
    let mut state = FunctionState::new_closure_entry(wrapper.clone(), HashMap::new(), None);
    state.blocks[0]
        .instrs
        .extend(common_closure_load_instrs(LlvmOperand::Local(LlvmLocal(
            "closure_raw".into(),
        ))));
    let mut instrs = emit_closure_param_unpack(&mut state, arity, 0);
    let mut args = Vec::with_capacity(arity);
    for index in 0..arity {
        args.push((
            LlvmType::i64(),
            LlvmOperand::Local(LlvmLocal(format!("param.{index}"))),
        ));
    }
    instrs.push(LlvmInstr::Call {
        dst: Some(LlvmLocal("result".into())),
        tail: false,
        call_conv: Some(CallConv::Fastcc),
        ret_ty: LlvmType::i64(),
        callee: LlvmOperand::Global(target.clone()),
        args,
        attrs: vec![],
    });
    state.blocks[0].instrs.extend(instrs);
    state.blocks[0].terminator = Some(LlvmTerminator::Ret {
        ty: LlvmType::i64(),
        value: LlvmOperand::Local(LlvmLocal("result".into())),
    });
    state.finish().expect("top-level wrapper should be valid")
}

pub(super) fn emit_closure_param_unpack(
    state: &mut FunctionState<'_>,
    arity: usize,
    capture_count: usize,
) -> Vec<LlvmInstr> {
    let mut instrs = Vec::new();
    let payload = LlvmOperand::Local(LlvmLocal("payload".into()));
    for index in 0..arity {
        let applied_gep = LlvmLocal(format!("param.src.applied.{index}"));
        let applied_load = LlvmLocal(format!("param.applied.{index}"));
        let applied_idx = capture_count as i32 + index as i32;
        instrs.push(LlvmInstr::GetElementPtr {
            dst: applied_gep.clone(),
            inbounds: true,
            element_ty: LlvmType::i64(),
            base: payload.clone(),
            indices: vec![(LlvmType::i32(), const_i32_operand(applied_idx))],
        });
        instrs.push(LlvmInstr::Load {
            dst: applied_load.clone(),
            ty: LlvmType::i64(),
            ptr: LlvmOperand::Local(applied_gep),
            align: Some(8),
        });
        let new_arg_idx = LlvmLocal(format!("param.new.idx.{index}"));
        instrs.push(LlvmInstr::Binary {
            dst: new_arg_idx.clone(),
            op: LlvmValueKind::Sub,
            ty: LlvmType::i32(),
            lhs: const_i32_operand(index as i32),
            rhs: LlvmOperand::Local(LlvmLocal("applied_count".into())),
        });
        let new_gep = LlvmLocal(format!("param.src.new.{index}"));
        instrs.push(LlvmInstr::GetElementPtr {
            dst: new_gep.clone(),
            inbounds: true,
            element_ty: LlvmType::i64(),
            base: LlvmOperand::Local(LlvmLocal("args".into())),
            indices: vec![(LlvmType::i32(), LlvmOperand::Local(new_arg_idx))],
        });
        let new_load = LlvmLocal(format!("param.new.{index}"));
        instrs.push(LlvmInstr::Load {
            dst: new_load.clone(),
            ty: LlvmType::i64(),
            ptr: LlvmOperand::Local(new_gep),
            align: Some(8),
        });
        let cond = LlvmLocal(format!("param.is_applied.{index}"));
        instrs.push(LlvmInstr::Icmp {
            dst: cond.clone(),
            op: crate::core_to_llvm::LlvmCmpOp::Slt,
            ty: LlvmType::i32(),
            lhs: const_i32_operand(index as i32),
            rhs: LlvmOperand::Local(LlvmLocal("applied_count".into())),
        });
        instrs.push(LlvmInstr::Select {
            dst: LlvmLocal(format!("param.{index}")),
            cond_ty: LlvmType::i1(),
            cond: LlvmOperand::Local(cond),
            value_ty: LlvmType::i64(),
            then_value: LlvmOperand::Local(applied_load),
            else_value: LlvmOperand::Local(new_load),
        });
    }
    let _ = state;
    instrs
}

/// State for self-tail-call optimization.  When present, the function body
/// is lowered inside a loop block; tail self-calls store updated argument
/// values into the parameter alloca slots and branch back to `loop_header`.
pub(super) struct TcoLoopState {
    /// Label of the loop header block (body starts here).
    pub loop_header: LabelId,
    /// Alloca slots for each parameter, in order.  Tail self-calls store new
    /// argument values into these slots before branching to `loop_header`.
    pub param_slots: Vec<LlvmLocal>,
}

pub(super) struct FunctionBlock {
    pub label: LabelId,
    pub instrs: Vec<LlvmInstr>,
    pub terminator: Option<LlvmTerminator>,
}

impl FunctionBlock {
    fn into_llvm(self) -> Result<LlvmBlock, CoreToLlvmError> {
        Ok(LlvmBlock {
            label: self.label,
            instrs: self.instrs,
            term: self.terminator.ok_or_else(|| CoreToLlvmError::Malformed {
                message: "LLVM block finished without terminator".into(),
            })?,
        })
    }
}

pub(super) struct FunctionState<'a> {
    pub symbol: GlobalId,
    pub interner: Option<&'a Interner>,
    #[allow(dead_code)]
    pub top_level_symbols: HashMap<CoreBinderId, GlobalId>,
    pub param_bindings: Vec<(CoreBinder, LlvmLocal)>,
    pub llvm_params: Vec<LlvmLocal>,
    pub llvm_param_types: Vec<LlvmType>,
    pub ret_ty: LlvmType,
    pub call_conv: CallConv,
    pub blocks: Vec<FunctionBlock>,
    pub current_block: usize,
    pub entry_allocas: Vec<LlvmInstr>,
    pub next_tmp: u32,
    pub next_slot: u32,
    pub next_block_id: u32,
    pub local_slots: HashMap<CoreBinderId, LlvmLocal>,
    pub binder_names: HashMap<CoreBinderId, Identifier>,
    /// TCO loop state — present when the function is self-recursive.
    pub tco_loop: Option<TcoLoopState>,
}

impl<'a> FunctionState<'a> {
    pub fn new_top_level(
        symbol: GlobalId,
        params: &[CoreBinder],
        top_level_symbols: HashMap<CoreBinderId, GlobalId>,
        interner: Option<&'a Interner>,
    ) -> Self {
        let param_bindings = params
            .iter()
            .enumerate()
            .map(|(idx, binder)| {
                (
                    CoreBinder::new(binder.id, binder.name),
                    LlvmLocal(format!("arg{idx}")),
                )
            })
            .collect::<Vec<_>>();
        let llvm_params = param_bindings
            .iter()
            .map(|(_, local)| local.clone())
            .collect::<Vec<_>>();
        let llvm_param_types = llvm_params.iter().map(|_| LlvmType::i64()).collect();
        Self::base(
            symbol,
            top_level_symbols,
            interner,
            param_bindings,
            llvm_params,
            llvm_param_types,
            LlvmType::i64(),
            CallConv::Fastcc,
        )
    }

    pub fn new_closure_entry(
        symbol: GlobalId,
        top_level_symbols: HashMap<CoreBinderId, GlobalId>,
        interner: Option<&'a Interner>,
    ) -> Self {
        Self::base(
            symbol,
            top_level_symbols,
            interner,
            Vec::new(),
            vec![
                LlvmLocal("closure_raw".into()),
                LlvmLocal("args".into()),
                LlvmLocal("nargs".into()),
            ],
            vec![LlvmType::i64(), LlvmType::ptr(), LlvmType::i32()],
            LlvmType::i64(),
            CallConv::Fastcc,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn base(
        symbol: GlobalId,
        top_level_symbols: HashMap<CoreBinderId, GlobalId>,
        interner: Option<&'a Interner>,
        param_bindings: Vec<(CoreBinder, LlvmLocal)>,
        llvm_params: Vec<LlvmLocal>,
        llvm_param_types: Vec<LlvmType>,
        ret_ty: LlvmType,
        call_conv: CallConv,
    ) -> Self {
        let binder_names = param_bindings
            .iter()
            .map(|(binder, _)| (binder.id, binder.name))
            .collect();
        Self {
            symbol,
            interner,
            top_level_symbols,
            param_bindings,
            llvm_params,
            llvm_param_types,
            ret_ty,
            call_conv,
            blocks: vec![FunctionBlock {
                label: LabelId("entry".into()),
                instrs: Vec::new(),
                terminator: None,
            }],
            current_block: 0,
            entry_allocas: Vec::new(),
            next_tmp: 0,
            next_slot: 0,
            next_block_id: 0,
            local_slots: HashMap::new(),
            binder_names,
            tco_loop: None,
        }
    }

    pub fn temp_local(&mut self, prefix: &str) -> LlvmLocal {
        let id = self.next_tmp;
        self.next_tmp += 1;
        LlvmLocal(format!("{prefix}.{id}"))
    }

    pub fn new_slot(&mut self) -> LlvmLocal {
        let id = self.next_slot;
        self.next_slot += 1;
        LlvmLocal(format!("slot.{id}"))
    }

    pub fn new_block_label(&mut self, prefix: &str) -> LabelId {
        let id = self.next_block_id;
        self.next_block_id += 1;
        LabelId(format!("{prefix}.{id}"))
    }

    pub fn emit(&mut self, instr: LlvmInstr) {
        self.blocks[self.current_block].instrs.push(instr);
    }

    pub fn emit_entry_alloca(&mut self, instr: LlvmInstr) {
        self.entry_allocas.push(instr);
    }

    pub fn set_terminator(&mut self, term: LlvmTerminator) {
        self.blocks[self.current_block].terminator = Some(term);
    }

    pub fn current_block_label(&self) -> LabelId {
        self.blocks[self.current_block].label.clone()
    }

    pub fn current_block_open(&self) -> bool {
        self.blocks[self.current_block].terminator.is_none()
    }

    pub fn push_block(&mut self, label: LabelId) -> usize {
        self.blocks.push(FunctionBlock {
            label,
            instrs: Vec::new(),
            terminator: None,
        });
        self.blocks.len() - 1
    }

    pub fn switch_to_block(&mut self, idx: usize) {
        self.current_block = idx;
    }

    pub fn bind_local(&mut self, binder: CoreBinder, slot: LlvmLocal) {
        self.local_slots.insert(binder.id, slot);
        self.binder_names.insert(binder.id, binder.name);
    }

    pub fn finish(mut self) -> Result<LlvmFunction, CoreToLlvmError> {
        self.blocks[0].instrs.splice(0..0, self.entry_allocas);
        let blocks = self
            .blocks
            .into_iter()
            .map(FunctionBlock::into_llvm)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(LlvmFunction {
            linkage: Linkage::Internal,
            name: self.symbol,
            sig: LlvmFunctionSig {
                ret: self.ret_ty,
                params: self.llvm_param_types,
                varargs: false,
                call_conv: self.call_conv,
            },
            params: self.llvm_params,
            attrs: vec![],
            blocks,
        })
    }
}

pub(super) fn const_i32_operand(value: i32) -> LlvmOperand {
    LlvmOperand::Const(crate::core_to_llvm::LlvmConst::Int {
        bits: 32,
        value: value.into(),
    })
}

pub(super) fn closure_entry_function(
    symbol: GlobalId,
    top_level_symbols: HashMap<CoreBinderId, GlobalId>,
    interner: Option<&Interner>,
) -> FunctionState<'_> {
    FunctionState::new_closure_entry(symbol, top_level_symbols, interner)
}
