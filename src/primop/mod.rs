use std::{fs, rc::Rc, time::SystemTime};

use crate::runtime::base::list_ops::format_value;

use crate::runtime::{RuntimeContext, hamt as rc_hamt, value::Value};

/// Primitive operations that can be invoked directly from VM bytecode.
///
/// IDs are encoded in bytecode, so existing discriminants must remain stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PrimOp {
    /// Integer addition: `Int x Int -> Int`.
    IAdd = 0,
    ISub = 1,
    IMul = 2,
    IDiv = 3,
    IMod = 4,
    FAdd = 5,
    FSub = 6,
    FMul = 7,
    FDiv = 8,
    ICmpEq = 9,
    ICmpNe = 10,
    ICmpLt = 11,
    ICmpLe = 12,
    ICmpGt = 13,
    ICmpGe = 14,
    FCmpEq = 15,
    FCmpNe = 16,
    FCmpLt = 17,
    FCmpLe = 18,
    FCmpGt = 19,
    FCmpGe = 20,
    CmpEq = 21,
    CmpNe = 22,
    ArrayLen = 23,
    ArrayGet = 24,
    ArraySet = 25,
    MapGet = 26,
    MapSet = 27,
    MapHas = 28,
    StringLen = 29,
    StringConcat = 30,
    StringSlice = 31,
    Println = 32,
    ReadFile = 33,
    ClockNow = 34,
    Panic = 35,
    Abs = 36,
    Min = 37,
    Max = 38,
    ConcatArray = 39,
    /// Polymorphic length: string/array/list → Int.
    Len = 40,
    /// Array push: (Array, elem) → Array.
    Push = 41,
    /// String split: (String, String) → Array<String>.
    Split = 42,
    /// Type check: is_int(value) → Bool.
    IsInt = 43,
    /// Convert to string: to_string(value) → String.
    ToString = 44,
    /// Write file: write_file(path, content) → None.
    WriteFile = 45,
    /// Read stdin: read_stdin() → String.
    ReadStdin = 46,
    /// String join: join(array, sep) → String.
    Join = 47,
    /// String trim: trim(string) → String.
    Trim = 48,
    /// String starts_with: starts_with(string, prefix) → Bool.
    StartsWith = 49,
    /// String ends_with: ends_with(string, suffix) → Bool.
    EndsWith = 50,
    /// String chars: chars(string) → Array<String>.
    Chars = 51,
    /// Array sort: sort(array) → Array.
    Sort = 52,
    /// Array slice: slice(array, start, end) → Array.
    Slice = 53,
    /// HAMT delete: delete(map, key) → Map.
    MapDelete = 54,
    /// HAMT keys: keys(map) → Array.
    MapKeys = 55,
    /// HAMT values: values(map) → Array.
    MapValues = 56,
    /// HAMT merge: merge(map, map) → Map.
    MapMerge = 57,
    /// HAMT size: size(map) → Int.
    MapSize = 58,
    /// type_of: type_of(value) → String.
    TypeOf = 59,
    /// is_float: is_float(value) → Bool.
    IsFloat = 60,
    /// is_string: is_string(value) → Bool.
    IsString = 61,
    /// is_bool: is_bool(value) → Bool.
    IsBool = 62,
    /// is_array: is_array(value) → Bool.
    IsArray = 63,
    /// is_none: is_none(value) → Bool.
    IsNone = 64,
    /// is_some: is_some_v(value) → Bool.
    IsSomeV = 65,
    /// is_list: is_list(value) → Bool.
    IsList = 66,
    /// is_map/is_hash: is_hash(value) → Bool.
    IsHash = 67,
    /// Cons-list head: hd(list) → value.
    Hd = 68,
    /// Cons-list tail: tl(list) → list.
    Tl = 69,
    /// Array to cons-list: to_list(array) → list.
    ToList = 70,
    /// Cons-list to array: to_array(list) → array.
    ToArray = 71,
    /// Parse int: parse_int(string) → Option<Int>.
    ParseInt = 72,
    /// Print (no newline): print_no_nl(value) → None.
    Print = 73,
    /// Time alias: time() → Int (alias for ClockNow).
    Time = 74,
    /// String replace: replace(str, from, to) → String.
    Replace = 75,
    /// String contains: str_contains(str, sub) → Bool.
    StrContains = 76,
    /// Read lines: read_lines(path) → Array<String>.
    ReadLines = 77,
    /// Parse ints: parse_ints(array) → Array<Int>.
    ParseInts = 78,
    /// Split ints: split_ints(str, sep) → Array<Int>.
    SplitInts = 79,
    /// String upper: upper(string) → String.
    Upper = 80,
    /// String lower: lower(string) → String.
    Lower = 81,
    /// Try: try(closure) → Result (catches runtime errors).
    Try = 82,
    /// Assert throws: assert_throws(closure) → Bool.
    AssertThrows = 83,
}

/// Side-effect classification for primitive operations.
///
/// This is used for optimization/planning decisions where purity matters.
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

impl PrimOp {
    /// Upper bound reserved for bytecode decoding tables.
    pub const COUNT: usize = 84;

    /// Returns the bytecode ID for this primitive op.
    pub fn id(self) -> u8 {
        self as u8
    }

