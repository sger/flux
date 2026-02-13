use std::rc::Rc;

use crate::runtime::value::Value;

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
}
