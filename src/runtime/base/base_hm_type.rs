#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaseHmType {
    Any,
    Int,
    Float,
    Bool,
    String,
    Unit,
    TypeVar(&'static str),
    Option(Box<BaseHmType>),
    List(Box<BaseHmType>),
    Array(Box<BaseHmType>),
    Map(Box<BaseHmType>, Box<BaseHmType>),
    Either(Box<BaseHmType>, Box<BaseHmType>),
    Tuple(Vec<BaseHmType>),
    Fun {
        params: Vec<BaseHmType>,
        ret: Box<BaseHmType>,
        effects: BaseHmEffectRow,
    },
}
