use crate::runtime::{RuntimeContext, value::Value};

/// Primitive operations that can be invoked directly from VM bytecode.
///
/// IDs are encoded in bytecode, so existing discriminants must remain stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PrimOp {
    /// Integer addition: `Int x Int -> Int`.
    IAdd = 0,
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
            _ => return None,
        })
    }

    /// Returns the fixed argument count for this operation.
    pub fn arity(self) -> usize {
        match self {
            Self::IAdd => 2,
        }
    }

    /// Human-readable name used in diagnostics and traces.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::IAdd => "iadd",
        }
    }
}

/// Executes a primitive operation with VM values.
///
/// Arity is validated here to keep direct-call paths and opcode paths consistent.
pub fn execute_primop(
    _ctx: &mut dyn RuntimeContext,
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
        PrimOp::IAdd => int2(args, |a, b| Value::Integer(a + b), op),
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

/// Extracts an integer operand or produces a typed primop error.
fn expect_int(value: &Value, op: PrimOp) -> Result<i64, String> {
    match value {
        Value::Integer(v) => Ok(*v),
        other => Err(type_error(op, "Int", other)),
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