    /// Decodes a bytecode ID into a [`PrimOp`].
    pub fn from_id(id: u8) -> Option<Self> {
        Some(match id {
            0 => Self::IAdd,
            1 => Self::ISub,
            2 => Self::IMul,
            3 => Self::IDiv,
            4 => Self::IMod,
            5 => Self::FAdd,
            6 => Self::FSub,
            7 => Self::FMul,
            8 => Self::FDiv,
            9 => Self::ICmpEq,
            10 => Self::ICmpNe,
            11 => Self::ICmpLt,
            12 => Self::ICmpLe,
            13 => Self::ICmpGt,
            14 => Self::ICmpGe,
            15 => Self::FCmpEq,
            16 => Self::FCmpNe,
            17 => Self::FCmpLt,
            18 => Self::FCmpLe,
            19 => Self::FCmpGt,
            20 => Self::FCmpGe,
            21 => Self::CmpEq,
            22 => Self::CmpNe,
            23 => Self::ArrayLen,
            24 => Self::ArrayGet,
            25 => Self::ArraySet,
            26 => Self::MapGet,
            27 => Self::MapSet,
            28 => Self::MapHas,
            29 => Self::StringLen,
            30 => Self::StringConcat,
            31 => Self::StringSlice,
            32 => Self::Println,
            33 => Self::ReadFile,
            34 => Self::ClockNow,
            35 => Self::Panic,
            36 => Self::Abs,
            37 => Self::Min,
            38 => Self::Max,
            39 => Self::ConcatArray,
            40 => Self::Len,
            41 => Self::Push,
            42 => Self::Split,
            43 => Self::IsInt,
            44 => Self::ToString,
            45 => Self::WriteFile,
            46 => Self::ReadStdin,
            47 => Self::Join,
            48 => Self::Trim,
            49 => Self::StartsWith,
            50 => Self::EndsWith,
            51 => Self::Chars,
            52 => Self::Sort,
            53 => Self::Slice,
            54 => Self::MapDelete,
            55 => Self::MapKeys,
            56 => Self::MapValues,
            57 => Self::MapMerge,
            58 => Self::MapSize,
            59 => Self::TypeOf,
            60 => Self::IsFloat,
            61 => Self::IsString,
            62 => Self::IsBool,
            63 => Self::IsArray,
            64 => Self::IsNone,
            65 => Self::IsSomeV,
            66 => Self::IsList,
            67 => Self::IsHash,
            68 => Self::Hd,
            69 => Self::Tl,
            70 => Self::ToList,
            71 => Self::ToArray,
            72 => Self::ParseInt,
            73 => Self::Print,
            74 => Self::Time,
            75 => Self::Replace,
            76 => Self::StrContains,
            77 => Self::ReadLines,
            78 => Self::ParseInts,
            79 => Self::SplitInts,
            80 => Self::Upper,
            81 => Self::Lower,
            82 => Self::Try,
            83 => Self::AssertThrows,
            _ => return None,
        })
    }

    /// Returns the fixed argument count for this operation.
    pub fn arity(self) -> usize {
        match self {
            Self::ClockNow | Self::ReadStdin | Self::Time => 0,
            Self::ArrayLen
            | Self::StringLen
            | Self::ReadFile
            | Self::Panic
            | Self::Println
            | Self::Abs
            | Self::Len
            | Self::IsInt
            | Self::ToString
            | Self::Trim
            | Self::Chars
            | Self::Sort
            | Self::TypeOf
            | Self::IsFloat
            | Self::IsString
            | Self::IsBool
            | Self::IsArray
            | Self::IsNone
            | Self::IsSomeV
            | Self::IsList
            | Self::IsHash
            | Self::Hd
            | Self::Tl
            | Self::ToList
            | Self::ToArray
            | Self::ParseInt
            | Self::Print
            | Self::ReadLines
            | Self::MapKeys
            | Self::MapValues
            | Self::MapSize
            | Self::ParseInts
            | Self::Upper
            | Self::Lower
            | Self::Try
            | Self::AssertThrows => 1,
            Self::IAdd
            | Self::ISub
            | Self::IMul
            | Self::IDiv
            | Self::IMod
            | Self::FAdd
            | Self::FSub
            | Self::FMul
            | Self::FDiv
            | Self::ICmpEq
            | Self::ICmpNe
            | Self::ICmpLt
            | Self::ICmpLe
            | Self::ICmpGt
            | Self::ICmpGe
            | Self::FCmpEq
            | Self::FCmpNe
            | Self::FCmpLt
            | Self::FCmpLe
            | Self::FCmpGt
            | Self::FCmpGe
            | Self::CmpEq
            | Self::CmpNe
            | Self::ArrayGet
            | Self::MapGet
            | Self::MapHas
            | Self::StringConcat
            | Self::Min
            | Self::Max
            | Self::ConcatArray
            | Self::Push
            | Self::Split
            | Self::Join
            | Self::StartsWith
            | Self::EndsWith
            | Self::MapDelete
            | Self::MapMerge
            | Self::WriteFile
            | Self::StrContains
            | Self::SplitInts => 2,
            Self::ArraySet | Self::MapSet | Self::StringSlice | Self::Slice | Self::Replace => 3,
        }
    }

    /// Returns the effect classification for this primitive operation.
    pub fn effect_kind(self) -> PrimEffect {
        match self {
            Self::Println | Self::ReadFile | Self::WriteFile | Self::ReadStdin | Self::Print
            | Self::ReadLines => PrimEffect::Io,
            Self::ClockNow | Self::Time => PrimEffect::Time,
            Self::Panic => PrimEffect::Control,
            _ => PrimEffect::Pure,
        }
    }

    /// Returns `true` when this operation is deterministic and side-effect free.
    pub fn is_pure(self) -> bool {
        self.effect_kind() == PrimEffect::Pure
    }

    /// Human-readable name used in diagnostics and traces.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::IAdd => "iadd",
            Self::ISub => "isub",
            Self::IMul => "imul",
            Self::IDiv => "idiv",
            Self::IMod => "imod",
            Self::FAdd => "fadd",
            Self::FSub => "fsub",
            Self::FMul => "fmul",
            Self::FDiv => "fdiv",
            Self::ICmpEq => "icmp_eq",
            Self::ICmpNe => "icmp_ne",
            Self::ICmpLt => "icmp_lt",
            Self::ICmpLe => "icmp_le",
            Self::ICmpGt => "icmp_gt",
            Self::ICmpGe => "icmp_ge",
            Self::FCmpEq => "fcmp_eq",
            Self::FCmpNe => "fcmp_ne",
            Self::FCmpLt => "fcmp_lt",
            Self::FCmpLe => "fcmp_le",
            Self::FCmpGt => "fcmp_gt",
            Self::FCmpGe => "fcmp_ge",
            Self::CmpEq => "cmp_eq",
            Self::CmpNe => "cmp_ne",
            Self::ArrayLen => "array_len",
            Self::ArrayGet => "array_get",
            Self::ArraySet => "array_set",
            Self::MapGet => "map_get",
            Self::MapSet => "map_set",
            Self::MapHas => "map_has",
            Self::StringLen => "string_len",
            Self::StringConcat => "string_concat",
            Self::StringSlice => "string_slice",
            Self::Println => "println",
            Self::ReadFile => "read_file",
            Self::ClockNow => "clock_now",
            Self::Panic => "panic",
            Self::Abs => "abs",
            Self::Min => "min",
            Self::Max => "max",
            Self::ConcatArray => "concat",
            Self::Len => "len",
            Self::Push => "push",
            Self::Split => "split",
            Self::IsInt => "is_int",
            Self::ToString => "to_string",
            Self::WriteFile => "write_file",
            Self::ReadStdin => "read_stdin",
            Self::Join => "join",
            Self::Trim => "trim",
            Self::StartsWith => "starts_with",
            Self::EndsWith => "ends_with",
            Self::Chars => "chars",
            Self::Sort => "sort",
            Self::Slice => "slice",
            Self::MapDelete => "delete",
            Self::MapKeys => "keys",
            Self::MapValues => "values",
            Self::MapMerge => "merge",
            Self::MapSize => "size",
            Self::TypeOf => "type_of",
            Self::IsFloat => "is_float",
            Self::IsString => "is_string",
            Self::IsBool => "is_bool",
            Self::IsArray => "is_array",
            Self::IsNone => "is_none",
            Self::IsSomeV => "is_some",
            Self::IsList => "is_list",
            Self::IsHash => "is_hash",
            Self::Hd => "hd",
            Self::Tl => "tl",
            Self::ToList => "to_list",
            Self::ToArray => "to_array",
            Self::ParseInt => "parse_int",
            Self::Print => "print",
            Self::Time => "time",
            Self::Replace => "replace",
            Self::StrContains => "str_contains",
            Self::ReadLines => "read_lines",
            Self::ParseInts => "parse_ints",
            Self::SplitInts => "split_ints",
            Self::Upper => "upper",
            Self::Lower => "lower",
            Self::Try => "try",
            Self::AssertThrows => "assert_throws",
        }
    }
}

