#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LlvmType {
    Void,
    Integer(u32),
    Float,
    Double,
    Ptr,
    Array {
        len: u64,
        element: Box<LlvmType>,
    },
    Struct {
        packed: bool,
        fields: Vec<LlvmType>,
    },
    Function {
        ret: Box<LlvmType>,
        params: Vec<LlvmType>,
        varargs: bool,
    },
    Named(String),
}

impl LlvmType {
    pub fn i1() -> Self {
        Self::Integer(1)
    }

    pub fn i8() -> Self {
        Self::Integer(8)
    }

    pub fn i32() -> Self {
        Self::Integer(32)
    }

    pub fn i64() -> Self {
        Self::Integer(64)
    }

    pub fn ptr() -> Self {
        Self::Ptr
    }
}
