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
    pub fn resolved(binder: &CoreBinder) -> Self {
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
/// Simplified from `InferType`, but capable of preserving semantic residue that
/// reaches Core instead of erasing it to a fake dynamic type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CoreAbstractType {
    ConstructorHead(crate::types::type_constructor::TypeConstructor),
    HigherKindedApp,
    UnsupportedApp(crate::types::type_constructor::TypeConstructor),
    Named(Identifier, Vec<CoreType>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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
    Adt(Identifier, Vec<CoreType>),
    Var(crate::types::TypeVarId),
    Forall(Vec<crate::types::TypeVarId>, Box<CoreType>),
    Abstract(CoreAbstractType),
}

impl CoreType {
    /// Convert an HM-inferred `InferType` into a `CoreType` without
    /// manufacturing a semantic fallback.
    pub fn try_from_infer(ty: &crate::types::infer_type::InferType) -> Option<Self> {
        use crate::types::{infer_type::InferType, type_constructor::TypeConstructor};
        Some(match ty {
            InferType::Var(var) => CoreType::Var(*var),
            InferType::Con(tc) => match tc {
                TypeConstructor::Int => CoreType::Int,
                TypeConstructor::Float => CoreType::Float,
                TypeConstructor::Bool => CoreType::Bool,
                TypeConstructor::String => CoreType::String,
                TypeConstructor::Unit => CoreType::Unit,
                TypeConstructor::Never => CoreType::Never,
                TypeConstructor::List
                | TypeConstructor::Array
                | TypeConstructor::Option
                | TypeConstructor::Either
                | TypeConstructor::Map => {
                    CoreType::Abstract(CoreAbstractType::ConstructorHead(tc.clone()))
                }
                TypeConstructor::Adt(sym) => CoreType::Adt(*sym, Vec::new()),
            },
            InferType::App(tc, args) => {
                let core_args: Vec<CoreType> = args
                    .iter()
                    .map(CoreType::try_from_infer)
                    .collect::<Option<_>>()?;
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
                    TypeConstructor::Adt(sym) => CoreType::Adt(*sym, core_args),
                    _ => CoreType::Abstract(CoreAbstractType::UnsupportedApp(tc.clone())),
                }
            }
            InferType::Fun(params, ret, _effects) => {
                let param_tys: Vec<CoreType> = params
                    .iter()
                    .map(CoreType::try_from_infer)
                    .collect::<Option<_>>()?;
                let ret_ty = CoreType::try_from_infer(ret)?;
                let free_vars: std::collections::BTreeSet<_> = param_tys
                    .iter()
                    .chain(std::iter::once(&ret_ty))
                    .flat_map(CoreType::free_type_vars)
                    .collect();
                let body = CoreType::Function(param_tys, Box::new(ret_ty));
                if free_vars.is_empty() {
                    body
                } else {
                    CoreType::Forall(free_vars.into_iter().collect(), Box::new(body))
                }
            }
            InferType::Tuple(elems) => CoreType::Tuple(
                elems
                    .iter()
                    .map(CoreType::try_from_infer)
                    .collect::<Option<_>>()?,
            ),
            InferType::HktApp(_, _) => CoreType::Abstract(CoreAbstractType::HigherKindedApp),
        })
    }

    /// Convert an HM-inferred `InferType` into a `CoreType`.
    pub fn from_infer(ty: &crate::types::infer_type::InferType) -> Self {
        Self::try_from_infer(ty).expect("CoreType::try_from_infer is total")
    }

    pub fn free_type_vars(&self) -> Vec<crate::types::TypeVarId> {
        let mut vars = std::collections::BTreeSet::new();
        self.collect_free_type_vars(&mut vars);
        vars.into_iter().collect()
    }

    fn collect_free_type_vars(
        &self,
        out: &mut std::collections::BTreeSet<crate::types::TypeVarId>,
    ) {
        match self {
            CoreType::Var(var) => {
                out.insert(*var);
            }
            CoreType::Forall(bound, body) => {
                let mut nested = std::collections::BTreeSet::new();
                body.collect_free_type_vars(&mut nested);
                for var in nested {
                    if !bound.contains(&var) {
                        out.insert(var);
                    }
                }
            }
            CoreType::List(elem) | CoreType::Array(elem) | CoreType::Option(elem) => {
                elem.collect_free_type_vars(out)
            }
            CoreType::Either(left, right) | CoreType::Map(left, right) => {
                left.collect_free_type_vars(out);
                right.collect_free_type_vars(out);
            }
            CoreType::Tuple(elems) => {
                for elem in elems {
                    elem.collect_free_type_vars(out);
                }
            }
            CoreType::Function(params, ret) => {
                for param in params {
                    param.collect_free_type_vars(out);
                }
                ret.collect_free_type_vars(out);
            }
            CoreType::Adt(_, args) => {
                for arg in args {
                    arg.collect_free_type_vars(out);
                }
            }
            CoreType::Int
            | CoreType::Float
            | CoreType::Bool
            | CoreType::String
            | CoreType::Unit
            | CoreType::Never => {}
            CoreType::Abstract(CoreAbstractType::Named(_, args)) => {
                for arg in args {
                    arg.collect_free_type_vars(out);
                }
            }
            CoreType::Abstract(_) => {}
        }
    }
}