const _: [(); PrimOp::COUNT] = [(); PrimOp::AssertThrows as usize + 1];

const PRIMOP_CALL_MAPPINGS: &[(&str, usize, PrimOp)] = &[
    ("abs", 1, PrimOp::Abs),
    ("array_get", 2, PrimOp::ArrayGet),
    ("array_len", 1, PrimOp::ArrayLen),
    ("array_set", 3, PrimOp::ArraySet),
    ("assert_throws", 1, PrimOp::AssertThrows),
    ("chars", 1, PrimOp::Chars),
    ("clock_now", 0, PrimOp::ClockNow),
    ("cmp_eq", 2, PrimOp::CmpEq),
    ("cmp_ne", 2, PrimOp::CmpNe),
    ("concat", 2, PrimOp::ConcatArray),
    ("delete", 2, PrimOp::MapDelete),
    ("ends_with", 2, PrimOp::EndsWith),
    ("fadd", 2, PrimOp::FAdd),
    ("fcmp_eq", 2, PrimOp::FCmpEq),
    ("fcmp_ge", 2, PrimOp::FCmpGe),
    ("fcmp_gt", 2, PrimOp::FCmpGt),
    ("fcmp_le", 2, PrimOp::FCmpLe),
    ("fcmp_lt", 2, PrimOp::FCmpLt),
    ("fcmp_ne", 2, PrimOp::FCmpNe),
    ("fdiv", 2, PrimOp::FDiv),
    ("fmul", 2, PrimOp::FMul),
    ("fsub", 2, PrimOp::FSub),
    ("get", 2, PrimOp::MapGet),
    ("has_key", 2, PrimOp::MapHas),
    ("hd", 1, PrimOp::Hd),
    ("iadd", 2, PrimOp::IAdd),
    ("icmp_eq", 2, PrimOp::ICmpEq),
    ("icmp_ge", 2, PrimOp::ICmpGe),
    ("icmp_gt", 2, PrimOp::ICmpGt),
    ("icmp_le", 2, PrimOp::ICmpLe),
    ("icmp_lt", 2, PrimOp::ICmpLt),
    ("icmp_ne", 2, PrimOp::ICmpNe),
    ("idiv", 2, PrimOp::IDiv),
    ("imod", 2, PrimOp::IMod),
    ("imul", 2, PrimOp::IMul),
    ("is_array", 1, PrimOp::IsArray),
    ("is_bool", 1, PrimOp::IsBool),
    ("is_float", 1, PrimOp::IsFloat),
    ("is_hash", 1, PrimOp::IsHash),
    ("is_int", 1, PrimOp::IsInt),
    ("is_list", 1, PrimOp::IsList),
    ("is_map", 1, PrimOp::IsHash),
    ("is_none", 1, PrimOp::IsNone),
    ("is_some", 1, PrimOp::IsSomeV),
    ("is_string", 1, PrimOp::IsString),
    ("isub", 2, PrimOp::ISub),
    ("join", 2, PrimOp::Join),
    ("keys", 1, PrimOp::MapKeys),
    ("len", 1, PrimOp::Len),
    ("lower", 1, PrimOp::Lower),
    ("map_get", 2, PrimOp::MapGet),
    ("map_has", 2, PrimOp::MapHas),
    ("map_set", 3, PrimOp::MapSet),
    ("max", 2, PrimOp::Max),
    ("merge", 2, PrimOp::MapMerge),
    ("min", 2, PrimOp::Min),
    ("now_ms", 0, PrimOp::ClockNow),
    ("panic", 1, PrimOp::Panic),
    ("parse_int", 1, PrimOp::ParseInt),
    ("parse_ints", 1, PrimOp::ParseInts),
    ("print", 1, PrimOp::Print),
    ("println", 1, PrimOp::Println),
    ("push", 2, PrimOp::Push),
    ("put", 3, PrimOp::MapSet),
    ("read_file", 1, PrimOp::ReadFile),
    ("read_lines", 1, PrimOp::ReadLines),
    ("read_stdin", 0, PrimOp::ReadStdin),
    ("replace", 3, PrimOp::Replace),
    ("size", 1, PrimOp::MapSize),
    ("slice", 3, PrimOp::Slice),
    ("sort", 1, PrimOp::Sort),
    ("split", 2, PrimOp::Split),
    ("split_ints", 2, PrimOp::SplitInts),
    ("starts_with", 2, PrimOp::StartsWith),
    ("str_contains", 2, PrimOp::StrContains),
    ("string_concat", 2, PrimOp::StringConcat),
    ("string_len", 1, PrimOp::StringLen),
    ("string_slice", 3, PrimOp::StringSlice),
    ("substring", 3, PrimOp::StringSlice),
    ("time", 0, PrimOp::Time),
    ("tl", 1, PrimOp::Tl),
    ("to_array", 1, PrimOp::ToArray),
    ("to_list", 1, PrimOp::ToList),
    ("to_string", 1, PrimOp::ToString),
    ("trim", 1, PrimOp::Trim),
    ("try", 1, PrimOp::Try),
    ("type_of", 1, PrimOp::TypeOf),
    ("upper", 1, PrimOp::Upper),
    ("values", 1, PrimOp::MapValues),
    ("write_file", 2, PrimOp::WriteFile),
];

