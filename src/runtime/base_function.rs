use std::fmt;

use crate::runtime::BaseFn;

#[derive(Clone)]
pub struct BaseFunction {
    pub name: &'static str,
    pub func: BaseFn,
}

impl fmt::Debug for BaseFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BaseFunction({})", self.name)
    }
}

impl PartialEq for BaseFunction {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}
