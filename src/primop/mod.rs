use std::{fs, rc::Rc, time::SystemTime};

use crate::runtime::{
    RuntimeContext,
    gc::{
        GcHandle, HeapObject,
        hamt::{hamt_delete, hamt_insert, hamt_iter, hamt_len, hamt_lookup, is_hamt},
    },
    value::Value,
};

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
    Len = 36,
    Abs = 37,
    Min = 38,
    Max = 39,
    TypeOf = 40,
    IsInt = 41,
    IsFloat = 42,
    IsString = 43,
    IsBool = 44,
    IsArray = 45,
    IsHash = 46,
    IsNone = 47,
    IsSome = 48,
    ToString = 49,
    First = 50,
    Last = 51,
    Rest = 52,
    Contains = 53,
    Slice = 54,
    Trim = 55,
    Upper = 56,
    Lower = 57,
    StartsWith = 58,
    EndsWith = 59,
    Replace = 60,
    Chars = 61,
    Keys = 62,
    Values = 63,
    Delete = 64,
    Merge = 65,
    IsMap = 66,
    ParseInt = 67,
    ParseInts = 68,
    SplitInts = 69,
    ConcatArray = 70,
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
    pub const COUNT: usize = 71;

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
            36 => Self::Len,
            37 => Self::Abs,
            38 => Self::Min,
            39 => Self::Max,
            40 => Self::TypeOf,
            41 => Self::IsInt,
            42 => Self::IsFloat,
            43 => Self::IsString,
            44 => Self::IsBool,
            45 => Self::IsArray,
            46 => Self::IsHash,
            47 => Self::IsNone,
            48 => Self::IsSome,
            49 => Self::ToString,
            50 => Self::First,
            51 => Self::Last,
            52 => Self::Rest,
            53 => Self::Contains,
            54 => Self::Slice,
            55 => Self::Trim,
            56 => Self::Upper,
            57 => Self::Lower,
            58 => Self::StartsWith,
            59 => Self::EndsWith,
            60 => Self::Replace,
            61 => Self::Chars,
            62 => Self::Keys,
            63 => Self::Values,
            64 => Self::Delete,
            65 => Self::Merge,
            66 => Self::IsMap,
            67 => Self::ParseInt,
            68 => Self::ParseInts,
            69 => Self::SplitInts,
            70 => Self::ConcatArray,
            _ => return None,
        })
    }

    /// Returns the fixed argument count for this operation.
    pub fn arity(self) -> usize {
        match self {
            Self::ClockNow => 0,
            Self::ArrayLen
            | Self::StringLen
            | Self::ReadFile
            | Self::Panic
            | Self::Println
            | Self::Len
            | Self::Abs
            | Self::TypeOf
            | Self::IsInt
            | Self::IsFloat
            | Self::IsString
            | Self::IsBool
            | Self::IsArray
            | Self::IsHash
            | Self::IsNone
            | Self::IsSome
            | Self::ToString
            | Self::First
            | Self::Last
            | Self::Rest
            | Self::Trim
            | Self::Upper
            | Self::Lower
            | Self::Chars
            | Self::Keys
            | Self::Values
            | Self::IsMap
            | Self::ParseInt
            | Self::ParseInts => 1,
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
            | Self::Contains
            | Self::StartsWith
            | Self::EndsWith
            | Self::ConcatArray
            | Self::Delete
            | Self::Merge
            | Self::SplitInts => 2,
            Self::ArraySet | Self::MapSet | Self::StringSlice | Self::Slice | Self::Replace => 3,
        }
    }

    /// Returns the effect classification for this primitive operation.
    pub fn effect_kind(self) -> PrimEffect {
        match self {
            Self::Println | Self::ReadFile => PrimEffect::Io,
            Self::ClockNow => PrimEffect::Time,
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
            Self::Len => "len",
            Self::Abs => "abs",
            Self::Min => "min",
            Self::Max => "max",
            Self::TypeOf => "type_of",
            Self::IsInt => "is_int",
            Self::IsFloat => "is_float",
            Self::IsString => "is_string",
            Self::IsBool => "is_bool",
            Self::IsArray => "is_array",
            Self::IsHash => "is_hash",
            Self::IsNone => "is_none",
            Self::IsSome => "is_some",
            Self::ToString => "to_string",
            Self::First => "first",
            Self::Last => "last",
            Self::Rest => "rest",
            Self::Contains => "contains",
            Self::Slice => "slice",
            Self::Trim => "trim",
            Self::Upper => "upper",
            Self::Lower => "lower",
            Self::StartsWith => "starts_with",
            Self::EndsWith => "ends_with",
            Self::Replace => "replace",
            Self::Chars => "chars",
            Self::Keys => "keys",
            Self::Values => "values",
            Self::Delete => "delete",
            Self::Merge => "merge",
            Self::IsMap => "is_map",
            Self::ParseInt => "parse_int",
            Self::ParseInts => "parse_ints",
            Self::SplitInts => "split_ints",
            Self::ConcatArray => "concat",
        }
    }
}