pub fn resolve_primop_call(name: &str, arity: usize) -> Option<PrimOp> {
    let key = (name, arity);
    PRIMOP_CALL_MAPPINGS
        .binary_search_by(|(entry_name, entry_arity, _)| (*entry_name, *entry_arity).cmp(&key))
        .ok()
        .map(|idx| PRIMOP_CALL_MAPPINGS[idx].2)
}

/// Executes a primitive operation with VM values.
///
/// Arity is validated here to keep direct-call paths and opcode paths consistent.
pub fn execute_primop(
    ctx: &mut dyn RuntimeContext,
    op: PrimOp,
    args: Vec<Value>,
) -> Result<Value, String> {
    if args.len() != op.arity() {
        return Err(format!(
            "primop {} expects {} arguments, got {}",
            op.display_name(),
            op.arity(),
            args.len()
        ));
    }

    match op {
        PrimOp::IAdd
        | PrimOp::ISub
        | PrimOp::IMul
        | PrimOp::IDiv
        | PrimOp::IMod
        | PrimOp::FAdd
        | PrimOp::FSub
        | PrimOp::FMul
        | PrimOp::FDiv
        | PrimOp::Abs
        | PrimOp::Min
        | PrimOp::Max => execute_numeric_primop(op, args),
        PrimOp::ICmpEq
        | PrimOp::ICmpNe
        | PrimOp::ICmpLt
        | PrimOp::ICmpLe
        | PrimOp::ICmpGt
        | PrimOp::ICmpGe
        | PrimOp::FCmpEq
        | PrimOp::FCmpNe
        | PrimOp::FCmpLt
        | PrimOp::FCmpLe
        | PrimOp::FCmpGt
        | PrimOp::FCmpGe
        | PrimOp::CmpEq
        | PrimOp::CmpNe => execute_compare_primop(op, args),
        PrimOp::ArrayLen | PrimOp::ArrayGet | PrimOp::ArraySet => execute_array_primop(op, args),
        PrimOp::MapGet | PrimOp::MapSet | PrimOp::MapHas => execute_map_primop(ctx, op, args),
        PrimOp::StringLen | PrimOp::StringConcat | PrimOp::StringSlice => {
            execute_string_primop(op, args)
        }
        PrimOp::Println | PrimOp::ReadFile | PrimOp::ClockNow | PrimOp::Panic => {
            execute_effect_primop(ctx, op, args)
        }
        PrimOp::ConcatArray => execute_concat_array_primop(args),
        // New primops — delegate to runtime::base implementations.
        PrimOp::Len
        | PrimOp::Push
        | PrimOp::Split
        | PrimOp::IsInt
        | PrimOp::ToString
        | PrimOp::WriteFile
        | PrimOp::ReadStdin
        | PrimOp::Join
        | PrimOp::Trim
        | PrimOp::StartsWith
        | PrimOp::EndsWith
        | PrimOp::Chars
        | PrimOp::Sort
        | PrimOp::Slice
        | PrimOp::MapDelete
        | PrimOp::MapKeys
        | PrimOp::MapValues
        | PrimOp::MapMerge
        | PrimOp::MapSize
        | PrimOp::TypeOf
        | PrimOp::IsFloat
        | PrimOp::IsString
        | PrimOp::IsBool
        | PrimOp::IsArray
        | PrimOp::IsNone
        | PrimOp::IsSomeV
        | PrimOp::IsList
        | PrimOp::IsHash
        | PrimOp::Hd
        | PrimOp::Tl
        | PrimOp::ToList
        | PrimOp::ToArray
        | PrimOp::ParseInt
        | PrimOp::Print
        | PrimOp::Time
        | PrimOp::Replace
        | PrimOp::StrContains
        | PrimOp::ReadLines
        | PrimOp::ParseInts
        | PrimOp::SplitInts
        | PrimOp::Upper
        | PrimOp::Lower
        | PrimOp::Try
        | PrimOp::AssertThrows => execute_new_primop(ctx, op, args),
    }
}

/// Executes arithmetic and numeric utility primops.
fn execute_numeric_primop(op: PrimOp, args: Vec<Value>) -> Result<Value, String> {
    match op {
        PrimOp::IAdd => int2(args, |a, b| Value::Integer(a + b), op),
        PrimOp::ISub => int2(args, |a, b| Value::Integer(a - b), op),
        PrimOp::IMul => int2(args, |a, b| Value::Integer(a * b), op),
        PrimOp::IDiv => int2_result(
            args,
            |a, b| {
                if b == 0 {
                    Err("division by zero".to_string())
                } else {
                    Ok(Value::Integer(a / b))
                }
            },
            op,
        ),
        PrimOp::IMod => int2_result(
            args,
            |a, b| {
                if b == 0 {
                    Err("modulo by zero".to_string())
                } else {
                    Ok(Value::Integer(a % b))
                }
            },
            op,
        ),
        PrimOp::FAdd => float2(args, |a, b| Value::Float(a + b), op),
        PrimOp::FSub => float2(args, |a, b| Value::Float(a - b), op),
        PrimOp::FMul => float2(args, |a, b| Value::Float(a * b), op),
        PrimOp::FDiv => float2(args, |a, b| Value::Float(a / b), op),
        PrimOp::Abs => match &args[0] {
            Value::Integer(v) => Ok(Value::Integer(v.abs())),
            Value::Float(v) => Ok(Value::Float(v.abs())),
            other => Err(type_error(op, "Number", other)),
        },
        PrimOp::Min => numeric_mix_max(args, op, true),
        PrimOp::Max => numeric_mix_max(args, op, false),
        _ => dispatch_error("numeric", op),
    }
}

