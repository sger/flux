//! Core IR → LIR lowering (Proposal 0132 Phases 2–3).
//!
//! Translates the functional Core IR into the flat, NaN-box-aware LIR CFG.
//! - Phase 2: literals, variables, let/letrec bindings, primop calls, top-level functions.
//! - Phase 3: pattern matching (Case), ADT/cons/tuple construction (Con), tuple field access.

use std::collections::HashMap;

use std::collections::HashSet;

use crate::core::{
    CoreAlt, CoreBinderId, CoreDef, CoreExpr, CoreHandler, CoreLit, CorePat, CorePrimOp,
    CoreProgram, CoreTag, CoreTopLevelItem,
};
use crate::lir::*;
use crate::syntax::Identifier;
use crate::syntax::interner::Interner;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedNativeSymbol {
    pub symbol: String,
    pub arity: usize,
}

// ── Object layout constants (match runtime/c/flux_rt.h) ──────────────────────

/// ADT header: {i32 ctor_tag, i32 field_count}, then i64 fields[].
const ADT_HEADER_SIZE: i32 = 8;
/// Tuple header: {i32 obj_tag, i32 arity}, then i64 fields[].
/// Used by the LLVM emitter for direct memory access; the bytecode emitter
/// uses TupleGet (→ OpTupleIndex) instead.
#[allow(dead_code)]
const TUPLE_PAYLOAD_OFFSET: i32 = 8;

/// Constructor tag IDs (must match core_to_llvm/codegen/adt.rs and runtime).
const SOME_TAG_ID: i64 = 1;
const LEFT_TAG_ID: i64 = 2;
const RIGHT_TAG_ID: i64 = 3;
const CONS_TAG_ID: i64 = 4;
const FIRST_USER_TAG_ID: i64 = 5;

/// RC runtime object type tags (match runtime/c/rc.c).
/// Used by the reuse path (Alloc instructions) and future LLVM emitter.
#[allow(dead_code)]
const OBJ_TAG_ADT: u8 = 3;
#[allow(dead_code)]
const OBJ_TAG_TUPLE: u8 = 4;
#[allow(dead_code)]
const OBJ_TAG_CLOSURE: u8 = 5;

// ── Library function → PrimOp resolution ────────────────────────────────────
//
// Maps unbound function names (from Flow.* prelude modules) to CorePrimOp
// variants that have C runtime implementations.  This allows the LIR to emit
// direct PrimCall instructions instead of GetGlobal + closure Call, which is
// essential for native compilation where the VM globals table is unavailable.

fn resolve_library_primop(name: &str, arity: usize) -> Option<CorePrimOp> {
    // Strip module prefix (e.g. "Flow.List.first" → "first")
    let short = name.rsplit('.').next().unwrap_or(name);
    match (short, arity) {
        ("sort", 1) => Some(CorePrimOp::Sort),
        ("sort_by", 2) => Some(CorePrimOp::SortBy),
        _ => None,
    }
}

// ── Qualified name resolution ────────────────────────────────────────────────

/// Walk the `CoreTopLevelItem` tree to build a module-qualified name for each
/// function.  E.g. `Module("Flow") → Module("List") → Function("sort")`
/// produces `"Flow_List_sort"`.
///
/// Returns a mapping from `Identifier` (bare function name) to a list of
/// qualified names.  Since multiple modules may export the same bare name
/// (e.g. Flow.List.sort and Flow.Array.sort), we return Vec to handle duplicates.
fn collect_module_paths(
    item: &CoreTopLevelItem,
    prefix: &[String],
    out: &mut Vec<(
        crate::syntax::Identifier,
        String,
        crate::diagnostics::position::Span,
    )>,
    interner: Option<&Interner>,
) {
    match item {
        CoreTopLevelItem::Function { name, span, .. } => {
            let func_name = interner
                .map(|i| i.resolve(*name).to_string())
                .unwrap_or_else(|| format!("sym_{}", name.as_u32()));
            let qualified = if prefix.is_empty() {
                func_name
            } else {
                let mut parts = prefix.to_vec();
                parts.push(func_name);
                parts.join("_")
            };
            // Sanitize for LLVM symbol names: replace '.' with '_'
            let sanitized = qualified.replace('.', "_");
            out.push((*name, sanitized, *span));
        }
        CoreTopLevelItem::Module { name, body, .. } => {
            let mod_name = interner
                .map(|i| i.resolve(*name).to_string())
                .unwrap_or_else(|| format!("mod_{}", name.as_u32()));
            // Sanitize module name for LLVM symbols
            let mod_name = mod_name.replace('.', "_");
            let mut new_prefix = prefix.to_vec();
            new_prefix.push(mod_name);
            for child in body {
                collect_module_paths(child, &new_prefix, out, interner);
            }
        }
        _ => {} // Import, Data, EffectDecl — skip
    }
}

/// Build a map from `CoreBinderId` → module-qualified name by cross-referencing
/// the module tree (`top_level_items`) with the flat def list (`defs`).
///
/// For each module function, finds the first unclaimed `CoreDef` with a matching
/// bare name and assigns it the qualified name.
fn build_qualified_names(
    program: &CoreProgram,
    interner: Option<&Interner>,
) -> HashMap<CoreBinderId, String> {
    // Step 1: Collect (bare_name, qualified_name, span) triples from the module tree.
    let mut name_qualified_pairs: Vec<(
        crate::syntax::Identifier,
        String,
        crate::diagnostics::position::Span,
    )> = Vec::new();
    for item in &program.top_level_items {
        collect_module_paths(item, &[], &mut name_qualified_pairs, interner);
    }

    // Step 2: Match CoreDef entries to qualified names.
    // Prefer exact (name, span) matches so duplicate module members like
    // Flow.Array.sort and Flow.List.sort cannot be crossed by encounter order.
    let mut result = HashMap::new();
    let mut claimed: HashSet<CoreBinderId> = HashSet::new();

    for (bare_name, qualified_name, span) in &name_qualified_pairs {
        for def in &program.defs {
            if def.name == *bare_name && def.span == *span && !claimed.contains(&def.binder.id) {
                result.insert(def.binder.id, qualified_name.clone());
                claimed.insert(def.binder.id);
                break;
            }
        }
    }

    // Fallback for any top-level functions that didn't get an exact span match.
    for (bare_name, qualified_name, _span) in &name_qualified_pairs {
        for def in &program.defs {
            if def.name == *bare_name && !claimed.contains(&def.binder.id) {
                result.insert(def.binder.id, qualified_name.clone());
                claimed.insert(def.binder.id);
                break;
            }
        }
    }

    // Step 3: Assign fallback names for defs not found in any module
    // (anonymous lambdas, letrec bindings, etc.)
    for def in &program.defs {
        result.entry(def.binder.id).or_insert_with(|| {
            interner
                .map(|i| i.resolve(def.name).to_string())
                .unwrap_or_else(|| format!("def_{}", def.binder.id.0))
        });
    }

    result
}

/// Collect user-defined ADT constructor tags from `CoreTopLevelItem::Data`
/// declarations.  Tags are assigned sequentially starting at `FIRST_USER_TAG_ID`.
fn collect_constructor_tags(
    items: &[CoreTopLevelItem],
    tags: &mut HashMap<String, i32>,
    interner: Option<&Interner>,
) {
    let mut next_tag = FIRST_USER_TAG_ID as i32;
    collect_ctor_tags_inner(items, tags, &mut next_tag, interner);
}

fn collect_ctor_tags_inner(
    items: &[CoreTopLevelItem],
    tags: &mut HashMap<String, i32>,
    next_tag: &mut i32,
    interner: Option<&Interner>,
) {
    for item in items {
        match item {
            CoreTopLevelItem::Data { variants, .. } => {
                for variant in variants {
                    let name = interner
                        .map(|i| i.resolve(variant.name).to_string())
                        .unwrap_or_else(|| format!("ctor_{}", variant.name.as_u32()));
                    if let std::collections::hash_map::Entry::Vacant(entry) = tags.entry(name) {
                        entry.insert(*next_tag);
                        *next_tag += 1;
                    }
                }
            }
            CoreTopLevelItem::Module { body, .. } => {
                collect_ctor_tags_inner(body, tags, next_tag, interner);
            }
            _ => {}
        }
    }
}

// ── Public entry point ───────────────────────────────────────────────────────

/// Lower a complete `CoreProgram` to `LirProgram`.
pub fn lower_program(program: &CoreProgram) -> LirProgram {
    lower_program_with_interner(program, None, None)
}

/// Lower a `CoreProgram` to `LirProgram` with symbol resolution support.
///
/// `globals_map` maps `Symbol` → VM global index for imported/prelude functions.
/// When a `CoreExpr::Var` has no binder (external reference), the lowerer checks
/// this map and emits `LirInstr::GetGlobal` instead of a `None` placeholder.
pub fn lower_program_with_interner(
    program: &CoreProgram,
    interner: Option<&Interner>,
    globals_map: Option<&HashMap<String, usize>>,
) -> LirProgram {
    lower_program_with_interner_and_externs(program, interner, globals_map, None, true)
}

/// Lower a `CoreProgram` to `LirProgram` with optional native external symbol
/// resolution for imported public functions.
pub fn lower_program_with_interner_and_externs(
    program: &CoreProgram,
    interner: Option<&Interner>,
    globals_map: Option<&HashMap<String, usize>>,
    extern_symbols: Option<&HashMap<String, ImportedNativeSymbol>>,
    emit_main: bool,
) -> LirProgram {
    let mut lir = LirProgram::new();

    // Build module-qualified names from the CoreTopLevelItem tree.
    let qualified_names = build_qualified_names(program, interner);

    // Collect user-defined ADT constructor tags from Data declarations.
    collect_constructor_tags(
        &program.top_level_items,
        &mut lir.constructor_tags,
        interner,
    );

    // Find the main def — it could be at any position in defs[].
    let main_idx = if emit_main {
        if let Some(interner) = interner {
            program
                .defs
                .iter()
                .position(|d| interner.resolve(d.name) == "main")
                .or(Some(0))
        } else {
            Some(0)
        }
    } else if let Some(interner) = interner {
        program
            .defs
            .iter()
            .position(|d| interner.resolve(d.name) == "main")
    } else {
        None
    };

    // Pre-assign LirFuncIds for top-level functions and separately track
    // top-level pure values for native-mode value resolution.
    let mut binder_func_map: HashMap<CoreBinderId, LirFuncId> = HashMap::new();
    let mut top_level_value_map: HashMap<CoreBinderId, &CoreExpr> = HashMap::new();
    for (i, def) in program.defs.iter().enumerate() {
        if main_idx.is_some_and(|idx| i == idx) {
            continue;
        }
        if matches!(def.expr, CoreExpr::Lam { .. }) {
            binder_func_map.insert(def.binder.id, LirFuncId(def.binder.id.0));
        } else {
            top_level_value_map.insert(def.binder.id, &def.expr);
        }
    }

    // Build name → binder map for MemberAccess resolution in the LLVM path
    // (where globals_map is None and we need to resolve qualified member
    // access by looking up the member's binder in the env).
    let mut name_binder_map: HashMap<crate::syntax::Identifier, Vec<CoreBinderId>> = HashMap::new();
    for def in &program.defs {
        name_binder_map
            .entry(def.name)
            .or_default()
            .push(def.binder.id);
    }

    // Build set of function binders known to return Int for local type propagation.
    let int_return_binders: HashSet<CoreBinderId> = program
        .defs
        .iter()
        .filter(|def| matches!(def.result_ty, Some(crate::core::CoreType::Int)))
        .map(|def| def.binder.id)
        .collect();

    // Phase 1: Lower all non-main defs.
    for (i, def) in program.defs.iter().enumerate() {
        if main_idx.is_some_and(|idx| i == idx) {
            continue;
        }
        let ctx = LowerDefCtx {
            program: &mut lir,
            binder_func_map: &binder_func_map,
            qualified_names: &qualified_names,
            name_binder_map: &name_binder_map,
            interner,
            globals_map,
            extern_symbols,
            top_level_value_map: &top_level_value_map,
            int_return_binders: &int_return_binders,
        };
        let func = lower_def(def, ctx);
        lir.push_function(func);
    }

    // Phase 2: Lower main with knowledge of all sibling functions.
    // Main is always last in LIR (emit_program expects this).
    if let Some(main_idx) = main_idx {
        let main_def = &program.defs[main_idx];
        let ctx = LowerDefCtx {
            program: &mut lir,
            binder_func_map: &binder_func_map,
            qualified_names: &qualified_names,
            name_binder_map: &name_binder_map,
            interner,
            globals_map,
            extern_symbols,
            top_level_value_map: &top_level_value_map,
            int_return_binders: &int_return_binders,
        };
        let func = lower_def(main_def, ctx);
        lir.push_function(func);
    }

    lir
}