pub fn resolve_primop_call(name: &str, arity: usize) -> Option<PrimOp> {
    match (name, arity) {
        ("array_len", 1) => Some(PrimOp::ArrayLen),
        ("array_get", 2) => Some(PrimOp::ArrayGet),
        ("array_set", 3) => Some(PrimOp::ArraySet),
        ("get", 2) | ("map_get", 2) => Some(PrimOp::MapGet),
        ("put", 3) | ("map_set", 3) => Some(PrimOp::MapSet),
        ("has_key", 2) | ("map_has", 2) => Some(PrimOp::MapHas),
        ("string_len", 1) => Some(PrimOp::StringLen),
        ("string_concat", 2) => Some(PrimOp::StringConcat),
        ("substring", 3) | ("string_slice", 3) => Some(PrimOp::StringSlice),
        ("print", 1) | ("println", 1) => Some(PrimOp::Println),
        ("read_file", 1) => Some(PrimOp::ReadFile),
        ("now_ms", 0) | ("clock_now", 0) => Some(PrimOp::ClockNow),
        ("panic", 1) => Some(PrimOp::Panic),
        ("len", 1) => Some(PrimOp::Len),
        ("abs", 1) => Some(PrimOp::Abs),
        ("min", 2) => Some(PrimOp::Min),
        ("max", 2) => Some(PrimOp::Max),
        ("type_of", 1) => Some(PrimOp::TypeOf),
        ("is_int", 1) => Some(PrimOp::IsInt),
        ("is_float", 1) => Some(PrimOp::IsFloat),
        ("is_string", 1) => Some(PrimOp::IsString),
        ("is_bool", 1) => Some(PrimOp::IsBool),
        ("is_array", 1) => Some(PrimOp::IsArray),
        ("is_hash", 1) => Some(PrimOp::IsHash),
        ("is_none", 1) => Some(PrimOp::IsNone),
        ("is_some", 1) => Some(PrimOp::IsSome),
        ("to_string", 1) => Some(PrimOp::ToString),
        ("first", 1) => Some(PrimOp::First),
        ("last", 1) => Some(PrimOp::Last),
        ("rest", 1) => Some(PrimOp::Rest),
        ("contains", 2) => Some(PrimOp::Contains),
        ("slice", 3) => Some(PrimOp::Slice),
        ("trim", 1) => Some(PrimOp::Trim),
        ("upper", 1) => Some(PrimOp::Upper),
        ("lower", 1) => Some(PrimOp::Lower),
        ("starts_with", 2) => Some(PrimOp::StartsWith),
        ("ends_with", 2) => Some(PrimOp::EndsWith),
        ("replace", 3) => Some(PrimOp::Replace),
        ("chars", 1) => Some(PrimOp::Chars),
        ("concat", 2) => Some(PrimOp::ConcatArray),
        ("keys", 1) => Some(PrimOp::Keys),
        ("values", 1) => Some(PrimOp::Values),
        ("delete", 2) => Some(PrimOp::Delete),
        ("merge", 2) => Some(PrimOp::Merge),
        ("is_map", 1) => Some(PrimOp::IsMap),
        ("parse_int", 1) => Some(PrimOp::ParseInt),
        ("parse_ints", 1) => Some(PrimOp::ParseInts),
        ("split_ints", 2) => Some(PrimOp::SplitInts),
        ("iadd", 2) => Some(PrimOp::IAdd),
        ("isub", 2) => Some(PrimOp::ISub),
        ("imul", 2) => Some(PrimOp::IMul),
        ("idiv", 2) => Some(PrimOp::IDiv),
        ("imod", 2) => Some(PrimOp::IMod),
        ("fadd", 2) => Some(PrimOp::FAdd),
        ("fsub", 2) => Some(PrimOp::FSub),
        ("fmul", 2) => Some(PrimOp::FMul),
        ("fdiv", 2) => Some(PrimOp::FDiv),
        ("icmp_eq", 2) => Some(PrimOp::ICmpEq),
        ("icmp_ne", 2) => Some(PrimOp::ICmpNe),
        ("icmp_lt", 2) => Some(PrimOp::ICmpLt),
        ("icmp_le", 2) => Some(PrimOp::ICmpLe),
        ("icmp_gt", 2) => Some(PrimOp::ICmpGt),
        ("icmp_ge", 2) => Some(PrimOp::ICmpGe),
        ("fcmp_eq", 2) => Some(PrimOp::FCmpEq),
        ("fcmp_ne", 2) => Some(PrimOp::FCmpNe),
        ("fcmp_lt", 2) => Some(PrimOp::FCmpLt),
        ("fcmp_le", 2) => Some(PrimOp::FCmpLe),
        ("fcmp_gt", 2) => Some(PrimOp::FCmpGt),
        ("fcmp_ge", 2) => Some(PrimOp::FCmpGe),
        ("cmp_eq", 2) => Some(PrimOp::CmpEq),
        ("cmp_ne", 2) => Some(PrimOp::CmpNe),
        _ => None,
    }
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
            execute_effect_primop(op, args)
        }
        PrimOp::Len
        | PrimOp::TypeOf
        | PrimOp::IsInt
        | PrimOp::IsFloat
        | PrimOp::IsString
        | PrimOp::IsBool
        | PrimOp::IsArray
        | PrimOp::IsHash
        | PrimOp::IsNone
        | PrimOp::IsSome
        | PrimOp::ToString => execute_builtin_compat_primop(ctx, op, args),
        PrimOp::First
        | PrimOp::Last
        | PrimOp::Rest
        | PrimOp::Contains
        | PrimOp::Slice
        | PrimOp::ConcatArray => execute_collection_primop(ctx, op, args),
        PrimOp::Trim
        | PrimOp::Upper
        | PrimOp::Lower
        | PrimOp::StartsWith
        | PrimOp::EndsWith
        | PrimOp::Replace
        | PrimOp::Chars => execute_string_ops_primop(op, args),
        PrimOp::Keys
        | PrimOp::Values
        | PrimOp::Delete
        | PrimOp::Merge
        | PrimOp::IsMap => execute_map_primop_extended(ctx, op, args),
        PrimOp::ParseInt | PrimOp::ParseInts | PrimOp::SplitInts => {
            execute_parse_primop(op, args)
        }
    }
}

