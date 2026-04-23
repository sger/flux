use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};

use crate::ast::free_vars::collect_free_vars_in_function_body;
use crate::cfg::{IrFunction, IrProgram};
use crate::{
    ast::type_infer::display_infer_type,
    bytecode::{debug_info::FunctionDebugInfo, op_code::OpCode},
    compiler::{
        Compiler, contracts::convert_type_expr_checked, module_constants::compile_module_constants,
        suggestions::suggest_effect_name, symbol_scope::SymbolScope,
    },
    diagnostics::{
        DUPLICATE_PARAMETER, Diagnostic, DiagnosticBuilder, DiagnosticPhase, ICE_SYMBOL_SCOPE_LET,
        IMPORT_SCOPE, INVALID_MODULE_CONTENT, INVALID_MODULE_NAME, MODULE_NAME_CLASH, MODULE_SCOPE,
        compiler_errors::{fun_return_annotation_mismatch, let_annotation_type_mismatch},
        position::{Position, Span},
        types::ErrorType,
    },
    runtime::{compiled_function::CompiledFunction, value::Value},
    syntax::{
        block::Block,
        data_variant::DataVariant,
        effect_expr::EffectExpr,
        expression::{Expression, Pattern},
        module_graph::{import_binding_name, is_valid_module_name, module_binding_name},
        statement::Statement,
        symbol::Symbol,
        type_expr::TypeExpr,
    },
    types::{infer_type::InferType, type_env::TypeEnv, unify::unify},
};

use super::hm_expr_typer::HmExprTypeResult;

type CompileResult<T> = Result<T, Box<Diagnostic>>;

/// A range [start, end) of statements forming a mutual recursion group.
struct MutualRecRange {
    start: usize,
    end: usize,
}

