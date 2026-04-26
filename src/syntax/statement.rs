use std::fmt;

use crate::{
    core::CorePrimOp,
    diagnostics::position::{Position, Span},
    syntax::{
        Identifier,
        block::Block,
        data_variant::DataVariant,
        effect_expr::EffectExpr,
        effect_ops::EffectOp,
        expression::{Expression, Pattern},
        interner::Interner,
        type_class::{ClassConstraint, ClassMethod, InstanceMethod},
        type_expr::TypeExpr,
    },
};

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionTypeParam {
    pub name: Identifier,
    pub constraints: Vec<Identifier>,
}

/// Specifies which members an `import` statement exposes unqualified.
///
/// - `None` — default: members require qualified access (`Module.member`).
/// - `All` — `exposing (..)`: all public members are unqualified.
/// - `Names(vec)` — `exposing (a, b)`: only listed members are unqualified.
#[derive(Debug, Clone, PartialEq)]
pub enum ImportExposing {
    /// No unqualified exposure (default).
    None,
    /// `exposing (..)` — all public members.
    All,
    /// `exposing (name, name, ...)` — selective.
    Names(Vec<Identifier>),
}

/// FBIP annotation on a function (Perceus Section 2.6).
///
/// - `@fip` — the function performs zero heap allocations on the unique path
///   (every constructor is reused in-place).
/// - `@fbip` — the function performs a finite (bounded) number of allocations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FipAnnotation {
    Fip,
    Fbip,
}

#[derive(Debug, Clone)]
pub enum Statement {
    Let {
        is_public: bool,
        name: Identifier,
        type_annotation: Option<TypeExpr>,
        value: Expression,
        span: Span,
    },
    LetDestructure {
        is_public: bool,
        pattern: Pattern,
        value: Expression,
        span: Span,
    },
    Return {
        value: Option<Expression>,
        span: Span,
    },
    Expression {
        expression: Expression,
        has_semicolon: bool,
        span: Span,
    },
    Function {
        is_public: bool,
        /// FBIP annotation: `@fip` or `@fbip` before `fn`.
        fip: Option<FipAnnotation>,
        /// Original `intrinsic fn ... = primop ...` binding, if this function
        /// came from an intrinsic declaration and was desugared to a normal
        /// function body.
        intrinsic: Option<CorePrimOp>,
        name: Identifier,
        /// Explicit generic type parameters, e.g. `[T, U]` for `fn f<T, U>(...)`.
        /// Empty for non-generic functions.
        type_params: Vec<FunctionTypeParam>,
        parameters: Vec<Identifier>,
        parameter_types: Vec<Option<TypeExpr>>,
        return_type: Option<TypeExpr>,
        effects: Vec<EffectExpr>,
        body: Block,
        span: Span,
    },
    Assign {
        name: Identifier,
        value: Expression,
        span: Span,
    },
    Module {
        name: Identifier,
        body: Block,
        span: Span,
    },
    Import {
        name: Identifier,
        alias: Option<Identifier>,
        except: Vec<Identifier>,
        exposing: ImportExposing,
        span: Span,
    },
    Data {
        /// Proposal 0151, Phase 2: visibility of this data declaration.
        ///
        /// `true` for `public data Foo`, `false` for unmarked / private.
        /// Used by the visibility walker to enforce that no `public class`
        /// signature names a private type (E451) and that no `public
        /// instance` of a public class has a private head ADT (E455).
        is_public: bool,
        name: Identifier,
        type_params: Vec<Identifier>,
        variants: Vec<DataVariant>,
        /// Classes to auto-derive: `data Foo { ... } deriving (Eq, Show)`
        deriving: Vec<Identifier>,
        span: Span,
    },
    /// effect Name { op: Params -> Ret, ... } - declares a user defined effect.
    /// Operation signatures are enforced by compiler static checks.
    EffectDecl {
        name: Identifier,
        ops: Vec<EffectOp>,
        span: Span,
    },
    /// `alias Name = <E1 | E2 | ...>` — declares an effect-row alias.
    ///
    /// Proposal 0161 Phase 1 (B1). At type-inference time, any occurrence of
    /// `Name` in an effect expression expands to `expansion`. Aliases are
    /// non-recursive in this first pass: an alias body may not reference
    /// another alias.
    EffectAlias {
        name: Identifier,
        expansion: EffectExpr,
        span: Span,
    },
    /// Type class declaration: class Eq<a> => Ord<a> { methods... }
    ///
    /// Proposal 0151: `is_public` controls whether the class name and its
    /// methods are exported through the owning module's surface. For top-level
    /// (non-module-scoped) declarations and during Phase 1a, this field is
    /// always `false`; visibility enforcement begins in Phase 2.
    Class {
        is_public: bool,
        name: Identifier,
        type_params: Vec<Identifier>,
        superclasses: Vec<ClassConstraint>,
        methods: Vec<ClassMethod>,
        span: Span,
    },
    /// Instance declaration: instance Eq<a> => Eq<List<a>> { methods... }
    ///
    /// Proposal 0151: `is_public` controls whether other modules can resolve
    /// against this instance via type-directed lookup. Private instances are
    /// only visible inside their defining module. During Phase 1a this field
    /// is always `false`; visibility enforcement begins in Phase 2.
    Instance {
        is_public: bool,
        class_name: Identifier,
        type_args: Vec<TypeExpr>,
        context: Vec<ClassConstraint>,
        methods: Vec<InstanceMethod>,
        span: Span,
    },
}

