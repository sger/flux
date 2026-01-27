use std::fmt;

use crate::runtime::BuiltinFn;

#[derive(Clone)]
pub struct BuiltinFunction {
    pub name: &'static str,
    pub func: BuiltinFn,
}

impl fmt::Debug for BuiltinFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BuiltinFucntion({})", self.name)
    }
}

impl PartialEq for BuiltinFunction {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}