// ── Per-function lowering context ────────────────────────────────────────────

/// Tracks state while lowering a single function body to LIR.
struct FnLower<'a> {
    /// Mapping from Core binder IDs to LIR variables.
    env: HashMap<CoreBinderId, LirVar>,
    /// The function being built.
    func: LirFunction,
    /// Index of the currently active block.
    current_block: usize,
    /// Reference to the program-level string pool.
    program: &'a mut LirProgram,
    /// Optional interner for resolving Symbol → string names.
    interner: Option<&'a Interner>,
    /// Optional mapping from function name → VM global index for imported functions.
    globals_map: Option<&'a HashMap<String, usize>>,
    /// Optional mapping from imported source-level name to the linked native
    /// symbol to use for cross-module calls/closures.
    extern_symbols: Option<&'a HashMap<String, ImportedNativeSymbol>>,
    /// Tracks LIR variables that were produced by GetGlobal, mapping to their
    /// function name.  Used by the App handler to intercept closure calls to
    /// known library functions and emit PrimCall instead.
    global_var_names: HashMap<LirVar, String>,
    /// Maps bare function Identifier → list of CoreBinderIds.
    /// Used for MemberAccess resolution when globals_map is None (LLVM path).
    name_binder_map: &'a HashMap<crate::syntax::Identifier, Vec<CoreBinderId>>,
    /// Maps LirVar → LirFuncId for variables produced by MakeClosure with
    /// no captures (top-level function references).  Used to detect known
    /// calls and emit CallKind::Direct.
    direct_func_vars: HashMap<LirVar, LirFuncId>,
    /// Maps CoreBinderId → LirFuncId for top-level functions.
    binder_func_id_map: &'a HashMap<CoreBinderId, LirFuncId>,
    /// Maps CoreBinderId → module-qualified name.
    qualified_names: &'a HashMap<CoreBinderId, String>,
    /// Maps CoreBinderId → top-level non-function rhs so native lowering can
    /// use the value directly instead of fabricating a closure placeholder.
    top_level_value_map: &'a HashMap<CoreBinderId, &'a CoreExpr>,
    /// Variables known to hold NaN-boxed integers at this point in lowering.
    /// Used for local type propagation: when a generic `Add` sees both
    /// operands in this set, it emits `IAdd` instead of a C runtime call.
    int_vars: HashSet<LirVar>,
    /// Function binders whose return type is known to be Int.
    int_return_binders: &'a HashSet<CoreBinderId>,
}

struct FnLowerCtx<'a> {
    program: &'a mut LirProgram,
    interner: Option<&'a Interner>,
    globals_map: Option<&'a HashMap<String, usize>>,
    extern_symbols: Option<&'a HashMap<String, ImportedNativeSymbol>>,
    name_binder_map: &'a HashMap<crate::syntax::Identifier, Vec<CoreBinderId>>,
    binder_func_id_map: &'a HashMap<CoreBinderId, LirFuncId>,
    qualified_names: &'a HashMap<CoreBinderId, String>,
    top_level_value_map: &'a HashMap<CoreBinderId, &'a CoreExpr>,
    /// Function binders known to return Int (from `CoreDef::result_ty`).
    int_return_binders: &'a HashSet<CoreBinderId>,
}

impl<'a> FnLower<'a> {
    fn new(name: String, id: LirFuncId, qualified_name: String, ctx: FnLowerCtx<'a>) -> Self {
        let entry_block = LirBlock {
            id: BlockId(0),
            params: Vec::new(),
            instrs: Vec::new(),
            terminator: LirTerminator::Unreachable, // placeholder
        };
        Self {
            env: HashMap::new(),
            func: LirFunction {
                name,
                id,
                qualified_name,
                params: Vec::new(),
                blocks: vec![entry_block],
                next_var: 0,
                capture_vars: Vec::new(),
            },
            current_block: 0,
            program: ctx.program,
            interner: ctx.interner,
            global_var_names: HashMap::new(),
            globals_map: ctx.globals_map,
            extern_symbols: ctx.extern_symbols,
            name_binder_map: ctx.name_binder_map,
            direct_func_vars: HashMap::new(),
            binder_func_id_map: ctx.binder_func_id_map,
            qualified_names: ctx.qualified_names,
            top_level_value_map: ctx.top_level_value_map,
            int_vars: HashSet::new(),
            int_return_binders: ctx.int_return_binders,
        }
    }

    /// Resolve an Identifier (Symbol) to a string name.
    /// Falls back to the numeric symbol ID if no interner is available.
    fn resolve_name(&self, name: crate::syntax::Identifier) -> String {
        if let Some(interner) = self.interner {
            interner.resolve(name).to_string()
        } else {
            format!("ctor_{}", name)
        }
    }

    fn resolve_external_symbol(&self, source_name: &str) -> Option<ImportedNativeSymbol> {
        self.extern_symbols
            .and_then(|symbols| symbols.get(source_name).cloned())
    }

    /// In merged native mode, duplicate bare names from different modules can
    /// carry the wrong binder. Prefer a sibling function from the current
    /// module when the qualified name matches exactly.
    fn prefer_same_module_binder(
        &self,
        binder: CoreBinderId,
        name: crate::syntax::Identifier,
    ) -> CoreBinderId {
        if self.globals_map.is_some() {
            return binder;
        }
        let Some(candidates) = self.name_binder_map.get(&name) else {
            return binder;
        };
        if candidates.len() <= 1 {
            return binder;
        }
        let Some((module_prefix, _)) = self.func.qualified_name.rsplit_once('_') else {
            return binder;
        };
        let target = format!("{}_{}", module_prefix, self.resolve_name(name));
        candidates
            .iter()
            .find(|bid| self.qualified_names.get(bid).is_some_and(|q| q == &target))
            .copied()
            .unwrap_or(binder)
    }

    /// Allocate a fresh LIR variable.
    fn fresh_var(&mut self) -> LirVar {
        self.func.fresh_var()
    }

    /// Emit an instruction into the current block.
    fn emit(&mut self, instr: LirInstr) {
        self.func.blocks[self.current_block].instrs.push(instr);
    }

    /// Set the terminator of the current block.
    fn set_terminator(&mut self, term: LirTerminator) {
        self.func.blocks[self.current_block].terminator = term;
    }

    /// Copy `val` into the first parameter of `target_block` (SSA phi-node bridging).
    fn emit_copy_to_join_param(&mut self, val: LirVar, target_block: BlockId) {
        let target_idx = target_block.0 as usize;
        if let Some(&param) = self.func.blocks[target_idx].params.first() {
            self.emit(LirInstr::Copy {
                dst: param,
                src: val,
            });
        }
    }

    /// Create a new block and return its index.
    fn new_block(&mut self) -> usize {
        let id = BlockId(self.func.blocks.len() as u32);
        self.func.blocks.push(LirBlock {
            id,
            params: Vec::new(),
            instrs: Vec::new(),
            terminator: LirTerminator::Unreachable,
        });
        self.func.blocks.len() - 1
    }

    /// Switch to emitting into a different block.
    fn switch_to_block(&mut self, block_idx: usize) {
        self.current_block = block_idx;
    }

    /// Bind a Core binder to a LIR variable.
    fn bind(&mut self, binder: CoreBinderId, var: LirVar) {
        self.env.insert(binder, var);
    }

    /// Look up a Core binder, returning its LIR variable.
    fn lookup(&self, binder: CoreBinderId) -> LirVar {
        *self
            .env
            .get(&binder)
            .unwrap_or_else(|| panic!("LIR lower: unbound CoreBinderId({})", binder.0))
    }

    // ── Expression lowering ──────────────────────────────────────────

