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
    /// Runtime representation (Proposal 0119).
    /// Determined by HM type inference during AST→Core lowering.
    /// Defaults to `TaggedRep` (NaN-boxed) when type is unknown.
    pub rep: FluxRep,
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
        Self {
            id,
            name,
            rep: FluxRep::TaggedRep,
        }
    }

    /// Create a binder with a known runtime representation.
    pub fn with_rep(id: CoreBinderId, name: Identifier, rep: FluxRep) -> Self {
        Self { id, name, rep }
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

// ── Core types ───────────────────────────────────────────────────────────────

/// Core-level type representation.
///
/// Simplified from `InferType` — no unification variables, no quantifiers.
/// Populated during AST→Core lowering from HM inference results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreType {
    Int,
    Float,
    Bool,
    String,
    Unit,
    Never,
    List(Box<CoreType>),
    Array(Box<CoreType>),
    Tuple(Vec<CoreType>),
    Function(Vec<CoreType>, Box<CoreType>),
    Option(Box<CoreType>),
    Either(Box<CoreType>, Box<CoreType>),
    Map(Box<CoreType>, Box<CoreType>),
    Adt(Identifier),
    /// Type variable or unresolved type.
    Any,
}

impl CoreType {
    /// Convert an HM-inferred `InferType` into a `CoreType`.
    ///
    /// Unification variables and `Any` both map to `CoreType::Any`.
    /// Effect rows on function types are erased (not relevant at Core level).
    pub fn from_infer(ty: &crate::types::infer_type::InferType) -> Self {
        use crate::types::{infer_type::InferType, type_constructor::TypeConstructor};
        match ty {
            InferType::Var(_) => CoreType::Any,
            InferType::Con(tc) => match tc {
                TypeConstructor::Int => CoreType::Int,
                TypeConstructor::Float => CoreType::Float,
                TypeConstructor::Bool => CoreType::Bool,
                TypeConstructor::String => CoreType::String,
                TypeConstructor::Unit => CoreType::Unit,
                TypeConstructor::Never => CoreType::Never,
                TypeConstructor::Any => CoreType::Any,
                TypeConstructor::List => CoreType::List(Box::new(CoreType::Any)),
                TypeConstructor::Array => CoreType::Array(Box::new(CoreType::Any)),
                TypeConstructor::Option => CoreType::Option(Box::new(CoreType::Any)),
                TypeConstructor::Either => {
                    CoreType::Either(Box::new(CoreType::Any), Box::new(CoreType::Any))
                }
                TypeConstructor::Map => {
                    CoreType::Map(Box::new(CoreType::Any), Box::new(CoreType::Any))
                }
                TypeConstructor::Adt(sym) => CoreType::Adt(*sym),
            },
            InferType::App(tc, args) => {
                let core_args: Vec<CoreType> = args.iter().map(CoreType::from_infer).collect();
                match tc {
                    TypeConstructor::List if core_args.len() == 1 => {
                        CoreType::List(Box::new(core_args.into_iter().next().unwrap()))
                    }
                    TypeConstructor::Array if core_args.len() == 1 => {
                        CoreType::Array(Box::new(core_args.into_iter().next().unwrap()))
                    }
                    TypeConstructor::Option if core_args.len() == 1 => {
                        CoreType::Option(Box::new(core_args.into_iter().next().unwrap()))
                    }
                    TypeConstructor::Either if core_args.len() == 2 => {
                        let mut it = core_args.into_iter();
                        CoreType::Either(Box::new(it.next().unwrap()), Box::new(it.next().unwrap()))
                    }
                    TypeConstructor::Map if core_args.len() == 2 => {
                        let mut it = core_args.into_iter();
                        CoreType::Map(Box::new(it.next().unwrap()), Box::new(it.next().unwrap()))
                    }
                    TypeConstructor::Adt(sym) => CoreType::Adt(*sym),
                    _ => CoreType::Any,
                }
            }
            InferType::Fun(params, ret, _effects) => {
                let param_tys = params.iter().map(CoreType::from_infer).collect();
                let ret_ty = CoreType::from_infer(ret);
                CoreType::Function(param_tys, Box::new(ret_ty))
            }
            InferType::Tuple(elems) => {
                CoreType::Tuple(elems.iter().map(CoreType::from_infer).collect())
            }
        }
    }
}

