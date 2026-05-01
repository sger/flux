//! Direct CorePrimOp dispatch for the VM (Proposal 0133 Step 5).
//!
//! Replaces the old PrimOp → execute_primop() path with a single dispatch
//! keyed by CorePrimOp.  Same Rust implementations, no translation layer.

use std::fs;
use std::io::Read as IoRead;
use std::rc::Rc;
use std::time::{Instant, SystemTime};

use crate::core::CorePrimOp;
use crate::runtime::RuntimeContext;
use crate::runtime::hamt as rc_hamt;
use crate::runtime::hash_key::HashKey;
use crate::runtime::value::{Value, format_value};

/// Execute a `CorePrimOp` with the given arguments.
///
/// This is the single dispatch point for all `OpPrimOp` instructions in the VM.
/// Each arm matches a `CorePrimOp` variant and runs the corresponding Rust
/// implementation inline (no sub-dispatch through `PrimOp`).
pub fn execute_core_primop(
    ctx: &mut dyn RuntimeContext,
    op: CorePrimOp,
    args: Vec<Value>,
) -> Result<Value, String> {
    use CorePrimOp::*;

    match op {
        // ── Typed integer arithmetic ──────────────────────────────────
        IAdd => int2(&args, |a, b| Value::Integer(a + b), "iadd"),
        ISub => int2(&args, |a, b| Value::Integer(a - b), "isub"),
        IMul => int2(&args, |a, b| Value::Integer(a * b), "imul"),
        IDiv => int2_result(
            &args,
            |a, b| {
                if b == 0 {
                    Err("division by zero".into())
                } else {
                    Ok(Value::Integer(a / b))
                }
            },
            "idiv",
        ),
        IMod => int2_result(
            &args,
            |a, b| {
                if b == 0 {
                    Err("modulo by zero".into())
                } else {
                    Ok(Value::Integer(a % b))
                }
            },
            "imod",
        ),

        // ── Safe arithmetic (Proposal 0135) ──────────────────────────
        SafeDiv => safe_arith_div(&args),
        SafeMod => safe_arith_mod(&args),

        // ── Typed float arithmetic ────────────────────────────────────
        FAdd => float2(&args, |a, b| Value::Float(a + b), "fadd"),
        FSub => float2(&args, |a, b| Value::Float(a - b), "fsub"),
        FMul => float2(&args, |a, b| Value::Float(a * b), "fmul"),
        FDiv => float2(&args, |a, b| Value::Float(a / b), "fdiv"),
        FSqrt => float1(&args, |a| Value::Float(a.sqrt()), "fsqrt"),
        FSin => float1(&args, |a| Value::Float(a.sin()), "fsin"),
        FCos => float1(&args, |a| Value::Float(a.cos()), "fcos"),
        FExp => float1(&args, |a| Value::Float(a.exp()), "fexp"),
        FLog => float1(&args, |a| Value::Float(a.ln()), "flog"),
        FFloor => float1(&args, |a| Value::Float(a.floor()), "ffloor"),
        FCeil => float1(&args, |a| Value::Float(a.ceil()), "fceil"),
        FRound => float1(&args, |a| Value::Float(a.round()), "fround"),
        FTan => float1(&args, |a| Value::Float(a.tan()), "ftan"),
        FAsin => float1(&args, |a| Value::Float(a.asin()), "fasin"),
        FAcos => float1(&args, |a| Value::Float(a.acos()), "facos"),
        FAtan => float1(&args, |a| Value::Float(a.atan()), "fatan"),
        FSinh => float1(&args, |a| Value::Float(a.sinh()), "fsinh"),
        FCosh => float1(&args, |a| Value::Float(a.cosh()), "fcosh"),
        FTanh => float1(&args, |a| Value::Float(a.tanh()), "ftanh"),
        FTruncate => float1(&args, |a| Value::Float(a.trunc()), "ftruncate"),
        BitAnd => int2(&args, |a, b| Value::Integer(a & b), "bit_and"),
        BitOr => int2(&args, |a, b| Value::Integer(a | b), "bit_or"),
        BitXor => int2(&args, |a, b| Value::Integer(a ^ b), "bit_xor"),
        BitShl => int2(
            &args,
            |a, b| Value::Integer(a.wrapping_shl(masked_shift_amount(b))),
            "bit_shl",
        ),
        BitShr => int2(
            &args,
            |a, b| Value::Integer(a.wrapping_shr(masked_shift_amount(b))),
            "bit_shr",
        ),

        // ── Numeric helpers ───────────────────────────────────────────
        Abs => match &args[0] {
            Value::Integer(v) => Ok(Value::Integer(v.abs())),
            Value::Float(v) => Ok(Value::Float(v.abs())),
            other => Err(terr("abs", "Number", other)),
        },
        Min => numeric_min_max(&args, "min", true),
        Max => numeric_min_max(&args, "max", false),
        Neg => match &args[0] {
            Value::Integer(v) => Ok(Value::Integer(-v)),
            Value::Float(v) => Ok(Value::Float(-v)),
            other => Err(terr("neg", "Number", other)),
        },

        // ── Typed integer comparisons ─────────────────────────────────
        ICmpEq => int_cmp(&args, |a, b| a == b, "icmp_eq"),
        ICmpNe => int_cmp(&args, |a, b| a != b, "icmp_ne"),
        ICmpLt => int_cmp(&args, |a, b| a < b, "icmp_lt"),
        ICmpLe => int_cmp(&args, |a, b| a <= b, "icmp_le"),
        ICmpGt => int_cmp(&args, |a, b| a > b, "icmp_gt"),
        ICmpGe => int_cmp(&args, |a, b| a >= b, "icmp_ge"),

        // ── Typed float comparisons ───────────────────────────────────
        FCmpEq => float_cmp(&args, |a, b| a == b, "fcmp_eq"),
        FCmpNe => float_cmp(&args, |a, b| a != b, "fcmp_ne"),
        FCmpLt => float_cmp(&args, |a, b| a < b, "fcmp_lt"),
        FCmpLe => float_cmp(&args, |a, b| a <= b, "fcmp_le"),
        FCmpGt => float_cmp(&args, |a, b| a > b, "fcmp_gt"),
        FCmpGe => float_cmp(&args, |a, b| a >= b, "fcmp_ge"),

        // ── Deep structural comparison ────────────────────────────────
        CmpEq => Ok(Value::Boolean(args[0] == args[1])),
        CmpNe => Ok(Value::Boolean(args[0] != args[1])),

        // ── Array operations ──────────────────────────────────────────
        ArrayLen => match &args[0] {
            Value::Array(items) => Ok(Value::Integer(items.len() as i64)),
            other => Err(terr("array_len", "Array", other)),
        },
        ArrayGet => {
            let index = eint(&args[1], "array_get")?;
            match &args[0] {
                Value::Array(items) => {
                    if index < 0 || index as usize >= items.len() {
                        Ok(Value::None)
                    } else {
                        Ok(items[index as usize].clone())
                    }
                }
                other => Err(terr("array_get", "Array", other)),
            }
        }
        ArraySet => {
            let index = eint(&args[1], "array_set")?;
            match &args[0] {
                Value::Array(items) => {
                    if index < 0 || index as usize >= items.len() {
                        return Err(format!(
                            "array_set: index {} out of bounds for length {}",
                            index,
                            items.len()
                        ));
                    }
                    let mut items = items.clone();
                    Rc::make_mut(&mut items)[index as usize] = args[2].clone();
                    Ok(Value::Array(items))
                }
                other => Err(terr("array_set", "Array", other)),
            }
        }
        ArrayPush => {
            let mut args = args;
            let elem = args.swap_remove(1);
            let arr_obj = args.swap_remove(0);
            match arr_obj {
                Value::Array(mut arr) => {
                    Rc::make_mut(&mut arr).push(elem);
                    Ok(Value::Array(arr))
                }
                other => Err(terr("push", "Array", &other)),
            }
        }
        ArrayConcat => {
            let left = earr(&args[0], "concat")?;
            let right = earr(&args[1], "concat")?;
            let mut out = left.clone();
            Rc::make_mut(&mut out).extend(right.iter().cloned());
            Ok(Value::Array(out))
        }
        ArraySlice => {
            let arr = earr(&args[0], "slice")?;
            let start = eint(&args[1], "slice")?;
            let end = eint(&args[2], "slice")?;
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

        // ── HAMT operations ───────────────────────────────────────────
        HamtGet => {
            let key = args[1]
                .to_hash_key()
                .ok_or_else(|| hkey_err("get", &args[1]))?;
            match &args[0] {
                Value::HashMap(node) => match rc_hamt::hamt_lookup(node, &key) {
                    Some(value) => Ok(Value::Some(Rc::new(value))),
                    None => Ok(Value::None),
                },
                other => Err(terr("get", "Map", other)),
            }
        }
        HamtSet => {
            let key = args[1]
                .to_hash_key()
                .ok_or_else(|| hkey_err("put", &args[1]))?;
            match &args[0] {
                Value::HashMap(node) => Ok(Value::HashMap(rc_hamt::hamt_insert(
                    node,
                    key,
                    args[2].clone(),
                ))),
                other => Err(terr("put", "Map", other)),
            }
        }
        HamtContains => {
            let key = args[1]
                .to_hash_key()
                .ok_or_else(|| hkey_err("has_key", &args[1]))?;
            match &args[0] {
                Value::HashMap(node) => {
                    Ok(Value::Boolean(rc_hamt::hamt_lookup(node, &key).is_some()))
                }
                other => Err(terr("has_key", "Map", other)),
            }
        }
        HamtDelete => {
            let node = ehamt(&args[0], "delete")?;
            let key = args[1]
                .to_hash_key()
                .ok_or_else(|| hkey_err("delete", &args[1]))?;
            Ok(Value::HashMap(rc_hamt::hamt_delete(node, &key)))
        }
        HamtKeys => {
            let node = ehamt(&args[0], "keys")?;
            let pairs = rc_hamt::hamt_iter(node);
            Ok(Value::Array(
                pairs
                    .iter()
                    .map(|(k, _)| hash_key_to_value(k))
                    .collect::<Vec<_>>()
                    .into(),
            ))
        }
        HamtValues => {
            let node = ehamt(&args[0], "values")?;
            let pairs = rc_hamt::hamt_iter(node);
            Ok(Value::Array(
                pairs.into_iter().map(|(_, v)| v).collect::<Vec<_>>().into(),
            ))
        }
        HamtMerge => {
            let node1 = ehamt(&args[0], "merge")?;
            let node2 = ehamt(&args[1], "merge")?;
            let pairs = rc_hamt::hamt_iter(node2);
            let mut result = Rc::clone(node1);
            for (k, v) in pairs {
                result = rc_hamt::hamt_insert(&result, k, v);
            }
            Ok(Value::HashMap(result))
        }
        HamtSize => {
            let node = ehamt(&args[0], "size")?;
            Ok(Value::Integer(rc_hamt::hamt_len(node) as i64))
        }

        // ── String operations ─────────────────────────────────────────
        StringLength => match &args[0] {
            Value::String(s) => Ok(Value::Integer(s.len() as i64)),
            other => Err(terr("string_length", "String", other)),
        },
        StringConcat => match (&args[0], &args[1]) {
            (Value::String(l), Value::String(r)) => Ok(Value::String(format!("{}{}", l, r).into())),
            (l, r) => Err(format!(
                "string_concat expects (String, String), got ({}, {})",
                l.type_name(),
                r.type_name()
            )),
        },
        StringSlice | Substring => {
            let s = estr(&args[0], "string_slice")?;
            let start = eint(&args[1], "string_slice")?;
            let end = eint(&args[2], "string_slice")?;
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
                Ok(Value::String(
                    chars[start..end].iter().collect::<String>().into(),
                ))
            }
        }
        StringToBytes => {
            let s = estr(&args[0], "string_to_bytes")?;
            Ok(Value::Bytes(s.as_bytes().to_vec().into()))
        }
        BytesLength => {
            let bytes = ebytes(&args[0], "bytes_length")?;
            Ok(Value::Integer(bytes.len() as i64))
        }
        BytesSlice => {
            let bytes = ebytes(&args[0], "bytes_slice")?;
            let start = eint(&args[1], "bytes_slice")?;
            let end = eint(&args[2], "bytes_slice")?;
            let len = bytes.len() as i64;
            let start = if start < 0 {
                0
            } else {
                start.min(len) as usize
            };
            let end = if end < 0 { 0 } else { end.min(len) as usize };
            if start >= end {
                Ok(Value::Bytes(Vec::new().into()))
            } else {
                Ok(Value::Bytes(bytes[start..end].to_vec().into()))
            }
        }
        BytesToString => {
            let bytes = ebytes(&args[0], "bytes_to_string")?;
            Ok(Value::String(
                String::from_utf8_lossy(bytes.as_slice())
                    .into_owned()
                    .into(),
            ))
        }
        ToString => Ok(Value::String(format_value(&args[0]).into())),
        Split => {
            let s = estr(&args[0], "split")?;
            let delim = estr(&args[1], "split")?;
            let parts: Vec<Value> = if delim.is_empty() {
                s.chars()
                    .map(|c| Value::String(c.to_string().into()))
                    .collect()
            } else {
                s.split(delim)
                    .map(|p| Value::String(p.to_string().into()))
                    .collect()
            };
            Ok(Value::Array(parts.into()))
        }
        Trim => Ok(Value::String(
            estr(&args[0], "trim")?.trim().to_string().into(),
        )),
        Upper => Ok(Value::String(
            estr(&args[0], "upper")?.to_uppercase().into(),
        )),
        Lower => Ok(Value::String(
            estr(&args[0], "lower")?.to_lowercase().into(),
        )),
        Replace => Ok(Value::String(
            estr(&args[0], "replace")?
                .replace(estr(&args[1], "replace")?, estr(&args[2], "replace")?)
                .into(),
        )),

        // ── Type tag inspection ───────────────────────────────────────
        IsInt => Ok(Value::Boolean(matches!(args[0], Value::Integer(_)))),
        IsFloat => Ok(Value::Boolean(matches!(args[0], Value::Float(_)))),
        IsString => Ok(Value::Boolean(matches!(args[0], Value::String(_)))),
        IsBool => Ok(Value::Boolean(matches!(args[0], Value::Boolean(_)))),
        IsArray => Ok(Value::Boolean(matches!(args[0], Value::Array(_)))),
        IsNone => Ok(Value::Boolean(matches!(args[0], Value::None))),
        IsSome => Ok(Value::Boolean(matches!(args[0], Value::Some(_)))),
        IsList => Ok(Value::Boolean(matches!(
            args[0],
            Value::None | Value::EmptyList | Value::Cons(_)
        ))),
        IsMap => Ok(Value::Boolean(matches!(args[0], Value::HashMap(_)))),
        TypeOf => {
            let name = match &args[0] {
                Value::Cons(_) => "List",
                Value::HashMap(_) => "Map",
                other => other.type_name(),
            };
            Ok(Value::String(name.to_string().into()))
        }

        // ── I/O ───────────────────────────────────────────────────────
        Print => {
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    print!(" ");
                }
                print!("{}", format_value(arg));
            }
            println!();
            Ok(Value::None)
        }
        Println => {
            println!("{}", format_value(&args[0]));
            Ok(Value::None)
        }
        DebugTrace => {
            // Debug output goes to stderr so program stdout stays clean for
            // piping to other tools. Matches GHC `Debug.Trace`, Rust `dbg!`,
            // Python's default logging behavior. The argument is expected to
            // be a pre-formatted string (the Flow.Debug wrappers call
            // `show()` on values before perform-ing the effect operation).
            eprintln!("{}", format_value(&args[0]));
            Ok(Value::None)
        }
        ReadFile => {
            let path = estr(&args[0], "read_file")?;
            let content = fs::read_to_string(path)
                .map_err(|e| format!("read_file failed for '{}': {}", path, e))?;
            Ok(Value::String(content.into()))
        }
        WriteFile => {
            let path = estr(&args[0], "write_file")?;
            let content = estr(&args[1], "write_file")?;
            fs::write(path, content)
                .map_err(|e| format!("write_file failed for '{}': {}", path, e))?;
            Ok(Value::None)
        }
        ReadStdin => {
            let mut input = String::new();
            std::io::stdin()
                .read_to_string(&mut input)
                .map_err(|e| format!("read_stdin failed: {}", e))?;
            Ok(Value::String(input.into()))
        }
        ReadLines => {
            let path = estr(&args[0], "read_lines")?;
            let content = fs::read_to_string(path)
                .map_err(|e| format!("read_lines failed for '{}': {}", path, e))?;
            let lines = content
                .lines()
                .map(|line| Value::String(line.trim_end_matches('\r').to_string().into()))
                .collect::<Vec<_>>();
            Ok(Value::Array(lines.into()))
        }

        // ── Task runtime (Proposal 0174 Phase 1a) ────────────────────
        TaskSpawn => ctx.task_spawn(args[0].clone()),
        TaskBlockingJoin => ctx.task_blocking_join(args[0].clone()),
        TaskCancel => ctx.task_cancel(args[0].clone()),
        AsyncSleep => ctx.async_sleep(args[0].clone()),
        AsyncYieldNow => ctx.async_yield_now(),
        AsyncBoth => ctx.async_both(args[0].clone(), args[1].clone()),
        AsyncRace => ctx.async_race(args[0].clone(), args[1].clone()),
        AsyncTimeout => ctx.async_timeout(args[0].clone(), args[1].clone()),
        AsyncTimeoutResult => ctx.async_timeout_result(args[0].clone(), args[1].clone()),
        AsyncScope => ctx.async_scope(args[0].clone()),
        AsyncFork => ctx.async_fork(args[0].clone(), args[1].clone()),
        AsyncTry => ctx.async_try(args[0].clone()),
        AsyncFinally => ctx.async_finally(args[0].clone(), args[1].clone()),
        AsyncBracket => ctx.async_bracket(args[0].clone(), args[1].clone(), args[2].clone()),
        ChannelBounded => ctx.channel_bounded(args[0].clone()),
        ChannelSend => ctx.channel_send(args[0].clone(), args[1].clone()),
        ChannelRecv => ctx.channel_recv(args[0].clone()),
        ChannelClose => ctx.channel_close(args[0].clone()),

        // ── Control ───────────────────────────────────────────────────
        Unwrap => match &args[0] {
            Value::None => Err("unwrap called on None".into()),
            other => Ok(other.clone()),
        },
        Panic => Err(format!("panic: {}", args[0].to_string_value())),
        ClockNow => {
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_err(|e| format!("clock_now failed: {}", e))?;
            Ok(Value::Integer(now.as_millis() as i64))
        }
        Time => {
            let start = Instant::now();
            let _ = ctx
                .invoke_value(args[0].clone(), vec![])
                .map_err(|e| format!("time: callback error: {}", e))?;
            let elapsed_ms = start.elapsed().as_millis();
            Ok(Value::Integer(elapsed_ms.min(i64::MAX as u128) as i64))
        }
        Try => match ctx.invoke_value(args[0].clone(), vec![]) {
            Ok(val) => Ok(Value::Tuple(Rc::new(vec![
                Value::String("ok".to_string().into()),
                val,
            ]))),
            Err(msg) => Ok(Value::Tuple(Rc::new(vec![
                Value::String("error".to_string().into()),
                Value::String(Rc::new(msg)),
            ]))),
        },
        AssertThrows => {
            let expected_msg: Option<&str> = if args.len() >= 2 {
                match &args[1] {
                    Value::String(s) => Some(s.as_ref()),
                    _ => None,
                }
            } else {
                None
            };
            match ctx.invoke_value(args[0].clone(), vec![]) {
                Ok(_) => Err("assert_throws failed: function completed without error".into()),
                Err(msg) => match expected_msg {
                    Some(expected) if msg.contains(expected) => Ok(Value::None),
                    Some(expected) => Err(format!(
                        "assert_throws failed\n  expected error containing: {}\n  actual error: {}",
                        expected, msg
                    )),
                    None => Ok(Value::None),
                },
            }
        }

        // ── Parsing ───────────────────────────────────────────────────
        ParseInt => {
            let text = estr(&args[0], "parse_int")?;
            let parsed = text
                .trim()
                .parse::<i64>()
                .map_err(|_| format!("parse_int: could not parse '{}' as Int", text))?;
            Ok(Value::Integer(parsed))
        }
        // ── Polymorphic length ────────────────────────────────────────
        Len => match &args[0] {
            Value::String(s) => Ok(Value::Integer(s.len() as i64)),
            Value::Bytes(bytes) => Ok(Value::Integer(bytes.len() as i64)),
            Value::Array(arr) => Ok(Value::Integer(arr.len() as i64)),
            Value::Tuple(t) => Ok(Value::Integer(t.len() as i64)),
            Value::None | Value::EmptyList => Ok(Value::Integer(0)),
            Value::Cons(_) => {
                let mut count: i64 = 0;
                let mut cur = &args[0];
                loop {
                    match cur {
                        Value::None | Value::EmptyList => break,
                        Value::Cons(cell) => {
                            count += 1;
                            cur = &cell.tail;
                        }
                        _ => break,
                    }
                }
                Ok(Value::Integer(count))
            }
            Value::HashMap(node) => Ok(Value::Integer(rc_hamt::hamt_len(node) as i64)),
            other => Err(terr("len", "String, Bytes, Array, Tuple, or Map", other)),
        },

        // ── Effect handler ops (native-only, Koka-style yield model) ────
        EvvGet | EvvSet | FreshMarker | EvvInsert | YieldTo | YieldExtend | YieldPrompt
        | IsYielding | PerformDirect | TcpListen | TcpAccept | TcpConnect | TcpRead | TcpWrite
        | TcpClose | TcpLocalAddr | TcpRemoteAddr | TcpCloseListener | TcpListenerLocalAddr => {
            Err(format!(
                "CorePrimOp {:?} is native-backend only (Koka yield model)",
                op
            ))
        }

        // ── Generic/structural ops (never emitted as OpPrimOp) ───────
        Add | Sub | Mul | Div | Mod | Not | Eq | NEq | Lt | Le | Gt | Ge | And | Or | Concat
        | Interpolate | MakeList | MakeArray | MakeTuple | MakeHash | Index => Err(format!(
            "CorePrimOp {:?} should not appear in OpPrimOp bytecode",
            op
        )),
    }
}

