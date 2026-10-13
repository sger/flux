//! Showing runtime values to the person debugging: a bounded one-line
//! rendering, and the children of compound values, loaded on demand.

use crate::runtime::value::Value;

/// How much of a value one line shows before it is cut.
const RENDER_LIMIT: usize = 120;

/// A value in a paused program.
#[derive(Debug, Clone)]
pub struct ValueView {
    value: Value,
}

impl ValueView {
    pub(crate) fn new(value: Value) -> Self {
        Self { value }
    }

    /// One line, cut at a fixed length. Floats keep their decimal point,
    /// so `2.0` does not read as the integer `2`.
    pub fn render(&self) -> String {
        let mut out = String::new();
        render_into(&self.value, &mut out);
        if out.chars().count() <= RENDER_LIMIT {
            return out;
        }
        let mut cut: String = out.chars().take(RENDER_LIMIT).collect();
        cut.push('…');
        cut
    }

    /// The value's runtime type, for display.
    pub fn type_name(&self) -> &'static str {
        self.value.type_name()
    }

    /// The parts of a compound value, labelled as `print` addresses them
    /// (`0`, `1`, …), or none for a scalar. A list shows at most
    /// `limit` elements.
    pub fn children(&self, limit: usize) -> Vec<(String, ValueView)> {
        let items: Vec<Value> = match &self.value {
            Value::Some(inner) | Value::Left(inner) | Value::Right(inner) => {
                vec![(**inner).clone()]
            }
            Value::Tuple(items) | Value::Array(items) => {
                items.iter().take(limit).cloned().collect()
            }
            Value::Adt(adt) => adt.fields.iter().take(limit).cloned().collect(),
            Value::Cons(_) => list_items(&self.value, limit),
            _ => Vec::new(),
        };
        items
            .into_iter()
            .enumerate()
            .map(|(index, value)| (index.to_string(), ValueView::new(value)))
            .collect()
    }

    /// The child `label` of this value (see [`ValueView::children`]).
    pub fn child(&self, label: &str) -> Option<ValueView> {
        let index: usize = label.parse().ok()?;
        self.children(index + 1)
            .into_iter()
            .nth(index)
            .map(|(_, view)| view)
    }

    /// Whether the value has parts to look into.
    pub fn has_children(&self) -> bool {
        !self.children(1).is_empty()
    }
}

fn list_items(list: &Value, limit: usize) -> Vec<Value> {
    let mut items = Vec::new();
    let mut current = list.clone();
    while items.len() < limit {
        let Value::Cons(cell) = current else {
            break;
        };
        items.push(cell.head.clone());
        current = cell.tail.clone();
    }
    items
}

/// Appends `value` to `out`, stopping once `out` is past the line limit.
fn render_into(value: &Value, out: &mut String) {
    if out.len() > RENDER_LIMIT * 4 {
        return;
    }
    let seq = |items: &mut dyn Iterator<Item = &Value>, out: &mut String| {
        for (index, item) in items.enumerate() {
            if index > 0 {
                out.push_str(", ");
            }
            render_into(item, out);
            if out.len() > RENDER_LIMIT * 4 {
                break;
            }
        }
    };
    match value {
        Value::Float(f) => {
            let text = format!("{f:?}");
            out.push_str(&text);
        }
        Value::Some(inner) => wrap(out, "Some", inner),
        Value::Left(inner) => wrap(out, "Left", inner),
        Value::Right(inner) => wrap(out, "Right", inner),
        Value::Tuple(items) => {
            out.push('(');
            seq(&mut items.iter(), out);
            if items.len() == 1 {
                out.push(',');
            }
            out.push(')');
        }
        Value::Array(items) => {
            out.push_str("[|");
            seq(&mut items.iter(), out);
            out.push_str("|]");
        }
        Value::Cons(_) => {
            out.push('[');
            let items = list_items(value, usize::MAX);
            seq(&mut items.iter(), out);
            out.push(']');
        }
        Value::Adt(adt) => {
            out.push_str(&adt.constructor);
            out.push('(');
            seq(&mut adt.fields.iter(), out);
            out.push(')');
        }
        other => out.push_str(&other.to_string()),
    }
}

fn wrap(out: &mut String, name: &str, inner: &Value) {
    out.push_str(name);
    out.push('(');
    render_into(inner, out);
    out.push(')');
}
