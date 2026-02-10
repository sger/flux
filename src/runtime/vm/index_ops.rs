use std::{collections::HashMap, rc::Rc};

use crate::runtime::{hash_key::HashKey, value::Value};

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
            (Value::Hash(hash), _) => self.execute_hash_index(hash, &index),
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
            self.push(Value::Some(Rc::new(elements[index as usize].clone())))
        }
    }

    fn execute_hash_index(
        &mut self,
        hash: &HashMap<HashKey, Value>,
        key: &Value,
    ) -> Result<(), String> {
        let hash_key = key
            .to_hash_key()
            .ok_or_else(|| format!("unusable as hash key: {}", key.type_name()))?;

        match hash.get(&hash_key) {
            Some(value) => self.push(Value::Some(Rc::new(value.clone()))),
            None => self.push(Value::None),
        }
    }
}