// ── Compact helper functions ─────────────────────────────────────────────────

fn terr(op: &str, expected: &str, got: &Value) -> String {
    format!(
        "primop {} expected {}, got {}",
        op,
        expected,
        got.type_name()
    )
}

fn hkey_err(op: &str, v: &Value) -> String {
    format!(
        "primop {} expects hashable key (String, Int, Bool), got {}",
        op,
        v.type_name()
    )
}

fn estr<'a>(v: &'a Value, op: &str) -> Result<&'a str, String> {
    match v {
        Value::String(s) => Ok(s.as_ref()),
        other => Err(terr(op, "String", other)),
    }
}

fn ebytes<'a>(v: &'a Value, op: &str) -> Result<&'a Vec<u8>, String> {
    match v {
        Value::Bytes(bytes) => Ok(bytes.as_ref()),
        other => Err(terr(op, "Bytes", other)),
    }
}

fn eint(v: &Value, op: &str) -> Result<i64, String> {
    match v {
        Value::Integer(n) => Ok(*n),
        other => Err(terr(op, "Int", other)),
    }
}

fn efloat(v: &Value, op: &str) -> Result<f64, String> {
    match v {
        Value::Float(n) => Ok(*n),
        other => Err(terr(op, "Float", other)),
    }
}

fn earr<'a>(v: &'a Value, op: &str) -> Result<&'a Rc<Vec<Value>>, String> {
    match v {
        Value::Array(a) => Ok(a),
        other => Err(terr(op, "Array", other)),
    }
}

