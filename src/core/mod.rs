//! Flux Core IR — the canonical semantic intermediate representation.
//!
//! `crate::core` is the stable architectural name for Flux's semantic IR layer.
//!
//! Long-term architecture:
//! - `crate::core` is the only semantic IR boundary used by production code.
//! - backends lower from Core, not directly from AST.
//! - backend IR remains a distinct lower layer below Core.

use crate::{
    diagnostics::position::Span,
    syntax::{
        Identifier, data_variant::DataVariant, effect_expr::EffectExpr, effect_ops::EffectOp,
        type_expr::TypeExpr,
    },
};

pub mod display;
pub mod lower_ast;
pub mod passes;
pub mod to_ir;

// ── Binder identity ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CoreBinderId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CoreBinder {
    /// Stable semantic identity used by Core passes and lowering.
    pub id: CoreBinderId,
    /// Source/debug name retained as metadata.
    pub name: Identifier,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CoreVarRef {
    /// Source/debug name retained for dumps and external-name lowering.
    pub name: Identifier,
    /// `None` means the reference is intentionally external/non-lexical.
    pub binder: Option<CoreBinderId>,
}

impl CoreBinder {
    pub fn new(id: CoreBinderId, name: Identifier) -> Self {
        Self { id, name }
    }
}

impl CoreVarRef {
    pub fn resolved(binder: CoreBinder) -> Self {
        Self {
            name: binder.name,
            binder: Some(binder.id),
        }
    }

    pub fn unresolved(name: Identifier) -> Self {
        Self { name, binder: None }
    }
}

// ── Literals ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum CoreLit {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Unit,
}

// ── Constructor tags ──────────────────────────────────────────────────────────

/// Tag for a constructor in a `Con` node or `CorePat::Con`.
///
/// Built-in constructors (None, Some, Left, Right, Nil, Cons) are
/// represented explicitly to avoid needing an interner at this layer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CoreTag {
    /// User-defined ADT constructor (e.g. `Ok`, `Err`, `Node`)
    Named(Identifier),
    None,
    Some,
    Left,
    Right,
    /// Empty list `[]`
    Nil,
    /// List cons cell
    Cons,
}

// ── Primitive operations ──────────────────────────────────────────────────────

/// All operators and built-in operations lower to `PrimOp`.
///
/// Generic variants (`Add`, `Mul`, …) are used when the operand type is not
/// known at Core IR construction time (e.g. polymorphic or unresolved).
/// Typed variants (`IAdd`, `FMul`, …) are emitted by `lower_ast` when the
/// HM-inferred result type is concretely `Int` or `Float`, enabling backends
/// to skip the runtime type-dispatch path entirely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorePrimOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    IAdd,
    ISub,
    IMul,
    IDiv,
    IMod,
    FAdd,
    FSub,
    FMul,
    FDiv,
    Neg,
    Not,
    Eq,
    NEq,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Concat,
    Interpolate,
    MakeList,
    MakeArray,
    MakeTuple,
    MakeHash,
    Index,
    MemberAccess(Identifier),
    TupleField(usize),
}

// ── Case alternatives ─────────────────────────────────────────────────────────

/// A single `case` alternative: pattern + optional guard + body.
#[derive(Debug, Clone)]
pub struct CoreAlt {
    pub pat: CorePat,
    /// Optional guard expression (boolean). Guards can be eliminated
    /// by a desugaring pass that nests `Case` expressions.
    pub guard: Option<CoreExpr>,
    pub rhs: CoreExpr,
    pub span: Span,
}

/// Patterns that appear in `Case` alternatives.
///
/// Complex surface patterns (nested, guards, wildcards mixed with
/// constructors) are kept here and can be further simplified by a
/// pattern-compilation pass into a decision tree.
#[derive(Debug, Clone)]
pub enum CorePat {
    Wildcard,
    Lit(CoreLit),
    Var(CoreBinder),
    Con { tag: CoreTag, fields: Vec<CorePat> },
    Tuple(Vec<CorePat>),
    EmptyList,
}

// ── Effect handlers ───────────────────────────────────────────────────────────

/// One arm of a `Handle` expression — handles a single effect operation.
#[derive(Debug, Clone)]
pub struct CoreHandler {
    pub operation: Identifier,
    pub params: Vec<CoreBinder>,
    pub resume: CoreBinder,
    pub body: CoreExpr,
    pub span: Span,
}

// ── Core expression ───────────────────────────────────────────────────────────

/// The Core IR expression — the central type of this module.
///
/// ~12 variants replace the surface AST's many expression forms by eliminating
/// all syntactic sugar into these primitives.
#[derive(Debug, Clone)]
pub enum CoreExpr {
    Var {
        var: CoreVarRef,
        span: Span,
    },
    Lit(CoreLit, Span),
    Lam {
        params: Vec<CoreBinder>,
        body: Box<CoreExpr>,
        span: Span,
    },
    App {
        func: Box<CoreExpr>,
        args: Vec<CoreExpr>,
        span: Span,
    },
    Let {
        var: CoreBinder,
        rhs: Box<CoreExpr>,
        body: Box<CoreExpr>,
        span: Span,
    },
    LetRec {
        var: CoreBinder,
        rhs: Box<CoreExpr>,
        body: Box<CoreExpr>,
        span: Span,
    },
    Case {
        scrutinee: Box<CoreExpr>,
        alts: Vec<CoreAlt>,
        span: Span,
    },
    Con {
        tag: CoreTag,
        fields: Vec<CoreExpr>,
        span: Span,
    },
    PrimOp {
        op: CorePrimOp,
        args: Vec<CoreExpr>,
        span: Span,
    },
    Return {
        value: Box<CoreExpr>,
        span: Span,
    },
    Perform {
        effect: Identifier,
        operation: Identifier,
        args: Vec<CoreExpr>,
        span: Span,
    },
    Handle {
        body: Box<CoreExpr>,
        effect: Identifier,
        handlers: Vec<CoreHandler>,
        span: Span,
    },
}

