#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PrimOp{
    IAdd = 0,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimEffect {
    Pure,
    Io,
    Time,
    Control,
}

impl PrimOp {
    pub const COUNT: usize = 50;

    pub fn id(self) -> u8 {
        self as u8
    }

    pub fn from_id(id: u8) -> Option<Self> {
        Some(match id {
            0 => Self::IAdd,
            _ => return None
        })
    }

    pub fn arity(self) -> usize {
        match self {
            Self::IAdd => 2,
        }
    }

    pub fn display_name(self) -> &'static str {
        match  self {
            Self::IAdd => "iadd",
        }
    }
 }