impl Statement {
    pub fn function_type_param_names(type_params: &[FunctionTypeParam]) -> Vec<Identifier> {
        type_params.iter().map(|tp| tp.name).collect()
    }

    fn format_function_type_params(
        type_params: &[FunctionTypeParam],
        render_ident: impl Fn(Identifier) -> String,
    ) -> String {
        if type_params.is_empty() {
            return String::new();
        }
        let rendered: Vec<String> = type_params
            .iter()
            .map(|tp| {
                if tp.constraints.is_empty() {
                    render_ident(tp.name)
                } else {
                    let constraints: Vec<String> =
                        tp.constraints.iter().map(|c| render_ident(*c)).collect();
                    format!("{}: {}", render_ident(tp.name), constraints.join(" + "))
                }
            })
            .collect();
        format!("<{}>", rendered.join(", "))
    }

    pub fn position(&self) -> Position {
        match self {
            Statement::Let { span, .. } => span.start,
            Statement::LetDestructure { span, .. } => span.start,
            Statement::Return { span, .. } => span.start,
            Statement::Expression { span, .. } => span.start,
            Statement::Function { span, .. } => span.start,
            Statement::Assign { span, .. } => span.start,
            Statement::Module { span, .. } => span.start,
            Statement::Import { span, .. } => span.start,
            Statement::Data { span, .. } => span.start,
            Statement::EffectDecl { span, .. } => span.start,
            Statement::EffectAlias { span, .. } => span.start,
            Statement::Class { span, .. } => span.start,
            Statement::Instance { span, .. } => span.start,
        }
    }

    pub fn span(&self) -> Span {
        match self {
            Statement::Let { span, .. } => *span,
            Statement::LetDestructure { span, .. } => *span,
            Statement::Return { span, .. } => *span,
            Statement::Expression { span, .. } => *span,
            Statement::Function { span, .. } => *span,
            Statement::Assign { span, .. } => *span,
            Statement::Module { span, .. } => *span,
            Statement::Import { span, .. } => *span,
            Statement::Data { span, .. } => *span,
            Statement::EffectDecl { span, .. } => *span,
            Statement::EffectAlias { span, .. } => *span,
            Statement::Class { span, .. } => *span,
            Statement::Instance { span, .. } => *span,
        }
    }
}

