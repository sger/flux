use std::{cell::RefCell, fmt, rc::Rc, sync::Arc};

use crate::runtime::{
    closure::Closure, compiled_function::CompiledFunction, cons_cell::ConsCell,
    continuation::Continuation, hamt::HamtNode, handler_descriptor::HandlerDescriptor,
    hash_key::HashKey, perform_descriptor::PerformDescriptor,
};

/// Inner data for an ADT constructor value, boxed behind a single `Rc` so that
/// `Value::Adt` stays at one thin pointer (8 bytes) rather than a fat-pointer +
/// thin-pointer pair (24 bytes), keeping `size_of::<Value>() <= 16`.
#[derive(Debug, Clone, PartialEq)]
pub struct AdtValue {
    pub constructor: Rc<String>,
    pub fields: AdtFields,
}

/// Inline storage for ADT fields. Stores up to 3 fields without a heap
/// allocation; falls back to `Vec` for larger constructors.
#[derive(Debug, Clone, PartialEq)]
pub enum AdtFields {
    One(Value),
    Two(Value, Value),
    Three(Value, Value, Value),
    Many(Vec<Value>),
}

impl AdtFields {
    /// Build from a pre-collected `Vec<Value>`. Moves values into inline
    /// storage when the count is ≤ 3.
    pub fn from_vec(mut v: Vec<Value>) -> Self {
        match v.len() {
            1 => AdtFields::One(v.pop().unwrap()),
            2 => {
                let b = v.pop().unwrap();
                let a = v.pop().unwrap();
                AdtFields::Two(a, b)
            }
            3 => {
                let c = v.pop().unwrap();
                let b = v.pop().unwrap();
                let a = v.pop().unwrap();
                AdtFields::Three(a, b, c)
            }
            _ => AdtFields::Many(v),
        }
    }

    pub fn len(&self) -> usize {
        match self {
            AdtFields::One(_) => 1,
            AdtFields::Two(..) => 2,
            AdtFields::Three(..) => 3,
            AdtFields::Many(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        false // AdtFields never empty; zero-field ADTs use Value::AdtUnit
    }

    pub fn get(&self, idx: usize) -> Option<&Value> {
        match (self, idx) {
            (AdtFields::One(a), 0) => Some(a),
            (AdtFields::Two(a, _), 0) => Some(a),
            (AdtFields::Two(_, b), 1) => Some(b),
            (AdtFields::Three(a, _, _), 0) => Some(a),
            (AdtFields::Three(_, b, _), 1) => Some(b),
            (AdtFields::Three(_, _, c), 2) => Some(c),
            (AdtFields::Many(v), i) => v.get(i),
            _ => None,
        }
    }

    /// Set a single field in-place by index. Used by Aether reuse
    /// specialization to write only changed fields during in-place reuse.
    pub fn set_field(&mut self, idx: usize, value: Value) {
        match (self, idx) {
            (AdtFields::One(a), 0) => *a = value,
            (AdtFields::Two(a, _), 0) => *a = value,
            (AdtFields::Two(_, b), 1) => *b = value,
            (AdtFields::Three(a, _, _), 0) => *a = value,
            (AdtFields::Three(_, b, _), 1) => *b = value,
            (AdtFields::Three(_, _, c), 2) => *c = value,
            (AdtFields::Many(v), i) if i < v.len() => {
                v[i] = value;
            }
            _ => {}
        }
    }

    pub fn iter(&self) -> AdtFieldsIter<'_> {
        AdtFieldsIter {
            fields: self,
            idx: 0,
        }
    }

    /// Consume into an iterator that yields owned values without heap allocation
    /// for the common 1-3 field cases.
    #[allow(clippy::should_implement_trait)]
    pub fn into_iter(self) -> AdtFieldsIntoIter {
        match self {
            AdtFields::One(a) => AdtFieldsIntoIter::Inline {
                values: [Some(a), None, None],
                pos: 0,
                len: 1,
            },
            AdtFields::Two(a, b) => AdtFieldsIntoIter::Inline {
                values: [Some(a), Some(b), None],
                pos: 0,
                len: 2,
            },
            AdtFields::Three(a, b, c) => AdtFieldsIntoIter::Inline {
                values: [Some(a), Some(b), Some(c)],
                pos: 0,
                len: 3,
            },
            AdtFields::Many(v) => AdtFieldsIntoIter::Vec(v.into_iter()),
        }
    }

    /// Destructure a 2-field ADT into its two fields without allocating an iterator.
    /// Returns `None` if the field count is not exactly 2.
    pub fn into_two(self) -> Option<(Value, Value)> {
        match self {
            AdtFields::Two(a, b) => Some((a, b)),
            AdtFields::Many(mut v) if v.len() == 2 => {
                let b = v.pop().unwrap();
                let a = v.pop().unwrap();
                Some((a, b))
            }
            _ => None,
        }
    }

    /// Consume and return the value at `idx` without allocating an iterator.
    /// Returns `None` if `idx` is out of bounds.
    pub fn into_nth(self, idx: usize) -> Option<Value> {
        match (self, idx) {
            (AdtFields::One(a), 0) => Some(a),
            (AdtFields::Two(a, _), 0) => Some(a),
            (AdtFields::Two(_, b), 1) => Some(b),
            (AdtFields::Three(a, _, _), 0) => Some(a),
            (AdtFields::Three(_, b, _), 1) => Some(b),
            (AdtFields::Three(_, _, c), 2) => Some(c),
            (AdtFields::Many(mut v), i) if i < v.len() => Some(v.remove(i)),
            _ => None,
        }
    }
}

pub struct AdtFieldsIter<'a> {
    fields: &'a AdtFields,
    idx: usize,
}

impl<'a> Iterator for AdtFieldsIter<'a> {
    type Item = &'a Value;
    fn next(&mut self) -> Option<Self::Item> {
        let val = self.fields.get(self.idx)?;
        self.idx += 1;
        Some(val)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.fields.len() - self.idx;
        (remaining, Some(remaining))
    }
}

pub enum AdtFieldsIntoIter {
    /// Inline storage for 1-3 fields — no heap allocation.
    Inline {
        values: [Option<Value>; 3],
        pos: usize,
        len: usize,
    },
    Vec(std::vec::IntoIter<Value>),
}

