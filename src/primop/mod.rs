use std::{fs, rc::Rc, time::SystemTime};

use crate::runtime::{
    RuntimeContext,
    gc::{
        GcHandle, HeapObject,
        hamt::{hamt_insert, hamt_len, hamt_lookup, is_hamt},
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
    pub const COUNT: usize = 50;

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
            | Self::ToString => 1,
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
            | Self::Max => 2,
            Self::ArraySet | Self::MapSet | Self::StringSlice => 3,
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
        }
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

        let is_hash = execute_primop(&mut ctx, PrimOp::IsHash, vec![map])
            .expect("is_hash should work");
        assert_eq!(is_hash, Value::Boolean(true));
    }

    #[test]
    fn execute_to_string_formats_value() {
        let mut ctx = TestRuntimeContext::new();
        let result = execute_primop(
            &mut ctx,
            PrimOp::ToString,
            vec![Value::Array(Rc::new(vec![Value::Integer(1), Value::Integer(2)]))],
        )
        .expect("to_string should work");

        assert_eq!(result, Value::String("[|1, 2|]".into()));
    }
}