    /// Lower a `CoreExpr` and return the `LirVar` holding the result.
    /// The result is always a NaN-boxed i64 value.
    fn lower_expr(&mut self, expr: &CoreExpr) -> LirVar {
        match expr {
            CoreExpr::Lit(lit, _span) => self.lower_lit(lit),

            CoreExpr::Var { var, .. } => {
                if let Some(raw_binder) = var.binder {
                    let binder = self.prefer_same_module_binder(raw_binder, var.name);
                    // Check env first (locals, parameters, letrec bindings).
                    if let Some(&v) = self.env.get(&binder) {
                        return v;
                    }
                    // Top-level pure values should lower as values on the
                    // native path, not as fabricated closure objects.
                    if let Some(value_expr) = self.top_level_value_map.get(&binder) {
                        let lowered = self.lower_expr(value_expr);
                        self.bind(binder, lowered);
                        return lowered;
                    }
                    // Not in env — check if it's a top-level function.
                    // Create a closure lazily (only when used as a value).
                    if let Some(&func_id) = self.binder_func_id_map.get(&binder) {
                        let var = self.fresh_var();
                        self.emit(LirInstr::MakeClosure {
                            dst: var,
                            func_id,
                            captures: Vec::new(),
                        });
                        self.bind(binder, var);
                        self.direct_func_vars.insert(var, func_id);
                        return var;
                    }
                    // Fallback: emit None for unknown binders.
                    let dst = self.fresh_var();
                    self.emit(LirInstr::Const {
                        dst,
                        value: LirConst::None,
                    });
                    dst
                } else if let Some(globals) = self.globals_map {
                    // External variable — resolve name and check the globals map.
                    let name = self.resolve_name(var.name);
                    if let Some(&global_idx) = globals.get(&name) {
                        let dst = self.fresh_var();
                        self.emit(LirInstr::GetGlobal { dst, global_idx });
                        self.global_var_names.insert(dst, name);
                        dst
                    } else {
                        // Unknown external — emit None placeholder.
                        let dst = self.fresh_var();
                        self.emit(LirInstr::Const {
                            dst,
                            value: LirConst::None,
                        });
                        dst
                    }
                } else {
                    // No globals map (LLVM native path).
                    // Check if this unbound name is a known ADT constructor
                    // (zero-field constructors like Dir.Up appear as unbound Vars).
                    let name = self.resolve_name(var.name);
                    if let Some(&ctor_tag) = self.program.constructor_tags.get(&name) {
                        let dst = self.fresh_var();
                        self.emit(LirInstr::MakeCtor {
                            dst,
                            ctor_tag,
                            ctor_name: Some(name),
                            fields: Vec::new(),
                        });
                        dst
                    } else if let Some(binders) = self.name_binder_map.get(&var.name) {
                        // Check if it's a top-level function reference.
                        for &bid in binders {
                            if let Some(&lir_var) = self.env.get(&bid) {
                                return lir_var;
                            }
                            if let Some(&func_id) = self.binder_func_id_map.get(&bid) {
                                let var = self.fresh_var();
                                self.emit(LirInstr::MakeClosure {
                                    dst: var,
                                    func_id,
                                    captures: Vec::new(),
                                });
                                self.bind(bid, var);
                                self.direct_func_vars.insert(var, func_id);
                                return var;
                            }
                        }
                        let dst = self.fresh_var();
                        self.emit(LirInstr::Const {
                            dst,
                            value: LirConst::None,
                        });
                        dst
                    } else if let Some(extern_fn) = self.resolve_external_symbol(&name) {
                        let dst = self.fresh_var();
                        self.emit(LirInstr::MakeExternClosure {
                            dst,
                            symbol: extern_fn.symbol,
                            arity: extern_fn.arity,
                        });
                        dst
                    } else {
                        let dst = self.fresh_var();
                        self.emit(LirInstr::Const {
                            dst,
                            value: LirConst::None,
                        });
                        dst
                    }
                }
            }

            CoreExpr::Let { var, rhs, body, .. } => {
                let rhs_var = self.lower_expr(rhs);
                self.bind(var.id, rhs_var);
                self.lower_expr(body)
            }

            CoreExpr::LetRec { var, rhs, body, .. } => {
                // For letrec where the RHS is a lambda (recursive function):
                // 1. Pre-assign a function index in the program
                // 2. Create a MakeClosure that the lambda body can reference
                // 3. The inner lambda sees itself via its own function index
                if let CoreExpr::Lam {
                    params,
                    body: lam_body,
                    ..
                } = rhs.as_ref()
                {
                    let free = collect_free_vars(rhs);
                    let outer_captures: Vec<(CoreBinderId, LirVar)> = free
                        .iter()
                        .filter(|id| **id != var.id) // exclude self-reference
                        .filter_map(|id| self.env.get(id).copied().map(|v| (*id, v)))
                        .collect();

                    let mut temp_program = std::mem::take(&mut *self.program);

                    // Assign a synthetic LirFuncId for this letrec function.
                    let synthetic_id = temp_program.alloc_synthetic_func_id();
                    let letrec_qname = format!("{}_letrec_{}", self.func.qualified_name, synthetic_id.0);

                    // Pre-assign function slot so self-recursion works.
                    let func_idx = temp_program.functions.len();
                    temp_program.functions.push(LirFunction {
                        name: format!("letrec_{}_placeholder", func_idx),
                        id: synthetic_id,
                        qualified_name: letrec_qname.clone(),
                        params: Vec::new(),
                        blocks: Vec::new(),
                        next_var: 0,
                        capture_vars: Vec::new(),
                    });
                    temp_program.func_index.insert(synthetic_id, func_idx);

                    let func_name = format!("letrec_{}", func_idx);
                    let mut inner = FnLower::new(
                        func_name,
                        synthetic_id,
                        letrec_qname,
                        FnLowerCtx {
                            program: &mut temp_program,
                            interner: self.interner,
                            globals_map: self.globals_map,
                            extern_symbols: self.extern_symbols,
                            name_binder_map: self.name_binder_map,
                            binder_func_id_map: self.binder_func_id_map,
                            qualified_names: self.qualified_names,
                            top_level_value_map: self.top_level_value_map,
                            int_return_binders: self.int_return_binders,
                        },
                    );

                    // Capture outer variables.
                    for &(binder_id, _outer_var) in &outer_captures {
                        let inner_var = inner.fresh_var();
                        inner.func.capture_vars.push(inner_var);
                        inner.bind(binder_id, inner_var);
                    }

                    // Self-reference: the letrec variable inside the lambda
                    // creates a MakeClosure to itself (same func_id).
                    let self_var = inner.fresh_var();
                    inner.emit(LirInstr::MakeClosure {
                        dst: self_var,
                        func_id: synthetic_id,
                        captures: (0..outer_captures.len())
                            .map(|i| inner.func.capture_vars[i])
                            .collect(),
                    });
                    inner.bind(var.id, self_var);

                    // Register parameters.
                    for param in params {
                        let pv = inner.fresh_var();
                        inner.bind(param.id, pv);
                        inner.func.params.push(pv);
                    }

                    // Lower the body.
                    let result = inner.lower_expr(lam_body);
                    inner.set_terminator(LirTerminator::Return(result));
                    let inner_func = inner.func;

                    *self.program = temp_program;
                    // Replace the placeholder with the actual function.
                    self.program.functions[func_idx] = inner_func;

                    // Emit MakeClosure in the outer context.
                    let outer_capture_vars: Vec<LirVar> =
                        outer_captures.iter().map(|&(_, v)| v).collect();
                    let dst = self.fresh_var();
                    self.emit(LirInstr::MakeClosure {
                        dst,
                        func_id: synthetic_id,
                        captures: outer_capture_vars,
                    });
                    self.bind(var.id, dst);
                    self.lower_expr(body)
                } else {
                    // Non-lambda letrec (rare): use placeholder approach.
                    let placeholder = self.fresh_var();
                    self.emit(LirInstr::Const {
                        dst: placeholder,
                        value: LirConst::None,
                    });
                    self.bind(var.id, placeholder);
                    let rhs_var = self.lower_expr(rhs);
                    self.bind(var.id, rhs_var);
                    self.lower_expr(body)
                }
            }

            CoreExpr::PrimOp { op, args, .. } => self.lower_primop(*op, args),

            CoreExpr::Lam { params, body, .. } => {
                // Always create a nested LirFunction for lambdas.
                // Even non-capturing lambdas need to be callable via OpCall.
                let free = collect_free_vars(expr);
                let outer_captures: Vec<(CoreBinderId, LirVar)> = free
                    .iter()
                    .filter_map(|id| self.env.get(id).copied().map(|v| (*id, v)))
                    .collect();

                {
                    // Temporarily take the program so the inner FnLower can use it
                    // (Rust can't have two &mut borrows of self.program).
                    let mut temp_program = std::mem::take(&mut *self.program);

                    let synthetic_id = temp_program.alloc_synthetic_func_id();
                    let lambda_qname = format!("{}_lambda_{}", self.func.qualified_name, synthetic_id.0);
                    let func_name = format!("closure_{}", temp_program.functions.len());
                    let mut inner = FnLower::new(
                        func_name,
                        synthetic_id,
                        lambda_qname,
                        FnLowerCtx {
                            program: &mut temp_program,
                            interner: self.interner,
                            globals_map: self.globals_map,
                            extern_symbols: self.extern_symbols,
                            name_binder_map: self.name_binder_map,
                            binder_func_id_map: self.binder_func_id_map,
                            qualified_names: self.qualified_names,
                            top_level_value_map: self.top_level_value_map,
                            int_return_binders: self.int_return_binders,
                        },
                    );

                    // Map captured variables: create fresh LirVars inside the inner
                    // function, mark them as capture_vars (→ OpGetFree in emitter).
                    for &(binder_id, _outer_var) in &outer_captures {
                        let inner_var = inner.fresh_var();
                        inner.func.capture_vars.push(inner_var);
                        inner.bind(binder_id, inner_var);
                    }

                    // Register parameters.
                    for param in params {
                        let pv = inner.fresh_var();
                        inner.bind(param.id, pv);
                        inner.func.params.push(pv);
                    }

                    // Lower the body in the inner context.
                    let result = inner.lower_expr(body);
                    inner.set_terminator(LirTerminator::Return(result));

                    let inner_func = inner.func;

                    // Restore the program and add the inner function.
                    *self.program = temp_program;
                    self.program.push_function(inner_func);

                    // Emit MakeClosure in the outer context.
                    let outer_capture_vars: Vec<LirVar> =
                        outer_captures.iter().map(|&(_, v)| v).collect();
                    let dst = self.fresh_var();
                    self.emit(LirInstr::MakeClosure {
                        dst,
                        func_id: synthetic_id,
                        captures: outer_capture_vars,
                    });
                    dst
                }
            }

            CoreExpr::App { func, args, .. } => self.lower_call_expr(func, args, None),
            CoreExpr::AetherCall {
                func,
                args,
                arg_modes,
                ..
            } => self.lower_call_expr(func, args, Some(arg_modes)),

            // ── Pattern matching (Phase 3) ────────────────────────────
            CoreExpr::Case {
                scrutinee, alts, ..
            } => self.lower_case(scrutinee, alts),

            // ── ADT / collection construction (Phase 3) ──────────────
            CoreExpr::Con { tag, fields, .. } => self.lower_con(tag, fields),

            CoreExpr::Return { value, .. } => {
                // Early return from function — emit Return terminator.
                let val = self.lower_expr(value);
                self.set_terminator(LirTerminator::Return(val));
                // Create a new unreachable block for any dead code after the return.
                let dead_idx = self.new_block();
                self.switch_to_block(dead_idx);
                val
            }

            CoreExpr::MemberAccess { object, member, .. } => {
                // Qualified member access (e.g. Array.sort, T.hello).
                //
                // Resolution strategy:
                // 1. If globals_map is available (bytecode VM path), look up
                //    the qualified/unqualified name in the globals table.
                // 2. If no globals_map (LLVM native path), check if the member
                //    has a binder registered in the env (from pre-registered
                //    top-level binders).  This handles cross-module references
                //    in the merged program.
                // 3. Fallback: lower the object expression.

                if let Some(globals) = self.globals_map {
                    let member_str = self.resolve_name(*member);
                    // Try resolving with the object's name as prefix.
                    let qualified = if let CoreExpr::Var { var, .. } = object.as_ref() {
                        let obj_name = self.resolve_name(var.name);
                        Some(format!("{obj_name}.{member_str}"))
                    } else {
                        None
                    };
                    if let Some(ref qname) = qualified
                        && let Some(&global_idx) = globals.get(qname.as_str())
                    {
                        let dst = self.fresh_var();
                        self.emit(LirInstr::GetGlobal { dst, global_idx });
                        self.global_var_names.insert(dst, member_str.clone());
                        return dst;
                    }
                    // Try just the member name (unqualified).
                    if let Some(&global_idx) = globals.get(&member_str) {
                        let dst = self.fresh_var();
                        self.emit(LirInstr::GetGlobal { dst, global_idx });
                        self.global_var_names.insert(dst, member_str.clone());
                        return dst;
                    }
                }

                // No globals_map (LLVM native path): resolve via binder env
                // or binder_func_id_map (lazy closure creation). If the object
                // is a module alias like `Array`, prefer a binder whose
                // qualified name ends with `Array_sort` over unrelated bare-name
                // collisions like `Flow_List_sort`.
                if let Some(binders) = self.name_binder_map.get(member) {
                    let member_str = self.resolve_name(*member);
                    let preferred_suffix = if let CoreExpr::Var { var, .. } = object.as_ref() {
                        let obj_name = self.resolve_name(var.name);
                        Some(format!("{obj_name}_{member_str}"))
                    } else {
                        None
                    };

                    let mut ordered_binders = binders.clone();
                    if let Some(ref suffix) = preferred_suffix
                        && let Some(pos) = ordered_binders.iter().position(|bid| {
                            self.qualified_names
                                .get(bid)
                                .is_some_and(|q| q.ends_with(suffix))
                        })
                    {
                        let preferred = ordered_binders.remove(pos);
                        ordered_binders.insert(0, preferred);
                    }

                    for bid in ordered_binders {
                        if let Some(&lir_var) = self.env.get(&bid) {
                            return lir_var;
                        }
                        // Not in env — create closure lazily for top-level function.
                        if let Some(&func_id) = self.binder_func_id_map.get(&bid) {
                            let var = self.fresh_var();
                            self.emit(LirInstr::MakeClosure {
                                dst: var,
                                func_id,
                                captures: Vec::new(),
                            });
                            self.bind(bid, var);
                            self.direct_func_vars.insert(var, func_id);
                            return var;
                        }
                    }
                }

                if let CoreExpr::Var { var, .. } = object.as_ref() {
                    let object_name = self.resolve_name(var.name);
                    let member_name = self.resolve_name(*member);
                    let qualified = format!("{object_name}.{member_name}");
                    if let Some(extern_fn) = self.resolve_external_symbol(&qualified) {
                        let dst = self.fresh_var();
                        self.emit(LirInstr::MakeExternClosure {
                            dst,
                            symbol: extern_fn.symbol,
                            arity: extern_fn.arity,
                        });
                        return dst;
                    }
                }
                let member_name = self.resolve_name(*member);
                if let Some(extern_fn) = self.resolve_external_symbol(&member_name) {
                    let dst = self.fresh_var();
                    self.emit(LirInstr::MakeExternClosure {
                        dst,
                        symbol: extern_fn.symbol,
                        arity: extern_fn.arity,
                    });
                    return dst;
                }

                // Fallback: lower the object and ignore the member.
                let obj = self.lower_expr(object);
                let dst = self.fresh_var();
                self.emit(LirInstr::Copy { dst, src: obj });
                dst
            }

            CoreExpr::TupleField { object, index, .. } => {
                let obj = self.lower_expr(object);
                let dst = self.fresh_var();
                self.emit(LirInstr::TupleGet {
                    dst,
                    tuple: obj,
                    index: *index,
                });
                dst
            }

            // ── Effect handlers (Phase 9 — Koka-style yield model) ────
            CoreExpr::Handle {
                body,
                effect,
                handlers,
                ..
            } => self.lower_handle(body, *effect, handlers),

            CoreExpr::Perform {
                effect,
                operation,
                args,
                ..
            } => self.lower_perform(*effect, *operation, args),

            // ── Aether RC nodes (Phase 5) ─────────────────────────────
            CoreExpr::Dup { var, body, .. } => {
                if let Some(binder) = var.binder {
                    let v = self.lookup(binder);
                    self.emit(LirInstr::Dup { val: v });
                }
                self.lower_expr(body)
            }

            CoreExpr::Drop { var, body, .. } => {
                if let Some(binder) = var.binder {
                    let v = self.lookup(binder);
                    self.emit(LirInstr::Drop { val: v });
                }
                self.lower_expr(body)
            }

            CoreExpr::Reuse {
                token,
                tag,
                fields,
                field_mask,
                ..
            } => self.lower_reuse(token, tag, fields, *field_mask),

            CoreExpr::DropSpecialized {
                scrutinee,
                unique_body,
                shared_body,
                ..
            } => self.lower_drop_specialized(scrutinee, unique_body, shared_body),
        }
    }

    // ── Literal lowering ─────────────────────────────────────────────

    fn lower_lit(&mut self, lit: &CoreLit) -> LirVar {
        let dst = self.fresh_var();
        let is_int = matches!(lit, CoreLit::Int(_));
        let value = match lit {
            CoreLit::Int(n) => {
                let n = *n;
                if n >= crate::runtime::nanbox::MIN_INLINE_INT
                    && n <= crate::runtime::nanbox::MAX_INLINE_INT
                {
                    LirConst::Tagged(crate::lir::nanbox_tag_int(n))
                } else {
                    LirConst::Int(n)
                }
            }
            CoreLit::Float(f) => LirConst::Float(*f),
            CoreLit::Bool(b) => LirConst::Bool(*b),
            CoreLit::String(s) => {
                self.program.intern_string(s.clone());
                LirConst::String(s.clone())
            }
            CoreLit::Unit => LirConst::None,
        };
        self.emit(LirInstr::Const { dst, value });
        if is_int {
            self.int_vars.insert(dst);
        }
        dst
    }