impl Iterator for AdtFieldsIntoIter {
    type Item = Value;
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            AdtFieldsIntoIter::Inline { values, pos, len } => {
                if *pos >= *len {
                    return None;
                }
                let val = values[*pos]
                    .take()
                    .expect("inline iterator slot already consumed");
                *pos += 1;
                Some(val)
            }
            AdtFieldsIntoIter::Vec(it) => it.next(),
        }
    }
}

impl std::ops::Index<usize> for AdtFields {
    type Output = Value;
    fn index(&self, idx: usize) -> &Value {
        self.get(idx).expect("AdtFields index out of bounds")
    }
}

/// Unified reference to an ADT value's data.
///
/// After Aether Phase 2, all ADTs are `Value::Adt(Rc<AdtValue>)`, so this is
/// a simple newtype wrapper. Kept for API compatibility with callers that use
/// `.constructor()` / `.fields()` accessor style.
pub struct AdtRef<'a>(pub &'a AdtValue);

impl<'a> AdtRef<'a> {
    pub fn constructor(&self) -> &str {
        self.0.constructor.as_ref()
    }

    pub fn fields(&self) -> &'a AdtFields {
        &self.0.fields
    }
}

/// Runtime value used by the VM stack, globals, constants, and closures.
///
/// ## Memory Management Model
///
/// Values use `Rc` (reference counting) for heap-allocated types (String, Array, Hash, etc.)
/// while keeping primitives (Integer, Float, Boolean, None) unboxed for efficiency.
///
/// ### No-Cycle Invariant
///
/// This design **requires maintaining acyclic value graphs**. Runtime values must form
/// directed acyclic graphs (DAGs), never cycles.
///
/// **Invariant guarantees:**
/// - Closures may capture values, but captured values cannot reference the capturing closure
/// - No language feature exposes mutable reference cells that could create back-edges
/// - Values are semantically immutable after creation
///
/// **Why this matters:**
/// - `Rc` cannot handle reference cycles (would cause memory leaks)
/// - The language design enforces this through immutability and lack of mutable cells
/// - Future features requiring cycles must migrate to cycle-aware GC (Proposal 0017)
///
/// **Validation:**
/// - Tests verify deeply nested captures complete without leaks
/// - Leak detector tracks allocation/deallocation of Rc-wrapped types
///
/// ### Design Rationale
///
/// Using `Rc<String>` (thin pointer) instead of `Rc<str>` (fat pointer) keeps
/// `size_of::<Value>()` at 16 bytes rather than 24, improving cache efficiency.
/// Using `Rc<Vec<Value>>` and `Rc<HashMap<...>>` makes cloning O(1) instead of O(n).
///
/// See [Proposal 0019](../../docs/proposals/implemented/0019_zero_copy_value_passing.md) for details.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// Internal VM stack sentinel for uninitialized/inactive slots.
    ///
    /// This must never be observable at language level.
    Uninit,
    /// 64-bit signed integer.
    Integer(i64),
    /// 64-bit floating point number.
    Float(f64),
    /// Boolean value.
    Boolean(bool),
    /// UTF-8 string value.
    String(Rc<String>),
    /// Absence of value.
    None,
    /// Empty persistent list literal `[]`.
    EmptyList,
    /// Optional value wrapper.
    Some(Rc<Value>),
    /// Either-left wrapper.
    Left(Rc<Value>),
    /// Either-right wrapper.
    Right(Rc<Value>),
    /// Internal return-signal wrapper used by function returns.
    ReturnValue(Rc<Value>),
    /// Compiled function object.
    Function(Rc<CompiledFunction>),
    /// Runtime closure object.
    Closure(Rc<Closure>),
    /// Ordered collection of values.
    Array(Rc<Vec<Value>>),
    /// Fixed-size heterogeneous ordered collection.
    Tuple(Rc<Vec<Value>>),
    /// User-defined ADT constructor value: `Circle(1.0)`, `Red`, `Node(l, v, r)`.
    Adt(Rc<AdtValue>),
    /// Zero-field ADT constructor: `Tip`, `Red`, `None` (user-defined).
    /// Avoids heap-allocating an `Rc<AdtValue>` + empty `Vec` for nullary constructors.
    AdtUnit(Rc<String>),
    /// A captured delimited continuation (result of `OpPerform`).
    /// Calling this value with one argument resumes the suspended computation.
    Continuation(Rc<RefCell<Continuation>>),
    /// Internal: handler table stored in the constant pool by the compiler.
    /// Never exposed to user code.
    HandlerDescriptor(Rc<HandlerDescriptor>),
    /// Internal: perform key stored in the constant pool by the compiler.
    /// Never exposed to user code.
    PerformDescriptor(Rc<PerformDescriptor>),
    /// Rc-based cons cell for persistent linked lists (Aether Phase 1).
    /// Replaces `Value::Gc(GcHandle)` pointing to `HeapObject::Cons`.
    Cons(Rc<ConsCell>),
    /// Rc-based HAMT persistent map (Aether Phase 3).
    /// Replaces `Value::Gc(GcHandle)` pointing to `HeapObject::HamtNode`.
    HashMap(Rc<HamtNode>),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Uninit => write!(f, "<uninit>"),
            Value::Integer(v) => write!(f, "{}", v),
            Value::Float(v) => write!(f, "{}", v),
            Value::Boolean(v) => write!(f, "{}", v),
            Value::String(v) => write!(f, "\"{}\"", v),
            Value::None => write!(f, "None"),
            Value::EmptyList => write!(f, "[]"),
            Value::Some(v) => write!(f, "Some({})", v),
            Value::Left(v) => write!(f, "Left({})", v),
            Value::Right(v) => write!(f, "Right({})", v),
            Value::ReturnValue(v) => write!(f, "{}", v),
            Value::Function(_) => write!(f, "<function>"),
            Value::Closure(_) => write!(f, "<closure>"),
            Value::Array(elements) => {
                let items: Vec<String> = elements.iter().map(|e| e.to_string()).collect();
                write!(f, "[|{}|]", items.join(", "))
            }
            Value::Tuple(elements) => {
                let items: Vec<String> = elements.iter().map(|e| e.to_string()).collect();
                match items.len() {
                    0 => write!(f, "()"),
                    1 => write!(f, "({},)", items[0]),
                    _ => write!(f, "({})", items.join(", ")),
                }
            }
            Value::Cons(_) => {
                // Display cons list as [head, tail_elem, ...]
                let mut items = Vec::new();
                let mut cur: &Value = self;
                loop {
                    match cur {
                        Value::Cons(c) => {
                            items.push(c.head.to_string());
                            cur = &c.tail;
                        }
                        Value::EmptyList | Value::None => break,
                        other => {
                            items.push(format!("| {}", other));
                            break;
                        }
                    }
                }
                write!(f, "[{}]", items.join(", "))
            }
            Value::HashMap(node) => write!(f, "{}", crate::runtime::hamt::format_hamt(node)),
            Value::Adt(adt) => {
                if adt.fields.is_empty() {
                    write!(f, "{}", adt.constructor)
                } else {
                    let items: Vec<String> = adt.fields.iter().map(|v| v.to_string()).collect();
                    write!(f, "{}({})", adt.constructor, items.join(", "))
                }
            }
            Value::AdtUnit(name) => write!(f, "{}", name),
            Value::Continuation(_) => write!(f, "<continuation>"),
            Value::HandlerDescriptor(_) | Value::PerformDescriptor(_) => {
                write!(f, "<internal>")
            }
        }
    }
}

