//! VM values to and from the effect tape's backend-neutral data.
//!
//! A tape stores what an effect produced as host data, never a runtime
//! value, so these conversions are structural: a string is text, an `Ok`
//! wraps whatever its payload is, an `Err(IoError)` is an I/O error. Only
//! the shapes the effect primops return appear here.

use std::rc::Rc;

use crate::core::CorePrimOp;
use crate::runtime::r#async::backend::{FsError, FsErrorKind};
use crate::runtime::replay::{
    EffectRequest, FsMeta, IoKind, Outcome, ProcOut, TapeBytes, TapeIoError,
};
use crate::runtime::value::{AdtFields, AdtValue, Value};

use super::core_dispatch::{vm_fs_error, vm_io_ok};

/// The tape request for `op` called with `args`. A wrong-typed argument is
/// reported here, before anything is recorded, with the same wording the
/// primop itself would use.
pub(super) fn request(op: CorePrimOp, args: &[Value]) -> Result<EffectRequest, String> {
    use CorePrimOp::*;
    let name = op_name(op);
    let path = |index: usize| arg_str(args, index, name);
    Ok(match op {
        ReadStdin => EffectRequest::ReadStdin,
        ReadFile => EffectRequest::ReadFile { path: path(0)? },
        ReadLines => EffectRequest::ReadLines { path: path(0)? },
        TryReadFile => EffectRequest::TryReadFile { path: path(0)? },
        FsExists => EffectRequest::FsExists { path: path(0)? },
        FsIsDir => EffectRequest::FsIsDir { path: path(0)? },
        FsIsFile => EffectRequest::FsIsFile { path: path(0)? },
        FsListDir => EffectRequest::FsListDir { path: path(0)? },
        FsMetadata => EffectRequest::FsMetadata { path: path(0)? },
        Sha256File => EffectRequest::Sha256File { path: path(0)? },
        EnvVar => EffectRequest::EnvVar { name: path(0)? },
        EnvArgs => EffectRequest::EnvArgs,
        EnvCwd => EffectRequest::EnvCwd,
        EnvHomeDir => EffectRequest::EnvHomeDir,
        ProcRun => {
            let Some(Value::Array(items)) = args.get(1) else {
                return Err(format!("{name} expects an Array of arguments"));
            };
            let argv = items
                .iter()
                .map(|item| match item {
                    Value::String(text) => Ok(text.to_string()),
                    other => Err(format!("{name} expects String, got {}", other.type_name())),
                })
                .collect::<Result<Vec<_>, _>>()?;
            EffectRequest::ProcRun {
                cmd: path(0)?,
                argv,
            }
        }
        ClockNow => EffectRequest::ClockNow,
        Time => EffectRequest::Time,
        WriteFile => EffectRequest::WriteFile {
            path: path(0)?,
            contents: TapeBytes::from(arg_str(args, 1, name)?),
        },
        FsWriteFile => EffectRequest::FsWriteFile {
            path: path(0)?,
            contents: TapeBytes::from(arg_str(args, 1, name)?),
        },
        FsCreateDirAll => EffectRequest::FsCreateDirAll { path: path(0)? },
        FsRemoveFile => EffectRequest::FsRemoveFile { path: path(0)? },
        FsRemoveDirAll => EffectRequest::FsRemoveDirAll { path: path(0)? },
        FsRename => EffectRequest::FsRename {
            from: path(0)?,
            to: path(1)?,
        },
        other => unreachable!("{other:?} is not an effect the tape records"),
    })
}

fn arg_str(args: &[Value], index: usize, name: &str) -> Result<String, String> {
    match args.get(index) {
        Some(Value::String(text)) => Ok(text.to_string()),
        Some(other) => Err(format!("{name} expects String, got {}", other.type_name())),
        None => Err(format!("{name} expects {} arguments", index + 1)),
    }
}

/// The primop's name as programs write it, for messages.
pub(super) fn op_name(op: CorePrimOp) -> &'static str {
    use CorePrimOp::*;
    match op {
        ReadStdin => "read_stdin",
        ReadFile => "read_file",
        ReadLines => "read_lines",
        TryReadFile => "try_read_file",
        FsExists => "exists",
        FsIsDir => "is_dir",
        FsIsFile => "is_file",
        FsListDir => "list_dir",
        FsMetadata => "metadata",
        Sha256File => "sha256_file",
        EnvVar => "env_var",
        EnvArgs => "env_args",
        EnvCwd => "env_cwd",
        EnvHomeDir => "env_home_dir",
        ProcRun => "proc_run",
        ClockNow => "now_ms",
        Time => "time",
        WriteFile => "write_file",
        FsWriteFile => "write_file",
        FsCreateDirAll => "create_dir_all",
        FsRemoveFile => "remove_file",
        FsRemoveDirAll => "remove_dir_all",
        FsRename => "rename",
        FiberRunAsync | FiberRunAsyncWith => "run_async",
        TaskSpawn | TaskSpawnMove | TaskSpawnScoped | TaskSpawnScopedMove => "Task.spawn",
        ChanMake => "Channel.make",
        TcpConnect | TcpListen => "Tcp",
        _ => "this effect",
    }
}