/// Executes comparison primops.
fn execute_compare_primop(op: PrimOp, args: Vec<Value>) -> Result<Value, String> {
    match op {
        PrimOp::ICmpEq => int_cmp(args, |a, b| a == b, op),
        PrimOp::ICmpNe => int_cmp(args, |a, b| a != b, op),
        PrimOp::ICmpLt => int_cmp(args, |a, b| a < b, op),
        PrimOp::ICmpLe => int_cmp(args, |a, b| a <= b, op),
        PrimOp::ICmpGt => int_cmp(args, |a, b| a > b, op),
        PrimOp::ICmpGe => int_cmp(args, |a, b| a >= b, op),
        PrimOp::FCmpEq => float_cmp(args, |a, b| a == b, op),
        PrimOp::FCmpNe => float_cmp(args, |a, b| a != b, op),
        PrimOp::FCmpLt => float_cmp(args, |a, b| a < b, op),
        PrimOp::FCmpLe => float_cmp(args, |a, b| a <= b, op),
        PrimOp::FCmpGt => float_cmp(args, |a, b| a > b, op),
        PrimOp::FCmpGe => float_cmp(args, |a, b| a >= b, op),
        PrimOp::CmpEq => {
            let mut args = args;
            let right = args.pop().expect("arity checked");
            let left = args.pop().expect("arity checked");
            Ok(Value::Boolean(left == right))
        }
        PrimOp::CmpNe => {
            let mut args = args;
            let right = args.pop().expect("arity checked");
            let left = args.pop().expect("arity checked");
            Ok(Value::Boolean(left != right))
        }
        _ => dispatch_error("compare", op),
    }
}

/// Executes array primops.
fn execute_array_primop(op: PrimOp, args: Vec<Value>) -> Result<Value, String> {
    match op {
        PrimOp::ArrayLen => match &args[0] {
            Value::Array(items) => Ok(Value::Integer(items.len() as i64)),
            other => Err(type_error(op, "Array", other)),
        },
        PrimOp::ArrayGet => {
            let mut args = args;
            let index = expect_int(&args.pop().expect("arity checked"), op)?;
            let array = args.pop().expect("arity checked");
            match array {
                Value::Array(items) => {
                    if index < 0 || index as usize >= items.len() {
                        Ok(Value::None)
                    } else {
                        Ok(items[index as usize].clone())
                    }
                }
                other => Err(type_error(op, "Array", &other)),
            }
        }
        PrimOp::ArraySet => {
            let mut args = args;
            let value = args.pop().expect("arity check");
            let index = expect_int(&args.pop().expect("arity checked"), op)?;
            let array = args.pop().expect("arity checked");
            match array {
                Value::Array(mut items) => {
                    if index < 0 || index as usize >= items.len() {
                        return Err(format!(
                            "primop {} index {} out of bounds for length {}",
                            op.display_name(),
                            index,
                            items.len()
                        ));
                    }
                    Rc::make_mut(&mut items)[index as usize] = value;
                    Ok(Value::Array(items))
                }
                other => Err(type_error(op, "Array", &other)),
            }
        }
        _ => dispatch_error("array", op),
    }
}

/// Executes map primops backed by HAMT runtime objects.
fn execute_map_primop(
    _ctx: &mut dyn RuntimeContext,
    op: PrimOp,
    args: Vec<Value>,
) -> Result<Value, String> {
    match op {
        PrimOp::MapGet => {
            let mut args = args;
            let key = args.pop().expect("arity checked");
            let map = args.pop().expect("arity checked");
            let hash = key.to_hash_key().ok_or_else(|| {
                format!(
                    "primop {} expects hashable key (String, Int, Bool), got {}",
                    op.display_name(),
                    key.type_name()
                )
            })?;
            let result = match &map {
                Value::HashMap(node) => rc_hamt::hamt_lookup(node, &hash),
                _ => return Err(type_error(op, "Map", &map)),
            };
            match result {
                Some(value) => Ok(Value::Some(Rc::new(value))),
                None => Ok(Value::None),
            }
        }
        PrimOp::MapSet => {
            let mut args = args;
            let value = args.pop().expect("arity checked");
            let key = args.pop().expect("arity checked");
            let map = args.pop().expect("arity checked");
            let hash = key.to_hash_key().ok_or_else(|| {
                format!(
                    "primop {} expects hashable key (String, Int, Bool), got {}",
                    op.display_name(),
                    key.type_name()
                )
            })?;
            match map {
                Value::HashMap(node) => {
                    Ok(Value::HashMap(rc_hamt::hamt_insert(&node, hash, value)))
                }
                _ => Err(type_error(op, "Map", &map)),
            }
        }
        PrimOp::MapHas => {
            let mut args = args;
            let key = args.pop().expect("arity checked");
            let map = args.pop().expect("arity checked");
            let hash = key.to_hash_key().ok_or_else(|| {
                format!(
                    "primop {} expects hashable key (String, Int, Bool), got {}",
                    op.display_name(),
                    key.type_name()
                )
            })?;
            let found = match &map {
                Value::HashMap(node) => rc_hamt::hamt_lookup(node, &hash).is_some(),
                _ => return Err(type_error(op, "Map", &map)),
            };
            Ok(Value::Boolean(found))
        }
        _ => dispatch_error("map", op),
    }
}

/// Executes string primops.
fn execute_string_primop(op: PrimOp, args: Vec<Value>) -> Result<Value, String> {
    match op {
        PrimOp::StringLen => match &args[0] {
            Value::String(s) => Ok(Value::Integer(s.len() as i64)),
            other => Err(type_error(op, "String", other)),
        },
        PrimOp::StringConcat => {
            let mut args = args;
            let right = args.pop().expect("arity checked");
            let left = args.pop().expect("arity checked");
            match (left, right) {
                (Value::String(l), Value::String(r)) => {
                    Ok(Value::String(format!("{}{}", l, r).into()))
                }
                (l, r) => Err(format!(
                    "primop {} expects (String, String), got ({}, {})",
                    op.display_name(),
                    l.type_name(),
                    r.type_name()
                )),
            }
        }
        PrimOp::StringSlice => {
            let mut args = args;
            let end = expect_int(&args.pop().expect("arity checked"), op)?;
            let start = expect_int(&args.pop().expect("arity checked"), op)?;
            let s = args.pop().expect("arity checked");
            let Value::String(s) = s else {
                return Err(type_error(op, "String", &s));
            };
            let chars: Vec<char> = s.chars().collect();
            let len = chars.len() as i64;
            let start = if start < 0 { 0 } else { start as usize };
            let end = if end < 0 {
                0
            } else if end > len {
                len as usize
            } else {
                end as usize
            };
            if start >= end || start >= chars.len() {
                Ok(Value::String(String::new().into()))
            } else {
                let sliced: String = chars[start..end].iter().collect();
                Ok(Value::String(sliced.into()))
            }
        }
        _ => dispatch_error("string", op),
    }
}

