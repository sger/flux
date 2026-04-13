use crate::driver::flags::{self, DriverFlags};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Vm,
    Native,
}

impl Backend {
    pub fn select(flags: &DriverFlags) -> Self {
        if flags.use_core_to_llvm || flags.emit_llvm || flags.emit_binary {
            Backend::Native
        } else {
            Backend::Vm
        }
    }
}