/// What the primop produced, as tape data. `Err` is a runtime error rather
/// than a value (a failed `read_file`); the tape replays it as the same error.
pub(super) fn outcome(op: CorePrimOp, result: Result<Value, String>) -> Outcome {
    match result {
        Ok(value) => value_outcome(op, &value),
        Err(message) => Outcome::Fail(message),
    }
}

fn value_outcome(op: CorePrimOp, value: &Value) -> Outcome {
    match value {
        // Unit and `None` are the same VM value; the operation says which.
        Value::None if is_unit_result(op) => Outcome::Unit,
        Value::None => Outcome::None,
        Value::Boolean(b) => Outcome::Bool(*b),
        Value::Integer(i) => Outcome::Int(*i),
        Value::String(text) => Outcome::Text(TapeBytes::from(text.to_string())),
        Value::Some(inner) => match inner.as_ref() {
            Value::String(text) => Outcome::Some(TapeBytes::from(text.to_string())),
            other => unrecordable(other),
        },
        Value::Array(items) => Outcome::List(
            items
                .iter()
                .map(|item| match item {
                    Value::String(text) => TapeBytes::from(text.to_string()),
                    other => TapeBytes::from(other.to_string_value()),
                })
                .collect(),
        ),
        Value::Adt(adt) => adt_outcome(op, adt),
        other => unrecordable(other),
    }
}

fn adt_outcome(op: CorePrimOp, adt: &AdtValue) -> Outcome {
    let field = |index: usize| adt.fields.get(index);
    match adt.constructor.as_str() {
        "Ok" => match field(0) {
            Some(inner) => Outcome::Ok(Box::new(value_outcome(op, inner))),
            None => Outcome::Ok(Box::new(Outcome::Unit)),
        },
        "Err" => match field(0) {
            Some(Value::Adt(error)) if error.constructor.as_str() == "IoError" => {
                let kind = match error.fields.get(0) {
                    Some(Value::AdtUnit(name)) => IoKind::from_constructor(name),
                    _ => IoKind::Other,
                };
                Outcome::Err(TapeIoError {
                    kind,
                    message: text_field(error.fields.get(1)),
                    path: text_field(error.fields.get(2)),
                })
            }
            _ => unrecordable(&Value::Adt(Rc::new(adt.clone()))),
        },
        "FileMeta" => Outcome::Meta(FsMeta {
            size: int_field(field(0)),
            modified: int_field(field(1)),
            is_dir: bool_field(field(2)),
            is_file: bool_field(field(3)),
        }),
        "ProcOutput" => Outcome::Proc(ProcOut {
            status: int_field(field(0)),
            stdout: TapeBytes::from(text_field(field(1))),
            stderr: TapeBytes::from(text_field(field(2))),
        }),
        _ => unrecordable(&Value::Adt(Rc::new(adt.clone()))),
    }
}

fn is_unit_result(op: CorePrimOp) -> bool {
    use CorePrimOp::*;
    matches!(
        op,
        WriteFile | FsWriteFile | FsCreateDirAll | FsRemoveFile | FsRemoveDirAll | FsRename
    )
}

fn text_field(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.to_string(),
        Some(other) => other.to_string_value(),
        None => String::new(),
    }
}

fn int_field(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Integer(i)) => *i,
        _ => 0,
    }
}

fn bool_field(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Boolean(true)))
}

/// An effect result this codec does not know. Effect primops only return the
/// shapes above, so reaching this is a compiler bug, reported where it shows.
fn unrecordable(value: &Value) -> Outcome {
    Outcome::Fail(format!(
        "internal error: the effect tape cannot record a {} value",
        value.type_name()
    ))
}

