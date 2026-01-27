use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HashKey {
    Integer(i64),
    Boolean(bool),
    String(String),
}

impl fmt::Display for HashKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HashKey::Integer(v) => write!(f, "{}", v),
            HashKey::Boolean(v) => write!(f, "{}", v),
            HashKey::String(v) => write!(f, "\"{}\"", v),
        }
    }
}
