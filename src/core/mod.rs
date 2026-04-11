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
            InferType::HktApp(_, _) => CoreType::Any,
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

/// Side-effect classification for primitive operations.
///
/// Used for optimization/planning decisions where purity matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimEffect {
    /// Deterministic and side-effect free.
    Pure,
    /// Performs observable I/O.
    Io,
    /// Depends on wall-clock or monotonic time.
    Time,
    /// Affects control flow in non-local ways.
    Control,
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

    // ── Collection helpers (promoted for native compilation) ─────────
    // First (104), Rest (105), Last (112) removed — stdlib is source of truth.
    ArrayReverse = 106,
    ArrayContains = 107,
    Sort = 108,
    SortBy = 109,
    HoMap = 110,
    HoFilter = 111,
    HoFold = 112,
    HoAny = 113,
    HoAll = 114,
    HoEach = 115,
    HoFind = 116,
    HoCount = 117,
    Zip = 118,
    Flatten = 119,
    HoFlatMap = 120,

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
    // ── Next free ID: 133 ─────────────────────────────────────────────
}

impl CorePrimOp {
    /// Discriminant as a `u8`, for bytecode encoding.
    pub fn id(self) -> u8 {
        self as u8
    }

    /// Reconstruct from a `u8` discriminant.  Returns `None` for invalid IDs.
    pub fn from_id(id: u8) -> Option<Self> {
        if id <= 132 {
            // SAFETY: all discriminants 0..=132 are defined and the enum is
            // `#[repr(u8)]`, so the transmute is valid for any value in range.
            Some(unsafe { std::mem::transmute::<u8, CorePrimOp>(id) })
        } else {
            None
        }
    }

    /// Resolve a function name + arity to a `CorePrimOp`, if it names a
    /// built-in primitive.  Used by the bytecode compiler to emit `OpPrimOp`.
    pub fn from_name(name: &str, arity: usize) -> Option<Self> {
        // Sorted by (name, arity) for binary search.
        static TABLE: &[(&str, usize, CorePrimOp)] = &[
            ("abs", 1, CorePrimOp::Abs),
            ("array_contains", 2, CorePrimOp::ArrayContains),
            ("array_concat", 2, CorePrimOp::ArrayConcat),
            ("array_get", 2, CorePrimOp::ArrayGet),
            ("array_len", 1, CorePrimOp::ArrayLen),
            ("array_push", 2, CorePrimOp::ArrayPush),
            ("array_reverse", 1, CorePrimOp::ArrayReverse),
            ("array_slice", 3, CorePrimOp::ArraySlice),
            ("array_set", 3, CorePrimOp::ArraySet),
            ("assert_throws", 1, CorePrimOp::AssertThrows),
            ("assert_throws", 2, CorePrimOp::AssertThrows),
            ("chars", 1, CorePrimOp::Chars),
            ("clock_now", 0, CorePrimOp::ClockNow),
            ("cmp_eq", 2, CorePrimOp::CmpEq),
            ("cmp_ne", 2, CorePrimOp::CmpNe),
            ("ends_with", 2, CorePrimOp::EndsWith),
            ("fadd", 2, CorePrimOp::FAdd),
            ("fcmp_eq", 2, CorePrimOp::FCmpEq),
            ("fcmp_ge", 2, CorePrimOp::FCmpGe),
            ("fcmp_gt", 2, CorePrimOp::FCmpGt),
            ("fcmp_le", 2, CorePrimOp::FCmpLe),
            ("fcmp_lt", 2, CorePrimOp::FCmpLt),
            ("fcmp_ne", 2, CorePrimOp::FCmpNe),
            ("fdiv", 2, CorePrimOp::FDiv),
            ("fmul", 2, CorePrimOp::FMul),
            ("fsub", 2, CorePrimOp::FSub),
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
            ("join", 2, CorePrimOp::Join),
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
            ("parse_ints", 1, CorePrimOp::ParseInts),
            ("print", 1, CorePrimOp::Print),
            ("println", 1, CorePrimOp::Println),
            ("read_file", 1, CorePrimOp::ReadFile),
            ("read_lines", 1, CorePrimOp::ReadLines),
            ("read_stdin", 0, CorePrimOp::ReadStdin),
            ("replace", 3, CorePrimOp::Replace),
            ("safe_div", 2, CorePrimOp::SafeDiv),
            ("safe_mod", 2, CorePrimOp::SafeMod),
            ("split", 2, CorePrimOp::Split),
            ("split_ints", 2, CorePrimOp::SplitInts),
            ("starts_with", 2, CorePrimOp::StartsWith),
            ("str_contains", 2, CorePrimOp::StrContains),
            ("string_concat", 2, CorePrimOp::StringConcat),
            ("string_len", 1, CorePrimOp::StringLength),
            ("string_length", 1, CorePrimOp::StringLength),
            ("string_slice", 3, CorePrimOp::StringSlice),
            ("substring", 3, CorePrimOp::Substring),
            ("time", 0, CorePrimOp::Time),
            ("to_array", 1, CorePrimOp::ToArray),
            ("to_list", 1, CorePrimOp::ToList),
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
            Abs | ArrayLen | Chars | IsArray | IsBool | IsFloat | IsInt | IsList | IsMap
            | IsNone | IsSome | IsString | Len | Lower | Panic | ParseInt | ParseInts | Print
            | Println | ReadFile | ReadLines | StringLength | ToArray | ToList | ToString
            | Trim | Try | AssertThrows | TypeOf | Upper | HamtKeys | HamtValues | HamtSize
            | Neg | Not | ArrayReverse | Sort | Flatten | Unwrap => 1,
            Add | Sub | Mul | Div | Mod | IAdd | ISub | IMul | IDiv | IMod | FAdd | FSub | FMul
            | FDiv | Eq | NEq | Lt | Le | Gt | Ge | ICmpEq | ICmpNe | ICmpLt | ICmpLe | ICmpGt
            | ICmpGe | FCmpEq | FCmpNe | FCmpLt | FCmpLe | FCmpGt | FCmpGe | CmpEq | CmpNe
            | And | Or | Concat | ArrayGet | ArrayPush | ArrayConcat | HamtGet | HamtContains
            | HamtDelete | HamtMerge | Index | Join | Max | Min | Split | SplitInts
            | StartsWith | EndsWith | StringConcat | StrContains | WriteFile | ArrayContains
            | SortBy | HoMap | HoFilter | HoAny | HoAll | HoEach | HoFind | HoCount | HoFlatMap
            | Zip | SafeDiv | SafeMod => 2,
            HoFold => 3,
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

    /// Side-effect classification. Used by the compiler to check that
    /// effectful primops have the required ambient effect in scope.
    pub fn effect_kind(self) -> PrimEffect {
        match self {
            Self::Println
            | Self::ReadFile
            | Self::WriteFile
            | Self::ReadStdin
            | Self::Print
            | Self::ReadLines => PrimEffect::Io,
            Self::ClockNow | Self::Time => PrimEffect::Time,
            Self::Panic => PrimEffect::Control,
            _ => PrimEffect::Pure,
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
            | CoreExpr::LetRecGroup { span, .. }
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