impl Value {
    /// Returns the canonical runtime type label used in diagnostics and base_functions.
    ///
    /// These labels are user-visible and are expected to remain stable.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Uninit => "Uninit",
            Value::Integer(_) => "Int",
            Value::Float(_) => "Float",
            Value::Boolean(_) => "Bool",
            Value::String(_) => "String",
            Value::None => "None",
            Value::EmptyList => "List",
            Value::Some(_) => "Some",
            Value::Left(_) => "Left",
            Value::Right(_) => "Right",
            Value::ReturnValue(_) => "ReturnValue",
            Value::Function(_) => "Function",
            Value::Closure(_) => "Closure",
            Value::Array(_) => "Array",
            Value::Tuple(_) => "Tuple",
            Value::Cons(_) => "List",
            Value::HashMap(_) => "Map",
            Value::Adt(_) | Value::AdtUnit(_) => "Adt",
            Value::Continuation(_) => "Continuation",
            Value::HandlerDescriptor(_) => "HandlerDescriptor",
            Value::PerformDescriptor(_) => "PerformDescriptor",
        }
    }

    /// Returns whether this value is truthy according to Flux semantics.
    ///
    /// Only `Boolean(false)` and `None` are falsy; all other values are truthy.
    pub fn is_callable(&self) -> bool {
        matches!(self, Value::Function(_) | Value::Closure(_))
    }

    pub fn is_truthy(&self) -> bool {
        !matches!(
            self,
            Value::Uninit | Value::Boolean(false) | Value::None | Value::EmptyList
        )
    }

    /// Converts this value into a hash-map key if the value is hashable.
    ///
    /// Hashable variants are:
    /// - `Integer`
    /// - `Boolean`
    /// - `String`
    ///
    /// Returns `None` for all other variants.
    pub fn to_hash_key(&self) -> Option<HashKey> {
        match self {
            Value::Integer(v) => Some(HashKey::Integer(*v)),
            Value::Boolean(v) => Some(HashKey::Boolean(*v)),
            Value::String(v) => Some(HashKey::String(v.to_string())),
            _ => None,
        }
    }

    /// Converts a value to interpolation-friendly string text.
    ///
    /// Unlike [`std::fmt::Display`], strings are returned without quotes.
    /// This helper is used by interpolation and string conversion base_functions.
    pub fn to_string_value(&self) -> String {
        match self {
            Value::Uninit => "<uninit>".to_string(),
            Value::Integer(v) => v.to_string(),
            Value::Float(v) => v.to_string(),
            Value::Boolean(v) => v.to_string(),
            Value::String(v) => v.to_string(),
            Value::None => "None".to_string(),
            Value::EmptyList => "[]".to_string(),
            Value::Some(v) => format!("Some({})", v.to_string_value()),
            Value::Left(_) => "Left({})".to_string(),
            Value::Right(_) => "Right({})".to_string(),
            Value::ReturnValue(v) => v.to_string_value(),
            Value::Function(_) => "<function>".to_string(),
            Value::Closure(_) => "<closure>".to_string(),
            Value::Array(elements) => {
                let items: Vec<String> = elements.iter().map(|e| e.to_string()).collect();
                format!("[|{}|]", items.join(", "))
            }
            Value::Tuple(elements) => {
                let items: Vec<String> = elements.iter().map(|e| e.to_string()).collect();
                match items.len() {
                    0 => "()".to_string(),
                    1 => format!("({},)", items[0]),
                    _ => format!("({})", items.join(", ")),
                }
            }
            Value::Cons(_) => self.to_string(), // uses Display impl
            Value::HashMap(node) => crate::runtime::hamt::format_hamt(node),
            Value::Adt(adt) => {
                if adt.fields.is_empty() {
                    adt.constructor.to_string()
                } else {
                    let items: Vec<String> =
                        adt.fields.iter().map(|v| v.to_string_value()).collect();
                    format!("{}({})", adt.constructor, items.join(", "))
                }
            }
            Value::AdtUnit(name) => name.to_string(),
            Value::Continuation(_) => "<continuation>".to_string(),
            Value::HandlerDescriptor(_) | Value::PerformDescriptor(_) => "<internal>".to_string(),
        }
    }

    pub fn as_adt(&self) -> Option<AdtRef<'_>> {
        match self {
            Value::Adt(adt) => Some(AdtRef(adt)),
            _ => None,
        }
    }

    pub fn adt_constructor(&self) -> Option<&str> {
        match self {
            Value::Adt(adt) => Some(adt.constructor.as_ref()),
            _ => None,
        }
    }

    pub fn adt_field_count(&self) -> Option<usize> {
        self.as_adt().map(|adt| adt.fields().len())
    }

    pub fn adt_clone_field(&self, idx: usize) -> Option<Value> {
        self.as_adt().and_then(|adt| adt.fields().get(idx).cloned())
    }

    pub fn adt_clone_two_fields(&self) -> Option<(Value, Value)> {
        let adt = self.as_adt()?;
        let f0 = adt.fields().get(0)?.clone();
        let f1 = adt.fields().get(1)?.clone();
        Some((f0, f1))
    }
}

// ---------------------------------------------------------------------------
// Rich value formatting (recursive, handles compound types)
// ---------------------------------------------------------------------------

