use crate::{
    bytecode::compiler::Compiler,
    diagnostics::{
        Diagnostic, DiagnosticBuilder, ErrorType, compiler_errors::type_unification_error,
    },
    syntax::{block::Block, expression::Expression, statement::Statement},
    types::{
        infer_type::InferType, type_constructor::TypeConstructor, type_env::TypeEnv,
        type_subst::TypeSubst, unify_error::unify,
    },
};
use std::collections::HashMap;

type CompileResult<T> = Result<T, Box<Diagnostic>>;

#[derive(Debug, Clone)]
pub enum UnresolvedReason {
    UnknownExpressionType,
}

#[derive(Debug, Clone)]
pub enum HmExprTypeResult {
    Known(InferType),
    Unresolved(UnresolvedReason),
}

impl Compiler {
    /// Authoritative HM expression typing for typed validation paths.
    /// This path does not use runtime-boundary compatibility typing.
    pub(super) fn hm_expr_type_strict_path(&self, expression: &Expression) -> HmExprTypeResult {
        match self.infer_hm_expr_type(expression) {
            Some(infer) if Self::is_hm_type_resolved(&infer) => HmExprTypeResult::Known(infer),
            Some(_) | None => HmExprTypeResult::Unresolved(UnresolvedReason::UnknownExpressionType),
        }
    }

    fn infer_hm_expr_type(&self, expression: &Expression) -> Option<InferType> {
        match expression {
            Expression::Integer { .. } => Some(InferType::Con(TypeConstructor::Int)),
            Expression::Float { .. } => Some(InferType::Con(TypeConstructor::Float)),
            Expression::Boolean { .. } => Some(InferType::Con(TypeConstructor::Bool)),
            Expression::String { .. } | Expression::InterpolatedString { .. } => {
                Some(InferType::Con(TypeConstructor::String))
            }
            Expression::None { .. } => Some(InferType::Con(TypeConstructor::Unit)),
            Expression::Identifier { name, .. } => {
                if let Some(scheme) = self.type_env.lookup(*name) {
                    let mut counter = self.type_env.counter;
                    let (instantiated, _) = scheme.instantiate(&mut counter);
                    return Some(instantiated);
                }
                self.lookup_static_type(*name)
                    .map(|rt| TypeEnv::infer_type_from_runtime(&rt))
            }
            Expression::Call {
                function,
                arguments,
                ..
            } => self.infer_hm_call_type(function, arguments),
            Expression::If {
                consequence,
                alternative,
                ..
            } => {
                let left = self.infer_hm_block_tail_type(consequence)?;
                let right = self.infer_hm_block_tail_type(alternative.as_ref()?)?;
                Self::unify_to_join_type(left, right)
            }
            Expression::DoBlock { block, .. } => self.infer_hm_block_tail_type(block),
            Expression::Match { arms, .. } => {
                let mut iter = arms.iter();
                let first = self.infer_hm_expr_type(&iter.next()?.body)?;
                iter.try_fold(first, |acc, arm| {
                    let arm_ty = self.infer_hm_expr_type(&arm.body)?;
                    Self::unify_to_join_type(acc, arm_ty)
                })
            }
            Expression::TupleLiteral { elements, .. } => Some(InferType::Tuple(
                elements
                    .iter()
                    .map(|e| self.infer_hm_expr_type(e))
                    .collect::<Option<Vec<_>>>()?,
            )),
            Expression::ListLiteral { elements, .. }
            | Expression::ArrayLiteral { elements, .. } => self.infer_hm_sequence_type(
                elements,
                matches!(expression, Expression::ArrayLiteral { .. }),
            ),
            Expression::Hash { pairs, .. } => self.infer_hm_map_type(pairs),
            Expression::Prefix {
                operator, right, ..
            } => {
                let right_ty = self.infer_hm_expr_type(right)?;
                match operator.as_str() {
                    "-" => {
                        let is_num = matches!(
                            right_ty,
                            InferType::Con(TypeConstructor::Int | TypeConstructor::Float)
                        );
                        is_num.then_some(right_ty)
                    }
                    "!" => Some(InferType::Con(TypeConstructor::Bool)),
                    _ => None,
                }
            }
            Expression::Infix {
                left,
                operator,
                right,
                ..
            } => self.infer_hm_infix_type(left, operator, right),
            Expression::Index { left, .. } => {
                let left_ty = self.infer_hm_expr_type(left)?;
                match left_ty {
                    InferType::App(TypeConstructor::Array, args)
                    | InferType::App(TypeConstructor::List, args)
                    | InferType::App(TypeConstructor::Option, args)
                        if args.len() == 1 =>
                    {
                        Some(InferType::App(
                            TypeConstructor::Option,
                            vec![args[0].clone()],
                        ))
                    }
                    InferType::Tuple(elements) => {
                        let joined = Self::join_infer_types(&elements)?;
                        Some(InferType::App(TypeConstructor::Option, vec![joined]))
                    }
                    InferType::App(TypeConstructor::Map, args) if args.len() == 2 => Some(
                        InferType::App(TypeConstructor::Option, vec![args[1].clone()]),
                    ),
                    _ => None,
                }
            }
            Expression::TupleFieldAccess { object, index, .. } => {
                let InferType::Tuple(elements) = self.infer_hm_expr_type(object)? else {
                    return None;
                };
                elements.get(*index).cloned()
            }
            Expression::Some { value, .. } => {
                let inner = self.infer_hm_expr_type(value)?;
                Some(InferType::App(TypeConstructor::Option, vec![inner]))
            }
            Expression::EmptyList { .. } => Some(InferType::App(
                TypeConstructor::List,
                vec![InferType::Con(TypeConstructor::Any)],
            )),
            Expression::Cons { head, tail, .. } => {
                let head_ty = self.infer_hm_expr_type(head)?;
                let InferType::App(TypeConstructor::List, args) = self.infer_hm_expr_type(tail)?
                else {
                    return None;
                };
                if args.len() != 1 {
                    return None;
                }
                let elem_ty = args[0].clone();
                if Self::contains_any(&elem_ty) || unify(&elem_ty, &head_ty).is_ok() {
                    Some(InferType::App(TypeConstructor::List, vec![head_ty]))
                } else {
                    None
                }
            }
            Expression::Left { value, .. } => {
                let left = self.infer_hm_expr_type(value)?;
                Some(InferType::App(
                    TypeConstructor::Either,
                    vec![left, InferType::Con(TypeConstructor::Any)],
                ))
            }
            Expression::Right { value, .. } => {
                let right = self.infer_hm_expr_type(value)?;
                Some(InferType::App(
                    TypeConstructor::Either,
                    vec![InferType::Con(TypeConstructor::Any), right],
                ))
            }
            _ => None,
        }
    }