/// Executes effectful primops that perform I/O, time reads, or control effects.
fn execute_effect_primop(
    ctx: &mut dyn RuntimeContext,
    op: PrimOp,
    args: Vec<Value>,
) -> Result<Value, String> {
    match op {
        PrimOp::Println => {
            println!("{}", format_value(ctx, &args[0]));
            Ok(Value::None)
        }
        PrimOp::ReadFile => {
            let path = match &args[0] {
                Value::String(path) => path.as_ref(),
                other => return Err(type_error(op, "String", other)),
            };
            let content = fs::read_to_string(path)
                .map_err(|e| format!("read_file failed for '{}': {}", path, e))?;
            Ok(Value::String(content.into()))
        }
        PrimOp::ClockNow => {
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_err(|e| format!("clock_now failed: {}", e))?;
            Ok(Value::Integer(now.as_millis() as i64))
        }
        PrimOp::Panic => Err(format!("panic: {}", args[0].to_string_value())),
        _ => dispatch_error("effect", op),
    }
}

/// Executes concat as a true primop for `Array + Array`.
///
/// Returns a typed primop error when either argument is not an array.
fn execute_concat_array_primop(args: Vec<Value>) -> Result<Value, String> {
    let left = match &args[0] {
        Value::Array(values) => values,
        other => return Err(type_error(PrimOp::ConcatArray, "Array", other)),
    };
    let right = match &args[1] {
        Value::Array(values) => values,
        other => return Err(type_error(PrimOp::ConcatArray, "Array", other)),
    };

    let mut out = left.clone();
    Rc::make_mut(&mut out).extend(right.iter().cloned());
    Ok(Value::Array(out))
}

/// Dispatches new primop variants to their runtime::base function implementations.
fn execute_new_primop(
    ctx: &mut dyn RuntimeContext,
    op: PrimOp,
    args: Vec<Value>,
) -> Result<Value, String> {
    use crate::runtime::base::get_base_function;
    let name = op.display_name();
    if let Some(base_fn) = get_base_function(name) {
        base_fn.call_owned(ctx, args)
    } else {
        Err(format!(
            "primop {} has no base function implementation",
            name
        ))
    }
}

/// Helper for binary integer primops.
fn int2<F>(args: Vec<Value>, f: F, op: PrimOp) -> Result<Value, String>
where
    F: FnOnce(i64, i64) -> Value,
{
    let mut args = args;
    let right = expect_int(&args.pop().expect("arity checked"), op)?;
    let left = expect_int(&args.pop().expect("arity checked"), op)?;
    Ok(f(left, right))
}

/// Executes a binary integer primop whose callback can fail.
fn int2_result<F>(args: Vec<Value>, f: F, op: PrimOp) -> Result<Value, String>
where
    F: FnOnce(i64, i64) -> Result<Value, String>,
{
    let mut args = args;
    let right = expect_int(&args.pop().expect("arity_checked"), op)?;
    let left = expect_int(&args.pop().expect("arity_checked"), op)?;
    f(left, right)
}

/// Executes a binary float primop after validating both operands.
fn float2<F>(args: Vec<Value>, f: F, op: PrimOp) -> Result<Value, String>
where
    F: FnOnce(f64, f64) -> Value,
{
    let mut args = args;
    let right = expect_float(&args.pop().expect("arity checked"), op)?;
    let left = expect_float(&args.pop().expect("arity checked"), op)?;
    Ok(f(left, right))
}

/// Executes a binary integer comparison and wraps the result in `Value::Boolean`.
fn int_cmp<F>(args: Vec<Value>, f: F, op: PrimOp) -> Result<Value, String>
where
    F: FnOnce(i64, i64) -> bool,
{
    let mut args = args;
    let right = expect_int(&args.pop().expect("arity_checked"), op)?;
    let left = expect_int(&args.pop().expect("arity_checked"), op)?;
    Ok(Value::Boolean(f(left, right)))
}

/// Executes a binary float comparison and wraps the result in `Value::Boolean`.
fn float_cmp<F>(args: Vec<Value>, f: F, op: PrimOp) -> Result<Value, String>
where
    F: FnOnce(f64, f64) -> bool,
{
    let mut args = args;
    let right = expect_float(&args.pop().expect("arity_checked"), op)?;
    let left = expect_float(&args.pop().expect("arity_checked"), op)?;
    Ok(Value::Boolean(f(left, right)))
}

/// Extracts an integer operand or produces a typed primop error.
fn expect_int(value: &Value, op: PrimOp) -> Result<i64, String> {
    match value {
        Value::Integer(v) => Ok(*v),
        other => Err(type_error(op, "Int", other)),
    }
}

/// Extracts a float operand or produces a typed primop error.
fn expect_float(value: &Value, op: PrimOp) -> Result<f64, String> {
    match value {
        Value::Float(v) => Ok(*v),
        other => Err(type_error(op, "Float", other)),
    }
}

/// Shared implementation for mixed numeric `min` and `max`.
///
/// Preserves integer return type when both operands are integers; otherwise returns float.
fn numeric_mix_max(args: Vec<Value>, op: PrimOp, is_min: bool) -> Result<Value, String> {
    let mut args = args;
    let b = args.pop().expect("arity checked");
    let a = args.pop().expect("arity checked");

    let (a_num, b_num) = match (&a, &b) {
        (Value::Integer(x), Value::Integer(y)) => (*x as f64, *y as f64),
        (Value::Integer(x), Value::Float(y)) => (*x as f64, *y),
        (Value::Float(x), Value::Integer(y)) => (*x, *y as f64),
        (Value::Float(x), Value::Float(y)) => (*x, *y),
        (left, right) => {
            return Err(format!(
                "primop {} expects (Number, Number), got ({}, {})",
                op.display_name(),
                left.type_name(),
                right.type_name()
            ));
        }
    };

    let result = if is_min {
        a_num.min(b_num)
    } else {
        a_num.max(b_num)
    };

    match (&a, &b) {
        (Value::Integer(_), Value::Integer(_)) => Ok(Value::Integer(result as i64)),
        _ => Ok(Value::Float(result)),
    }
}

/// Standardized type-mismatch diagnostic for primops.
fn type_error(op: PrimOp, expected: &str, got: &Value) -> String {
    format!(
        "primop {} expected {}, got {}",
        op.display_name(),
        expected,
        got.type_name()
    )
}

