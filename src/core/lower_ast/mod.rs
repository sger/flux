/// Direct AST → Core IR lowering (runs immediately after HM type inference).
///
/// This is the primary Core IR entry point. By operating directly on the typed
/// AST rather than the intermediate `IrTopLevelItem` representation, it:
///
/// 1. Puts Core IR in the right pipeline position:
///    `HM inference → Core IR → passes → backend lowering`
///
/// 2. Gives every binder access to its HM-inferred type (via `hm_expr_types`),
///    enabling type-driven passes such as:
///    - Emitting `IAdd`/`IDiv` instead of generic `Add`/`Div`
///    - Deciding which values need reference-count operations
///    - Detecting integer vs float arithmetic
///
/// All surface Flux constructs (sugar, n-ary functions, multi-argument calls,
/// pattern matching, effects) are desugared into the ~12-variant `CoreExpr`.
use std::collections::HashMap;

use crate::{
    diagnostics::position::Span,
    syntax::{block::Block, expression::ExprId, program::Program, statement::Statement},
    types::infer_type::InferType,
};

use super::{CoreAlt, CoreBinder, CoreDef, CoreExpr, CoreLit, CoreProgram, CoreTopLevelItem};

mod binder_resolution;
mod expression;
mod pattern;

use binder_resolution::{resolve_program_binders, validate_program_binders};

// ── Public entry point ────────────────────────────────────────────────────────

/// Lower a typed Flux `Program` into a `CoreProgram`.
///
/// `hm_expr_types` maps every `ExprId` (assigned by the parser) to its
/// HM-inferred type. The lowering uses this to guide type-directed decisions.
pub fn lower_program_ast(
    program: &Program,
    hm_expr_types: &HashMap<ExprId, InferType>,
) -> CoreProgram {
    lower_program_ast_with_interner(program, hm_expr_types, None)
}

/// Lower with an optional interner for resolving source-level type annotations
/// on function return types.  When the interner is available, annotated return
/// types (e.g. `-> Int`) can fill in `CoreDef::result_ty` even when HM
/// inference leaves the type unresolved (common for recursive functions).
pub fn lower_program_ast_with_interner(
    program: &Program,
    hm_expr_types: &HashMap<ExprId, InferType>,
    interner: Option<&crate::syntax::interner::Interner>,
) -> CoreProgram {
    let mut lowerer = AstLowerer::new(hm_expr_types, interner);
    let mut defs = Vec::new();
    let mut top_level_items = Vec::new();
    for stmt in &program.statements {
        lowerer.lower_top_level(stmt, &mut defs, &mut top_level_items);
    }
    let mut core = CoreProgram {
        defs,
        top_level_items,
    };
    resolve_program_binders(&mut core);
    assert!(
        validate_program_binders(&core),
        "Core binder resolution invariant failed after AST→Core lowering"
    );
    core
}

// ── Lowerer ───────────────────────────────────────────────────────────────────

pub(super) struct AstLowerer<'a> {
    /// HM-inferred types keyed by ExprId — used for type-directed code selection.
    pub(super) hm_expr_types: &'a HashMap<ExprId, InferType>,
    /// Counter for synthesizing fresh binding names.
    pub(super) fresh: u32,
    pub(super) next_binder_id: u32,
    /// Optional interner for resolving source-level type annotations.
    interner: Option<&'a crate::syntax::interner::Interner>,
}

impl<'a> AstLowerer<'a> {
    fn new(
        hm_expr_types: &'a HashMap<ExprId, InferType>,
        interner: Option<&'a crate::syntax::interner::Interner>,
    ) -> Self {
        Self {
            hm_expr_types,
            fresh: 0,
            next_binder_id: 0,
            interner,
        }
    }

    pub(super) fn bind_name(&mut self, name: crate::syntax::Identifier) -> CoreBinder {
        let id = super::CoreBinderId(self.next_binder_id);
        self.next_binder_id += 1;
        CoreBinder::new(id, name)
    }