// ── Runtime representation ───────────────────────────────────────────────────

/// Runtime representation of a Flux value (Proposal 0119).
///
/// Determined by HM type inference and carried through Core IR on binders.
/// The `core_to_llvm` backend uses this to choose between boxed (NaN-boxed)
/// and unboxed (raw register) code generation.
///
/// Analogous to GHC's `PrimRep`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FluxRep {
    /// Raw signed 64-bit integer. No NaN-boxing.
    IntRep,
    /// Raw IEEE 754 double. No NaN-boxing.
    FloatRep,
    /// Raw boolean (i1). No NaN-boxing.
    BoolRep,
    /// Heap-allocated boxed value (NaN-boxed pointer).
    /// Used for: String, Array, Closure, ADT, Cons, HashMap.
    BoxedRep,
    /// NaN-boxed value with unknown or polymorphic type.
    /// Fallback when the type is not statically known.
    TaggedRep,
    /// Unit `()`. Minimal runtime representation (tagged None).
    UnitRep,
}

impl FluxRep {
    /// Derive the runtime representation from a `CoreType`.
    pub fn from_core_type(ty: &CoreType) -> Self {
        match ty {
            CoreType::Int => FluxRep::IntRep,
            CoreType::Float => FluxRep::FloatRep,
            CoreType::Bool => FluxRep::BoolRep,
            CoreType::Unit | CoreType::Never => FluxRep::UnitRep,
            CoreType::String
            | CoreType::List(_)
            | CoreType::Array(_)
            | CoreType::Tuple(_)
            | CoreType::Function(_, _)
            | CoreType::Option(_)
            | CoreType::Either(_, _)
            | CoreType::Map(_, _)
            | CoreType::Adt(_) => FluxRep::BoxedRep,
            CoreType::Any => FluxRep::TaggedRep,
        }
    }

    /// Derive the runtime representation from an HM `InferType`.
    pub fn from_infer_type(ty: &crate::types::infer_type::InferType) -> Self {
        FluxRep::from_core_type(&CoreType::from_infer(ty))
    }

    /// Whether this rep is unboxed (no NaN-boxing overhead).
    pub fn is_unboxed(self) -> bool {
        matches!(self, FluxRep::IntRep | FluxRep::FloatRep | FluxRep::BoolRep)
    }

