macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub u32);
    };
}

id_type!(IrVar);
id_type!(BlockId);
id_type!(FunctionId);
id_type!(GlobalId);
id_type!(LiteralId);
id_type!(AdtId);
id_type!(EffectId);