    // ── Effect handler lowering (Proposal 0134) ─────────────────────

    /// Lower `handle body with Effect { op(resume, params) -> handler_body }`.
    ///
    /// Emits:
    ///   1. Save current evidence vector
    ///   2. Build handler clause closure(s)
    ///   3. Fresh marker + insert evidence
    ///   4. Lower body
    ///   5. Prompt check (yield_prompt)
    fn lower_handle(
        &mut self,
        body: &CoreExpr,
        effect: Identifier,
        handlers: &[CoreHandler],
    ) -> LirVar {
        use crate::core::CorePrimOp;

        // 1. Save current evidence vector.
        let saved_evv = self.fresh_var();
        self.emit(LirInstr::PrimCall {
            dst: Some(saved_evv),
            op: CorePrimOp::EvvGet,
            args: vec![],
        });

        // 2. Build handler clause closure.
        //    For single-operation effects (the common case), the handler
        //    closure IS the clause: fn(resume, param0, ...) -> handler_body.
        //    For multi-operation effects, we'd need a dispatch closure, but
        //    for now we support single-operation effects.
        let handler_closure = if handlers.len() == 1 {
            self.lower_handler_clause(&handlers[0])
        } else {
            // Multi-operation: build the first handler for now.
            // TODO: dispatch closure for multi-op effects.
            self.lower_handler_clause(&handlers[0])
        };

        // 3. Create fresh marker + insert evidence.
        let marker = self.fresh_var();
        self.emit(LirInstr::PrimCall {
            dst: Some(marker),
            op: CorePrimOp::FreshMarker,
            args: vec![],
        });

        // Effect tag: Symbol(u32) → NaN-boxed integer.
        let htag = self.fresh_var();
        self.emit(LirInstr::Const {
            dst: htag,
            value: LirConst::Tagged(crate::lir::nanbox_tag_int(effect.as_u32() as i64)),
        });

        let new_evv = self.fresh_var();
        self.emit(LirInstr::PrimCall {
            dst: Some(new_evv),
            op: CorePrimOp::EvvInsert,
            args: vec![saved_evv, htag, marker, handler_closure],
        });

        self.emit(LirInstr::PrimCall {
            dst: None,
            op: CorePrimOp::EvvSet,
            args: vec![new_evv],
        });

        // 4. Lower the body expression.
        let body_result = self.lower_expr(body);

        // 5. Prompt check: restore evv and check for yields.
        let result = self.fresh_var();
        self.emit(LirInstr::PrimCall {
            dst: Some(result),
            op: CorePrimOp::YieldPrompt,
            args: vec![marker, saved_evv, body_result],
        });

        result
    }

    fn lower_call_expr(
        &mut self,
        func: &CoreExpr,
        args: &[CoreExpr],
        arg_modes: Option<&[crate::aether::borrow_infer::BorrowMode]>,
    ) -> LirVar {
        let resolved_name = match func {
            CoreExpr::Var { var, .. } if var.binder.is_none() => Some(self.resolve_name(var.name)),
            CoreExpr::MemberAccess { member, .. } => Some(self.resolve_name(*member)),
            _ => None,
        };
        if let Some(ref name) = resolved_name
            && let Some(op) = resolve_library_primop(name, args.len())
        {
            let arg_vars: Vec<LirVar> = args.iter().map(|a| self.lower_expr(a)).collect();
            self.dup_owned_call_args(&arg_vars, arg_modes);
            let dst = self.fresh_var();
            self.emit(LirInstr::PrimCall {
                dst: Some(dst),
                op,
                args: arg_vars,
            });
            return dst;
        }

        if let Some(ref name) = resolved_name
            && let Some(&ctor_tag) = self.program.constructor_tags.get(name.as_str())
        {
            let arg_vars: Vec<LirVar> = args.iter().map(|a| self.lower_expr(a)).collect();
            self.dup_owned_call_args(&arg_vars, arg_modes);
            let dst = self.fresh_var();
            self.emit(LirInstr::MakeCtor {
                dst,
                ctor_tag,
                ctor_name: Some(name.clone()),
                fields: arg_vars,
            });
            return dst;
        }

        let (direct_func_id, callee_binder) = match func {
            CoreExpr::Var { var, .. } if var.binder.is_some() => {
                let bid = var.binder.unwrap();
                let preferred = self.prefer_same_module_binder(bid, var.name);
                (
                    self.binder_func_id_map.get(&preferred).copied(),
                    Some(preferred),
                )
            }
            _ => (None, None),
        };
        let direct_external_symbol = if direct_func_id.is_none() {
            match func {
                CoreExpr::Var { var, .. } if var.binder.is_none() => {
                    self.resolve_external_symbol(&self.resolve_name(var.name))
                }
                CoreExpr::MemberAccess { object, member, .. } => {
                    let member_name = self.resolve_name(*member);
                    let qualified = if let CoreExpr::Var { var, .. } = object.as_ref() {
                        Some(format!("{}.{}", self.resolve_name(var.name), member_name))
                    } else {
                        None
                    };
                    qualified
                        .as_ref()
                        .and_then(|name| self.resolve_external_symbol(name))
                        .or_else(|| self.resolve_external_symbol(&member_name))
                }
                _ => None,
            }
        } else {
            None
        };

        let arg_vars: Vec<LirVar> = args.iter().map(|a| self.lower_expr(a)).collect();
        self.dup_owned_call_args(&arg_vars, arg_modes);

        if let Some(func_id) = direct_func_id {
            let cont_idx = self.new_block();
            let cont_id = BlockId(cont_idx as u32);
            let result = self.fresh_var();
            self.func.blocks[cont_idx].params.push(result);

            let dummy = self.fresh_var();
            self.emit(LirInstr::Const {
                dst: dummy,
                value: LirConst::None,
            });

            self.set_terminator(LirTerminator::Call {
                dst: result,
                func: dummy,
                args: arg_vars,
                cont: cont_id,
                kind: CallKind::Direct { func_id },
                yield_cont: None,
            });
            self.switch_to_block(cont_idx);
            // Mark result as integer if the callee is known to return Int.
            if let Some(bid) = callee_binder {
                if self.int_return_binders.contains(&bid) {
                    self.int_vars.insert(result);
                }
            }
            return result;
        }

        if let Some(extern_fn) = direct_external_symbol {
            let cont_idx = self.new_block();
            let cont_id = BlockId(cont_idx as u32);
            let result = self.fresh_var();
            self.func.blocks[cont_idx].params.push(result);

            let dummy = self.fresh_var();
            self.emit(LirInstr::Const {
                dst: dummy,
                value: LirConst::None,
            });

            self.set_terminator(LirTerminator::Call {
                dst: result,
                func: dummy,
                args: arg_vars,
                cont: cont_id,
                kind: CallKind::DirectExtern {
                    symbol: extern_fn.symbol,
                },
                yield_cont: None,
            });
            self.switch_to_block(cont_idx);
            if let Some(bid) = callee_binder {
                if self.int_return_binders.contains(&bid) {
                    self.int_vars.insert(result);
                }
            }
            return result;
        }

        let func_var = self.lower_expr(func);
        if let Some(name) = self.global_var_names.get(&func_var).cloned()
            && let Some(op) = resolve_library_primop(&name, arg_vars.len())
        {
            let dst = self.fresh_var();
            self.emit(LirInstr::PrimCall {
                dst: Some(dst),
                op,
                args: arg_vars,
            });
            return dst;
        }

        let cont_idx = self.new_block();
        let cont_id = BlockId(cont_idx as u32);
        let result = self.fresh_var();
        self.func.blocks[cont_idx].params.push(result);

        self.set_terminator(LirTerminator::Call {
            dst: result,
            func: func_var,
            args: arg_vars,
            cont: cont_id,
            kind: CallKind::Indirect,
            yield_cont: None,
        });

        self.switch_to_block(cont_idx);
        result
    }

    fn dup_owned_call_args(
        &mut self,
        arg_vars: &[LirVar],
        arg_modes: Option<&[crate::aether::borrow_infer::BorrowMode]>,
    ) {
        use crate::aether::borrow_infer::BorrowMode;

        let Some(arg_modes) = arg_modes else {
            return;
        };
        for (index, arg_var) in arg_vars.iter().enumerate() {
            if arg_modes.get(index) == Some(&BorrowMode::Owned) {
                self.emit(LirInstr::Dup { val: *arg_var });
            }
        }
    }

    /// Lower a single handler arm into a synthetic closure function.
    ///
    /// The closure takes (resume, param0, param1, ...) and executes the
    /// handler body. Free variables from the enclosing scope are captured.
    fn lower_handler_clause(&mut self, handler: &CoreHandler) -> LirVar {
        // Collect free variables in the handler body, excluding params and resume.
        let free = {
            let mut free = HashSet::new();
            let mut bound = HashSet::new();
            bound.insert(handler.resume.id);
            for p in &handler.params {
                bound.insert(p.id);
            }
            free_vars_rec(&handler.body, &mut bound, &mut free);
            free
        };

        let outer_captures: Vec<(CoreBinderId, LirVar)> = free
            .iter()
            .filter_map(|id| self.env.get(id).copied().map(|v| (*id, v)))
            .collect();

        // Temporarily take the program for the inner FnLower.
        let mut temp_program = std::mem::take(self.program);

        let synthetic_id = temp_program.alloc_synthetic_func_id();
        let func_name = format!("handler_clause_{}", temp_program.functions.len());
        let mut inner = FnLower::new(
            func_name,
            synthetic_id,
            format!("handler_{}", synthetic_id.0),
            FnLowerCtx {
                program: &mut temp_program,
                interner: self.interner,
                globals_map: self.globals_map,
                extern_symbols: self.extern_symbols,
                name_binder_map: self.name_binder_map,
                binder_func_id_map: self.binder_func_id_map,
                qualified_names: self.qualified_names,
                top_level_value_map: self.top_level_value_map,
                int_return_binders: self.int_return_binders,
            },
        );

        // Map captured variables.
        for &(binder_id, _outer_var) in &outer_captures {
            let inner_var = inner.fresh_var();
            inner.func.capture_vars.push(inner_var);
            inner.bind(binder_id, inner_var);
        }

        // Parameters: resume first, then handler params.
        let resume_var = inner.fresh_var();
        inner.bind(handler.resume.id, resume_var);
        inner.func.params.push(resume_var);

        for p in &handler.params {
            let pv = inner.fresh_var();
            inner.bind(p.id, pv);
            inner.func.params.push(pv);
        }

        // Lower handler body.
        let result = inner.lower_expr(&handler.body);
        inner.set_terminator(LirTerminator::Return(result));

        let inner_func = inner.func;

        // Restore program and add the inner function.
        *self.program = temp_program;
        self.program.push_function(inner_func);

        // Emit MakeClosure in the outer context.
        let outer_capture_vars: Vec<LirVar> = outer_captures.iter().map(|&(_, v)| v).collect();
        let dst = self.fresh_var();
        self.emit(LirInstr::MakeClosure {
            dst,
            func_id: synthetic_id,
            captures: outer_capture_vars,
        });
        dst
    }

    /// Lower `perform Effect.operation(args)`.
    ///
    /// Phase 1 (tail-resumptive fast path): directly calls the handler via
    /// `flux_perform_direct` with an identity resume closure. No yield flag
    /// or continuation building needed.
    ///
    /// The identity closure, when called with a value, simply returns it.
    /// This is correct for tail-resumptive handlers where `resume(v)` is
    /// the last thing the handler does.
    fn lower_perform(
        &mut self,
        effect: Identifier,
        operation: Identifier,
        args: &[CoreExpr],
    ) -> LirVar {
        use crate::core::CorePrimOp;

        // Lower the argument (currently single-arg effects).
        let arg = if args.is_empty() {
            let none = self.fresh_var();
            self.emit(LirInstr::Const {
                dst: none,
                value: LirConst::None,
            });
            none
        } else {
            self.lower_expr(&args[0])
        };

        // Build identity resume closure: fn(v) → v
        let resume = self.make_identity_closure();

        // Effect tag and operation tag as NaN-boxed integers.
        let htag = self.fresh_var();
        self.emit(LirInstr::Const {
            dst: htag,
            value: LirConst::Tagged(crate::lir::nanbox_tag_int(effect.as_u32() as i64)),
        });

        let optag = self.fresh_var();
        self.emit(LirInstr::Const {
            dst: optag,
            value: LirConst::Tagged(crate::lir::nanbox_tag_int(operation.as_u32() as i64)),
        });

        // Direct perform: calls handler inline with (resume, arg).
        let result = self.fresh_var();
        self.emit(LirInstr::PrimCall {
            dst: Some(result),
            op: CorePrimOp::PerformDirect,
            args: vec![htag, optag, arg, resume],
        });

        result
    }