    fn infer_hm_call_type(
        &self,
        function: &Expression,
        arguments: &[Expression],
    ) -> Option<InferType> {
        if let Some(contract) = self.resolve_call_contract(function, arguments.len()) {
            let mut counter = self.type_env.counter;
            let type_param_map = contract
                .type_params
                .iter()
                .map(|param| {
                    let v = counter;
                    counter += 1;
                    (*param, v)
                })
                .collect::<HashMap<_, _>>();
            let mut subst = TypeSubst::empty();
            for (idx, argument) in arguments.iter().enumerate() {
                let param_expr = contract.params.get(idx)?.as_ref()?;
                let expected = TypeEnv::infer_type_from_type_expr(
                    param_expr,
                    &type_param_map,
                    &self.interner,
                )?;
                let actual = self.infer_hm_expr_type(argument)?;
                let expected = expected.apply_type_subst(&subst);
                let actual = actual.apply_type_subst(&subst);
                let next = unify(&expected, &actual).ok()?;
                subst = subst.compose(&next);
            }
            let ret_expr = contract.ret.as_ref()?;
            let ret =
                TypeEnv::infer_type_from_type_expr(ret_expr, &type_param_map, &self.interner)?;
            return Some(ret.apply_type_subst(&subst));
        }

        let fn_ty = self.infer_hm_expr_type(function)?;
        let InferType::Fun(params, ret, _) = fn_ty else {
            return None;
        };
        if params.len() != arguments.len() {
            return None;
        }

        let mut subst = TypeSubst::empty();
        for (param, argument) in params.iter().zip(arguments.iter()) {
            let actual = self.infer_hm_expr_type(argument)?;
            let expected = param.apply_type_subst(&subst);
            let actual = actual.apply_type_subst(&subst);
            let next = unify(&expected, &actual).ok()?;
            subst = subst.compose(&next);
        }
        Some(ret.apply_type_subst(&subst))
    }