fn ehamt<'a>(v: &'a Value, op: &str) -> Result<&'a Rc<rc_hamt::HamtNode>, String> {
    match v {
        Value::HashMap(n) => Ok(n),
        other => Err(terr(op, "Map", other)),
    }
}

fn int2(args: &[Value], f: impl FnOnce(i64, i64) -> Value, op: &str) -> Result<Value, String> {
    Ok(f(eint(&args[0], op)?, eint(&args[1], op)?))
}

fn int2_result(
    args: &[Value],
    f: impl FnOnce(i64, i64) -> Result<Value, String>,
    op: &str,
) -> Result<Value, String> {
    f(eint(&args[0], op)?, eint(&args[1], op)?)
}

fn float2(args: &[Value], f: impl FnOnce(f64, f64) -> Value, op: &str) -> Result<Value, String> {
    Ok(f(efloat(&args[0], op)?, efloat(&args[1], op)?))
}

fn float1(args: &[Value], f: impl FnOnce(f64) -> Value, op: &str) -> Result<Value, String> {
    Ok(f(efloat(&args[0], op)?))
}

fn masked_shift_amount(value: i64) -> u32 {
    (value as u64 & 63) as u32
}

fn int_cmp(args: &[Value], f: impl FnOnce(i64, i64) -> bool, op: &str) -> Result<Value, String> {
    Ok(Value::Boolean(f(eint(&args[0], op)?, eint(&args[1], op)?)))
}

