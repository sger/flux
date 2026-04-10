use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};

use super::suggestions::suggest_effect_name;
use crate::{
    ast::{free_vars::collect_free_vars_in_function_body, type_infer::display_infer_type},
    bytecode::{
        binding::Binding,
        compiler::{
            Compiler,
            contracts::{FnContract, convert_type_expr},
            effect_rows::{
                EffectRow, RowConstraint, RowConstraintViolation, solve_row_constraints,
            },
        },
        debug_info::FunctionDebugInfo,
        op_code::{OpCode, make},
        symbol_scope::SymbolScope,
    },
    core::{CorePrimOp, PrimEffect},
    diagnostics::{
        ADT_NON_EXHAUSTIVE_MATCH, CONSTRUCTOR_ARITY_MISMATCH, DUPLICATE_PARAMETER, Diagnostic,
        DiagnosticBuilder, DiagnosticCategory, ICE_SYMBOL_SCOPE_PATTERN,
        ICE_TEMP_SYMBOL_LEFT_BINDING, ICE_TEMP_SYMBOL_LEFT_PATTERN, ICE_TEMP_SYMBOL_MATCH,
        ICE_TEMP_SYMBOL_RIGHT_BINDING, ICE_TEMP_SYMBOL_RIGHT_PATTERN, ICE_TEMP_SYMBOL_SOME_BINDING,
        ICE_TEMP_SYMBOL_SOME_PATTERN, LEGACY_LIST_TAIL_NONE, MODULE_NOT_IMPORTED,
        NON_EXHAUSTIVE_MATCH, PRIVATE_MEMBER, UNKNOWN_CONSTRUCTOR, UNKNOWN_INFIX_OPERATOR,
        UNKNOWN_MODULE_MEMBER, UNKNOWN_PREFIX_OPERATOR,
        compiler_errors::{
            UNREACHABLE_PATTERN_ARM, call_arg_type_mismatch, constructor_pattern_arity_mismatch,
            cross_module_constructor_access_error, cross_module_constructor_access_warning,
            guarded_wildcard_non_exhaustive, type_unification_error, wrong_argument_count,
        },
        diagnostic_for, dynamic_explained_diagnostic,
        position::{Position, Span},
        quality::{EffectConstraintOrigin, with_effect_constraint_origin},
        types::ErrorType,
    },
    runtime::{
        compiled_function::CompiledFunction, handler_descriptor::HandlerDescriptor,
        perform_descriptor::PerformDescriptor, runtime_type::RuntimeType, value::Value,
    },
    syntax::{
        block::Block,
        effect_expr::EffectExpr,
        expression::{Expression, HandleArm, MatchArm, Pattern, StringPart},
        module_graph::is_valid_module_name,
        statement::Statement,
        symbol::Symbol,
        type_expr::TypeExpr,
    },
    types::{infer_type::InferType, type_env::TypeEnv, type_subst::TypeSubst, unify::unify},
};

type CompileResult<T> = Result<T, Box<Diagnostic>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GeneralCoverageDomain {
    Bool,
    Option,
    Either,
    ListLike,
    Tuple(usize),
    Unknown,
}

#[derive(Debug, Clone, Copy)]
struct ConditionalJump {
    position: usize,
    leaves_condition_on_jump: bool,
    /// For 2-operand jumps (e.g. `OpIsAdtJump`): the first operand that must be
    /// preserved when patching the jump target. `None` for single-operand jumps.
    first_operand: Option<usize>,
}

impl Compiler {
    fn collect_consumable_captured_uses(
        &mut self,
        free_vars: impl IntoIterator<Item = Symbol>,
        counts: &mut HashMap<Symbol, usize>,
    ) {
        for name in free_vars {
            if let Some(symbol) = self.symbol_table.resolve(name)
                && self.is_consumable_local(&symbol)
            {
                *counts.entry(name).or_insert(0) += 1;
            }
        }
    }

    /// Patch a conditional jump's target, handling single-, two-, and three-operand jump opcodes.
    fn patch_cond_jump(&mut self, jump: &ConditionalJump, target: usize) {
        let op = OpCode::from(self.current_instructions()[jump.position]);
        if op == OpCode::OpIsAdtJumpLocal {
            // 3-operand instruction: [local_idx: u8, const_idx: u16, jump_offset: u16].
            // Read local_idx and const_idx from the already-emitted bytes, then rewrite
            // all 6 bytes in-place with the patched jump target.
            let local_idx = self.current_instructions()[jump.position + 1] as usize;
            let const_hi = self.current_instructions()[jump.position + 2] as usize;
            let const_lo = self.current_instructions()[jump.position + 3] as usize;
            let const_idx = (const_hi << 8) | const_lo;
            self.replace_instruction(jump.position, make(op, &[local_idx, const_idx, target]));
            return;
        }
        match jump.first_operand {
            None => self.change_operand(jump.position, target),
            Some(first) => {
                self.replace_instruction(jump.position, make(op, &[first, target]));
            }
        }
    }

    /// Returns `true` if this Constructor pattern qualifies for the fast
    /// `OpIsAdtJump` + `OpAdtFields2` path: all fields are simple
    /// (Identifier or Wildcard) and arity is 0, 1, or 2.
    fn is_simple_adt_pattern(fields: &[Pattern]) -> bool {
        fields.len() <= 2
            && fields
                .iter()
                .all(|f| matches!(f, Pattern::Identifier { .. } | Pattern::Wildcard { .. }))
    }

    /// Fast path for `Pattern::Constructor` arms where all fields are
    /// Identifier or Wildcard and arity ≤ 2.
    ///
    /// Emits:
    /// - `load_symbol(scrutinee)` (one Rc clone)
    /// - `OpIsAdtJump const_idx <placeholder>` — peeks at constructor, jumps
    ///   on mismatch leaving the ADT on the stack.  Falls through on match
    ///   with the ADT still on the stack.
    /// - Field extraction from the on-stack ADT (no further loads):
    ///   - arity 0: `OpPop` (clean up the ADT)
    ///   - arity 1: `OpAdtField 0` (pops ADT, pushes field)
    ///   - arity 2: `OpAdtFields2` (pops ADT, pushes field0 then field1)
    /// - Identifier bindings: `define(name)` + `OpSetLocal` for each non-wildcard field.
    ///
    /// Returns the `ConditionalJump` that must be patched to the next arm.
    fn compile_adt_arm_simple(
        &mut self,
        name: &crate::syntax::symbol::Symbol,
        fields: &[Pattern],
        pattern_span: Span,
        scrutinee: &Binding,
    ) -> CompileResult<ConditionalJump> {
        // Validate constructor exists and arity matches (same as compile_pattern_check).
        let constructor_name = self.interner.resolve(*name).to_string();
        if let Some(info) = self.adt_registry.lookup_constructor(*name)
            && fields.len() != info.arity
        {
            return Err(Self::boxed(
                constructor_pattern_arity_mismatch(
                    pattern_span,
                    &constructor_name,
                    info.arity,
                    fields.len(),
                )
                .with_file(self.file_path.clone()),
            ));
        }
        // If the constructor is unknown, compile_pattern_check will produce the
        // proper diagnostic.  The caller guarantees this path is only taken for
        // constructors whose arity equals `fields.len()` (checked via
        // `is_known_simple_adt_arm` before this call).

        let const_idx = self.add_constant(Value::String(Rc::new(constructor_name.clone())));

        self.load_symbol(scrutinee);
        let jump_pos = self.emit(OpCode::OpIsAdtJump, &[const_idx, 9999]);

        match fields.len() {
            0 => {
                // ADT (AdtUnit) is on stack after a successful match; pop it.
                self.emit(OpCode::OpPop, &[]);
            }
            1 => {
                // ADT is on stack; extract field 0 with the existing opcode.
                self.emit(OpCode::OpAdtField, &[0]);
                if let Pattern::Identifier {
                    name: field_name,
                    span,
                } = &fields[0]
                {
                    let sym = self.symbol_table.define(*field_name, *span);
                    match sym.symbol_scope {
                        SymbolScope::Local => {
                            self.emit(OpCode::OpSetLocal, &[sym.index]);
                        }
                        SymbolScope::Global => {
                            self.emit(OpCode::OpSetGlobal, &[sym.index]);
                        }
                        _ => {}
                    }
                } else {
                    // Wildcard: field is on stack but unused; discard it.
                    self.emit(OpCode::OpPop, &[]);
                }
            }
            2 => {
                // ADT is on stack; extract both fields atomically.
                self.emit(OpCode::OpAdtFields2, &[]);
                // Stack after: [..., field0, field1]  (field1 on top)
                // Bind field1 first (top of stack), then field0.
                for field_pat in fields.iter().rev() {
                    if let Pattern::Identifier {
                        name: field_name,
                        span,
                    } = field_pat
                    {
                        let sym = self.symbol_table.define(*field_name, *span);
                        match sym.symbol_scope {
                            SymbolScope::Local => {
                                self.emit(OpCode::OpSetLocal, &[sym.index]);
                            }
                            SymbolScope::Global => {
                                self.emit(OpCode::OpSetGlobal, &[sym.index]);
                            }
                            _ => {}
                        }
                    } else {
                        // Wildcard: discard the field.
                        self.emit(OpCode::OpPop, &[]);
                    }
                }
            }
            _ => unreachable!("compile_adt_arm_simple: arity > 2 should not be called"),
        }

        Ok(ConditionalJump {
            position: jump_pos,
            // ADT clone is on the stack when jumping → needs OpPop at the next arm.
            leaves_condition_on_jump: true,
            first_operand: Some(const_idx),
        })
    }

    /// Variant of `compile_adt_arm_simple` for when the scrutinee is a consumable local variable.
    ///
    /// Emits `OpIsAdtJumpLocal` (peeks at the local slot without cloning) followed by
    /// `OpConsumeLocal` (moves the value to the stack with `Rc` strong_count == 1). This
    /// allows `Rc::try_unwrap` in `OpAdtFields2` / `OpAdtField` to succeed, eliminating
    /// the clone-then-drop cycle that `OpGetLocal` + `OpAdtFields2` would otherwise produce.
    ///
    /// Unlike `compile_adt_arm_simple`, the returned `ConditionalJump` has
    /// `leaves_condition_on_jump: false` — no value is left on the stack when the jump is
    /// taken, so the next arm needs no `OpPop` prefix.
    fn compile_adt_arm_simple_local(
        &mut self,
        name: &crate::syntax::symbol::Symbol,
        fields: &[Pattern],
        pattern_span: Span,
        local_idx: usize,
    ) -> CompileResult<ConditionalJump> {
        let constructor_name = self.interner.resolve(*name).to_string();
        if let Some(info) = self.adt_registry.lookup_constructor(*name)
            && fields.len() != info.arity
        {
            return Err(Self::boxed(
                constructor_pattern_arity_mismatch(
                    pattern_span,
                    &constructor_name,
                    info.arity,
                    fields.len(),
                )
                .with_file(self.file_path.clone()),
            ));
        }

        let const_idx = self.add_constant(Value::String(Rc::new(constructor_name.clone())));

        // Peek at the local slot without cloning (no stack push).
        // On match: fall through; on mismatch: jump (local unchanged, nothing on stack).
        let jump_pos = self.emit(OpCode::OpIsAdtJumpLocal, &[local_idx, const_idx, 9999]);

        // Move the matched value from the local slot onto the stack.
        // After this, local[local_idx] == Uninit and Rc strong_count == 1.
        self.emit_consume_local(local_idx);

        match fields.len() {
            0 => {
                // AdtUnit on stack; pop it.
                self.emit(OpCode::OpPop, &[]);
            }
            1 => {
                self.emit(OpCode::OpAdtField, &[0]);
                if let Pattern::Identifier {
                    name: field_name,
                    span,
                } = &fields[0]
                {
                    let sym = self.symbol_table.define(*field_name, *span);
                    match sym.symbol_scope {
                        SymbolScope::Local => {
                            self.emit(OpCode::OpSetLocal, &[sym.index]);
                        }
                        SymbolScope::Global => {
                            self.emit(OpCode::OpSetGlobal, &[sym.index]);
                        }
                        _ => {}
                    }
                } else {
                    self.emit(OpCode::OpPop, &[]);
                }
            }
            2 => {
                // Rc strong_count == 1 here → Rc::try_unwrap succeeds → zero field clones.
                self.emit(OpCode::OpAdtFields2, &[]);
                for field_pat in fields.iter().rev() {
                    if let Pattern::Identifier {
                        name: field_name,
                        span,
                    } = field_pat
                    {
                        let sym = self.symbol_table.define(*field_name, *span);
                        match sym.symbol_scope {
                            SymbolScope::Local => {
                                self.emit(OpCode::OpSetLocal, &[sym.index]);
                            }
                            SymbolScope::Global => {
                                self.emit(OpCode::OpSetGlobal, &[sym.index]);
                            }
                            _ => {}
                        }
                    } else {
                        self.emit(OpCode::OpPop, &[]);
                    }
                }
            }
            _ => unreachable!("compile_adt_arm_simple_local: arity > 2 should not be called"),
        }

        Ok(ConditionalJump {
            position: jump_pos,
            // Nothing is left on the stack when jumping → no OpPop needed at the next arm.
            leaves_condition_on_jump: false,
            first_operand: None,
        })
    }

    /// Returns `true` if every arm in `arms` satisfies the simple ADT fast-path conditions:
    /// no guard, `Constructor` pattern, arity ≤ 2, all-identifier/wildcard fields, known ADT.
    fn all_arms_simple_adt(&mut self, arms: &[MatchArm]) -> bool {
        for arm in arms {
            if arm.guard.is_some() {
                return false;
            }
            let Pattern::Constructor { name, fields, .. } = &arm.pattern else {
                return false;
            };
            if !Self::is_simple_adt_pattern(fields) {
                return false;
            }
            if self
                .adt_registry
                .lookup_constructor(*name)
                .is_none_or(|info| info.arity != fields.len())
            {
                return false;
            }
        }
        true
    }

    /// If `scrutinee` is a simple identifier that refers to a consumable local variable
    /// used exactly once in the enclosing function body, returns its local slot index.
    /// Otherwise returns `None`.
    ///
    /// When `Some(idx)` is returned it is safe to replace the temp-variable pattern with
    /// `OpIsAdtJumpLocal(idx, …)` + `OpConsumeLocal(idx)`, keeping `Rc` strong_count == 1
    /// at the point of field extraction.
    fn scrutinee_as_simple_consumable_local(&mut self, scrutinee: &Expression) -> Option<usize> {
        let Expression::Identifier { name, .. } = scrutinee else {
            return None;
        };
        let name = *name;
        let symbol = self.resolve_visible_symbol(name)?;
        if !self.is_consumable_local(&symbol) {
            return None;
        }
        let counts = self.current_consumable_local_use_counts()?.clone();
        if counts.get(&name) != Some(&1) {
            return None;
        }
        Some(symbol.index)
    }

    fn emit_conditional_jump_not_truthy_for_compiled_comparison(
        &mut self,
        comparison_op: OpCode,
    ) -> ConditionalJump {
        ConditionalJump {
            position: self.emit_jump_not_truthy_comparison(comparison_op),
            leaves_condition_on_jump: false,
            first_operand: None,
        }
    }

    fn compile_jump_not_truthy_condition(
        &mut self,
        condition: &Expression,
    ) -> CompileResult<ConditionalJump> {
        if let Expression::Infix {
            left,
            operator,
            right,
            ..
        } = condition
        {
            let comparison_op = match operator.as_str() {
                "==" => Some(OpCode::OpEqual),
                "!=" => Some(OpCode::OpNotEqual),
                ">" => Some(OpCode::OpGreaterThan),
                ">=" => Some(OpCode::OpGreaterThanOrEqual),
                "<=" => Some(OpCode::OpLessThanOrEqual),
                "<" => Some(OpCode::OpGreaterThan),
                _ => None,
            };

            if let Some(comparison_op) = comparison_op {
                if operator == "<" {
                    self.compile_non_tail_expression(right)?;
                    self.compile_non_tail_expression(left)?;
                } else {
                    self.compile_non_tail_expression(left)?;
                    self.compile_non_tail_expression(right)?;
                }
                return Ok(
                    self.emit_conditional_jump_not_truthy_for_compiled_comparison(comparison_op)
                );
            }
        }

        self.compile_non_tail_expression(condition)?;
        Ok(ConditionalJump {
            position: self.emit(OpCode::OpJumpNotTruthy, &[9999]),
            leaves_condition_on_jump: true,
            first_operand: None,
        })
    }

    fn effect_constraint_origin(
        &self,
        function: &Expression,
        expected_row: Option<String>,
    ) -> EffectConstraintOrigin {
        let call_name = self.call_function_name(function);
        let mut origin = EffectConstraintOrigin::new(
            function.span(),
            format!("this call to `{call_name}` creates the effect obligation"),
            format!("constraint source: effect-row checking for `{call_name}`"),
        );
        if let Some(expected_row) = expected_row {
            origin = origin.with_expected_row(expected_row);
        }
        origin
    }

