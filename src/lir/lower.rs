//! Core IR → LIR lowering (Proposal 0132 Phases 2–3).
//!
//! Translates the functional Core IR into the flat, NaN-box-aware LIR CFG.
//! - Phase 2: literals, variables, let/letrec bindings, primop calls, top-level functions.
//! - Phase 3: pattern matching (Case), ADT/cons/tuple construction (Con), tuple field access.

use std::collections::HashMap;

use std::collections::HashSet;

use crate::core::{
    CoreAlt, CoreBinderId, CoreDef, CoreExpr, CoreLit, CorePat, CorePrimOp, CoreProgram, CoreTag,
    CoreTopLevelItem,
};
use crate::lir::*;
use crate::syntax::interner::Interner;

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
        // Collection access
        ("first", 1) => Some(CorePrimOp::First),
        ("last", 1) => Some(CorePrimOp::Last),
        ("rest", 1) => Some(CorePrimOp::Rest),
        // Higher-order (C runtime calls closures via flux_call_closure_c)
        ("map", 2) => Some(CorePrimOp::HoMap),
        ("filter", 2) => Some(CorePrimOp::HoFilter),
        ("sort", 1) => Some(CorePrimOp::Sort),
        ("sort_by", 2) => Some(CorePrimOp::SortBy),
        ("any", 2) => Some(CorePrimOp::HoAny),
        ("all", 2) => Some(CorePrimOp::HoAll),
        ("each", 2) => Some(CorePrimOp::HoEach),
        ("find", 2) => Some(CorePrimOp::HoFind),
        ("count", 2) => Some(CorePrimOp::HoCount),
        ("flat_map", 2) => Some(CorePrimOp::HoFlatMap),
        ("zip", 2) => Some(CorePrimOp::Zip),
        ("flatten", 1) => Some(CorePrimOp::Flatten),
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
    out: &mut Vec<(crate::syntax::Identifier, String)>,
    interner: Option<&Interner>,
) {
    match item {
        CoreTopLevelItem::Function { name, .. } => {
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
            out.push((*name, sanitized));
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
    // Step 1: Collect (bare_name, qualified_name) pairs from the module tree.
    let mut name_qualified_pairs: Vec<(crate::syntax::Identifier, String)> = Vec::new();
    for item in &program.top_level_items {
        collect_module_paths(item, &[], &mut name_qualified_pairs, interner);
    }

    // Step 2: Match CoreDef entries to qualified names.
    // For each (bare_name, qualified_name) pair, find the first def with that
    // bare name that hasn't been claimed yet.
    let mut result = HashMap::new();
    let mut claimed: HashSet<CoreBinderId> = HashSet::new();

    for (bare_name, qualified_name) in &name_qualified_pairs {
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
        if !result.contains_key(&def.binder.id) {
            let bare = interner
                .map(|i| i.resolve(def.name).to_string())
                .unwrap_or_else(|| format!("def_{}", def.binder.id.0));
            result.insert(def.binder.id, bare);
        }
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
                    if !tags.contains_key(&name) {
                        tags.insert(name, *next_tag);
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
    let mut lir = LirProgram::new();

    // Build module-qualified names from the CoreTopLevelItem tree.
    let qualified_names = build_qualified_names(program, interner);

    // Collect user-defined ADT constructor tags from Data declarations.
    collect_constructor_tags(
        &program.top_level_items,
        &mut lir.constructor_tags,
        interner,
    );

    // Collect all top-level binder IDs so cross-function references resolve.
    let top_level_binders: Vec<CoreBinderId> = program.defs.iter().map(|d| d.binder.id).collect();

    // Find the main def — it could be at any position in defs[].
    let main_idx = if let Some(interner) = interner {
        program
            .defs
            .iter()
            .position(|d| interner.resolve(d.name) == "main")
            .unwrap_or(0)
    } else {
        0
    };

    // Pre-assign LirFuncIds for all top-level defs (1:1 with CoreBinderId).
    let mut binder_func_map: HashMap<CoreBinderId, LirFuncId> = HashMap::new();
    for (i, def) in program.defs.iter().enumerate() {
        if i == main_idx {
            continue;
        }
        binder_func_map.insert(def.binder.id, LirFuncId(def.binder.id.0));
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

    // Phase 1: Lower all non-main defs.
    for (i, def) in program.defs.iter().enumerate() {
        if i == main_idx {
            continue;
        }
        let func = lower_def(
            def,
            &mut lir,
            &top_level_binders,
            &binder_func_map,
            &qualified_names,
            &name_binder_map,
            interner,
            globals_map,
        );
        lir.push_function(func);
    }

    // Phase 2: Lower main with knowledge of all sibling functions.
    // Main is always last in LIR (emit_program expects this).
    let main_def = &program.defs[main_idx];
    let func = lower_def(
        main_def,
        &mut lir,
        &top_level_binders,
        &binder_func_map,
        &qualified_names,
        &name_binder_map,
        interner,
        globals_map,
    );
    lir.push_function(func);

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
}

impl<'a> FnLower<'a> {
    fn new(
        name: String,
        id: LirFuncId,
        qualified_name: String,
        program: &'a mut LirProgram,
        interner: Option<&'a Interner>,
        globals_map: Option<&'a HashMap<String, usize>>,
        name_binder_map: &'a HashMap<crate::syntax::Identifier, Vec<CoreBinderId>>,
        binder_func_id_map: &'a HashMap<CoreBinderId, LirFuncId>,
    ) -> Self {
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
            program,
            interner,
            global_var_names: HashMap::new(),
            globals_map,
            name_binder_map,
            direct_func_vars: HashMap::new(),
            binder_func_id_map,
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
                if let Some(binder) = var.binder {
                    // Check env first (locals, parameters, letrec bindings).
                    if let Some(&v) = self.env.get(&binder) {
                        return v;
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
                        }
                        let dst = self.fresh_var();
                        self.emit(LirInstr::Const {
                            dst,
                            value: LirConst::None,
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

                    let mut temp_program = std::mem::replace(&mut *self.program, LirProgram::new());

                    // Assign a synthetic LirFuncId for this letrec function.
                    let synthetic_id = LirFuncId(u32::MAX - temp_program.functions.len() as u32);

                    // Pre-assign function slot so self-recursion works.
                    let func_idx = temp_program.functions.len();
                    temp_program.functions.push(LirFunction {
                        name: format!("letrec_{}_placeholder", func_idx),
                        id: synthetic_id,
                        qualified_name: format!("letrec_{}", synthetic_id.0),
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
                        format!("letrec_{}", synthetic_id.0),
                        &mut temp_program,
                        self.interner,
                        self.globals_map,
                        self.name_binder_map,
                        self.binder_func_id_map,
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
                    let mut temp_program = std::mem::replace(&mut *self.program, LirProgram::new());

                    let synthetic_id = LirFuncId(u32::MAX - temp_program.functions.len() as u32);
                    let func_name = format!("closure_{}", temp_program.functions.len());
                    let mut inner = FnLower::new(
                        func_name,
                        synthetic_id,
                        format!("lambda_{}", synthetic_id.0),
                        &mut temp_program,
                        self.interner,
                        self.globals_map,
                        self.name_binder_map,
                        self.binder_func_id_map,
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

            CoreExpr::App { func, args, .. } | CoreExpr::AetherCall { func, args, .. } => {
                // Check if func is an unbound variable that maps to a known
                // library function with a C runtime implementation.  If so,
                // emit PrimCall directly instead of GetGlobal + Call (which
                // would crash in native mode where the globals table is empty).
                // Check for unbound Var or MemberAccess → library primop.
                let resolved_name = match func.as_ref() {
                    CoreExpr::Var { var, .. } if var.binder.is_none() => {
                        Some(self.resolve_name(var.name))
                    }
                    CoreExpr::MemberAccess { member, .. } => Some(self.resolve_name(*member)),
                    _ => None,
                };
                if let Some(ref name) = resolved_name {
                    if let Some(op) = resolve_library_primop(name, args.len()) {
                        let arg_vars: Vec<LirVar> =
                            args.iter().map(|a| self.lower_expr(a)).collect();
                        let dst = self.fresh_var();
                        self.emit(LirInstr::PrimCall {
                            dst: Some(dst),
                            op,
                            args: arg_vars,
                        });
                        return dst;
                    }
                }

                // Check if func is an ADT constructor applied to arguments.
                // e.g. JumpNext(hit+1, c, Rgt) in an AetherCall.
                if let Some(ref name) = resolved_name {
                    if let Some(&ctor_tag) = self.program.constructor_tags.get(name.as_str()) {
                        let arg_vars: Vec<LirVar> =
                            args.iter().map(|a| self.lower_expr(a)).collect();
                        let dst = self.fresh_var();
                        self.emit(LirInstr::MakeCtor {
                            dst,
                            ctor_tag,
                            ctor_name: Some(name.clone()),
                            fields: arg_vars,
                        });
                        return dst;
                    }
                }

                // Detect known direct calls BEFORE lowering func.
                // If func is a Var with a binder in binder_func_id_map,
                // emit a Direct call without creating a closure.
                let direct_func_id = match func.as_ref() {
                    CoreExpr::Var { var, .. } if var.binder.is_some() => var
                        .binder
                        .and_then(|bid| self.binder_func_id_map.get(&bid).copied()),
                    _ => None,
                };

                let arg_vars: Vec<LirVar> = args.iter().map(|a| self.lower_expr(a)).collect();

                if let Some(func_id) = direct_func_id {
                    // Direct call — no closure needed (GHC-style known call).
                    let cont_idx = self.new_block();
                    let cont_id = BlockId(cont_idx as u32);
                    let result = self.fresh_var();
                    self.func.blocks[cont_idx].params.push(result);

                    // Use a dummy var for func (not needed in direct calls).
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
                    });
                    self.switch_to_block(cont_idx);
                    return result;
                }

                let func_var = self.lower_expr(func);

                // If func_var was produced by a GetGlobal for a known library
                // function, emit a direct PrimCall instead of a closure Call.
                if let Some(name) = self.global_var_names.get(&func_var).cloned() {
                    if let Some(op) = resolve_library_primop(&name, arg_vars.len()) {
                        let dst = self.fresh_var();
                        self.emit(LirInstr::PrimCall {
                            dst: Some(dst),
                            op,
                            args: arg_vars,
                        });
                        return dst;
                    }
                }

                let call_kind = CallKind::Indirect;

                // Emit a Call.  The continuation block receives the result.
                let cont_idx = self.new_block();
                let cont_id = BlockId(cont_idx as u32);
                let result = self.fresh_var();
                self.func.blocks[cont_idx].params.push(result);

                self.set_terminator(LirTerminator::Call {
                    dst: result,
                    func: func_var,
                    args: arg_vars,
                    cont: cont_id,
                    kind: call_kind,
                });

                self.switch_to_block(cont_idx);
                result
            }

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
                // or binder_func_id_map (lazy closure creation).
                if let Some(binders) = self.name_binder_map.get(member) {
                    for &bid in binders {
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

            // ── Effect handlers (Phase 9) ────────────────────────────
            CoreExpr::Perform { .. } | CoreExpr::Handle { .. } => {
                let dst = self.fresh_var();
                self.emit(LirInstr::Const {
                    dst,
                    value: LirConst::None,
                });
                dst
            }

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
        let value = match lit {
            CoreLit::Int(n) => LirConst::Int(*n),
            CoreLit::Float(f) => LirConst::Float(*f),
            CoreLit::Bool(b) => LirConst::Bool(*b),
            CoreLit::String(s) => {
                self.program.intern_string(s.clone());
                LirConst::String(s.clone())
            }
            CoreLit::Unit => LirConst::None,
        };
        self.emit(LirInstr::Const { dst, value });
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

        let reuse_ptr = self.fresh_var();
        // Size: 8 (ADT header: tag + field_count) + 8 * nfields
        let alloc_size = (ADT_HEADER_SIZE as u32) + (field_vars.len() as u32) * 8;
        self.emit(LirInstr::DropReuse {
            dst: reuse_ptr,
            val: token_var,
            size: alloc_size,
        });

        // Branch: if reuse_ptr != 0, reuse; else fresh alloc.
        let is_reusable = self.fresh_var();
        let null = self.fresh_var();
        self.emit(LirInstr::Const {
            dst: null,
            value: LirConst::Tagged(0),
        });
        self.emit(LirInstr::ICmp {
            dst: is_reusable,
            op: CmpOp::Ne,
            a: reuse_ptr,
            b: null,
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
        let tag_val = self.fresh_var();
        self.emit(LirInstr::Const {
            dst: tag_val,
            value: LirConst::Tagged(ctor_tag as i64),
        });
        self.emit(LirInstr::Store {
            ptr: reuse_ptr,
            offset: 0,
            val: tag_val,
        });
        let count_val = self.fresh_var();
        self.emit(LirInstr::Const {
            dst: count_val,
            value: LirConst::Tagged(field_vars.len() as i64),
        });
        self.emit(LirInstr::Store {
            ptr: reuse_ptr,
            offset: 4,
            val: count_val,
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
fn lower_def(
    def: &CoreDef,
    program: &mut LirProgram,
    _top_level_binders: &[CoreBinderId],
    binder_func_map: &HashMap<CoreBinderId, LirFuncId>,
    qualified_names: &HashMap<CoreBinderId, String>,
    name_binder_map: &HashMap<crate::syntax::Identifier, Vec<CoreBinderId>>,
    interner: Option<&Interner>,
    globals_map: Option<&HashMap<String, usize>>,
) -> LirFunction {
    let func_id = LirFuncId(def.binder.id.0);
    let debug_name = format!("def_{}", def.binder.id.0);
    let qualified_name = qualified_names
        .get(&def.binder.id)
        .cloned()
        .unwrap_or_else(|| debug_name.clone());
    let mut ctx = FnLower::new(
        debug_name,
        func_id,
        qualified_name,
        program,
        interner,
        globals_map,
        name_binder_map,
        binder_func_map,
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
        } => {
            let args_str: Vec<String> = args.iter().map(|v| format!("{v}")).collect();
            let kind_str = match kind {
                CallKind::Direct { func_id } => format!(" [direct {}]", func_id),
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
