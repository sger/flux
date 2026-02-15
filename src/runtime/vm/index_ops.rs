use crate::runtime::{
    gc::{GcHandle, HeapObject, hamt::hamt_lookup},
    value::Value,
};

use super::VM;

impl VM {
    pub(super) fn execute_index_expression(
        &mut self,
        left: Value,
        index: Value,
    ) -> Result<(), String> {
        match (&left, &index) {
            (Value::Array(elements), Value::Integer(idx)) => {
                self.execute_array_index(elements, *idx)
            }
            (Value::Gc(handle), _) => {
                match &index {
                    Value::Integer(idx) => {
                        // Check if it's a list (Cons) or a map (HamtNode)
                        match self.gc_heap.get(*handle) {
                            HeapObject::Cons { .. } => self.execute_list_index(*handle, *idx),
                            _ => self.execute_hamt_index(*handle, &index),
                        }
                    }
                    _ => self.execute_hamt_index(*handle, &index),
                }
            }
            _ => Err(format!(
                "index operator not supported: {}",
                left.type_name()
            )),
        }
    }

    fn execute_array_index(&mut self, elements: &[Value], index: i64) -> Result<(), String> {
        if index < 0 || index as usize >= elements.len() {
            self.push(Value::None)
        } else {
            self.push(Value::Some(std::rc::Rc::new(elements[index as usize].clone())))
        }
    }

    /// Indexes into a cons-cell list by traversing the spine.
    /// Returns Some(element) or None for out-of-bounds.
    fn execute_list_index(&mut self, handle: GcHandle, index: i64) -> Result<(), String> {
        if index < 0 {
            return self.push(Value::None);
        }

        let mut current = Value::Gc(handle);
        let mut remaining = index as usize;

        loop {
            match &current {
                Value::Gc(h) => match self.gc_heap.get(*h) {
                    HeapObject::Cons { head, tail } => {
                        if remaining == 0 {
                            return self.push(Value::Some(std::rc::Rc::new(head.clone())));
                        }
                        remaining -= 1;
                        current = tail.clone();
                    }
                    _ => return self.push(Value::None),
                },
                _ => return self.push(Value::None),
            }
        }
    }

    fn execute_hamt_index(&mut self, handle: GcHandle, key: &Value) -> Result<(), String> {
        let hash_key = key
            .to_hash_key()
            .ok_or_else(|| format!("unusable as hash key: {}", key.type_name()))?;
        match hamt_lookup(&self.gc_heap, handle, &hash_key) {
            Some(value) => self.push(Value::Some(std::rc::Rc::new(value))),
            None => self.push(Value::None),
        }
    }
}
