use crate::{
    core::CorePrimOp,
    diagnostics::{
        Diagnostic, DiagnosticBuilder, DiagnosticCategory, DiagnosticPhase, types::ErrorType,
    },
    syntax::{
        block::Block,
        expression::{Expression, StringPart},
        program::Program,
        statement::Statement,
    },
};

use super::super::{Compiler, pipeline::CollectionResult};

enum LegacyHelperSurfacePolicy {
    Allow,
    Warn,
    Error,
}

impl Compiler {
    /// Phase 1: Collect definitions and validate program structure.
    pub(in crate::compiler) fn phase_collection(&mut self, program: &Program) -> CollectionResult {
        self.warn_on_legacy_builtin_helper_surface(program);
        self.validate_intrinsic_surface(program);
        // Proposal 0161 B1: pre-collect effect-row aliases so downstream
        // contract collection and validators can resolve `with Alias`
        // references. The full effect_ops_registry collection still happens
        // below; this step only touches `effect_row_aliases`.
        self.collect_effect_aliases_for_contracts(program);
        self.collect_module_function_visibility(program);
        self.collect_module_adt_constructors(program);
        self.collect_module_contracts(program);
        // Re-apply Flow stdlib auto-exposure after refreshing module visibility
        // for the current compilation unit. This keeps VM/import resolution in
        // sync with the latest collected Flow members.
        self.auto_expose_flow_modules();
        self.infer_unannotated_function_effects(program);
        self.collect_adt_definitions(program);
        self.collect_effect_declarations(program);
        self.collect_class_declarations(program);
        // Proposal 0151, Phase 3: catch import-collision diagnostics
        // (E457, E458) introduced by `exposing (...)` clauses.
        self.validate_import_collisions(program);
        // Proposal 0151, Phase 4a: enforce floor semantics on
        // `with`-annotated class methods (E452).
        self.validate_class_method_effect_floor(program);
        let main_state = self.validate_main_entrypoint(program);
        self.validate_top_level_effectful_code(program, main_state.has_main);
        self.validate_main_root_effect_discharge(program, main_state);
        self.validate_strict_mode(program, main_state.has_main);

        CollectionResult { main_state }
    }

    pub(in crate::compiler) fn validate_reserved_primop_names(&mut self, program: &Program) {
        if self
            .file_path
            .replace('\\', "/")
            .ends_with("lib/Flow/Primops.flx")
        {
            return;
        }
        for statement in &program.statements {
            self.validate_reserved_primop_statement(statement);
        }
    }

    fn validate_reserved_primop_statement(&mut self, statement: &Statement) {
        match statement {
            Statement::Function {
                name, body, span, ..
            } => {
                if self.sym(*name).starts_with("__primop_") {
                    self.errors.push(
                        Diagnostic::make_error_dynamic(
                            "E034",
                            "RESERVED PRIMOP NAME",
                            ErrorType::Compiler,
                            format!(
                                "`{}` is reserved for compiler-synthesized effect handlers.",
                                self.sym(*name)
                            ),
                            Some("Choose a non-reserved function name.".to_string()),
                            self.file_path.clone(),
                            *span,
                        )
                        .with_category(DiagnosticCategory::NameResolution)
                        .with_phase(DiagnosticPhase::Validation)
                        .with_primary_label(*span, "reserved internal primop name"),
                    );
                }
                self.validate_reserved_primop_block(body);
            }
            Statement::Let {
                name, value, span, ..
            } => {
                if self.sym(*name).starts_with("__primop_") {
                    self.errors.push(
                        Diagnostic::make_error_dynamic(
                            "E034",
                            "RESERVED PRIMOP NAME",
                            ErrorType::Compiler,
                            format!(
                                "`{}` is reserved for compiler-synthesized effect handlers.",
                                self.sym(*name)
                            ),
                            Some("Choose a non-reserved binding name.".to_string()),
                            self.file_path.clone(),
                            *span,
                        )
                        .with_category(DiagnosticCategory::NameResolution)
                        .with_phase(DiagnosticPhase::Validation)
                        .with_primary_label(*span, "reserved internal primop name"),
                    );
                }
                self.validate_reserved_primop_expression(value);
            }
            Statement::LetDestructure { value, .. } | Statement::Assign { value, .. } => {
                self.validate_reserved_primop_expression(value);
            }
            Statement::Return {
                value: Some(value), ..
            } => {
                self.validate_reserved_primop_expression(value);
            }
            Statement::Return { value: None, .. } => {}
            Statement::Expression { expression, .. } => {
                self.validate_reserved_primop_expression(expression);
            }
            Statement::Module { body, .. } => self.validate_reserved_primop_block(body),
            Statement::Import { .. }
            | Statement::Data { .. }
            | Statement::EffectDecl { .. }
            | Statement::EffectAlias { .. }
            | Statement::Class { .. }
            | Statement::Instance { .. } => {}
        }
    }