    /// Whether this rep needs reference counting (Aether dup/drop).
    pub fn needs_rc(self) -> bool {
        matches!(self, FluxRep::BoxedRep | FluxRep::TaggedRep)
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
///
/// Discriminants are stable across compiler versions for bytecode cache
/// compatibility. New variants must be appended with the next free ID.
/// Never reuse or renumber existing discriminants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CorePrimOp {
    // ── Generic arithmetic (polymorphic, may fail on type mismatch) ───
    Add = 0,
    Sub = 1,
    Mul = 2,
    Div = 3,
    Mod = 4,

    // ── Typed integer arithmetic ──────────────────────────────────────
    IAdd = 5,
    ISub = 6,
    IMul = 7,
    IDiv = 8,
    IMod = 9,

    // ── Typed float arithmetic ────────────────────────────────────────
    FAdd = 10,
    FSub = 11,
    FMul = 12,
    FDiv = 13,

    // ── Numeric helpers ───────────────────────────────────────────────
    Abs = 14,
    Min = 15,
    Max = 16,
    Neg = 17,

    // ── Logic ─────────────────────────────────────────────────────────
    Not = 18,
    And = 19,
    Or = 20,

    // ── Generic comparisons (polymorphic) ─────────────────────────────
    Eq = 21,
    NEq = 22,
    Lt = 23,
    Le = 24,
    Gt = 25,
    Ge = 26,

    // ── Typed integer comparisons ─────────────────────────────────────
    ICmpEq = 27,
    ICmpNe = 28,
    ICmpLt = 29,
    ICmpLe = 30,
    ICmpGt = 31,
    ICmpGe = 32,

    // ── Typed float comparisons ───────────────────────────────────────
    FCmpEq = 33,
    FCmpNe = 34,
    FCmpLt = 35,
    FCmpLe = 36,
    FCmpGt = 37,
    FCmpGe = 38,

    // ── Deep structural comparison ────────────────────────────────────
    CmpEq = 39,
    CmpNe = 40,

    // ── String / collection constructors ──────────────────────────────
    Concat = 41,
    Interpolate = 42,
    MakeList = 43,
    MakeArray = 44,
    MakeTuple = 45,
    MakeHash = 46,
    Index = 47,

    // ── I/O ───────────────────────────────────────────────────────────
    Print = 48,
    Println = 49,
    ReadFile = 50,
    WriteFile = 51,
    ReadStdin = 52,
    ReadLines = 53,

    // ── String operations ─────────────────────────────────────────────
    StringLength = 54,
    StringConcat = 55,
    StringSlice = 56,
    ToString = 57,
    Split = 58,
    Join = 59,
    Trim = 60,
    Upper = 61,
    Lower = 62,
    StartsWith = 63,
    EndsWith = 64,
    Replace = 65,
    Substring = 66,
    Chars = 67,
    StrContains = 68,

    // ── Array operations ──────────────────────────────────────────────
    ArrayLen = 69,
    ArrayGet = 70,
    ArraySet = 71,
    ArrayPush = 72,
    ArrayConcat = 73,
    ArraySlice = 74,

    // ── HAMT operations ───────────────────────────────────────────────
    HamtGet = 75,
    HamtSet = 76,
    HamtDelete = 77,
    HamtKeys = 78,
    HamtValues = 79,
    HamtMerge = 80,
    HamtSize = 81,
    HamtContains = 82,

    // ── Type tag inspection ───────────────────────────────────────────
    TypeOf = 83,
    IsInt = 84,
    IsFloat = 85,
    IsString = 86,
    IsBool = 87,
    IsArray = 88,
    IsNone = 89,
    IsSome = 90,
    IsList = 91,
    IsMap = 92,

    // ── Control ───────────────────────────────────────────────────────
    Panic = 93,
    ClockNow = 94,
    Try = 95,
    AssertThrows = 96,
    Time = 97,

    // ── Parsing ───────────────────────────────────────────────────────
    ParseInt = 98,
    ParseInts = 99,
    SplitInts = 100,

    // ── List / cons cell operations ───────────────────────────────────
    ToList = 101,
    ToArray = 102,

    // ── Polymorphic length (dispatches on type tag) ───────────────────
    Len = 103,
    // ── Next free ID: 104 ─────────────────────────────────────────────
}

impl CorePrimOp {
    /// Discriminant as a `u8`, for bytecode encoding.
    pub fn id(self) -> u8 {
        self as u8
    }