fn float_cmp(args: &[Value], f: impl FnOnce(f64, f64) -> bool, op: &str) -> Result<Value, String> {
    Ok(Value::Boolean(f(
        efloat(&args[0], op)?,
        efloat(&args[1], op)?,
    )))
}

fn numeric_min_max(args: &[Value], op: &str, is_min: bool) -> Result<Value, String> {
    let (a_num, b_num) = match (&args[0], &args[1]) {
        (Value::Integer(x), Value::Integer(y)) => (*x as f64, *y as f64),
        (Value::Integer(x), Value::Float(y)) => (*x as f64, *y),
        (Value::Float(x), Value::Integer(y)) => (*x, *y as f64),
        (Value::Float(x), Value::Float(y)) => (*x, *y),
        (l, r) => {
            return Err(format!(
                "primop {} expects (Number, Number), got ({}, {})",
                op,
                l.type_name(),
                r.type_name()
            ));
        }
    };
    let result = if is_min {
        a_num.min(b_num)
    } else {
        a_num.max(b_num)
    };
    match (&args[0], &args[1]) {
        (Value::Integer(_), Value::Integer(_)) => Ok(Value::Integer(result as i64)),
        _ => Ok(Value::Float(result)),
    }
}

fn hash_key_to_value(key: &HashKey) -> Value {
    match key {
        HashKey::Integer(v) => Value::Integer(*v),
        HashKey::Boolean(v) => Value::Boolean(*v),
        HashKey::String(v) => Value::String(v.clone().into()),
    }
}