    /// Create an identity closure: a function that returns its argument.
    /// Used as the `resume` parameter for tail-resumptive effect handlers.
    fn make_identity_closure(&mut self) -> LirVar {
        let mut temp_program = std::mem::take(self.program);

        let synthetic_id = temp_program.alloc_synthetic_func_id();
        let func_name = format!("resume_identity_{}", temp_program.functions.len());
        let mut inner = FnLower::new(
            func_name,
            synthetic_id,
            format!("resume_id_{}", synthetic_id.0),
            FnLowerCtx {
                program: &mut temp_program,
                interner: self.interner,
                globals_map: self.globals_map,
                extern_symbols: self.extern_symbols,
                name_binder_map: self.name_binder_map,
                binder_func_id_map: self.binder_func_id_map,
                qualified_names: self.qualified_names,
                top_level_value_map: self.top_level_value_map,
                int_return_binders: self.int_return_binders,
            },
        );

        // Single parameter: the value to return.
        let param = inner.fresh_var();
        inner.func.params.push(param);
        inner.set_terminator(LirTerminator::Return(param));

        let inner_func = inner.func;
        *self.program = temp_program;
        self.program.push_function(inner_func);

        // Emit MakeClosure (no captures).
        let dst = self.fresh_var();
        self.emit(LirInstr::MakeClosure {
            dst,
            func_id: synthetic_id,
            captures: vec![],
        });
        dst
    }

    // ── PrimOp lowering ──────────────────────────────────────────────

    fn lower_primop(&mut self, op: CorePrimOp, args: &[CoreExpr]) -> LirVar {
        let arg_vars: Vec<LirVar> = args.iter().map(|a| self.lower_expr(a)).collect();

        match op {
            // Typed integer arithmetic → inline LIR instructions.
            // Untag operands, compute, retag result.
            CorePrimOp::IAdd => self.lower_int_binop(LirIntOp::Add, &arg_vars),
            CorePrimOp::ISub => self.lower_int_binop(LirIntOp::Sub, &arg_vars),
            CorePrimOp::IMul => self.lower_int_binop(LirIntOp::Mul, &arg_vars),
            CorePrimOp::IDiv => self.lower_int_binop(LirIntOp::Div, &arg_vars),
            CorePrimOp::IMod => self.lower_int_binop(LirIntOp::Rem, &arg_vars),

            // Typed integer comparisons → inline ICmp.
            CorePrimOp::ICmpEq => self.lower_int_cmp(CmpOp::Eq, &arg_vars),
            CorePrimOp::ICmpNe => self.lower_int_cmp(CmpOp::Ne, &arg_vars),
            CorePrimOp::ICmpLt => self.lower_int_cmp(CmpOp::Slt, &arg_vars),
            CorePrimOp::ICmpLe => self.lower_int_cmp(CmpOp::Sle, &arg_vars),
            CorePrimOp::ICmpGt => self.lower_int_cmp(CmpOp::Sgt, &arg_vars),
            CorePrimOp::ICmpGe => self.lower_int_cmp(CmpOp::Sge, &arg_vars),

            // Boolean logic → control flow (no VM opcode for And/Or).
            CorePrimOp::And => {
                // a && b → if a then b else false
                let a = arg_vars[0];
                let b = arg_vars[1];
                let then_idx = self.new_block();
                let else_idx = self.new_block();
                let join_idx = self.new_block();
                let then_id = BlockId(then_idx as u32);
                let else_id = BlockId(else_idx as u32);
                let join_id = BlockId(join_idx as u32);
                let result = self.fresh_var();
                self.func.blocks[join_idx].params.push(result);

                // Branch on a.
                let a_bool = self.fresh_var();
                self.emit(LirInstr::UntagBool {
                    dst: a_bool,
                    val: a,
                });
                self.set_terminator(LirTerminator::Branch {
                    cond: a_bool,
                    then_block: then_id,
                    else_block: else_id,
                });

                // Then: result = b
                self.switch_to_block(then_idx);
                self.emit_copy_to_join_param(b, join_id);
                self.set_terminator(LirTerminator::Jump(join_id));

                // Else: result = false
                self.switch_to_block(else_idx);
                let false_val = self.fresh_var();
                self.emit(LirInstr::Const {
                    dst: false_val,
                    value: LirConst::Bool(false),
                });
                self.emit_copy_to_join_param(false_val, join_id);
                self.set_terminator(LirTerminator::Jump(join_id));

                self.switch_to_block(join_idx);
                result
            }
            CorePrimOp::Or => {
                // a || b → if a then true else b
                let a = arg_vars[0];
                let b = arg_vars[1];
                let then_idx = self.new_block();
                let else_idx = self.new_block();
                let join_idx = self.new_block();
                let then_id = BlockId(then_idx as u32);
                let else_id = BlockId(else_idx as u32);
                let join_id = BlockId(join_idx as u32);
                let result = self.fresh_var();
                self.func.blocks[join_idx].params.push(result);

                let a_bool = self.fresh_var();
                self.emit(LirInstr::UntagBool {
                    dst: a_bool,
                    val: a,
                });
                self.set_terminator(LirTerminator::Branch {
                    cond: a_bool,
                    then_block: then_id,
                    else_block: else_id,
                });

                // Then: result = true
                self.switch_to_block(then_idx);
                let true_val = self.fresh_var();
                self.emit(LirInstr::Const {
                    dst: true_val,
                    value: LirConst::Bool(true),
                });
                self.emit_copy_to_join_param(true_val, join_id);
                self.set_terminator(LirTerminator::Jump(join_id));

                // Else: result = b
                self.switch_to_block(else_idx);
                self.emit_copy_to_join_param(b, join_id);
                self.set_terminator(LirTerminator::Jump(join_id));

                self.switch_to_block(join_idx);
                result
            }

            // Collection constructors → dedicated LIR instructions.
            CorePrimOp::MakeArray => {
                let dst = self.fresh_var();
                self.emit(LirInstr::MakeArray {
                    dst,
                    elements: arg_vars,
                });
                dst
            }
            CorePrimOp::MakeTuple => {
                let dst = self.fresh_var();
                self.emit(LirInstr::MakeTuple {
                    dst,
                    elements: arg_vars,
                });
                dst
            }
            CorePrimOp::MakeHash => {
                let dst = self.fresh_var();
                self.emit(LirInstr::MakeHash {
                    dst,
                    pairs: arg_vars,
                });
                dst
            }
            CorePrimOp::MakeList => {
                let dst = self.fresh_var();
                self.emit(LirInstr::MakeList {
                    dst,
                    elements: arg_vars,
                });
                dst
            }
            CorePrimOp::Interpolate => {
                let dst = self.fresh_var();
                self.emit(LirInstr::Interpolate {
                    dst,
                    parts: arg_vars,
                });
                dst
            }

            // Generic arithmetic → specialize to inline integer ops when both
            // operands are known integers (local type propagation, Phase 5).
            CorePrimOp::Add
                if arg_vars.len() == 2
                    && self.int_vars.contains(&arg_vars[0])
                    && self.int_vars.contains(&arg_vars[1]) =>
            {
                self.lower_int_binop(LirIntOp::Add, &arg_vars)
            }
            CorePrimOp::Sub
                if arg_vars.len() == 2
                    && self.int_vars.contains(&arg_vars[0])
                    && self.int_vars.contains(&arg_vars[1]) =>
            {
                self.lower_int_binop(LirIntOp::Sub, &arg_vars)
            }
            CorePrimOp::Mul
                if arg_vars.len() == 2
                    && self.int_vars.contains(&arg_vars[0])
                    && self.int_vars.contains(&arg_vars[1]) =>
            {
                self.lower_int_binop(LirIntOp::Mul, &arg_vars)
            }
            CorePrimOp::Div
                if arg_vars.len() == 2
                    && self.int_vars.contains(&arg_vars[0])
                    && self.int_vars.contains(&arg_vars[1]) =>
            {
                self.lower_int_binop(LirIntOp::Div, &arg_vars)
            }
            CorePrimOp::Mod
                if arg_vars.len() == 2
                    && self.int_vars.contains(&arg_vars[0])
                    && self.int_vars.contains(&arg_vars[1]) =>
            {
                self.lower_int_binop(LirIntOp::Rem, &arg_vars)
            }

            // Everything else → C runtime call via PrimCall.
            _ => {
                let dst = self.fresh_var();
                self.emit(LirInstr::PrimCall {
                    dst: Some(dst),
                    op,
                    args: arg_vars,
                });
                dst
            }
        }
    }

    /// Lower typed integer binary op: untag → compute → retag.
    fn lower_int_binop(&mut self, int_op: LirIntOp, args: &[LirVar]) -> LirVar {
        let a_raw = self.fresh_var();
        let b_raw = self.fresh_var();
        self.emit(LirInstr::UntagInt {
            dst: a_raw,
            val: args[0],
        });
        self.emit(LirInstr::UntagInt {
            dst: b_raw,
            val: args[1],
        });

        let result_raw = self.fresh_var();
        let instr = match int_op {
            LirIntOp::Add => LirInstr::IAdd {
                dst: result_raw,
                a: a_raw,
                b: b_raw,
            },
            LirIntOp::Sub => LirInstr::ISub {
                dst: result_raw,
                a: a_raw,
                b: b_raw,
            },
            LirIntOp::Mul => LirInstr::IMul {
                dst: result_raw,
                a: a_raw,
                b: b_raw,
            },
            LirIntOp::Div => LirInstr::IDiv {
                dst: result_raw,
                a: a_raw,
                b: b_raw,
            },
            LirIntOp::Rem => LirInstr::IRem {
                dst: result_raw,
                a: a_raw,
                b: b_raw,
            },
        };
        self.emit(instr);

        let dst = self.fresh_var();
        self.emit(LirInstr::TagInt {
            dst,
            raw: result_raw,
        });
        self.int_vars.insert(dst);
        dst
    }

    /// Lower typed integer comparison: untag → ICmp → retag as bool.
    fn lower_int_cmp(&mut self, cmp_op: CmpOp, args: &[LirVar]) -> LirVar {
        let a_raw = self.fresh_var();
        let b_raw = self.fresh_var();
        self.emit(LirInstr::UntagInt {
            dst: a_raw,
            val: args[0],
        });
        self.emit(LirInstr::UntagInt {
            dst: b_raw,
            val: args[1],
        });

        let cmp_result = self.fresh_var();
        self.emit(LirInstr::ICmp {
            dst: cmp_result,
            op: cmp_op,
            a: a_raw,
            b: b_raw,
        });

        let dst = self.fresh_var();
        self.emit(LirInstr::TagBool {
            dst,
            raw: cmp_result,
        });
        dst
    }

    // ── Phase 3: Pattern matching ────────────────────────────────────

    /// Lower a `Case` expression to LIR blocks with branches/switches.
    fn lower_case(&mut self, scrutinee: &CoreExpr, alts: &[CoreAlt]) -> LirVar {
        let scrut = self.lower_expr(scrutinee);

        // Single wildcard/var alt: no branching needed.
        if alts.len() == 1 {
            return self.lower_single_alt(scrut, &alts[0]);
        }

        // Create a join block where all alt branches merge their results.
        let join_idx = self.new_block();
        let join_id = self.func.blocks[join_idx].id;
        let result_var = self.fresh_var();
        self.func.blocks[join_idx].params.push(result_var);

        // Classify patterns to decide dispatch strategy.
        let has_lit = alts.iter().any(|a| matches!(a.pat, CorePat::Lit(_)));
        let has_con = alts.iter().any(|a| {
            matches!(
                a.pat,
                CorePat::Con { .. } | CorePat::EmptyList | CorePat::Tuple(_)
            )
        });

        if has_lit {
            self.lower_case_lit(scrut, alts, join_id);
        } else if has_con {
            self.lower_case_con(scrut, alts, join_id);
        } else {
            // All wildcards/vars — just take the first alt.
            let val = self.lower_single_alt(scrut, &alts[0]);
            self.emit(LirInstr::Copy {
                dst: result_var,
                src: val,
            });
            self.set_terminator(LirTerminator::Jump(join_id));
            self.switch_to_block(join_idx);
            return result_var;
        }

        // Switch to join block for subsequent code.
        self.switch_to_block(join_idx);
        result_var
    }

    /// Lower a single case alternative (bind pattern vars, evaluate body).
    fn lower_single_alt(&mut self, scrut: LirVar, alt: &CoreAlt) -> LirVar {
        self.bind_pattern(scrut, &alt.pat);
        if let Some(guard) = &alt.guard {
            // Guards: evaluate guard, if false fall through.
            // For now, just evaluate guard and ignore it (Phase 3 simplification).
            let _guard_val = self.lower_expr(guard);
        }
        self.lower_expr(&alt.rhs)
    }