    /// Reconstruct from a `u8` discriminant.  Returns `None` for invalid IDs.
    pub fn from_id(id: u8) -> Option<Self> {
        if id <= 103 {
            // SAFETY: all discriminants 0..=103 are defined and the enum is
            // `#[repr(u8)]`, so the transmute is valid for any value in range.
            Some(unsafe { std::mem::transmute::<u8, CorePrimOp>(id) })
        } else {
            None
        }
    }
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
    /// Aether: explicit call-site ownership contract.
    /// Each argument position is marked as borrowed or owned after Aether
    /// insertion so later passes do not need to rediscover call semantics.
    AetherCall {
        func: Box<CoreExpr>,
        args: Vec<CoreExpr>,
        arg_modes: Vec<crate::aether::borrow_infer::BorrowMode>,
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
    /// Module/struct member access, resolved at compile time.
    /// Moved out of `CorePrimOp` because it carries data (`Identifier`)
    /// which prevents `#[repr(u8)]` on the primop enum.
    MemberAccess {
        object: Box<CoreExpr>,
        member: Identifier,
        span: Span,
    },
    /// Tuple field access by index.
    /// Moved out of `CorePrimOp` because it carries data (`usize`)
    /// which prevents `#[repr(u8)]` on the primop enum.
    TupleField {
        object: Box<CoreExpr>,
        index: usize,
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
    /// Aether: explicitly duplicate (Rc::clone) a variable reference.
    /// Inserted by the dup/drop pass for variables used more than once.
    Dup {
        var: CoreVarRef,
        body: Box<CoreExpr>,
        span: Span,
    },
    /// Aether: explicitly drop (early release) a variable reference.
    /// Inserted by the dup/drop pass for unused variables.
    Drop {
        var: CoreVarRef,
        body: Box<CoreExpr>,
        span: Span,
    },
    /// Aether: reuse a dropped value's allocation for a new constructor.
    /// If the token's Rc is uniquely owned, writes fields in-place.
    /// If shared, falls back to fresh allocation.
    Reuse {
        token: CoreVarRef,
        tag: CoreTag,
        fields: Vec<CoreExpr>,
        /// Perceus reuse specialization (Section 2.5): bitmask of fields that
        /// actually changed. Bit `i` set means field `i` must be written; clear
        /// means it is unchanged from the destructured original and can be
        /// skipped on the fast (unique-reuse) path. `None` = write all fields.
        field_mask: Option<u64>,
        span: Span,
    },
    /// Aether: Perceus drop specialization (Section 2.3).
    /// Tests if a scrutinee's Rc is uniquely owned (strong_count == 1).
    /// - unique_body: extracted fields are already owned, no dups needed, free shell only.
    /// - shared_body: dup fields, decrement scrutinee refcount (don't free recursively).
    ///   After dup/drop fusion, the unique path has zero RC operations.
    DropSpecialized {
        scrutinee: CoreVarRef,
        unique_body: Box<CoreExpr>,
        shared_body: Box<CoreExpr>,
        span: Span,
    },
}

// ── Top-level definitions ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CoreDef {
    pub name: Identifier,
    pub binder: CoreBinder,
    pub expr: CoreExpr,
    /// Compiler-owned borrow metadata inferred/registered for this definition.
    pub borrow_signature: Option<crate::aether::borrow_infer::BorrowSignature>,
    /// HM-inferred result type for this definition, if available.
    pub result_ty: Option<CoreType>,
    pub is_anonymous: bool,
    pub is_recursive: bool,
    /// FBIP annotation from source: `@fip` or `@fbip` (Perceus Section 2.6).
    pub fip: Option<crate::syntax::statement::FipAnnotation>,
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
        exposing: crate::syntax::statement::ImportExposing,
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
            | CoreExpr::AetherCall { span, .. }
            | CoreExpr::Let { span, .. }
            | CoreExpr::LetRec { span, .. }
            | CoreExpr::Case { span, .. }
            | CoreExpr::Con { span, .. }
            | CoreExpr::PrimOp { span, .. }
            | CoreExpr::Return { span, .. }
            | CoreExpr::Perform { span, .. }
            | CoreExpr::Handle { span, .. }
            | CoreExpr::Dup { span, .. }
            | CoreExpr::Drop { span, .. }
            | CoreExpr::Reuse { span, .. }
            | CoreExpr::DropSpecialized { span, .. }
            | CoreExpr::MemberAccess { span, .. }
            | CoreExpr::TupleField { span, .. } => *span,
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
            borrow_signature: None,
            result_ty: None,
            is_anonymous,
            is_recursive,
            fip: None,
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
        super::passes::run_core_passes(&mut core).expect("core passes should succeed");
        let ir = super::to_ir::lower_core_to_ir(&core);

        assert!(!ir.functions.is_empty(), "expected backend IR functions");
    }
}