/// Executes collection-oriented primops over arrays and cons-lists.
///
/// Handles `first`, `last`, `rest`, `contains`, `slice`, and `concat`.
fn execute_collection_primop(
    ctx: &mut dyn RuntimeContext,
    op: PrimOp,
    args: Vec<Value>,
) -> Result<Value, String> {
    match op {
        PrimOp::First => match &args[0] {
            Value::Array(arr) => Ok(arr.first().cloned().unwrap_or(Value::None)),
            Value::None | Value::EmptyList => Ok(Value::None),
            Value::Gc(h) => match ctx.gc_heap().get(*h) {
                HeapObject::Cons { head, .. } => Ok(head.clone()),
                _ => Err(type_error(op, "Array or List", &args[0])),
            },
            other => Err(type_error(op, "Array or List", other)),
        },
        PrimOp::Last => match &args[0] {
            Value::Array(arr) => Ok(arr.last().cloned().unwrap_or(Value::None)),
            Value::None | Value::EmptyList => Ok(Value::None),
            Value::Gc(h) => match ctx.gc_heap().get(*h) {
                HeapObject::Cons { .. } => {
                    let elems = collect_list_values(ctx, &args[0])
                        .ok_or_else(|| "last: malformed list".to_string())?;
                    Ok(elems.into_iter().last().unwrap_or(Value::None))
                }
                _ => Err(type_error(op, "Array or List", &args[0])),
            },
            other => Err(type_error(op, "Array or List", other)),
        },
        PrimOp::Rest => match &args[0] {
            Value::Array(arr) => {
                if arr.is_empty() {
                    Ok(Value::None)
                } else {
                    Ok(Value::Array(arr[1..].to_vec().into()))
                }
            }
            Value::None | Value::EmptyList => Ok(Value::None),
            Value::Gc(h) => match ctx.gc_heap().get(*h) {
                HeapObject::Cons { tail, .. } => Ok(tail.clone()),
                _ => Err(type_error(op, "Array or List", &args[0])),
            },
            other => Err(type_error(op, "Array or List", other)),
        },
        PrimOp::Contains => {
            let needle = &args[1];
            match &args[0] {
                Value::Array(arr) => Ok(Value::Boolean(arr.iter().any(|item| item == needle))),
                Value::None | Value::EmptyList => Ok(Value::Boolean(false)),
                Value::Gc(h) => match ctx.gc_heap().get(*h) {
                    HeapObject::Cons { .. } => {
                        let elems = collect_list_values(ctx, &args[0])
                            .ok_or_else(|| "contains: malformed list".to_string())?;
                        Ok(Value::Boolean(elems.iter().any(|item| item == needle)))
                    }
                    _ => Err(type_error(op, "Array or List", &args[0])),
                },
                other => Err(type_error(op, "Array or List", other)),
            }
        }
        PrimOp::Slice => {
            let arr = match &args[0] {
                Value::Array(arr) => arr,
                other => return Err(type_error(op, "Array", other)),
            };
            let start = expect_int(&args[1], op)?;
            let end = expect_int(&args[2], op)?;
            let len = arr.len() as i64;
            let start = if start < 0 { 0 } else { start as usize };
            let end = if end > len {
                len as usize
            } else {
                end as usize
            };
            if start >= end || start >= arr.len() {
                Ok(Value::Array(vec![].into()))
            } else {
                Ok(Value::Array(arr[start..end].to_vec().into()))
            }
        }
        PrimOp::ConcatArray => execute_concat_array_primop(ctx, args),
        _ => dispatch_error("collection", op),
    }
}