// ── Runtime representation ───────────────────────────────────────────────────

/// Runtime representation of a Flux value (Proposal 0119).
///
/// Determined by HM type inference and carried through Core IR on binders.
/// The `llvm` backend uses this to choose between boxed (NaN-boxed)
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
            | CoreType::Adt(_, _) => FluxRep::BoxedRep,
            CoreType::Var(_) | CoreType::Forall(_, _) | CoreType::Abstract(_) => FluxRep::TaggedRep,
        }
    }

    /// Derive the runtime representation from an HM `InferType`.
    pub fn from_infer_type(ty: &crate::types::infer_type::InferType) -> Self {
        CoreType::try_from_infer(ty)
            .map(|ty| FluxRep::from_core_type(&ty))
            .unwrap_or(FluxRep::TaggedRep)
    }

    /// Whether this rep is unboxed (no NaN-boxing overhead).
    pub fn is_unboxed(self) -> bool {
        matches!(self, FluxRep::IntRep | FluxRep::FloatRep | FluxRep::BoolRep)
    }

    /// Whether this rep needs reference counting (Aether dup/drop).
    pub fn needs_rc(self) -> bool {
        matches!(self, FluxRep::BoxedRep | FluxRep::TaggedRep)
    }

    /// Derive the runtime representation from a syntactic `TypeExpr`.
    ///
    /// Requires an interner to resolve named types (e.g. `Int`, `Float`).
    /// Type variables and unknown types default to `TaggedRep`.
    pub fn from_type_expr(
        ty: &crate::syntax::type_expr::TypeExpr,
        interner: &crate::syntax::interner::Interner,
    ) -> Self {
        use crate::syntax::type_expr::TypeExpr;
        match ty {
            TypeExpr::Named { name, args, .. } => {
                let resolved = interner.resolve(*name);
                match resolved {
                    "Int" => FluxRep::IntRep,
                    "Float" => FluxRep::FloatRep,
                    "Bool" => FluxRep::BoolRep,
                    "Unit" | "Never" => FluxRep::UnitRep,
                    "String" | "Array" | "List" | "Map" | "Option" | "Either" => FluxRep::BoxedRep,
                    _ => {
                        // Named type with no args could be an ADT (boxed) or a
                        // type variable (tagged). Single-char names are likely
                        // type params; anything else is an ADT.
                        if args.is_empty()
                            && resolved.len() == 1
                            && resolved.chars().next().is_some_and(|c| c.is_lowercase())
                        {
                            FluxRep::TaggedRep
                        } else {
                            FluxRep::BoxedRep
                        }
                    }
                }
            }
            TypeExpr::Tuple { .. } => FluxRep::BoxedRep,
            TypeExpr::Function { .. } => FluxRep::BoxedRep,
        }
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

// ── Effect classification ─────────────────────────────────────────────────────
//
// The old coarse `PrimEffect { Pure, Io, Time, Control }` enum was retired as
// part of Proposal 0161. The single source of truth for primop effect labels
// is now `crate::syntax::builtin_effects::primop_fine_effect_label`, which
// returns the decomposed label (`Console`, `FileSystem`, `Stdin`, `Clock`,
// `Panic`) or `None` for pure primops. The `Pure`/`CanFail`/`HasEffect`
// classification used by optimizer passes lives in
// `crate::core::passes::helpers::PrimOpEffectClass` (Proposal 0161 Phase 3).

/// Whether a primop is part of the long-term internal compiler/runtime
/// contract or only kept temporarily while public stdlib ownership is
/// migrating.
///
/// After Proposal 0164 Phase 7, no primop is currently `TransitionalStdlib`
/// — the variant is retained so future migrations can reuse the classifier
/// without reintroducing the type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimSurfaceKind {
    CoreContract,
    #[allow(dead_code)]
    TransitionalStdlib,
}

// ── Primitive operations ──────────────────────────────────────────────────────