impl fmt::Display for Statement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Statement::Let {
                is_public,
                name,
                type_annotation,
                value,
                ..
            } => {
                let prefix = if *is_public { "public " } else { "" };
                if let Some(ta) = type_annotation {
                    write!(f, "{}let {}: {} = {};", prefix, name, ta, value)
                } else {
                    write!(f, "{}let {} = {};", prefix, name, value)
                }
            }
            Statement::LetDestructure {
                is_public,
                pattern,
                value,
                ..
            } => {
                let prefix = if *is_public { "public " } else { "" };
                write!(f, "{}let {} = {};", prefix, pattern, value)
            }
            Statement::Return { value: Some(v), .. } => {
                write!(f, "return {};", v)
            }
            Statement::Return { value: None, .. } => {
                write!(f, "return;")
            }
            Statement::Expression {
                expression,
                has_semicolon,
                ..
            } => {
                if *has_semicolon {
                    write!(f, "{};", expression)
                } else {
                    write!(f, "{}", expression)
                }
            }
            Statement::Function {
                is_public,
                intrinsic,
                name,
                type_params,
                parameters,
                parameter_types,
                return_type,
                effects,
                body,
                ..
            } => {
                let params: Vec<String> = parameters
                    .iter()
                    .enumerate()
                    .map(
                        |(idx, param)| match parameter_types.get(idx).and_then(|ty| ty.as_ref()) {
                            Some(ty) => format!("{param}: {ty}"),
                            None => param.to_string(),
                        },
                    )
                    .collect();
                let fn_kw = match (*is_public, intrinsic.is_some()) {
                    (true, true) => "public intrinsic fn",
                    (false, true) => "intrinsic fn",
                    (true, false) => "public fn",
                    (false, false) => "fn",
                };
                let type_params_text =
                    Self::format_function_type_params(type_params, |id| id.to_string());
                if let Some(primop) = intrinsic {
                    if let Some(return_type) = return_type {
                        if effects.is_empty() {
                            write!(
                                f,
                                "{} {}{}({}) -> {} = primop {:?}",
                                fn_kw,
                                name,
                                type_params_text,
                                params.join(", "),
                                return_type,
                                primop
                            )
                        } else {
                            let effects_text: Vec<String> =
                                effects.iter().map(ToString::to_string).collect();
                            write!(
                                f,
                                "{} {}{}({}) -> {} with {} = primop {:?}",
                                fn_kw,
                                name,
                                type_params_text,
                                params.join(", "),
                                return_type,
                                effects_text.join(", "),
                                primop
                            )
                        }
                    } else if effects.is_empty() {
                        write!(
                            f,
                            "{} {}{}({}) = primop {:?}",
                            fn_kw,
                            name,
                            type_params_text,
                            params.join(", "),
                            primop
                        )
                    } else {
                        let effects_text: Vec<String> =
                            effects.iter().map(ToString::to_string).collect();
                        write!(
                            f,
                            "{} {}{}({}) with {} = primop {:?}",
                            fn_kw,
                            name,
                            type_params_text,
                            params.join(", "),
                            effects_text.join(", "),
                            primop
                        )
                    }
                } else if let Some(return_type) = return_type {
                    if effects.is_empty() {
                        write!(
                            f,
                            "{} {}{}({}) -> {} {}",
                            fn_kw,
                            name,
                            type_params_text,
                            params.join(", "),
                            return_type,
                            body
                        )
                    } else {
                        let effects_text: Vec<String> =
                            effects.iter().map(ToString::to_string).collect();
                        write!(
                            f,
                            "{} {}{}({}) -> {} with {} {}",
                            fn_kw,
                            name,
                            type_params_text,
                            params.join(", "),
                            return_type,
                            effects_text.join(", "),
                            body
                        )
                    }
                } else if effects.is_empty() {
                    write!(
                        f,
                        "{} {}{}({}) {}",
                        fn_kw,
                        name,
                        type_params_text,
                        params.join(", "),
                        body
                    )
                } else {
                    let effects_text: Vec<String> =
                        effects.iter().map(ToString::to_string).collect();
                    write!(
                        f,
                        "{} {}{}({}) with {} {}",
                        fn_kw,
                        name,
                        type_params_text,
                        params.join(", "),
                        effects_text.join(", "),
                        body
                    )
                }
            }
            Statement::Assign { name, value, .. } => {
                write!(f, "{} = {};", name, value)
            }
            Statement::Module { name, body, .. } => {
                write!(f, "module {} {}", name, body)
            }
            Statement::Import {
                name,
                except,
                exposing,
                ..
            } => {
                let mut s = String::from("import ");
                s.push_str(&name.to_string());
                if let Some(alias) = &self.get_import_alias() {
                    s.push_str(&format!(" as {}", alias));
                }
                if !except.is_empty() {
                    let names: Vec<String> = except.iter().map(ToString::to_string).collect();
                    s.push_str(&format!(" except [{}]", names.join(", ")));
                }
                match exposing {
                    ImportExposing::All => s.push_str(" exposing (..)"),
                    ImportExposing::Names(names) => {
                        let names: Vec<String> = names.iter().map(ToString::to_string).collect();
                        s.push_str(&format!(" exposing ({})", names.join(", ")));
                    }
                    ImportExposing::None => {}
                }
                write!(f, "{}", s)
            }
            Statement::Data { name, variants, .. } => {
                write!(f, "data {} {{", name)?;
                for (i, v) in variants.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, " {}", v.name)?;
                    if !v.fields.is_empty() {
                        let fields: Vec<String> =
                            v.fields.iter().map(|t| format!("{}", t)).collect();
                        write!(f, "({})", fields.join(", "))?;
                    }
                }
                write!(f, " }}")
            }
            Statement::EffectDecl { name, ops, .. } => {
                write!(f, "effect {} {{", name)?;
                for op in ops {
                    write!(f, " {}: {},", op.name, op.type_expr)?;
                }
                write!(f, " }}")
            }
            Statement::EffectAlias {
                name, expansion, ..
            } => {
                write!(f, "alias {} = <{}>", name, expansion)
            }
            Statement::Class {
                name,
                type_params,
                methods,
                ..
            } => {
                write!(f, "class {}", name)?;
                if !type_params.is_empty() {
                    write!(
                        f,
                        "<{}>",
                        type_params
                            .iter()
                            .map(|p| p.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )?;
                }
                write!(f, " {{ {} methods }}", methods.len())
            }
            Statement::Instance {
                class_name,
                type_args,
                ..
            } => {
                write!(f, "instance {}", class_name)?;
                if !type_args.is_empty() {
                    write!(
                        f,
                        "<{}>",
                        type_args
                            .iter()
                            .map(|t| t.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )?;
                }
                write!(f, " {{ ... }}")
            }
        }
    }
}