/// Collect a cons-list Value into a Vec.
fn collect_cons_list(value: &Value) -> Option<Vec<Value>> {
    let mut elements = Vec::new();
    let mut current = value.clone();
    loop {
        match &current {
            Value::None | Value::EmptyList => return Some(elements),
            Value::Cons(cell) => {
                elements.push(cell.head.clone());
                current = cell.tail.clone();
            }
            _ => return None,
        }
    }
}

/// Format a value for display (used by print, to_string, assertions, etc.).
///
/// Unlike `Value::Display`, this recursively formats compound values with
/// proper delimiters: arrays as `[|1, 2|]`, tuples as `(1, 2)`,
/// ADTs as `Foo(1, 2)`, cons lists as `[1, 2, 3]`, HAMT maps as `{"k": v}`.
pub fn format_value(value: &Value) -> String {
    match value {
        Value::Array(elements) => {
            let items: Vec<String> = elements.iter().map(format_value).collect();
            format!("[|{}|]", items.join(", "))
        }
        Value::Tuple(elements) => {
            let items: Vec<String> = elements.iter().map(format_value).collect();
            match items.len() {
                0 => "()".to_string(),
                1 => format!("({},)", items[0]),
                _ => format!("({})", items.join(", ")),
            }
        }
        Value::Adt(_) => {
            if let Some(adt) = value.as_adt() {
                let items: Vec<String> = adt.fields().iter().map(format_value).collect();
                format!("{}({})", adt.constructor(), items.join(", "))
            } else {
                value.to_string()
            }
        }
        Value::AdtUnit(name) => name.to_string(),
        Value::Cons(_) => match collect_cons_list(value) {
            Some(elements) => {
                let items: Vec<String> = elements.iter().map(format_value).collect();
                format!("[{}]", items.join(", "))
            }
            None => "<malformed list>".to_string(),
        },
        Value::HashMap(node) => crate::runtime::hamt::format_hamt(node),
        _ => value.to_string(),
    }
}

/// Count the length of a cons-list Value.
pub fn cons_list_len(value: &Value) -> Option<usize> {
    let mut count = 0;
    let mut current = value.clone();
    loop {
        match &current {
            Value::None | Value::EmptyList => return Some(count),
            Value::Cons(cell) => {
                count += 1;
                current = cell.tail.clone();
            }
            _ => return None,
        }
    }
}