/// All operators and built-in operations lower to `CorePrimOp`.
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
    // Join (59) removed — see Flow.String.join.
    Trim = 60,
    Upper = 61,
    Lower = 62,
    // StartsWith (63), EndsWith (64) removed — see
    // Flow.String.starts_with / ends_with.
    Replace = 65,
    Substring = 66,
    // Chars (67), StrContains (68) removed — see
    // Flow.String.chars / str_contains.

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

    // ── List / cons cell operations ───────────────────────────────────
    // ToList (101), ToArray (102) removed — see
    // Flow.Array.to_list / Flow.Array.to_array.

    // ── Polymorphic length (dispatches on type tag) ───────────────────
    Len = 103,

    // ── Collection helpers (promoted for native compilation) ─────────
    // First (104), Rest (105), Last (112), ArrayReverse (106),
    // ArrayContains (107), Sort (108), SortBy (109), HoMap (110),
    // HoFilter (111), HoFold (112), HoAny (113), HoAll (114),
    // HoEach (115), HoFind (116), HoCount (117), Zip (118),
    // Flatten (119), HoFlatMap (120) removed — stdlib is source of truth.

    // ── Effect handlers (Koka-style yield model, Proposal 0134) ───────
    EvvGet = 121,
    EvvSet = 122,
    FreshMarker = 123,
    EvvInsert = 124,
    YieldTo = 125,
    YieldExtend = 126,
    YieldPrompt = 127,
    IsYielding = 128,
    /// Direct (tail-resumptive) perform: calls handler inline, no yield.
    PerformDirect = 129,

    // ── Option operations ────────────────────────────────────────────
    /// Unwrap a Some value — panics if None.
    Unwrap = 130,

    // ── Safe arithmetic (Proposal 0135) ──────────────────────────────
    /// Total division: returns `Some(a / b)` or `None` when `b == 0`.
    SafeDiv = 131,
    /// Total modulo:   returns `Some(a % b)` or `None` when `b == 0`.
    SafeMod = 132,

    // ── Float math helpers (Phase 2) ─────────────────────────────────
    FSqrt = 133,
    FSin = 134,
    FCos = 135,
    FExp = 136,
    FLog = 137,
    FFloor = 138,
    FCeil = 139,
    FRound = 140,

    // ── Bitwise integer helpers (Phase 2) ────────────────────────────
    BitAnd = 141,
    BitOr = 142,
    BitXor = 143,
    BitShl = 144,
    BitShr = 145,

    // ── Float math helpers extension (trig + hyperbolic + truncate) ──
    FTan = 146,
    FAsin = 147,
    FAcos = 148,
    FAtan = 149,
    FSinh = 150,
    FCosh = 151,
    FTanh = 152,
    FTruncate = 153,
    // ── Next free ID: 154 ─────────────────────────────────────────────
}

impl CorePrimOp {
    /// Resolve a primop name used in an `intrinsic fn ... = primop ...`
    /// declaration. Accepts exact enum-style names like `ArrayLen` and a
    /// fallback snake_case form like `array_len`.
    pub fn from_intrinsic_name(name: &str) -> Option<Self> {
        match name {
            "HamtGet" => return Some(Self::HamtGet),
            "HamtSet" => return Some(Self::HamtSet),
            "HamtDelete" => return Some(Self::HamtDelete),
            "HamtMerge" => return Some(Self::HamtMerge),
            "HamtKeys" => return Some(Self::HamtKeys),
            "HamtValues" => return Some(Self::HamtValues),
            "HamtSize" => return Some(Self::HamtSize),
            "HamtContains" => return Some(Self::HamtContains),
            "FSqrt" => return Some(Self::FSqrt),
            "FSin" => return Some(Self::FSin),
            "FCos" => return Some(Self::FCos),
            "FExp" => return Some(Self::FExp),
            "FLog" => return Some(Self::FLog),
            "FFloor" => return Some(Self::FFloor),
            "FCeil" => return Some(Self::FCeil),
            "FRound" => return Some(Self::FRound),
            "BitAnd" => return Some(Self::BitAnd),
            "BitOr" => return Some(Self::BitOr),
            "BitXor" => return Some(Self::BitXor),
            "BitShl" => return Some(Self::BitShl),
            "BitShr" => return Some(Self::BitShr),
            "FTan" => return Some(Self::FTan),
            "FAsin" => return Some(Self::FAsin),
            "FAcos" => return Some(Self::FAcos),
            "FAtan" => return Some(Self::FAtan),
            "FSinh" => return Some(Self::FSinh),
            "FCosh" => return Some(Self::FCosh),
            "FTanh" => return Some(Self::FTanh),
            "FTruncate" => return Some(Self::FTruncate),
            _ => {}
        }
        let snake = camel_to_snake(name);
        (0..=5)
            .find_map(|arity| Self::from_name(&snake, arity))
            .or_else(|| (0..=5).find_map(|arity| Self::from_name(name, arity)))
    }