// ── Top-level definitions ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CoreDef {
    pub name: Identifier,
    pub binder: CoreBinder,
    pub expr: CoreExpr,
    pub is_anonymous: bool,
    pub is_recursive: bool,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum CoreTopLevelItem {
    Function {
        is_public: bool,
        name: Identifier,
        type_params: Vec<Identifier>,
        parameters: Vec<Identifier>,
        parameter_types: Vec<Option<TypeExpr>>,
        return_type: Option<TypeExpr>,
        effects: Vec<EffectExpr>,
        span: Span,
    },
    Module {
        name: Identifier,
        body: Vec<CoreTopLevelItem>,
        span: Span,
    },
    Import {
        name: Identifier,
        alias: Option<Identifier>,
        except: Vec<Identifier>,
        span: Span,
    },
    Data {
        name: Identifier,
        type_params: Vec<Identifier>,
        variants: Vec<DataVariant>,
        span: Span,
    },
    EffectDecl {
        name: Identifier,
        ops: Vec<EffectOp>,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub struct CoreProgram {
    pub defs: Vec<CoreDef>,
    pub top_level_items: Vec<CoreTopLevelItem>,
}

// ── CoreExpr helpers ──────────────────────────────────────────────────────────

impl CoreExpr {
    pub fn bound_var(binder: CoreBinder, span: Span) -> CoreExpr {
        CoreExpr::Var {
            var: CoreVarRef::resolved(binder),
            span,
        }
    }

    pub fn external_var(name: Identifier, span: Span) -> CoreExpr {
        CoreExpr::Var {
            var: CoreVarRef::unresolved(name),
            span,
        }
    }

    pub fn span(&self) -> Span {
        match self {
            CoreExpr::Var { span, .. } | CoreExpr::Lit(_, span) => *span,
            CoreExpr::Lam { span, .. }
            | CoreExpr::App { span, .. }
            | CoreExpr::Let { span, .. }
            | CoreExpr::LetRec { span, .. }
            | CoreExpr::Case { span, .. }
            | CoreExpr::Con { span, .. }
            | CoreExpr::PrimOp { span, .. }
            | CoreExpr::Return { span, .. }
            | CoreExpr::Perform { span, .. }
            | CoreExpr::Handle { span, .. } => *span,
        }
    }

    pub fn apply(func: CoreExpr, args: Vec<CoreExpr>, span: Span) -> CoreExpr {
        if args.is_empty() {
            func
        } else {
            CoreExpr::App {
                func: Box::new(func),
                args,
                span,
            }
        }
    }

    pub fn lambda(params: Vec<CoreBinder>, body: CoreExpr, span: Span) -> CoreExpr {
        if params.is_empty() {
            body
        } else {
            CoreExpr::Lam {
                params,
                body: Box::new(body),
                span,
            }
        }
    }

    pub fn let_seq(bindings: Vec<(CoreBinder, CoreExpr)>, body: CoreExpr, span: Span) -> CoreExpr {
        bindings
            .into_iter()
            .rev()
            .fold(body, |b, (var, rhs)| CoreExpr::Let {
                var,
                rhs: Box::new(rhs),
                body: Box::new(b),
                span,
            })
    }
}

impl CoreDef {
    pub fn new(binder: CoreBinder, expr: CoreExpr, is_recursive: bool, span: Span) -> Self {
        Self::new_with_flags(binder, expr, false, is_recursive, span)
    }

    pub fn new_anonymous(
        binder: CoreBinder,
        expr: CoreExpr,
        is_recursive: bool,
        span: Span,
    ) -> Self {
        Self::new_with_flags(binder, expr, true, is_recursive, span)
    }

    pub fn new_with_flags(
        binder: CoreBinder,
        expr: CoreExpr,
        is_anonymous: bool,
        is_recursive: bool,
        span: Span,
    ) -> Self {
        Self {
            name: binder.name,
            binder,
            expr,
            is_anonymous,
            is_recursive,
            span,
        }
    }

    pub fn is_anonymous(&self) -> bool {
        self.is_anonymous
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::syntax::{lexer::Lexer, parser::Parser};

    #[test]
    fn core_facade_lowers_typed_program() {
        let lexer = Lexer::new(
            r#"
fn inc(x) { x + 1 }
fn main() { inc(41) }
"#,
        );
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );

        let core = super::lower_ast::lower_program_ast(&program, &HashMap::new());
        assert_eq!(core.defs.len(), 2);
        assert_eq!(parser.take_interner().resolve(core.defs[0].name), "inc");
    }

    #[test]
    fn core_facade_runs_core_passes_and_backend_lowering() {
        let lexer = Lexer::new("fn main() { 40 + 2 }");
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "parser errors: {:?}",
            parser.errors
        );

        let mut core = super::lower_ast::lower_program_ast(&program, &HashMap::new());
        super::passes::run_core_passes(&mut core);
        let ir = super::to_ir::lower_core_to_ir(&core);

        assert!(!ir.functions.is_empty(), "expected backend IR functions");
    }
}