    /// Lower a Case on literal patterns — chain of if-else comparisons.
    fn lower_case_lit(&mut self, scrut: LirVar, alts: &[CoreAlt], join_block: BlockId) {
        for alt in alts {
            match &alt.pat {
                CorePat::Lit(lit) => {
                    let raw_cmp = if matches!(lit, CoreLit::Bool(true)) {
                        // Optimize: `case x of true -> ...` can just untag the
                        // scrutinee directly instead of comparing with a boxed
                        // `true` literal via `flux_rt_eq`.
                        let raw = self.fresh_var();
                        self.emit(LirInstr::UntagBool { dst: raw, val: scrut });
                        raw
                    } else {
                        let lit_var = self.lower_lit(lit);
                        let cmp = self.fresh_var();
                        self.emit(LirInstr::PrimCall {
                            dst: Some(cmp),
                            op: CorePrimOp::CmpEq,
                            args: vec![scrut, lit_var],
                        });
                        let raw = self.fresh_var();
                        self.emit(LirInstr::UntagBool { dst: raw, val: cmp });
                        raw
                    };

                    let then_idx = self.new_block();
                    let else_idx = self.new_block();
                    let then_id = BlockId(then_idx as u32);
                    let else_id = BlockId(else_idx as u32);

                    self.set_terminator(LirTerminator::Branch {
                        cond: raw_cmp,
                        then_block: then_id,
                        else_block: else_id,
                    });

                    // Then: evaluate body, jump to join.
                    self.switch_to_block(then_idx);
                    self.bind_pattern(scrut, &alt.pat);
                    let val = self.lower_expr(&alt.rhs);
                    self.emit_copy_to_join_param(val, join_block);
                    self.set_terminator(LirTerminator::Jump(join_block));

                    // Else: continue chain.
                    self.switch_to_block(else_idx);
                }
                CorePat::Wildcard | CorePat::Var(_) => {
                    self.bind_pattern(scrut, &alt.pat);
                    let val = self.lower_expr(&alt.rhs);
                    self.emit_copy_to_join_param(val, join_block);
                    self.set_terminator(LirTerminator::Jump(join_block));
                    return; // default handled, done.
                }
                _ => {
                    let val = self.lower_single_alt(scrut, alt);
                    self.emit_copy_to_join_param(val, join_block);
                    self.set_terminator(LirTerminator::Jump(join_block));
                }
            }
        }
        // No default — unreachable.
        self.set_terminator(LirTerminator::Unreachable);
    }

    /// Lower a Case on constructor patterns (ADT, cons, None, Some, etc.).
    fn lower_case_con(&mut self, scrut: LirVar, alts: &[CoreAlt], join_block: BlockId) {
        if constructor_case_needs_refutable_field_checks(alts) {
            self.lower_case_con_chain(scrut, alts, join_block);
            return;
        }

        // Pre-allocate blocks for all alts.
        let mut alt_block_indices: Vec<usize> = Vec::new();
        for _alt in alts {
            alt_block_indices.push(self.new_block());
        }

        // Build MatchCtor arms from patterns.
        let mut arms: Vec<CtorArm> = Vec::new();
        let mut default_idx: Option<usize> = None;

        for (i, alt) in alts.iter().enumerate() {
            let block_id = BlockId(alt_block_indices[i] as u32);

            let (ctor_tag, field_pats) = match &alt.pat {
                CorePat::EmptyList => (CtorTag::EmptyList, vec![]),
                CorePat::Con {
                    tag: core_tag,
                    fields,
                    ..
                } => {
                    let ct = match core_tag {
                        CoreTag::None => CtorTag::None,
                        CoreTag::Nil => CtorTag::EmptyList,
                        CoreTag::Some => CtorTag::Some,
                        CoreTag::Left => CtorTag::Left,
                        CoreTag::Right => CtorTag::Right,
                        CoreTag::Cons => CtorTag::Cons,
                        CoreTag::Named(name) => CtorTag::Named(self.resolve_name(*name)),
                    };
                    (ct, fields.clone())
                }
                CorePat::Tuple(fields) => (CtorTag::Tuple, fields.clone()),
                CorePat::Wildcard | CorePat::Var(_) | CorePat::Lit(_) => {
                    default_idx = Some(alt_block_indices[i]);
                    continue;
                }
            };

            // Create field binder LirVars for this arm.
            let field_binders: Vec<LirVar> = field_pats.iter().map(|_| self.fresh_var()).collect();

            arms.push(CtorArm {
                tag: ctor_tag,
                field_binders: field_binders.clone(),
                target: block_id,
            });

            // Pre-bind pattern variables in the target block.
            // We'll bind them when we switch to the block below.
        }

        // Default block.
        let default_block_idx = default_idx.unwrap_or_else(|| {
            let idx = self.new_block();
            let save = self.current_block;
            self.switch_to_block(idx);
            self.set_terminator(LirTerminator::Unreachable);
            self.switch_to_block(save);
            idx
        });
        let default_id = BlockId(default_block_idx as u32);

        // Emit the MatchCtor terminator.
        self.set_terminator(LirTerminator::MatchCtor {
            scrutinee: scrut,
            arms: arms.clone(),
            default: default_id,
        });

        // Lower each alt's body, binding field binders from MatchCtor arms.
        let mut arm_idx = 0;
        for (i, alt) in alts.iter().enumerate() {
            self.switch_to_block(alt_block_indices[i]);

            match &alt.pat {
                CorePat::Wildcard | CorePat::Var(_) | CorePat::Lit(_) => {
                    // Default arm — bind scrutinee to variable if Var pattern.
                    if let CorePat::Var(binder) = &alt.pat {
                        self.bind(binder.id, scrut);
                    }
                }
                CorePat::EmptyList => {
                    // No fields to bind.
                    arm_idx += 1;
                }
                CorePat::Con { fields, .. } | CorePat::Tuple(fields) => {
                    // Bind field binders from the MatchCtor arm.
                    if arm_idx < arms.len() {
                        let arm = &arms[arm_idx];
                        for (j, field_pat) in fields.iter().enumerate() {
                            if j < arm.field_binders.len() {
                                self.bind_pattern(arm.field_binders[j], field_pat);
                            }
                        }
                    }
                    arm_idx += 1;
                }
            }

            let val = self.lower_expr(&alt.rhs);
            self.emit_copy_to_join_param(val, join_block);
            self.set_terminator(LirTerminator::Jump(join_block));
        }
    }

    fn lower_case_con_chain(&mut self, scrut: LirVar, alts: &[CoreAlt], join_block: BlockId) {
        for (index, alt) in alts.iter().enumerate() {
            let success_idx = self.new_block();
            let success_id = BlockId(success_idx as u32);
            let fail_id = if index + 1 == alts.len() {
                let fail_idx = self.new_block();
                BlockId(fail_idx as u32)
            } else {
                let fail_idx = self.new_block();
                BlockId(fail_idx as u32)
            };

            self.emit_pattern_check(scrut, &alt.pat, success_id, fail_id);

            self.switch_to_block(success_idx);
            self.bind_pattern(scrut, &alt.pat);
            let val = self.lower_expr(&alt.rhs);
            self.emit_copy_to_join_param(val, join_block);
            self.set_terminator(LirTerminator::Jump(join_block));

            self.switch_to_block(fail_id.0 as usize);
        }

        self.set_terminator(LirTerminator::Unreachable);
    }

    fn emit_pattern_check(
        &mut self,
        scrut: LirVar,
        pat: &CorePat,
        success: BlockId,
        fail: BlockId,
    ) {
        match pat {
            CorePat::Wildcard | CorePat::Var(_) => {
                self.set_terminator(LirTerminator::Jump(success));
            }
            CorePat::Lit(lit) => {
                let lit_var = self.lower_lit(lit);
                let cmp = self.fresh_var();
                self.emit(LirInstr::PrimCall {
                    dst: Some(cmp),
                    op: CorePrimOp::CmpEq,
                    args: vec![scrut, lit_var],
                });
                let raw_cmp = self.fresh_var();
                self.emit(LirInstr::UntagBool {
                    dst: raw_cmp,
                    val: cmp,
                });
                self.set_terminator(LirTerminator::Branch {
                    cond: raw_cmp,
                    then_block: success,
                    else_block: fail,
                });
            }
            CorePat::EmptyList => {
                let empty = self.fresh_var();
                self.emit(LirInstr::Const {
                    dst: empty,
                    value: LirConst::EmptyList,
                });
                let cmp = self.fresh_var();
                self.emit(LirInstr::PrimCall {
                    dst: Some(cmp),
                    op: CorePrimOp::CmpEq,
                    args: vec![scrut, empty],
                });
                let raw_cmp = self.fresh_var();
                self.emit(LirInstr::UntagBool {
                    dst: raw_cmp,
                    val: cmp,
                });
                self.set_terminator(LirTerminator::Branch {
                    cond: raw_cmp,
                    then_block: success,
                    else_block: fail,
                });
            }
            CorePat::Con { tag, fields } => {
                let next_idx = self.new_block();
                let next_id = BlockId(next_idx as u32);
                let field_binders: Vec<LirVar> = fields.iter().map(|_| self.fresh_var()).collect();
                let ctor_tag = match tag {
                    CoreTag::None => CtorTag::None,
                    CoreTag::Nil => CtorTag::EmptyList,
                    CoreTag::Some => CtorTag::Some,
                    CoreTag::Left => CtorTag::Left,
                    CoreTag::Right => CtorTag::Right,
                    CoreTag::Cons => CtorTag::Cons,
                    CoreTag::Named(name) => CtorTag::Named(self.resolve_name(*name)),
                };
                self.set_terminator(LirTerminator::MatchCtor {
                    scrutinee: scrut,
                    arms: vec![CtorArm {
                        tag: ctor_tag,
                        field_binders: field_binders.clone(),
                        target: next_id,
                    }],
                    default: fail,
                });

                self.switch_to_block(next_idx);
                self.emit_field_pattern_checks(&field_binders, fields, success, fail);
            }
            CorePat::Tuple(fields) => {
                if fields.is_empty() {
                    self.set_terminator(LirTerminator::Jump(success));
                    return;
                }
                let field_vars: Vec<LirVar> = fields
                    .iter()
                    .enumerate()
                    .map(|(index, _)| {
                        let field_val = self.fresh_var();
                        self.emit(LirInstr::TupleGet {
                            dst: field_val,
                            tuple: scrut,
                            index,
                        });
                        field_val
                    })
                    .collect();
                self.emit_field_pattern_checks(&field_vars, fields, success, fail);
            }
        }
    }

    fn emit_field_pattern_checks(
        &mut self,
        field_vars: &[LirVar],
        field_pats: &[CorePat],
        success: BlockId,
        fail: BlockId,
    ) {
        if field_vars.is_empty() || field_pats.is_empty() {
            self.set_terminator(LirTerminator::Jump(success));
            return;
        }

        for (index, (field_var, field_pat)) in field_vars.iter().zip(field_pats.iter()).enumerate() {
            let next_block = if index + 1 == field_vars.len() {
                success
            } else {
                BlockId(self.new_block() as u32)
            };
            self.emit_pattern_check(*field_var, field_pat, next_block, fail);
            if index + 1 < field_vars.len() {
                self.switch_to_block(next_block.0 as usize);
            }
        }
    }

    /// Bind pattern variables to LIR vars by extracting fields from scrutinee.
    fn bind_pattern(&mut self, scrut: LirVar, pat: &CorePat) {
        match pat {
            CorePat::Wildcard => {}
            CorePat::Var(binder) => {
                self.bind(binder.id, scrut);
            }
            CorePat::Lit(_) => {}
            CorePat::EmptyList => {}
            CorePat::Con { tag: _, fields, .. } => {
                if fields.is_empty() {
                    return;
                }
                // Untag the pointer to access heap fields.
                let ptr = self.fresh_var();
                self.emit(LirInstr::UntagPtr {
                    dst: ptr,
                    val: scrut,
                });
                for (i, field_pat) in fields.iter().enumerate() {
                    let field_val = self.fresh_var();
                    let offset = ADT_HEADER_SIZE + (i as i32) * 8;
                    self.emit(LirInstr::Load {
                        dst: field_val,
                        ptr,
                        offset,
                    });
                    self.bind_pattern(field_val, field_pat);
                }
            }
            CorePat::Tuple(fields) => {
                if fields.is_empty() {
                    return;
                }
                for (i, field_pat) in fields.iter().enumerate() {
                    let field_val = self.fresh_var();
                    self.emit(LirInstr::TupleGet {
                        dst: field_val,
                        tuple: scrut,
                        index: i,
                    });
                    self.bind_pattern(field_val, field_pat);
                }
            }
        }
    }