/// Executes string utility primops that extend the core string operation set.
///
/// Handles `trim`, `upper`, `lower`, `starts_with`, `ends_with`, `replace`, and `chars`.
fn execute_string_ops_primop(op: PrimOp, args: Vec<Value>) -> Result<Value, String> {
    match op {
        PrimOp::Trim => {
            let s = expect_string(&args[0], op)?;
            Ok(Value::String(s.trim().to_string().into()))
        }
        PrimOp::Upper => {
            let s = expect_string(&args[0], op)?;
            Ok(Value::String(s.to_uppercase().into()))
        }
        PrimOp::Lower => {
            let s = expect_string(&args[0], op)?;
            Ok(Value::String(s.to_lowercase().into()))
        }
        PrimOp::StartsWith => {
            let s = expect_string(&args[0], op)?;
            let prefix = expect_string(&args[1], op)?;
            Ok(Value::Boolean(s.starts_with(prefix)))
        }
        PrimOp::EndsWith => {
            let s = expect_string(&args[0], op)?;
            let suffix = expect_string(&args[1], op)?;
            Ok(Value::Boolean(s.ends_with(suffix)))
        }
        PrimOp::Replace => {
            let s = expect_string(&args[0], op)?;
            let from = expect_string(&args[1], op)?;
            let to = expect_string(&args[2], op)?;
            Ok(Value::String(s.replace(from, to).into()))
        }
        PrimOp::Chars => {
            let s = expect_string(&args[0], op)?;
            let chars: Vec<Value> = s
                .chars()
                .map(|c| Value::String(c.to_string().into()))
                .collect();
            Ok(Value::Array(chars.into()))
        }
        _ => dispatch_error("string-extended", op),
    }
}