impl Statement {
    fn get_import_alias(&self) -> Option<&Identifier> {
        match self {
            Statement::Import { alias, .. } => alias.as_ref(),
            _ => None,
        }
    }

    /// Formats this statement using the interner to resolve identifier names.
    pub fn display_with(&self, interner: &Interner) -> String {
        match self {
            Statement::Let {
                name,
                type_annotation,
                value,
                ..
            } => {
                if let Some(ta) = type_annotation {
                    format!(
                        "let {}: {} = {};",
                        interner.resolve(*name),
                        ta.display_with(interner),
                        value.display_with(interner)
                    )
                } else {
                    format!(
                        "let {} = {};",
                        interner.resolve(*name),
                        value.display_with(interner)
                    )
                }
            }
            Statement::LetDestructure { pattern, value, .. } => {
                format!(
                    "let {} = {};",
                    pattern.display_with(interner),
                    value.display_with(interner)
                )
            }
            Statement::Return { value: Some(v), .. } => {
                format!("return {};", v.display_with(interner))
            }
            Statement::Return { value: None, .. } => "return;".to_string(),
            Statement::Expression {
                expression,
                has_semicolon,
                ..
            } => {
                if *has_semicolon {
                    format!("{};", expression.display_with(interner))
                } else {
                    expression.display_with(interner)
                }
            }
            Statement::Function {
                is_public,
                intrinsic,
                name,
                type_params,
                parameters,
                parameter_types,
                return_type,
                effects,
                body,
                ..
            } => {
                let params: Vec<String> = parameters
                    .iter()
                    .enumerate()
                    .map(|(idx, param)| {
                        let param_name = interner.resolve(*param);
                        match parameter_types.get(idx).and_then(|ty| ty.as_ref()) {
                            Some(ty) => format!("{param_name}: {}", ty.display_with(interner)),
                            None => param_name.to_string(),
                        }
                    })
                    .collect();
                let fn_kw = match (*is_public, intrinsic.is_some()) {
                    (true, true) => "public intrinsic fn",
                    (false, true) => "intrinsic fn",
                    (true, false) => "public fn",
                    (false, false) => "fn",
                };
                let type_params_text = Self::format_function_type_params(type_params, |id| {
                    interner.resolve(id).to_string()
                });
                if let Some(primop) = intrinsic {
                    if let Some(return_type) = return_type {
                        if effects.is_empty() {
                            format!(
                                "{} {}{}({}) -> {} = primop {:?}",
                                fn_kw,
                                interner.resolve(*name),
                                type_params_text,
                                params.join(", "),
                                return_type.display_with(interner),
                                primop
                            )
                        } else {
                            let effects_text: Vec<String> =
                                effects.iter().map(|e| e.display_with(interner)).collect();
                            format!(
                                "{} {}{}({}) -> {} with {} = primop {:?}",
                                fn_kw,
                                interner.resolve(*name),
                                type_params_text,
                                params.join(", "),
                                return_type.display_with(interner),
                                effects_text.join(", "),
                                primop
                            )
                        }
                    } else if effects.is_empty() {
                        format!(
                            "{} {}{}({}) = primop {:?}",
                            fn_kw,
                            interner.resolve(*name),
                            type_params_text,
                            params.join(", "),
                            primop
                        )
                    } else {
                        let effects_text: Vec<String> =
                            effects.iter().map(|e| e.display_with(interner)).collect();
                        format!(
                            "{} {}{}({}) with {} = primop {:?}",
                            fn_kw,
                            interner.resolve(*name),
                            type_params_text,
                            params.join(", "),
                            effects_text.join(", "),
                            primop
                        )
                    }
                } else if let Some(return_type) = return_type {
                    if effects.is_empty() {
                        format!(
                            "{} {}{}({}) -> {} {}",
                            fn_kw,
                            interner.resolve(*name),
                            type_params_text,
                            params.join(", "),
                            return_type.display_with(interner),
                            body
                        )
                    } else {
                        let effects_text: Vec<String> =
                            effects.iter().map(|e| e.display_with(interner)).collect();
                        format!(
                            "{} {}{}({}) -> {} with {} {}",
                            fn_kw,
                            interner.resolve(*name),
                            type_params_text,
                            params.join(", "),
                            return_type.display_with(interner),
                            effects_text.join(", "),
                            body
                        )
                    }
                } else if effects.is_empty() {
                    format!(
                        "{} {}{}({}) {}",
                        fn_kw,
                        interner.resolve(*name),
                        type_params_text,
                        params.join(", "),
                        body
                    )
                } else {
                    let effects_text: Vec<String> =
                        effects.iter().map(|e| e.display_with(interner)).collect();
                    format!(
                        "{} {}{}({}) with {} {}",
                        fn_kw,
                        interner.resolve(*name),
                        type_params_text,
                        params.join(", "),
                        effects_text.join(", "),
                        body
                    )
                }
            }
            Statement::Assign { name, value, .. } => {
                format!(
                    "{} = {};",
                    interner.resolve(*name),
                    value.display_with(interner)
                )
            }
            Statement::Module { name, body, .. } => {
                format!("module {} {}", interner.resolve(*name), body)
            }
            Statement::Import {
                name,
                alias,
                except,
                exposing,
                ..
            } => {
                let mut text = format!("import {}", interner.resolve(*name));
                if let Some(alias) = alias {
                    text.push_str(&format!(" as {}", interner.resolve(*alias)));
                }
                if !except.is_empty() {
                    let except_names: Vec<&str> =
                        except.iter().map(|n| interner.resolve(*n)).collect();
                    text.push_str(&format!(" except [{}]", except_names.join(", ")));
                }
                match exposing {
                    ImportExposing::All => text.push_str(" exposing (..)"),
                    ImportExposing::Names(names) => {
                        let exposed: Vec<&str> =
                            names.iter().map(|n| interner.resolve(*n)).collect();
                        text.push_str(&format!(" exposing ({})", exposed.join(", ")));
                    }
                    ImportExposing::None => {}
                }
                text
            }
            Statement::Data { name, variants, .. } => {
                let mut text = format!("data {} {{", interner.resolve(*name));
                for (i, v) in variants.iter().enumerate() {
                    if i > 0 {
                        text.push_str(", ");
                    }
                    text.push_str(&format!(" {}", interner.resolve(v.name)));
                    if !v.fields.is_empty() {
                        let fields: Vec<String> =
                            v.fields.iter().map(|t| t.display_with(interner)).collect();
                        text.push_str(&format!("({})", fields.join(", ")));
                    }
                }
                text.push_str(" }");
                text
            }
            Statement::EffectDecl { name, ops, .. } => {
                let mut text = format!("effect {} {{", interner.resolve(*name));
                for op in ops {
                    text.push_str(&format!(
                        " {}: {},",
                        interner.resolve(op.name),
                        op.type_expr.display_with(interner)
                    ));
                }
                text.push_str(" }");
                text
            }
            Statement::EffectAlias {
                name, expansion, ..
            } => {
                format!(
                    "alias {} = <{}>",
                    interner.resolve(*name),
                    expansion.display_with(interner)
                )
            }
            Statement::Class {
                name,
                type_params,
                methods,
                ..
            } => {
                let params: Vec<&str> = type_params.iter().map(|p| interner.resolve(*p)).collect();
                format!(
                    "class {}<{}> {{ {} methods }}",
                    interner.resolve(*name),
                    params.join(", "),
                    methods.len()
                )
            }
            Statement::Instance {
                class_name,
                type_args,
                methods,
                ..
            } => {
                let args: Vec<String> =
                    type_args.iter().map(|t| t.display_with(interner)).collect();
                format!(
                    "instance {}<{}> {{ {} methods }}",
                    interner.resolve(*class_name),
                    args.join(", "),
                    methods.len()
                )
            }
        }
    }
}