    // ── Phase 3: Constructor lowering ────────────────────────────────

    /// Lower a `Con` expression (ADT, cons, some, none, etc.).
    fn lower_con(&mut self, tag: &CoreTag, fields: &[CoreExpr]) -> LirVar {
        let field_vars: Vec<LirVar> = fields.iter().map(|f| self.lower_expr(f)).collect();

        match tag {
            CoreTag::None | CoreTag::Nil => {
                // Immediate values — no heap allocation.
                let dst = self.fresh_var();
                let value = if matches!(tag, CoreTag::Nil) {
                    LirConst::EmptyList
                } else {
                    LirConst::None
                };
                self.emit(LirInstr::Const { dst, value });
                dst
            }
            CoreTag::Some | CoreTag::Left | CoreTag::Right | CoreTag::Cons => {
                let ctor_tag = match tag {
                    CoreTag::Some => SOME_TAG_ID as i32,
                    CoreTag::Left => LEFT_TAG_ID as i32,
                    CoreTag::Right => RIGHT_TAG_ID as i32,
                    CoreTag::Cons => CONS_TAG_ID as i32,
                    _ => unreachable!(),
                };
                let dst = self.fresh_var();
                self.emit(LirInstr::MakeCtor {
                    dst,
                    ctor_tag,
                    ctor_name: None, // built-in ctors don't need a name
                    fields: field_vars,
                });
                dst
            }
            CoreTag::Named(name) => {
                let ctor_name = self.resolve_name(*name);
                let ctor_tag = self
                    .program
                    .constructor_tags
                    .get(&ctor_name)
                    .copied()
                    .unwrap_or(FIRST_USER_TAG_ID as i32);
                let dst = self.fresh_var();
                self.emit(LirInstr::MakeCtor {
                    dst,
                    ctor_tag,
                    ctor_name: Some(ctor_name),
                    fields: field_vars,
                });
                dst
            }
        }
    }

    // ── Phase 5: Aether RC ───────────────────────────────────────────

    /// Lower `Reuse { token, tag, fields, field_mask }`.
    ///
    /// Perceus reuse: try to reuse `token`'s heap allocation for a new
    /// constructor.  Emit `DropReuse` to test uniqueness.  If the token
    /// was unique (returned non-null), write fields in-place.  If shared,
    /// fall back to a fresh allocation.
    fn lower_reuse(
        &mut self,
        token: &crate::core::CoreVarRef,
        tag: &CoreTag,
        fields: &[CoreExpr],
        field_mask: Option<u64>,
    ) -> LirVar {
        let field_vars: Vec<LirVar> = fields.iter().map(|f| self.lower_expr(f)).collect();

        let ctor_tag = match tag {
            CoreTag::Some => SOME_TAG_ID as i32,
            CoreTag::Left => LEFT_TAG_ID as i32,
            CoreTag::Right => RIGHT_TAG_ID as i32,
            CoreTag::Cons => CONS_TAG_ID as i32,
            CoreTag::Named(name) => {
                let ctor_name = self.resolve_name(*name);
                self.program
                    .constructor_tags
                    .get(&ctor_name)
                    .copied()
                    .unwrap_or(FIRST_USER_TAG_ID as i32)
            }
            CoreTag::None | CoreTag::Nil => {
                // None/Nil are immediates — no allocation to reuse.
                let dst = self.fresh_var();
                let value = if matches!(tag, CoreTag::Nil) {
                    LirConst::EmptyList
                } else {
                    LirConst::None
                };
                self.emit(LirInstr::Const { dst, value });
                return dst;
            }
        };

        // Try to reuse the token's allocation.
        let token_var = if let Some(b) = token.binder {
            self.lookup(b)
        } else {
            let v = self.fresh_var();
            self.emit(LirInstr::Const {
                dst: v,
                value: LirConst::None,
            });
            v
        };

        let is_reusable = self.fresh_var();
        self.emit(LirInstr::IsUnique {
            dst: is_reusable,
            val: token_var,
        });

        let reuse_idx = self.new_block();
        let fresh_idx = self.new_block();
        let join_idx = self.new_block();
        let reuse_id = BlockId(reuse_idx as u32);
        let fresh_id = BlockId(fresh_idx as u32);
        let join_id = BlockId(join_idx as u32);

        let result = self.fresh_var();
        self.func.blocks[join_idx].params.push(result);

        self.set_terminator(LirTerminator::Branch {
            cond: is_reusable,
            then_block: reuse_id,
            else_block: fresh_id,
        });

        // Reuse path: write header + fields into existing allocation.
        self.switch_to_block(reuse_idx);
        let reuse_ptr = self.fresh_var();
        self.emit(LirInstr::UntagPtr {
            dst: reuse_ptr,
            val: token_var,
        });
        self.emit(LirInstr::StoreI32 {
            ptr: reuse_ptr,
            offset: 0,
            value: ctor_tag,
        });
        self.emit(LirInstr::StoreI32 {
            ptr: reuse_ptr,
            offset: 4,
            value: field_vars.len() as i32,
        });
        for (i, fv) in field_vars.iter().enumerate() {
            // If field_mask is set, skip unchanged fields on the reuse path.
            if let Some(mask) = field_mask
                && mask & (1 << i) == 0
            {
                continue; // field unchanged, skip write
            }
            self.emit(LirInstr::Store {
                ptr: reuse_ptr,
                offset: ADT_HEADER_SIZE + (i as i32) * 8,
                val: *fv,
            });
        }
        let reuse_tagged = self.fresh_var();
        self.emit(LirInstr::TagPtr {
            dst: reuse_tagged,
            ptr: reuse_ptr,
        });
        self.emit_copy_to_join_param(reuse_tagged, join_id);
        self.set_terminator(LirTerminator::Jump(join_id));

        // Fresh alloc path — use MakeCtor (high-level).
        self.switch_to_block(fresh_idx);
        self.emit(LirInstr::Drop { val: token_var });
        let ctor_name = match tag {
            CoreTag::Named(name) => Some(self.resolve_name(*name)),
            _ => None,
        };
        let fresh_val = self.fresh_var();
        self.emit(LirInstr::MakeCtor {
            dst: fresh_val,
            ctor_tag,
            ctor_name,
            fields: field_vars.clone(),
        });
        self.emit_copy_to_join_param(fresh_val, join_id);
        self.set_terminator(LirTerminator::Jump(join_id));

        // Join.
        self.switch_to_block(join_idx);
        result
    }

    /// Lower `DropSpecialized { scrutinee, unique_body, shared_body }`.
    ///
    /// Perceus drop specialization: test if scrutinee is uniquely owned.
    /// - Unique: fields are already owned, only free the shell.
    /// - Shared: dup fields, decrement scrutinee refcount.
    fn lower_drop_specialized(
        &mut self,
        scrutinee: &crate::core::CoreVarRef,
        unique_body: &CoreExpr,
        shared_body: &CoreExpr,
    ) -> LirVar {
        let scrut_var = if let Some(b) = scrutinee.binder {
            self.lookup(b)
        } else {
            let v = self.fresh_var();
            self.emit(LirInstr::Const {
                dst: v,
                value: LirConst::None,
            });
            v
        };

        let is_unique = self.fresh_var();
        self.emit(LirInstr::IsUnique {
            dst: is_unique,
            val: scrut_var,
        });

        let unique_idx = self.new_block();
        let shared_idx = self.new_block();
        let join_idx = self.new_block();
        let unique_id = BlockId(unique_idx as u32);
        let shared_id = BlockId(shared_idx as u32);
        let join_id = BlockId(join_idx as u32);

        let result = self.fresh_var();
        self.func.blocks[join_idx].params.push(result);

        self.set_terminator(LirTerminator::Branch {
            cond: is_unique,
            then_block: unique_id,
            else_block: shared_id,
        });

        // Unique path.
        self.switch_to_block(unique_idx);
        let unique_val = self.lower_expr(unique_body);
        self.emit_copy_to_join_param(unique_val, join_id);
        self.set_terminator(LirTerminator::Jump(join_id));

        // Shared path.
        self.switch_to_block(shared_idx);
        let shared_val = self.lower_expr(shared_body);
        self.emit_copy_to_join_param(shared_val, join_id);
        self.set_terminator(LirTerminator::Jump(join_id));

        // Join.
        self.switch_to_block(join_idx);
        result
    }
}

/// Internal enum for typed integer binary operations.
enum LirIntOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

// ── Top-level definition lowering ────────────────────────────────────────────

/// Lower a single `CoreDef` to a `LirFunction`.
///
/// `binder_func_map` maps sibling function binder IDs to their LIR function
/// indices, so cross-function references emit `MakeClosure` instead of
/// `None` placeholders.
struct LowerDefCtx<'a> {
    program: &'a mut LirProgram,
    binder_func_map: &'a HashMap<CoreBinderId, LirFuncId>,
    qualified_names: &'a HashMap<CoreBinderId, String>,
    name_binder_map: &'a HashMap<crate::syntax::Identifier, Vec<CoreBinderId>>,
    interner: Option<&'a Interner>,
    globals_map: Option<&'a HashMap<String, usize>>,
    extern_symbols: Option<&'a HashMap<String, ImportedNativeSymbol>>,
    top_level_value_map: &'a HashMap<CoreBinderId, &'a CoreExpr>,
    int_return_binders: &'a HashSet<CoreBinderId>,
}

fn lower_def(def: &CoreDef, ctx: LowerDefCtx<'_>) -> LirFunction {
    let func_id = LirFuncId(def.binder.id.0);
    let debug_name = format!("def_{}", def.binder.id.0);
    let qualified_name = ctx
        .qualified_names
        .get(&def.binder.id)
        .cloned()
        .unwrap_or_else(|| debug_name.clone());
    let mut ctx = FnLower::new(
        debug_name,
        func_id,
        qualified_name,
        FnLowerCtx {
            program: ctx.program,
            interner: ctx.interner,
            globals_map: ctx.globals_map,
            extern_symbols: ctx.extern_symbols,
            name_binder_map: ctx.name_binder_map,
            binder_func_id_map: ctx.binder_func_map,
            qualified_names: ctx.qualified_names,
            top_level_value_map: ctx.top_level_value_map,
            int_return_binders: ctx.int_return_binders,
        },
    );

    // Register top-level binders for direct call resolution.
    // Unlike the old approach (which created MakeClosure for every sibling),
    // we only record the binder→func_id mapping.  Closures are created lazily
    // in lower_expr(Var) only when a function is used as a higher-order value.
    // Direct calls (CallKind::Direct) don't need closure objects at all.
    // This follows GHC's approach: known calls are free, closures are only
    // created when functions escape as values.

    // If the def is a lambda, register its parameters.
    let body = match &def.expr {
        CoreExpr::Lam { params, body, .. } => {
            for param in params {
                let pv = ctx.fresh_var();
                ctx.bind(param.id, pv);
                ctx.func.params.push(pv);
            }
            body.as_ref()
        }
        other => other,
    };

    let result = ctx.lower_expr(body);
    ctx.set_terminator(LirTerminator::Return(result));

    ctx.func
}

// ── Display ──────────────────────────────────────────────────────────────────

/// Pretty-print a `LirProgram` for `--dump-lir`.
pub fn display_program(program: &LirProgram) -> String {
    let mut out = String::new();
    for func in &program.functions {
        display_function(func, &mut out);
        out.push('\n');
    }
    out
}

fn display_function(func: &LirFunction, out: &mut String) {
    use std::fmt::Write;
    let params: Vec<String> = func.params.iter().map(|v| format!("{v}")).collect();
    if func.capture_vars.is_empty() {
        writeln!(
            out,
            "fn {} [{}] ({}) {{",
            func.qualified_name,
            func.id,
            params.join(", ")
        )
        .unwrap();
    } else {
        let caps: Vec<String> = func.capture_vars.iter().map(|v| format!("{v}")).collect();
        writeln!(
            out,
            "fn {} [{}] ({}) captures [{}] {{",
            func.qualified_name,
            func.id,
            params.join(", "),
            caps.join(", ")
        )
        .unwrap();
    }
    for block in &func.blocks {
        display_block(block, out);
    }
    writeln!(out, "}}").unwrap();
}

fn display_block(block: &LirBlock, out: &mut String) {
    use std::fmt::Write;
    let params: Vec<String> = block.params.iter().map(|v| format!("{v}")).collect();
    if params.is_empty() {
        writeln!(out, "  {}:", block.id).unwrap();
    } else {
        writeln!(out, "  {}({}):", block.id, params.join(", ")).unwrap();
    }
    for instr in &block.instrs {
        writeln!(out, "    {}", display_instr(instr)).unwrap();
    }
    writeln!(out, "    {}", display_terminator(&block.terminator)).unwrap();
}

