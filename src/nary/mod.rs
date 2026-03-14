/// Flux Core IR — a small functional intermediate representation.
///
/// All Flux surface constructs lower into these primitives:
///
/// ```text
/// Surface Flux                →  Core IR
/// ─────────────────────────────────────────────────────────────
/// fn f(x, y) { ... }         →  let f = Lam([x, y], ...)
/// f(a, b)                    →  App(f, [a, b])
/// if cond then a else b       →  Case(cond, [True→a, False→b])
/// match p { A(x) → e }       →  Case(p, [Con(A,[x])→e])
/// let x = e; body             →  Let(x, e, body)
/// x + y                      →  PrimOp(Add, [x, y])
/// perform Eff.op(a)           →  Perform(Eff, op, [a])
/// ```
///
/// This is the layer where analysis passes (inlining, beta reduction,
/// case-of-known-constructor, closure conversion) will run.
pub mod display;
pub mod lower_ast;
pub mod passes;
pub mod to_ir;

use crate::{
    diagnostics::position::Span,
    syntax::Identifier,
};

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
    // Generic arithmetic (type unknown / polymorphic)
    Add, Sub, Mul, Div, Mod,
    // Typed integer arithmetic — emitted when result type is Int
    IAdd, ISub, IMul, IDiv, IMod,
    // Typed float arithmetic — emitted when result type is Float
    FAdd, FSub, FMul, FDiv,
    // Unary
    Neg, Not,
    // Comparisons
    Eq, NEq, Lt, Le, Gt, Ge,
    // Logical (short-circuit at expression level in Core)
    And, Or,
    // String
    Concat,
    // Interpolated string: args are the parts in order
    Interpolate,
    // Collection construction
    MakeList,
    MakeArray,
    MakeTuple,
    MakeHash,
    // Access
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
    /// `_` — always succeeds, binds nothing
    Wildcard,
    /// Literal match: `0`, `true`, `"hello"`
    Lit(CoreLit),
    /// Variable binding: always succeeds, binds identifier
    Var(Identifier),
    /// Constructor match with nested field patterns: `Some(x)`, `Node(l, r)`
    Con { tag: CoreTag, fields: Vec<CorePat> },
    /// Tuple destructure: `(x, y, z)`
    Tuple(Vec<CorePat>),
    /// Empty list `[]`
    EmptyList,
}

// ── Effect handlers ───────────────────────────────────────────────────────────

/// One arm of a `Handle` expression — handles a single effect operation.
#[derive(Debug, Clone)]
pub struct CoreHandler {
    pub operation: Identifier,
    pub params: Vec<Identifier>,
    /// The continuation (resume) parameter
    pub resume: Identifier,
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
    /// Variable reference
    Var(Identifier, Span),

    /// Literal constant
    Lit(CoreLit, Span),

    /// N-ary lambda abstraction.
    /// `fn f(x, y) { body }` → `Lam([x, y], body)`
    Lam {
        params: Vec<Identifier>,
        body: Box<CoreExpr>,
        span: Span,
    },

    /// N-ary function application.
    /// `f(a, b)` → `App(f, [a, b])`
    App {
        func: Box<CoreExpr>,
        args: Vec<CoreExpr>,
        span: Span,
    },

    /// Non-recursive let binding.
    /// `let x = rhs; body`
    Let {
        var: Identifier,
        rhs: Box<CoreExpr>,
        body: Box<CoreExpr>,
        span: Span,
    },

    /// Recursive let binding — for self-referential functions.
    /// `let rec f = Lam(...f...); body`
    LetRec {
        var: Identifier,
        rhs: Box<CoreExpr>,
        body: Box<CoreExpr>,
        span: Span,
    },

    /// Case expression — the *only* branching construct in Core IR.
    ///
    /// `if cond then a else b`  →  `Case(cond, [Lit(true)→a, Wildcard→b])`
    /// `match p { arms }`       →  `Case(p, lower_arms(arms))`
    Case {
        scrutinee: Box<CoreExpr>,
        alts: Vec<CoreAlt>,
        span: Span,
    },

    /// Constructor application (ADT / built-in).
    /// `Some(x)` → `Con(Some, [Var(x)])`
    /// `Node(l, r)` → `Con(Named("Node"), [Var(l), Var(r)])`
    Con {
        tag: CoreTag,
        fields: Vec<CoreExpr>,
        span: Span,
    },

    /// Primitive operation — replaces all infix/prefix operators.
    /// `x + y` → `PrimOp(Add, [Var(x), Var(y)])`
    PrimOp {
        op: CorePrimOp,
        args: Vec<CoreExpr>,
        span: Span,
    },

    /// Algebraic effect — perform an operation.
    Perform {
        effect: Identifier,
        operation: Identifier,
        args: Vec<CoreExpr>,
        span: Span,
    },

    /// Algebraic effect — install a handler.
    Handle {
        body: Box<CoreExpr>,
        effect: Identifier,
        handlers: Vec<CoreHandler>,
        span: Span,
    },
}

// ── Top-level definitions ─────────────────────────────────────────────────────

/// A top-level definition in Core IR.
#[derive(Debug, Clone)]
pub struct CoreDef {
    pub name: Identifier,
    pub expr: CoreExpr,
    /// True when the binding is self-referential (functions, recursive lets).
    pub is_recursive: bool,
    pub span: Span,
}

/// A complete program in Core IR — a sequence of top-level definitions.
#[derive(Debug, Clone)]
pub struct CoreProgram {
    pub defs: Vec<CoreDef>,
}

// ── CoreExpr helpers ──────────────────────────────────────────────────────────

impl CoreExpr {
    pub fn span(&self) -> Span {
        match self {
            CoreExpr::Var(_, s) | CoreExpr::Lit(_, s) => *s,
            CoreExpr::Lam { span, .. }
            | CoreExpr::App { span, .. }
            | CoreExpr::Let { span, .. }
            | CoreExpr::LetRec { span, .. }
            | CoreExpr::Case { span, .. }
            | CoreExpr::Con { span, .. }
            | CoreExpr::PrimOp { span, .. }
            | CoreExpr::Perform { span, .. }
            | CoreExpr::Handle { span, .. } => *span,
        }
    }

    /// Build an n-ary application.
    pub fn apply(func: CoreExpr, args: Vec<CoreExpr>, span: Span) -> CoreExpr {
        if args.is_empty() {
            func
        } else {
            CoreExpr::App { func: Box::new(func), args, span }
        }
    }

    /// Build an n-ary lambda.
    pub fn lambda(params: Vec<Identifier>, body: CoreExpr, span: Span) -> CoreExpr {
        if params.is_empty() {
            body
        } else {
            CoreExpr::Lam { params, body: Box::new(body), span }
        }
    }

    /// Sequence a list of `(var, rhs)` bindings into nested `Let` nodes
    /// terminating in `body`.
    pub fn let_seq(bindings: Vec<(Identifier, CoreExpr)>, body: CoreExpr, span: Span) -> CoreExpr {
        bindings.into_iter().rev().fold(body, |b, (var, rhs)| CoreExpr::Let {
            var,
            rhs: Box::new(rhs),
            body: Box::new(b),
            span,
        })
    }
}