    fn infer_hm_infix_type(
        &self,
        left: &Expression,
        operator: &str,
        right: &Expression,
    ) -> Option<InferType> {
        let left_ty = self.infer_hm_expr_type(left)?;
        let right_ty = self.infer_hm_expr_type(right)?;
        match operator {
            "+" => {
                if unify(&left_ty, &right_ty).is_err() {
                    return None;
                }
                match left_ty {
                    InferType::Con(TypeConstructor::Int)
                    | InferType::Con(TypeConstructor::Float)
                    | InferType::Con(TypeConstructor::String) => Some(left_ty),
                    _ => None,
                }
            }
            "-" | "*" | "/" | "%" => {
                if unify(&left_ty, &right_ty).is_err() {
                    return None;
                }
                match left_ty {
                    InferType::Con(TypeConstructor::Int)
                    | InferType::Con(TypeConstructor::Float) => Some(left_ty),
                    _ => None,
                }
            }
            "&&" | "||" => {
                if left_ty == InferType::Con(TypeConstructor::Bool)
                    && right_ty == InferType::Con(TypeConstructor::Bool)
                {
                    Some(InferType::Con(TypeConstructor::Bool))
                } else {
                    None
                }
            }
            "==" | "!=" | ">" | ">=" | "<" | "<=" => Some(InferType::Con(TypeConstructor::Bool)),
            _ => None,
        }
    }

    fn infer_hm_block_tail_type(&self, block: &Block) -> Option<InferType> {
        let Statement::Expression {
            expression,
            has_semicolon: false,
            ..
        } = block.statements.last()?
        else {
            return None;
        };
        self.infer_hm_expr_type(expression)
    }

    fn infer_hm_sequence_type(&self, elements: &[Expression], as_array: bool) -> Option<InferType> {
        let ctor = if as_array {
            TypeConstructor::Array
        } else {
            TypeConstructor::List
        };
        if elements.is_empty() {
            return Some(InferType::App(
                ctor,
                vec![InferType::Con(TypeConstructor::Any)],
            ));
        }
        let joined = Self::join_infer_types(
            &elements
                .iter()
                .map(|e| self.infer_hm_expr_type(e))
                .collect::<Option<Vec<_>>>()?,
        )?;
        Some(InferType::App(ctor, vec![joined]))
    }

    fn infer_hm_map_type(&self, pairs: &[(Expression, Expression)]) -> Option<InferType> {
        if pairs.is_empty() {
            return Some(InferType::App(
                TypeConstructor::Map,
                vec![
                    InferType::Con(TypeConstructor::Any),
                    InferType::Con(TypeConstructor::Any),
                ],
            ));
        }
        let mut keys = Vec::with_capacity(pairs.len());
        let mut vals = Vec::with_capacity(pairs.len());
        for (k, v) in pairs {
            keys.push(self.infer_hm_expr_type(k)?);
            vals.push(self.infer_hm_expr_type(v)?);
        }
        Some(InferType::App(
            TypeConstructor::Map,
            vec![
                Self::join_infer_types(&keys)?,
                Self::join_infer_types(&vals)?,
            ],
        ))
    }

    fn join_infer_types(types: &[InferType]) -> Option<InferType> {
        let first = types.first()?.clone();
        types
            .iter()
            .skip(1)
            .try_fold(first, |acc, ty| Self::unify_to_join_type(acc, ty.clone()))
    }

    fn unify_to_join_type(left: InferType, right: InferType) -> Option<InferType> {
        let subst = unify(&left, &right).ok()?;
        Some(left.apply_type_subst(&subst))
    }

    fn contains_any(infer: &InferType) -> bool {
        match infer {
            InferType::Con(TypeConstructor::Any) => true,
            InferType::App(_, args) | InferType::Tuple(args) => args.iter().any(Self::contains_any),
            InferType::Fun(params, ret, _) => {
                params.iter().any(Self::contains_any) || Self::contains_any(ret)
            }
            InferType::Var(_) => false,
            InferType::Con(_) => false,
        }
    }

