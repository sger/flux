use std::fmt;

use crate::{
    diagnostics::position::{Position, Span},
    syntax::{
        Identifier,
        block::Block,
        effect_expr::EffectExpr,
        expression::{Expression, Pattern},
        interner::Interner,
        type_expr::TypeExpr,
    },
};

#[derive(Debug, Clone)]
pub enum Statement {
    Let {
        name: Identifier,
        type_annotation: Option<TypeExpr>,
        value: Expression,
        span: Span,
    },
    LetDestructure {
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
        name: Identifier,
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
        span: Span,
    },
}

impl Statement {
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
        }
    }
}

impl fmt::Display for Statement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Statement::Let {
                name,
                type_annotation,
                value,
                ..
            } => {
                if let Some(ta) = type_annotation {
                    write!(f, "let {}: {} = {};", name, ta, value)
                } else {
                    write!(f, "let {} = {};", name, value)
                }
            }
            Statement::LetDestructure { pattern, value, .. } => {
                write!(f, "let {} = {};", pattern, value)
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
                name,
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
                if let Some(return_type) = return_type {
                    if effects.is_empty() {
                        write!(
                            f,
                            "fn {}({}) -> {} {}",
                            name,
                            params.join(", "),
                            return_type,
                            body
                        )
                    } else {
                        let effects_text: Vec<String> =
                            effects.iter().map(ToString::to_string).collect();
                        write!(
                            f,
                            "fn {}({}) -> {} with {} {}",
                            name,
                            params.join(", "),
                            return_type,
                            effects_text.join(", "),
                            body
                        )
                    }
                } else if effects.is_empty() {
                    write!(f, "fn {}({}) {}", name, params.join(", "), body)
                } else {
                    let effects_text: Vec<String> =
                        effects.iter().map(ToString::to_string).collect();
                    write!(
                        f,
                        "fn {}({}) with {} {}",
                        name,
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
            Statement::Import { name, except, .. } => {
                if let Some(alias) = &self.get_import_alias() {
                    if except.is_empty() {
                        write!(f, "import {} as {}", name, alias)
                    } else {
                        let names: Vec<String> = except.iter().map(ToString::to_string).collect();
                        write!(
                            f,
                            "import {} as {} except [{}]",
                            name,
                            alias,
                            names.join(", ")
                        )
                    }
                } else if except.is_empty() {
                    write!(f, "import {}", name)
                } else {
                    let names: Vec<String> = except.iter().map(ToString::to_string).collect();
                    write!(f, "import {} except [{}]", name, names.join(", "))
                }
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
                name,
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
                if let Some(return_type) = return_type {
                    if effects.is_empty() {
                        format!(
                            "fn {}({}) -> {} {}",
                            interner.resolve(*name),
                            params.join(", "),
                            return_type.display_with(interner),
                            body
                        )
                    } else {
                        let effects_text: Vec<String> =
                            effects.iter().map(|e| e.display_with(interner)).collect();
                        format!(
                            "fn {}({}) -> {} with {} {}",
                            interner.resolve(*name),
                            params.join(", "),
                            return_type.display_with(interner),
                            effects_text.join(", "),
                            body
                        )
                    }
                } else if effects.is_empty() {
                    format!(
                        "fn {}({}) {}",
                        interner.resolve(*name),
                        params.join(", "),
                        body
                    )
                } else {
                    let effects_text: Vec<String> =
                        effects.iter().map(|e| e.display_with(interner)).collect();
                    format!(
                        "fn {}({}) with {} {}",
                        interner.resolve(*name),
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
                ..
            } => {
                if let Some(alias) = alias {
                    let mut text = format!(
                        "import {} as {}",
                        interner.resolve(*name),
                        interner.resolve(*alias)
                    );
                    if !except.is_empty() {
                        let except_names: Vec<&str> =
                            except.iter().map(|n| interner.resolve(*n)).collect();
                        text.push_str(&format!(" except [{}]", except_names.join(", ")));
                    }
                    text
                } else {
                    let mut text = format!("import {}", interner.resolve(*name));
                    if !except.is_empty() {
                        let except_names: Vec<&str> =
                            except.iter().map(|n| interner.resolve(*n)).collect();
                        text.push_str(&format!(" except [{}]", except_names.join(", ")));
                    }
                    text
                }
            }
        }
    }
}