fn display_instr(instr: &LirInstr) -> String {
    match instr {
        LirInstr::Load { dst, ptr, offset } => format!("{dst} = load {ptr}[{offset}]"),
        LirInstr::Store { ptr, offset, val } => format!("store {ptr}[{offset}] = {val}"),
        LirInstr::StoreI32 { ptr, offset, value } => {
            format!("store_i32 {ptr}[{offset}] = {value}")
        }
        LirInstr::Alloc {
            dst,
            size,
            scan_fields,
            obj_tag,
        } => {
            format!("{dst} = alloc({size}, scan={scan_fields}, tag={obj_tag})")
        }
        LirInstr::TagInt { dst, raw } => format!("{dst} = tag_int({raw})"),
        LirInstr::UntagInt { dst, val } => format!("{dst} = untag_int({val})"),
        LirInstr::TagFloat { dst, raw } => format!("{dst} = tag_float({raw})"),
        LirInstr::UntagFloat { dst, val } => format!("{dst} = untag_float({val})"),
        LirInstr::GetTag { dst, val } => format!("{dst} = get_tag({val})"),
        LirInstr::TagPtr { dst, ptr } => format!("{dst} = tag_ptr({ptr})"),
        LirInstr::UntagPtr { dst, val } => format!("{dst} = untag_ptr({val})"),
        LirInstr::TagBool { dst, raw } => format!("{dst} = tag_bool({raw})"),
        LirInstr::UntagBool { dst, val } => format!("{dst} = untag_bool({val})"),
        LirInstr::IAdd { dst, a, b } => format!("{dst} = iadd {a}, {b}"),
        LirInstr::ISub { dst, a, b } => format!("{dst} = isub {a}, {b}"),
        LirInstr::IMul { dst, a, b } => format!("{dst} = imul {a}, {b}"),
        LirInstr::IDiv { dst, a, b } => format!("{dst} = idiv {a}, {b}"),
        LirInstr::IRem { dst, a, b } => format!("{dst} = irem {a}, {b}"),
        LirInstr::ICmp { dst, op, a, b } => format!("{dst} = icmp {op} {a}, {b}"),
        LirInstr::PrimCall { dst, op, args } => {
            let args_str: Vec<String> = args.iter().map(|v| format!("{v}")).collect();
            match dst {
                Some(d) => format!("{d} = call {:?}({})", op, args_str.join(", ")),
                None => format!("call {:?}({})", op, args_str.join(", ")),
            }
        }
        LirInstr::Dup { val } => format!("dup {val}"),
        LirInstr::Drop { val } => format!("drop {val}"),
        LirInstr::IsUnique { dst, val } => format!("{dst} = is_unique({val})"),
        LirInstr::DropReuse { dst, val, size } => format!("{dst} = drop_reuse({val}, size={size})"),
        LirInstr::MakeClosure {
            dst,
            func_id,
            captures,
        } => {
            let caps: Vec<String> = captures.iter().map(|v| format!("{v}")).collect();
            format!("{dst} = make_closure({func_id}, [{}])", caps.join(", "))
        }
        LirInstr::MakeExternClosure { dst, symbol, arity } => {
            format!("{dst} = make_extern_closure({symbol}, arity={arity})")
        }
        LirInstr::MakeArray { dst, elements } => {
            let es: Vec<String> = elements.iter().map(|v| format!("{v}")).collect();
            format!("{dst} = make_array([{}])", es.join(", "))
        }
        LirInstr::MakeTuple { dst, elements } => {
            let es: Vec<String> = elements.iter().map(|v| format!("{v}")).collect();
            format!("{dst} = make_tuple([{}])", es.join(", "))
        }
        LirInstr::MakeHash { dst, pairs } => {
            let ps: Vec<String> = pairs.iter().map(|v| format!("{v}")).collect();
            format!("{dst} = make_hash([{}])", ps.join(", "))
        }
        LirInstr::MakeList { dst, elements } => {
            let es: Vec<String> = elements.iter().map(|v| format!("{v}")).collect();
            format!("{dst} = make_list([{}])", es.join(", "))
        }
        LirInstr::Interpolate { dst, parts } => {
            let ps: Vec<String> = parts.iter().map(|v| format!("{v}")).collect();
            format!("{dst} = interpolate([{}])", ps.join(", "))
        }
        LirInstr::MakeCtor {
            dst,
            ctor_tag,
            ctor_name,
            fields,
        } => {
            let fs: Vec<String> = fields.iter().map(|v| format!("{v}")).collect();
            let name = ctor_name.as_deref().unwrap_or("?");
            format!(
                "{dst} = make_ctor(tag={ctor_tag}, name={name}, [{}])",
                fs.join(", ")
            )
        }
        LirInstr::Copy { dst, src } => format!("{dst} = copy {src}"),
        LirInstr::Const { dst, value } => format!("{dst} = const {value:?}"),
        LirInstr::TupleGet { dst, tuple, index } => format!("{dst} = tuple_get({tuple}, {index})"),
        LirInstr::GetGlobal { dst, global_idx } => format!("{dst} = get_global({global_idx})"),
    }
}

fn display_terminator(term: &LirTerminator) -> String {
    match term {
        LirTerminator::Return(v) => format!("ret {v}"),
        LirTerminator::Jump(block) => format!("jmp {block}"),
        LirTerminator::Branch {
            cond,
            then_block,
            else_block,
        } => {
            format!("br {cond}, {then_block}, {else_block}")
        }
        LirTerminator::Switch {
            scrutinee,
            cases,
            default,
        } => {
            let cases_str: Vec<String> = cases
                .iter()
                .map(|(val, block)| format!("{val} -> {block}"))
                .collect();
            format!(
                "switch {scrutinee} [{}, default -> {default}]",
                cases_str.join(", ")
            )
        }
        LirTerminator::TailCall { func, args, kind } => {
            let args_str: Vec<String> = args.iter().map(|v| format!("{v}")).collect();
            let kind_str = match kind {
                CallKind::Direct { func_id } => format!(" [direct {}]", func_id),
                CallKind::DirectExtern { symbol } => format!(" [extern {}]", symbol),
                CallKind::Indirect => String::new(),
            };
            format!("tailcall {func}({}){kind_str}", args_str.join(", "))
        }
        LirTerminator::Call {
            dst,
            func,
            args,
            cont,
            kind,
            yield_cont: _,
        } => {
            let args_str: Vec<String> = args.iter().map(|v| format!("{v}")).collect();
            let kind_str = match kind {
                CallKind::Direct { func_id } => format!(" [direct {}]", func_id),
                CallKind::DirectExtern { symbol } => format!(" [extern {}]", symbol),
                CallKind::Indirect => String::new(),
            };
            format!(
                "{dst} = call {func}({}){kind_str} -> {cont}",
                args_str.join(", ")
            )
        }
        LirTerminator::MatchCtor {
            scrutinee,
            arms,
            default,
        } => {
            let arms_str: Vec<String> = arms
                .iter()
                .map(|arm| {
                    let fs: Vec<String> =
                        arm.field_binders.iter().map(|v| format!("{v}")).collect();
                    format!("{:?}({}) -> {}", arm.tag, fs.join(", "), arm.target)
                })
                .collect();
            format!(
                "match_ctor {scrutinee} [{}, default -> {default}]",
                arms_str.join(", ")
            )
        }
        LirTerminator::Unreachable => "unreachable".to_string(),
    }
}

fn constructor_case_needs_refutable_field_checks(alts: &[CoreAlt]) -> bool {
    let mut seen_tags = HashSet::new();
    for alt in alts {
        match &alt.pat {
            CorePat::Con { tag, fields } => {
                let tag_key = format!("{tag:?}");
                if !seen_tags.insert(tag_key) {
                    return true;
                }
                if fields.iter().any(pattern_is_refutable) {
                    return true;
                }
            }
            CorePat::Tuple(fields) => {
                if fields.iter().any(pattern_is_refutable) {
                    return true;
                }
            }
            CorePat::EmptyList | CorePat::Wildcard | CorePat::Var(_) | CorePat::Lit(_) => {}
        }
    }
    false
}

fn pattern_is_refutable(pat: &CorePat) -> bool {
    match pat {
        CorePat::Wildcard | CorePat::Var(_) => false,
        CorePat::Lit(_) | CorePat::EmptyList | CorePat::Con { .. } | CorePat::Tuple(_) => true,
    }
}

// ── Free variable collection ─────────────────────────────────────────────────

/// Collect free variable binder IDs in a `CoreExpr`.
fn collect_free_vars(expr: &CoreExpr) -> HashSet<CoreBinderId> {
    let mut free = HashSet::new();
    let mut bound = HashSet::new();
    free_vars_rec(expr, &mut bound, &mut free);
    free
}

fn free_vars_rec(
    expr: &CoreExpr,
    bound: &mut HashSet<CoreBinderId>,
    free: &mut HashSet<CoreBinderId>,
) {
    match expr {
        CoreExpr::Var { var, .. } => {
            if let Some(b) = var.binder
                && !bound.contains(&b)
            {
                free.insert(b);
            }
        }
        CoreExpr::Lit(_, _) => {}
        CoreExpr::Lam { params, body, .. } => {
            let added: Vec<_> = params
                .iter()
                .filter(|p| bound.insert(p.id))
                .map(|p| p.id)
                .collect();
            free_vars_rec(body, bound, free);
            for id in added {
                bound.remove(&id);
            }
        }
        CoreExpr::App { func, args, .. } | CoreExpr::AetherCall { func, args, .. } => {
            free_vars_rec(func, bound, free);
            for a in args {
                free_vars_rec(a, bound, free);
            }
        }
        CoreExpr::Let { var, rhs, body, .. } => {
            free_vars_rec(rhs, bound, free);
            let added = bound.insert(var.id);
            free_vars_rec(body, bound, free);
            if added {
                bound.remove(&var.id);
            }
        }
        CoreExpr::LetRec { var, rhs, body, .. } => {
            let added = bound.insert(var.id);
            free_vars_rec(rhs, bound, free);
            free_vars_rec(body, bound, free);
            if added {
                bound.remove(&var.id);
            }
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            free_vars_rec(scrutinee, bound, free);
            for alt in alts {
                let added = bind_pat(&alt.pat, bound);
                if let Some(g) = &alt.guard {
                    free_vars_rec(g, bound, free);
                }
                free_vars_rec(&alt.rhs, bound, free);
                for id in added {
                    bound.remove(&id);
                }
            }
        }
        CoreExpr::Con { fields, .. } | CoreExpr::PrimOp { args: fields, .. } => {
            for f in fields {
                free_vars_rec(f, bound, free);
            }
        }
        CoreExpr::Return { value, .. } => free_vars_rec(value, bound, free),
        CoreExpr::Perform { args, .. } => {
            for a in args {
                free_vars_rec(a, bound, free);
            }
        }
        CoreExpr::Handle { body, handlers, .. } => {
            free_vars_rec(body, bound, free);
            for h in handlers {
                let mut added = vec![];
                if bound.insert(h.resume.id) {
                    added.push(h.resume.id);
                }
                for p in &h.params {
                    if bound.insert(p.id) {
                        added.push(p.id);
                    }
                }
                free_vars_rec(&h.body, bound, free);
                for id in added {
                    bound.remove(&id);
                }
            }
        }
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            free_vars_rec(object, bound, free);
        }
        CoreExpr::Dup { var, body, .. } | CoreExpr::Drop { var, body, .. } => {
            if let Some(b) = var.binder
                && !bound.contains(&b)
            {
                free.insert(b);
            }
            free_vars_rec(body, bound, free);
        }
        CoreExpr::Reuse { token, fields, .. } => {
            if let Some(b) = token.binder
                && !bound.contains(&b)
            {
                free.insert(b);
            }
            for f in fields {
                free_vars_rec(f, bound, free);
            }
        }
        CoreExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            ..
        } => {
            if let Some(b) = scrutinee.binder
                && !bound.contains(&b)
            {
                free.insert(b);
            }
            free_vars_rec(unique_body, bound, free);
            free_vars_rec(shared_body, bound, free);
        }
    }
}

/// Bind pattern variables into the bound set, returning newly added IDs.
fn bind_pat(pat: &CorePat, bound: &mut HashSet<CoreBinderId>) -> Vec<CoreBinderId> {
    let mut added = vec![];
    match pat {
        CorePat::Var(b) => {
            if bound.insert(b.id) {
                added.push(b.id);
            }
        }
        CorePat::Con { fields, .. } => {
            for f in fields {
                added.extend(bind_pat(f, bound));
            }
        }
        CorePat::Tuple(fields) => {
            for f in fields {
                added.extend(bind_pat(f, bound));
            }
        }
        CorePat::Wildcard | CorePat::Lit(_) | CorePat::EmptyList => {}
    }
    added
}