    fn validate_reserved_primop_block(&mut self, block: &Block) {
        for statement in &block.statements {
            self.validate_reserved_primop_statement(statement);
        }
    }

    fn validate_reserved_primop_expression(&mut self, expression: &Expression) {
        match expression {
            Expression::Identifier { name, span, .. }
                if self.sym(*name).starts_with("__primop_") =>
            {
                self.errors.push(
                    Diagnostic::make_error_dynamic(
                        "E034",
                        "RESERVED PRIMOP NAME",
                        ErrorType::Compiler,
                        format!(
                            "`{}` is reserved for compiler-synthesized effect handlers.",
                            self.sym(*name)
                        ),
                        Some("Call the public effectful operation instead.".to_string()),
                        self.file_path.clone(),
                        *span,
                    )
                    .with_category(DiagnosticCategory::NameResolution)
                    .with_phase(DiagnosticPhase::Validation)
                    .with_primary_label(*span, "reserved internal primop call"),
                );
            }
            _ => {}
        }
        self.validate_reserved_primop_expression_children(expression);
    }

    fn validate_reserved_primop_expression_children(&mut self, expression: &Expression) {
        match expression {
            Expression::Function { body, .. } | Expression::DoBlock { block: body, .. } => {
                self.validate_reserved_primop_block(body);
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                self.validate_reserved_primop_expression(condition);
                self.validate_reserved_primop_block(consequence);
                if let Some(alt) = alternative {
                    self.validate_reserved_primop_block(alt);
                }
            }
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                self.validate_reserved_primop_expression(function);
                for argument in arguments {
                    self.validate_reserved_primop_expression(argument);
                }
            }
            Expression::Infix { left, right, .. } => {
                self.validate_reserved_primop_expression(left);
                self.validate_reserved_primop_expression(right);
            }
            Expression::Prefix { right, .. } => self.validate_reserved_primop_expression(right),
            Expression::Match {
                scrutinee, arms, ..
            } => {
                self.validate_reserved_primop_expression(scrutinee);
                for arm in arms {
                    if let Some(guard) = arm.guard.as_ref() {
                        self.validate_reserved_primop_expression(guard);
                    }
                    self.validate_reserved_primop_expression(&arm.body);
                }
            }
            Expression::Perform { args, .. } => {
                for arg in args {
                    self.validate_reserved_primop_expression(arg);
                }
            }
            Expression::Handle { expr, arms, .. } => {
                self.validate_reserved_primop_expression(expr);
                for arm in arms {
                    self.validate_reserved_primop_expression(&arm.body);
                }
            }
            Expression::Sealing { expr, .. }
            | Expression::MemberAccess { object: expr, .. }
            | Expression::TupleFieldAccess { object: expr, .. }
            | Expression::Some { value: expr, .. }
            | Expression::Left { value: expr, .. }
            | Expression::Right { value: expr, .. } => {
                self.validate_reserved_primop_expression(expr);
            }
            Expression::Index { left, index, .. } => {
                self.validate_reserved_primop_expression(left);
                self.validate_reserved_primop_expression(index);
            }
            Expression::ListLiteral { elements, .. }
            | Expression::ArrayLiteral { elements, .. }
            | Expression::TupleLiteral { elements, .. } => {
                for element in elements {
                    self.validate_reserved_primop_expression(element);
                }
            }
            Expression::Hash { pairs, .. } => {
                for (key, value) in pairs {
                    self.validate_reserved_primop_expression(key);
                    self.validate_reserved_primop_expression(value);
                }
            }
            Expression::Cons { head, tail, .. } => {
                self.validate_reserved_primop_expression(head);
                self.validate_reserved_primop_expression(tail);
            }
            Expression::InterpolatedString { parts, .. } => {
                for part in parts {
                    if let StringPart::Interpolation(expr) = part {
                        self.validate_reserved_primop_expression(expr);
                    }
                }
            }
            Expression::NamedConstructor { fields, .. } => {
                for field in fields {
                    if let Some(value) = field.value.as_ref() {
                        self.validate_reserved_primop_expression(value);
                    }
                }
            }
            Expression::Spread {
                base, overrides, ..
            } => {
                self.validate_reserved_primop_expression(base);
                for field in overrides {
                    if let Some(value) = field.value.as_ref() {
                        self.validate_reserved_primop_expression(value);
                    }
                }
            }
            Expression::Identifier { .. }
            | Expression::Integer { .. }
            | Expression::Float { .. }
            | Expression::String { .. }
            | Expression::Boolean { .. }
            | Expression::EmptyList { .. }
            | Expression::None { .. } => {}
        }
    }

    fn warn_on_legacy_builtin_helper_surface(&mut self, program: &Program) {
        if matches!(
            self.legacy_helper_surface_policy(),
            LegacyHelperSurfacePolicy::Allow
        ) {
            return;
        }

        for statement in &program.statements {
            self.warn_on_legacy_statement(statement);
        }
    }

    fn warn_on_legacy_statement(&mut self, statement: &Statement) {
        match statement {
            Statement::Let { value, .. }
            | Statement::LetDestructure { value, .. }
            | Statement::Assign { value, .. } => self.warn_on_legacy_expression(value),
            Statement::Return { value, .. } => {
                if let Some(value) = value {
                    self.warn_on_legacy_expression(value);
                }
            }
            Statement::Expression { expression, .. } => self.warn_on_legacy_expression(expression),
            Statement::Function { body, .. } | Statement::Module { body, .. } => {
                self.warn_on_legacy_block(body);
            }
            Statement::Import { .. }
            | Statement::Data { .. }
            | Statement::EffectDecl { .. }
            | Statement::EffectAlias { .. }
            | Statement::Class { .. }
            | Statement::Instance { .. } => {}
        }
    }

    fn validate_intrinsic_surface(&mut self, program: &Program) {
        for statement in &program.statements {
            self.validate_intrinsic_statement(statement);
        }
    }

    fn validate_intrinsic_statement(&mut self, statement: &Statement) {
        match statement {
            Statement::Function {
                intrinsic: Some(primop),
                name,
                span,
                ..
            } => {
                if !self.file_path.replace('\\', "/").contains("lib/Flow/") {
                    self.errors.push(
                        Diagnostic::make_error_dynamic(
                            "E034",
                            "INTRINSIC DECLARATION OUTSIDE STDLIB",
                            ErrorType::Compiler,
                            format!(
                                "`{}` is declared as an intrinsic bound to `{:?}` outside the Flow stdlib surface.",
                                self.sym(*name),
                                primop
                            ),
                            Some(
                                "Move this intrinsic declaration into `lib/Flow/*` or use an ordinary `fn` wrapper instead."
                                    .to_string(),
                            ),
                            self.file_path.clone(),
                            *span,
                        )
                        .with_category(DiagnosticCategory::NameResolution)
                        .with_phase(DiagnosticPhase::Validation)
                        .with_primary_label(*span, "intrinsic declarations are restricted to Flow stdlib modules"),
                    );
                }
            }
            Statement::Function { body, .. } | Statement::Module { body, .. } => {
                self.validate_intrinsic_block(body);
            }
            _ => {}
        }
    }

    fn validate_intrinsic_block(&mut self, block: &Block) {
        for statement in &block.statements {
            self.validate_intrinsic_statement(statement);
        }
    }

    fn warn_on_legacy_block(&mut self, block: &Block) {
        for statement in &block.statements {
            self.warn_on_legacy_statement(statement);
        }
    }

    fn warn_on_legacy_expression(&mut self, expression: &Expression) {
        match expression {
            Expression::Call {
                function,
                arguments,
                ..
            } => {
                if let Expression::Identifier { name, span, .. } = function.as_ref()
                    && let Some(replacement) =
                        CorePrimOp::legacy_surface_replacement(self.sym(*name), arguments.len())
                {
                    match self.legacy_helper_surface_policy() {
                        LegacyHelperSurfacePolicy::Allow => {}
                        LegacyHelperSurfacePolicy::Warn => self.warnings.push(
                            Diagnostic::make_warning(
                                "W034",
                                "Legacy Builtin Helper",
                                format!(
                                    "`{}` is a legacy builtin helper spelling; prefer `{replacement}`.",
                                    self.sym(*name)
                                ),
                                self.file_path.clone(),
                                *span,
                            )
                            .with_help(
                                "Import the owning Flow module and call the module-qualified API instead.",
                            )
                            .with_category(DiagnosticCategory::NameResolution)
                            .with_phase(DiagnosticPhase::Validation)
                            .with_primary_label(*span, "legacy helper spelling used here"),
                        ),
                        LegacyHelperSurfacePolicy::Error => self.errors.push(
                            Diagnostic::make_error_dynamic(
                                "E034",
                                "LEGACY BUILTIN HELPER DISALLOWED",
                                ErrorType::Compiler,
                                format!(
                                    "`{}` is a legacy builtin helper spelling; prefer `{replacement}`.",
                                    self.sym(*name)
                                ),
                                Some(
                                    "Import the owning Flow module and call the module-qualified API instead."
                                        .to_string(),
                                ),
                                self.file_path.clone(),
                                *span,
                            )
                            .with_category(DiagnosticCategory::NameResolution)
                            .with_phase(DiagnosticPhase::Validation)
                            .with_primary_label(*span, "legacy helper spelling used here"),
                        ),
                    }
                }

                self.warn_on_legacy_expression(function);
                for argument in arguments {
                    self.warn_on_legacy_expression(argument);
                }
            }
            Expression::Prefix { right, .. }
            | Expression::Some { value: right, .. }
            | Expression::Left { value: right, .. }
            | Expression::Right { value: right, .. }
            | Expression::TupleFieldAccess { object: right, .. } => {
                self.warn_on_legacy_expression(right);
            }
            Expression::Infix { left, right, .. }
            | Expression::Index {
                left, index: right, ..
            }
            | Expression::Cons {
                head: left,
                tail: right,
                ..
            } => {
                self.warn_on_legacy_expression(left);
                self.warn_on_legacy_expression(right);
            }
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => {
                self.warn_on_legacy_expression(condition);
                self.warn_on_legacy_block(consequence);
                if let Some(alternative) = alternative {
                    self.warn_on_legacy_block(alternative);
                }
            }
            Expression::DoBlock { block, .. } | Expression::Function { body: block, .. } => {
                self.warn_on_legacy_block(block);
            }
            Expression::ListLiteral { elements, .. }
            | Expression::ArrayLiteral { elements, .. } => {
                for element in elements {
                    self.warn_on_legacy_expression(element);
                }
            }
            Expression::TupleLiteral { elements, .. } => {
                for element in elements {
                    self.warn_on_legacy_expression(element);
                }
            }
            Expression::Hash { pairs, .. } => {
                for (key, value) in pairs {
                    self.warn_on_legacy_expression(key);
                    self.warn_on_legacy_expression(value);
                }
            }
            Expression::MemberAccess { object, .. } => self.warn_on_legacy_expression(object),
            Expression::Match {
                scrutinee, arms, ..
            } => {
                self.warn_on_legacy_expression(scrutinee);
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        self.warn_on_legacy_expression(guard);
                    }
                    self.warn_on_legacy_expression(&arm.body);
                }
            }
            Expression::Perform { args, .. } => {
                for argument in args {
                    self.warn_on_legacy_expression(argument);
                }
            }
            Expression::Handle { expr, arms, .. } => {
                self.warn_on_legacy_expression(expr);
                for arm in arms {
                    self.warn_on_legacy_expression(&arm.body);
                }
            }
            Expression::Sealing { expr, .. } => {
                self.warn_on_legacy_expression(expr);
            }
            Expression::InterpolatedString { parts, .. } => {
                for part in parts {
                    if let StringPart::Interpolation(expr) = part {
                        self.warn_on_legacy_expression(expr);
                    }
                }
            }
            Expression::NamedConstructor { fields, .. }
            | Expression::Spread {
                overrides: fields, ..
            } => {
                for field in fields {
                    if let Some(value) = &field.value {
                        self.warn_on_legacy_expression(value);
                    }
                }
                if let Expression::Spread { base, .. } = expression {
                    self.warn_on_legacy_expression(base);
                }
            }
            Expression::Identifier { .. }
            | Expression::Integer { .. }
            | Expression::Float { .. }
            | Expression::String { .. }
            | Expression::Boolean { .. }
            | Expression::EmptyList { .. }
            | Expression::None { .. } => {}
        }
    }

    fn legacy_helper_surface_policy(&self) -> LegacyHelperSurfacePolicy {
        let normalized = self.file_path.replace('\\', "/");
        let path = normalized.as_str();

        if path.contains("lib/Flow/")
            || path.starts_with("examples/primop/")
            || path.starts_with("examples/runtime_errors/primop_")
            || path == "tests/flux/primops_all.flx"
            || path.starts_with("tests/parity/primop_")
        {
            return LegacyHelperSurfacePolicy::Allow;
        }

        if (path.starts_with("examples/") && path != "examples/test.flx")
            || (path.starts_with("tests/flux/") && path != "tests/flux/primops_all.flx")
        {
            return LegacyHelperSurfacePolicy::Error;
        }

        LegacyHelperSurfacePolicy::Warn
    }
}
