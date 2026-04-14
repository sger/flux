use crate::driver::flags::DriverFlags;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Vm,
    Native,
}

impl Backend {
    pub fn select(flags: &DriverFlags) -> Self {
        if flags.backend.use_llvm || flags.backend.emit_llvm || flags.backend.emit_binary {
            Backend::Native
        } else {
            Backend::Vm
        }
    }
}