/// Produces a standardized internal-dispatch error for unreachable group branches.
fn dispatch_error(group: &str, op: PrimOp) -> Result<Value, String> {
    Err(format!(
        "internal primop dispatch error in {} group for {}",
        group,
        op.display_name()
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::runtime::{hamt as rc_hamt_mod, hash_key::HashKey};

    struct TestRuntimeContext;

    impl TestRuntimeContext {
        fn new() -> Self {
            Self
        }
    }

    impl RuntimeContext for TestRuntimeContext {
        fn invoke_value(&mut self, _callee: Value, _args: Vec<Value>) -> Result<Value, String> {
            Err("invoke_value is not used by these primop tests".to_string())
        }

        fn invoke_base_function_borrowed(
            &mut self,
            _base_fn_index: usize,
            _args: &[&Value],
        ) -> Result<Value, String> {
            Err("borrowed base dispatch is not used by these primop tests".to_string())
        }
    }

    #[test]
    fn primop_id_roundtrip_for_all_known_ids() {
        for id in 0..PrimOp::COUNT as u8 {
            let op = PrimOp::from_id(id).expect("primop id should decode");
            assert_eq!(op.id(), id);
        }
        assert!(PrimOp::from_id(PrimOp::COUNT as u8).is_none());
    }

    #[test]
    fn primop_effect_classification_is_consistent() {
        assert_eq!(PrimOp::Println.effect_kind(), PrimEffect::Io);
        assert_eq!(PrimOp::ReadFile.effect_kind(), PrimEffect::Io);
        assert_eq!(PrimOp::ClockNow.effect_kind(), PrimEffect::Time);
        assert_eq!(PrimOp::Panic.effect_kind(), PrimEffect::Control);
        assert!(PrimOp::IAdd.is_pure());
    }

    #[test]
    fn execute_iadd_returns_integer_sum() {
        let mut ctx = TestRuntimeContext::new();
        let result = execute_primop(
            &mut ctx,
            PrimOp::IAdd,
            vec![Value::Integer(2), Value::Integer(40)],
        )
        .expect("iadd should succeed");
        assert_eq!(result, Value::Integer(42));
    }

    #[test]
    fn execute_idiv_by_zero_returns_error() {
        let mut ctx = TestRuntimeContext::new();
        let err = execute_primop(
            &mut ctx,
            PrimOp::IDiv,
            vec![Value::Integer(42), Value::Integer(0)],
        )
        .expect_err("idiv by zero should fail");
        assert!(err.contains("division by zero"));
    }

    #[test]
    fn execute_array_get_out_of_bounds_returns_none() {
        let mut ctx = TestRuntimeContext::new();
        let arr = Value::Array(Rc::new(vec![Value::Integer(1), Value::Integer(2)]));
        let result = execute_primop(&mut ctx, PrimOp::ArrayGet, vec![arr, Value::Integer(10)])
            .expect("array_get should succeed");
        assert_eq!(result, Value::None);
    }

    #[test]
    fn execute_string_concat_returns_joined_string() {
        let mut ctx = TestRuntimeContext::new();
        let result = execute_primop(
            &mut ctx,
            PrimOp::StringConcat,
            vec![
                Value::String("Flux ".to_string().into()),
                Value::String("Lang".to_string().into()),
            ],
        )
        .expect("string_concat should succeed");

        assert_eq!(result, Value::String("Flux Lang".to_string().into()));
    }

    #[test]
    fn execute_string_slice_returns_sliced_string() {
        let mut ctx = TestRuntimeContext::new();
        let result = execute_primop(
            &mut ctx,
            PrimOp::StringSlice,
            vec![
                Value::String("Hello World".to_string().into()),
                Value::Integer(0),
                Value::Integer(2),
            ],
        )
        .expect("string_slice should succeed");

        assert_eq!(result, Value::String("He".to_string().into()))
    }

    #[test]
    fn execute_map_has_rejects_non_map_receiver() {
        let mut ctx = TestRuntimeContext::new();
        let err = execute_primop(
            &mut ctx,
            PrimOp::MapHas,
            vec![Value::None, Value::Integer(1)],
        )
        .expect_err("non-map receiver should fail");
        assert!(err.contains("expected Map"));
    }

    fn hamt_value(_ctx: &mut TestRuntimeContext, entries: Vec<(HashKey, Value)>) -> Value {
        let mut root = rc_hamt_mod::hamt_empty();
        for (k, v) in entries {
            root = rc_hamt_mod::hamt_insert(&root, k, v);
        }
        Value::HashMap(root)
    }

    #[test]
    fn execute_map_get_returns_some_for_existing_key() {
        let mut ctx = TestRuntimeContext::new();
        let map = hamt_value(
            &mut ctx,
            vec![(
                HashKey::String("lang".to_string()),
                Value::String("flux".to_string().into()),
            )],
        );

        let result = execute_primop(
            &mut ctx,
            PrimOp::MapGet,
            vec![map, Value::String("lang".to_string().into())],
        )
        .expect("map_get should succeed");

        assert_eq!(
            result,
            Value::Some(Rc::new(Value::String("flux".to_string().into())))
        );
    }

    #[test]
    fn execute_map_get_returns_none_for_missing_key() {
        let mut ctx = TestRuntimeContext::new();
        let map = hamt_value(
            &mut ctx,
            vec![(
                HashKey::String("lang".to_string()),
                Value::String("flux".to_string().into()),
            )],
        );

        let result = execute_primop(
            &mut ctx,
            PrimOp::MapGet,
            vec![map, Value::String("missing".to_string().into())],
        )
        .expect("map_get should succeed");

        assert_eq!(result, Value::None);
    }

    #[test]
    fn execute_map_get_rejects_non_hashable_key() {
        let mut ctx = TestRuntimeContext::new();
        let map = hamt_value(
            &mut ctx,
            vec![(
                HashKey::String("lang".to_string()),
                Value::String("flux".to_string().into()),
            )],
        );

        let err = execute_primop(
            &mut ctx,
            PrimOp::MapGet,
            vec![map, Value::Array(Rc::new(vec![]))],
        )
        .expect_err("non-hashable key should fail");

        assert!(err.contains("expects hashable key"));
    }

    #[test]
    fn execute_println() {
        let mut ctx = TestRuntimeContext::new();

        let result = execute_primop(
            &mut ctx,
            PrimOp::Println,
            vec![Value::String("Hello World".to_string().into())],
        )
        .expect("should println");

        assert_eq!(result, Value::None)
    }

    #[test]
    fn execute_primop_reports_arity_mismatch() {
        let mut ctx = TestRuntimeContext::new();
        let err = execute_primop(&mut ctx, PrimOp::IAdd, vec![Value::Integer(1)])
            .expect_err("arity mismatch should fail");
        assert!(err.contains("expects 2 arguments, got 1"));
    }

    #[test]
    fn execute_map_set_then_get_returns_inserted_value() {
        let mut ctx = TestRuntimeContext::new();
        let map = hamt_value(&mut ctx, vec![]);

        let updated = execute_primop(
            &mut ctx,
            PrimOp::MapSet,
            vec![
                map,
                Value::String("answer".to_string().into()),
                Value::Integer(42),
            ],
        )
        .expect("map_set should succeed");

        let fetched = execute_primop(
            &mut ctx,
            PrimOp::MapGet,
            vec![updated, Value::String("answer".to_string().into())],
        )
        .expect("map_get should succeed");

        assert_eq!(fetched, Value::Some(Rc::new(Value::Integer(42))));
    }

    #[test]
    fn execute_abs_min_max_support_numeric_inputs() {
        let mut ctx = TestRuntimeContext::new();

        let abs = execute_primop(&mut ctx, PrimOp::Abs, vec![Value::Integer(-10)])
            .expect("abs should work");
        assert_eq!(abs, Value::Integer(10));

        let min = execute_primop(
            &mut ctx,
            PrimOp::Min,
            vec![Value::Integer(3), Value::Integer(7)],
        )
        .expect("min should work");
        assert_eq!(min, Value::Integer(3));

        let max = execute_primop(
            &mut ctx,
            PrimOp::Max,
            vec![Value::Integer(3), Value::Float(7.5)],
        )
        .expect("max should work");
        assert_eq!(max, Value::Float(7.5));
    }

    #[test]
    fn resolve_primop_call_extended_mappings_and_concat_array_mapping() {
        assert_eq!(resolve_primop_call("array_len", 1), Some(PrimOp::ArrayLen));
        assert_eq!(resolve_primop_call("get", 2), Some(PrimOp::MapGet));
        assert_eq!(
            resolve_primop_call("string_slice", 3),
            Some(PrimOp::StringSlice)
        );
        assert_eq!(resolve_primop_call("print", 1), Some(PrimOp::Print));
        assert_eq!(resolve_primop_call("concat", 2), Some(PrimOp::ConcatArray));
    }

    #[test]
    fn primop_call_mapping_entries_are_unique_and_arity_consistent() {
        let mut seen = HashSet::new();
        for (name, arity, op) in PRIMOP_CALL_MAPPINGS {
            assert!(
                seen.insert((*name, *arity)),
                "duplicate primop resolver entry"
            );
            assert_eq!(*arity, op.arity(), "resolver arity mismatch for {}", name);
        }
    }

    #[test]
    fn primop_call_mappings_are_sorted_for_binary_search() {
        assert!(
            PRIMOP_CALL_MAPPINGS.windows(2).all(|pair| {
                let left = (pair[0].0, pair[0].1);
                let right = (pair[1].0, pair[1].1);
                left <= right
            }),
            "PRIMOP_CALL_MAPPINGS must remain sorted by (name, arity)"
        );
    }

    #[test]
    fn string_len_uses_utf8_byte_length_for_non_ascii() {
        let mut ctx = TestRuntimeContext::new();
        let input = Value::String("é".to_string().into());

        let string_len = execute_primop(&mut ctx, PrimOp::StringLen, vec![input.clone()])
            .expect("string_len should work");
        assert_eq!(string_len, Value::Integer(2));
    }

    #[test]
    fn execute_map_primop_ops_work() {
        let mut ctx = TestRuntimeContext::new();
        let map = hamt_value(
            &mut ctx,
            vec![(HashKey::String("a".to_string()), Value::Integer(1))],
        );
        let fetched = execute_primop(
            &mut ctx,
            PrimOp::MapGet,
            vec![map, Value::String("a".to_string().into())],
        )
        .expect("map_get works");
        assert_eq!(fetched, Value::Some(Rc::new(Value::Integer(1))));
    }

    #[test]
    fn promoted_names_resolve_to_primops() {
        let promoted = [
            ("len", 1, PrimOp::Len),
            ("type_of", 1, PrimOp::TypeOf),
            ("is_int", 1, PrimOp::IsInt),
            ("to_string", 1, PrimOp::ToString),
            ("trim", 1, PrimOp::Trim),
            ("keys", 1, PrimOp::MapKeys),
            ("delete", 2, PrimOp::MapDelete),
            ("parse_int", 1, PrimOp::ParseInt),
            ("split_ints", 2, PrimOp::SplitInts),
        ];
        for (name, arity, expected) in promoted {
            assert_eq!(
                resolve_primop_call(name, arity),
                Some(expected),
                "expected {name}/{arity} to resolve to primop"
            );
        }
    }

    #[test]
    fn string_len_rejects_non_string_input() {
        let mut ctx = TestRuntimeContext::new();
        let err = execute_primop(&mut ctx, PrimOp::StringLen, vec![Value::Integer(1)])
            .expect_err("string_len should reject non-string");
        assert!(err.contains("expected String"));
    }

    #[test]
    fn map_has_returns_false_for_missing_key() {
        let mut ctx = TestRuntimeContext::new();
        let map = hamt_value(
            &mut ctx,
            vec![(HashKey::String("a".to_string()), Value::Integer(1))],
        );
        let fetched = execute_primop(
            &mut ctx,
            PrimOp::MapHas,
            vec![map, Value::String("missing".to_string().into())],
        )
        .expect("map_has works");
        assert_eq!(fetched, Value::Boolean(false));
    }

    #[test]
    fn execute_concat_array_fast_path_and_type_errors() {
        let mut ctx = TestRuntimeContext::new();

        let joined = execute_primop(
            &mut ctx,
            PrimOp::ConcatArray,
            vec![
                Value::Array(vec![Value::Integer(1), Value::Integer(2)].into()),
                Value::Array(vec![Value::Integer(3)].into()),
            ],
        )
        .expect("concat fast path should work");
        assert_eq!(
            joined,
            Value::Array(vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)].into())
        );

        let err = execute_primop(
            &mut ctx,
            PrimOp::ConcatArray,
            vec![
                Value::Integer(1),
                Value::Array(vec![Value::Integer(2)].into()),
            ],
        )
        .expect_err("concat should reject non-array arguments");
        assert_eq!(err, "primop concat expected Array, got Int");
    }
}