    /// Create a binder with a known runtime representation from HM type info.
    pub(super) fn bind_name_with_expr_type(
        &mut self,
        name: crate::syntax::Identifier,
        expr_id: ExprId,
    ) -> CoreBinder {
        let id = super::CoreBinderId(self.next_binder_id);
        self.next_binder_id += 1;
        let rep = self.rep_for_expr(expr_id);
        CoreBinder::with_rep(id, name, rep)
    }

    /// Get the `FluxRep` for an expression from HM type info.
    pub(super) fn rep_for_expr(&self, id: ExprId) -> super::FluxRep {
        self.hm_expr_types
            .get(&id)
            .map(super::FluxRep::from_infer_type)
            .unwrap_or(super::FluxRep::TaggedRep)
    }

    pub(super) fn fresh_binder(&mut self, name: crate::syntax::Identifier) -> CoreBinder {
        self.bind_name(name)
    }

    /// Allocate a fresh synthetic `Identifier` for compiler-generated bindings.
    /// Used by future passes that need to introduce new named temporaries.
    #[allow(dead_code)]
    pub(super) fn fresh_name(
        &mut self,
        interner: &mut crate::syntax::interner::Interner,
    ) -> crate::syntax::Identifier {
        let id = self.fresh;
        self.fresh += 1;
        interner.intern(&format!("$tmp{id}"))
    }

    /// Look up the HM-inferred type for an expression, if available.
    /// Intended for type-driven passes (e.g. emitting `IAdd` vs `Add`).
    #[allow(dead_code)]
    pub(super) fn expr_type(&self, id: ExprId) -> Option<&InferType> {
        self.hm_expr_types.get(&id)
    }

    /// Convert an HM-inferred expression type to a `CoreType`, if available.
    fn infer_core_type(&self, id: ExprId) -> Option<super::CoreType> {
        self.hm_expr_types.get(&id).map(super::CoreType::from_infer)
    }