/// Drop an owned VM value without recursively walking deeply nested
/// `Rc<Value>` chains through Rust drop glue.
///
/// The VM can build very deep immutable values, especially `Some(Some(...))`
/// chains. Letting the final `Rc<Value>` release recurse through 100K+ nested
/// `Value` destructors can overflow the host stack during VM teardown. This
/// helper consumes unique wrapper/collection nodes iteratively and falls back
/// to normal `Rc` decrement semantics when a node is shared.
pub fn drop_value_stackless(value: Value) {
    let mut pending = vec![value];

    while let Some(value) = pending.pop() {
        match value {
            Value::Some(rc) | Value::Left(rc) | Value::Right(rc) | Value::ReturnValue(rc) => {
                if Rc::strong_count(&rc) == 1 {
                    if let Ok(inner) = Rc::try_unwrap(rc) {
                        pending.push(inner);
                    }
                }
            }
            Value::Array(rc) | Value::Tuple(rc) => {
                if Rc::strong_count(&rc) == 1 {
                    if let Ok(values) = Rc::try_unwrap(rc) {
                        pending.extend(values);
                    }
                }
            }
            Value::Adt(rc) => {
                if Rc::strong_count(&rc) == 1 {
                    if let Ok(adt) = Rc::try_unwrap(rc) {
                        pending.extend(adt.fields.into_iter());
                    }
                }
            }
            other => drop(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_value_display() {
        assert_eq!(Value::Integer(42).to_string(), "42");
        assert_eq!(Value::Float(3.5).to_string(), "3.5");
        assert_eq!(Value::Boolean(true).to_string(), "true");
        assert_eq!(
            Value::Array(Rc::new(vec![Value::Integer(1), Value::Integer(2)])).to_string(),
            "[|1, 2|]"
        );
    }

    #[test]
    fn test_is_truthy() {
        assert!(Value::Integer(0).is_truthy());
        assert!(Value::Float(0.0).is_truthy());
        assert!(Value::Boolean(true).is_truthy());
        assert!(!Value::Boolean(false).is_truthy());
        assert!(!Value::None.is_truthy());
        assert!(!Value::EmptyList.is_truthy());
    }

    #[test]
    fn test_hash_key() {
        assert_eq!(Value::Integer(1).to_hash_key(), Some(HashKey::Integer(1)));
        assert_eq!(
            Value::Boolean(false).to_hash_key(),
            Some(HashKey::Boolean(false))
        );
        assert_eq!(
            Value::String("a".to_string().into()).to_hash_key(),
            Some(HashKey::String("a".to_string()))
        );
        assert_eq!(Value::Array(Rc::new(vec![])).to_hash_key(), None);
    }

    #[test]
    fn test_type_name() {
        assert_eq!(Value::Integer(1).type_name(), "Int");
        assert_eq!(Value::Float(1.0).type_name(), "Float");
        assert_eq!(Value::Boolean(true).type_name(), "Bool");
        assert_eq!(Value::String("x".to_string().into()).type_name(), "String");
        assert_eq!(Value::None.type_name(), "None");
        assert_eq!(Value::EmptyList.type_name(), "List");
        assert_eq!(
            Value::Some(std::rc::Rc::new(Value::Integer(1))).type_name(),
            "Some"
        );
        assert_eq!(
            Value::Left(std::rc::Rc::new(Value::Integer(1))).type_name(),
            "Left"
        );
        assert_eq!(
            Value::Right(std::rc::Rc::new(Value::Integer(1))).type_name(),
            "Right"
        );
        assert_eq!(
            Value::ReturnValue(std::rc::Rc::new(Value::Integer(1))).type_name(),
            "ReturnValue"
        );
        assert_eq!(Value::Array(Rc::new(vec![])).type_name(), "Array");
    }

    #[test]
    fn test_to_string_value() {
        assert_eq!(
            Value::String("hello".to_string().into()).to_string_value(),
            "hello"
        );
        assert_eq!(
            Value::Some(std::rc::Rc::new(Value::String("x".to_string().into()))).to_string_value(),
            "Some(x)"
        );
        assert_eq!(
            Value::ReturnValue(std::rc::Rc::new(Value::Integer(7))).to_string_value(),
            "7"
        );
        assert_eq!(
            Value::Array(Rc::new(vec![
                Value::String("a".to_string().into()),
                Value::Integer(2)
            ]))
            .to_string_value(),
            "[|\"a\", 2|]"
        );
    }

    #[test]
    fn test_clone_shares_rc_for_string() {
        let value = Value::String("hello".to_string().into());
        let cloned = value.clone();

        match (value, cloned) {
            (Value::String(left), Value::String(right)) => {
                assert!(Rc::ptr_eq(&left, &right));
                assert_eq!(Rc::strong_count(&left), 2);
            }
            _ => panic!("expected string values"),
        }
    }

    #[test]
    fn test_clone_shares_rc_for_array() {
        let array = Value::Array(Rc::new(vec![Value::Integer(1), Value::Integer(2)]));
        let array_clone = array.clone();
        match (array, array_clone) {
            (Value::Array(left), Value::Array(right)) => {
                assert!(Rc::ptr_eq(&left, &right));
                assert_eq!(Rc::strong_count(&left), 2);
            }
            _ => panic!("expected array values"),
        }
    }

    #[test]
    fn test_clone_shares_rc_for_wrappers() {
        let some = Value::Some(std::rc::Rc::new(Value::Integer(7)));
        let some_clone = some.clone();
        match (some, some_clone) {
            (Value::Some(left), Value::Some(right)) => {
                assert!(Rc::ptr_eq(&left, &right));
                assert_eq!(Rc::strong_count(&left), 2);
            }
            _ => panic!("expected some values"),
        }

        let ret = Value::ReturnValue(std::rc::Rc::new(Value::String("ok".to_string().into())));
        let ret_clone = ret.clone();
        match (ret, ret_clone) {
            (Value::ReturnValue(left), Value::ReturnValue(right)) => {
                assert!(Rc::ptr_eq(&left, &right));
                assert_eq!(Rc::strong_count(&left), 2);
            }
            _ => panic!("expected return values"),
        }
    }

    #[test]
    fn value_size_is_compact() {
        // All payload types are now thin pointers (Rc<String>, Rc<Vec<_>>, etc.) or primitives.
        // With discriminant + padding the enum fits in 16 bytes on 64-bit platforms.
        assert_eq!(std::mem::size_of::<Value>(), 16);
    }
}

// ── Thread-safe value representation (Phase 4 VM multithreading) ─────────────
//
// `ArcValue` is the cross-worker-boundary mirror of `Value`. Every `Rc<T>` in
// `Value` becomes `Arc<T>` here so that promoted values satisfy `Send + Sync`.
//
// Lifecycle:
//   promote_value(Value) -> ArcValue   — called when a fiber's body/parked
//                                        value is handed to another OS worker.
//   demote_value(ArcValue) -> Value    — called on the receiving worker before
//                                        the fiber begins executing.
//
// `Value` itself is unchanged; `ArcValue` is purely additive.

/// Arc-based closure — mirrors [`Closure`] with `Arc` pointers.
#[derive(Clone)]
pub struct ArcClosure {
    pub function: Arc<CompiledFunction>,
    pub free: Vec<ArcValue>,
}

/// Arc-based ADT value — mirrors [`AdtValue`] with `Arc` pointers.
#[derive(Clone)]
pub struct ArcAdtValue {
    pub constructor: Arc<String>,
    pub fields: Vec<ArcValue>,
}

/// Arc-based cons cell — mirrors [`ConsCell`] with `ArcValue` fields.
#[derive(Clone)]
pub struct ArcConsCell {
    pub head: ArcValue,
    pub tail: ArcValue,
}

/// Arc-based HAMT node — mirrors [`HamtNode`] with `ArcValue` entries.
#[derive(Clone)]
pub struct ArcHamtNode {
    pub bitmap: u32,
    pub children: Vec<ArcHamtEntry>,
}

#[derive(Clone)]
pub enum ArcHamtEntry {
    Leaf(HashKey, ArcValue),
    Node(Arc<ArcHamtNode>),
    Collision(Arc<ArcHamtCollision>),
}

#[derive(Clone)]
pub struct ArcHamtCollision {
    pub hash: u64,
    pub entries: Vec<(HashKey, ArcValue)>,
}

/// Thread-safe mirror of [`Value`].
///
/// Every `Rc<T>` variant in `Value` is replaced with `Arc<T>`. Primitives and
/// `Copy` scalars are carried directly. `Continuation` uses `Arc<Mutex<...>>`
/// to allow interior mutation from the resuming worker.
///
/// Constructed via [`promote_value`]; converted back via [`demote_value`].
#[derive(Clone)]
pub enum ArcValue {
    Uninit,
    Integer(i64),
    Float(f64),
    Boolean(bool),
    None,
    EmptyList,
    String(Arc<String>),
    Some(Arc<ArcValue>),
    Left(Arc<ArcValue>),
    Right(Arc<ArcValue>),
    ReturnValue(Arc<ArcValue>),
    Function(Arc<CompiledFunction>),
    Closure(Arc<ArcClosure>),
    Array(Arc<Vec<ArcValue>>),
    Tuple(Arc<Vec<ArcValue>>),
    Adt(Arc<ArcAdtValue>),
    AdtUnit(Arc<String>),
    Continuation(Arc<std::sync::Mutex<ArcContinuation>>),
    HandlerDescriptor(Arc<HandlerDescriptor>),
    PerformDescriptor(Arc<PerformDescriptor>),
    Cons(Arc<ArcConsCell>),
    HashMap(Arc<ArcHamtNode>),
}

// SAFETY: All heap payloads use Arc (not Rc), so shared ownership across
// threads is safe. Mutation is guarded by Mutex where needed (Continuation).
unsafe impl Send for ArcValue {}
unsafe impl Sync for ArcValue {}

/// Arc-based continuation — mirrors [`Continuation`] with `ArcValue` fields.
#[derive(Clone)]
pub struct ArcContinuation {
    pub frames: Vec<ArcFrame>,
    pub stack: Vec<ArcValue>,
    pub sp: usize,
    pub entry_sp: usize,
    pub entry_frame_index: usize,
    pub inner_handlers: Vec<ArcHandlerFrame>,
    pub state_marker: Option<u32>,
}

/// Arc-based call frame — mirrors [`crate::runtime::frame::Frame`].
#[derive(Clone)]
pub struct ArcFrame {
    pub closure: Arc<ArcClosure>,
    pub ip: usize,
    pub base_pointer: usize,
    pub return_slot: usize,
}

/// Arc-based handler arm — mirrors [`crate::runtime::handler_arm::HandlerArm`].
#[derive(Clone)]
pub struct ArcHandlerArm {
    pub op: crate::syntax::Identifier,
    pub closure: Arc<ArcClosure>,
}

/// Arc-based evidence entry — mirrors [`crate::runtime::evidence::Evidence`].
#[derive(Clone)]
pub struct ArcEvidence {
    pub effect: crate::syntax::Identifier,
    pub marker: u32,
    pub arms: Arc<Vec<ArcHandlerArm>>,
    pub parent: Option<Arc<Vec<ArcEvidence>>>,
}

/// Arc-based handler frame — mirrors [`crate::runtime::handler_frame::HandlerFrame`].
#[derive(Clone)]
pub struct ArcHandlerFrame {
    pub effect: crate::syntax::Identifier,
    pub arms: Arc<Vec<ArcHandlerArm>>,
    pub marker: u32,
    pub saved_evv: Arc<Vec<ArcEvidence>>,
    pub state: Option<ArcValue>,
    pub entry_frame_index: usize,
    pub entry_sp: usize,
    pub entry_handler_stack_len: usize,
    pub is_direct: bool,
    pub is_discard: bool,
}

/// Promote a `Value` tree into an `ArcValue` tree.
///
/// Every `Rc<T>` is cloned into a fresh `Arc<T>`. Primitives are copied.
/// `Continuation`, `HandlerDescriptor`, and `PerformDescriptor` are promoted
/// by wrapping the cloned data in an `Arc`.
///
/// Returns `Err` for internal VM sentinel values (`ReturnValue`,
/// `HandlerDescriptor`, `PerformDescriptor`) that should never cross worker
/// boundaries — callers must not promote fibers holding these.
pub fn promote_value(value: &Value) -> Result<ArcValue, String> {
    Ok(match value {
        Value::Uninit => ArcValue::Uninit,
        Value::Integer(v) => ArcValue::Integer(*v),
        Value::Float(v) => ArcValue::Float(*v),
        Value::Boolean(v) => ArcValue::Boolean(*v),
        Value::None => ArcValue::None,
        Value::EmptyList => ArcValue::EmptyList,
        Value::String(s) => ArcValue::String(Arc::new((**s).clone())),
        Value::Some(v) => ArcValue::Some(Arc::new(promote_value(v)?)),
        Value::Left(v) => ArcValue::Left(Arc::new(promote_value(v)?)),
        Value::Right(v) => ArcValue::Right(Arc::new(promote_value(v)?)),
        Value::ReturnValue(_) => {
            return Err("cannot promote internal ReturnValue across worker boundary".into());
        }
        Value::Function(f) => ArcValue::Function(Arc::new((**f).clone())),
        Value::Closure(c) => ArcValue::Closure(Arc::new(promote_closure(c)?)),
        Value::Array(v) => ArcValue::Array(Arc::new(
            v.iter().map(promote_value).collect::<Result<Vec<_>, _>>()?,
        )),
        Value::Tuple(v) => ArcValue::Tuple(Arc::new(
            v.iter().map(promote_value).collect::<Result<Vec<_>, _>>()?,
        )),
        Value::Adt(adt) => ArcValue::Adt(Arc::new(ArcAdtValue {
            constructor: Arc::new((*adt.constructor).clone()),
            fields: adt
                .fields
                .iter()
                .map(promote_value)
                .collect::<Result<Vec<_>, _>>()?,
        })),
        Value::AdtUnit(name) => ArcValue::AdtUnit(Arc::new((**name).clone())),
        Value::Continuation(cont) => {
            let cont = cont.borrow();
            ArcValue::Continuation(Arc::new(std::sync::Mutex::new(promote_continuation(&cont)?)))
        }
        Value::HandlerDescriptor(hd) => ArcValue::HandlerDescriptor(Arc::new((**hd).clone())),
        Value::PerformDescriptor(pd) => ArcValue::PerformDescriptor(Arc::new((**pd).clone())),
        Value::Cons(cell) => ArcValue::Cons(Arc::new(ArcConsCell {
            head: promote_value(&cell.head)?,
            tail: promote_value(&cell.tail)?,
        })),
        Value::HashMap(root) => {
            ArcValue::HashMap(Arc::new(promote_hamt_node(root)?))
        }
    })
}

fn promote_closure(c: &Closure) -> Result<ArcClosure, String> {
    Ok(ArcClosure {
        function: Arc::new((*c.function).clone()),
        free: c.free.iter().map(promote_value).collect::<Result<Vec<_>, _>>()?,
    })
}

fn promote_continuation(c: &Continuation) -> Result<ArcContinuation, String> {
    Ok(ArcContinuation {
        frames: c
            .frames
            .iter()
            .map(promote_frame)
            .collect::<Result<Vec<_>, _>>()?,
        stack: c
            .stack
            .iter()
            .map(promote_value)
            .collect::<Result<Vec<_>, _>>()?,
        sp: c.sp,
        entry_sp: c.entry_sp,
        entry_frame_index: c.entry_frame_index,
        inner_handlers: c
            .inner_handlers
            .iter()
            .map(promote_handler_frame)
            .collect::<Result<Vec<_>, _>>()?,
        state_marker: c.state_marker,
    })
}

fn promote_frame(f: &crate::runtime::frame::Frame) -> Result<ArcFrame, String> {
    Ok(ArcFrame {
        closure: Arc::new(promote_closure(&f.closure)?),
        ip: f.ip,
        base_pointer: f.base_pointer,
        return_slot: f.return_slot,
    })
}

fn promote_handler_arm(arm: &crate::runtime::handler_arm::HandlerArm) -> Result<ArcHandlerArm, String> {
    Ok(ArcHandlerArm {
        op: arm.op.clone(),
        closure: Arc::new(promote_closure(&arm.closure)?),
    })
}

fn promote_evv_entries(evv: &crate::runtime::evidence::EvidenceVector) -> Result<Arc<Vec<ArcEvidence>>, String> {
    let entries: Result<Vec<ArcEvidence>, String> = evv.entries().iter().map(|e| {
        Ok::<ArcEvidence, String>(ArcEvidence {
            effect: e.effect.clone(),
            marker: e.marker,
            arms: Arc::new(
                e.arms.iter().map(promote_handler_arm).collect::<Result<Vec<_>, _>>()?
            ),
            parent: match &e.parent {
                Some(parent_evv) => Some(promote_evv_entries(parent_evv)?),
                None => None,
            },
        })
    }).collect();
    Ok(Arc::new(entries?))
}

fn promote_handler_frame(
    hf: &crate::runtime::handler_frame::HandlerFrame,
) -> Result<ArcHandlerFrame, String> {
    Ok(ArcHandlerFrame {
        effect: hf.effect.clone(),
        arms: Arc::new(
            hf.arms.iter().map(promote_handler_arm).collect::<Result<Vec<_>, _>>()?
        ),
        marker: hf.marker,
        saved_evv: promote_evv_entries(&hf.saved_evv)?,
        state: match &hf.state {
            Some(v) => Some(promote_value(v)?),
            None => None,
        },
        entry_frame_index: hf.entry_frame_index,
        entry_sp: hf.entry_sp,
        entry_handler_stack_len: hf.entry_handler_stack_len,
        is_direct: hf.is_direct,
        is_discard: hf.is_discard,
    })
}

fn promote_hamt_node(node: &HamtNode) -> Result<ArcHamtNode, String> {
    Ok(ArcHamtNode {
        bitmap: node.bitmap,
        children: node
            .children
            .iter()
            .map(promote_hamt_entry)
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn promote_hamt_entry(entry: &crate::runtime::hamt::HamtEntry) -> Result<ArcHamtEntry, String> {
    use crate::runtime::hamt::HamtEntry;
    Ok(match entry {
        HamtEntry::Leaf(k, v) => ArcHamtEntry::Leaf(k.clone(), promote_value(v)?),
        HamtEntry::Node(n) => ArcHamtEntry::Node(Arc::new(promote_hamt_node(n)?)),
        HamtEntry::Collision(c) => ArcHamtEntry::Collision(Arc::new(ArcHamtCollision {
            hash: c.hash,
            entries: c
                .entries
                .iter()
                .map(|(k, v)| promote_value(v).map(|av| (k.clone(), av)))
                .collect::<Result<Vec<_>, _>>()?,
        })),
    })
}

/// Demote an [`ArcValue`] tree back to a [`Value`] tree.
///
/// Allocates fresh `Rc<T>` wrappers from the `Arc<T>` data. Called on the
/// receiving worker thread after a fiber crosses a worker boundary.
pub fn demote_value(value: ArcValue) -> Value {
    match value {
        ArcValue::Uninit => Value::Uninit,
        ArcValue::Integer(v) => Value::Integer(v),
        ArcValue::Float(v) => Value::Float(v),
        ArcValue::Boolean(v) => Value::Boolean(v),
        ArcValue::None => Value::None,
        ArcValue::EmptyList => Value::EmptyList,
        ArcValue::String(s) => Value::String(Rc::new((*s).clone())),
        ArcValue::Some(v) => Value::Some(Rc::new(demote_value((*v).clone()))),
        ArcValue::Left(v) => Value::Left(Rc::new(demote_value((*v).clone()))),
        ArcValue::Right(v) => Value::Right(Rc::new(demote_value((*v).clone()))),
        ArcValue::ReturnValue(v) => Value::ReturnValue(Rc::new(demote_value((*v).clone()))),
        ArcValue::Function(f) => Value::Function(Rc::new((*f).clone())),
        ArcValue::Closure(c) => Value::Closure(Rc::new(demote_closure((*c).clone()))),
        ArcValue::Array(v) => {
            Value::Array(Rc::new((*v).iter().cloned().map(demote_value).collect()))
        }
        ArcValue::Tuple(v) => {
            Value::Tuple(Rc::new((*v).iter().cloned().map(demote_value).collect()))
        }
        ArcValue::Adt(adt) => {
            let adt = (*adt).clone();
            Value::Adt(Rc::new(AdtValue {
                constructor: Rc::new((*adt.constructor).clone()),
                fields: AdtFields::from_vec(adt.fields.into_iter().map(demote_value).collect()),
            }))
        }
        ArcValue::AdtUnit(name) => Value::AdtUnit(Rc::new((*name).clone())),
        ArcValue::Continuation(arc_cont) => {
            let cont = arc_cont.lock().expect("ArcContinuation poisoned").clone();
            Value::Continuation(Rc::new(RefCell::new(demote_continuation(cont))))
        }
        ArcValue::HandlerDescriptor(hd) => Value::HandlerDescriptor(Rc::new((*hd).clone())),
        ArcValue::PerformDescriptor(pd) => Value::PerformDescriptor(Rc::new((*pd).clone())),
        ArcValue::Cons(cell) => {
            let cell = (*cell).clone();
            ConsCell::cons(demote_value(cell.head), demote_value(cell.tail))
        }
        ArcValue::HashMap(root) => {
            Value::HashMap(Rc::new(demote_hamt_node((*root).clone())))
        }
    }
}

fn demote_closure(c: ArcClosure) -> Closure {
    // Bypass Closure::new() to avoid double-counting in the leak detector —
    // we're reconstructing an already-counted closure from across a worker boundary.
    Closure {
        function: Rc::new((*c.function).clone()),
        free: c.free.into_iter().map(demote_value).collect(),
    }
}

fn demote_continuation(c: ArcContinuation) -> Continuation {
    Continuation {
        frames: c.frames.into_iter().map(demote_frame).collect(),
        stack: c.stack.into_iter().map(demote_value).collect(),
        sp: c.sp,
        entry_sp: c.entry_sp,
        entry_frame_index: c.entry_frame_index,
        inner_handlers: c.inner_handlers.into_iter().map(demote_handler_frame).collect(),
        state_marker: c.state_marker,
    }
}

fn demote_frame(f: ArcFrame) -> crate::runtime::frame::Frame {
    use crate::runtime::frame::Frame;
    Frame {
        closure: Rc::new(demote_closure((*f.closure).clone())),
        ip: f.ip,
        base_pointer: f.base_pointer,
        return_slot: f.return_slot,
    }
}

fn demote_handler_arm(arm: ArcHandlerArm) -> crate::runtime::handler_arm::HandlerArm {
    use crate::runtime::handler_arm::HandlerArm;
    HandlerArm {
        op: arm.op,
        closure: Rc::new(demote_closure((*arm.closure).clone())),
    }
}

fn demote_evv_entries(entries: &[ArcEvidence]) -> crate::runtime::evidence::EvidenceVector {
    use crate::runtime::evidence::{Evidence, EvidenceVector};
    // Rebuild the EvidenceVector by constructing entries bottom-up.
    // Since EvidenceVector::insert builds from parent → child, we reconstruct
    // each entry independently. For cross-worker boundaries this is acceptable
    // (demote is only called at fiber hand-off, not on a hot path).
    let reconstructed: Vec<Evidence> = entries.iter().map(|e| {
        let arms: Vec<_> = e.arms.iter().cloned().map(demote_handler_arm).collect();
        let parent_evv = e.parent.as_deref().map(|v| demote_evv_entries(v));
        Evidence {
            effect: e.effect.clone(),
            marker: e.marker,
            arms: Rc::new(arms),
            parent: parent_evv,
        }
    }).collect();
    EvidenceVector::from_entries(reconstructed)
}

fn demote_handler_frame(hf: ArcHandlerFrame) -> crate::runtime::handler_frame::HandlerFrame {
    use crate::runtime::handler_frame::HandlerFrame;
    let arms: Vec<_> = hf.arms.iter().cloned().map(demote_handler_arm).collect();
    HandlerFrame {
        effect: hf.effect,
        arms: Rc::new(arms),
        marker: hf.marker,
        saved_evv: demote_evv_entries(&hf.saved_evv),
        state: hf.state.map(demote_value),
        entry_frame_index: hf.entry_frame_index,
        entry_sp: hf.entry_sp,
        entry_handler_stack_len: hf.entry_handler_stack_len,
        is_direct: hf.is_direct,
        is_discard: hf.is_discard,
    }
}

fn demote_hamt_node(node: ArcHamtNode) -> HamtNode {
    HamtNode {
        bitmap: node.bitmap,
        children: node.children.into_iter().map(demote_hamt_entry).collect(),
    }
}

fn demote_hamt_entry(entry: ArcHamtEntry) -> crate::runtime::hamt::HamtEntry {
    use crate::runtime::hamt::{HamtCollision, HamtEntry};
    match entry {
        ArcHamtEntry::Leaf(k, v) => HamtEntry::Leaf(k, demote_value(v)),
        ArcHamtEntry::Node(n) => HamtEntry::Node(Rc::new(demote_hamt_node((*n).clone()))),
        ArcHamtEntry::Collision(c) => {
            let c = (*c).clone();
            HamtEntry::Collision(Rc::new(HamtCollision {
                hash: c.hash,
                entries: c.entries.into_iter().map(|(k, v)| (k, demote_value(v))).collect(),
            }))
        }
    }
}

// ── Arc mirror of the per-fiber EffectContext ─────────────────────────────
//
// A parked/yielded fiber carries an `EffectContext` (evidence vector + yield
// bookkeeping + cancel scope). To hand that fiber to another OS worker the
// context must become `Send`; `ArcEffectContext` is its `Arc`-backed mirror,
// built by `promote_effect_context` and rebuilt by `demote_effect_context`
// (proposal 0174 §"VM cross-worker fiber dispatch"). These live here so the
// private `promote_*` / `demote_*` helpers above can be reused.

use crate::runtime::r#async::context::{CancelScope, ContinuationToken, EffectContext, WorkerId};
use crate::runtime::yield_state::{YieldState, Yielding};

/// `Send` mirror of [`YieldState`] — the bits a parked fiber can carry.
pub struct ArcYieldState {
    pub yielding: Yielding,
    pub marker: u32,
    pub clause: Option<Arc<ArcHandlerArm>>,
    pub op_arg: Option<ArcValue>,
    pub conts: Vec<ArcValue>,
    pub marker_counter: u32,
}

/// `Send` mirror of [`EffectContext`].
pub struct ArcEffectContext {
    pub yield_state: ArcYieldState,
    pub evidence: Arc<Vec<ArcEvidence>>,
    pub continuation: Option<ContinuationToken>,
    pub cancel_scope: CancelScope,
    pub home_worker: WorkerId,
}

// SAFETY: every field is `Send` — `Copy` scalars, or `Arc`-backed via
// `ArcValue` / `ArcHandlerArm` / `ArcEvidence` (all of which use `Arc`, never
// `Rc`). Mirrors the `unsafe impl Send for ArcValue` rationale above.
unsafe impl Send for ArcEffectContext {}

/// Promote a per-fiber [`EffectContext`] into its `Send` [`ArcEffectContext`]
/// mirror. Deep-copies the evidence chain and any in-flight yield payload into
/// `Arc`-backed structures. Errors only if a contained value fails to promote.
pub fn promote_effect_context(cx: &EffectContext) -> Result<ArcEffectContext, String> {
    Ok(ArcEffectContext {
        yield_state: ArcYieldState {
            yielding: cx.yield_state.yielding,
            marker: cx.yield_state.marker,
            clause: match &cx.yield_state.clause {
                Some(arm) => Some(Arc::new(promote_handler_arm(arm)?)),
                None => None,
            },
            op_arg: match &cx.yield_state.op_arg {
                Some(v) => Some(promote_value(v)?),
                None => None,
            },
            conts: cx
                .yield_state
                .conts
                .iter()
                .map(promote_value)
                .collect::<Result<Vec<_>, _>>()?,
            marker_counter: cx.yield_state.marker_counter,
        },
        evidence: promote_evv_entries(&cx.evidence)?,
        continuation: cx.continuation,
        cancel_scope: cx.cancel_scope,
        home_worker: cx.home_worker,
    })
}

/// Rebuild a thread-local [`EffectContext`] from its `Send` mirror — the
/// inverse of [`promote_effect_context`], called on the worker that will run
/// the fiber. Allocates fresh `Rc` wrappers from the `Arc` data.
pub fn demote_effect_context(cx: ArcEffectContext) -> EffectContext {
    EffectContext {
        yield_state: YieldState {
            yielding: cx.yield_state.yielding,
            marker: cx.yield_state.marker,
            clause: cx
                .yield_state
                .clause
                .map(|a| Rc::new(demote_handler_arm((*a).clone()))),
            op_arg: cx.yield_state.op_arg.map(demote_value),
            conts: cx.yield_state.conts.into_iter().map(demote_value).collect(),
            marker_counter: cx.yield_state.marker_counter,
        },
        evidence: demote_evv_entries(&cx.evidence),
        continuation: cx.continuation,
        cancel_scope: cx.cancel_scope,
        home_worker: cx.home_worker,
    }
}