/// Executes map utility primops built on HAMT iteration/update helpers.
///
/// Handles `keys`, `values`, `delete`, `merge`, and `is_map`.
fn execute_map_primop_extended(
    ctx: &mut dyn RuntimeContext,
    op: PrimOp,
    args: Vec<Value>,
) -> Result<Value, String> {
    match op {
        PrimOp::Keys => {
            let handle = expect_hamt_handle(ctx, &args[0], op)?;
            let pairs = hamt_iter(ctx.gc_heap(), handle);
            let keys: Vec<Value> = pairs
                .iter()
                .map(|(k, _)| match k {
                    crate::runtime::hash_key::HashKey::Integer(v) => Value::Integer(*v),
                    crate::runtime::hash_key::HashKey::Boolean(v) => Value::Boolean(*v),
                    crate::runtime::hash_key::HashKey::String(v) => Value::String(v.clone().into()),
                })
                .collect();
            Ok(Value::Array(keys.into()))
        }
        PrimOp::Values => {
            let handle = expect_hamt_handle(ctx, &args[0], op)?;
            let pairs = hamt_iter(ctx.gc_heap(), handle);
            let values: Vec<Value> = pairs.into_iter().map(|(_, v)| v).collect();
            Ok(Value::Array(values.into()))
        }
        PrimOp::Delete => {
            let handle = expect_hamt_handle(ctx, &args[0], op)?;
            let key = args[1].to_hash_key().ok_or_else(|| {
                format!(
                    "primop {} expects hashable key (String, Int, Bool), got {}",
                    op.display_name(),
                    args[1].type_name()
                )
            })?;
            Ok(Value::Gc(hamt_delete(ctx.gc_heap_mut(), handle, &key)))
        }
        PrimOp::Merge => {
            let h1 = expect_hamt_handle(ctx, &args[0], op)?;
            let h2 = expect_hamt_handle(ctx, &args[1], op)?;
            let pairs = hamt_iter(ctx.gc_heap(), h2);
            let mut result = h1;
            for (k, v) in pairs {
                result = hamt_insert(ctx.gc_heap_mut(), result, k, v);
            }
            Ok(Value::Gc(result))
        }
        PrimOp::IsMap => {
            let result = matches!(&args[0], Value::Gc(h) if is_hamt(ctx.gc_heap(), *h));
            Ok(Value::Boolean(result))
        }
        _ => dispatch_error("map-extended", op),
    }
}

/// Executes parse-related primops for integer conversion helpers.
///
/// Handles `parse_int`, `parse_ints`, and `split_ints`.
fn execute_parse_primop(op: PrimOp, args: Vec<Value>) -> Result<Value, String> {
    match op {
        PrimOp::ParseInt => {
            let text = expect_string(&args[0], op)?;
            let parsed = text
                .trim()
                .parse::<i64>()
                .map_err(|_| format!("parse_int: could not parse '{}' as Int", text))?;
            Ok(Value::Integer(parsed))
        }
        PrimOp::ParseInts => {
            let lines = match &args[0] {
                Value::Array(lines) => lines,
                other => return Err(type_error(op, "Array", other)),
            };
            let mut out = Vec::with_capacity(lines.len());
            for value in lines.iter() {
                match value {
                    Value::String(s) => {
                        let parsed = s.trim().parse::<i64>().map_err(|_| {
                            format!("parse_ints: could not parse '{}' as Int", s)
                        })?;
                        out.push(Value::Integer(parsed));
                    }
                    other => {
                        return Err(format!(
                            "primop {} expected array elements String, got {}",
                            op.display_name(),
                            other.type_name()
                        ));
                    }
                }
            }
            Ok(Value::Array(out.into()))
        }
        PrimOp::SplitInts => {
            let s = expect_string(&args[0], op)?;
            let delim = expect_string(&args[1], op)?;

            if delim.is_empty() {
                let mut out = Vec::with_capacity(s.chars().count());
                for ch in s.chars() {
                    let text = ch.to_string();
                    let parsed = text
                        .trim()
                        .parse::<i64>()
                        .map_err(|_| format!("split_ints: could not parse '{}' as Int", text))?;
                    out.push(Value::Integer(parsed));
                }
                return Ok(Value::Array(out.into()));
            }

            let mut out = Vec::new();
            for part in s.split(delim) {
                let parsed = part
                    .trim()
                    .parse::<i64>()
                    .map_err(|_| format!("split_ints: could not parse '{}' as Int", part))?;
                out.push(Value::Integer(parsed));
            }
            Ok(Value::Array(out.into()))
        }
        _ => dispatch_error("parse", op),
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
    ctx: &mut dyn RuntimeContext,
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
            let handle = expect_hamt_handle(ctx, &map, op)?;
            match hamt_lookup(ctx.gc_heap(), handle, &hash) {
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
            let handle = expect_hamt_handle(ctx, &map, op)?;
            let updated = hamt_insert(ctx.gc_heap_mut(), handle, hash, value);
            Ok(Value::Gc(updated))
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
            let handle = expect_hamt_handle(ctx, &map, op)?;
            Ok(Value::Boolean(
                hamt_lookup(ctx.gc_heap(), handle, &hash).is_some(),
            ))
        }
        _ => dispatch_error("map", op),
    }
}