/// The VM value for a recorded outcome.
pub(super) fn to_value(outcome: Outcome) -> Result<Value, String> {
    Ok(match outcome {
        Outcome::Fail(message) => return Err(message),
        Outcome::Unit | Outcome::None => Value::None,
        Outcome::Bool(b) => Value::Boolean(b),
        Outcome::Int(i) => Value::Integer(i),
        Outcome::Text(text) => string(text),
        Outcome::Some(text) => Value::Some(Rc::new(string(text))),
        Outcome::List(items) => {
            Value::Array(items.into_iter().map(string).collect::<Vec<_>>().into())
        }
        Outcome::Ok(inner) => vm_io_ok(to_value(*inner)?),
        Outcome::Err(error) => vm_fs_error(&FsError {
            kind: fs_error_kind(error.kind),
            message: error.message,
            path: error.path,
        }),
        Outcome::Meta(meta) => adt(
            "FileMeta",
            vec![
                Value::Integer(meta.size),
                Value::Integer(meta.modified),
                Value::Boolean(meta.is_dir),
                Value::Boolean(meta.is_file),
            ],
        ),
        Outcome::Proc(proc) => adt(
            "ProcOutput",
            vec![
                Value::Integer(proc.status),
                string(proc.stdout),
                string(proc.stderr),
            ],
        ),
    })
}

fn string(text: TapeBytes) -> Value {
    Value::String(text.lossy().into())
}

fn adt(constructor: &str, fields: Vec<Value>) -> Value {
    Value::Adt(Rc::new(AdtValue {
        constructor: Rc::new(constructor.to_string()),
        fields: AdtFields::from_vec(fields),
    }))
}

fn fs_error_kind(kind: IoKind) -> FsErrorKind {
    match kind {
        IoKind::NotFound => FsErrorKind::NotFound,
        IoKind::PermissionDenied => FsErrorKind::PermissionDenied,
        IoKind::AlreadyExists => FsErrorKind::AlreadyExists,
        IoKind::NotADirectory => FsErrorKind::NotADirectory,
        IoKind::IsADirectory => FsErrorKind::IsADirectory,
        IoKind::DirectoryNotEmpty => FsErrorKind::DirectoryNotEmpty,
        IoKind::Interrupted => FsErrorKind::Interrupted,
        IoKind::Other => FsErrorKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use CorePrimOp::*;

    /// A value survives recording and replay unchanged.
    fn round_trip(op: CorePrimOp, value: Value) {
        let recorded = outcome(op, Ok(value.clone()));
        let json = serde_json::to_string(&recorded).unwrap();
        let loaded: Outcome = serde_json::from_str(&json).unwrap();
        let replayed = to_value(loaded).unwrap();
        assert_eq!(
            format!("{replayed:?}"),
            format!("{value:?}"),
            "{op:?} via {json}"
        );
    }

    #[test]
    fn effect_results_round_trip_through_the_tape() {
        round_trip(ClockNow, Value::Integer(1_700_000_000_123));
        round_trip(FsExists, Value::Boolean(true));
        round_trip(
            ReadStdin,
            Value::String("line one\nline two\n".to_string().into()),
        );
        round_trip(
            EnvVar,
            Value::Some(Rc::new(Value::String("v".to_string().into()))),
        );
        round_trip(EnvVar, Value::None);
        round_trip(
            EnvArgs,
            Value::Array(vec![Value::String("app.flx".to_string().into())].into()),
        );
        round_trip(
            TryReadFile,
            vm_io_ok(Value::String("contents".to_string().into())),
        );
        round_trip(FsWriteFile, vm_io_ok(Value::None));
        round_trip(
            FsMetadata,
            vm_io_ok(adt(
                "FileMeta",
                vec![
                    Value::Integer(12),
                    Value::Integer(99),
                    Value::Boolean(false),
                    Value::Boolean(true),
                ],
            )),
        );
        round_trip(
            ProcRun,
            vm_io_ok(adt(
                "ProcOutput",
                vec![
                    Value::Integer(3),
                    Value::String("out".to_string().into()),
                    Value::String("err".to_string().into()),
                ],
            )),
        );
        for kind in IoKind::ALL {
            round_trip(
                TryReadFile,
                vm_fs_error(&FsError {
                    kind: fs_error_kind(kind),
                    message: "failed".to_string(),
                    path: "in.txt".to_string(),
                }),
            );
        }
    }

    #[test]
    fn a_runtime_error_replays_as_the_same_error() {
        let recorded = outcome(ReadFile, Err("read_file failed for 'x'".to_string()));
        assert_eq!(
            to_value(recorded),
            Err("read_file failed for 'x'".to_string())
        );
    }

    #[test]
    fn write_results_are_unit_and_absent_values_are_none() {
        assert_eq!(outcome(WriteFile, Ok(Value::None)), Outcome::Unit);
        assert_eq!(outcome(EnvHomeDir, Ok(Value::None)), Outcome::None);
    }
}
