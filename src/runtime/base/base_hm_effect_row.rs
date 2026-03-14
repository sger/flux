#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BaseHmEffectRow {
    pub concrete: Vec<&'static str>,
    pub tail: Option<&'static str>,
}