impl Compiler {
    pub(super) fn find_ir_function_by_symbol<'a>(
        &self,
        ir_program: &'a IrProgram,
        symbol: Symbol,
    ) -> Option<&'a IrFunction> {
        ir_program.functions().iter().find(|function| {
            self.ir_function_symbols
                .get(&function.id)
                .is_some_and(|mapped| *mapped == symbol)
        })
    }

    /// Quick pre-codegen validation to detect semantic errors that the CFG
    /// path can't catch. Returns `true` if errors are present — caller should
    /// skip CFG and fall through to the AST path for proper error reporting.
    ///
    /// Checks: E001 duplicate let, E002/E003 assignment, E056 wrong arg count,
    /// E400/E419-E422 effect row violations.
    fn quick_validate_function_body(
        &mut self,
        body: &Block,
        parameters: &[crate::syntax::symbol::Symbol],
        declared_effects: &[EffectExpr],
        param_effect_rows: &HashMap<Symbol, super::effect_rows::EffectRow>,
    ) -> bool {
        Self::block_has_semantic_errors(body, parameters)
            || self.block_has_call_arity_error(body)
            || self.block_has_effect_row_error(body, declared_effects, param_effect_rows)
            || self.block_has_typed_let_error(body)
            || self.block_has_prefix_type_error(body)
            || self.block_has_effectful_base_ref(body, declared_effects)
            || self.block_has_index_type_error(body)
            || self.block_has_undefined_identifier(body, parameters)
    }

    /// Check if any index expression uses a non-Int index (E300).
    fn block_has_index_type_error(&self, body: &Block) -> bool {
        body.statements.iter().any(|s| match s {
            Statement::Expression { expression, .. }
            | Statement::Let {
                value: expression, ..
            } => self.expr_has_index_type_error(expression),
            _ => false,
        })
    }

    fn expr_has_index_type_error(&self, expr: &Expression) -> bool {
        match expr {
            Expression::Index { left, index, .. } => {
                use crate::types::type_constructor::TypeConstructor;
                // Check index type is Int
                if let super::hm_expr_typer::HmExprTypeResult::Known(idx_ty) =
                    self.hm_expr_type_strict_path(index)
                    && !matches!(idx_ty, InferType::Con(TypeConstructor::Int))
                {
                    return true;
                }
                // Check left type is indexable
                if let super::hm_expr_typer::HmExprTypeResult::Known(left_ty) =
                    self.hm_expr_type_strict_path(left)
                {
                    !matches!(
                        left_ty,
                        InferType::App(
                            TypeConstructor::Array | TypeConstructor::List | TypeConstructor::Map,
                            _,
                        ) | InferType::Tuple(_)
                            | InferType::Con(TypeConstructor::String)
                    )
                } else {
                    false
                }
            }
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                self.expr_has_index_type_error(function)
                    || arguments.iter().any(|a| self.expr_has_index_type_error(a))
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                self.expr_has_index_type_error(condition)
                    || self.block_has_index_type_error(consequence)
                    || alternative
                        .as_ref()
                        .is_some_and(|b| self.block_has_index_type_error(b))
            }
            Expression::DoBlock { block, .. } => self.block_has_index_type_error(block),
            Expression::Match { arms, .. } => arms
                .iter()
                .any(|arm| self.expr_has_index_type_error(&arm.body)),
            _ => false,
        }
    }

    /// Check if any identifier in the body references a base function with
    /// effects not declared by this function. Catches aliased effectful calls
    /// like `let p = print; p("hi")` in a pure function.
    fn block_has_effectful_base_ref(
        &mut self,
        body: &Block,
        declared_effects: &[EffectExpr],
    ) -> bool {
        body.statements.iter().any(|s| match s {
            Statement::Let { value, .. }
            | Statement::Expression {
                expression: value, ..
            } => self.expr_has_effectful_base_ref(value, declared_effects),
            _ => false,
        })
    }

    fn expr_has_effectful_base_ref(
        &mut self,
        expr: &Expression,
        declared_effects: &[EffectExpr],
    ) -> bool {
        match expr {
            Expression::Identifier { name, .. } => {
                let name_str = self.sym(*name);
                if let Some(required_effect) = self.required_effect_for_base_name(name_str) {
                    let required_sym = self.interner.intern(required_effect);
                    !Self::is_effect_in_declared(required_sym, declared_effects)
                } else {
                    false
                }
            }
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                self.expr_has_effectful_base_ref(function, declared_effects)
                    || arguments
                        .iter()
                        .any(|a| self.expr_has_effectful_base_ref(a, declared_effects))
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                self.expr_has_effectful_base_ref(condition, declared_effects)
                    || self.block_has_effectful_base_ref(consequence, declared_effects)
                    || alternative
                        .as_ref()
                        .is_some_and(|b| self.block_has_effectful_base_ref(b, declared_effects))
            }
            Expression::DoBlock { block, .. } => {
                self.block_has_effectful_base_ref(block, declared_effects)
            }
            Expression::Match {
                scrutinee, arms, ..
            } => {
                self.expr_has_effectful_base_ref(scrutinee, declared_effects)
                    || arms
                        .iter()
                        .any(|arm| self.expr_has_effectful_base_ref(&arm.body, declared_effects))
            }
            _ => false,
        }
    }

    /// Check if any prefix `-` expression has a non-numeric operand (E300).
    fn block_has_prefix_type_error(&self, body: &Block) -> bool {
        body.statements.iter().any(|s| match s {
            Statement::Expression { expression, .. }
            | Statement::Let {
                value: expression, ..
            } => self.expr_has_prefix_type_error(expression),
            _ => false,
        })
    }

    fn expr_has_prefix_type_error(&self, expr: &Expression) -> bool {
        match expr {
            Expression::Prefix {
                operator, right, ..
            } if operator == "-" => {
                if let super::hm_expr_typer::HmExprTypeResult::Known(ty) =
                    self.hm_expr_type_strict_path(right)
                {
                    !matches!(
                        ty,
                        InferType::Con(
                            crate::types::type_constructor::TypeConstructor::Int
                                | crate::types::type_constructor::TypeConstructor::Float
                        )
                    )
                } else {
                    false
                }
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                self.expr_has_prefix_type_error(condition)
                    || self.block_has_prefix_type_error(consequence)
                    || alternative
                        .as_ref()
                        .is_some_and(|b| self.block_has_prefix_type_error(b))
            }
            Expression::DoBlock { block, .. } => self.block_has_prefix_type_error(block),
            Expression::Match { arms, .. } => arms
                .iter()
                .any(|arm| self.expr_has_prefix_type_error(&arm.body)),
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                self.expr_has_prefix_type_error(function)
                    || arguments.iter().any(|a| self.expr_has_prefix_type_error(a))
            }
            _ => false,
        }
    }

    /// Check if any typed let binding carries an unsound annotation-vs-initializer
    /// relationship that the CFG path cannot diagnose:
    ///
    /// 1. **E300 type mismatch** — annotation resolves to a concrete type, the
    ///    initializer's HM type is known and concrete, and they fail to unify.
    /// 2. **E425 unresolved boundary** — strict mode is on and either the
    ///    annotation itself cannot be resolved to a concrete type (unknown
    ///    type constructor), or the initializer's HM type is still unresolved.
    ///
    /// Returns `true` when the function body should fall back to the AST path
    /// so the specialized diagnostic can be rendered. Well-typed annotated
    /// lets return `false` and proceed through CFG (Proposal 0167 Part 4).
    fn block_has_typed_let_error(&self, body: &Block) -> bool {
        body.statements.iter().any(|s| {
            let Statement::Let {
                type_annotation: Some(annotation),
                value,
                ..
            } = s
            else {
                return false;
            };
            match crate::types::type_env::TypeEnv::infer_type_from_type_expr(
                annotation,
                &Default::default(),
                &self.interner,
            ) {
                Some(expected) => {
                    if self
                        .known_concrete_expr_type_mismatch(&expected, value)
                        .is_some()
                    {
                        return true;
                    }
                    if self.strict_mode
                        && matches!(
                            self.hm_expr_type_strict_path(value),
                            super::hm_expr_typer::HmExprTypeResult::Unresolved(_)
                        )
                    {
                        return true;
                    }
                    false
                }
                // Annotation could not be resolved (unknown type constructor).
                // In strict mode this is E425 on the AST path; let it fall back.
                None => self.strict_mode,
            }
        })
    }

    /// Check if any call expression has wrong argument count vs HM type (E056).
    fn block_has_call_arity_error(&self, body: &Block) -> bool {
        body.statements.iter().any(|s| match s {
            Statement::Expression { expression, .. }
            | Statement::Let {
                value: expression, ..
            } => self.expr_has_call_arity_error(expression),
            _ => false,
        })
    }

    fn expr_has_call_arity_error(&self, expr: &Expression) -> bool {
        match expr {
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                // Use HM type to check expected arity
                if let super::hm_expr_typer::HmExprTypeResult::Known(InferType::Fun(params, _, _)) =
                    self.hm_expr_type_strict_path(function)
                    && self.visible_call_arity(function, params.len()) != arguments.len()
                {
                    return true;
                }
                // Recurse into subexpressions
                self.expr_has_call_arity_error(function)
                    || arguments.iter().any(|a| self.expr_has_call_arity_error(a))
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                self.expr_has_call_arity_error(condition)
                    || self.block_has_call_arity_error(consequence)
                    || alternative
                        .as_ref()
                        .is_some_and(|b| self.block_has_call_arity_error(b))
            }
            Expression::DoBlock { block, .. } => self.block_has_call_arity_error(block),
            Expression::Match { arms, .. } => arms
                .iter()
                .any(|arm| self.expr_has_call_arity_error(&arm.body)),
            _ => false,
        }
    }

    /// HM types may include hidden dictionary/evidence parameters for
    /// constrained identifiers. For user-facing AST validation we should
    /// compare against the visible source arity, not the elaborated one.
    fn visible_call_arity(&self, function: &Expression, raw_arity: usize) -> usize {
        let Expression::Identifier { name, .. } = function else {
            return raw_arity;
        };
        let hidden_dicts = self
            .type_env
            .lookup(*name)
            .map(|scheme| scheme.constraints.len())
            .unwrap_or(0);
        raw_arity.saturating_sub(hidden_dicts)
    }

    fn block_contains_constrained_calls(&self, body: &Block) -> bool {
        body.statements.iter().any(|statement| match statement {
            Statement::Expression { expression, .. }
            | Statement::Let {
                value: expression, ..
            } => self.expr_contains_constrained_calls(expression),
            _ => false,
        })
    }

    fn expr_contains_constrained_calls(&self, expr: &Expression) -> bool {
        match expr {
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                let callee_is_constrained = matches!(
                    function.as_ref(),
                    Expression::Identifier { name, .. }
                        if self
                            .type_env
                            .lookup(*name)
                            .is_some_and(|scheme| !scheme.constraints.is_empty())
                );
                callee_is_constrained
                    || self.expr_contains_constrained_calls(function)
                    || arguments
                        .iter()
                        .any(|argument| self.expr_contains_constrained_calls(argument))
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                self.expr_contains_constrained_calls(condition)
                    || self.block_contains_constrained_calls(consequence)
                    || alternative
                        .as_ref()
                        .is_some_and(|block| self.block_contains_constrained_calls(block))
            }
            Expression::DoBlock { block, .. } => self.block_contains_constrained_calls(block),
            Expression::Match {
                scrutinee, arms, ..
            } => {
                self.expr_contains_constrained_calls(scrutinee)
                    || arms
                        .iter()
                        .any(|arm| self.expr_contains_constrained_calls(&arm.body))
            }
            Expression::Function { body, .. } => self.block_contains_constrained_calls(body),
            _ => false,
        }
    }

    /// Check if any statement in the block (recursively) contains errors
    /// that only the AST path catches: assignments (E002/E003) or duplicate
    /// let bindings in the same scope (E001).
    fn block_has_semantic_errors(body: &Block, parameters: &[Symbol]) -> bool {
        // Check for assignments (Flux is purely immutable)
        let has_assign = body
            .statements
            .iter()
            .any(|s| matches!(s, Statement::Assign { .. }));
        if has_assign {
            return true;
        }
        // Check for duplicate let bindings in the same scope (including params)
        let mut seen: std::collections::HashSet<Symbol> = parameters.iter().copied().collect();
        for stmt in &body.statements {
            if let Statement::Let { name, .. } = stmt
                && !seen.insert(*name)
            {
                return true;
            }
        }
        // Recurse into nested blocks and let values
        body.statements.iter().any(|s| match s {
            Statement::Expression { expression, .. }
            | Statement::Let {
                value: expression, ..
            } => Self::expr_has_semantic_errors(expression),
            _ => false,
        })
    }

    fn expr_has_semantic_errors(expr: &Expression) -> bool {
        match expr {
            Expression::If {
                consequence,
                alternative,
                ..
            } => {
                Self::block_has_semantic_errors(consequence, &[])
                    || alternative
                        .as_ref()
                        .is_some_and(|b| Self::block_has_semantic_errors(b, &[]))
            }
            Expression::DoBlock { block, .. } => Self::block_has_semantic_errors(block, &[]),
            Expression::Match { arms, .. } => arms
                .iter()
                .any(|arm| Self::expr_has_semantic_errors(&arm.body)),
            Expression::Function {
                body, parameters, ..
            } => Self::block_has_semantic_errors(body, parameters),
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                Self::expr_has_semantic_errors(function)
                    || arguments.iter().any(Self::expr_has_semantic_errors)
            }
            _ => false,
        }
    }

    /// Check if any call in the block would trigger effect row errors
    /// (E400, E419-E422). Uses explicit `declared_effects` and `param_effect_rows`
    /// instead of reading from the function context stack (which hasn't been pushed yet).
    fn block_has_effect_row_error(
        &mut self,
        body: &Block,
        declared_effects: &[EffectExpr],
        param_effect_rows: &HashMap<Symbol, super::effect_rows::EffectRow>,
    ) -> bool {
        body.statements.iter().any(|s| match s {
            Statement::Expression { expression, .. }
            | Statement::Let {
                value: expression, ..
            } => self.expr_has_effect_row_error(expression, declared_effects, param_effect_rows),
            _ => false,
        })
    }

    fn expr_has_effect_row_error(
        &mut self,
        expr: &Expression,
        declared_effects: &[EffectExpr],
        param_effect_rows: &HashMap<Symbol, super::effect_rows::EffectRow>,
    ) -> bool {
        match expr {
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                if self.call_has_effect_row_error(
                    function,
                    arguments,
                    declared_effects,
                    param_effect_rows,
                ) {
                    return true;
                }
                // Recurse into subexpressions
                self.expr_has_effect_row_error(function, declared_effects, param_effect_rows)
                    || arguments.iter().any(|a| {
                        self.expr_has_effect_row_error(a, declared_effects, param_effect_rows)
                    })
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                self.expr_has_effect_row_error(condition, declared_effects, param_effect_rows)
                    || self.block_has_effect_row_error(
                        consequence,
                        declared_effects,
                        param_effect_rows,
                    )
                    || alternative.as_ref().is_some_and(|b| {
                        self.block_has_effect_row_error(b, declared_effects, param_effect_rows)
                    })
            }
            Expression::DoBlock { block, .. } => {
                self.block_has_effect_row_error(block, declared_effects, param_effect_rows)
            }
            Expression::Sealing { expr, allowed, .. } => {
                self.sealing_has_effect_row_error(expr, allowed, declared_effects)
                    || self.expr_has_effect_row_error(expr, declared_effects, param_effect_rows)
            }
            Expression::Match { arms, .. } => arms.iter().any(|arm| {
                self.expr_has_effect_row_error(&arm.body, declared_effects, param_effect_rows)
            }),
            // Check perform for unknown effects/operations (E404/E405)
            Expression::Perform {
                effect, operation, ..
            } => {
                // Check if the effect resolves
                let resolved_effect = self.lookup_effect_alias(*effect).unwrap_or(*effect);
                self.effect_op_signature(resolved_effect, *operation)
                    .is_none()
            }
            // Don't recurse into nested function bodies — they have their own effect context
            _ => false,
        }
    }

    /// Check a single call expression for effect row errors.
    /// Returns `true` if the call would trigger E400 or E419-E422.
    fn call_has_effect_row_error(
        &mut self,
        function: &Expression,
        arguments: &[Expression],
        declared_effects: &[EffectExpr],
        param_effect_rows: &HashMap<Symbol, super::effect_rows::EffectRow>,
    ) -> bool {
        use super::effect_rows::{EffectRow, solve_row_constraints};

        // Check base function effect requirements (e.g., print requires IO)
        if let Expression::Identifier { name, .. } = function {
            let name_str = self.sym(*name);
            if let Some(required_effect) = self.required_effect_for_base_name(name_str) {
                let required_sym = self.interner.intern(required_effect);
                if !Self::is_effect_in_declared(required_sym, declared_effects) {
                    return true;
                }
            }
        }

        let Some(contract) = self
            .resolve_call_contract(function, arguments.len())
            .cloned()
        else {
            return false;
        };

        if contract.effects.is_empty() {
            return false;
        }

        let required_row = EffectRow::from_effect_exprs(&contract.effects);
        let constraints =
            self.collect_effect_row_constraints_with_rows(&contract, arguments, param_effect_rows);
        let solution = solve_row_constraints(&constraints);

        // Check for constraint violations (E421, E422)
        if !solution.violations.is_empty() {
            return true;
        }

        // Check for unresolved effect variables (E419, E420)
        let unresolved: Vec<Symbol> = required_row
            .unresolved_vars(&solution)
            .into_iter()
            .filter(|effect_var| !Self::is_effect_in_declared(*effect_var, declared_effects))
            .collect();
        if !unresolved.is_empty() {
            return true;
        }

        // Check for missing concrete effects (E400)
        let required_effects = required_row.concrete_effects(&solution);
        for required_name in required_effects {
            if !Self::is_effect_in_declared(required_name, declared_effects) {
                return true;
            }
        }

        false
    }

    fn sealing_has_effect_row_error(
        &mut self,
        expr: &Expression,
        allowed: &[EffectExpr],
        declared_effects: &[EffectExpr],
    ) -> bool {
        let io_effect = crate::syntax::builtin_effects::io_effect_symbol(&mut self.interner);
        let time_effect = crate::syntax::builtin_effects::time_effect_symbol(&mut self.interner);
        let inferred = self.contract_effect_sets();
        let actual = self.infer_effects_from_expr(
            expr,
            self.current_module_prefix,
            &inferred,
            io_effect,
            time_effect,
        );
        let allowed = self.evaluate_sealing_allowed_effects_for_declared(allowed, declared_effects);
        !actual.is_subset(&allowed)
    }

    fn evaluate_sealing_allowed_effects_for_declared(
        &self,
        allowed: &[EffectExpr],
        declared_effects: &[EffectExpr],
    ) -> HashSet<Symbol> {
        let ambient: HashSet<Symbol> = declared_effects
            .iter()
            .flat_map(EffectExpr::normalized_names)
            .collect();
        let mut out = HashSet::new();
        for effect in allowed {
            out.extend(self.evaluate_sealing_effect_expr_for_declared(effect, &ambient));
        }
        out
    }

    fn evaluate_sealing_effect_expr_for_declared(
        &self,
        effect: &EffectExpr,
        ambient: &HashSet<Symbol>,
    ) -> HashSet<Symbol> {
        match effect {
            EffectExpr::Named { name, .. } => HashSet::from([*name]),
            EffectExpr::RowVar { name, .. } if self.sym(*name) == "ambient" => ambient.clone(),
            EffectExpr::RowVar { .. } => HashSet::new(),
            EffectExpr::Add { left, right, .. } => {
                let mut out = self.evaluate_sealing_effect_expr_for_declared(left, ambient);
                out.extend(self.evaluate_sealing_effect_expr_for_declared(right, ambient));
                out
            }
            EffectExpr::Subtract { left, right, .. } => {
                let mut out = self.evaluate_sealing_effect_expr_for_declared(left, ambient);
                for effect in self.evaluate_sealing_effect_expr_for_declared(right, ambient) {
                    out.remove(&effect);
                }
                out
            }
        }
    }

    /// Check if an effect is available in the declared effects list.
    /// When no effects are declared (empty slice), no effects are available —
    /// matching the AST path's behavior where `current_function_effects()` returns
    /// `Some(&[])` and `is_effect_available()` returns false.
    fn is_effect_in_declared(effect: Symbol, declared_effects: &[EffectExpr]) -> bool {
        declared_effects
            .iter()
            .any(|e| e.normalized_names().contains(&effect))
    }

    fn block_contains_cfg_incompatible_statements_ast(body: &Block) -> bool {
        body.statements.iter().any(|statement| {
            matches!(
                statement,
                Statement::Module { .. } | Statement::Import { .. }
            )
        })
    }

    /// Check if the function body references identifiers that can't be resolved
    /// against the symbol table, base functions, or local scope. These are truly
    /// undefined variables (e.g. `mystery_value`) whose Core IR degenerates to
    /// `()`, making the CFG path compile without error. The AST path must handle
    /// these for proper E004 reporting.
    fn block_has_undefined_identifier(&mut self, body: &Block, parameters: &[Symbol]) -> bool {
        let mut local_names: Vec<Symbol> = parameters.to_vec();
        for stmt in &body.statements {
            if let Statement::Let { name, .. } = stmt {
                local_names.push(*name);
            }
            if self.stmt_has_undefined_ident(stmt, &local_names) {
                return true;
            }
        }
        false
    }

    fn stmt_has_undefined_ident(&mut self, stmt: &Statement, locals: &[Symbol]) -> bool {
        match stmt {
            Statement::Expression { expression, .. } => {
                self.expr_has_undefined_ident(expression, locals)
            }
            Statement::Let { value, .. } => self.expr_has_undefined_ident(value, locals),
            _ => false,
        }
    }

    fn expr_has_undefined_ident(&mut self, expr: &Expression, locals: &[Symbol]) -> bool {
        match expr {
            Expression::Identifier { name, .. } => {
                // Check: local binding, symbol table, exposed bindings,
                // known primop name (any arity), or variadic builtins.
                if locals.contains(name) || self.exposed_bindings.contains_key(name) {
                    return false;
                }
                if self.symbol_table.resolve(*name).is_some() {
                    return false;
                }
                let name_str = self.sym(*name).to_string();
                name_str != "list"
                    && crate::core::CorePrimOp::from_name(&name_str, 0).is_none()
                    && crate::core::CorePrimOp::from_name(&name_str, 1).is_none()
                    && crate::core::CorePrimOp::from_name(&name_str, 2).is_none()
                    && crate::core::CorePrimOp::from_name(&name_str, 3).is_none()
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                self.expr_has_undefined_ident(condition, locals)
                    || consequence
                        .statements
                        .iter()
                        .any(|s| self.stmt_has_undefined_ident(s, locals))
                    || alternative.as_ref().is_some_and(|b| {
                        b.statements
                            .iter()
                            .any(|s| self.stmt_has_undefined_ident(s, locals))
                    })
            }
            Expression::Match {
                scrutinee, arms, ..
            } => {
                self.expr_has_undefined_ident(scrutinee, locals)
                    || arms
                        .iter()
                        .any(|arm| self.expr_has_undefined_ident(&arm.body, locals))
            }
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                self.expr_has_undefined_ident(function, locals)
                    || arguments
                        .iter()
                        .any(|a| self.expr_has_undefined_ident(a, locals))
            }
            Expression::DoBlock { block, .. } => block
                .statements
                .iter()
                .any(|s| self.stmt_has_undefined_ident(s, locals)),
            _ => false,
        }
    }

    #[allow(dead_code)]
    pub(super) fn emit_store_binding(&mut self, binding: &crate::compiler::binding::Binding) {
        match binding.symbol_scope {
            SymbolScope::Local => {
                self.emit(OpCode::OpSetLocal, &[binding.index]);
            }
            SymbolScope::Global => {
                self.emit(OpCode::OpSetGlobal, &[binding.index]);
            }
            _ => {}
        }
    }

    fn destructure_pattern_expected_type(&mut self, pattern: &Pattern) -> Option<InferType> {
        match pattern {
            Pattern::Tuple { elements, .. } => Some(InferType::Tuple(
                elements
                    .iter()
                    .map(|elem| {
                        self.destructure_pattern_expected_type(elem)
                            .unwrap_or_else(|| self.type_env.alloc_infer_type_var())
                    })
                    .collect(),
            )),
            Pattern::Some { pattern, .. } => Some(InferType::App(
                crate::types::type_constructor::TypeConstructor::Option,
                vec![
                    self.destructure_pattern_expected_type(pattern)
                        .unwrap_or_else(|| self.type_env.alloc_infer_type_var()),
                ],
            )),
            Pattern::None { .. } => Some(InferType::App(
                crate::types::type_constructor::TypeConstructor::Option,
                vec![self.type_env.alloc_infer_type_var()],
            )),
            Pattern::Left { pattern, .. } => Some(InferType::App(
                crate::types::type_constructor::TypeConstructor::Either,
                vec![
                    self.destructure_pattern_expected_type(pattern)
                        .unwrap_or_else(|| self.type_env.alloc_infer_type_var()),
                    self.type_env.alloc_infer_type_var(),
                ],
            )),
            Pattern::Right { pattern, .. } => Some(InferType::App(
                crate::types::type_constructor::TypeConstructor::Either,
                vec![
                    self.type_env.alloc_infer_type_var(),
                    self.destructure_pattern_expected_type(pattern)
                        .unwrap_or_else(|| self.type_env.alloc_infer_type_var()),
                ],
            )),
            Pattern::EmptyList { .. } => Some(InferType::App(
                crate::types::type_constructor::TypeConstructor::List,
                vec![self.type_env.alloc_infer_type_var()],
            )),
            Pattern::Cons { .. } => Some(InferType::App(
                crate::types::type_constructor::TypeConstructor::List,
                vec![self.type_env.alloc_infer_type_var()],
            )),
            Pattern::Identifier { .. } | Pattern::Wildcard { .. } => {
                Some(self.type_env.alloc_infer_type_var())
            }
            Pattern::Literal { .. }
            | Pattern::Constructor { .. }
            | Pattern::NamedConstructor { .. } => None,
        }
    }

    fn known_concrete_expr_type_mismatch(
        &self,
        expected: &InferType,
        expression: &crate::syntax::expression::Expression,
    ) -> Option<(String, String)> {
        let HmExprTypeResult::Known(actual) = self.hm_expr_type_strict_path(expression) else {
            return None;
        };
        if !expected.is_concrete() || !actual.is_concrete() {
            return None;
        }

        let compatible = if let Ok(subst) = unify(expected, &actual) {
            let resolved_expected = expected.apply_type_subst(&subst);
            let resolved_actual = actual.apply_type_subst(&subst);
            resolved_expected == resolved_actual
        } else {
            false
        };
        if compatible {
            return None;
        }

        Some((
            display_infer_type(expected, &self.interner),
            display_infer_type(&actual, &self.interner),
        ))
    }

    fn is_known_function_effect_annotation(&self, effect: Symbol) -> bool {
        if self.effect_declared_ops(effect).is_some() {
            return true;
        }
        // Proposal 0161 B3: `IO` and `Time` are coarse aliases that expand to
        // fine-grained rows; the decomposed labels themselves are also valid
        // annotations (`with Console`, `with FileSystem`, `with Clock`, …).
        let effect_name = self.sym(effect);
        effect_name == "State"
            || crate::syntax::builtin_effects::is_known_function_effect_annotation_name(
                effect_name,
            )
    }

    fn effect_named_span(effect: &EffectExpr, target: Symbol) -> Option<Span> {
        match effect {
            EffectExpr::Named { name, span } => (*name == target).then_some(*span),
            EffectExpr::RowVar { name, span } => (*name == target).then_some(*span),
            EffectExpr::Add { left, right, .. } | EffectExpr::Subtract { left, right, .. } => {
                Self::effect_named_span(left, target)
                    .or_else(|| Self::effect_named_span(right, target))
            }
        }
    }

    fn unknown_function_effect_diagnostic(&self, effect: Symbol, span: Span) -> Diagnostic {
        let effect_name = self.sym(effect).to_string();
        let hint = suggest_effect_name(&effect_name).unwrap_or_else(|| {
            "Use a declared effect name in `with ...` or declare the effect first.".to_string()
        });
        Diagnostic::make_error_dynamic(
            "E407",
            "UNKNOWN FUNCTION EFFECT",
            ErrorType::Compiler,
            format!(
                "Function effect annotation references unknown effect `{}`.",
                effect_name
            ),
            Some(hint),
            self.file_path.clone(),
            span,
        )
        .with_display_title("Unknown Effect")
        .with_category(crate::diagnostics::DiagnosticCategory::Effects)
        .with_primary_label(span, "unknown effect in function annotation")
        .with_phase(DiagnosticPhase::Effect)
    }

    fn compile_statement_collect_error(
        &mut self,
        statement: &Statement,
    ) -> Option<Box<Diagnostic>> {
        self.compile_statement(statement).err()
    }

    pub(super) fn compile_statement(&mut self, statement: &Statement) -> CompileResult<()> {
        let previous_span = self.current_span;
        self.current_span = Some(statement.span());
        let result = (|| {
            match statement {
                Statement::Expression {
                    expression,
                    has_semicolon,
                    ..
                } => {
                    self.compile_expression(expression)?;
                    if !self.in_tail_position || *has_semicolon {
                        self.emit(OpCode::OpPop, &[]);
                    }
                }
                Statement::Let {
                    name,
                    type_annotation,
                    value,
                    span,
                    ..
                } => {
                    let name = *name;
                    // Check for duplicate in current scope FIRST (takes precedence)
                    if let Some(existing) = self.symbol_table.resolve(name)
                        && self.symbol_table.exists_in_current_scope(name)
                    {
                        let name_str = self.sym(name);
                        return Err(Self::boxed(self.make_redeclaration_error(
                            name_str,
                            *span,
                            Some(existing.span),
                            None,
                        )));
                    }
                    // Then check for import collision (only if not a duplicate in same scope)
                    if self.scope_index == 0 && self.file_scope_symbols.contains(&name) {
                        let name_str = self.sym(name);
                        return Err(Self::boxed(
                            self.make_import_collision_error(name_str, *span),
                        ));
                    }

                    let symbol = self.symbol_table.define(name, *span);
                    if let Some(annotation) = type_annotation {
                        if let Some(expected_infer) = TypeEnv::infer_type_from_type_expr(
                            annotation,
                            &Default::default(),
                            &self.interner,
                        ) {
                            if let Some((expected_str, actual_str)) =
                                self.known_concrete_expr_type_mismatch(&expected_infer, value)
                            {
                                let name_str = self.sym(name).to_string();
                                return Err(Self::boxed(let_annotation_type_mismatch(
                                    self.file_path.clone(),
                                    annotation.span(),
                                    value.span(),
                                    &name_str,
                                    &expected_str,
                                    &actual_str,
                                )));
                            }
                            self.validate_expr_expected_type_with_policy(
                                &expected_infer,
                                value,
                                "initializer type is known at compile time",
                                "binding initializer does not match type annotation".to_string(),
                                "typed let initializer",
                                true,
                            )?;
                        } else if self.strict_mode {
                            return Err(Self::boxed(
                                self.unresolved_boundary_error(value, "typed let initializer"),
                            ));
                        }
                    }
                    self.compile_expression(value)?;
                    if let Some(annotation) = type_annotation
                        && let Ok(ty) = convert_type_expr_checked(
                            annotation,
                            &self.interner,
                            &[],
                            &self.adt_contract_specs,
                        )
                    {
                        self.bind_static_type(name, ty);
                    } else if let crate::compiler::hm_expr_typer::HmExprTypeResult::Known(inferred) =
                        self.hm_expr_type_strict_path(value)
                        && let Ok(runtime) = TypeEnv::try_to_runtime(&inferred, &Default::default())
                    {
                        self.bind_static_type(name, runtime);
                    }

                    // Track aliases of effectful base functions so indirect calls
                    // like `let p = print; p(...)` keep static effect checking.
                    self.track_effect_alias_for_binding(name, value);

                    match symbol.symbol_scope {
                        SymbolScope::Global => self.emit(OpCode::OpSetGlobal, &[symbol.index]),
                        SymbolScope::Local => self.emit(OpCode::OpSetLocal, &[symbol.index]),
                        _ => {
                            return Err(Self::boxed(Diagnostic::make_error(
                                &ICE_SYMBOL_SCOPE_LET,
                                &[],
                                self.file_path.clone(),
                                Span::new(Position::default(), Position::default()),
                            )));
                        }
                    };

                    self.symbol_table.mark_assigned(name).ok();
                    if self.scope_index == 0 {
                        self.file_scope_symbols.insert(name);
                    }
                }
                Statement::LetDestructure { pattern, value, .. } => {
                    if let Some(expected_infer) = self.destructure_pattern_expected_type(pattern) {
                        self.validate_expr_expected_type_with_policy(
                            &expected_infer,
                            value,
                            "destructure source type is known at compile time",
                            "destructure source does not match pattern shape".to_string(),
                            "tuple destructure source",
                            true,
                        )?;
                    }
                    self.compile_expression(value)?;
                    let temp_symbol = self.symbol_table.define_temp();
                    match temp_symbol.symbol_scope {
                        SymbolScope::Global => {
                            self.emit(OpCode::OpSetGlobal, &[temp_symbol.index]);
                        }
                        SymbolScope::Local => {
                            self.emit(OpCode::OpSetLocal, &[temp_symbol.index]);
                        }
                        _ => {
                            return Err(Self::boxed(Diagnostic::make_error(
                                &ICE_SYMBOL_SCOPE_LET,
                                &[],
                                self.file_path.clone(),
                                Span::new(Position::default(), Position::default()),
                            )));
                        }
                    }
                    self.compile_pattern_bind(&temp_symbol, pattern)?;
                }
                Statement::Assign { name, span, .. } => {
                    let name = *name;
                    // Check if variable exists
                    let name_str = self.sym(name).to_string();
                    let symbol = self.resolve_visible_symbol(name).ok_or_else(|| {
                        Self::boxed(self.make_undefined_variable_error(&name_str, *span))
                    })?;

                    if symbol.symbol_scope == SymbolScope::Free {
                        let name_str = self.sym(name);
                        return Err(Self::boxed(
                            self.make_outer_assignment_error(name_str, *span),
                        ));
                    }

                    // Flux bindings are immutable today: assignment syntax is parsed so we can
                    // emit a targeted diagnostic, but reassignment is not allowed.
                    let name_str = self.sym(name);
                    return Err(Self::boxed(self.make_immutability_error(name_str, *span)));
                }
                Statement::Return { value, .. } => match value {
                    Some(expr) => {
                        self.compile_expression(expr)?;
                        // Don't emit OpReturnValue if we just emitted OpTailCall
                        // because the tail call will loop back instead of returning
                        if !self.is_last_instruction(OpCode::OpTailCall) {
                            self.emit(OpCode::OpReturnCheck, &[]);
                            self.emit(OpCode::OpReturnValue, &[]);
                        }
                    }
                    None => {
                        self.emit(OpCode::OpReturnCheck, &[]);
                        self.emit(OpCode::OpReturn, &[]);
                    }
                },
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
                    let name = *name;
                    // For top-level functions, checks were already done in pass 1
                    // Only check for nested functions (scope_index > 0)
                    if self.scope_index > 0
                        && let Some(existing) = self.symbol_table.resolve(name)
                        && self.symbol_table.exists_in_current_scope(name)
                        && existing.symbol_scope != SymbolScope::Function
                        // Skip if this binding was predeclared for forward references
                        && existing.span != *span
                    {
                        let name_str = self.sym(name);
                        return Err(Self::boxed(self.make_redeclaration_error(
                            name_str,
                            *span,
                            Some(existing.span),
                            Some("Use a different name or remove the previous definition"),
                        )));
                    }
                    let effective_effects: Vec<crate::syntax::effect_expr::EffectExpr> =
                        if effects.is_empty() {
                            self.lookup_unqualified_contract(name, parameters.len())
                                .map(|contract| contract.effects.clone())
                                .unwrap_or_default()
                        } else {
                            effects.clone()
                        };
                    self.compile_function_statement(
                        name,
                        parameters,
                        parameter_types,
                        return_type,
                        &effective_effects,
                        body,
                        *intrinsic,
                        None,
                        *span,
                    )?;
                    // For nested functions, add to file_scope_symbols
                    if self.scope_index == 0 {
                        // Already added in pass 1 for top-level functions
                        self.file_scope_symbols.insert(name);
                    }
                }
                Statement::Module { name, body, span } => {
                    let name = *name;
                    if self.scope_index > 0 {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &MODULE_SCOPE,
                            &[],
                            self.file_path.clone(),
                            *span,
                        )));
                    }
                    let name_str = self.sym(name).to_string();
                    let binding_name_str = module_binding_name(&name_str).to_string();
                    let binding_name = self.interner.intern(&binding_name_str);
                    if self.scope_index == 0 && self.file_scope_symbols.contains(&binding_name) {
                        let binding_name_str = self.sym(binding_name);
                        return Err(Self::boxed(
                            self.make_import_collision_error(binding_name_str, *span),
                        ));
                    }
                    let name_str = self.sym(name);
                    if !is_valid_module_name(name_str) {
                        let name_str = self.sym(name);
                        return Err(Self::boxed(Diagnostic::make_error(
                            &INVALID_MODULE_NAME,
                            &[name_str],
                            self.file_path.clone(),
                            *span,
                        )));
                    }
                    self.compile_module_statement(name, body, span.start, None)?;
                    if self.scope_index == 0 {
                        let name_str = self.sym(name).to_string();
                        let binding_name_str = module_binding_name(&name_str).to_string();
                        let binding_name = self.interner.intern(&binding_name_str);
                        self.file_scope_symbols.insert(binding_name);
                    }
                }
                Statement::Import {
                    name,
                    alias,
                    except,
                    exposing,
                    span,
                } => {
                    let name = *name;
                    if self.scope_index > 0 {
                        let name_str = self.sym(name);
                        return Err(Self::boxed(Diagnostic::make_error(
                            &IMPORT_SCOPE,
                            &[name_str],
                            self.file_path.clone(),
                            *span,
                        )));
                    }
                    if self.is_flow_module_symbol(name) {
                        self.compile_import_statement(name, *alias, except)?;
                        self.register_exposed_bindings(name, exposing, *span)?;
                        return Ok(());
                    }
                    let name_str = self.sym(name).to_string();
                    let alias_str = alias.map(|a| self.sym(a).to_string());
                    let binding_name_str =
                        import_binding_name(&name_str, alias_str.as_deref()).to_string();
                    let binding_name = self.interner.intern(&binding_name_str);

                    if self.file_scope_symbols.contains(&binding_name) {
                        let binding_name_str = self.sym(binding_name);
                        return Err(Self::boxed(
                            self.make_import_collision_error(binding_name_str, *span),
                        ));
                    }
                    // Reserve the name for this file so later declarations can't collide.
                    self.file_scope_symbols.insert(binding_name);
                    self.compile_import_statement(name, *alias, except)?;

                    // Register exposed bindings for unqualified access.
                    self.register_exposed_bindings(name, exposing, *span)?;
                }
                Statement::Data { name, variants, .. } => {
                    self.compile_data_statement(*name, variants)?;
                }
                // Effect declarations are syntax only for now no bytecode emitted.
                Statement::EffectDecl { .. } => {}
                // Effect aliases are compile-time only (Proposal 0161 B1); the
                // compiler's alias table is populated before codegen runs.
                Statement::EffectAlias { .. } => {}
                // Type class declarations are syntax only — no bytecode emitted.
                Statement::Class { .. } => {}
                Statement::Instance { .. } => {}
            }
            Ok(())
        })();
        self.current_span = previous_span;
        result
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn compile_function_statement(
        &mut self,
        name: Symbol,
        parameters: &[Symbol],
        parameter_types: &[Option<TypeExpr>],
        return_type: &Option<TypeExpr>,
        effects: &[EffectExpr],
        body: &Block,
        intrinsic: Option<crate::core::CorePrimOp>,
        ir_function: Option<&IrFunction>,
        function_span: Span,
    ) -> CompileResult<()> {
        let definition_span = Span::new(function_span.start, function_span.start);

        if let Some(param) = Self::find_duplicate_name(parameters) {
            let param_str = self.sym(param);
            return Err(Self::boxed(Diagnostic::make_error(
                &DUPLICATE_PARAMETER,
                &[param_str],
                self.file_path.clone(),
                definition_span,
            )));
        }

        for effect_expr in effects {
            for effect_name in effect_expr.normalized_names() {
                if !self.is_known_function_effect_annotation(effect_name) {
                    let span = Self::effect_named_span(effect_expr, effect_name)
                        .unwrap_or(definition_span);
                    return Err(Self::boxed(
                        self.unknown_function_effect_diagnostic(effect_name, span),
                    ));
                }
            }
        }

        // Only reuse a current-scope predeclaration (for top-level/module pass 1).
        // If an outer or Base binding already exists, this nested function must
        // create a new local binding so it correctly shadows that name.
        let symbol = if self.symbol_table.exists_in_current_scope(name) {
            let existing = self
                .symbol_table
                .resolve(name)
                .expect("current-scope function binding must resolve");
            if existing.symbol_scope == SymbolScope::Function {
                // A current-scope self-binding belongs to the enclosing
                // function, not this nested declaration. Create a new local
                // binding so the nested function shadows the enclosing name.
                self.symbol_table.define(name, definition_span)
            } else {
                existing
            }
        } else {
            self.symbol_table.define(name, definition_span)
        };

        self.enter_scope();
        self.symbol_table
            .define_function_name(name, definition_span);

        // If the IR function has extra dict params (from dict elaboration),
        // define them in the scope BEFORE the AST params so they get the
        // correct local indices matching the VM calling convention.
        // Only apply to user-defined constrained functions (not __tc_*
        // mangled instance methods, which may have contextual dict params
        // handled via a separate mechanism).
        if let Some(ir_fn) = ir_function {
            let extra = ir_fn.params.len().saturating_sub(parameters.len());
            if extra > 0 {
                let has_scheme_constraints = self
                    .type_env
                    .lookup(name)
                    .is_some_and(|s| !s.constraints.is_empty());
                if has_scheme_constraints {
                    for ir_param in &ir_fn.params[..extra] {
                        self.symbol_table.define(ir_param.name, Span::default());
                    }
                }
            }
        }

        for (index, param) in parameters.iter().enumerate() {
            self.symbol_table.define(*param, Span::default());
            if let Some(Some(param_ty)) = parameter_types.get(index)
                && let Ok(runtime_ty) = convert_type_expr_checked(
                    param_ty,
                    &self.interner,
                    &[],
                    &self.adt_contract_specs,
                )
            {
                self.bind_static_type(*param, runtime_ty);
            }
        }

        // Emit cost centre entry when profiling is enabled.
        if self.profiling {
            let module_name = self
                .current_module_prefix
                .map(|s| self.sym(s).to_string())
                .unwrap_or_else(|| "<main>".to_string());
            let fn_name = self.sym(name).to_string();
            let cc_idx = self.register_cost_centre(&fn_name, &module_name);
            self.emit(OpCode::OpEnterCC, &[cc_idx as usize]);
        }

        // Track the runtime-facing IR parameter count when CFG path succeeds.
        // Dict elaboration may add dictionary parameters that the AST doesn't
        // know about, while lifted function literals also include captures in
        // `ir_fn.params`. The VM closure arity must count only explicit call
        // arguments, not captured values loaded by `OpClosure`.
        let cfg_param_count: std::cell::Cell<Option<usize>> = std::cell::Cell::new(None);
        if let Some(ir_function) = ir_function {
            let explicit_param_count = match ir_function.origin {
                crate::cfg::IrFunctionOrigin::FunctionLiteral => ir_function
                    .params
                    .len()
                    .saturating_sub(ir_function.captures.len()),
                _ => ir_function.params.len(),
            };
            cfg_param_count.set(Some(explicit_param_count));
        }

        let compile_result: CompileResult<()> = (|| {
            if let Some(op) = intrinsic {
                for param in parameters {
                    let Some(binding) = self.symbol_table.resolve(*param) else {
                        continue;
                    };
                    self.load_symbol(&binding);
                }
                self.emit(OpCode::OpPrimOp, &[op.id() as usize, parameters.len()]);
                self.emit(OpCode::OpReturnCheck, &[]);
                self.emit(OpCode::OpReturnValue, &[]);
                return Ok(());
            }

            // Compile-time return type check: if the declared return type and
            // the static type of the tail expression are both known, verify
            // they're compatible.
            if let Some(ret_annotation) = return_type
                && let Some(expected_ret) = TypeEnv::infer_type_from_type_expr(
                    ret_annotation,
                    &Default::default(),
                    &self.interner,
                )
                && self.block_has_value_tail(body)
                && let Some(Statement::Expression {
                    expression,
                    has_semicolon: false,
                    ..
                }) = body.statements.last()
            {
                if let Some((expected_str, actual_str)) =
                    self.known_concrete_expr_type_mismatch(&expected_ret, expression)
                {
                    let fn_name = self.sym(name).to_string();
                    return Err(Self::boxed(fun_return_annotation_mismatch(
                        self.file_path.clone(),
                        ret_annotation.span(),
                        expression.span(),
                        &fn_name,
                        &expected_str,
                        &actual_str,
                    )));
                }
                self.validate_expr_expected_type_with_policy(
                    &expected_ret,
                    expression,
                    "return expression type is known at compile time",
                    "return expression type does not match the declared return type".to_string(),
                    "function return expression",
                    true,
                )?;
            }

            let param_effect_rows = self.build_param_effect_rows(parameters, parameter_types);
            let requires_ir_only = ir_function.is_some_and(|function| {
                function.params.len() != parameters.len()
                    || self.block_contains_constrained_calls(body)
            });

            // ── CFG primary path ─────────────────────────────────────────
            // Pre-validate semantic errors that CFG can't catch (E001, E002,
            // E056, E300, E400/E419-E422). If errors exist, fall through to
            // AST for proper diagnostic reporting.
            let has_body_errors =
                self.quick_validate_function_body(body, parameters, effects, &param_effect_rows);

            // CFG handles all well-typed functions. AST fallback is used only
            // for: (a) functions with pre-validation errors (incl. strict-mode
            // typed-let errors — see `block_has_typed_let_error`),
            // (b) HM type errors, (c) CFG-incompatible statements such as
            // module/import statements.
            //
            // Well-typed strict-mode annotated lets now stay on the CFG path
            // (Proposal 0167 Part 4).
            let use_ast_path = if requires_ir_only {
                ir_function.is_none() || Self::block_contains_cfg_incompatible_statements_ast(body)
            } else {
                has_body_errors
                    || self.has_hm_diagnostics
                    || ir_function.is_none()
                    || Self::block_contains_cfg_incompatible_statements_ast(body)
            };

            if !use_ast_path {
                let ir_function = ir_function.unwrap();
                let scope_snapshot = self.scopes[self.scope_index].clone();
                let const_len = self.constants.len();
                match self.try_compile_ir_cfg_function_body(ir_function, name) {
                    Some(Ok(())) => {
                        let explicit_param_count = match ir_function.origin {
                            crate::cfg::IrFunctionOrigin::FunctionLiteral => ir_function
                                .params
                                .len()
                                .saturating_sub(ir_function.captures.len()),
                            _ => ir_function.params.len(),
                        };
                        cfg_param_count.set(Some(explicit_param_count));
                        return Ok(());
                    }
                    Some(Err(ref _e)) => {
                        // CFG compilation error (e.g. unresolved name) — roll
                        // back and fall through to AST for proper diagnostics.
                        self.scopes[self.scope_index] = scope_snapshot;
                        self.constants.truncate(const_len);
                    }
                    None => {
                        // Unsupported expression — should be unreachable now
                        // that all IrExpr variants are in supported_expr().
                        // Fall through to AST as a safety net.
                        debug_assert!(
                            false,
                            "CFG returned None for well-typed function '{}' — \
                             all IrExpr variants should be supported",
                            self.sym(name),
                        );
                    }
                }
            }

            // ── AST fallback path ──────────────────────────────────────────
            let body_errors = self.with_function_context_with_param_effect_rows(
                parameters.len(),
                effects,
                param_effect_rows,
                |compiler| compiler.compile_block_with_tail_collect_errors(body),
            );
            if body_errors.is_empty() {
                if self.block_has_value_tail(body) {
                    if self.is_last_instruction(OpCode::OpPop) {
                        self.replace_last_pop_with_return();
                    } else if self.replace_last_local_read_with_return() {
                        // The replacement turned the last instruction into
                        // OpReturnLocal in-place, so the alternative branch
                        // returns directly.  However, if the body ends with
                        // an if-else expression, the consequence branch's
                        // OpJump was patched to `instructions.len()` (the
                        // position right after the alternative).  Because the
                        // replacement didn't grow the buffer, that jump now
                        // targets one byte past the end.  Emit a landing-pad
                        // OpReturnValue so the jump has a valid target.
                        // It is dead code for the alternative path but
                        // harmless (1 extra byte per affected function).
                        self.emit(OpCode::OpReturnCheck, &[]);
                        self.emit(OpCode::OpReturnValue, &[]);
                    } else if !self.is_last_instruction(OpCode::OpReturnValue)
                        && !self.is_last_instruction(OpCode::OpReturnLocal)
                    {
                        self.emit(OpCode::OpReturnCheck, &[]);
                        self.emit(OpCode::OpReturnValue, &[]);
                    }
                } else if !self.is_last_instruction(OpCode::OpReturnValue)
                    && !self.is_last_instruction(OpCode::OpReturnLocal)
                {
                    self.emit(OpCode::OpReturnCheck, &[]);
                    self.emit(OpCode::OpReturn, &[]);
                }
            }
            for err in body_errors {
                let mut diag = *err;
                if diag.phase().is_none() {
                    diag.phase = Some(DiagnosticPhase::TypeCheck);
                }
                self.errors.push(diag);
            }
            Ok(())
        })();

        let free_symbols = self.symbol_table.free_symbols.clone();
        for free in &free_symbols {
            if free.symbol_scope == SymbolScope::Local {
                self.mark_captured_in_current_function(free.index);
            }
        }

        let num_locals = self.symbol_table.num_definitions;

        let (instructions, locations, files, effect_summary) = self.leave_scope();

        compile_result?;

        let boundary_location = {
            let mut files = files;
            let file_id = files
                .iter()
                .position(|file| file == &self.file_path)
                .map(|index| index as u32)
                .unwrap_or_else(|| {
                    files.push(self.file_path.clone());
                    (files.len() - 1) as u32
                });
            (
                files,
                crate::bytecode::debug_info::Location {
                    file_id,
                    span: function_span,
                },
            )
        };
        let (files, boundary_location) = boundary_location;

        for free in &free_symbols {
            self.load_symbol(free);
        }

        let runtime_contract = {
            let contract = crate::compiler::contracts::FnContract {
                type_params: vec![],
                params: parameter_types.to_vec(),
                ret: return_type.clone(),
                effects: effects.to_vec(),
            };
            self.to_runtime_contract(&contract)
        };

        let fn_idx = self.add_constant(Value::Function(Rc::new(
            CompiledFunction::new(
                instructions,
                num_locals,
                cfg_param_count.get().unwrap_or(parameters.len()),
                Some(
                    FunctionDebugInfo::new(Some(self.sym(name).to_string()), files, locations)
                        .with_boundary_location(Some(boundary_location))
                        .with_effect_summary(effect_summary),
                ),
            )
            .with_contract(runtime_contract),
        )));
        self.emit_closure_index(fn_idx, free_symbols.len());

        match symbol.symbol_scope {
            SymbolScope::Global => self.emit(OpCode::OpSetGlobal, &[symbol.index]),
            SymbolScope::Local => self.emit(OpCode::OpSetLocal, &[symbol.index]),
            _ => 0,
        };
        Ok(())
    }

    /// Compile a group of mutually recursive nested functions using sibling
    /// reconstruction: each function captures only shared outer variables and
    /// reconstructs sibling closures on entry from its own captures.
    ///
    /// This avoids circular Rc references (which would violate the no-cycle
    /// invariant) and the Uninit capture problem.
    pub(super) fn compile_mutual_rec_group(
        &mut self,
        fn_stmts: &[&Statement],
    ) -> CompileResult<()> {
        // Collect function info.
        struct FnEntry<'a> {
            name: Symbol,
            parameters: &'a [Symbol],
            body: &'a Block,
            span: Span,
        }

        let entries: Vec<FnEntry<'_>> = fn_stmts
            .iter()
            .map(|stmt| {
                if let Statement::Function {
                    name,
                    parameters,
                    body,
                    span,
                    ..
                } = stmt
                {
                    FnEntry {
                        name: *name,
                        parameters: parameters.as_slice(),
                        body,
                        span: *span,
                    }
                } else {
                    unreachable!("compile_mutual_rec_group called with non-function");
                }
            })
            .collect();

        let group_names: HashSet<Symbol> = entries.iter().map(|e| e.name).collect();

        // Reserve constant pool slots for all functions.
        let const_slots: Vec<usize> = entries
            .iter()
            .map(|_| self.add_constant(Value::None))
            .collect();

        // Compute shared outer captures: union of all free vars minus group members.
        let mut shared_captures: Vec<Symbol> = Vec::new();
        {
            let mut seen = HashSet::new();
            for entry in &entries {
                let fv = collect_free_vars_in_function_body(entry.parameters, entry.body);
                for sym in fv {
                    if !group_names.contains(&sym) && seen.insert(sym) {
                        shared_captures.push(sym);
                    }
                }
            }
            // Sort for deterministic ordering across all functions in the group.
            shared_captures.sort_by_key(|s| s.as_u32());
        }

        // Compile each function body with sibling reconstruction.
        for (idx, entry) in entries.iter().enumerate() {
            let definition_span = Span::new(entry.span.start, entry.span.start);

            self.enter_scope();

            // Self-reference (OpCurrentClosure).
            self.symbol_table
                .define_function_name(entry.name, definition_span);

            // Parameters.
            for param in entry.parameters {
                self.symbol_table.define(*param, Span::default());
            }

            // Define siblings as locals (so they won't become free vars).
            let mut sibling_locals: Vec<(usize, usize)> = Vec::new(); // (group_idx, local_slot)
            for (j, sib_entry) in entries.iter().enumerate() {
                if j != idx {
                    let binding = self.symbol_table.define(sib_entry.name, sib_entry.span);
                    sibling_locals.push((j, binding.index));
                }
            }

            // Force-resolve shared outer captures to populate free_symbols
            // with a consistent ordering across all functions. Only non-global
            // symbols that exist in the enclosing scope become actual Free vars.
            for &cap_sym in &shared_captures {
                let _ = self.symbol_table.resolve(cap_sym);
            }

            // Emit sibling reconstruction prologue:
            // For each sibling, reconstruct its closure from our own captures.
            // Use the actual free_symbols count (excludes globals).
            let actual_captures = self.symbol_table.free_symbols.len();
            for &(group_j, local_slot) in &sibling_locals {
                for cap_idx in 0..actual_captures {
                    self.emit(OpCode::OpGetFree, &[cap_idx]);
                }
                self.emit_closure_index(const_slots[group_j], actual_captures);
                self.emit(OpCode::OpSetLocal, &[local_slot]);
            }

            // Compile the function body.
            let body_errors = self.with_tail_position(true, |c| {
                c.compile_block_with_tail_collect_errors(entry.body)
            });
            if self.block_has_value_tail(entry.body) {
                if !self.is_last_instruction(OpCode::OpReturnValue)
                    && !self.is_last_instruction(OpCode::OpReturnLocal)
                {
                    self.emit(OpCode::OpReturnCheck, &[]);
                    self.emit(OpCode::OpReturnValue, &[]);
                }
            } else if !self.is_last_instruction(OpCode::OpReturnValue)
                && !self.is_last_instruction(OpCode::OpReturnLocal)
            {
                self.emit(OpCode::OpReturnCheck, &[]);
                self.emit(OpCode::OpReturn, &[]);
            }
            for err in body_errors {
                self.errors.push(*err);
            }

            let free_symbols = self.symbol_table.free_symbols.clone();
            for free in &free_symbols {
                if free.symbol_scope == SymbolScope::Local {
                    self.mark_captured_in_current_function(free.index);
                }
            }

            let num_locals = self.symbol_table.num_definitions;
            let (instructions, locations, files, effect_summary) = self.leave_scope();

            // Store compiled function in the reserved constant pool slot.
            let fn_name = self.sym(entry.name).to_string();
            let compiled = CompiledFunction::new(
                instructions,
                num_locals,
                entry.parameters.len(),
                Some(
                    FunctionDebugInfo::new(Some(fn_name), files, locations)
                        .with_effect_summary(effect_summary),
                ),
            );
            self.constants[const_slots[idx]] = Value::Function(Rc::new(compiled));

            // Push captures for this function in the enclosing scope.
            for free in &free_symbols {
                self.load_symbol(free);
            }
            self.emit_closure_index(const_slots[idx], free_symbols.len());

            // Assign to the predeclared local in the enclosing scope.
            let enclosing_symbol = self
                .symbol_table
                .resolve(entry.name)
                .expect("mutual rec function must be predeclared");
            match enclosing_symbol.symbol_scope {
                SymbolScope::Global => {
                    self.emit(OpCode::OpSetGlobal, &[enclosing_symbol.index]);
                }
                SymbolScope::Local => {
                    self.emit(OpCode::OpSetLocal, &[enclosing_symbol.index]);
                }
                _ => {}
            };
        }

        Ok(())
    }

    pub(super) fn compile_module_statement(
        &mut self,
        name: Symbol,
        body: &Block,
        position: Position,
        ir_program: Option<&IrProgram>,
    ) -> CompileResult<()> {
        let name_str = self.sym(name);
        if !is_valid_module_name(name_str) {
            return Err(Self::boxed(Diagnostic::make_error(
                &INVALID_MODULE_NAME,
                &[name_str],
                self.file_path.clone(),
                Span::new(position, position),
            )));
        }

        // Check if module is already defined
        let binding_name_str = module_binding_name(name_str).to_string();
        let binding_name = self.interner.intern(&binding_name_str);
        if self.symbol_table.exists_in_current_scope(binding_name) {
            let binding_name_str = self.sym(binding_name);
            return Err(Self::boxed(self.make_redeclaration_error(
                binding_name_str,
                Span::new(position, position),
                None,
                None,
            )));
        }

        // Collect all functions from the module body and validate contents
        for statement in &body.statements {
            match statement {
                Statement::Function { name: fn_name, .. } => {
                    if *fn_name == binding_name {
                        let pos = statement.position();
                        let binding_name_str = self.sym(binding_name);
                        return Err(Self::boxed(Diagnostic::make_error(
                            &MODULE_NAME_CLASH,
                            &[binding_name_str],
                            self.file_path.clone(),
                            Span::new(pos, pos),
                        )));
                    }
                }
                // Module Constants Allow let statements in modules
                Statement::Let { .. } => {
                    // Let statements are allowed for module constants
                }
                // ADT type declarations are allowed inside modules
                Statement::Data { .. } => {}
                // Type class declarations are allowed inside modules (Proposal 0151).
                // Semantic processing happens in the class collection pipeline; the
                // bytecode compiler treats them as transparent here.
                Statement::Class { .. } => {}
                Statement::Instance { .. } => {}
                // Imports inside module bodies are allowed (Proposal 0151 §5a).
                // Resolution happens via the module graph; the bytecode compiler
                // ignores them at this site.
                Statement::Import { .. } => {}
                // Effect declarations are allowed inside modules (Proposal 0151
                // Phase 4a-prereq). Semantic processing happens in the existing
                // effect-handling pipeline; the bytecode compiler treats the
                // declaration as transparent here, identical to how it treats
                // top-level `effect` declarations.
                Statement::EffectDecl { .. } => {}
                // Effect aliases (Proposal 0161 B1) are allowed inside modules
                // for the same reason EffectDecl is — they only affect the
                // compile-time alias table.
                Statement::EffectAlias { .. } => {}
                _ => {
                    let pos = statement.position();
                    return Err(Self::boxed(Diagnostic::make_error(
                        &INVALID_MODULE_CONTENT,
                        &[],
                        self.file_path.clone(),
                        Span::new(pos, pos),
                    )));
                }
            }
        }

        self.imported_modules.insert(binding_name);
        let previous_module = self.current_module_prefix;
        self.current_module_prefix = Some(binding_name);

        // ====================================================================
        // START: MODULE CONSTANTS (bytecode/module_constants/)
        // ====================================================================
        // PASS 0: MODULE CONSTANTS
        // Compile-time constant evaluation with automatic dependency resolution.
        // Implementation uses utilities from bytecode/module_constants/:
        // - find_constant_refs: Find dependencies in expressions
        // - topological_sort_constants: Order constants (dependencies first)
        // - eval_const_expr: Evaluate constant expressions at compile time
        // ====================================================================

        // Compile module constants (analysis + evaluation)
        let constants = match compile_module_constants(body, binding_name, &mut self.interner) {
            Ok(result) => result,
            Err(err) => {
                self.current_module_prefix = previous_module;
                return Err(Self::boxed(self.convert_const_compile_error(err, position)));
            }
        };

        // Store evaluated constants in compiler's module_constants map
        self.module_constants.extend(constants);

        // ====================================================================
        // END: MODULE CONSTANTS
        // ====================================================================

        let module_prefix = format!("{}.", self.sym(binding_name));
        let const_names: HashSet<Symbol> = self
            .module_constants
            .keys()
            .copied()
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|qualified| {
                let qualified_str = self.interner.try_resolve(qualified)?.to_string();
                qualified_str
                    .strip_prefix(module_prefix.as_str())
                    .map(|short| self.interner.intern(short))
            })
            .collect();

        // PASS 1: Predeclare all module function names with qualified names
        // This enables forward references within the module
        for statement in &body.statements {
            if let Statement::Function {
                name: fn_name,
                span,
                ..
            } = statement
            {
                let qualified_name = self.interner.intern_join(binding_name, *fn_name);
                // Check for duplicate declaration
                if let Some(existing) = self.symbol_table.resolve(qualified_name)
                    && self.symbol_table.exists_in_current_scope(qualified_name)
                {
                    self.current_module_prefix = previous_module;
                    let qualified_name_str = self.sym(qualified_name);
                    return Err(Self::boxed(self.make_redeclaration_error(
                        qualified_name_str,
                        *span,
                        Some(existing.span),
                        Some("Use a different name or remove the previous definition"),
                    )));
                }
                // Predeclare the function
                self.symbol_table.define(qualified_name, *span);
            }
        }

        // PASS 1b: Predeclare non-constant module values so functions can
        // reference them regardless of source order.
        for statement in &body.statements {
            if let Statement::Let {
                is_public,
                name,
                span,
                ..
            } = statement
            {
                if const_names.contains(name) && !*is_public {
                    continue;
                }
                let qualified_name = self.interner.intern_join(binding_name, *name);
                if self.symbol_table.resolve(qualified_name).is_none() {
                    self.symbol_table.define(qualified_name, *span);
                }
            }
        }

        // PASS 2: Compile each function body
        for statement in &body.statements {
            if let Statement::Function {
                name: fn_name,
                parameters,
                parameter_types,
                return_type,
                effects,
                body: fn_body,
                intrinsic,
                span,
                ..
            } = statement
            {
                let qualified_name = self.interner.intern_join(binding_name, *fn_name);
                let effective_effects: Vec<crate::syntax::effect_expr::EffectExpr> =
                    if effects.is_empty() {
                        self.lookup_contract(Some(binding_name), *fn_name, parameters.len())
                            .map(|contract| contract.effects.clone())
                            .unwrap_or_default()
                    } else {
                        effects.clone()
                    };
                if let Err(err) = self.compile_function_statement(
                    qualified_name,
                    parameters,
                    parameter_types,
                    return_type,
                    &effective_effects,
                    fn_body,
                    *intrinsic,
                    ir_program.and_then(|program| {
                        self.find_ir_function_by_symbol(program, qualified_name)
                    }),
                    *span,
                ) {
                    let mut diag = *err;
                    if diag.phase().is_none() {
                        diag.phase = Some(DiagnosticPhase::TypeCheck);
                    }
                    self.errors.push(diag);
                }
            }
        }

        // PASS 3: Evaluate non-constant module values eagerly once in source order.
        for statement in &body.statements {
            if let Statement::Let {
                is_public,
                name,
                type_annotation,
                value,
                span,
                ..
            } = statement
            {
                if const_names.contains(name) && !*is_public {
                    continue;
                }
                self.compile_module_value_binding(
                    binding_name,
                    *name,
                    type_annotation,
                    value,
                    *span,
                )?;
            }
        }

        self.current_module_prefix = previous_module;

        Ok(())
    }

    pub(super) fn compile_data_statement(
        &mut self,
        name: Symbol,
        variants: &[DataVariant],
    ) -> CompileResult<()> {
        // Data declarations are handled at the registry level (collect_adt_definitions).
        // No bytecode is emitted here — constructors are compiled on-demand when called.
        let _ = (name, variants);
        Ok(())
    }

    fn compile_module_value_binding(
        &mut self,
        module_name: Symbol,
        name: Symbol,
        type_annotation: &Option<TypeExpr>,
        value: &Expression,
        span: Span,
    ) -> CompileResult<()> {
        let qualified_name = self.interner.intern_join(module_name, name);
        let symbol = self
            .symbol_table
            .resolve(qualified_name)
            .unwrap_or_else(|| self.symbol_table.define(qualified_name, span));

        if let Some(annotation) = type_annotation
            && let Some(expected_infer) =
                TypeEnv::infer_type_from_type_expr(annotation, &Default::default(), &self.interner)
        {
            if let Some((expected_str, actual_str)) =
                self.known_concrete_expr_type_mismatch(&expected_infer, value)
            {
                let name_str = self.sym(name).to_string();
                return Err(Self::boxed(let_annotation_type_mismatch(
                    self.file_path.clone(),
                    annotation.span(),
                    value.span(),
                    &name_str,
                    &expected_str,
                    &actual_str,
                )));
            }
            self.validate_expr_expected_type_with_policy(
                &expected_infer,
                value,
                "initializer type is known at compile time",
                "binding initializer does not match type annotation".to_string(),
                "typed let initializer",
                true,
            )?;
        }

        self.compile_expression(value)?;
        if let Some(annotation) = type_annotation
            && let Ok(ty) =
                convert_type_expr_checked(annotation, &self.interner, &[], &self.adt_contract_specs)
        {
            self.bind_static_type(qualified_name, ty);
        } else if let HmExprTypeResult::Known(inferred) = self.hm_expr_type_strict_path(value)
            && let Ok(runtime) = TypeEnv::try_to_runtime(&inferred, &Default::default())
        {
            self.bind_static_type(qualified_name, runtime);
        }

        self.track_effect_alias_for_binding(qualified_name, value);
        match symbol.symbol_scope {
            SymbolScope::Global => self.emit(OpCode::OpSetGlobal, &[symbol.index]),
            SymbolScope::Local => self.emit(OpCode::OpSetLocal, &[symbol.index]),
            _ => {
                return Err(Self::boxed(Diagnostic::make_error(
                    &ICE_SYMBOL_SCOPE_LET,
                    &[],
                    self.file_path.clone(),
                    Span::new(Position::default(), Position::default()),
                )));
            }
        };
        self.symbol_table.mark_assigned(qualified_name).ok();
        Ok(())
    }

    pub(super) fn compile_import_statement(
        &mut self,
        name: Symbol,
        alias: Option<Symbol>,
        except: &[Symbol],
    ) -> CompileResult<()> {
        if !except.is_empty() {
            let binding_name = alias.unwrap_or(name);
            let excluded: std::collections::HashSet<Symbol> = except.iter().copied().collect();
            self.imported_module_exclusions
                .insert(binding_name, excluded);
        }

        if let Some(alias) = alias {
            self.import_aliases.insert(alias, name);
        } else {
            self.imported_modules.insert(name);
        }
        Ok(())
    }

    /// Register exposed bindings so module members can be used unqualified.
    ///
    /// For `exposing (..)`, all public members of the module are registered.
    /// For `exposing (a, b)`, only the named members are registered.
    pub(super) fn register_exposed_bindings(
        &mut self,
        module_name: Symbol,
        exposing: &crate::syntax::statement::ImportExposing,
        span: Span,
    ) -> CompileResult<()> {
        use crate::syntax::statement::ImportExposing;

        match exposing {
            ImportExposing::None => {}
            ImportExposing::All => {
                // Expose all public members of the module.
                let public_members: Vec<Symbol> = self
                    .module_function_visibility
                    .iter()
                    .filter(|((mod_name, _), is_public)| *mod_name == module_name && **is_public)
                    .map(|((_, member), _)| *member)
                    .collect();
                for member in public_members {
                    let qualified = self.interner.intern_join(module_name, member);
                    self.exposed_bindings.insert(member, qualified);
                }
            }
            ImportExposing::Names(names) => {
                for &member in names {
                    // Validate the member exists and is public.
                    let is_public = self
                        .module_function_visibility
                        .get(&(module_name, member))
                        .copied();
                    match is_public {
                        Some(true) => {
                            let qualified = self.interner.intern_join(module_name, member);
                            self.exposed_bindings.insert(member, qualified);
                        }
                        Some(false) => {
                            let member_str = self.sym(member).to_string();
                            let module_str = self.sym(module_name).to_string();
                            return Err(Self::boxed(Diagnostic::make_error_dynamic(
                                "E420",
                                "Private Member in Exposing",
                                ErrorType::Compiler,
                                format!(
                                    "Cannot expose `{}` because it is not a public member of `{}`.",
                                    member_str, module_str
                                ),
                                Some(
                                    "Mark the function as `public fn` in the module to expose it."
                                        .to_string(),
                                ),
                                self.file_path.clone(),
                                span,
                            )));
                        }
                        None => {
                            let member_str = self.sym(member).to_string();
                            let module_str = self.sym(module_name).to_string();
                            return Err(Self::boxed(Diagnostic::make_error_dynamic(
                                "E421",
                                "Unknown Member in Exposing",
                                ErrorType::Compiler,
                                format!(
                                    "`{}` is not defined in module `{}`.",
                                    member_str, module_str
                                ),
                                Some(format!(
                                    "Check the public members of `{}` or remove `{}` from the exposing list.",
                                    module_str, member_str
                                )),
                                self.file_path.clone(),
                                span,
                            )));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn compile_block(&mut self, block: &Block) -> CompileResult<()> {
        // If we already have consumable counts from a parent scope (e.g.,
        // function body), don't override them with branch-local counts.
        // Branch blocks (if/else, match arms) must use the parent's counts
        // to avoid incorrectly consuming variables that are still needed
        // after the branch.
        if self.current_consumable_local_use_counts().is_some() {
            for statement in &block.statements {
                if let Some(err) = self.compile_statement_collect_error(statement) {
                    return Err(err);
                }
            }
            return Ok(());
        }

        let mut consumable_counts = HashMap::new();
        for statement in &block.statements {
            self.collect_consumable_param_uses_statement(statement, &mut consumable_counts);
        }

        self.with_consumable_local_use_counts(consumable_counts, |compiler| {
            for statement in &block.statements {
                if let Some(err) = compiler.compile_statement_collect_error(statement) {
                    return Err(err);
                }
            }
            Ok(())
        })
    }

    #[allow(clippy::vec_box)]
    fn compile_block_with_tail_collect_errors(&mut self, block: &Block) -> Vec<Box<Diagnostic>> {
        // Predeclare nested function names so forward references and mutual
        // recursion work inside function bodies (mirrors top-level pass 1).
        if self.scope_index > 0 {
            for stmt in &block.statements {
                if let Statement::Function { name, span, .. } = stmt {
                    let should_predeclare = if self.symbol_table.exists_in_current_scope(*name) {
                        self.symbol_table
                            .resolve(*name)
                            .is_some_and(|binding| binding.symbol_scope == SymbolScope::Function)
                    } else {
                        true
                    };
                    if should_predeclare {
                        self.symbol_table.define(*name, *span);
                    }
                }
            }
        }

        // Detect mutual recursion groups among consecutive function statements.
        let mutual_groups = Self::detect_mutual_rec_groups(&block.statements);

        let len = block.statements.len();
        let mut errors = Vec::new();
        let mut consumable_counts = HashMap::new();
        for statement in &block.statements {
            self.collect_consumable_param_uses_statement(statement, &mut consumable_counts);
        }

        self.with_consumable_local_use_counts(consumable_counts, |compiler| {
            let mut i = 0;
            while i < len {
                // Check if this statement starts a mutual recursion group.
                if let Some(group) = mutual_groups.iter().find(|g| g.start == i) {
                    let stmts: Vec<&Statement> =
                        block.statements[group.start..group.end].iter().collect();
                    if let Err(err) = compiler.compile_mutual_rec_group(&stmts) {
                        errors.push(err);
                    }
                    i = group.end;
                    continue;
                }

                let statement = &block.statements[i];
                let is_last = i == len - 1;
                let tail_eligible = matches!(
                    statement,
                    Statement::Expression {
                        has_semicolon: false,
                        ..
                    } | Statement::Return { .. }
                );

                let result = if is_last && tail_eligible {
                    compiler
                        .with_tail_position(true, |compiler| compiler.compile_statement(statement))
                } else {
                    compiler
                        .with_tail_position(false, |compiler| compiler.compile_statement(statement))
                };

                if let Err(err) = result {
                    errors.push(err);
                }
                i += 1;
            }
        });

        errors
    }

    /// Detect runs of consecutive function statements that form mutual
    /// recursion groups (any function references a sibling defined later).
    fn detect_mutual_rec_groups(stmts: &[Statement]) -> Vec<MutualRecRange> {
        let mut groups = Vec::new();
        let mut i = 0;
        while i < stmts.len() {
            if !matches!(stmts[i], Statement::Function { .. }) {
                i += 1;
                continue;
            }
            // Find the end of this run of consecutive functions.
            let run_start = i;
            while i < stmts.len() && matches!(stmts[i], Statement::Function { .. }) {
                i += 1;
            }
            let run_end = i;
            if run_end - run_start < 2 {
                continue;
            }
            // Check for forward references among siblings.
            let fn_run = &stmts[run_start..run_end];
            if Self::has_mutual_references(fn_run) {
                groups.push(MutualRecRange {
                    start: run_start,
                    end: run_end,
                });
            }
        }
        groups
    }

    /// Check whether any function in a run references a sibling defined later.
    fn has_mutual_references(fn_stmts: &[Statement]) -> bool {
        let names: Vec<Symbol> = fn_stmts
            .iter()
            .filter_map(|s| {
                if let Statement::Function { name, .. } = s {
                    Some(*name)
                } else {
                    None
                }
            })
            .collect();

        for (idx, stmt) in fn_stmts.iter().enumerate() {
            if let Statement::Function {
                parameters, body, ..
            } = stmt
            {
                let fv = collect_free_vars_in_function_body(parameters, body);
                for &sibling_name in &names[idx + 1..] {
                    if fv.contains(&sibling_name) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Compile a block with tail position awareness for the last statement
    pub(super) fn compile_block_with_tail(&mut self, block: &Block) -> CompileResult<()> {
        let mut errors = self
            .compile_block_with_tail_collect_errors(block)
            .into_iter();
        if let Some(first) = errors.next() {
            for err in errors {
                let mut diag = *err;
                if diag.phase().is_none() {
                    diag.phase = Some(DiagnosticPhase::TypeCheck);
                }
                self.errors.push(diag);
            }
            return Err(first);
        }

        Ok(())
    }

    pub(super) fn block_has_value_tail(&self, block: &Block) -> bool {
        matches!(
            block.statements.last(),
            Some(Statement::Expression {
                has_semicolon: false,
                ..
            })
        )
    }
}