    /// Preferred function-style helper spelling to use when desugaring an
    /// intrinsic declaration into an ordinary function body.
    pub fn intrinsic_helper_name(self) -> Option<&'static str> {
        match self {
            Self::ArrayLen => Some("array_len"),
            Self::ArrayGet => Some("array_get"),
            Self::ArraySet => Some("array_set"),
            Self::ArrayPush => Some("array_push"),
            Self::ArrayConcat => Some("array_concat"),
            Self::ArraySlice => Some("array_slice"),
            Self::HamtGet => Some("map_get"),
            Self::HamtSet => Some("map_set"),
            Self::HamtDelete => Some("map_delete"),
            Self::HamtMerge => Some("map_merge"),
            Self::HamtKeys => Some("map_keys"),
            Self::HamtValues => Some("map_values"),
            Self::HamtSize => Some("map_size"),
            Self::HamtContains => Some("map_has"),
            Self::StringLength => Some("string_length"),
            // Helper names used by `public intrinsic fn … = primop …` desugaring.
            // These must differ from the declared intrinsic names in
            // `lib/Flow/String.flx` (`string_concat`, `string_slice`); otherwise
            // the synthesized body is a recursive self-call rather than a
            // builtin-helper call and the primop is never invoked at runtime.
            Self::StringConcat => Some("string_concat_builtin"),
            Self::StringSlice => Some("string_slice_builtin"),
            Self::Len => Some("len"),
            Self::ParseInt => Some("parse_int"),
            Self::ToString => Some("to_string"),
            Self::Print => Some("print"),
            Self::Println => Some("println"),
            Self::ReadFile => Some("read_file"),
            Self::WriteFile => Some("write_file"),
            Self::ReadStdin => Some("read_stdin"),
            Self::ReadLines => Some("read_lines"),
            Self::ClockNow => Some("clock_now"),
            Self::Time => Some("time"),
            Self::Try => Some("try"),
            Self::AssertThrows => Some("assert_throws"),
            Self::TypeOf => Some("type_of"),
            Self::FSqrt => Some("fsqrt"),
            Self::FSin => Some("fsin"),
            Self::FCos => Some("fcos"),
            Self::FExp => Some("fexp"),
            Self::FLog => Some("flog"),
            Self::FFloor => Some("ffloor"),
            Self::FCeil => Some("fceil"),
            Self::FRound => Some("fround"),
            Self::BitAnd => Some("bit_and"),
            Self::BitOr => Some("bit_or"),
            Self::BitXor => Some("bit_xor"),
            Self::BitShl => Some("bit_shl"),
            Self::BitShr => Some("bit_shr"),
            Self::FTan => Some("ftan"),
            Self::FAsin => Some("fasin"),
            Self::FAcos => Some("facos"),
            Self::FAtan => Some("fatan"),
            Self::FSinh => Some("fsinh"),
            Self::FCosh => Some("fcosh"),
            Self::FTanh => Some("ftanh"),
            Self::FTruncate => Some("ftruncate"),
            _ => None,
        }
    }

    /// Discriminant as a `u8`, for bytecode encoding.
    pub fn id(self) -> u8 {
        self as u8
    }

    /// Reconstruct from a `u8` discriminant.  Returns `None` for invalid IDs.
    ///
    /// Phase 7 of Proposal 0164 left gaps in the discriminant space
    /// (e.g. 59, 63–64, 67–68, 99–102, 104–120). A plain `transmute` for
    /// values in those gaps would be undefined behavior, so this function
    /// explicitly matches each retained variant. Mappings are kept in sync
    /// with the `CorePrimOp` enum declaration — verify any new variant is
    /// added to both.
    pub fn from_id(id: u8) -> Option<Self> {
        use CorePrimOp::*;
        let op = match id {
            0 => Add, 1 => Sub, 2 => Mul, 3 => Div, 4 => Mod,
            5 => IAdd, 6 => ISub, 7 => IMul, 8 => IDiv, 9 => IMod,
            10 => FAdd, 11 => FSub, 12 => FMul, 13 => FDiv,
            14 => Abs, 15 => Min, 16 => Max, 17 => Neg,
            18 => Not, 19 => And, 20 => Or,
            21 => Eq, 22 => NEq, 23 => Lt, 24 => Le, 25 => Gt, 26 => Ge,
            27 => ICmpEq, 28 => ICmpNe, 29 => ICmpLt, 30 => ICmpLe,
            31 => ICmpGt, 32 => ICmpGe,
            33 => FCmpEq, 34 => FCmpNe, 35 => FCmpLt, 36 => FCmpLe,
            37 => FCmpGt, 38 => FCmpGe,
            39 => CmpEq, 40 => CmpNe,
            41 => Concat, 42 => Interpolate,
            43 => MakeList, 44 => MakeArray, 45 => MakeTuple, 46 => MakeHash,
            47 => Index,
            48 => Print, 49 => Println,
            50 => ReadFile, 51 => WriteFile, 52 => ReadStdin, 53 => ReadLines,
            54 => StringLength, 55 => StringConcat, 56 => StringSlice,
            57 => ToString, 58 => Split,
            60 => Trim, 61 => Upper, 62 => Lower,
            65 => Replace, 66 => Substring,
            69 => ArrayLen, 70 => ArrayGet, 71 => ArraySet, 72 => ArrayPush,
            73 => ArrayConcat, 74 => ArraySlice,
            75 => HamtGet, 76 => HamtSet, 77 => HamtDelete, 78 => HamtKeys,
            79 => HamtValues, 80 => HamtMerge, 81 => HamtSize, 82 => HamtContains,
            83 => TypeOf, 84 => IsInt, 85 => IsFloat, 86 => IsString,
            87 => IsBool, 88 => IsArray, 89 => IsNone, 90 => IsSome,
            91 => IsList, 92 => IsMap,
            93 => Panic, 94 => ClockNow, 95 => Try, 96 => AssertThrows,
            97 => Time, 98 => ParseInt,
            103 => Len,
            121 => EvvGet, 122 => EvvSet, 123 => FreshMarker, 124 => EvvInsert,
            125 => YieldTo, 126 => YieldExtend, 127 => YieldPrompt,
            128 => IsYielding, 129 => PerformDirect,
            130 => Unwrap, 131 => SafeDiv, 132 => SafeMod,
            133 => FSqrt, 134 => FSin, 135 => FCos, 136 => FExp,
            137 => FLog, 138 => FFloor, 139 => FCeil, 140 => FRound,
            141 => BitAnd, 142 => BitOr, 143 => BitXor, 144 => BitShl, 145 => BitShr,
            146 => FTan, 147 => FAsin, 148 => FAcos, 149 => FAtan,
            150 => FSinh, 151 => FCosh, 152 => FTanh, 153 => FTruncate,
            _ => return None,
        };
        Some(op)
    }

    /// Resolve a function name + arity to a `CorePrimOp`, if it names a
    /// built-in primitive.  Used by the bytecode compiler to emit `OpPrimOp`.
    pub fn from_name(name: &str, arity: usize) -> Option<Self> {
        // Sorted by (name, arity) for binary search.
        static TABLE: &[(&str, usize, CorePrimOp)] = &[
            ("abs", 1, CorePrimOp::Abs),
            ("array_concat", 2, CorePrimOp::ArrayConcat),
            ("array_get", 2, CorePrimOp::ArrayGet),
            ("array_len", 1, CorePrimOp::ArrayLen),
            ("array_push", 2, CorePrimOp::ArrayPush),
            ("array_slice", 3, CorePrimOp::ArraySlice),
            ("array_set", 3, CorePrimOp::ArraySet),
            ("assert_throws", 1, CorePrimOp::AssertThrows),
            ("assert_throws", 2, CorePrimOp::AssertThrows),
            ("bit_and", 2, CorePrimOp::BitAnd),
            ("bit_or", 2, CorePrimOp::BitOr),
            ("bit_shl", 2, CorePrimOp::BitShl),
            ("bit_shr", 2, CorePrimOp::BitShr),
            ("bit_xor", 2, CorePrimOp::BitXor),
            ("clock_now", 0, CorePrimOp::ClockNow),
            ("cmp_eq", 2, CorePrimOp::CmpEq),
            ("cmp_ne", 2, CorePrimOp::CmpNe),
            ("fadd", 2, CorePrimOp::FAdd),
            ("fceil", 1, CorePrimOp::FCeil),
            ("fcmp_eq", 2, CorePrimOp::FCmpEq),
            ("fcmp_ge", 2, CorePrimOp::FCmpGe),
            ("fcmp_gt", 2, CorePrimOp::FCmpGt),
            ("fcmp_le", 2, CorePrimOp::FCmpLe),
            ("fcmp_lt", 2, CorePrimOp::FCmpLt),
            ("fcmp_ne", 2, CorePrimOp::FCmpNe),
            ("facos", 1, CorePrimOp::FAcos),
            ("fasin", 1, CorePrimOp::FAsin),
            ("fatan", 1, CorePrimOp::FAtan),
            ("fcos", 1, CorePrimOp::FCos),
            ("fcosh", 1, CorePrimOp::FCosh),
            ("fdiv", 2, CorePrimOp::FDiv),
            ("fexp", 1, CorePrimOp::FExp),
            ("ffloor", 1, CorePrimOp::FFloor),
            ("flog", 1, CorePrimOp::FLog),
            ("fmul", 2, CorePrimOp::FMul),
            ("fround", 1, CorePrimOp::FRound),
            ("fsin", 1, CorePrimOp::FSin),
            ("fsinh", 1, CorePrimOp::FSinh),
            ("fsqrt", 1, CorePrimOp::FSqrt),
            ("fsub", 2, CorePrimOp::FSub),
            ("ftan", 1, CorePrimOp::FTan),
            ("ftanh", 1, CorePrimOp::FTanh),
            ("ftruncate", 1, CorePrimOp::FTruncate),
            ("acos", 1, CorePrimOp::FAcos),
            ("asin", 1, CorePrimOp::FAsin),
            ("atan", 1, CorePrimOp::FAtan),
            ("ceil", 1, CorePrimOp::FCeil),
            ("cos", 1, CorePrimOp::FCos),
            ("cosh", 1, CorePrimOp::FCosh),
            ("exp", 1, CorePrimOp::FExp),
            ("floor", 1, CorePrimOp::FFloor),
            ("log", 1, CorePrimOp::FLog),
            ("sinh", 1, CorePrimOp::FSinh),
            ("tan", 1, CorePrimOp::FTan),
            ("tanh", 1, CorePrimOp::FTanh),
            ("truncate", 1, CorePrimOp::FTruncate),
            ("iadd", 2, CorePrimOp::IAdd),
            ("icmp_eq", 2, CorePrimOp::ICmpEq),
            ("icmp_ge", 2, CorePrimOp::ICmpGe),
            ("icmp_gt", 2, CorePrimOp::ICmpGt),
            ("icmp_le", 2, CorePrimOp::ICmpLe),
            ("icmp_lt", 2, CorePrimOp::ICmpLt),
            ("icmp_ne", 2, CorePrimOp::ICmpNe),
            ("idiv", 2, CorePrimOp::IDiv),
            ("imod", 2, CorePrimOp::IMod),
            ("imul", 2, CorePrimOp::IMul),
            ("is_array", 1, CorePrimOp::IsArray),
            ("is_bool", 1, CorePrimOp::IsBool),
            ("is_float", 1, CorePrimOp::IsFloat),
            ("is_hash", 1, CorePrimOp::IsMap),
            ("is_int", 1, CorePrimOp::IsInt),
            ("is_list", 1, CorePrimOp::IsList),
            ("is_map", 1, CorePrimOp::IsMap),
            ("is_none", 1, CorePrimOp::IsNone),
            ("is_some", 1, CorePrimOp::IsSome),
            ("is_string", 1, CorePrimOp::IsString),
            ("isub", 2, CorePrimOp::ISub),
            ("len", 1, CorePrimOp::Len),
            ("lower", 1, CorePrimOp::Lower),
            ("map_delete", 2, CorePrimOp::HamtDelete),
            ("map_get", 2, CorePrimOp::HamtGet),
            ("map_has", 2, CorePrimOp::HamtContains),
            ("map_keys", 1, CorePrimOp::HamtKeys),
            ("map_merge", 2, CorePrimOp::HamtMerge),
            ("map_set", 3, CorePrimOp::HamtSet),
            ("map_size", 1, CorePrimOp::HamtSize),
            ("map_values", 1, CorePrimOp::HamtValues),
            ("max", 2, CorePrimOp::Max),
            ("min", 2, CorePrimOp::Min),
            ("now_ms", 0, CorePrimOp::ClockNow),
            ("panic", 1, CorePrimOp::Panic),
            ("parse_int", 1, CorePrimOp::ParseInt),
            ("print", 1, CorePrimOp::Print),
            ("println", 1, CorePrimOp::Println),
            ("read_file", 1, CorePrimOp::ReadFile),
            ("read_lines", 1, CorePrimOp::ReadLines),
            ("read_stdin", 0, CorePrimOp::ReadStdin),
            ("replace", 3, CorePrimOp::Replace),
            ("round", 1, CorePrimOp::FRound),
            ("safe_div", 2, CorePrimOp::SafeDiv),
            ("safe_mod", 2, CorePrimOp::SafeMod),
            ("split", 2, CorePrimOp::Split),
            ("sin", 1, CorePrimOp::FSin),
            ("string_concat", 2, CorePrimOp::StringConcat),
            ("string_concat_builtin", 2, CorePrimOp::StringConcat),
            ("string_len", 1, CorePrimOp::StringLength),
            ("string_length", 1, CorePrimOp::StringLength),
            ("string_slice", 3, CorePrimOp::StringSlice),
            ("string_slice_builtin", 3, CorePrimOp::StringSlice),
            ("substring", 3, CorePrimOp::Substring),
            ("sqrt", 1, CorePrimOp::FSqrt),
            ("time", 0, CorePrimOp::Time),
            ("to_string", 1, CorePrimOp::ToString),
            ("trim", 1, CorePrimOp::Trim),
            ("try", 1, CorePrimOp::Try),
            ("type_of", 1, CorePrimOp::TypeOf),
            ("unwrap", 1, CorePrimOp::Unwrap),
            ("upper", 1, CorePrimOp::Upper),
            ("write_file", 2, CorePrimOp::WriteFile),
        ];
        TABLE
            .iter()
            .find(|(n, a, _)| *n == name && *a == arity)
            .map(|(_, _, op)| *op)
    }

    /// Number of arguments this primop expects.
    pub fn arity(self) -> usize {
        use CorePrimOp::*;
        match self {
            ClockNow | ReadStdin | Time => 0,
            Abs | ArrayLen | IsArray | IsBool | IsFloat | IsInt | IsList | IsMap
            | IsNone | IsSome | IsString | Len | Lower | Panic | ParseInt | Print
            | Println | ReadFile | ReadLines | StringLength | ToString
            | Trim | Try | AssertThrows | TypeOf | Upper | HamtKeys | HamtValues | HamtSize
            | Neg | Not | Unwrap | FSqrt | FSin | FCos | FExp
            | FLog | FFloor | FCeil | FRound
            | FTan | FAsin | FAcos | FAtan | FSinh | FCosh | FTanh | FTruncate => 1,
            Add | Sub | Mul | Div | Mod | IAdd | ISub | IMul | IDiv | IMod | FAdd | FSub | FMul
            | FDiv | Eq | NEq | Lt | Le | Gt | Ge | ICmpEq | ICmpNe | ICmpLt | ICmpLe | ICmpGt
            | ICmpGe | FCmpEq | FCmpNe | FCmpLt | FCmpLe | FCmpGt | FCmpGe | CmpEq | CmpNe
            | And | Or | Concat | ArrayGet | ArrayPush | ArrayConcat | HamtGet | HamtContains
            | HamtDelete | HamtMerge | Index | Max | Min | Split
            | StringConcat | WriteFile
            | SafeDiv | SafeMod | BitAnd | BitOr | BitXor | BitShl | BitShr => 2,
            ArraySet | ArraySlice | HamtSet | Replace | StringSlice | Substring => 3,
            // Variadic: MakeList, MakeArray, MakeTuple, MakeHash, Interpolate
            // are handled separately by the compiler, not via OpPrimOp.
            MakeList | MakeArray | MakeTuple | MakeHash | Interpolate => 0,
            // Effect handler ops (native-only, arity used for display only)
            EvvGet | IsYielding => 0,
            EvvSet | YieldExtend | FreshMarker => 1,
            YieldTo => 3,
            EvvInsert => 4,
            YieldPrompt => 3,
            PerformDirect => 5,
        }
    }

    /// Phase 1 inventory freeze classification.
    ///
    /// After Phase 7 removals, no primop is classified as
    /// `TransitionalStdlib` — the previously-transitional text helpers
    /// (`Split`, `Trim`, `Upper`, `Lower`, `Replace`, `Substring`) were
    /// reclassified to `CoreContract` because they depend on Unicode-aware
    /// C runtime behavior that can't be replicated efficiently in pure
    /// Flux. All surviving primops are part of the long-term internal
    /// contract.
    pub fn surface_kind(self) -> PrimSurfaceKind {
        PrimSurfaceKind::CoreContract
    }

    /// Legacy globally-recognized helper spellings that should move toward
    /// module-qualified `Flow.*` APIs. Returns the preferred replacement.
    pub fn legacy_surface_replacement(name: &str, arity: usize) -> Option<&'static str> {
        match (name, arity) {
            ("array_contains", 2) => Some("Flow.Array.contains"),
            ("array_concat", 2) => Some("Flow.Array.concat"),
            ("array_get", 2) => Some("Flow.Array.get"),
            ("array_len", 1) => Some("Flow.Array.length"),
            ("array_push", 2) => Some("Flow.Array.push"),
            ("array_reverse", 1) => Some("Flow.Array.reverse"),
            ("array_slice", 3) => Some("Flow.Array.slice"),
            ("array_set", 3) => Some("Flow.Array.update"),
            ("chars", 1) => Some("Flow.String.chars"),
            ("ends_with", 2) => Some("Flow.String.ends_with"),
            ("join", 2) => Some("Flow.String.join"),
            ("map_delete", 2) => Some("Flow.Map.delete"),
            ("map_get", 2) => Some("Flow.Map.get"),
            ("map_has", 2) => Some("Flow.Map.has"),
            ("map_keys", 1) => Some("Flow.Map.keys"),
            ("map_merge", 2) => Some("Flow.Map.merge"),
            ("map_set", 3) => Some("Flow.Map.set"),
            ("map_size", 1) => Some("Flow.Map.size"),
            ("map_values", 1) => Some("Flow.Map.values"),
            ("parse_ints", 1) => Some("Flow.IO.parse_ints"),
            ("split_ints", 2) => Some("Flow.IO.split_ints"),
            ("starts_with", 2) => Some("Flow.String.starts_with"),
            ("str_contains", 2) => Some("Flow.String.str_contains"),
            ("string_len", 1) | ("string_length", 1) => Some("Flow.String.string_len"),
            ("to_array", 1) => Some("Flow.List.to_array"),
            ("to_list", 1) => Some("Flow.Array.to_list"),
            _ => None,
        }
    }

    /// Whether this primop borrows its arguments (no ownership transfer).
    /// Most primops borrow; only `ArrayPush` and `ArrayConcat` consume args.
    pub fn borrows_args(self) -> bool {
        !matches!(self, Self::ArrayPush | Self::ArrayConcat)
    }

    /// Look up borrow mode for a named function that may be a primop.
    /// Returns `Some((arity, borrows))` if the name resolves at any arity.
    /// Used by Aether borrow inference.
    pub fn resolve_borrow_info(name: &str) -> Option<(usize, bool)> {
        // Try arities 0..=3 (covers all primops).
        for arity in 0..=3 {
            if let Some(op) = Self::from_name(name, arity) {
                return Some((arity, op.borrows_args()));
            }
        }
        None
    }
}