    fn is_hm_type_resolved(infer: &InferType) -> bool {
        infer.free_vars().is_empty() && !Self::contains_any(infer)
    }

    fn format_infer_type(infer: &InferType) -> String {
        TypeEnv::to_runtime(infer, &TypeSubst::empty()).type_name()
    }

    fn format_unify_pair(expected: &InferType, actual: &InferType) -> (String, String) {
        let expected_str = Self::format_infer_type(expected);
        let actual_str = Self::format_infer_type(actual);
        (expected_str, actual_str)
    }

    fn resolved_unify_types(
        expected: &InferType,
        actual: &InferType,
    ) -> Option<(InferType, InferType)> {
        let subst = unify(expected, actual).ok()?;
        Some((
            expected.apply_type_subst(&subst),
            actual.apply_type_subst(&subst),
        ))
    }

    fn ensure_unify(expected: &InferType, actual: &InferType) -> Result<(), (String, String)> {
        if let Some((resolved_expected, resolved_actual)) =
            Self::resolved_unify_types(expected, actual)
        {
            if resolved_expected == resolved_actual {
                return Ok(());
            }
        }
        let (expected_str, actual_str) = Self::format_unify_pair(expected, actual);
        Err((expected_str, actual_str))
    }

    fn build_type_mismatch(
        &self,
        expression: &Expression,
        primary_label: &str,
        help: String,
        expected: &str,
        actual: &str,
    ) -> Diagnostic {
        type_unification_error(self.file_path.clone(), expression.span(), expected, actual)
            .with_secondary_label(expression.span(), primary_label)
            .with_help(help)
    }

    pub(super) fn unresolved_boundary_error(
        &self,
        expression: &Expression,
        unresolved_context: &str,
    ) -> Diagnostic {
        Diagnostic::make_error_dynamic(
            "E425",
            "STRICT UNRESOLVED BOUNDARY TYPE",
            ErrorType::Compiler,
            format!(
                "Strict mode cannot enforce runtime boundary check for unresolved expression type in {}.",
                unresolved_context
            ),
            Some(
                "Use concrete types (or additional annotations) so HM can resolve this expression."
                    .to_string(),
            ),
            self.file_path.clone(),
            expression.span(),
        )
        .with_primary_label(
            expression.span(),
            "expression type is unresolved in strict mode",
        )
    }

    fn maybe_unresolved(
        &self,
        expression: &Expression,
        unresolved_context: &str,
        strict_unresolved_error: bool,
    ) -> CompileResult<()> {
        if !strict_unresolved_error || !self.strict_mode {
            return Ok(());
        }
        Err(Box::new(
            self.unresolved_boundary_error(expression, unresolved_context),
        ))
    }

    pub(super) fn validate_expr_expected_type(
        &self,
        expected: &InferType,
        expression: &Expression,
        primary_label: &str,
        help: String,
        unresolved_context: &str,
    ) -> CompileResult<()> {
        self.validate_expr_expected_type_with_policy(
            expected,
            expression,
            primary_label,
            help,
            unresolved_context,
            false,
        )
    }

    pub(super) fn validate_expr_expected_type_with_policy(
        &self,
        expected: &InferType,
        expression: &Expression,
        primary_label: &str,
        help: String,
        unresolved_context: &str,
        strict_unresolved_error: bool,
    ) -> CompileResult<()> {
        match self.hm_expr_type_strict_path(expression) {
            HmExprTypeResult::Known(actual) => {
                if Self::ensure_unify(expected, &actual).is_ok() {
                    return Ok(());
                }

                let (expected_str, actual_str) = Self::format_unify_pair(expected, &actual);
                Err(Box::new(self.build_type_mismatch(
                    expression,
                    primary_label,
                    help,
                    &expected_str,
                    &actual_str,
                )))
            }
            HmExprTypeResult::Unresolved(_) => {
                self.maybe_unresolved(expression, unresolved_context, strict_unresolved_error)
            }
        }
    }
}
