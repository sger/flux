use std::{
    fs,
    io::Read,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use crate::runtime::{RuntimeContext, value::Value};

use super::helpers::{arg_array, arg_string, check_arity, format_hint, type_error};

pub(super) fn builtin_read_file(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "read_file", "read_file(path)")?;
    let path = arg_string(&args, 0, "read_file", "argument", "read_file(path)")?;
    let content = fs::read_to_string(path).map_err(|e| {
        format!(
            "read_file: could not read file: {}{}",
            e,
            format_hint("read_file(path)")
        )
    })?;
    Ok(Value::String(content.into()))
}

pub(super) fn builtin_read_lines(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "read_lines", "read_lines(path)")?;
    let path = arg_string(&args, 0, "read_lines", "argument", "read_lines(path)")?;
    let content = fs::read_to_string(path).map_err(|e| {
        format!(
            "read_lines: could not read file: {}{}",
            e,
            format_hint("read_lines(path)")
        )
    })?;

    let lines = content
        .lines()
        .map(|line| Value::String(line.trim_end_matches('\r').to_string().into()))
        .collect::<Vec<_>>();
    Ok(Value::Array(lines.into()))
}

pub(super) fn builtin_read_stdin(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 0, "read_stdin", "read_stdin()")?;
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).map_err(|e| {
        format!(
            "read_stdin: failed to read stdin: {}{}",
            e,
            format_hint("read_stdin()")
        )
    })?;
    Ok(Value::String(input.into()))
}

pub(super) fn builtin_parse_int(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "parse_int", "parse_int(s)")?;
    let text = arg_string(&args, 0, "parse_int", "argument", "parse_int(s)")?;
    let parsed = text.trim().parse::<i64>().map_err(|_| {
        format!(
            "parse_int: could not parse '{}' as Int{}",
            text,
            format_hint("parse_int(s)")
        )
    })?;
    Ok(Value::Integer(parsed))
}

pub(super) fn builtin_parse_ints(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "parse_ints", "parse_ints(lines)")?;
    let lines = arg_array(&args, 0, "parse_ints", "argument", "parse_ints(lines)")?;

    let mut out = Vec::with_capacity(lines.len());
    for value in lines {
        match value {
            Value::String(s) => {
                let parsed = s.trim().parse::<i64>().map_err(|_| {
                    format!(
                        "parse_ints: could not parse '{}' as Int{}",
                        s,
                        format_hint("parse_ints(lines)")
                    )
                })?;
                out.push(Value::Integer(parsed));
            }
            other => {
                return Err(type_error(
                    "parse_ints",
                    "array elements",
                    "String",
                    other.type_name(),
                    "parse_ints(lines)",
                ));
            }
        }
    }

    Ok(Value::Array(out.into()))
}

pub(super) fn builtin_split_ints(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 2, "split_ints", "split_ints(s, delim)")?;
    let s = arg_string(
        &args,
        0,
        "split_ints",
        "first argument",
        "split_ints(s, delim)",
    )?;
    let delim = arg_string(
        &args,
        1,
        "split_ints",
        "second argument",
        "split_ints(s, delim)",
    )?;

    if delim.is_empty() {
        let mut out = Vec::with_capacity(s.chars().count());
        for ch in s.chars() {
            let text = ch.to_string();
            let parsed = text.trim().parse::<i64>().map_err(|_| {
                format!(
                    "split_ints: could not parse '{}' as Int{}",
                    text,
                    format_hint("split_ints(s, delim)")
                )
            })?;
            out.push(Value::Integer(parsed));
        }
        return Ok(Value::Array(out.into()));
    }

    let mut out = Vec::new();
    for part in s.split(delim) {
        let parsed = part.trim().parse::<i64>().map_err(|_| {
            format!(
                "split_ints: could not parse '{}' as Int{}",
                part,
                format_hint("split_ints(s, delim)")
            )
        })?;
        out.push(Value::Integer(parsed));
    }
    Ok(Value::Array(out.into()))
}

pub(super) fn builtin_now_ms(
    _ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 0, "now_ms", "now_ms()")?;
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| {
            format!(
                "now_ms: system clock error: {}{}",
                e,
                format_hint("now_ms()")
            )
        })?
        .as_millis();
    Ok(Value::Integer(millis.min(i64::MAX as u128) as i64))
}

pub(super) fn builtin_time(
    ctx: &mut dyn RuntimeContext,
    args: Vec<Value>,
) -> Result<Value, String> {
    check_arity(&args, 1, "time", "time(fn)")?;
    match &args[0] {
        Value::Closure(_) | Value::Builtin(_) | Value::JitClosure(_) => {}
        other => {
            return Err(type_error(
                "time",
                "first argument",
                "Function",
                other.type_name(),
                "time(fn)",
            ));
        }
    }

    let start = Instant::now();
    let _ = ctx
        .invoke_value(args[0].clone(), vec![])
        .map_err(|e| format!("time: callback error: {}", e))?;
    let elapsed_ms = start.elapsed().as_millis();
    Ok(Value::Integer(elapsed_ms.min(i64::MAX as u128) as i64))
}