// ── Safe arithmetic (Proposal 0135) ─────────────────────────────────────────

fn safe_arith_div(args: &[Value]) -> Result<Value, String> {
    match (&args[0], &args[1]) {
        (Value::Integer(a), Value::Integer(b)) => {
            if *b == 0 {
                Ok(Value::None)
            } else {
                Ok(Value::Some(Rc::new(Value::Integer(a / b))))
            }
        }
        (Value::Float(a), Value::Float(b)) => {
            if *b == 0.0 {
                Ok(Value::None)
            } else {
                Ok(Value::Some(Rc::new(Value::Float(a / b))))
            }
        }
        (Value::Integer(a), Value::Float(b)) => {
            if *b == 0.0 {
                Ok(Value::None)
            } else {
                Ok(Value::Some(Rc::new(Value::Float(*a as f64 / b))))
            }
        }
        (Value::Float(a), Value::Integer(b)) => {
            if *b == 0 {
                Ok(Value::None)
            } else {
                Ok(Value::Some(Rc::new(Value::Float(a / *b as f64))))
            }
        }
        (a, b) => Err(format!(
            "safe_div expects (Number, Number), got ({}, {})",
            a.type_name(),
            b.type_name()
        )),
    }
}

fn safe_arith_mod(args: &[Value]) -> Result<Value, String> {
    match (&args[0], &args[1]) {
        (Value::Integer(a), Value::Integer(b)) => {
            if *b == 0 {
                Ok(Value::None)
            } else {
                Ok(Value::Some(Rc::new(Value::Integer(a % b))))
            }
        }
        (Value::Float(a), Value::Float(b)) => {
            if *b == 0.0 {
                Ok(Value::None)
            } else {
                Ok(Value::Some(Rc::new(Value::Float(a % b))))
            }
        }
        (Value::Integer(a), Value::Float(b)) => {
            if *b == 0.0 {
                Ok(Value::None)
            } else {
                Ok(Value::Some(Rc::new(Value::Float(*a as f64 % b))))
            }
        }
        (Value::Float(a), Value::Integer(b)) => {
            if *b == 0 {
                Ok(Value::None)
            } else {
                Ok(Value::Some(Rc::new(Value::Float(*a % *b as f64))))
            }
        }
        (a, b) => Err(format!(
            "safe_mod expects (Number, Number), got ({}, {})",
            a.type_name(),
            b.type_name()
        )),
    }
}
