use std::{cell::RefCell, fmt, rc::Rc};

use crate::runtime::{
    closure::Closure,
    compiled_function::CompiledFunction,
    cons_cell::ConsCell,
    continuation::Continuation,
    gc::{gc_handle::GcHandle, gc_heap::GcHeap},
    hamt::HamtNode,
    handler_descriptor::HandlerDescriptor,
    hash_key::HashKey,
    jit_closure::JitClosure,
    perform_descriptor::PerformDescriptor,
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
    /// JIT-compiled closure object.
    JitClosure(Rc<JitClosure>),
    /// Base function handle (index into base function table).
    BaseFunction(u8),
    /// Ordered collection of values.
    Array(Rc<Vec<Value>>),
    /// Fixed-size heterogeneous ordered collection.
    Tuple(Rc<Vec<Value>>),
    /// GC-managed heap object (cons cell, HAMT map node).
    Gc(GcHandle),
    /// User-defined ADT constructor value: `Circle(1.0)`, `Red`, `Node(l, v, r)`.
    Adt(Rc<AdtValue>),
    /// Zero-field ADT constructor: `Tip`, `Red`, `None` (user-defined).
    /// Avoids heap-allocating an `Rc<AdtValue>` + empty `Vec` for nullary constructors.
    AdtUnit(Rc<String>),
    /// A captured one-shot delimited continuation (result of `OpPerform`).
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
            Value::JitClosure(_) => write!(f, "<jit-closure>"),
            Value::BaseFunction(_) => write!(f, "<base-fn>"),
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
            Value::Gc(handle) => write!(f, "<gc@{}", handle.index()),
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
            Value::JitClosure(_) => "JitClosure",
            Value::BaseFunction(_) => "BaseFunction",
            Value::Array(_) => "Array",
            Value::Tuple(_) => "Tuple",
            Value::Cons(_) => "List",
            Value::HashMap(_) => "Map",
            Value::Gc(_) => "Gc",
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
        matches!(
            self,
            Value::Function(_) | Value::Closure(_) | Value::JitClosure(_) | Value::BaseFunction(_)
        )
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
            Value::JitClosure(_) => "<jit-closure>".to_string(),
            Value::BaseFunction(_) => "<base-fn>".to_string(),
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
            Value::Gc(handle) => format!("<gc@{}>", handle.index()),
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

    pub fn as_adt(&self, _heap: &GcHeap) -> Option<AdtRef<'_>> {
        match self {
            Value::Adt(adt) => Some(AdtRef(adt)),
            _ => None,
        }
    }

    pub fn adt_constructor(&self, _heap: &GcHeap) -> Option<&str> {
        match self {
            Value::Adt(adt) => Some(adt.constructor.as_ref()),
            _ => None,
        }
    }

    pub fn adt_field_count(&self, heap: &GcHeap) -> Option<usize> {
        self.as_adt(heap).map(|adt| adt.fields().len())
    }

    pub fn adt_clone_field(&self, heap: &GcHeap, idx: usize) -> Option<Value> {
        self.as_adt(heap)
            .and_then(|adt| adt.fields().get(idx).cloned())
    }

    pub fn adt_clone_two_fields(&self, heap: &GcHeap) -> Option<(Value, Value)> {
        let adt = self.as_adt(heap)?;
        let f0 = adt.fields().get(0)?.clone();
        let f1 = adt.fields().get(1)?.clone();
        Some((f0, f1))
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
    fn test_clone_shares_gc_handle() {
        let gc = Value::Gc(GcHandle::new_for_test(42));
        let gc_clone = gc.clone();
        assert_eq!(gc, gc_clone);
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
