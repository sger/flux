#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolScope {
    Global,
    Local,
    Free,
    Function,
}
