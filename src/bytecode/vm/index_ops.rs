use crate::runtime::{hamt, value::Value};

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
            (Value::Tuple(elements), Value::Integer(idx)) => {
                self.execute_array_index(elements, *idx)
            }
            (Value::Cons(_), Value::Integer(idx)) => self.execute_cons_list_index(&left, *idx),
            (Value::HashMap(node), _) => self.execute_rc_hamt_index(node, &index),
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
            self.push(Value::Some(std::rc::Rc::new(
                elements[index as usize].clone(),
            )))
        }
    }

    /// Indexes into a cons-cell list (Value::Cons) by traversing the spine.
    /// Returns Some(element) or None for out-of-bounds.
    fn execute_cons_list_index(&mut self, value: &Value, index: i64) -> Result<(), String> {
        if index < 0 {
            return self.push(Value::None);
        }

        let mut current = value.clone();
        let mut remaining = index as usize;

        loop {
            match &current {
                Value::Cons(cell) => {
                    if remaining == 0 {
                        return self.push(Value::Some(std::rc::Rc::new(cell.head.clone())));
                    }
                    remaining -= 1;
                    current = cell.tail.clone();
                }
                _ => return self.push(Value::None),
            }
        }
    }

    fn execute_rc_hamt_index(
        &mut self,
        node: &crate::runtime::hamt::HamtNode,
        key: &Value,
    ) -> Result<(), String> {
        let hash_key = key
            .to_hash_key()
            .ok_or_else(|| format!("unusable as hash key: {}", key.type_name()))?;
        match hamt::hamt_lookup(node, &hash_key) {
            Some(value) => self.push(Value::Some(std::rc::Rc::new(value))),
            None => self.push(Value::None),
        }
    }
}