    /// Convert a source-level type annotation to a `CoreType`, if the interner
    /// is available and the annotation is a simple named type (Int, Float, etc.).
    fn core_type_from_type_expr(
        &self,
        type_expr: &crate::syntax::type_expr::TypeExpr,
    ) -> Option<super::CoreType> {
        let interner = self.interner?;
        match type_expr {
            crate::syntax::type_expr::TypeExpr::Named { name, args, .. } if args.is_empty() => {
                match interner.resolve(*name) {
                    "Int" => Some(super::CoreType::Int),
                    "Float" => Some(super::CoreType::Float),
                    "Bool" => Some(super::CoreType::Bool),
                    "String" => Some(super::CoreType::String),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    // ── Top-level statements ─────────────────────────────────────────────────

    fn lower_top_level(
        &mut self,
        stmt: &Statement,
        out: &mut Vec<CoreDef>,
        top_level_items: &mut Vec<CoreTopLevelItem>,
    ) {
        if let Some(item) = self.lower_decl_item(stmt) {
            top_level_items.push(item);
        }
        match stmt {
            // Named function definition → CoreDef wrapping a curried Lam.
            Statement::Function {
                name,
                parameters,
                body,
                fip,
                span,
                return_type,
                ..
            } => {
                let binder = self.bind_name(*name);
                let params: Vec<_> = parameters.iter().map(|&p| self.bind_name(p)).collect();
                let body_expr = self.lower_block(body);
                // Always wrap in Lam, even for parameterless functions — the
                // Core→IR lowerer uses the Lam marker to distinguish function
                // definitions from value bindings.  We construct Lam directly
                // instead of using CoreExpr::lambda() which elides empty params.
                let expr = CoreExpr::Lam {
                    params,
                    body: Box::new(body_expr),
                    span: *span,
                };
                // For functions, build a Function CoreType from body's last expression type.
                // The function itself doesn't have a single ExprId, but the body block does.
                let mut def = CoreDef::new(binder, expr, true, *span);
                def.fip = *fip;
                // Try to get the type from the last expression in the block.
                if let Some(
                    Statement::Expression { expression, .. }
                    | Statement::Return {
                        value: Some(expression),
                        ..
                    },
                ) = body.statements.last()
                {
                    def.result_ty = self.infer_core_type(expression.expr_id());
                }
                // If HM didn't resolve the result type, fall back to the
                // source return-type annotation.  This is critical for
                // recursive functions where HM may leave the return type as
                // an unresolved variable.
                if (def.result_ty.is_none() || matches!(def.result_ty, Some(super::CoreType::Any)))
                    && let Some(rt) = return_type
                    && let Some(annotated_ty) = self.core_type_from_type_expr(rt)
                {
                    def.result_ty = Some(annotated_ty);
                }
                out.push(def);
            }

            // Value binding → CoreDef for the RHS expression.
            Statement::Let {
                name, value, span, ..
            } => {
                let result_ty = self.infer_core_type(value.expr_id());
                let binder = self.bind_name_with_expr_type(*name, value.expr_id());
                let mut def = CoreDef::new(binder, self.lower_expr(value), false, *span);
                def.result_ty = result_ty;
                out.push(def);
            }

            // Assignment (mutable rebind) → treat as a new CoreDef.
            Statement::Assign { name, value, span } => {
                let result_ty = self.infer_core_type(value.expr_id());
                let binder = self.bind_name_with_expr_type(*name, value.expr_id());
                let mut def = CoreDef::new(binder, self.lower_expr(value), false, *span);
                def.result_ty = result_ty;
                out.push(def);
            }

            // Expression statement → anonymous CoreDef (evaluated for effects).
            Statement::Expression {
                expression, span, ..
            } => {
                let result_ty = self.infer_core_type(expression.expr_id());
                let mut def = CoreDef::new_anonymous(
                    self.bind_name(crate::syntax::symbol::Symbol::new(0)),
                    self.lower_expr(expression),
                    false,
                    *span,
                );
                def.result_ty = result_ty;
                out.push(def);
            }

            // Destructuring let → synthetic tmp var + Case alt for the pattern.
            Statement::LetDestructure {
                pattern,
                value,
                span,
            } => {
                let rhs = self.lower_expr(value);
                let core_pat = self.lower_pattern(pattern);
                // Wrap: let $destructure = value in case $destructure of { pat -> () }
                // At the top level we emit this as one def per bound variable by
                // expanding the pattern into field accesses where possible.
                self.expand_destructure_top_level(core_pat, rhs, *span, out);
            }

            // Return at top level is unusual but syntactically valid in some contexts.
            Statement::Return { value, span } => {
                if let Some(val) = value {
                    out.push(CoreDef::new_anonymous(
                        self.bind_name(crate::syntax::symbol::Symbol::new(0)),
                        self.lower_expr(val),
                        false,
                        *span,
                    ));
                }
            }

            // Declarations that don't produce runtime values — skip.
            Statement::Module { body, .. } => {
                self.lower_functions_in_module(&body.statements, out);
            }

            Statement::Import { .. } | Statement::Data { .. } | Statement::EffectDecl { .. } => {}
        }
    }

    fn lower_functions_in_module(&mut self, stmts: &[Statement], out: &mut Vec<CoreDef>) {
        for stmt in stmts {
            match stmt {
                Statement::Function {
                    name,
                    parameters,
                    body,
                    span,
                    ..
                } => {
                    let binder = self.bind_name(*name);
                    let params: Vec<_> = parameters.iter().map(|&p| self.bind_name(p)).collect();
                    let body_expr = self.lower_block(body);
                    let expr = CoreExpr::Lam {
                        params,
                        body: Box::new(body_expr),
                        span: *span,
                    };
                    out.push(CoreDef::new(binder, expr, true, *span));
                }
                Statement::Module { body, .. } => {
                    self.lower_functions_in_module(&body.statements, out);
                }
                _ => {}
            }
        }
    }

    fn lower_decl_item(&mut self, stmt: &Statement) -> Option<CoreTopLevelItem> {
        match stmt {
            Statement::Function {
                is_public,
                name,
                type_params,
                parameters,
                parameter_types,
                return_type,
                effects,
                body: _,
                span,
                fip: _,
            } => Some(CoreTopLevelItem::Function {
                is_public: *is_public,
                name: *name,
                type_params: type_params.clone(),
                parameters: parameters.clone(),
                parameter_types: parameter_types.clone(),
                return_type: return_type.clone(),
                effects: effects.clone(),
                span: *span,
            }),
            Statement::Module { name, body, span } => Some(CoreTopLevelItem::Module {
                name: *name,
                body: body
                    .statements
                    .iter()
                    .filter_map(|item| self.lower_decl_item(item))
                    .collect(),
                span: *span,
            }),
            Statement::Import {
                name,
                alias,
                except,
                exposing,
                span,
            } => Some(CoreTopLevelItem::Import {
                name: *name,
                alias: *alias,
                except: except.clone(),
                exposing: exposing.clone(),
                span: *span,
            }),
            Statement::Data {
                name,
                type_params,
                variants,
                span,
            } => Some(CoreTopLevelItem::Data {
                name: *name,
                type_params: type_params.clone(),
                variants: variants.clone(),
                span: *span,
            }),
            Statement::EffectDecl { name, ops, span } => Some(CoreTopLevelItem::EffectDecl {
                name: *name,
                ops: ops.clone(),
                span: *span,
            }),
            Statement::Let { .. }
            | Statement::LetDestructure { .. }
            | Statement::Return { .. }
            | Statement::Expression { .. }
            | Statement::Assign { .. } => None,
        }
    }

    // ── Block lowering ───────────────────────────────────────────────────────

    /// Lower a `Block` (sequence of statements) into a single `CoreExpr`.
    ///
    /// Each statement is desugared:
    /// - `let x = e; rest` → `Let(x, e, rest)`
    /// - `fn f(p) { b }; rest` → `LetRec(f, Lam(p, b), rest)`
    /// - `e; rest` → `Let($_, e, rest)`   (sequence — result is discarded)
    /// - last expression → the block's return value
    pub(super) fn lower_block(&mut self, block: &Block) -> CoreExpr {
        self.lower_stmts(&block.statements, block.span)
    }

    fn lower_stmts(&mut self, stmts: &[Statement], span: Span) -> CoreExpr {
        // Find where the "value" of the block comes from.
        // It's the last statement if it's a non-semicolon expression,
        // otherwise the block returns unit.
        match stmts {
            [] => CoreExpr::Lit(CoreLit::Unit, span),

            [single] => self.lower_stmt_as_expr(single, span),

            [rest @ .., last] => {
                let tail = self.lower_stmts(std::slice::from_ref(last), span);
                self.prepend_stmt(&rest[rest.len() - 1..], rest, tail, span)
            }
        }
    }

    /// Lower the full statement slice by prepending one statement at a time.
    fn prepend_stmts(&mut self, stmts: &[Statement], body: CoreExpr, span: Span) -> CoreExpr {
        stmts
            .iter()
            .rev()
            .fold(body, |acc, stmt| self.prepend_one_stmt(stmt, acc, span))
    }

    fn prepend_stmt(
        &mut self,
        _one: &[Statement],
        all_but_last: &[Statement],
        tail: CoreExpr,
        span: Span,
    ) -> CoreExpr {
        self.prepend_stmts(all_but_last, tail, span)
    }

    /// Wrap `tail` with the binding/effect introduced by a single statement.
    fn prepend_one_stmt(&mut self, stmt: &Statement, tail: CoreExpr, _span: Span) -> CoreExpr {
        match stmt {
            Statement::Let {
                name,
                value,
                span: s,
                ..
            } => CoreExpr::Let {
                var: self.bind_name_with_expr_type(*name, value.expr_id()),
                rhs: Box::new(self.lower_expr(value)),
                body: Box::new(tail),
                span: *s,
            },

            Statement::Function { .. } => {
                let Statement::Function {
                    name,
                    parameters,
                    body,
                    span: s,
                    ..
                } = stmt
                else {
                    unreachable!();
                };
                let binder = self.bind_name(*name);
                let params: Vec<_> = parameters.iter().map(|&p| self.bind_name(p)).collect();
                let body_expr = self.lower_block(body);
                CoreExpr::LetRec {
                    var: binder,
                    rhs: Box::new(CoreExpr::Lam {
                        params,
                        body: Box::new(body_expr),
                        span: *s,
                    }),
                    body: Box::new(tail),
                    span: *s,
                }
            }

            Statement::Assign {
                name,
                value,
                span: s,
            } => CoreExpr::Let {
                var: self.bind_name_with_expr_type(*name, value.expr_id()),
                rhs: Box::new(self.lower_expr(value)),
                body: Box::new(tail),
                span: *s,
            },

            Statement::LetDestructure {
                pattern,
                value,
                span: s,
            } => {
                // Bind the scrutinee to a tmp var and use Case to extract fields.
                let rhs = self.lower_expr(value);
                let core_pat = self.lower_pattern(pattern);
                // Build: Let($tmp, rhs, Case($tmp, [core_pat → tail]))
                // We use a special sentinel name for the tmp var.
                let tmp_name = crate::syntax::symbol::Symbol::new(self.fresh + 1_000_000);
                self.fresh += 1;
                let tmp_binder = self.fresh_binder(tmp_name);
                let tmp_var = CoreExpr::bound_var(tmp_binder, *s);
                let alt = CoreAlt {
                    pat: core_pat,
                    guard: None,
                    rhs: tail,
                    span: *s,
                };
                CoreExpr::Let {
                    var: tmp_binder,
                    rhs: Box::new(rhs),
                    body: Box::new(CoreExpr::Case {
                        scrutinee: Box::new(tmp_var),
                        alts: vec![alt],
                        span: *s,
                    }),
                    span: *s,
                }
            }

            // Expression statement: evaluate for its effect, discard result.
            Statement::Expression {
                expression,
                has_semicolon: true,
                span: s,
            } => {
                let tmp_name = crate::syntax::symbol::Symbol::new(self.fresh + 2_000_000);
                self.fresh += 1;
                CoreExpr::Let {
                    var: self.fresh_binder(tmp_name),
                    rhs: Box::new(self.lower_expr(expression)),
                    body: Box::new(tail),
                    span: *s,
                }
            }

            // Expression without semicolon in a non-last position is unusual.
            // Treat as sequencing (result discarded).
            Statement::Expression {
                expression,
                has_semicolon: false,
                span: s,
            } => {
                let tmp_name = crate::syntax::symbol::Symbol::new(self.fresh + 2_000_000);
                self.fresh += 1;
                CoreExpr::Let {
                    var: self.fresh_binder(tmp_name),
                    rhs: Box::new(self.lower_expr(expression)),
                    body: Box::new(tail),
                    span: *s,
                }
            }

            Statement::Return { value, span: s } => {
                let ret_value = match value {
                    Some(v) => self.lower_expr(v),
                    None => CoreExpr::Lit(CoreLit::Unit, *s),
                };
                CoreExpr::Return {
                    value: Box::new(ret_value),
                    span: *s,
                }
            }

            // Declarations don't contribute to block value.
            Statement::Import { .. }
            | Statement::Data { .. }
            | Statement::EffectDecl { .. }
            | Statement::Module { .. } => tail,
        }
    }

    /// Lower the last (or only) statement in a block as an expression.
    fn lower_stmt_as_expr(&mut self, stmt: &Statement, span: Span) -> CoreExpr {
        match stmt {
            Statement::Expression {
                expression,
                has_semicolon: false,
                ..
            } => self.lower_expr(expression),
            // Semicolon-terminated expression discards its value — evaluate
            // for effect, then return unit.
            Statement::Expression {
                expression,
                has_semicolon: true,
                span: s,
            } => {
                let tmp = crate::syntax::symbol::Symbol::new(self.fresh + 2_000_000);
                self.fresh += 1;
                CoreExpr::Let {
                    var: self.fresh_binder(tmp),
                    rhs: Box::new(self.lower_expr(expression)),
                    body: Box::new(CoreExpr::Lit(CoreLit::Unit, *s)),
                    span: *s,
                }
            }
            Statement::Return { value, span: s } => match value {
                Some(v) => CoreExpr::Return {
                    value: Box::new(self.lower_expr(v)),
                    span: *s,
                },
                None => CoreExpr::Return {
                    value: Box::new(CoreExpr::Lit(CoreLit::Unit, *s)),
                    span: *s,
                },
            },
            // A `let` or `fn` as the last statement returns unit.
            other => {
                let unit = CoreExpr::Lit(CoreLit::Unit, span);
                self.prepend_one_stmt(other, unit, span)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ast::type_infer::{InferProgramConfig, infer_program},
        core::{CoreExpr, CorePrimOp},
        syntax::{interner::Interner, lexer::Lexer, parser::Parser},
    };

    fn parse_and_infer(
        src: &str,
    ) -> (
        crate::syntax::program::Program,
        HashMap<ExprId, InferType>,
        Interner,
    ) {
        let mut parser = Parser::new(Lexer::new(src));
        let program = parser.parse_program();
        let mut interner = parser.take_interner();
        let flow_sym = interner.intern("Flow");
        let hm = infer_program(
            &program,
            &interner,
            InferProgramConfig {
                file_path: None,
                preloaded_base_schemes: HashMap::new(),
                preloaded_module_member_schemes: HashMap::new(),
                known_flow_names: std::collections::HashSet::new(),
                flow_module_symbol: flow_sym,
                preloaded_effect_op_signatures: HashMap::new(),
            },
        );
        let types = hm.expr_types;
        (program, types, interner)
    }

    fn collect_primops(
        program: &crate::syntax::program::Program,
        types: &HashMap<ExprId, InferType>,
    ) -> Vec<CorePrimOp> {
        let core = lower_program_ast(program, types);
        let mut ops = Vec::new();
        for def in &core.defs {
            collect_ops_in_expr(&def.expr, &mut ops);
        }
        ops
    }

    fn collect_ops_in_expr(expr: &CoreExpr, out: &mut Vec<CorePrimOp>) {
        match expr {
            CoreExpr::PrimOp { op, args, .. } => {
                out.push(*op);
                for a in args {
                    collect_ops_in_expr(a, out);
                }
            }
            CoreExpr::Lam { body, .. } => collect_ops_in_expr(body, out),
            CoreExpr::App { func, args, .. } => {
                collect_ops_in_expr(func, out);
                for a in args {
                    collect_ops_in_expr(a, out);
                }
            }
            CoreExpr::Return { value, .. } => collect_ops_in_expr(value, out),
            CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
                collect_ops_in_expr(rhs, out);
                collect_ops_in_expr(body, out);
            }
            CoreExpr::Case {
                scrutinee, alts, ..
            } => {
                collect_ops_in_expr(scrutinee, out);
                for alt in alts {
                    collect_ops_in_expr(&alt.rhs, out);
                }
            }
            _ => {}
        }
    }

    fn collect_var_refs<'a>(expr: &'a CoreExpr, out: &mut Vec<&'a crate::core::CoreVarRef>) {
        match expr {
            CoreExpr::Var { var, .. } => out.push(var),
            CoreExpr::Lam { body, .. } => collect_var_refs(body, out),
            CoreExpr::App { func, args, .. } | CoreExpr::AetherCall { func, args, .. } => {
                collect_var_refs(func, out);
                for arg in args {
                    collect_var_refs(arg, out);
                }
            }
            CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
                collect_var_refs(rhs, out);
                collect_var_refs(body, out);
            }
            CoreExpr::Case {
                scrutinee, alts, ..
            } => {
                collect_var_refs(scrutinee, out);
                for alt in alts {
                    if let Some(guard) = &alt.guard {
                        collect_var_refs(guard, out);
                    }
                    collect_var_refs(&alt.rhs, out);
                }
            }
            CoreExpr::Con { fields, .. } => {
                for field in fields {
                    collect_var_refs(field, out);
                }
            }
            CoreExpr::Return { value, .. } => collect_var_refs(value, out),
            CoreExpr::PrimOp { args, .. } | CoreExpr::Perform { args, .. } => {
                for arg in args {
                    collect_var_refs(arg, out);
                }
            }
            CoreExpr::Handle { body, handlers, .. } => {
                collect_var_refs(body, out);
                for handler in handlers {
                    collect_var_refs(&handler.body, out);
                }
            }
            CoreExpr::Lit(_, _) => {}
            CoreExpr::Dup { var, body, .. } | CoreExpr::Drop { var, body, .. } => {
                out.push(var);
                collect_var_refs(body, out);
            }
            CoreExpr::Reuse { token, fields, .. } => {
                out.push(token);
                for field in fields {
                    collect_var_refs(field, out);
                }
            }
            CoreExpr::DropSpecialized {
                scrutinee,
                unique_body,
                shared_body,
                ..
            } => {
                out.push(scrutinee);
                collect_var_refs(unique_body, out);
                collect_var_refs(shared_body, out);
            }
            CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
                collect_var_refs(object, out);
            }
        }
    }

    #[test]
    fn integer_add_emits_iadd() {
        let src = "fn add(x: Int, y: Int) -> Int { x + y }";
        let (prog, types, _) = parse_and_infer(src);
        let ops = collect_primops(&prog, &types);
        assert!(
            ops.contains(&CorePrimOp::IAdd),
            "expected IAdd for Int addition, got: {:?}",
            ops
        );
        assert!(
            !ops.contains(&CorePrimOp::Add),
            "should not emit generic Add for typed Int addition"
        );
    }

    #[test]
    fn integer_mul_emits_imul() {
        let src = "fn mul(x: Int, y: Int) -> Int { x * y }";
        let (prog, types, _) = parse_and_infer(src);
        let ops = collect_primops(&prog, &types);
        assert!(
            ops.contains(&CorePrimOp::IMul),
            "expected IMul, got: {:?}",
            ops
        );
    }

    #[test]
    fn float_add_emits_fadd() {
        let src = "fn fadd(x: Float, y: Float) -> Float { x + y }";
        let (prog, types, _) = parse_and_infer(src);
        let ops = collect_primops(&prog, &types);
        assert!(
            ops.contains(&CorePrimOp::FAdd),
            "expected FAdd, got: {:?}",
            ops
        );
    }

    #[test]
    fn generic_add_stays_generic_without_type_info() {
        // Unannotated polymorphic add — HM resolves to Int here, so we get IAdd.
        // This test verifies that untyped (Any) additions stay as generic Add.
        // We use a String + String expression so HM infers String, not Int/Float,
        // which means no typed variant should be emitted — just generic Add.
        let src = r#"fn cat(a: String, b: String) -> String { a + b }"#;
        let (prog, types, _) = parse_and_infer(src);
        let ops = collect_primops(&prog, &types);
        // String + String should emit generic Add (no IAdd/FAdd)
        assert!(
            ops.contains(&CorePrimOp::Add),
            "expected generic Add for String addition, got: {:?}",
            ops
        );
        assert!(
            !ops.contains(&CorePrimOp::IAdd),
            "should not emit IAdd for String addition"
        );
        assert!(
            !ops.contains(&CorePrimOp::FAdd),
            "should not emit FAdd for String addition"
        );
    }

    #[test]
    fn lower_ast_resolves_shadowed_let_to_inner_binder() {
        let src = r#"
fn main() {
    let x = 1;
    let x = 2;
    x
}
"#;
        let (prog, types, mut interner) = parse_and_infer(src);
        let main_name = interner.intern("main");
        let core = lower_program_ast(&prog, &types);
        let main = core.defs.iter().find(|def| def.name == main_name).unwrap();
        let mut refs = Vec::new();
        collect_var_refs(&main.expr, &mut refs);
        let x_refs: Vec<_> = refs
            .into_iter()
            .filter(|var| interner.try_resolve(var.name) == Some("x"))
            .collect();
        let last = x_refs.last().unwrap();
        assert!(
            last.binder.is_some(),
            "expected lexical x reference to be resolved"
        );
    }

    #[test]
    fn lower_ast_leaves_external_name_unbound() {
        let src = r#"
fn main() {
    print("ok")
}
"#;
        let (prog, types, mut interner) = parse_and_infer(src);
        let main_name = interner.intern("main");
        let core = lower_program_ast(&prog, &types);
        let main = core.defs.iter().find(|def| def.name == main_name).unwrap();
        let mut refs = Vec::new();
        collect_var_refs(&main.expr, &mut refs);
        let print_ref = refs
            .into_iter()
            .find(|var| interner.try_resolve(var.name) == Some("print"))
            .expect("expected print reference");
        assert_eq!(
            print_ref.binder, None,
            "external name should stay unresolved in Core"
        );
    }

    #[test]
    fn lower_ast_preserves_module_data_and_effect_declarations() {
        let src = r#"
module Demo {
    fn value() { 1 }
}

data MaybeInt {
    SomeInt(Int)
    NoneInt
}

effect Console {
    print: String -> Unit
}
"#;
        let (prog, types, _) = parse_and_infer(src);
        let core = lower_program_ast(&prog, &types);

        assert!(matches!(
            core.top_level_items.first(),
            Some(CoreTopLevelItem::Module { .. })
        ));
        assert!(matches!(
            core.top_level_items.get(1),
            Some(CoreTopLevelItem::Data { .. })
        ));
        assert!(matches!(
            core.top_level_items.get(2),
            Some(CoreTopLevelItem::EffectDecl { .. })
        ));

        let module_body = match &core.top_level_items[0] {
            CoreTopLevelItem::Module { body, .. } => body,
            other => panic!("expected module item, got {other:?}"),
        };
        assert!(matches!(
            module_body.first(),
            Some(CoreTopLevelItem::Function { .. })
        ));
    }

    #[test]
    fn lower_ast_does_not_drop_symbol_zero_named_global_bindings() {
        let src = r#"
let f = len
f("flux")
"#;
        let (prog, types, mut interner) = parse_and_infer(src);
        let f_name = interner.intern("f");
        assert_eq!(
            f_name.as_u32(),
            0,
            "test requires first identifier to be Symbol(0)"
        );

        let core = lower_program_ast(&prog, &types);
        let f_def = core
            .defs
            .iter()
            .find(|def| def.name == f_name && !def.is_anonymous())
            .expect("expected named top-level def for f");
        let anon_def = core
            .defs
            .iter()
            .find(|def| def.is_anonymous())
            .expect("expected anonymous top-level expression def");

        let mut refs = Vec::new();
        collect_var_refs(&anon_def.expr, &mut refs);
        let f_ref = refs
            .into_iter()
            .find(|var| var.name == f_name)
            .expect("expected call through top-level f binding");
        assert_eq!(
            f_ref.binder,
            Some(f_def.binder.id),
            "top-level Symbol(0) binding should still resolve lexically"
        );
    }
}