/// Executes string primops.
fn execute_string_primop(op: PrimOp, args: Vec<Value>) -> Result<Value, String> {
    match op {
        PrimOp::StringLen => match &args[0] {
            Value::String(s) => Ok(Value::Integer(s.chars().count() as i64)),
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
            let end = if end > len {
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
fn execute_effect_primop(op: PrimOp, args: Vec<Value>) -> Result<Value, String> {
    match op {
        PrimOp::Println => {
            println!("{}", args[0].to_string_value());
            Ok(Value::None)
        }
        PrimOp::ReadFile => {
            let path = expect_string(&args[0], op)?;
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

/// Executes compatibility primops that mirror existing builtin behavior.
fn execute_builtin_compat_primop(
    ctx: &mut dyn RuntimeContext,
    op: PrimOp,
    args: Vec<Value>,
) -> Result<Value, String> {
    match op {
        PrimOp::Len => match &args[0] {
            Value::String(s) => Ok(Value::Integer(s.len() as i64)),
            Value::Array(arr) => Ok(Value::Integer(arr.len() as i64)),
            Value::Tuple(tuple) => Ok(Value::Integer(tuple.len() as i64)),
            Value::None | Value::EmptyList => Ok(Value::Integer(0)),
            Value::Gc(h) => match ctx.gc_heap().get(*h) {
                HeapObject::Cons { .. } => Ok(Value::Integer(cons_len(ctx, &args[0]) as i64)),
                HeapObject::HamtNode { .. } | HeapObject::HamtCollision { .. } => {
                    Ok(Value::Integer(hamt_len(ctx.gc_heap(), *h) as i64))
                }
            },
            other => Err(type_error(op, "String, Array, Tuple, List, or Map", other)),
        },
        PrimOp::TypeOf => {
            let name = match &args[0] {
                Value::Gc(h) => match ctx.gc_heap().get(*h) {
                    HeapObject::Cons { .. } => "List",
                    HeapObject::HamtNode { .. } | HeapObject::HamtCollision { .. } => "Map",
                },
                other => other.type_name(),
            };
            Ok(Value::String(name.to_string().into()))
        }
        PrimOp::IsInt => Ok(Value::Boolean(matches!(args[0], Value::Integer(_)))),
        PrimOp::IsFloat => Ok(Value::Boolean(matches!(args[0], Value::Float(_)))),
        PrimOp::IsString => Ok(Value::Boolean(matches!(args[0], Value::String(_)))),
        PrimOp::IsBool => Ok(Value::Boolean(matches!(args[0], Value::Boolean(_)))),
        PrimOp::IsArray => Ok(Value::Boolean(matches!(args[0], Value::Array(_)))),
        PrimOp::IsHash => {
            let result = match &args[0] {
                Value::Gc(h) => is_hamt(ctx.gc_heap(), *h),
                _ => false,
            };
            Ok(Value::Boolean(result))
        }
        PrimOp::IsNone => Ok(Value::Boolean(matches!(args[0], Value::None))),
        PrimOp::IsSome => Ok(Value::Boolean(matches!(args[0], Value::Some(_)))),
        PrimOp::ToString => Ok(Value::String(args[0].to_string_value().into())),
        _ => dispatch_error("builtin-compat", op),
    }
}

/// Executes concat as a true primop for `Array + Array`.
///
/// Returns a typed primop error when either argument is not an array.
fn execute_concat_array_primop(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
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

/// Extracts a HAMT-backed map handle from a runtime value.
///
/// Returns a typed primop error when the value is not a map.
fn expect_hamt_handle(
    ctx: &dyn RuntimeContext,
    value: &Value,
    op: PrimOp,
) -> Result<GcHandle, String> {
    match value {
        Value::Gc(h) if is_hamt(ctx.gc_heap(), *h) => Ok(*h),
        Value::Gc(h) => match ctx.gc_heap().get(*h) {
            HeapObject::Cons { .. } => Err(type_error(op, "Map", value)),
            _ => Err(type_error(op, "Map", value)),
        },
        other => Err(type_error(op, "Map", other)),
    }
}

/// Extracts a string slice from a runtime value.
///
/// Returns a typed primop error when the value is not a string.
fn expect_string(value: &Value, op: PrimOp) -> Result<&str, String> {
    match value {
        Value::String(v) => Ok(v.as_ref()),
        other => Err(type_error(op, "String", other)),
    }
}

/// Counts the length of a GC cons-list by following `tail` links until terminal.
///
/// Non-list values terminate traversal and return the count accumulated so far.
fn cons_len(ctx: &dyn RuntimeContext, value: &Value) -> usize {
    let mut count = 0usize;
    let mut current = value.clone();
    loop {
        match &current {
            Value::None | Value::EmptyList => return count,
            Value::Gc(h) => match ctx.gc_heap().get(*h) {
                HeapObject::Cons { tail, .. } => {
                    count += 1;
                    current = tail.clone();
                }
                _ => return count,
            },
            _ => return count,
        }
    }
}

/// Collects a cons-list into a vector, preserving element order.
///
/// Returns `None` when the input is not a well-formed list.
fn collect_list_values(ctx: &dyn RuntimeContext, value: &Value) -> Option<Vec<Value>> {
    let mut elements = Vec::new();
    let mut current = value.clone();
    loop {
        match &current {
            Value::None | Value::EmptyList => return Some(elements),
            Value::Gc(h) => match ctx.gc_heap().get(*h) {
                HeapObject::Cons { head, tail } => {
                    elements.push(head.clone());
                    current = tail.clone();
                }
                _ => return None,
            },
            _ => return None,
        }
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
    use super::*;
    use crate::runtime::{
        gc::{GcHeap, hamt::hamt_empty, hamt::hamt_insert},
        hash_key::HashKey,
    };

    struct TestRuntimeContext {
        heap: GcHeap,
    }

    impl TestRuntimeContext {
        fn new() -> Self {
            Self {
                heap: GcHeap::new(),
            }
        }
    }

    impl RuntimeContext for TestRuntimeContext {
        fn invoke_value(&mut self, _callee: Value, _args: Vec<Value>) -> Result<Value, String> {
            Err("invoke_value is not used by these primop tests".to_string())
        }

        fn gc_heap(&self) -> &GcHeap {
            &self.heap
        }

        fn gc_heap_mut(&mut self) -> &mut GcHeap {
            &mut self.heap
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
            vec![Value::String("Flux ".into()), Value::String("Lang".into())],
        )
        .expect("string_concat should succeed");

        assert_eq!(result, Value::String("Flux Lang".into()));
    }

    #[test]
    fn execute_string_slice_returns_sliced_string() {
        let mut ctx = TestRuntimeContext::new();
        let result = execute_primop(
            &mut ctx,
            PrimOp::StringSlice,
            vec![
                Value::String("Hello World".into()),
                Value::Integer(0),
                Value::Integer(2),
            ],
        )
        .expect("string_slice should succeed");

        assert_eq!(result, Value::String("He".into()))
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

    fn hamt_value(ctx: &mut TestRuntimeContext, entries: Vec<(HashKey, Value)>) -> Value {
        let mut root = hamt_empty(ctx.gc_heap_mut());
        for (k, v) in entries {
            root = hamt_insert(ctx.gc_heap_mut(), root, k, v);
        }
        Value::Gc(root)
    }

    #[test]
    fn execute_map_get_returns_some_for_existing_key() {
        let mut ctx = TestRuntimeContext::new();
        let map = hamt_value(
            &mut ctx,
            vec![(
                HashKey::String("lang".to_string()),
                Value::String("flux".into()),
            )],
        );

        let result = execute_primop(
            &mut ctx,
            PrimOp::MapGet,
            vec![map, Value::String("lang".into())],
        )
        .expect("map_get should succeed");

        assert_eq!(result, Value::Some(Rc::new(Value::String("flux".into()))));
    }

    #[test]
    fn execute_map_get_returns_none_for_missing_key() {
        let mut ctx = TestRuntimeContext::new();
        let map = hamt_value(
            &mut ctx,
            vec![(
                HashKey::String("lang".to_string()),
                Value::String("flux".into()),
            )],
        );

        let result = execute_primop(
            &mut ctx,
            PrimOp::MapGet,
            vec![map, Value::String("missing".into())],
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
                Value::String("flux".into()),
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
            vec![Value::String("Hello World".into())],
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
            vec![map, Value::String("answer".into()), Value::Integer(42)],
        )
        .expect("map_set should succeed");

        let fetched = execute_primop(
            &mut ctx,
            PrimOp::MapGet,
            vec![updated, Value::String("answer".into())],
        )
        .expect("map_get should succeed");

        assert_eq!(fetched, Value::Some(Rc::new(Value::Integer(42))));
    }

    #[test]
    fn execute_len_counts_hamt_entries() {
        let mut ctx = TestRuntimeContext::new();
        let map = hamt_value(
            &mut ctx,
            vec![
                (HashKey::String("a".to_string()), Value::Integer(1)),
                (HashKey::String("b".to_string()), Value::Integer(2)),
            ],
        );

        let result = execute_primop(&mut ctx, PrimOp::Len, vec![map]).expect("len should work");
        assert_eq!(result, Value::Integer(2));
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
    fn execute_type_of_and_is_hash_for_map_gc_value() {
        let mut ctx = TestRuntimeContext::new();
        let map = hamt_value(
            &mut ctx,
            vec![(HashKey::String("k".to_string()), Value::Integer(1))],
        );

        let ty = execute_primop(&mut ctx, PrimOp::TypeOf, vec![map.clone()])
            .expect("type_of should work");
        assert_eq!(ty, Value::String("Map".into()));

        let is_hash =
            execute_primop(&mut ctx, PrimOp::IsHash, vec![map]).expect("is_hash should work");
        assert_eq!(is_hash, Value::Boolean(true));
    }

    #[test]
    fn execute_to_string_formats_value() {
        let mut ctx = TestRuntimeContext::new();
        let result = execute_primop(
            &mut ctx,
            PrimOp::ToString,
            vec![Value::Array(Rc::new(vec![
                Value::Integer(1),
                Value::Integer(2),
            ]))],
        )
        .expect("to_string should work");

        assert_eq!(result, Value::String("[|1, 2|]".into()));
    }

    #[test]
    fn resolve_primop_call_extended_mappings_and_concat_array_mapping() {
        assert_eq!(resolve_primop_call("first", 1), Some(PrimOp::First));
        assert_eq!(resolve_primop_call("trim", 1), Some(PrimOp::Trim));
        assert_eq!(resolve_primop_call("keys", 1), Some(PrimOp::Keys));
        assert_eq!(resolve_primop_call("parse_int", 1), Some(PrimOp::ParseInt));
        assert_eq!(
            resolve_primop_call("split_ints", 2),
            Some(PrimOp::SplitInts)
        );
        assert_eq!(resolve_primop_call("concat", 2), Some(PrimOp::ConcatArray));
    }

    #[test]
    fn execute_string_primop_ops_match_builtin_behavior() {
        let mut ctx = TestRuntimeContext::new();
        let trimmed = execute_primop(&mut ctx, PrimOp::Trim, vec![Value::String("  hi  ".into())])
            .expect("trim should work");
        assert_eq!(trimmed, Value::String("hi".into()));

        let starts = execute_primop(
            &mut ctx,
            PrimOp::StartsWith,
            vec![Value::String("hello".into()), Value::String("he".into())],
        )
        .expect("starts_with should work");
        assert_eq!(starts, Value::Boolean(true));
    }

    #[test]
    fn execute_map_primop_ops_work() {
        let mut ctx = TestRuntimeContext::new();
        let map = hamt_value(
            &mut ctx,
            vec![(HashKey::String("a".to_string()), Value::Integer(1))],
        );

        let keys = execute_primop(&mut ctx, PrimOp::Keys, vec![map.clone()]).expect("keys works");
        match keys {
            Value::Array(items) => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0], Value::String("a".into()));
            }
            other => panic!("expected Array, got {}", other.type_name()),
        }

        let deleted = execute_primop(
            &mut ctx,
            PrimOp::Delete,
            vec![map, Value::String("a".into())],
        )
        .expect("delete works");
        let fetched = execute_primop(
            &mut ctx,
            PrimOp::MapGet,
            vec![deleted, Value::String("a".into())],
        )
        .expect("map_get works");
        assert_eq!(fetched, Value::None);
    }

    #[test]
    fn execute_parse_primop_errors_preserve_builtin_wording() {
        let mut ctx = TestRuntimeContext::new();
        let err = execute_primop(
            &mut ctx,
            PrimOp::ParseInt,
            vec![Value::String("12x".into())],
        )
        .expect_err("parse_int should fail");
        assert!(err.contains("could not parse"));
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