    fn effect_operation_function_parts<'a>(
        &'a self,
        effect: Symbol,
        op: Symbol,
        span: Span,
        context: &str,
    ) -> CompileResult<(&'a [TypeExpr], &'a TypeExpr)> {
        let Some(signature) = self.effect_op_signature(effect, op) else {
            let effect_name = self.sym(effect).to_string();
            let op_name = self.sym(op).to_string();
            return Err(Self::boxed(
                Diagnostic::make_error_dynamic(
                    "E404",
                    "UNKNOWN EFFECT OPERATION",
                    ErrorType::Compiler,
                    format!(
                        "Effect `{}` has no declared operation `{}`.",
                        effect_name, op_name
                    ),
                    Some("Add the operation to the effect declaration or rename it.".to_string()),
                    self.file_path.clone(),
                    span,
                )
                .with_primary_label(span, "unknown operation in effect declaration lookup"),
            ));
        };

        let TypeExpr::Function {
            params,
            ret,
            effects: _,
            span: _,
        } = signature
        else {
            return Err(Self::boxed(
                Diagnostic::make_error_dynamic(
                    "E300",
                    "TYPE UNIFICATION ERROR",
                    ErrorType::Compiler,
                    format!(
                        "Effect operation `{}` in `{}` must use a function type declaration for {}.",
                        self.sym(op),
                        self.sym(effect),
                        context
                    ),
                    Some("Declare operations as function types (for example `op: A -> B`).".to_string()),
                    self.file_path.clone(),
                    span,
                )
                .with_primary_label(span, "invalid effect operation signature"),
            ));
        };

        Ok((params, ret))
    }

    pub(super) fn compile_expression(&mut self, expression: &Expression) -> CompileResult<()> {
        let previous_span = self.current_span;
        self.current_span = Some(expression.span());
        match expression {
            Expression::Integer { value, .. } => {
                let idx = self.add_constant(Value::Integer(*value));
                self.emit_constant_index(idx);
            }
            Expression::Float { value, .. } => {
                let idx = self.add_constant(Value::Float(*value));
                self.emit_constant_index(idx);
            }
            Expression::String { value, .. } => {
                let idx = self.add_constant(Value::String(value.clone().into()));
                self.emit_constant_index(idx);
            }
            Expression::InterpolatedString { parts, .. } => {
                self.compile_interpolated_string(parts)?;
            }
            Expression::Boolean { value, .. } => {
                if *value {
                    self.emit(OpCode::OpTrue, &[]);
                } else {
                    self.emit(OpCode::OpFalse, &[]);
                }
            }
            Expression::Identifier { name, span, .. } => {
                let name = *name;
                // Local/lexically visible bindings must shadow names imported
                // via `import M exposing (..)`.
                if let Some(symbol) = self.resolve_visible_symbol(name) {
                    if !self.try_emit_consumed_local(name) {
                        self.load_symbol(&symbol);
                    }
                } else if let Some(&qualified) = self.exposed_bindings.get(&name) {
                    // Unqualified access to an exposed module member.
                    if let Some(symbol) = self.resolve_visible_symbol(qualified) {
                        self.load_symbol(&symbol);
                    } else {
                        let name_str = self.sym(name);
                        return Err(Self::boxed(
                            self.make_undefined_variable_error(name_str, *span),
                        ));
                    }
                } else if let Some(prefix) = self.current_module_prefix {
                    let qualified = self.interner.intern_join(prefix, name);
                    if let Some(symbol) = self.resolve_visible_symbol(qualified) {
                        if !self.try_emit_consumed_local(qualified) {
                            self.load_symbol(&symbol);
                        }
                    } else if let Some(constant_value) = self.module_constants.get(&qualified) {
                        // Module constant - inline the value
                        self.emit_constant_value(constant_value.clone());
                    } else if let Some(info) = self.adt_registry.lookup_constructor(name) {
                        // Zero-arg ADT constructor used inside a module (e.g. `Dot`, `Leaf`)
                        if info.arity != 0 {
                            let name_str = self.interner.resolve(name).to_string();
                            return Err(Self::boxed(
                                diagnostic_for(&CONSTRUCTOR_ARITY_MISMATCH)
                                    .with_span(*span)
                                    .with_message(format!(
                                        "Constructor `{}` expects {} argument(s) but got 0.",
                                        name_str, info.arity
                                    )),
                            ));
                        }

                        let constructor_name = self.interner.resolve(name).to_string();
                        let const_idx =
                            self.add_constant(Value::String(Rc::new(constructor_name.clone())));
                        self.emit(OpCode::OpMakeAdt, &[const_idx, 0]);
                    } else {
                        if let Some((member_name, qualifier)) =
                            self.module_constructor_boundary_from_qualified_identifier(name)
                        {
                            if self.strict_mode {
                                return Err(Self::boxed(
                                    cross_module_constructor_access_error(
                                        *span,
                                        member_name.as_str(),
                                        qualifier.as_str(),
                                    )
                                    .with_file(self.file_path.clone()),
                                ));
                            }
                            self.warnings.push(
                                cross_module_constructor_access_warning(
                                    *span,
                                    member_name.as_str(),
                                    qualifier.as_str(),
                                )
                                .with_file(self.file_path.clone()),
                            );
                        }
                        let name_str = self.sym(name);
                        return Err(Self::boxed(
                            self.make_undefined_variable_error(name_str, *span),
                        ));
                    }
                } else if let Some(info) = self.adt_registry.lookup_constructor(name) {
                    // Zero-arg ADT constructor used as a value (e.g. `Point`, `None_`)
                    if info.arity != 0 {
                        let name_str = self.interner.resolve(name).to_string();
                        return Err(Self::boxed(
                            diagnostic_for(&CONSTRUCTOR_ARITY_MISMATCH)
                                .with_span(*span)
                                .with_message(format!(
                                    "Constructor `{}` expects {} argument(s) but got 0.",
                                    name_str, info.arity
                                )),
                        ));
                    }
                    let constructor_name = self.interner.resolve(name).to_string();
                    let const_idx =
                        self.add_constant(Value::String(Rc::new(constructor_name.clone())));
                    self.emit(OpCode::OpMakeAdt, &[const_idx, 0]);
                } else {
                    if let Some((member_name, qualifier)) =
                        self.module_constructor_boundary_from_qualified_identifier(name)
                    {
                        if self.strict_mode {
                            return Err(Self::boxed(
                                cross_module_constructor_access_error(
                                    *span,
                                    member_name.as_str(),
                                    qualifier.as_str(),
                                )
                                .with_file(self.file_path.clone()),
                            ));
                        }
                        self.warnings.push(
                            cross_module_constructor_access_warning(
                                *span,
                                member_name.as_str(),
                                qualifier.as_str(),
                            )
                            .with_file(self.file_path.clone()),
                        );
                    }
                    let name_str = self.sym(name);
                    return Err(Self::boxed(
                        self.make_undefined_variable_error(name_str, *span),
                    ));
                }
            }
            Expression::Prefix {
                operator, right, ..
            } => {
                self.validate_prefix_operator_types(operator, right)?;
                self.compile_non_tail_expression(right)?;
                match operator.as_str() {
                    "!" => self.emit(OpCode::OpBang, &[]),
                    "-" => self.emit(OpCode::OpMinus, &[]),
                    _ => {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &UNKNOWN_PREFIX_OPERATOR,
                            &[operator],
                            self.file_path.clone(),
                            expression.span(),
                        )));
                    }
                };
            }
            Expression::Infix {
                left,
                operator,
                right,
                ..
            } => {
                self.validate_infix_operator_types(left, operator, right)?;

                if operator == "<" {
                    self.compile_non_tail_expression(right)?;
                    self.compile_non_tail_expression(left)?;
                    self.emit(OpCode::OpGreaterThan, &[]);
                    return Ok(());
                }

                if operator == "<=" {
                    self.compile_non_tail_expression(left)?;
                    self.compile_non_tail_expression(right)?;
                    self.emit(OpCode::OpLessThanOrEqual, &[]);
                    return Ok(());
                }
                // a && b: if a is falsy, result is a (short-circuit); otherwise result is b
                // OpJumpNotTruthy: peeks value, jumps if falsy (keeps value), pops if truthy
                if operator == "&&" {
                    self.compile_non_tail_expression(left)?;
                    let jump_pos = self.emit(OpCode::OpJumpNotTruthy, &[9999]);
                    self.compile_non_tail_expression(right)?;
                    self.change_operand(jump_pos, self.current_instructions().len());
                    return Ok(());
                }
                // a || b: if a is truthy, result is a (short-circuit); otherwise result is b
                // OpJumpTruthy: peeks value, jumps if truthy (keeps value), pops if falsy
                if operator == "||" {
                    self.compile_non_tail_expression(left)?;
                    let jump_pos = self.emit(OpCode::OpJumpTruthy, &[9999]);
                    self.compile_non_tail_expression(right)?;
                    self.change_operand(jump_pos, self.current_instructions().len());
                    return Ok(());
                }

                self.compile_non_tail_expression(left)?;
                self.compile_non_tail_expression(right)?;

                match operator.as_str() {
                    "+" => self.emit(OpCode::OpAdd, &[]),
                    "-" => self.emit(OpCode::OpSub, &[]),
                    "*" => self.emit(OpCode::OpMul, &[]),
                    "/" => self.emit(OpCode::OpDiv, &[]),
                    "%" => self.emit(OpCode::OpMod, &[]),
                    "==" => self.emit(OpCode::OpEqual, &[]),
                    "!=" => self.emit(OpCode::OpNotEqual, &[]),
                    ">" => self.emit(OpCode::OpGreaterThan, &[]),
                    ">=" => self.emit(OpCode::OpGreaterThanOrEqual, &[]),
                    _ => {
                        return Err(Self::boxed(
                            Diagnostic::make_error(
                                &UNKNOWN_INFIX_OPERATOR,
                                &[operator],
                                self.file_path.clone(),
                                expression.span(),
                            )
                            .with_secondary_label(left.span(), "left operand")
                            .with_secondary_label(right.span(), "right operand"),
                        ));
                    }
                };
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                self.compile_if_expression(condition, consequence, alternative)?;
            }
            Expression::DoBlock { block, .. } => {
                self.compile_block_with_tail(block)?;
                if !self.block_has_value_tail(block) {
                    self.emit(OpCode::OpNone, &[]);
                }
            }
            Expression::Function {
                parameters,
                parameter_types,
                return_type,
                effects,
                body,
                ..
            } => {
                self.compile_function_literal(
                    parameters,
                    parameter_types,
                    return_type,
                    effects,
                    body,
                )?;
            }
            Expression::ListLiteral { elements, .. } => {
                // Build cons-list: push elements forward, then EmptyList,
                // then OpCons N times (right-to-left construction).
                for element in elements {
                    self.compile_non_tail_expression(element)?;
                }
                self.emit_constant_value(Value::EmptyList);
                for _ in 0..elements.len() {
                    self.emit(OpCode::OpCons, &[]);
                }
            }
            Expression::ArrayLiteral { elements, .. } => {
                for element in elements {
                    self.compile_non_tail_expression(element)?;
                }
                self.emit_array_count(elements.len());
            }
            Expression::TupleLiteral { elements, .. } => {
                for element in elements {
                    self.compile_non_tail_expression(element)?;
                }
                self.emit_tuple_count(elements.len());
            }
            Expression::EmptyList { .. } => {
                self.emit_constant_value(Value::EmptyList);
            }
            Expression::Hash { pairs, .. } => {
                let mut sorted_pairs: Vec<_> = pairs.iter().collect();
                sorted_pairs.sort_by(|a, b| a.0.to_string().cmp(&b.0.to_string()));

                for (key, value) in sorted_pairs {
                    self.compile_non_tail_expression(key)?;
                    self.compile_non_tail_expression(value)?;
                }
                self.emit_hash_count(pairs.len() * 2);
            }
            Expression::Index { left, index, .. } => {
                self.validate_index_expression_types(left, index)?;
                if let Expression::Identifier { name, .. } = left.as_ref()
                    && let Some(binding) = self.resolve_visible_symbol(*name)
                    && binding.symbol_scope == SymbolScope::Local
                {
                    self.compile_non_tail_expression(index)?;
                    self.emit(OpCode::OpGetLocalIndex, &[binding.index]);
                } else {
                    self.compile_non_tail_expression(left)?;
                    self.compile_non_tail_expression(index)?;
                    self.emit(OpCode::OpIndex, &[]);
                }
            }
            // Note: Pipe operator (|>) is handled at parse time by transforming
            // `a |> f(b, c)` into `f(a, b, c)` - a regular Call expression.
            // No special compilation needed here.
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                self.check_known_call_arity(expression.span(), function, arguments)?;
                self.check_static_contract_call(function, arguments)?;

                if self.try_emit_adt_constructor_call(function, arguments, expression.span())? {
                    self.current_span = previous_span;
                    return Ok(());
                }

                if self.try_emit_primop_call(function, arguments)? {
                    self.current_span = previous_span;
                    return Ok(());
                }

                // Phase 4 Step 5: compile-time class method dispatch.
                // If the callee is a class method with a known argument type,
                // compile a call to the mangled instance function directly.
                if let Expression::Identifier { name, .. } = function.as_ref()
                    && let Some(mangled) = self.try_resolve_class_method_call(*name, arguments)
                {
                    let mut resolved_args = self.resolve_direct_class_call_dict_args_ast(
                        *name,
                        arguments,
                        function.span(),
                    );
                    resolved_args.extend(arguments.clone());
                    let mangled_expr = Expression::Identifier {
                        name: mangled,
                        span: function.span(),
                        id: crate::syntax::expression::ExprId::UNSET,
                    };
                    let call = Expression::Call {
                        function: Box::new(mangled_expr),
                        arguments: resolved_args,
                        span: expression.span(),
                        id: crate::syntax::expression::ExprId::UNSET,
                    };
                    self.compile_non_tail_expression(&call)?;
                    self.current_span = previous_span;
                    return Ok(());
                }

                // Proposal 0151, Phase 1a (commit #6): module-qualified class
                // method dispatch. `Module.method(...)` where `Module` resolves
                // to a module and `method` is a class method should lower to
                // the same mangled `__tc_*` call as the bare-name form. The
                // class environment is global (not yet keyed on `ClassId`), so
                // we only need the method name to find the mangled function.
                if let Expression::MemberAccess { object, member, .. } = function.as_ref()
                    && self.resolve_module_name_from_expr(object).is_some()
                    && let Some(mangled) = self.try_resolve_class_method_call(*member, arguments)
                {
                    let mut resolved_args = self.resolve_direct_class_call_dict_args_ast(
                        *member,
                        arguments,
                        function.span(),
                    );
                    resolved_args.extend(arguments.clone());
                    let mangled_expr = Expression::Identifier {
                        name: mangled,
                        span: function.span(),
                        id: crate::syntax::expression::ExprId::UNSET,
                    };
                    let call = Expression::Call {
                        function: Box::new(mangled_expr),
                        arguments: resolved_args,
                        span: expression.span(),
                        id: crate::syntax::expression::ExprId::UNSET,
                    };
                    self.compile_non_tail_expression(&call)?;
                    self.current_span = previous_span;
                    return Ok(());
                }

                if let Expression::Identifier { name, span, .. } = function.as_ref()
                    && let Some(dict_call) = self.try_build_dict_class_method_call(
                        *name,
                        *span,
                        arguments,
                        expression.span(),
                    )
                {
                    self.compile_non_tail_expression(&dict_call)?;
                    self.current_span = previous_span;
                    return Ok(());
                }

                let is_direct_self_call = self.is_self_call(function);
                let is_self_tail_call = self.in_tail_position && is_direct_self_call;
                let is_self_non_tail_call = !self.in_tail_position && is_direct_self_call;

                if !is_self_tail_call
                    && !is_self_non_tail_call
                    && !self.in_tail_position
                    && arguments.len() == 1
                    && let Expression::Identifier { name, .. } = function.as_ref()
                    && let Some(binding) = self.resolve_visible_symbol(*name)
                    && binding.symbol_scope == SymbolScope::Local
                {
                    self.compile_non_tail_expression(&arguments[0])?;
                    self.emit(OpCode::OpGetLocalCall1, &[binding.index]);
                    self.current_span = previous_span;
                    return Ok(());
                }

                if !is_self_non_tail_call {
                    self.compile_non_tail_expression(function)?;
                }

                let mut consumable_counts: HashMap<Symbol, usize> = HashMap::new();

                if is_self_tail_call {
                    for argument in arguments {
                        self.collect_consumable_param_uses(argument, &mut consumable_counts);
                    }
                }

                for argument in arguments {
                    if is_self_tail_call {
                        self.compile_tail_call_argument(argument, &consumable_counts)?;
                    } else {
                        self.compile_non_tail_expression(argument)?;
                    }
                }

                // Emit OpTailCall for tail-position calls (self or sibling),
                // otherwise OpCall. OpTailCall reuses the current stack frame
                // and works for any callee (self-recursive, mutual, or any
                // known function).
                if is_self_tail_call || self.in_tail_position {
                    self.emit(OpCode::OpTailCall, &[arguments.len()]);
                } else if is_self_non_tail_call {
                    self.emit(OpCode::OpCallSelf, &[arguments.len()]);
                } else {
                    self.emit(OpCode::OpCall, &[arguments.len()]);
                }
            }
            Expression::MemberAccess { object, member, .. } => {
                let expr_span = expression.span();
                let member = *member;

                let module_binding_name = match object.as_ref() {
                    Expression::Identifier { name, .. } => Some(*name),
                    _ => None,
                };
                let module_name = self.resolve_module_name_from_expr(object);

                if let Some(module_name) = module_name {
                    if let Some(binding_name) = module_binding_name
                        && self
                            .imported_module_exclusions
                            .get(&binding_name)
                            .is_some_and(|excluded| excluded.contains(&member))
                    {
                        let module_name_str = self.sym(module_name);
                        let member_str = self.sym(member);
                        return Err(Self::boxed(Diagnostic::make_error(
                            &UNKNOWN_MODULE_MEMBER,
                            &[module_name_str, member_str],
                            self.file_path.clone(),
                            expr_span,
                        )));
                    }

                    let member_str = self.sym(member);
                    self.check_private_member(member_str, expr_span, Some(self.sym(module_name)))?;
                    if self.current_module_prefix != Some(module_name)
                        && self.module_member_function_is_public(module_name, member) == Some(false)
                    {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &PRIVATE_MEMBER,
                            &[member_str],
                            self.file_path.clone(),
                            expr_span,
                        )));
                    }

                    let qualified = self.interner.intern_join(module_name, member);
                    // Module Constants check if this is a compile-time constant
                    // If so, inline the constant value directly instead of loading from symbol
                    if let Some(constant_value) = self.module_constants.get(&qualified) {
                        self.emit_constant_value(constant_value.clone());
                        return Ok(());
                    }

                    if let Some(symbol) = self.resolve_visible_symbol(qualified) {
                        self.load_symbol(&symbol);
                        return Ok(());
                    }

                    let module_name_str = self.sym(module_name).to_string();
                    let member_str = self.sym(member).to_string();
                    if self.current_module_prefix != Some(module_name)
                        && self
                            .module_member_adt_constructor_owner(module_name, member)
                            .is_some()
                    {
                        if self.strict_mode {
                            return Err(Self::boxed(
                                cross_module_constructor_access_error(
                                    expr_span,
                                    member_str.as_str(),
                                    module_name_str.as_str(),
                                )
                                .with_file(self.file_path.clone()),
                            ));
                        }
                        self.warnings.push(
                            cross_module_constructor_access_warning(
                                expr_span,
                                member_str.as_str(),
                                module_name_str.as_str(),
                            )
                            .with_file(self.file_path.clone()),
                        );
                    }

                    return Err(Self::boxed(Diagnostic::make_error(
                        &UNKNOWN_MODULE_MEMBER,
                        &[module_name_str.as_str(), member_str.as_str()],
                        self.file_path.clone(),
                        expr_span,
                    )));
                }

                if let Some(name) = module_binding_name
                    && module_name.is_none()
                    && is_valid_module_name(self.sym(name))
                {
                    let has_symbol = self.resolve_visible_symbol(name).is_some();
                    if !has_symbol {
                        let name_str = self.sym(name);
                        return Err(Self::boxed(Diagnostic::make_error(
                            &MODULE_NOT_IMPORTED,
                            &[name_str],
                            self.file_path.clone(),
                            expr_span,
                        )));
                    }
                }

                let member_str = self.sym(member);
                self.check_private_member(member_str, expr_span, None)?;

                // Compile the object (e.g., the module identifier)
                self.compile_non_tail_expression(object)?;

                // Emit the member name as a string constant (the hash key)
                let member_str = self.sym(member).to_string();
                let member_idx = self.add_constant(Value::String(member_str.into()));
                self.emit_constant_index(member_idx);

                // Use index operation to access the member from the hash
                self.emit(OpCode::OpIndex, &[]);
                // Member access should yield the value, not Option.
                self.emit(OpCode::OpUnwrapSome, &[]);
            }
            Expression::TupleFieldAccess { object, index, .. } => {
                self.compile_non_tail_expression(object)?;
                self.emit(OpCode::OpTupleIndex, &[*index]);
            }
            Expression::None { .. } => {
                self.emit(OpCode::OpNone, &[]);
            }
            Expression::Some { value, .. } => {
                self.compile_non_tail_expression(value)?;
                self.emit(OpCode::OpSome, &[]);
            }
            // Either type expressions
            Expression::Left { value, .. } => {
                self.compile_non_tail_expression(value)?;
                self.emit(OpCode::OpLeft, &[]);
            }
            Expression::Right { value, .. } => {
                self.compile_non_tail_expression(value)?;
                self.emit(OpCode::OpRight, &[]);
            }
            Expression::Match {
                scrutinee,
                arms,
                span,
                ..
            } => {
                self.compile_match_expression(scrutinee, arms, *span)?;
            }
            Expression::Cons { head, tail, .. } => {
                if let Expression::None { span, .. } = tail.as_ref() {
                    return Err(Self::boxed(Diagnostic::make_error(
                        &LEGACY_LIST_TAIL_NONE,
                        &[],
                        self.file_path.clone(),
                        *span,
                    )));
                }
                self.compile_non_tail_expression(head)?;
                self.compile_non_tail_expression(tail)?;
                self.emit(OpCode::OpCons, &[]);
            }
            Expression::Perform {
                effect,
                operation,
                args,
                ..
            } => {
                self.compile_perform(*effect, *operation, args)?;
            }
            Expression::Handle {
                expr, effect, arms, ..
            } => {
                self.compile_handle(expr, *effect, arms)?;
            }
        }
        self.current_span = previous_span;
        Ok(())
    }

    fn check_static_contract_call(
        &mut self,
        function: &Expression,
        arguments: &[Expression],
    ) -> CompileResult<()> {
        self.check_direct_builtin_effect_call(function)?;

        let Some(contract) = self
            .resolve_call_contract(function, arguments.len())
            .cloned()
        else {
            return Ok(());
        };

        if !contract.effects.is_empty() {
            let required_row = EffectRow::from_effect_exprs(&contract.effects);
            let constraints = self.collect_effect_row_constraints(&contract, arguments);
            let solution = solve_row_constraints(&constraints);

            if let Some(first_violation) = solution.violations.first() {
                return Err(Self::boxed(
                    self.diagnostic_for_row_violation(function, first_violation),
                ));
            }

            let unresolved: Vec<Symbol> = required_row
                .unresolved_vars(&solution)
                .into_iter()
                .filter(|effect_var| !self.is_effect_available(*effect_var))
                .collect();

            if !unresolved.is_empty() {
                let origin = self.effect_constraint_origin(function, None);
                return Err(Self::boxed(self.unresolved_effect_vars_diagnostic(
                    &unresolved,
                    function.span(),
                    &origin,
                )));
            }

            let mut required_effects: Vec<Symbol> = required_row
                .concrete_effects(&solution)
                .into_iter()
                .collect();
            required_effects.sort_by_key(|symbol| self.sym(*symbol).to_string());

            for required_name in required_effects {
                if !self.is_effect_available(required_name) {
                    let function_name = match function {
                        Expression::Identifier { name, .. } => self.sym(*name).to_string(),
                        Expression::MemberAccess { member, .. } => self.sym(*member).to_string(),
                        _ => "<call>".to_string(),
                    };
                    let missing = self.sym(required_name).to_string();
                    return Err(Self::boxed(
                        Diagnostic::make_error_dynamic(
                            "E400",
                            "MISSING EFFECT",
                            ErrorType::Compiler,
                            format!(
                                "Call to `{}` requires effect `{}` in this function signature.",
                                function_name, missing
                            ),
                            Some(format!("Add `with {}` to the enclosing function.", missing)),
                            self.file_path.clone(),
                            function.span(),
                        )
                        .with_display_title("Missing Ambient Effect")
                        .with_category(DiagnosticCategory::Effects)
                        .with_phase(crate::diagnostics::DiagnosticPhase::Effect)
                        .with_primary_label(function.span(), "effectful call occurs here"),
                    ));
                }
            }
        }

        let function_name = self.call_function_name(function);
        let def_span = self.call_definition_span(function);

        for (index, argument) in arguments.iter().enumerate() {
            let Some(expected_ty) = contract.params.get(index).and_then(|p| p.as_ref()) else {
                continue;
            };
            let Some(expected_runtime) = convert_type_expr(expected_ty, &self.interner) else {
                if self.strict_mode && !matches!(expected_ty, TypeExpr::Function { .. }) {
                    return Err(Self::boxed(
                        Diagnostic::make_error_dynamic(
                            "E425",
                            "STRICT UNRESOLVED BOUNDARY TYPE",
                            ErrorType::Compiler,
                            format!(
                                "Strict mode cannot enforce runtime boundary check for unresolved parameter type `{}`.",
                                expected_ty
                            ),
                            Some(
                                "Use a concrete parameter type (avoid unresolved generic-only boundary types) or make this API internal."
                                    .to_string(),
                            ),
                            self.file_path.clone(),
                            argument.span(),
                        )
                        .with_display_title("Unresolved Boundary Type")
                        .with_category(DiagnosticCategory::TypeInference)
                        .with_primary_label(
                            argument.span(),
                            "runtime boundary check is unresolved in strict mode",
                        ),
                    ));
                }
                // Stage 2 (0051): when the contract has no concrete RuntimeType (e.g. generic
                // param T or user ADT), fall back to HM's call-site-instantiated function type.
                // Only fires when HM has fully resolved the function's param type and the
                // argument type, and they don't match. Skipped if HM already emitted E300 for
                // this argument to avoid duplicate diagnostics.
                if !self.type_error_already_reported_for(argument) {
                    use super::hm_expr_typer::HmExprTypeResult;
                    if let HmExprTypeResult::Known(InferType::Fun(hm_params, _, _)) =
                        self.hm_expr_type_strict_path(function)
                        && let Some(hm_expected) = hm_params.get(index)
                        && hm_expected.free_vars().is_empty()
                        && !hm_expected.contains_any()
                        && let HmExprTypeResult::Known(actual) =
                            self.hm_expr_type_strict_path(argument)
                        && actual.free_vars().is_empty()
                        && !actual.contains_any()
                    {
                        let types_match = if let Ok(subst) = unify(hm_expected, &actual) {
                            hm_expected.apply_type_subst(&subst) == actual.apply_type_subst(&subst)
                        } else {
                            false
                        };
                        if !types_match {
                            let expected_str = display_infer_type(hm_expected, &self.interner);
                            let actual_str = display_infer_type(&actual, &self.interner);
                            return Err(Self::boxed(call_arg_type_mismatch(
                                self.file_path.clone(),
                                argument.span(),
                                Some(&function_name),
                                index + 1,
                                def_span,
                                &expected_str,
                                &actual_str,
                            )));
                        }
                    }
                }
                continue;
            };
            let expected_infer = TypeEnv::infer_type_from_runtime(&expected_runtime);
            let maybe_contextual = match self.hm_expr_type_strict_path(argument) {
                super::hm_expr_typer::HmExprTypeResult::Known(actual) => {
                    if expected_infer.is_concrete()
                        && actual.is_concrete()
                        && !expected_infer.contains_any()
                        && !actual.contains_any()
                    {
                        let compatible = if let Ok(subst) = unify(&expected_infer, &actual) {
                            expected_infer.apply_type_subst(&subst)
                                == actual.apply_type_subst(&subst)
                        } else {
                            false
                        };
                        if compatible {
                            None
                        } else {
                            let expected_str = display_infer_type(&expected_infer, &self.interner);
                            let actual_str = display_infer_type(&actual, &self.interner);
                            Some(call_arg_type_mismatch(
                                self.file_path.clone(),
                                argument.span(),
                                Some(&function_name),
                                index + 1,
                                def_span,
                                &expected_str,
                                &actual_str,
                            ))
                        }
                    } else {
                        None
                    }
                }
                _ => None,
            };
            if let Some(diag) = maybe_contextual {
                return Err(Self::boxed(diag));
            }
            self.validate_expr_expected_type_with_policy(
                &expected_infer,
                argument,
                "argument type is known at compile time",
                format!("argument #{} does not match function contract", index + 1),
                "function contract argument",
                true,
            )?;
        }

        Ok(())
    }

    fn call_function_name(&self, function: &Expression) -> String {
        match function {
            Expression::Identifier { name, .. } => self.sym(*name).to_string(),
            Expression::MemberAccess { member, .. } => self.sym(*member).to_string(),
            _ => "<call>".to_string(),
        }
    }

    fn call_definition_span(&mut self, function: &Expression) -> Option<Span> {
        match function {
            Expression::Identifier { name, .. } => {
                self.resolve_visible_symbol(*name).map(|b| b.span)
            }
            Expression::MemberAccess { object, member, .. } => {
                let module_name = self.resolve_module_name_from_expr(object)?;
                let qualified = self.interner.intern_join(module_name, *member);
                self.resolve_visible_symbol(qualified).map(|b| b.span)
            }
            _ => None,
        }
    }

    fn check_known_call_arity(
        &mut self,
        call_span: Span,
        function: &Expression,
        arguments: &[Expression],
    ) -> CompileResult<()> {
        use crate::bytecode::compiler::hm_expr_typer::HmExprTypeResult;

        let HmExprTypeResult::Known(InferType::Fun(params, _, _)) =
            self.hm_expr_type_strict_path(function)
        else {
            return Ok(());
        };

        let expected = params.len();
        let actual = arguments.len();
        if expected == actual {
            return Ok(());
        }

        let function_name = self.call_function_name(function);
        let def_span = self.call_definition_span(function);
        Err(Self::boxed(wrong_argument_count(
            self.file_path.clone(),
            call_span,
            &function_name,
            expected,
            actual,
            def_span,
        )))
    }

    fn module_constructor_boundary_from_qualified_identifier(
        &self,
        name: Symbol,
    ) -> Option<(String, String)> {
        let full_name = self.sym(name);
        let (qualifier, member) = full_name.rsplit_once('.')?;
        let module_name = self
            .imported_modules
            .iter()
            .copied()
            .find(|module| self.sym(*module) == qualifier)
            .or_else(|| {
                self.import_aliases
                    .iter()
                    .find(|(alias, _)| self.sym(**alias) == qualifier)
                    .map(|(_, target)| *target)
            })?;
        let is_adt_constructor = self
            .module_adt_constructors
            .keys()
            .any(|(owner, ctor)| *owner == module_name && self.sym(*ctor) == member);
        if !is_adt_constructor {
            return None;
        }

        Some((member.to_string(), qualifier.to_string()))
    }

    fn unresolved_effect_vars_diagnostic(
        &self,
        vars: &[Symbol],
        span: Span,
        origin: &EffectConstraintOrigin,
    ) -> Diagnostic {
        if vars.len() == 1 {
            let effect_name = self.sym(vars[0]).to_string();
            with_effect_constraint_origin(dynamic_explained_diagnostic(
                "E419",
                "UNRESOLVED EFFECT VARIABLE",
                format!(
                    "I cannot resolve the effect variable `{effect_name}` introduced by this call."
                ),
                self.file_path.clone(),
                span,
                "this call leaves an effect variable unconstrained",
                [format!("unresolved effect variable: {effect_name}")],
                format!(
                    "Add an explicit effect annotation such as `with {effect_name}` or pass a callback with concrete effects."
                ),
            ), origin)
            .with_display_title("Unresolved Effect Row")
            .with_category(DiagnosticCategory::Effects)
            .with_phase(crate::diagnostics::DiagnosticPhase::Effect)
        } else {
            let mut names: Vec<String> = vars
                .iter()
                .map(|symbol| self.sym(*symbol).to_string())
                .collect();
            names.sort();
            with_effect_constraint_origin(dynamic_explained_diagnostic(
                "E420",
                "AMBIGUOUS EFFECT VARIABLES",
                format!(
                    "I cannot determine which effects this call should carry because these effect variables stay ambiguous: {}.",
                    names.join(", ")
                ),
                self.file_path.clone(),
                span,
                "this call leaves multiple effect variables ambiguous",
                [format!("ambiguous effect variables: {}", names.join(", "))],
                "Add explicit `with ...` annotations or use callbacks with concrete effects to disambiguate.",
            ), origin)
            .with_display_title("Unresolved Effect Row")
            .with_category(DiagnosticCategory::Effects)
            .with_phase(crate::diagnostics::DiagnosticPhase::Effect)
        }
    }

    fn diagnostic_for_row_violation(
        &self,
        function: &Expression,
        violation: &RowConstraintViolation,
    ) -> Diagnostic {
        let origin = self.effect_constraint_origin(function, None);
        match violation {
            RowConstraintViolation::InvalidSubtract { atom } => {
                let effect_name = self.sym(*atom).to_string();
                with_effect_constraint_origin(
                    dynamic_explained_diagnostic(
                        "E421",
                        "INVALID EFFECT SUBTRACTION",
                        format!("I cannot subtract effect `{effect_name}` from this effect row."),
                        self.file_path.clone(),
                        function.span(),
                        "this call violates an effect-row subtraction constraint",
                        [format!("requested subtraction: {effect_name}")],
                        "Handle or include this effect before subtracting it from an effect row.",
                    ),
                    &origin,
                )
                .with_display_title("Effect Requirement Mismatch")
                .with_category(DiagnosticCategory::Effects)
                .with_phase(crate::diagnostics::DiagnosticPhase::Effect)
            }
            RowConstraintViolation::UnresolvedVars { vars } => {
                self.unresolved_effect_vars_diagnostic(vars, function.span(), &origin)
            }
            RowConstraintViolation::UnsatisfiedSubset { missing } => {
                let mut names: Vec<String> = missing
                    .iter()
                    .map(|effect| self.sym(*effect).to_string())
                    .collect();
                names.sort();
                with_effect_constraint_origin(dynamic_explained_diagnostic(
                    "E422",
                    "UNSATISFIED EFFECT SUBSET",
                    format!(
                        "This call requires effects that are missing from the surrounding effect row: {}.",
                        names.join(", ")
                    ),
                    self.file_path.clone(),
                    function.span(),
                    "this call needs effects that are not currently available",
                    [format!("missing required effects: {}", names.join(", "))],
                    "Add the missing effects to the enclosing function or handle them before this call.",
                ), &origin)
                .with_display_title("Effect Requirement Mismatch")
                .with_category(DiagnosticCategory::Effects)
                .with_phase(crate::diagnostics::DiagnosticPhase::Effect)
            }
        }
    }

    fn check_direct_builtin_effect_call(&mut self, function: &Expression) -> CompileResult<()> {
        let required_name = match function {
            Expression::Identifier { name, .. } => self
                .lookup_effect_alias(*name)
                .map(|effect| self.sym(effect).to_string()),
            _ => None,
        };

        let Some(required_name) = required_name else {
            return Ok(());
        };

        if self.is_effect_available_name(required_name.as_str()) {
            return Ok(());
        }

        let function_name = match function {
            Expression::Identifier { name, .. } => self.sym(*name).to_string(),
            _ => "<call>".to_string(),
        };
        Err(Self::boxed(
            Diagnostic::make_error_dynamic(
                "E400",
                "MISSING EFFECT",
                ErrorType::Compiler,
                format!(
                    "Call to `{}` requires effect `{}` in this function signature.",
                    function_name, required_name
                ),
                Some(format!(
                    "Add `with {}` to the enclosing function.",
                    required_name
                )),
                self.file_path.clone(),
                function.span(),
            )
            .with_display_title("Missing Ambient Effect")
            .with_category(DiagnosticCategory::Effects)
            .with_phase(crate::diagnostics::DiagnosticPhase::Effect)
            .with_primary_label(function.span(), "effectful call occurs here"),
        ))
    }

    pub(super) fn required_effect_for_base_name(&self, base_name: &str) -> Option<&'static str> {
        match base_name {
            "print" | "read_file" | "read_lines" | "read_stdin" => Some("IO"),
            "now" | "clock_now" | "now_ms" | "time" => Some("Time"),
            _ => None,
        }
    }

    fn collect_effect_row_constraints(
        &mut self,
        contract: &FnContract,
        arguments: &[Expression],
    ) -> Vec<RowConstraint> {
        let mut constraints = Vec::new();

        for (idx, argument) in arguments.iter().enumerate() {
            let Some(Some(TypeExpr::Function {
                params,
                effects: param_effects,
                ..
            })) = contract.params.get(idx)
            else {
                continue;
            };

            let expected = EffectRow::from_effect_exprs(param_effects);
            let Some(actual) = self.infer_argument_function_effect_row(argument, params.len())
            else {
                // Keep current permissive behavior when argument effect info is unavailable.
                continue;
            };

            constraints.push(RowConstraint::Eq(expected.clone(), actual.clone()));
            constraints.push(RowConstraint::Subset(expected, actual.clone()));
            for effect in param_effects {
                self.collect_effect_expr_absence_constraints(effect, &actual, &mut constraints);
            }
        }

        constraints
    }

    pub(super) fn collect_effect_expr_absence_constraints(
        &self,
        effect: &EffectExpr,
        actual: &EffectRow,
        constraints: &mut Vec<RowConstraint>,
    ) {
        match effect {
            EffectExpr::Named { .. } | EffectExpr::RowVar { .. } => {}
            EffectExpr::Add { left, right, .. } => {
                self.collect_effect_expr_absence_constraints(left, actual, constraints);
                self.collect_effect_expr_absence_constraints(right, actual, constraints);
            }
            EffectExpr::Subtract { left, right, .. } => {
                self.collect_effect_expr_absence_constraints(left, actual, constraints);
                self.collect_effect_expr_absence_constraints(right, actual, constraints);

                let right_row = EffectRow::from_effect_expr(right);

                for atom in right_row.atoms {
                    constraints.push(RowConstraint::Absent(actual.clone(), atom));
                }
            }
        }
    }

    fn infer_argument_function_effect_row(
        &mut self,
        argument: &Expression,
        expected_arity: usize,
    ) -> Option<EffectRow> {
        match argument {
            Expression::Function { effects, .. } => Some(EffectRow::from_effect_exprs(effects)),
            Expression::Identifier { name, .. } => {
                if let Some(local) = self.current_function_param_effect_row(*name) {
                    return Some(local);
                }
                self.lookup_unqualified_contract(*name, expected_arity)
                    .map(|contract| EffectRow::from_effect_exprs(&contract.effects))
                    .or_else(|| self.infer_argument_effect_row_from_hm(argument))
            }
            Expression::MemberAccess { object, member, .. } => {
                let module_name = self.resolve_module_name_from_expr(object);
                module_name
                    .and_then(|module_name| {
                        self.lookup_contract(Some(module_name), *member, expected_arity)
                    })
                    .map(|contract| EffectRow::from_effect_exprs(&contract.effects))
                    .or_else(|| self.infer_argument_effect_row_from_hm(argument))
            }
            _ => self.infer_argument_effect_row_from_hm(argument),
        }
    }

    /// Like `collect_effect_row_constraints`, but uses explicit `param_effect_rows`
    /// instead of reading from the function context stack. Used by pre-codegen
    /// validation before the function context has been pushed.
    pub(super) fn collect_effect_row_constraints_with_rows(
        &mut self,
        contract: &FnContract,
        arguments: &[Expression],
        param_effect_rows: &HashMap<Symbol, EffectRow>,
    ) -> Vec<RowConstraint> {
        let mut constraints = Vec::new();

        for (idx, argument) in arguments.iter().enumerate() {
            let Some(Some(TypeExpr::Function {
                params,
                effects: param_effects,
                ..
            })) = contract.params.get(idx)
            else {
                continue;
            };

            let expected = EffectRow::from_effect_exprs(param_effects);
            let Some(actual) = self.infer_argument_function_effect_row_with_rows(
                argument,
                params.len(),
                param_effect_rows,
            ) else {
                continue;
            };

            constraints.push(RowConstraint::Eq(expected.clone(), actual.clone()));
            constraints.push(RowConstraint::Subset(expected, actual.clone()));
            for effect in param_effects {
                self.collect_effect_expr_absence_constraints(effect, &actual, &mut constraints);
            }
        }

        constraints
    }

    /// Like `infer_argument_function_effect_row`, but uses explicit `param_effect_rows`
    /// instead of reading from the function context stack.
    fn infer_argument_function_effect_row_with_rows(
        &mut self,
        argument: &Expression,
        expected_arity: usize,
        param_effect_rows: &HashMap<Symbol, EffectRow>,
    ) -> Option<EffectRow> {
        match argument {
            Expression::Function { effects, .. } => Some(EffectRow::from_effect_exprs(effects)),
            Expression::Identifier { name, .. } => {
                if let Some(local) = param_effect_rows.get(name).cloned() {
                    return Some(local);
                }
                self.lookup_unqualified_contract(*name, expected_arity)
                    .map(|contract| EffectRow::from_effect_exprs(&contract.effects))
                    .or_else(|| self.infer_argument_effect_row_from_hm(argument))
            }
            Expression::MemberAccess { object, member, .. } => {
                let module_name = self.resolve_module_name_from_expr(object);
                module_name
                    .and_then(|module_name| {
                        self.lookup_contract(Some(module_name), *member, expected_arity)
                    })
                    .map(|contract| EffectRow::from_effect_exprs(&contract.effects))
                    .or_else(|| self.infer_argument_effect_row_from_hm(argument))
            }
            _ => self.infer_argument_effect_row_from_hm(argument),
        }
    }

    pub(super) fn resolve_call_contract<'a>(
        &'a self,
        function: &Expression,
        arity: usize,
    ) -> Option<&'a FnContract> {
        match function {
            Expression::Identifier { name, .. } => self.lookup_unqualified_contract(*name, arity),
            Expression::MemberAccess { object, member, .. } => {
                let module_name = self.resolve_module_name_from_expr(object)?;
                self.lookup_contract(Some(module_name), *member, arity)
            }
            _ => None,
        }
    }

    pub(super) fn validate_runtime_expected_type(
        &self,
        expected: &RuntimeType,
        expression: &Expression,
        primary_label: &str,
        help: String,
    ) -> CompileResult<()> {
        let expected_infer = TypeEnv::infer_type_from_runtime(expected);
        self.validate_expr_expected_type(
            &expected_infer,
            expression,
            primary_label,
            help,
            "runtime-typed expectation",
        )
    }

    fn validate_infix_operator_types(
        &self,
        left: &Expression,
        operator: &str,
        right: &Expression,
    ) -> CompileResult<()> {
        let crate::bytecode::compiler::hm_expr_typer::HmExprTypeResult::Known(left_ty) =
            self.hm_expr_type_strict_path(left)
        else {
            return Ok(());
        };
        let crate::bytecode::compiler::hm_expr_typer::HmExprTypeResult::Known(right_ty) =
            self.hm_expr_type_strict_path(right)
        else {
            return Ok(());
        };
        let is_num = |ty: &InferType| {
            matches!(
                ty,
                InferType::Con(
                    crate::types::type_constructor::TypeConstructor::Int
                        | crate::types::type_constructor::TypeConstructor::Float
                )
            )
        };
        let is_bool = |ty: &InferType| {
            *ty == InferType::Con(crate::types::type_constructor::TypeConstructor::Bool)
        };
        let is_int = |ty: &InferType| {
            *ty == InferType::Con(crate::types::type_constructor::TypeConstructor::Int)
        };
        let is_float = |ty: &InferType| {
            *ty == InferType::Con(crate::types::type_constructor::TypeConstructor::Float)
        };
        let is_string = |ty: &InferType| {
            *ty == InferType::Con(crate::types::type_constructor::TypeConstructor::String)
        };
        let op_compatible = match operator {
            "+" => {
                (is_int(&left_ty) && is_int(&right_ty))
                    || (is_float(&left_ty) && is_float(&right_ty))
                    || (is_string(&left_ty) && is_string(&right_ty))
            }
            "-" | "*" | "/" | "%" => is_num(&left_ty) && is_num(&right_ty),
            "&&" | "||" => is_bool(&left_ty) && is_bool(&right_ty),
            _ => true,
        };
        if op_compatible {
            return Ok(());
        }

        let expected = match operator {
            "+" => "matching '+' operands (Int+Int, Float+Float, or String+String)".to_string(),
            "-" | "*" | "/" | "%" => "numeric operands (Int or Float)".to_string(),
            "&&" | "||" => "Bool operands".to_string(),
            _ => return Ok(()),
        };
        let actual = format!(
            "{} and {}",
            TypeEnv::to_runtime(&left_ty, &TypeSubst::empty()).type_name(),
            TypeEnv::to_runtime(&right_ty, &TypeSubst::empty()).type_name()
        );
        let op_span = Span::new(left.span().start, right.span().end);

        Err(Self::boxed(
            type_unification_error(self.file_path.clone(), op_span, &expected, &actual)
                .with_secondary_label(op_span, "operator operands are known at compile time")
                .with_help("adjust operand types or add explicit conversion"),
        ))
    }

    fn validate_prefix_operator_types(
        &self,
        operator: &str,
        right: &Expression,
    ) -> CompileResult<()> {
        if operator != "-" {
            return Ok(());
        }
        let crate::bytecode::compiler::hm_expr_typer::HmExprTypeResult::Known(right_ty) =
            self.hm_expr_type_strict_path(right)
        else {
            return Ok(());
        };
        if matches!(
            right_ty,
            InferType::Con(
                crate::types::type_constructor::TypeConstructor::Int
                    | crate::types::type_constructor::TypeConstructor::Float
            )
        ) {
            return Ok(());
        }

        let actual = TypeEnv::to_runtime(&right_ty, &TypeSubst::empty()).type_name();
        Err(Self::boxed(
            type_unification_error(
                self.file_path.clone(),
                right.span(),
                "numeric operand (Int or Float)",
                &actual,
            )
            .with_secondary_label(right.span(), "unary '-' operand is known at compile time")
            .with_help("use a numeric operand or convert the value before applying unary '-'"),
        ))
    }

    fn validate_index_expression_types(
        &self,
        left: &Expression,
        index: &Expression,
    ) -> CompileResult<()> {
        let crate::bytecode::compiler::hm_expr_typer::HmExprTypeResult::Known(left_ty) =
            self.hm_expr_type_strict_path(left)
        else {
            return Ok(());
        };
        let index_known = matches!(
            self.hm_expr_type_strict_path(index),
            crate::bytecode::compiler::hm_expr_typer::HmExprTypeResult::Known(_)
        );

        match left_ty {
            InferType::App(
                crate::types::type_constructor::TypeConstructor::Array
                | crate::types::type_constructor::TypeConstructor::List,
                _,
            )
            | InferType::Tuple(_) => {
                if index_known {
                    self.validate_runtime_expected_type(
                        &RuntimeType::Int,
                        index,
                        "index expression is known at compile time",
                        "use an Int index for Array/List/Tuple access".to_string(),
                    )?;
                }
                Ok(())
            }
            InferType::App(crate::types::type_constructor::TypeConstructor::Map, _) => Ok(()),
            other => Err(Self::boxed(
                type_unification_error(
                    self.file_path.clone(),
                    left.span(),
                    "indexable value (Array/List/Tuple/Map)",
                    &TypeEnv::to_runtime(&other, &TypeSubst::empty()).type_name(),
                )
                .with_secondary_label(left.span(), "indexed value is known at compile time")
                .with_help("index only arrays, lists, tuples, or maps"),
            )),
        }
    }

    fn validate_boolean_expression(
        &self,
        expression: &Expression,
        context: &str,
    ) -> CompileResult<()> {
        self.validate_runtime_expected_type(
            &RuntimeType::Bool,
            expression,
            &format!("{context} is known at compile time"),
            "use a Bool expression, or make the condition/guard explicitly boolean".to_string(),
        )
    }

    pub(super) fn compile_function_literal(
        &mut self,
        parameters: &[Symbol],
        parameters_types: &[Option<TypeExpr>],
        return_type: &Option<TypeExpr>,
        effects: &[crate::syntax::effect_expr::EffectExpr],
        body: &Block,
    ) -> CompileResult<()> {
        if let Some(name) = Self::find_duplicate_name(parameters) {
            let name_str = self.sym(name);
            return Err(Self::boxed(Diagnostic::make_error(
                &DUPLICATE_PARAMETER,
                &[name_str],
                self.file_path.clone(),
                Span::new(Position::default(), Position::default()),
            )));
        }

        self.enter_scope();

        for (index, param) in parameters.iter().enumerate() {
            self.symbol_table.define(*param, Span::default());
            if let Some(Some(param_ty)) = parameters_types.get(index)
                && let Some(runtime_ty) = convert_type_expr(param_ty, &self.interner)
            {
                self.bind_static_type(*param, runtime_ty);
            }
        }

        let param_effect_rows = self.build_param_effect_rows(parameters, parameters_types);
        self.with_function_context_with_param_effect_rows(
            parameters.len(),
            effects,
            param_effect_rows,
            |compiler| compiler.compile_block_with_tail(body),
        )?;

        if self.block_has_value_tail(body) {
            if self.is_last_instruction(OpCode::OpPop) {
                self.replace_last_pop_with_return();
            } else if self.replace_last_local_read_with_return() {
            } else if !self.is_last_instruction(OpCode::OpReturnValue)
                && !self.is_last_instruction(OpCode::OpReturnLocal)
            {
                self.emit(OpCode::OpReturnValue, &[]);
            }
        } else if !self.is_last_instruction(OpCode::OpReturnValue)
            && !self.is_last_instruction(OpCode::OpReturnLocal)
        {
            self.emit(OpCode::OpReturn, &[]);
        }

        let free_symbols = self.symbol_table.free_symbols.clone();

        for free in &free_symbols {
            if free.symbol_scope == SymbolScope::Local {
                self.mark_captured_in_current_function(free.index);
            }
        }

        let num_locals = self.symbol_table.num_definitions;

        let (instructions, locations, files, effect_summary) = self.leave_scope();

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
                    span: body.span(),
                },
            )
        };
        let (files, boundary_location) = boundary_location;

        for free in &free_symbols {
            self.load_symbol(free);
        }

        let runtime_contract = {
            let contract = FnContract {
                type_params: vec![],
                params: parameters_types.to_vec(),
                ret: return_type.clone(),
                effects: effects.to_vec(),
            };
            self.to_runtime_contract(&contract)
        };

        let fn_idx = self.add_constant(Value::Function(Rc::new(
            CompiledFunction::new(
                instructions,
                num_locals,
                parameters.len(),
                Some(
                    FunctionDebugInfo::new(None, files, locations)
                        .with_boundary_location(Some(boundary_location))
                        .with_effect_summary(effect_summary),
                ),
            )
            .with_contract(runtime_contract),
        )));

        self.emit_closure_index(fn_idx, free_symbols.len());

        Ok(())
    }

    pub(super) fn compile_interpolated_string(
        &mut self,
        parts: &[StringPart],
    ) -> CompileResult<()> {
        if parts.is_empty() {
            // Empty interpolated string - just push an empty string
            let idx = self.add_constant(Value::String(String::new().into()));
            self.emit_constant_index(idx);
            return Ok(());
        }

        // Compile the first part
        match &parts[0] {
            StringPart::Literal(s) => {
                let idx = self.add_constant(Value::String(s.clone().into()));
                self.emit_constant_index(idx);
            }
            StringPart::Interpolation(expression) => {
                self.compile_non_tail_expression(expression)?;
                self.emit(OpCode::OpToString, &[]);
            }
        }

        // Compile remaining parts, concatenating each with OpAdd
        for part in &parts[1..] {
            match part {
                StringPart::Literal(s) => {
                    let idx = self.add_constant(Value::String(s.clone().into()));
                    self.emit_constant_index(idx);
                }
                StringPart::Interpolation(expression) => {
                    self.compile_non_tail_expression(expression)?;
                    self.emit(OpCode::OpToString, &[]);
                }
            }
            self.emit(OpCode::OpAdd, &[]);
        }

        Ok(())
    }

    pub(super) fn compile_if_expression(
        &mut self,
        condition: &Expression,
        consequence: &Block,
        alternative: &Option<Block>,
    ) -> CompileResult<()> {
        self.validate_boolean_expression(condition, "if condition")?;
        let condition_jump = self.compile_jump_not_truthy_condition(condition)?;

        let consequence_has_value = self.block_has_value_tail(consequence);
        // Consequence branch inherits tail position
        if self.in_tail_position {
            self.compile_block_with_tail(consequence)?;
        } else {
            self.compile_block(consequence)?;
        }

        if consequence_has_value {
            if self.is_last_instruction(OpCode::OpPop) {
                self.remove_last_pop();
            }
        } else {
            self.emit(OpCode::OpNone, &[]);
        }

        let jump_pos = self.emit(OpCode::OpJump, &[9999]);
        self.change_operand(condition_jump.position, self.current_instructions().len());

        // Pop the condition value that was left on stack when we jumped here
        // (OpJumpNotTruthy keeps value on stack when jumping for short-circuit support)
        if condition_jump.leaves_condition_on_jump {
            self.emit(OpCode::OpPop, &[]);
        }

        // Alternative branch also inherits tail position
        if let Some(alt) = alternative {
            let alternative_has_value = self.block_has_value_tail(alt);
            if self.in_tail_position {
                self.compile_block_with_tail(alt)?;
            } else {
                self.compile_block(alt)?;
            }

            if alternative_has_value {
                if self.is_last_instruction(OpCode::OpPop) {
                    self.remove_last_pop();
                }
            } else {
                self.emit(OpCode::OpNone, &[]);
            }
        } else {
            self.emit(OpCode::OpNone, &[]);
        }

        self.change_operand(jump_pos, self.current_instructions().len());
        Ok(())
    }

    pub(super) fn compile_match_expression(
        &mut self,
        scrutinee: &Expression,
        arms: &[MatchArm],
        match_span: Span,
    ) -> CompileResult<()> {
        // Exhaustiveness check before compiling arms.
        self.check_match_exhaustiveness(scrutinee, arms, match_span)?;
        // Unreachable arm detection: warn on arms provably subsumed by earlier ones.
        for diag in self.collect_unreachable_arm_warnings(arms) {
            self.warnings.push(diag);
        }
        // Optimisation: if the scrutinee is a simple consumable local used exactly once AND
        // every arm is a simple ADT pattern (no guards, arity ≤ 2, all-identifier/wildcard
        // fields, known constructor), skip the temp variable entirely.
        //
        // Instead, each arm emits `OpIsAdtJumpLocal(local_idx, …)` which peeks at the local
        // slot without cloning (Rc strong_count stays 1), then on match emits
        // `OpConsumeLocal(local_idx)` to move the value to the stack — so `Rc::try_unwrap`
        // succeeds in `OpAdtFields2`, moving fields without any clone/drop overhead.
        let consume_local_idx: Option<usize> = if self.all_arms_simple_adt(arms) {
            self.scrutinee_as_simple_consumable_local(scrutinee)
        } else {
            None
        };

        // Compile scrutinee once and store it in a temp symbol (standard path only).
        // Keep it in the current scope so top-level matches use globals, not stack-backed locals.
        let temp_symbol: Option<Binding> = if consume_local_idx.is_none() {
            self.compile_non_tail_expression(scrutinee)?;
            let sym = self.symbol_table.define_temp();
            match sym.symbol_scope {
                SymbolScope::Global => {
                    self.emit(OpCode::OpSetGlobal, &[sym.index]);
                }
                SymbolScope::Local => {
                    self.emit(OpCode::OpSetLocal, &[sym.index]);
                }
                _ => {
                    return Err(Self::boxed(Diagnostic::make_error(
                        &ICE_TEMP_SYMBOL_MATCH,
                        &[],
                        self.file_path.clone(),
                        Span::new(Position::default(), Position::default()),
                    )));
                }
            };
            Some(sym)
        } else {
            None
        };

        let mut end_jumps = Vec::new();
        let mut next_arm_jumps: Vec<ConditionalJump> = Vec::new();

        // Compile each arm
        for arm in arms {
            if !next_arm_jumps.is_empty() {
                let pop_start = self.current_instructions().len();
                let arm_start = if next_arm_jumps
                    .iter()
                    .any(|jump| jump.leaves_condition_on_jump)
                {
                    self.emit(OpCode::OpPop, &[]);
                    self.current_instructions().len()
                } else {
                    pop_start
                };
                for jump in next_arm_jumps.drain(..) {
                    let target = if jump.leaves_condition_on_jump {
                        pop_start
                    } else {
                        arm_start
                    };
                    self.patch_cond_jump(&jump, target);
                }
            }

            // Fast path: Constructor patterns with all-identifier/wildcard fields,
            // arity ≤ 2, a known constructor, and no guard.
            // When `consume_local_idx` is set, uses OpIsAdtJumpLocal which avoids cloning
            // and keeps Rc strong_count == 1 so OpAdtFields2's Rc::try_unwrap succeeds.
            let mut arm_next_jumps = if arm.guard.is_none()
                && let Pattern::Constructor {
                    name,
                    fields,
                    span: pat_span,
                } = &arm.pattern
                && Self::is_simple_adt_pattern(fields)
                && self
                    .adt_registry
                    .lookup_constructor(*name)
                    .is_some_and(|info| info.arity == fields.len())
            {
                self.enter_block_scope();
                let jump = if let Some(local_idx) = consume_local_idx {
                    self.compile_adt_arm_simple_local(name, fields, *pat_span, local_idx)?
                } else {
                    self.compile_adt_arm_simple(
                        name,
                        fields,
                        *pat_span,
                        temp_symbol
                            .as_ref()
                            .expect("temp_symbol must exist for standard path"),
                    )?
                };
                vec![jump]
            } else {
                // General path: separate check + bind.
                let ts = temp_symbol
                    .as_ref()
                    .expect("temp_symbol must exist for general path");
                let jumps = self.compile_pattern_check(ts, &arm.pattern)?;
                self.enter_block_scope();
                self.compile_pattern_bind(ts, &arm.pattern)?;
                jumps
            };

            // Guard runs only after a successful pattern match and in the arm binding scope.
            if let Some(guard) = &arm.guard {
                self.validate_boolean_expression(guard, "match guard")?;
                arm_next_jumps.push(self.compile_jump_not_truthy_condition(guard)?);
            }

            // Compute arm-binding-specific use counts so that bindings used exactly
            // once in the arm body (e.g. `left`, `right` in a Node pattern) are emitted
            // as `OpConsumeLocal` instead of `OpGetLocal`, keeping Rc strong_count == 1
            // and enabling `Rc::try_unwrap` to succeed in `OpAdtFields2` / `OpAdtField`.
            let merged_counts = {
                let outer_clone = self.current_consumable_local_use_counts().cloned();
                if let Some(mut merged) = outer_clone {
                    let mut arm_body_counts: HashMap<Symbol, usize> = HashMap::new();
                    self.collect_consumable_param_uses(&arm.body, &mut arm_body_counts);
                    for (sym, count) in arm_body_counts {
                        merged.entry(sym).or_insert(count);
                    }
                    Some(merged)
                } else {
                    None
                }
            };
            let in_tail = self.in_tail_position;
            if let Some(counts) = merged_counts {
                self.with_consumable_local_use_counts(counts, |compiler| -> CompileResult<()> {
                    if in_tail {
                        compiler.with_tail_position(true, |c| c.compile_expression(&arm.body))
                    } else {
                        compiler.compile_expression(&arm.body)
                    }
                })?;
            } else if in_tail {
                self.with_tail_position(true, |compiler| compiler.compile_expression(&arm.body))?;
            } else {
                self.compile_expression(&arm.body)?;
            }
            self.leave_block_scope();

            // Jump to end after executing this arm's body.
            end_jumps.push(self.emit(OpCode::OpJump, &[9999]));
            next_arm_jumps = arm_next_jumps;
        }

        // If no arm matched (or all guards failed), leave a sentinel value on stack.
        if !next_arm_jumps.is_empty() {
            let pop_start = self.current_instructions().len();
            let no_match_start = if next_arm_jumps
                .iter()
                .any(|jump| jump.leaves_condition_on_jump)
            {
                self.emit(OpCode::OpPop, &[]);
                self.current_instructions().len()
            } else {
                pop_start
            };
            for jump in next_arm_jumps {
                let target = if jump.leaves_condition_on_jump {
                    pop_start
                } else {
                    no_match_start
                };
                self.patch_cond_jump(&jump, target);
            }
        }
        self.emit(OpCode::OpNone, &[]);

        // Patch all end jumps to point here
        for jump_pos in end_jumps {
            self.change_operand(jump_pos, self.current_instructions().len());
        }

        Ok(())
    }

    fn compile_pattern_check(
        &mut self,
        scrutinee: &Binding,
        pattern: &Pattern,
    ) -> CompileResult<Vec<ConditionalJump>> {
        match pattern {
            Pattern::Wildcard { .. } => {
                // Wildcard always matches, so we never jump to next arm
                // Emit OpTrue and OpJumpNotTruthy (which will never jump)
                // Actually, for wildcard we should always execute this arm
                // So we return a dummy jump position that will never be used
                // For simplicity, emit a condition that's always true
                self.emit(OpCode::OpTrue, &[]);
                Ok(vec![ConditionalJump {
                    position: self.emit(OpCode::OpJumpNotTruthy, &[9999]),
                    leaves_condition_on_jump: true,
                    first_operand: None,
                }])
            }
            Pattern::Literal { expression, .. } => {
                // Push pattern value onto stack: [scrutinee, pattern]
                // OpEqual compares and pushes boolean: [result]
                // OpJumpNotTruthy jumps when false (no match), continues when true (match)
                self.load_symbol(scrutinee);
                self.compile_non_tail_expression(expression)?;
                Ok(vec![
                    self.emit_conditional_jump_not_truthy_for_compiled_comparison(OpCode::OpEqual),
                ])
            }
            Pattern::None { .. } => {
                // Check if scrutinee is None
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpNone, &[]);
                Ok(vec![
                    self.emit_conditional_jump_not_truthy_for_compiled_comparison(OpCode::OpEqual),
                ])
            }
            Pattern::Some { pattern: inner, .. } => {
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpIsSome, &[]);
                let mut jumps = vec![ConditionalJump {
                    position: self.emit(OpCode::OpJumpNotTruthy, &[9999]),
                    leaves_condition_on_jump: true,
                    first_operand: None,
                }];

                match inner.as_ref() {
                    Pattern::Wildcard { .. } | Pattern::Identifier { .. } => Ok(jumps),
                    _ => {
                        let inner_symbol = self.symbol_table.define_temp();
                        self.load_symbol(scrutinee);
                        self.emit(OpCode::OpUnwrapSome, &[]);
                        match inner_symbol.symbol_scope {
                            SymbolScope::Global => {
                                self.emit(OpCode::OpSetGlobal, &[inner_symbol.index]);
                            }
                            SymbolScope::Local => {
                                self.emit(OpCode::OpSetLocal, &[inner_symbol.index]);
                            }
                            _ => {
                                return Err(Self::boxed(Diagnostic::make_error(
                                    &ICE_TEMP_SYMBOL_SOME_PATTERN,
                                    &[],
                                    self.file_path.clone(),
                                    Span::new(Position::default(), Position::default()),
                                )));
                            }
                        }
                        let inner_jumps = self.compile_pattern_check(&inner_symbol, inner)?;
                        jumps.extend(inner_jumps);
                        Ok(jumps)
                    }
                }
            }
            Pattern::Left { pattern: inner, .. } => {
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpIsLeft, &[]);

                let mut jumps = vec![ConditionalJump {
                    position: self.emit(OpCode::OpJumpNotTruthy, &[9999]),
                    leaves_condition_on_jump: true,
                    first_operand: None,
                }];

                match inner.as_ref() {
                    Pattern::Wildcard { .. } | Pattern::Identifier { .. } => Ok(jumps),
                    _ => {
                        let inner_symbol = self.symbol_table.define_temp();
                        self.load_symbol(scrutinee);
                        self.emit(OpCode::OpUnwrapLeft, &[]);

                        match inner_symbol.symbol_scope {
                            SymbolScope::Global => {
                                self.emit(OpCode::OpSetGlobal, &[inner_symbol.index]);
                            }
                            SymbolScope::Local => {
                                self.emit(OpCode::OpSetLocal, &[inner_symbol.index]);
                            }
                            _ => {
                                return Err(Self::boxed(Diagnostic::make_error(
                                    &ICE_TEMP_SYMBOL_LEFT_PATTERN,
                                    &[],
                                    self.file_path.clone(),
                                    Span::new(Position::default(), Position::default()),
                                )));
                            }
                        }

                        let inner_jumps = self.compile_pattern_check(&inner_symbol, inner)?;
                        jumps.extend(inner_jumps);
                        Ok(jumps)
                    }
                }
            }
            Pattern::Right { pattern: inner, .. } => {
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpIsRight, &[]);

                let mut jumps = vec![ConditionalJump {
                    position: self.emit(OpCode::OpJumpNotTruthy, &[9999]),
                    leaves_condition_on_jump: true,
                    first_operand: None,
                }];

                match inner.as_ref() {
                    Pattern::Wildcard { .. } | Pattern::Identifier { .. } => Ok(jumps),
                    _ => {
                        let inner_symbol = self.symbol_table.define_temp();
                        self.load_symbol(scrutinee);
                        self.emit(OpCode::OpUnwrapRight, &[]);

                        match inner_symbol.symbol_scope {
                            SymbolScope::Global => {
                                self.emit(OpCode::OpSetGlobal, &[inner_symbol.index]);
                            }
                            SymbolScope::Local => {
                                self.emit(OpCode::OpSetLocal, &[inner_symbol.index]);
                            }
                            _ => {
                                return Err(Self::boxed(Diagnostic::make_error(
                                    &ICE_TEMP_SYMBOL_RIGHT_PATTERN,
                                    &[],
                                    self.file_path.clone(),
                                    Span::new(Position::default(), Position::default()),
                                )));
                            }
                        }

                        let inner_jumps = self.compile_pattern_check(&inner_symbol, inner)?;
                        jumps.extend(inner_jumps);
                        Ok(jumps)
                    }
                }
            }
            Pattern::Identifier { .. } => {
                // Identifier patterns always match; value binding is performed in
                // `compile_pattern_bind` after a successful check.
                self.emit(OpCode::OpTrue, &[]);
                Ok(vec![ConditionalJump {
                    position: self.emit(OpCode::OpJumpNotTruthy, &[9999]),
                    leaves_condition_on_jump: true,
                    first_operand: None,
                }])
            }
            Pattern::EmptyList { .. } => {
                // Check if scrutinee is an empty list
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpIsEmptyList, &[]);
                Ok(vec![ConditionalJump {
                    position: self.emit(OpCode::OpJumpNotTruthy, &[9999]),
                    leaves_condition_on_jump: true,
                    first_operand: None,
                }])
            }
            Pattern::Cons { head, tail, .. } => {
                // Check if scrutinee is a non-empty cons cell
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpIsCons, &[]);
                let mut jumps = vec![ConditionalJump {
                    position: self.emit(OpCode::OpJumpNotTruthy, &[9999]),
                    leaves_condition_on_jump: true,
                    first_operand: None,
                }];

                // Check head pattern
                let head_symbol = self.symbol_table.define_temp();
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpConsHead, &[]);

                match head_symbol.symbol_scope {
                    SymbolScope::Global => {
                        self.emit(OpCode::OpSetGlobal, &[head_symbol.index]);
                    }
                    SymbolScope::Local => {
                        self.emit(OpCode::OpSetLocal, &[head_symbol.index]);
                    }
                    _ => {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &ICE_TEMP_SYMBOL_MATCH,
                            &[],
                            self.file_path.clone(),
                            Span::new(Position::default(), Position::default()),
                        )));
                    }
                }
                match head.as_ref() {
                    Pattern::Wildcard { .. } | Pattern::Identifier { .. } => {}
                    _ => {
                        let head_jumps = self.compile_pattern_check(&head_symbol, head)?;
                        jumps.extend(head_jumps);
                    }
                }

                // Check tail pattern
                let tail_symbol = self.symbol_table.define_temp();
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpConsTail, &[]);

                match tail_symbol.symbol_scope {
                    SymbolScope::Global => {
                        self.emit(OpCode::OpSetGlobal, &[tail_symbol.index]);
                    }
                    SymbolScope::Local => {
                        self.emit(OpCode::OpSetLocal, &[tail_symbol.index]);
                    }
                    _ => {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &ICE_TEMP_SYMBOL_MATCH,
                            &[],
                            self.file_path.clone(),
                            Span::new(Position::default(), Position::default()),
                        )));
                    }
                }
                match tail.as_ref() {
                    Pattern::Wildcard { .. } | Pattern::Identifier { .. } => {}
                    _ => {
                        let tail_jumps = self.compile_pattern_check(&tail_symbol, tail)?;
                        jumps.extend(tail_jumps);
                    }
                }

                Ok(jumps)
            }
            Pattern::Tuple { elements, .. } => {
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpIsTuple, &[]);
                let mut jumps = vec![ConditionalJump {
                    position: self.emit(OpCode::OpJumpNotTruthy, &[9999]),
                    leaves_condition_on_jump: true,
                    first_operand: None,
                }];

                for (index, element) in elements.iter().enumerate() {
                    match element {
                        Pattern::Wildcard { .. } | Pattern::Identifier { .. } => continue,
                        _ => {}
                    }
                    let inner_symbol = self.symbol_table.define_temp();
                    self.load_symbol(scrutinee);
                    self.emit(OpCode::OpTupleIndex, &[index]);
                    match inner_symbol.symbol_scope {
                        SymbolScope::Global => {
                            self.emit(OpCode::OpSetGlobal, &[inner_symbol.index]);
                        }
                        SymbolScope::Local => {
                            self.emit(OpCode::OpSetLocal, &[inner_symbol.index]);
                        }
                        _ => {
                            return Err(Self::boxed(Diagnostic::make_error(
                                &ICE_TEMP_SYMBOL_MATCH,
                                &[],
                                self.file_path.clone(),
                                Span::new(Position::default(), Position::default()),
                            )));
                        }
                    }
                    let inner_jumps = self.compile_pattern_check(&inner_symbol, element)?;
                    jumps.extend(inner_jumps);
                }

                Ok(jumps)
            }
            Pattern::Constructor { name, fields, span } => {
                // 1. Check if this is a known constructor
                let Some(constructor_info) = self.adt_registry.lookup_constructor(*name) else {
                    // Before reporting unknown constructor, check for cross-module access
                    // via a qualified name (e.g. Module.Ctor used in a pattern).
                    if let Some((member_name, qualifier)) =
                        self.module_constructor_boundary_from_qualified_identifier(*name)
                    {
                        if self.strict_mode {
                            return Err(Self::boxed(
                                cross_module_constructor_access_error(
                                    *span,
                                    member_name.as_str(),
                                    qualifier.as_str(),
                                )
                                .with_file(self.file_path.clone()),
                            ));
                        }
                        self.warnings.push(
                            cross_module_constructor_access_warning(
                                *span,
                                member_name.as_str(),
                                qualifier.as_str(),
                            )
                            .with_file(self.file_path.clone()),
                        );
                    }
                    let name_str = self.interner.resolve(*name).to_string();
                    return Err(Self::boxed(
                        diagnostic_for(&UNKNOWN_CONSTRUCTOR)
                            .with_span(*span)
                            .with_message(format!("Unknown constructor `{}`.", name_str)),
                    ));
                };

                if fields.len() != constructor_info.arity {
                    let name_str = self.interner.resolve(*name).to_string();
                    return Err(Self::boxed(
                        constructor_pattern_arity_mismatch(
                            *span,
                            &name_str,
                            constructor_info.arity,
                            fields.len(),
                        )
                        .with_file(self.file_path.clone()),
                    ));
                }

                // tag_idx not used in check
                let _ = constructor_info.tag_idx;

                // 2. Load scrutinee, emit OpIsAdt with constructor name constant
                self.load_symbol(scrutinee);
                let constructor_name = self.interner.resolve(*name).to_string();
                let const_idx = self.add_constant(Value::String(Rc::new(constructor_name.clone())));
                self.emit(OpCode::OpIsAdt, &[const_idx]);

                let mut jumps = vec![ConditionalJump {
                    position: self.emit(OpCode::OpJumpNotTruthy, &[9999]),
                    leaves_condition_on_jump: true,
                    first_operand: None,
                }];

                // 3. For each non-wildcard/non-identifier field pattern, extract and sub-check
                for (field_idx, field_pat) in fields.iter().enumerate() {
                    if matches!(
                        field_pat,
                        Pattern::Wildcard { .. } | Pattern::Identifier { .. }
                    ) {
                        continue;
                    }

                    let inner_symbol = self.symbol_table.define_temp();
                    self.load_symbol(scrutinee);
                    self.emit(OpCode::OpAdtField, &[field_idx]);

                    match inner_symbol.symbol_scope {
                        SymbolScope::Global => {
                            self.emit(OpCode::OpSetGlobal, &[inner_symbol.index]);
                        }
                        SymbolScope::Local => {
                            self.emit(OpCode::OpSetLocal, &[inner_symbol.index]);
                        }
                        _ => {
                            return Err(Self::boxed(Diagnostic::make_error(
                                &ICE_TEMP_SYMBOL_MATCH,
                                &[],
                                self.file_path.clone(),
                                Span::new(Position::default(), Position::default()),
                            )));
                        }
                    }
                    let inner_jumps = self.compile_pattern_check(&inner_symbol, field_pat)?;
                    jumps.extend(inner_jumps);
                }

                Ok(jumps)
            }
        }
    }

    pub(super) fn compile_pattern_bind(
        &mut self,
        scrutinee: &Binding,
        pattern: &Pattern,
    ) -> CompileResult<()> {
        match pattern {
            Pattern::Identifier { name, span } => {
                self.load_symbol(scrutinee);
                let symbol = self.symbol_table.define(*name, *span);
                match symbol.symbol_scope {
                    SymbolScope::Global => {
                        self.emit(OpCode::OpSetGlobal, &[symbol.index]);
                    }
                    SymbolScope::Local => {
                        self.emit(OpCode::OpSetLocal, &[symbol.index]);
                    }
                    _ => {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &ICE_SYMBOL_SCOPE_PATTERN,
                            &[],
                            self.file_path.clone(),
                            Span::new(Position::default(), Position::default()),
                        )));
                    }
                };
            }
            Pattern::Some { pattern: inner, .. } => {
                let inner_symbol = self.symbol_table.define_temp();
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpUnwrapSome, &[]);
                match inner_symbol.symbol_scope {
                    SymbolScope::Global => {
                        self.emit(OpCode::OpSetGlobal, &[inner_symbol.index]);
                    }
                    SymbolScope::Local => {
                        self.emit(OpCode::OpSetLocal, &[inner_symbol.index]);
                    }
                    _ => {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &ICE_TEMP_SYMBOL_SOME_BINDING,
                            &[],
                            self.file_path.clone(),
                            Span::new(Position::default(), Position::default()),
                        )));
                    }
                }
                self.compile_pattern_bind(&inner_symbol, inner)?;
            }
            // Either type pattern bindings
            Pattern::Left { pattern: inner, .. } => {
                let inner_symbol = self.symbol_table.define_temp();
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpUnwrapLeft, &[]);

                match inner_symbol.symbol_scope {
                    SymbolScope::Global => {
                        self.emit(OpCode::OpSetGlobal, &[inner_symbol.index]);
                    }
                    SymbolScope::Local => {
                        self.emit(OpCode::OpSetLocal, &[inner_symbol.index]);
                    }
                    _ => {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &ICE_TEMP_SYMBOL_LEFT_BINDING,
                            &[],
                            self.file_path.clone(),
                            Span::new(Position::default(), Position::default()),
                        )));
                    }
                }
                self.compile_pattern_bind(&inner_symbol, inner)?;
            }
            Pattern::Right { pattern: inner, .. } => {
                let inner_symbol = self.symbol_table.define_temp();
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpUnwrapRight, &[]);
                match inner_symbol.symbol_scope {
                    SymbolScope::Global => {
                        self.emit(OpCode::OpSetGlobal, &[inner_symbol.index]);
                    }
                    SymbolScope::Local => {
                        self.emit(OpCode::OpSetLocal, &[inner_symbol.index]);
                    }
                    _ => {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &ICE_TEMP_SYMBOL_RIGHT_BINDING,
                            &[],
                            self.file_path.clone(),
                            Span::new(Position::default(), Position::default()),
                        )));
                    }
                }
                self.compile_pattern_bind(&inner_symbol, inner)?;
            }
            Pattern::EmptyList { .. } => {}
            Pattern::Cons { head, tail, .. } => {
                // Bind head
                let head_symbol = self.symbol_table.define_temp();
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpConsHead, &[]);
                match head_symbol.symbol_scope {
                    SymbolScope::Global => {
                        self.emit(OpCode::OpSetGlobal, &[head_symbol.index]);
                    }
                    SymbolScope::Local => {
                        self.emit(OpCode::OpSetLocal, &[head_symbol.index]);
                    }
                    _ => {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &ICE_TEMP_SYMBOL_MATCH,
                            &[],
                            self.file_path.clone(),
                            Span::new(Position::default(), Position::default()),
                        )));
                    }
                }
                self.compile_pattern_bind(&head_symbol, head)?;

                // Bind tail
                let tail_symbol = self.symbol_table.define_temp();
                self.load_symbol(scrutinee);
                self.emit(OpCode::OpConsTail, &[]);
                match tail_symbol.symbol_scope {
                    SymbolScope::Global => {
                        self.emit(OpCode::OpSetGlobal, &[tail_symbol.index]);
                    }
                    SymbolScope::Local => {
                        self.emit(OpCode::OpSetLocal, &[tail_symbol.index]);
                    }
                    _ => {
                        return Err(Self::boxed(Diagnostic::make_error(
                            &ICE_TEMP_SYMBOL_MATCH,
                            &[],
                            self.file_path.clone(),
                            Span::new(Position::default(), Position::default()),
                        )));
                    }
                }
                self.compile_pattern_bind(&tail_symbol, tail)?;
            }
            Pattern::Tuple { elements, .. } => {
                for (index, element) in elements.iter().enumerate() {
                    let inner_symbol = self.symbol_table.define_temp();
                    self.load_symbol(scrutinee);
                    self.emit(OpCode::OpTupleIndex, &[index]);
                    match inner_symbol.symbol_scope {
                        SymbolScope::Global => {
                            self.emit(OpCode::OpSetGlobal, &[inner_symbol.index]);
                        }
                        SymbolScope::Local => {
                            self.emit(OpCode::OpSetLocal, &[inner_symbol.index]);
                        }
                        _ => {
                            return Err(Self::boxed(Diagnostic::make_error(
                                &ICE_TEMP_SYMBOL_MATCH,
                                &[],
                                self.file_path.clone(),
                                Span::new(Position::default(), Position::default()),
                            )));
                        }
                    }
                    self.compile_pattern_bind(&inner_symbol, element)?;
                }
            }
            Pattern::Wildcard { .. } | Pattern::Literal { .. } | Pattern::None { .. } => {}
            Pattern::Constructor { name, fields, span } => {
                let Some(constructor_info) = self.adt_registry.lookup_constructor(*name) else {
                    // Before reporting unknown constructor, check for cross-module access
                    // via a qualified name (e.g. Module.Ctor used in a pattern).
                    if let Some((member_name, qualifier)) =
                        self.module_constructor_boundary_from_qualified_identifier(*name)
                    {
                        if self.strict_mode {
                            return Err(Self::boxed(
                                cross_module_constructor_access_error(
                                    *span,
                                    member_name.as_str(),
                                    qualifier.as_str(),
                                )
                                .with_file(self.file_path.clone()),
                            ));
                        }
                        self.warnings.push(
                            cross_module_constructor_access_warning(
                                *span,
                                member_name.as_str(),
                                qualifier.as_str(),
                            )
                            .with_file(self.file_path.clone()),
                        );
                    }
                    let name_str = self.interner.resolve(*name).to_string();
                    return Err(Self::boxed(
                        diagnostic_for(&UNKNOWN_CONSTRUCTOR)
                            .with_span(*span)
                            .with_message(format!("Unknown constructor `{}`.", name_str)),
                    ));
                };
                if fields.len() != constructor_info.arity {
                    let name_str = self.interner.resolve(*name).to_string();
                    return Err(Self::boxed(
                        constructor_pattern_arity_mismatch(
                            *span,
                            &name_str,
                            constructor_info.arity,
                            fields.len(),
                        )
                        .with_file(self.file_path.clone()),
                    ));
                }

                for (field_idx, field_pat) in fields.iter().enumerate() {
                    if matches!(field_pat, Pattern::Wildcard { .. }) {
                        continue;
                    }

                    let inner_symbol = self.symbol_table.define_temp();
                    self.load_symbol(scrutinee);
                    self.emit(OpCode::OpAdtField, &[field_idx]);

                    match inner_symbol.symbol_scope {
                        SymbolScope::Global => {
                            self.emit(OpCode::OpSetGlobal, &[inner_symbol.index]);
                        }
                        SymbolScope::Local => {
                            self.emit(OpCode::OpSetLocal, &[inner_symbol.index]);
                        }
                        _ => {
                            return Err(Self::boxed(Diagnostic::make_error(
                                &ICE_TEMP_SYMBOL_MATCH,
                                &[],
                                self.file_path.clone(),
                                Span::new(Position::default(), Position::default()),
                            )));
                        }
                    }
                    self.compile_pattern_bind(&inner_symbol, field_pat)?;
                }
            }
        }
        Ok(())
    }

    fn try_emit_primop_call(
        &mut self,
        function: &Expression,
        arguments: &[Expression],
    ) -> CompileResult<bool> {
        let Expression::Identifier { name, .. } = function else {
            return Ok(false);
        };

        // Exposed Flux stdlib functions shadow primops.
        if self.exposed_bindings.contains_key(name) {
            return Ok(false);
        }

        // Shadowed names must resolve through the regular call path.
        if self.resolve_visible_symbol(*name).is_some() {
            return Ok(false);
        }

        // Special case: `list(a, b, c)` → cons-list construction.
        // Variadic, so can't be in the fixed-arity primop table.
        if self.sym(*name) == "list" {
            for arg in arguments {
                self.compile_non_tail_expression(arg)?;
            }
            self.emit_constant_value(Value::EmptyList);
            for _ in 0..arguments.len() {
                self.emit(OpCode::OpCons, &[]);
            }
            return Ok(true);
        }

        let Some(primop) = Self::resolve_library_primop(self.sym(*name), arguments.len())
            .or_else(|| CorePrimOp::from_name(self.sym(*name), arguments.len()))
        else {
            return Ok(false);
        };

        let required_name = match primop.effect_kind() {
            PrimEffect::Io => Some("IO"),
            PrimEffect::Time => Some("Time"),
            PrimEffect::Control | PrimEffect::Pure => None,
        };
        if let Some(required_name) = required_name
            && !self.is_effect_available_name(required_name)
        {
            return Err(Self::boxed(
                Diagnostic::make_error_dynamic(
                    "E400",
                    "MISSING EFFECT",
                    ErrorType::Compiler,
                    format!(
                        "Call to `{}` requires effect `{}` in this function signature.",
                        self.sym(*name),
                        required_name
                    ),
                    Some(format!(
                        "Add `with {}` to the enclosing function.",
                        required_name
                    )),
                    self.file_path.clone(),
                    function.span(),
                )
                .with_display_title("Missing Ambient Effect")
                .with_category(DiagnosticCategory::Effects)
                .with_phase(crate::diagnostics::DiagnosticPhase::Effect)
                .with_primary_label(function.span(), "effectful call occurs here"),
            ));
        }

        for argument in arguments {
            self.compile_non_tail_expression(argument)?;
        }

        self.emit(OpCode::OpPrimOp, &[primop.id() as usize, arguments.len()]);
        Ok(true)
    }

    /// Compile `perform Effect.op(args)` — push args, then `OpPerform`.
    fn compile_perform(
        &mut self,
        effect: Symbol,
        op: Symbol,
        args: &[Expression],
    ) -> CompileResult<()> {
        let span = self
            .current_span
            .unwrap_or_else(|| Span::new(Position::default(), Position::default()));

        let Some(has_operation) = self
            .effect_declared_ops(effect)
            .map(|ops| ops.contains(&op))
        else {
            let effect_name = self.sym(effect).to_string();
            let hint = suggest_effect_name(&effect_name)
                .unwrap_or_else(|| "Declare the effect before using `perform`.".to_string());
            return Err(Self::boxed(
                Diagnostic::make_error_dynamic(
                    "E403",
                    "UNKNOWN EFFECT",
                    ErrorType::Compiler,
                    format!("Effect `{}` is not declared.", effect_name),
                    Some(hint),
                    self.file_path.clone(),
                    span,
                )
                .with_display_title("Unknown Effect")
                .with_category(DiagnosticCategory::Effects)
                .with_phase(crate::diagnostics::DiagnosticPhase::Effect)
                .with_primary_label(span, "unknown effect in perform"),
            ));
        };
        if !has_operation {
            let effect_name = self.sym(effect).to_string();
            let op_name = self.sym(op).to_string();
            return Err(Self::boxed(
                Diagnostic::make_error_dynamic(
                    "E404",
                    "UNKNOWN EFFECT OPERATION",
                    ErrorType::Compiler,
                    format!(
                        "Effect `{}` has no declared operation `{}`.",
                        effect_name, op_name
                    ),
                    Some("Add the operation to the effect declaration or rename it.".to_string()),
                    self.file_path.clone(),
                    span,
                )
                .with_display_title("Unknown Effect Operation")
                .with_category(DiagnosticCategory::Effects)
                .with_phase(crate::diagnostics::DiagnosticPhase::Effect)
                .with_primary_label(span, "unknown operation in perform"),
            ));
        }
        let (op_params, _op_ret) =
            self.effect_operation_function_parts(effect, op, span, "perform checks")?;
        if args.len() != op_params.len() {
            return Err(Self::boxed(
                Diagnostic::make_error_dynamic(
                    "E300",
                    "TYPE UNIFICATION ERROR",
                    ErrorType::Compiler,
                    format!(
                        "`perform {}.{}` expects {} argument(s), got {}.",
                        self.sym(effect),
                        self.sym(op),
                        op_params.len(),
                        args.len()
                    ),
                    Some("Pass arguments that match the effect operation signature.".to_string()),
                    self.file_path.clone(),
                    span,
                )
                .with_display_title("Wrong Number Of Arguments")
                .with_category(DiagnosticCategory::TypeInference)
                .with_phase(crate::diagnostics::DiagnosticPhase::TypeInference)
                .with_primary_label(span, "perform argument count mismatch"),
            ));
        }

        for (arg, expected_ty) in args.iter().zip(op_params.iter()) {
            let Some(expected) = TypeEnv::infer_type_from_type_expr(
                expected_ty,
                &Default::default(),
                &self.interner,
            ) else {
                continue;
            };
            self.validate_expr_expected_type_with_policy(
                &expected,
                arg,
                "perform argument type is known at compile time",
                "argument does not match effect operation signature".to_string(),
                "perform argument",
                true,
            )?;
        }

        if !self.is_effect_available(effect) {
            let effect_name = self.sym(effect).to_string();
            return Err(Self::boxed(
                Diagnostic::make_error_dynamic(
                    "E400",
                    "MISSING EFFECT",
                    ErrorType::Compiler,
                    format!(
                        "Performing `{}` requires effect `{}` in this function signature.",
                        self.sym(op),
                        effect_name
                    ),
                    Some(format!(
                        "Add `with {}` to the enclosing function.",
                        effect_name
                    )),
                    self.file_path.clone(),
                    span,
                )
                .with_display_title("Missing Ambient Effect")
                .with_category(DiagnosticCategory::Effects)
                .with_phase(crate::diagnostics::DiagnosticPhase::Effect)
                .with_primary_label(span, "effectful perform occurs here"),
            ));
        }

        // Evidence-passing path: if the handler has evidence locals, emit a
        // direct function call (OpGetLocal + identity resume + args + OpCall)
        // instead of handler stack access.
        if let Some(ev_local) = self.resolve_evidence_local(effect, op) {
            // Push arm closure from evidence local.
            self.emit(OpCode::OpGetLocal, &[ev_local]);
            // Push identity closure as resume parameter.
            self.emit_identity_closure();
            // Compile arguments.
            for arg in args {
                self.compile_non_tail_expression(arg)?;
            }
            // Call: arm_closure(resume, arg0, ..., argN)
            self.emit(OpCode::OpCall, &[1 + args.len()]);
        } else {
            // Non-evidence path: compile args first, then dispatch opcode.
            for arg in args {
                self.compile_non_tail_expression(arg)?;
            }
            if let Some((depth, arm_idx)) = self.resolve_handler_statically(effect, op) {
                self.emit(
                    OpCode::OpPerformDirectIndexed,
                    &[depth, arm_idx, args.len()],
                );
            } else {
                let effect_name = self.interner.resolve(effect).to_string().into_boxed_str();
                let op_name = self.interner.resolve(op).to_string().into_boxed_str();
                let desc = Value::PerformDescriptor(Rc::new(PerformDescriptor {
                    effect,
                    op,
                    effect_name,
                    op_name,
                }));
                let const_idx = self.add_constant(desc);
                self.emit(OpCode::OpPerform, &[const_idx, args.len()]);
            }
        }

        Ok(())
    }

    /// Compile `expr handle Effect { op(resume, args) -> body, ... }`.
    ///
    /// Emits one `OpClosure` per arm (leaving closures on the stack in order),
    /// then `OpHandle desc_idx`, then the handled `expr`, then `OpEndHandle`.
    fn compile_handle(
        &mut self,
        expr: &Expression,
        effect: Symbol,
        arms: &[HandleArm],
    ) -> CompileResult<()> {
        let mut operations = Vec::new();
        let mut arm_ops = HashSet::new();

        let Some(declared_ops) = self.effect_declared_ops(effect) else {
            let effect_name = self.sym(effect).to_string();
            let hint = suggest_effect_name(&effect_name).unwrap_or_else(|| {
                "Declare the effect before using `handle`, or fix the effect name.".to_string()
            });
            return Err(Self::boxed(
                Diagnostic::make_error_dynamic(
                    "E405",
                    "UNKNOWN HANDLER EFFECT",
                    ErrorType::Compiler,
                    format!(
                        "Effect `{}` is not declared for this handle block.",
                        effect_name
                    ),
                    Some(hint),
                    self.file_path.clone(),
                    expr.span(),
                )
                .with_display_title("Unknown Effect")
                .with_category(DiagnosticCategory::Effects)
                .with_phase(crate::diagnostics::DiagnosticPhase::Effect)
                .with_primary_label(expr.span(), "unknown effect in handle"),
            ));
        };

        for arm in arms {
            if !declared_ops.contains(&arm.operation_name) {
                let effect_name = self.sym(effect).to_string();
                let op_name = self.sym(arm.operation_name).to_string();
                return Err(Self::boxed(
                    Diagnostic::make_error_dynamic(
                        "E401",
                        "UNKNOWN HANDLER OPERATION",
                        ErrorType::Compiler,
                        format!(
                            "Handler for `{}` includes unknown operation `{}`.",
                            effect_name, op_name
                        ),
                        Some(
                            "Add this operation to the effect declaration or remove the arm."
                                .to_string(),
                        ),
                        self.file_path.clone(),
                        arm.span,
                    )
                    .with_display_title("Unknown Effect Operation")
                    .with_category(DiagnosticCategory::Effects)
                    .with_phase(crate::diagnostics::DiagnosticPhase::Effect)
                    .with_primary_label(arm.span, "unknown operation arm"),
                ));
            }
            arm_ops.insert(arm.operation_name);
        }

        let mut missing: Vec<Symbol> = declared_ops.difference(&arm_ops).copied().collect();
        if !missing.is_empty() {
            missing.sort_by_key(|sym| self.sym(*sym).to_string());
            let missing_names = missing
                .iter()
                .map(|sym| self.sym(*sym).to_string())
                .collect::<Vec<_>>()
                .join(", ");
            let effect_name = self.sym(effect).to_string();
            return Err(Self::boxed(
                Diagnostic::make_error_dynamic(
                    "E402",
                    "INCOMPLETE EFFECT HANDLER",
                    ErrorType::Compiler,
                    format!(
                        "Handler for `{}` is missing operations: {}.",
                        effect_name, missing_names
                    ),
                    Some("Add handler arms for all declared operations of the effect.".to_string()),
                    self.file_path.clone(),
                    expr.span(),
                )
                .with_display_title("Missing Effect Handler Arm")
                .with_category(DiagnosticCategory::Effects)
                .with_phase(crate::diagnostics::DiagnosticPhase::Effect)
                .with_primary_label(expr.span(), "handled expression"),
            ));
        }

        let handled_expression_type = match self.hm_expr_type_strict_path(expr) {
            crate::bytecode::compiler::hm_expr_typer::HmExprTypeResult::Known(ty) => Some(ty),
            _ => None,
        };

        for arm in arms {
            let (op_params, _op_ret) = self.effect_operation_function_parts(
                effect,
                arm.operation_name,
                arm.span,
                "handle arm checks",
            )?;
            if arm.params.len() != op_params.len() {
                return Err(Self::boxed(
                    Diagnostic::make_error_dynamic(
                        "E300",
                        "TYPE UNIFICATION ERROR",
                        ErrorType::Compiler,
                        format!(
                            "Handle arm `{}` expects {} parameter(s), got {}.",
                            self.sym(arm.operation_name),
                            op_params.len(),
                            arm.params.len()
                        ),
                        Some(
                            "Adjust handler arm parameters to match the effect operation signature."
                                .to_string(),
                        ),
                        self.file_path.clone(),
                        arm.span,
                    )
                    .with_display_title("Wrong Number Of Arguments")
                    .with_category(DiagnosticCategory::TypeInference)
                    .with_phase(crate::diagnostics::DiagnosticPhase::TypeInference)
                    .with_primary_label(arm.span, "handler arm parameter mismatch"),
                ));
            }

            if let Some(expected_handled_ty) = handled_expression_type.as_ref() {
                self.validate_expr_expected_type_with_policy(
                    expected_handled_ty,
                    &arm.body,
                    "handler arm result type is known at compile time",
                    "handler arm result should match the handled expression type".to_string(),
                    "handle arm result",
                    true,
                )?;
            }

            operations.push(arm.operation_name);

            // Build parameter list: [resume_param, param0, param1, ...]
            let mut params: Vec<Symbol> = Vec::with_capacity(1 + arm.params.len());
            params.push(arm.resume_param);
            params.extend_from_slice(&arm.params);
            let mut parameter_types: Vec<Option<TypeExpr>> =
                Vec::with_capacity(1 + op_params.len());
            parameter_types.push(None);
            for ty in op_params {
                parameter_types.push(Some(ty.clone()));
            }

            // Wrap arm body in a synthetic block for compile_function_literal
            let arm_span = arm.body.span();
            let arm_block = Block {
                statements: vec![Statement::Expression {
                    expression: arm.body.clone(),
                    has_semicolon: false,
                    span: arm_span,
                }],
                span: arm_span,
            };

            // compile_function_literal emits OpClosure, leaving a closure on the stack
            self.compile_function_literal(&params, &parameter_types, &None, &[], &arm_block)?;
        }

        // Detect tail-resumptive handlers and emit the optimized opcode.
        let is_direct =
            crate::bytecode::compiler::tail_resumptive::is_handler_tail_resumptive(arms);
        // Save operations for handler scope before moving into descriptor.
        let scope_ops = operations.clone();

        // Evidence-passing: for TR handlers, store arm closures in local variables
        // so that performs can load them directly instead of indexing handler_stack.
        let evidence_locals = if is_direct {
            let n = arms.len();
            let mut ev_locals = vec![0usize; n];
            // Stack has [c0, c1, ..., cN-1]. Pop in reverse order into locals.
            for i in (0..n).rev() {
                let temp = self.symbol_table.define_temp();
                self.emit(OpCode::OpSetLocal, &[temp.index]);
                ev_locals[i] = temp.index;
            }
            // Reload closures onto stack for OpHandleDirect fallback
            // (needed for cross-function performs that use handler_stack).
            for ev_local in &ev_locals {
                self.emit(OpCode::OpGetLocal, &[*ev_local]);
            }
            Some(ev_locals)
        } else {
            None
        };

        let is_discard =
            !is_direct && crate::bytecode::compiler::tail_resumptive::is_handler_discard(arms);

        let desc = Value::HandlerDescriptor(Rc::new(HandlerDescriptor {
            effect,
            ops: operations,
            is_discard,
        }));

        let desc_idx = self.add_constant(desc);
        let handle_op = if is_direct {
            OpCode::OpHandleDirect
        } else {
            OpCode::OpHandle
        };
        self.emit(handle_op, &[desc_idx]);

        // Push handler scope for static handler resolution.
        self.handler_scopes.push(super::HandlerScope {
            effect,
            is_direct,
            ops: scope_ops,
            evidence_locals,
        });

        // Compile the handled expression with the effect available in scope.
        self.with_handled_effect(effect, |compiler| {
            compiler.compile_non_tail_expression(expr)
        })?;

        // Pop handler scope and remove the handler frame.
        self.handler_scopes.pop();
        self.emit(OpCode::OpEndHandle, &[]);
        Ok(())
    }

    fn compile_non_tail_expression(&mut self, expression: &Expression) -> CompileResult<()> {
        self.with_tail_position(false, |compiler| compiler.compile_expression(expression))
    }

    fn compile_tail_call_argument(
        &mut self,
        expression: &Expression,
        consumable_counts: &HashMap<Symbol, usize>,
    ) -> CompileResult<()> {
        match expression {
            Expression::Identifier { name, .. } => {
                if self.try_emit_consumed_param(*name, consumable_counts) {
                    Ok(())
                } else {
                    self.compile_non_tail_expression(expression)
                }
            }
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                // Check primop first — if the callee is a known primop, emit
                // the primop instruction directly instead of compiling it as
                // a function+call.
                if self.try_emit_primop_call(function, arguments)? {
                    return Ok(());
                }

                self.compile_non_tail_expression(function)?;

                for arg in arguments {
                    if let Expression::Identifier { name, .. } = arg {
                        if !self.try_emit_consumed_param(*name, consumable_counts) {
                            self.compile_non_tail_expression(arg)?;
                        }
                    } else {
                        self.compile_non_tail_expression(arg)?;
                    }
                }
                self.emit(OpCode::OpCall, &[arguments.len()]);
                Ok(())
            }
            _ => self.compile_non_tail_expression(expression),
        }
    }

    fn try_emit_consumed_param(
        &mut self,
        name: Symbol,
        consumable_counts: &HashMap<Symbol, usize>,
    ) -> bool {
        if consumable_counts.get(&name).copied().unwrap_or(0) != 1 {
            return false;
        }
        if let Some(symbol) = self.resolve_visible_symbol(name)
            && self.is_consumable_local(&symbol)
        {
            self.emit_consume_local(symbol.index);
            return true;
        }
        false
    }

    fn try_emit_consumed_local(&mut self, name: Symbol) -> bool {
        let Some(counts) = self.current_consumable_local_use_counts().cloned() else {
            return false;
        };
        self.try_emit_consumed_param(name, &counts)
    }

    pub(super) fn collect_consumable_param_uses_statement(
        &mut self,
        statement: &Statement,
        counts: &mut HashMap<Symbol, usize>,
    ) {
        match statement {
            Statement::Expression { expression, .. } => {
                self.collect_consumable_param_uses(expression, counts);
            }
            Statement::Let { value, .. } | Statement::Assign { value, .. } => {
                self.collect_consumable_param_uses(value, counts)
            }
            Statement::LetDestructure { value, .. } => {
                self.collect_consumable_param_uses(value, counts)
            }
            Statement::Return { value, .. } => {
                if let Some(value) = value {
                    self.collect_consumable_param_uses(value, counts);
                }
            }
            Statement::Function {
                parameters, body, ..
            } => {
                let free_vars = collect_free_vars_in_function_body(parameters, body);
                self.collect_consumable_captured_uses(free_vars, counts);
            }
            Statement::Module { body, .. } => {
                for statement in &body.statements {
                    self.collect_consumable_param_uses_statement(statement, counts);
                }
            }
            Statement::Import { .. } => {}
            Statement::Data { .. } => {}
            Statement::EffectDecl { .. } => {}
            Statement::Class { .. } => {}
            Statement::Instance { .. } => {}
        }
    }

    pub(super) fn collect_consumable_param_uses(
        &mut self,
        expression: &Expression,
        counts: &mut HashMap<Symbol, usize>,
    ) {
        match expression {
            Expression::Identifier { name, .. } => {
                if let Some(symbol) = self.symbol_table.resolve(*name)
                    && self.is_consumable_local(&symbol)
                {
                    *counts.entry(*name).or_insert(0) += 1;
                }
            }
            Expression::Prefix { right, .. } => self.collect_consumable_param_uses(right, counts),
            Expression::Infix { left, right, .. } => {
                self.collect_consumable_param_uses(left, counts);
                self.collect_consumable_param_uses(right, counts);
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                self.collect_consumable_param_uses(condition, counts);
                for statement in &consequence.statements {
                    self.collect_consumable_param_uses_statement(statement, counts);
                }
                if let Some(alt) = alternative {
                    for statement in &alt.statements {
                        self.collect_consumable_param_uses_statement(statement, counts);
                    }
                }
            }
            Expression::DoBlock { block, .. } => {
                for statement in &block.statements {
                    self.collect_consumable_param_uses_statement(statement, counts);
                }
            }
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                self.collect_consumable_param_uses(function, counts);

                for argument in arguments {
                    self.collect_consumable_param_uses(argument, counts);
                }
            }
            Expression::ListLiteral { elements, .. }
            | Expression::ArrayLiteral { elements, .. }
            | Expression::TupleLiteral { elements, .. } => {
                for element in elements {
                    self.collect_consumable_param_uses(element, counts);
                }
            }
            Expression::Index { left, index, .. } => {
                self.collect_consumable_param_uses(left, counts);
                self.collect_consumable_param_uses(index, counts);
            }
            Expression::Hash { pairs, .. } => {
                for (key, value) in pairs {
                    self.collect_consumable_param_uses(key, counts);
                    self.collect_consumable_param_uses(value, counts);
                }
            }
            Expression::MemberAccess { object, .. } => {
                self.collect_consumable_param_uses(object, counts);
            }
            Expression::TupleFieldAccess { object, .. } => {
                self.collect_consumable_param_uses(object, counts);
            }
            Expression::Match {
                scrutinee, arms, ..
            } => {
                self.collect_consumable_param_uses(scrutinee, counts);

                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        self.collect_consumable_param_uses(guard, counts);
                    }
                    self.collect_consumable_param_uses(&arm.body, counts);
                }
            }
            Expression::InterpolatedString { parts, .. } => {
                for part in parts {
                    if let StringPart::Interpolation(expression) = part {
                        self.collect_consumable_param_uses(expression, counts);
                    }
                }
            }
            Expression::Some { value, .. }
            | Expression::Left { value, .. }
            | Expression::Right { value, .. } => self.collect_consumable_param_uses(value, counts),
            Expression::Cons { head, tail, .. } => {
                self.collect_consumable_param_uses(head, counts);
                self.collect_consumable_param_uses(tail, counts);
            }
            Expression::Perform { args, .. } => {
                for arg in args {
                    self.collect_consumable_param_uses(arg, counts);
                }
            }
            Expression::Handle { expr, arms, .. } => {
                self.collect_consumable_param_uses(expr, counts);

                for arm in arms {
                    self.collect_consumable_param_uses(&arm.body, counts);
                }
            }
            Expression::Function {
                parameters, body, ..
            } => {
                let free_vars = collect_free_vars_in_function_body(parameters, body);
                self.collect_consumable_captured_uses(free_vars, counts);
            }
            Expression::Integer { .. }
            | Expression::Float { .. }
            | Expression::String { .. }
            | Expression::Boolean { .. }
            | Expression::None { .. }
            | Expression::EmptyList { .. } => {}
        }
    }

    /// Check if an expression is a self recursive call
    fn is_self_call(&mut self, expression: &Expression) -> bool {
        match expression {
            Expression::Identifier { name, .. } => {
                if let Some(symbol) = self.symbol_table.resolve(*name) {
                    symbol.symbol_scope == SymbolScope::Function
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn is_consumable_local(&self, symbol: &Binding) -> bool {
        if symbol.symbol_scope != SymbolScope::Local {
            return false;
        }
        if self
            .current_function_captured_locals()
            .is_some_and(|captured| captured.contains(&symbol.index))
        {
            return false;
        }
        true
    }

    fn try_emit_adt_constructor_call(
        &mut self,
        function: &Expression,
        arguments: &[Expression],
        span: Span,
    ) -> CompileResult<bool> {
        let (name, boundary_module): (Symbol, Option<Symbol>) = match function {
            Expression::Identifier { name, .. } => (*name, None),
            Expression::MemberAccess { object, member, .. } => {
                let Some(module_name) = self.resolve_module_name_from_expr(object) else {
                    return Ok(false);
                };
                let Some(_adt_owner) =
                    self.module_member_adt_constructor_owner(module_name, *member)
                else {
                    return Ok(false);
                };
                (*member, Some(module_name))
            }
            _ => return Ok(false),
        };

        // Only intercept if the constructor is known.
        let Some(info) = self.adt_registry.lookup_constructor(name) else {
            if let Some((member_name, qualifier)) =
                self.module_constructor_boundary_from_qualified_identifier(name)
            {
                if self.strict_mode {
                    return Err(Self::boxed(
                        cross_module_constructor_access_error(
                            span,
                            member_name.as_str(),
                            qualifier.as_str(),
                        )
                        .with_file(self.file_path.clone()),
                    ));
                }
                self.warnings.push(
                    cross_module_constructor_access_warning(
                        span,
                        member_name.as_str(),
                        qualifier.as_str(),
                    )
                    .with_file(self.file_path.clone()),
                );
                for arg in arguments {
                    self.compile_non_tail_expression(arg)?;
                }
                let const_idx = self.add_constant(Value::String(Rc::new(member_name.clone())));
                self.emit(OpCode::OpMakeAdt, &[const_idx, arguments.len()]);
                return Ok(true);
            }
            return Ok(false);
        };

        if let Some(module_name) = boundary_module
            && self.current_module_prefix != Some(module_name)
        {
            let module_name_str = self.sym(module_name).to_string();
            let ctor_name_str = self.sym(name).to_string();
            if self.strict_mode {
                return Err(Self::boxed(
                    cross_module_constructor_access_error(
                        span,
                        ctor_name_str.as_str(),
                        module_name_str.as_str(),
                    )
                    .with_file(self.file_path.clone()),
                ));
            }
            self.warnings.push(
                cross_module_constructor_access_warning(
                    span,
                    ctor_name_str.as_str(),
                    module_name_str.as_str(),
                )
                .with_file(self.file_path.clone()),
            );
        }

        let expected_arity = info.arity;
        let actual_arity = arguments.len();

        if actual_arity != expected_arity {
            let name_str = self.interner.resolve(name).to_string();
            return Err(Self::boxed(
                diagnostic_for(&CONSTRUCTOR_ARITY_MISMATCH)
                    .with_span(span)
                    .with_message(format!(
                        "Constructor `{}` expects {} argument(s) but got {}.",
                        name_str, expected_arity, actual_arity
                    )),
            ));
        }

        // Compile each argument
        for arg in arguments {
            self.compile_non_tail_expression(arg)?;
        }

        // Add constructor name as a string constant
        let constructor_name = self.interner.resolve(name).to_string();
        let const_idx = self.add_constant(Value::String(Rc::new(constructor_name.clone())));
        self.emit(OpCode::OpMakeAdt, &[const_idx, actual_arity]);

        Ok(true)
    }

    fn check_match_exhaustiveness(
        &self,
        scrutinee: &Expression,
        arms: &[MatchArm],
        span: Span,
    ) -> CompileResult<()> {
        let constructor_names: Vec<Symbol> = arms
            .iter()
            .filter_map(|arm| {
                if let Pattern::Constructor { name, .. } = &arm.pattern {
                    Some(*name)
                } else {
                    None
                }
            })
            .collect();

        if !constructor_names.is_empty()
            && constructor_names
                .iter()
                .all(|name| self.adt_registry.lookup_constructor(*name).is_some())
        {
            return self.check_adt_match_exhaustiveness(arms, span);
        }

        self.check_general_match_exhaustiveness(scrutinee, arms, span)
    }

    fn check_general_match_exhaustiveness(
        &self,
        scrutinee: &Expression,
        arms: &[MatchArm],
        span: Span,
    ) -> CompileResult<()> {
        if arms.iter().any(|arm| {
            arm.guard.is_none()
                && matches!(
                    arm.pattern,
                    Pattern::Wildcard { .. } | Pattern::Identifier { .. }
                )
        }) {
            return Ok(());
        }

        let domain = self.infer_general_match_domain(scrutinee, arms);
        match domain {
            GeneralCoverageDomain::Bool => {
                let mut seen_true = false;
                let mut seen_false = false;
                for arm in arms.iter().filter(|arm| arm.guard.is_none()) {
                    if let Pattern::Literal { expression, .. } = &arm.pattern
                        && let Expression::Boolean { value, .. } = expression
                    {
                        if *value {
                            seen_true = true;
                        } else {
                            seen_false = true;
                        }
                    }
                }
                if seen_true && seen_false {
                    return Ok(());
                }
                let mut missing = Vec::new();
                if !seen_true {
                    missing.push("true");
                }
                if !seen_false {
                    missing.push("false");
                }
                let missing_text = missing.join(", ");
                Err(Self::boxed(
                    diagnostic_for(&NON_EXHAUSTIVE_MATCH)
                        .with_span(span)
                        .with_message(format!(
                            "Match is non-exhaustive: missing Bool case(s): {}.",
                            missing_text
                        ))
                        .with_hint_text(
                            "Add missing boolean arms or an unguarded `_ -> ...` catch-all."
                                .to_string(),
                        ),
                ))
            }
            GeneralCoverageDomain::Option => {
                let mut seen_none = false;
                let mut seen_some = false;
                for arm in arms.iter().filter(|arm| arm.guard.is_none()) {
                    match &arm.pattern {
                        Pattern::None { .. } => seen_none = true,
                        Pattern::Some { .. } => seen_some = true,
                        _ => {}
                    }
                }
                if seen_none && seen_some {
                    return Ok(());
                }
                let mut missing = Vec::new();
                if !seen_none {
                    missing.push("None");
                }
                if !seen_some {
                    missing.push("Some(_)");
                }
                let missing_text = missing.join(", ");
                Err(Self::boxed(
                    diagnostic_for(&NON_EXHAUSTIVE_MATCH)
                        .with_span(span)
                        .with_message(format!(
                            "Match is non-exhaustive: missing Option case(s): {}.",
                            missing_text
                        ))
                        .with_hint_text(
                            "Add missing Option arms or an unguarded `_ -> ...` catch-all."
                                .to_string(),
                        ),
                ))
            }
            GeneralCoverageDomain::Either => {
                let mut seen_left = false;
                let mut seen_right = false;
                for arm in arms.iter().filter(|arm| arm.guard.is_none()) {
                    match &arm.pattern {
                        Pattern::Left { .. } => seen_left = true,
                        Pattern::Right { .. } => seen_right = true,
                        _ => {}
                    }
                }
                if seen_left && seen_right {
                    return Ok(());
                }
                let mut missing = Vec::new();
                if !seen_left {
                    missing.push("Left(_)");
                }
                if !seen_right {
                    missing.push("Right(_)");
                }
                let missing_text = missing.join(", ");
                Err(Self::boxed(
                    diagnostic_for(&NON_EXHAUSTIVE_MATCH)
                        .with_span(span)
                        .with_message(format!(
                            "Match is non-exhaustive: missing Either case(s): {}.",
                            missing_text
                        ))
                        .with_hint_text(
                            "Add missing Either arms or an unguarded `_ -> ...` catch-all."
                                .to_string(),
                        ),
                ))
            }
            GeneralCoverageDomain::ListLike => {
                let mut seen_empty = false;
                let mut seen_cons = false;
                for arm in arms.iter().filter(|arm| arm.guard.is_none()) {
                    match &arm.pattern {
                        Pattern::EmptyList { .. } => seen_empty = true,
                        Pattern::Cons { .. } => seen_cons = true,
                        _ => {}
                    }
                }
                if seen_empty && seen_cons {
                    return Ok(());
                }
                let mut missing = Vec::new();
                if !seen_empty {
                    missing.push("[]");
                }
                if !seen_cons {
                    missing.push("[h | t]");
                }
                let missing_text = missing.join(", ");
                Err(Self::boxed(
                    diagnostic_for(&NON_EXHAUSTIVE_MATCH)
                        .with_span(span)
                        .with_message(format!(
                            "Match is non-exhaustive: missing list case(s): {}.",
                            missing_text
                        ))
                        .with_hint_text(
                            "Add missing list arms or an unguarded `_ -> ...` catch-all."
                                .to_string(),
                        ),
                ))
            }
            GeneralCoverageDomain::Tuple(_) => {
                if Self::has_guarded_wildcard_without_unguarded_catchall(arms) {
                    return Err(Self::boxed(guarded_wildcard_non_exhaustive(span)));
                }
                Err(Self::boxed(
                    diagnostic_for(&NON_EXHAUSTIVE_MATCH)
                        .with_span(span)
                        .with_message(
                            "Match over tuple domains is conservatively non-exhaustive without an unguarded catch-all arm."
                                .to_string(),
                        )
                        .with_hint_text(
                            "Add an unguarded `_ -> ...` arm. Tuple exhaustiveness is checked conservatively.".to_string(),
                        ),
                ))
            }
            GeneralCoverageDomain::Unknown => {
                if Self::has_guarded_wildcard_without_unguarded_catchall(arms) {
                    return Err(Self::boxed(guarded_wildcard_non_exhaustive(span)));
                }
                Err(Self::boxed(
                    diagnostic_for(&NON_EXHAUSTIVE_MATCH)
                        .with_span(span)
                        .with_message(
                            "Match is non-exhaustive without an unguarded catch-all arm."
                                .to_string(),
                        )
                        .with_hint_text(
                            "Add an unguarded `_ -> ...` arm for conservative exhaustive coverage."
                                .to_string(),
                        ),
                ))
            }
        }
    }

    fn has_guarded_wildcard_without_unguarded_catchall(arms: &[MatchArm]) -> bool {
        let has_unguarded_catchall = arms.iter().any(|arm| {
            arm.guard.is_none()
                && matches!(
                    arm.pattern,
                    Pattern::Wildcard { .. } | Pattern::Identifier { .. }
                )
        });
        if has_unguarded_catchall {
            return false;
        }

        arms.iter().any(|arm| {
            arm.guard.is_some()
                && matches!(
                    arm.pattern,
                    Pattern::Wildcard { .. } | Pattern::Identifier { .. }
                )
        })
    }

    fn infer_general_match_domain(
        &self,
        scrutinee: &Expression,
        arms: &[MatchArm],
    ) -> GeneralCoverageDomain {
        if let crate::bytecode::compiler::hm_expr_typer::HmExprTypeResult::Known(ty) =
            self.hm_expr_type_strict_path(scrutinee)
            && let Some(domain) = Self::domain_from_infer_type(&ty)
        {
            return domain;
        }
        Self::domain_from_unguarded_patterns(arms)
    }

    fn domain_from_infer_type(ty: &InferType) -> Option<GeneralCoverageDomain> {
        match ty {
            InferType::Con(crate::types::type_constructor::TypeConstructor::Bool) => {
                Some(GeneralCoverageDomain::Bool)
            }
            InferType::App(crate::types::type_constructor::TypeConstructor::Option, _) => {
                Some(GeneralCoverageDomain::Option)
            }
            InferType::App(crate::types::type_constructor::TypeConstructor::Either, _) => {
                Some(GeneralCoverageDomain::Either)
            }
            InferType::App(crate::types::type_constructor::TypeConstructor::List, _)
            | InferType::App(crate::types::type_constructor::TypeConstructor::Array, _) => {
                Some(GeneralCoverageDomain::ListLike)
            }
            InferType::Tuple(elements) => Some(GeneralCoverageDomain::Tuple(elements.len())),
            _ => None,
        }
    }

    fn domain_from_unguarded_patterns(arms: &[MatchArm]) -> GeneralCoverageDomain {
        let patterns: Vec<&Pattern> = arms
            .iter()
            .filter(|arm| arm.guard.is_none())
            .map(|arm| &arm.pattern)
            .collect();
        if patterns.is_empty() {
            return GeneralCoverageDomain::Unknown;
        }

        if patterns.iter().all(|p| {
            matches!(
                p,
                Pattern::Literal {
                    expression: Expression::Boolean { .. },
                    ..
                }
            )
        }) {
            return GeneralCoverageDomain::Bool;
        }
        if patterns
            .iter()
            .all(|p| matches!(p, Pattern::None { .. } | Pattern::Some { .. }))
        {
            return GeneralCoverageDomain::Option;
        }
        if patterns
            .iter()
            .all(|p| matches!(p, Pattern::Left { .. } | Pattern::Right { .. }))
        {
            return GeneralCoverageDomain::Either;
        }
        if patterns
            .iter()
            .all(|p| matches!(p, Pattern::EmptyList { .. } | Pattern::Cons { .. }))
        {
            return GeneralCoverageDomain::ListLike;
        }
        if let Some(tuple_arity) = patterns.iter().find_map(|p| match p {
            Pattern::Tuple { elements, .. } => Some(elements.len()),
            _ => None,
        }) && patterns
            .iter()
            .all(|p| matches!(p, Pattern::Tuple { elements, .. } if elements.len() == tuple_arity))
        {
            return GeneralCoverageDomain::Tuple(tuple_arity);
        }

        GeneralCoverageDomain::Unknown
    }

    fn check_adt_match_exhaustiveness(&self, arms: &[MatchArm], span: Span) -> CompileResult<()> {
        // Collect constructor patterns:
        // - `all_constructor_names`: any constructor arm (guarded or unguarded)
        // - `constructor_names`: unguarded constructor arms only (these can prove coverage)
        let all_constructor_names: Vec<Symbol> = arms
            .iter()
            .filter_map(|arm| {
                if let Pattern::Constructor { name, .. } = &arm.pattern {
                    Some(*name)
                } else {
                    None
                }
            })
            .collect();

        // Guarded constructor arms do not prove exhaustiveness.
        let constructor_names: Vec<Symbol> = arms
            .iter()
            .filter_map(|arm| {
                if arm.guard.is_none()
                    && let Pattern::Constructor { name, .. } = &arm.pattern
                {
                    Some(*name)
                } else {
                    None
                }
            })
            .collect();

        // If no constructor patterns at all, nothing to check
        if all_constructor_names.is_empty() {
            return Ok(());
        }

        // Look up the ADT from the first constructor name.
        let first_constructor = all_constructor_names[0];
        let Some(constructor_info) = self.adt_registry.lookup_constructor(first_constructor) else {
            return Ok(()); // Unknown constructor, already reported elsewhere
        };
        let adt_name = constructor_info.adt_name;
        let Some(adt_def) = self.adt_registry.lookup_adt(adt_name) else {
            return Ok(());
        };

        // Constructor arms for exhaustiveness must belong to the same ADT.
        for constructor_name in &all_constructor_names {
            let Some(info) = self.adt_registry.lookup_constructor(*constructor_name) else {
                continue;
            };
            if info.adt_name != adt_name {
                let first_adt = self.interner.resolve(adt_name).to_string();
                let mixed_adt = self.interner.resolve(info.adt_name).to_string();
                return Err(Self::boxed(
                    diagnostic_for(&ADT_NON_EXHAUSTIVE_MATCH)
                        .with_span(span)
                        .with_message(format!(
                            "Match arms mix constructors from different ADTs: `{}` and `{}`.",
                            first_adt, mixed_adt
                        ))
                        .with_hint_text(
                            "Use constructors from a single ADT in a given match expression."
                                .to_string(),
                        ),
                ));
            }
        }

        // If any arm has a wildcard or identifier (catch-all), it's exhaustive
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

        // If all constructor arms are guarded and there is no unguarded catch-all,
        // the match is non-exhaustive because guards may fail.
        if constructor_names.is_empty() {
            let adt_name_str = self.interner.resolve(adt_name).to_string();
            let missing_list = adt_def
                .constructors
                .iter()
                .map(|(name, ..)| self.interner.resolve(*name))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(Self::boxed(
                diagnostic_for(&ADT_NON_EXHAUSTIVE_MATCH)
                    .with_span(span)
                    .with_message(format!(
                        "Match on `{}` is non-exhaustive because all constructor arms are guarded.",
                        adt_name_str
                    ))
                    .with_hint_text(format!(
                        "Add unguarded arms for {} or add a `_ -> ...` catch-all.",
                        missing_list
                    )),
            ));
        }

        // Check if all constructors are covered
        let covered: HashSet<Symbol> = constructor_names.into_iter().collect();
        let missing: Vec<&str> = adt_def
            .constructors
            .iter()
            .filter(|(name, ..)| !covered.contains(name))
            .map(|(name, ..)| self.interner.resolve(*name))
            .collect();

        if !missing.is_empty() {
            let adt_name_str = self.interner.resolve(adt_name).to_string();
            let missing_list = missing.join(", ");
            return Err(Self::boxed(
                diagnostic_for(&ADT_NON_EXHAUSTIVE_MATCH)
                    .with_span(span)
                    .with_message(format!(
                        "Match on `{}` is missing constructors: {}.",
                        adt_name_str, missing_list
                    ))
                    .with_hint_text(format!(
                        "Add arms for {} or add a `_ -> ...` catch-all.",
                        missing_list
                    )),
            ));
        }

        // Stronger nested check (v0.0.4 scope):
        // For unary constructors, verify nested constructor-space coverage when
        // arms constrain the nested field with constructor patterns.
        self.check_nested_constructor_exhaustiveness(arms, adt_name, adt_def, span)?;

        Ok(())
    }

    fn check_nested_constructor_exhaustiveness(
        &self,
        arms: &[MatchArm],
        _adt_name: Symbol,
        adt_def: &crate::bytecode::compiler::adt_definition::AdtDefinition,
        span: Span,
    ) -> CompileResult<()> {
        for (outer_ctor_name, outer_arity, _) in &adt_def.constructors {
            let mut ctor_fields: Vec<&[Pattern]> = Vec::new();
            for arm in arms {
                if arm.guard.is_some() {
                    continue;
                }
                let Pattern::Constructor { name, fields, .. } = &arm.pattern else {
                    continue;
                };
                if name != outer_ctor_name {
                    continue;
                }
                ctor_fields.push(fields.as_slice());
            }

            if ctor_fields.is_empty() || *outer_arity == 0 {
                continue;
            }

            for field_idx in 0..*outer_arity {
                let field_patterns: Vec<&Pattern> = ctor_fields
                    .iter()
                    .filter_map(|fields| fields.get(field_idx))
                    .collect();
                if field_patterns.is_empty() {
                    continue;
                }

                self.check_nested_pattern_set(
                    &field_patterns,
                    &format!(
                        "under constructor `{}` field #{}",
                        self.interner.resolve(*outer_ctor_name),
                        field_idx + 1
                    ),
                    span,
                )?;
            }
        }

        Ok(())
    }

    fn check_nested_pattern_set(
        &self,
        patterns: &[&Pattern],
        context: &str,
        span: Span,
    ) -> CompileResult<()> {
        if patterns.is_empty() || patterns.iter().any(|p| self.is_irrefutable_pattern(p)) {
            return Ok(());
        }

        let ctor_patterns: Vec<(Symbol, &[Pattern])> = patterns
            .iter()
            .filter_map(|p| {
                if let Pattern::Constructor { name, fields, .. } = p {
                    Some((*name, fields.as_slice()))
                } else {
                    None
                }
            })
            .collect();

        // If we have constructor patterns and no irrefutable branch, attempt ADT coverage.
        if !ctor_patterns.is_empty() {
            if ctor_patterns.len() != patterns.len() {
                return Ok(());
            }

            let Some(first_ctor_info) = self.adt_registry.lookup_constructor(ctor_patterns[0].0)
            else {
                return Ok(());
            };
            let nested_adt_name = first_ctor_info.adt_name;
            let Some(nested_adt_def) = self.adt_registry.lookup_adt(nested_adt_name) else {
                return Ok(());
            };

            for (ctor_name, _) in &ctor_patterns {
                let Some(info) = self.adt_registry.lookup_constructor(*ctor_name) else {
                    continue;
                };
                if info.adt_name != nested_adt_name {
                    return Err(Self::boxed(
                        diagnostic_for(&ADT_NON_EXHAUSTIVE_MATCH)
                            .with_span(span)
                            .with_message(format!(
                                "Nested constructor patterns {} mix ADTs.",
                                context
                            ))
                            .with_hint_text(
                                "Use constructors from a single ADT in a nested pattern set."
                                    .to_string(),
                            ),
                    ));
                }
            }

            let covered: HashSet<Symbol> = ctor_patterns.iter().map(|(name, _)| *name).collect();
            let missing: Vec<&str> = nested_adt_def
                .constructors
                .iter()
                .filter(|(name, ..)| !covered.contains(name))
                .map(|(name, ..)| self.interner.resolve(*name))
                .collect();

            if !missing.is_empty() {
                let nested_adt = self.interner.resolve(nested_adt_name);
                let missing_list = missing.join(", ");
                return Err(Self::boxed(
                    diagnostic_for(&ADT_NON_EXHAUSTIVE_MATCH)
                        .with_span(span)
                        .with_message(format!(
                            "Match is non-exhaustive: nested `{}` patterns {} miss constructors: {}.",
                            nested_adt, context, missing_list
                        ))
                        .with_hint_text(format!(
                            "Add nested arms for {} or use a nested catch-all (`_`).",
                            missing_list
                        )),
                ));
            }

            // Recurse into constructor fields.
            for (ctor_name, arity, _) in &nested_adt_def.constructors {
                let ctor_rows: Vec<&[Pattern]> = ctor_patterns
                    .iter()
                    .filter_map(|(name, fields)| {
                        if name == ctor_name {
                            Some(*fields)
                        } else {
                            None
                        }
                    })
                    .collect();
                if ctor_rows.is_empty() || *arity == 0 {
                    continue;
                }
                for field_idx in 0..*arity {
                    let next: Vec<&Pattern> = ctor_rows
                        .iter()
                        .filter_map(|fields| fields.get(field_idx))
                        .collect();
                    if next.is_empty() {
                        continue;
                    }
                    self.check_nested_pattern_set(
                        &next,
                        &format!(
                            "{} -> `{}` field #{}",
                            context,
                            self.interner.resolve(*ctor_name),
                            field_idx + 1
                        ),
                        span,
                    )?;
                }
            }

            return Ok(());
        }

        // Tuple nested coverage: recurse per position when all nested patterns are tuples
        // with the same arity.
        if let Some(tuple_len) = patterns.iter().find_map(|p| {
            if let Pattern::Tuple { elements, .. } = p {
                Some(elements.len())
            } else {
                None
            }
        }) && patterns
            .iter()
            .all(|p| matches!(p, Pattern::Tuple { elements, .. } if elements.len() == tuple_len))
        {
            for idx in 0..tuple_len {
                let next: Vec<&Pattern> = patterns
                    .iter()
                    .filter_map(|p| match p {
                        Pattern::Tuple { elements, .. } => elements.get(idx),
                        _ => None,
                    })
                    .collect();
                if next.is_empty() {
                    continue;
                }
                self.check_nested_pattern_set(
                    &next,
                    &format!("{} -> tuple position #{}", context, idx + 1),
                    span,
                )?;
            }
        } else if patterns.iter().any(|p| matches!(p, Pattern::Tuple { .. })) {
            return Err(Self::boxed(
                diagnostic_for(&ADT_NON_EXHAUSTIVE_MATCH)
                    .with_span(span)
                    .with_message(format!(
                        "Match is non-exhaustive: nested tuple patterns {} are mixed-shape and cannot be proven exhaustive conservatively.",
                        context
                    ))
                    .with_hint_text(
                        "Use consistent tuple shapes in nested patterns or add a nested catch-all (`_`)."
                            .to_string(),
                    ),
            ));
        }

        // List nested coverage: enforce empty/non-empty partition when list patterns are used.
        let mut has_empty = false;
        let mut has_cons = false;
        for p in patterns {
            match p {
                Pattern::EmptyList { .. } => has_empty = true,
                Pattern::Cons { .. } => has_cons = true,
                _ => {}
            }
        }
        if has_empty || has_cons {
            if !has_empty {
                return Err(Self::boxed(
                    diagnostic_for(&ADT_NON_EXHAUSTIVE_MATCH)
                        .with_span(span)
                        .with_message(format!(
                            "Match is non-exhaustive: nested list patterns {} miss the empty list case.",
                            context
                        ))
                        .with_hint_text(
                            "Add a `[]` nested pattern or a nested catch-all (`_`).".to_string(),
                        ),
                ));
            }
            if !has_cons {
                return Err(Self::boxed(
                    diagnostic_for(&ADT_NON_EXHAUSTIVE_MATCH)
                        .with_span(span)
                        .with_message(format!(
                            "Match is non-exhaustive: nested list patterns {} miss non-empty list cases.",
                            context
                        ))
                        .with_hint_text(
                            "Add a `[h | t]` nested pattern or a nested catch-all (`_`)."
                                .to_string(),
                        ),
                ));
            }
        }

        Ok(())
    }

    fn is_irrefutable_pattern(&self, pattern: &Pattern) -> bool {
        matches!(
            pattern,
            Pattern::Wildcard { .. } | Pattern::Identifier { .. }
        )
    }

    // ── Unreachable arm detection ──────────────────────────────────────────

    /// Collect W202 warnings for arms whose patterns are provably subsumed by
    /// an earlier unguarded arm.  Conservative: only patterns that can be
    /// structurally proven unreachable are reported.  Guarded arms are never
    /// considered to subsume later arms (a guard may fail at runtime).
    fn collect_unreachable_arm_warnings(&self, arms: &[MatchArm]) -> Vec<Diagnostic> {
        let mut diags = Vec::new();
        // Build the list of preceding unguarded patterns as we go.
        let mut covered: Vec<&Pattern> = Vec::new();

        for arm in arms {
            // A guarded arm can never be proven unreachable — the guard may fail.
            if arm.guard.is_none() {
                let is_unreachable = covered
                    .iter()
                    .any(|prev| Self::pattern_subsumes(prev, &arm.pattern));

                if is_unreachable {
                    diags.push(
                        Diagnostic::make_warning_from_code(
                            &UNREACHABLE_PATTERN_ARM,
                            &[],
                            self.file_path.clone(),
                            arm.pattern.span(),
                        )
                        .with_primary_label(arm.pattern.span(), "unreachable arm"),
                    );
                } else {
                    // Only add to covered set if this arm actually extends coverage.
                    covered.push(&arm.pattern);
                }
            }
        }

        diags
    }

    /// Returns `true` if every value matched by `specific` is also matched by
    /// `general`, meaning `specific` is fully covered when `general` fires first.
    ///
    /// Only covers cases that can be determined structurally without type
    /// information (conservative: returns `false` on ambiguous cases).
    fn pattern_subsumes(general: &Pattern, specific: &Pattern) -> bool {
        match general {
            // Wildcard/identifier catch-alls subsume everything.
            Pattern::Wildcard { .. } | Pattern::Identifier { .. } => true,

            // None / EmptyList are atomic — only subsume themselves.
            Pattern::None { .. } => matches!(specific, Pattern::None { .. }),
            Pattern::EmptyList { .. } => matches!(specific, Pattern::EmptyList { .. }),

            // Wrapper patterns: subsume if the inner pattern subsumes too.
            Pattern::Some {
                pattern: inner_g, ..
            } => {
                if let Pattern::Some {
                    pattern: inner_s, ..
                } = specific
                {
                    Self::pattern_subsumes(inner_g, inner_s)
                } else {
                    false
                }
            }
            Pattern::Left {
                pattern: inner_g, ..
            } => {
                if let Pattern::Left {
                    pattern: inner_s, ..
                } = specific
                {
                    Self::pattern_subsumes(inner_g, inner_s)
                } else {
                    false
                }
            }
            Pattern::Right {
                pattern: inner_g, ..
            } => {
                if let Pattern::Right {
                    pattern: inner_s, ..
                } = specific
                {
                    Self::pattern_subsumes(inner_g, inner_s)
                } else {
                    false
                }
            }

            // Cons: both head and tail must be subsumed.
            Pattern::Cons {
                head: hg, tail: tg, ..
            } => {
                if let Pattern::Cons {
                    head: hs, tail: ts, ..
                } = specific
                {
                    Self::pattern_subsumes(hg, hs) && Self::pattern_subsumes(tg, ts)
                } else {
                    false
                }
            }

            // Tuple: element-wise subsumption (same arity required).
            Pattern::Tuple {
                elements: elems_g, ..
            } => {
                if let Pattern::Tuple {
                    elements: elems_s, ..
                } = specific
                    && elems_g.len() == elems_s.len()
                {
                    elems_g
                        .iter()
                        .zip(elems_s.iter())
                        .all(|(g, s)| Self::pattern_subsumes(g, s))
                } else {
                    false
                }
            }

            // ADT Constructor: same name and field-wise subsumption.
            Pattern::Constructor {
                name: name_g,
                fields: fields_g,
                ..
            } => {
                if let Pattern::Constructor {
                    name: name_s,
                    fields: fields_s,
                    ..
                } = specific
                    && name_g == name_s
                    && fields_g.len() == fields_s.len()
                {
                    fields_g
                        .iter()
                        .zip(fields_s.iter())
                        .all(|(g, s)| Self::pattern_subsumes(g, s))
                } else {
                    false
                }
            }

            // Literal: subsumes the same literal value only.
            Pattern::Literal {
                expression: expr_g, ..
            } => {
                if let Pattern::Literal {
                    expression: expr_s, ..
                } = specific
                {
                    Self::literals_equal(expr_g, expr_s)
                } else {
                    false
                }
            }
        }
    }

    fn literals_equal(a: &Expression, b: &Expression) -> bool {
        match (a, b) {
            (Expression::Integer { value: v1, .. }, Expression::Integer { value: v2, .. }) => {
                v1 == v2
            }
            (Expression::Float { value: v1, .. }, Expression::Float { value: v2, .. }) => v1 == v2,
            (Expression::Boolean { value: v1, .. }, Expression::Boolean { value: v2, .. }) => {
                v1 == v2
            }
            (Expression::String { value: v1, .. }, Expression::String { value: v2, .. }) => {
                v1 == v2
            }
            _ => false,
        }
    }

    /// Try to resolve a class method call at compile time.
    ///
    /// If `name` is a class method and the first argument's HM-inferred type
    /// is concrete, returns the mangled instance function symbol.
    fn try_resolve_class_method_call(
        &self,
        name: crate::syntax::Identifier,
        arguments: &[Expression],
    ) -> Option<crate::syntax::Identifier> {
        if self.class_env.classes.is_empty() {
            return None;
        }
        let (class_name, _) = self.class_env.method_to_class(name)?;

        // Try compile-time resolution: if the first argument's type is concrete,
        // resolve directly to the mangled instance function.
        if let Some(first_arg) = arguments.first()
            && let Some(first_arg_type) = self.hm_expr_types.get(&first_arg.expr_id())
            && let Some((instance, _)) = self.class_env.resolve_instance_with_subst(
                class_name,
                std::slice::from_ref(first_arg_type),
                &self.interner,
            )
        {
            // Build mangled name from all instance type args (multi-param support).
            let type_key = instance
                .type_args
                .iter()
                .map(|a| a.display_with(&self.interner))
                .collect::<Vec<_>>()
                .join("_");
            let class_str = self.interner.resolve(class_name);
            let method_str = self.interner.resolve(name);
            let mangled = format!("__tc_{class_str}_{type_key}_{method_str}");
            if let Some(sym) = self.interner.lookup(&mangled) {
                return Some(sym);
            }
        }

        // No compile-time resolution possible — return None.
        // Dictionary elaboration handles polymorphic calls via dict params.
        None
    }

    fn try_build_dict_class_method_call(
        &mut self,
        name: crate::syntax::Identifier,
        function_span: Span,
        arguments: &[Expression],
        call_span: Span,
    ) -> Option<Expression> {
        if self.class_env.classes.is_empty() {
            return None;
        }
        let (class_name, _) = self.class_env.method_to_class(name)?;
        let method_index = self.class_env.method_index(class_name, name)?;
        let class_str = self.interner.resolve(class_name);
        let dict_name = format!("__dict_{class_str}");
        let dict_sym = self.interner.lookup(&dict_name)?;
        self.symbol_table.resolve(dict_sym)?;

        Some(Expression::Call {
            function: Box::new(Expression::TupleFieldAccess {
                object: Box::new(Expression::Identifier {
                    name: dict_sym,
                    span: function_span,
                    id: crate::syntax::expression::ExprId::UNSET,
                }),
                index: method_index,
                span: function_span,
                id: crate::syntax::expression::ExprId::UNSET,
            }),
            arguments: arguments.to_vec(),
            span: call_span,
            id: crate::syntax::expression::ExprId::UNSET,
        })
    }

    fn resolve_direct_class_call_dict_args_ast(
        &self,
        method_name: crate::syntax::Identifier,
        arguments: &[Expression],
        span: Span,
    ) -> Vec<Expression> {
        let Some((class_name, _)) = self.class_env.method_to_class(method_name) else {
            return Vec::new();
        };
        let Some(first_arg) = arguments.first() else {
            return Vec::new();
        };
        let Some(first_arg_ty) = self.hm_expr_types.get(&first_arg.expr_id()) else {
            return Vec::new();
        };

        self.class_env
            .resolve_instance_context_dictionaries(
                class_name,
                std::slice::from_ref(first_arg_ty),
                &self.interner,
            )
            .map(|dicts| {
                dicts
                    .iter()
                    .map(|dict_ref| self.lower_dictionary_ref_ast(dict_ref, span))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn lower_dictionary_ref_ast(
        &self,
        dict_ref: &crate::types::class_env::ResolvedDictionaryRef,
        span: Span,
    ) -> Expression {
        if dict_ref.context_args.is_empty() {
            if let Some(methods) = self
                .class_env
                .dictionary_method_symbols(dict_ref.dict_name, &self.interner)
            {
                return Expression::TupleLiteral {
                    elements: methods
                        .into_iter()
                        .map(|name| Expression::Identifier {
                            name,
                            span,
                            id: crate::syntax::expression::ExprId::UNSET,
                        })
                        .collect(),
                    span,
                    id: crate::syntax::expression::ExprId::UNSET,
                };
            }
            return Expression::Identifier {
                name: dict_ref.dict_name,
                span,
                id: crate::syntax::expression::ExprId::UNSET,
            };
        }

        Expression::Call {
            function: Box::new(Expression::Identifier {
                name: dict_ref.dict_name,
                span,
                id: crate::syntax::expression::ExprId::UNSET,
            }),
            arguments: dict_ref
                .context_args
                .iter()
                .map(|arg| self.lower_dictionary_ref_ast(arg, span))
                .collect(),
            span,
            id: crate::syntax::expression::ExprId::UNSET,
        }
    }
}