fn camel_to_snake(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (idx, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if idx != 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
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
    pub param_types: Vec<Option<CoreType>>,
    pub resume: CoreBinder,
    pub resume_ty: Option<CoreType>,
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
        param_types: Vec<Option<CoreType>>,
        result_ty: Option<CoreType>,
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
    /// Multi-binding recursive let for mutually recursive functions.
    /// All binders are in scope for all RHS expressions, enabling mutual
    /// recursion. Analogous to GHC's `Rec` / Koka's `DefRec`.
    LetRecGroup {
        bindings: Vec<(CoreBinder, Box<CoreExpr>)>,
        body: Box<CoreExpr>,
        span: Span,
    },
    Case {
        scrutinee: Box<CoreExpr>,
        alts: Vec<CoreAlt>,
        join_ty: Option<CoreType>,
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
    Let {
        is_public: bool,
        name: Identifier,
        span: Span,
    },
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
    Class {
        name: Identifier,
        type_params: Vec<Identifier>,
        superclasses: Vec<crate::syntax::type_class::ClassConstraint>,
        methods: Vec<crate::syntax::type_class::ClassMethod>,
        span: Span,
    },
    Instance {
        class_name: Identifier,
        type_args: Vec<TypeExpr>,
        context: Vec<crate::syntax::type_class::ClassConstraint>,
        methods: Vec<crate::syntax::type_class::InstanceMethod>,
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
    pub fn bound_var(binder: &CoreBinder, span: Span) -> CoreExpr {
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
            | CoreExpr::LetRecGroup { span, .. }
            | CoreExpr::Case { span, .. }
            | CoreExpr::Con { span, .. }
            | CoreExpr::PrimOp { span, .. }
            | CoreExpr::Return { span, .. }
            | CoreExpr::Perform { span, .. }
            | CoreExpr::Handle { span, .. }
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
        Self::lambda_typed(params, Vec::new(), None, body, span)
    }

    pub fn lambda_typed(
        params: Vec<CoreBinder>,
        param_types: Vec<Option<CoreType>>,
        result_ty: Option<CoreType>,
        body: CoreExpr,
        span: Span,
    ) -> CoreExpr {
        if params.is_empty() {
            body
        } else {
            CoreExpr::Lam {
                params,
                param_types,
                result_ty,
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
    use crate::types::{infer_type::InferType, type_constructor::TypeConstructor};

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

    #[test]
    fn try_from_infer_preserves_unresolved_vars() {
        assert_eq!(
            super::CoreType::try_from_infer(&InferType::Var(0)),
            Some(super::CoreType::Var(0))
        );
    }

    #[test]
    fn try_from_infer_preserves_concrete_map_shape() {
        let inferred = InferType::App(
            TypeConstructor::Map,
            vec![
                InferType::Con(TypeConstructor::String),
                InferType::Con(TypeConstructor::Int),
            ],
        );
        assert_eq!(
            super::CoreType::try_from_infer(&inferred),
            Some(super::CoreType::Map(
                Box::new(super::CoreType::String),
                Box::new(super::CoreType::Int),
            ))
        );
    }

    #[test]
    fn try_from_infer_preserves_parameterized_adt_shape() {
        let inferred = InferType::App(
            TypeConstructor::Adt(crate::syntax::symbol::Symbol::new(7)),
            vec![InferType::Con(TypeConstructor::Int), InferType::Var(3)],
        );
        assert_eq!(
            super::CoreType::try_from_infer(&inferred),
            Some(super::CoreType::Adt(
                crate::syntax::symbol::Symbol::new(7),
                vec![super::CoreType::Int, super::CoreType::Var(3)]
            ))
        );
    }

    #[test]
    fn try_from_infer_wraps_polymorphic_function_in_forall() {
        let inferred = InferType::Fun(
            vec![InferType::Var(1)],
            Box::new(InferType::Var(1)),
            crate::types::infer_effect_row::InferEffectRow::closed_empty(),
        );
        assert_eq!(
            super::CoreType::try_from_infer(&inferred),
            Some(super::CoreType::Forall(
                vec![1],
                Box::new(super::CoreType::Function(
                    vec![super::CoreType::Var(1)],
                    Box::new(super::CoreType::Var(1))
                ))
            ))
        );
    }

    #[test]
    fn primop_surface_kind_keeps_len_and_parse_int_in_core_contract() {
        assert_eq!(
            super::CorePrimOp::Len.surface_kind(),
            super::PrimSurfaceKind::CoreContract
        );
        assert_eq!(
            super::CorePrimOp::ParseInt.surface_kind(),
            super::PrimSurfaceKind::CoreContract
        );
        assert_eq!(
            super::CorePrimOp::ArrayLen.surface_kind(),
            super::PrimSurfaceKind::CoreContract
        );
    }

    /// Spot-check that `from_id` inverts `id` for a representative sample of
    /// variants across the whole range. Each entry pairs a variant with its
    /// expected numeric discriminant; if either side drifts, this fires and
    /// prevents a regression like the one that made `from_id(14)` return
    /// `Eq` instead of `Abs`.
    #[test]
    fn primop_from_id_roundtrips_representative_variants() {
        use super::CorePrimOp::*;
        let cases = [
            (Add, 0u8),
            (Abs, 14),
            (Not, 18),
            (Eq, 21),
            (CmpEq, 39),
            (MakeArray, 44),
            (Index, 47),
            (Print, 48),
            (StringLength, 54),
            (Trim, 60),
            (ArrayLen, 69),
            (HamtGet, 75),
            (TypeOf, 83),
            (ParseInt, 98),
            (Len, 103),
            (EvvGet, 121),
            (SafeDiv, 131),
            (FSqrt, 133),
            (BitAnd, 141),
            (FTan, 146),
            (FTruncate, 153),
        ];
        for (variant, id) in cases {
            assert_eq!(
                variant.id(),
                id,
                "variant {variant:?} should have discriminant {id}"
            );
            assert_eq!(
                super::CorePrimOp::from_id(id),
                Some(variant),
                "from_id({id}) should return {variant:?}"
            );
        }
    }

    #[test]
    fn legacy_surface_replacements_point_to_flow_modules() {
        assert_eq!(
            super::CorePrimOp::legacy_surface_replacement("array_len", 1),
            Some("Flow.Array.length")
        );
        assert_eq!(
            super::CorePrimOp::legacy_surface_replacement("map_get", 2),
            Some("Flow.Map.get")
        );
        assert_eq!(
            super::CorePrimOp::legacy_surface_replacement("string_len", 1),
            Some("Flow.String.string_len")
        );
        assert_eq!(
            super::CorePrimOp::legacy_surface_replacement("len", 1),
            None
        );
    }
}
