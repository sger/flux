#[cfg(test)]
mod tests {
    use flux::bytecode::{binding::Binding, symbol_scope::SymbolScope, symbol_table::SymbolTable};
    use flux::syntax::interner::Interner;
    use flux::syntax::position::Span;

    fn assert_symbol(
        interner: &Interner,
        sym: Binding,
        name: &str,
        scope: SymbolScope,
        index: usize,
    ) {
        assert_eq!(interner.resolve(sym.name), name);
        assert_eq!(sym.symbol_scope, scope);
        assert_eq!(sym.index, index);
    }

    #[test]
    fn define_resolve() {
        let mut interner = Interner::new();
        let mut global = SymbolTable::new();
        let a = interner.intern("a");
        let b = interner.intern("b");
        global.define(a, Span::default());
        global.define(b, Span::default());

        assert_eq!(global.resolve(a).unwrap().index, 0);
        assert_eq!(global.resolve(b).unwrap().index, 1);
    }

    #[test]
    fn nested_scopes() {
        let mut interner = Interner::new();
        let mut global = SymbolTable::new();
        let a = interner.intern("a");
        let b = interner.intern("b");
        global.define(a, Span::default());

        let mut local = SymbolTable::new_enclosed(global);
        local.define(b, Span::default());

        assert_eq!(local.resolve(a).unwrap().symbol_scope, SymbolScope::Global);
        assert_eq!(local.resolve(b).unwrap().symbol_scope, SymbolScope::Local);
    }

    #[test]
    fn define_global() {
        let mut interner = Interner::new();
        let mut global = SymbolTable::new();

        let a = global.define(interner.intern("a"), Span::default());
        let b = global.define(interner.intern("b"), Span::default());

        assert_symbol(&interner, a, "a", SymbolScope::Global, 0);
        assert_symbol(&interner, b, "b", SymbolScope::Global, 1);
    }

    #[test]
    fn define_local() {
        let mut interner = Interner::new();
        let global = SymbolTable::new();
        let mut local = SymbolTable::new_enclosed(global);

        let a = local.define(interner.intern("a"), Span::default());
        let b = local.define(interner.intern("b"), Span::default());

        assert_symbol(&interner, a, "a", SymbolScope::Local, 0);
        assert_symbol(&interner, b, "b", SymbolScope::Local, 1);
    }

    #[test]
    fn resolve_global() {
        let mut interner = Interner::new();
        let mut global = SymbolTable::new();
        let sym_a = interner.intern("a");
        let sym_b = interner.intern("b");
        global.define(sym_a, Span::default());
        global.define(sym_b, Span::default());

        let a = global.resolve(sym_a).unwrap();
        let b = global.resolve(sym_b).unwrap();

        assert_symbol(&interner, a, "a", SymbolScope::Global, 0);
        assert_symbol(&interner, b, "b", SymbolScope::Global, 1);
    }

    #[test]
    fn resolve_local_and_global() {
        let mut interner = Interner::new();
        let mut global = SymbolTable::new();
        let sym_a = interner.intern("a");
        let sym_b = interner.intern("b");
        let sym_c = interner.intern("c");
        let sym_d = interner.intern("d");
        global.define(sym_a, Span::default());
        global.define(sym_b, Span::default());

        let mut local = SymbolTable::new_enclosed(global);
        local.define(sym_c, Span::default());
        local.define(sym_d, Span::default());

        // locals
        assert_symbol(
            &interner,
            local.resolve(sym_c).unwrap(),
            "c",
            SymbolScope::Local,
            0,
        );
        assert_symbol(
            &interner,
            local.resolve(sym_d).unwrap(),
            "d",
            SymbolScope::Local,
            1,
        );

        // globals should still resolve as globals
        assert_symbol(
            &interner,
            local.resolve(sym_a).unwrap(),
            "a",
            SymbolScope::Global,
            0,
        );
        assert_symbol(
            &interner,
            local.resolve(sym_b).unwrap(),
            "b",
            SymbolScope::Global,
            1,
        );
    }

    #[test]
    fn define_and_resolve_builtin() {
        let mut interner = Interner::new();
        let mut global = SymbolTable::new();
        let sym_len = interner.intern("len");
        let sym_first = interner.intern("first");
        global.define_builtin(0, sym_len);
        global.define_builtin(1, sym_first);

        assert_symbol(
            &interner,
            global.resolve(sym_len).unwrap(),
            "len",
            SymbolScope::Builtin,
            0,
        );
        assert_symbol(
            &interner,
            global.resolve(sym_first).unwrap(),
            "first",
            SymbolScope::Builtin,
            1,
        );

        // builtins should resolve through enclosed scopes too
        let mut local = SymbolTable::new_enclosed(global);
        assert_symbol(
            &interner,
            local.resolve(sym_len).unwrap(),
            "len",
            SymbolScope::Builtin,
            0,
        );
    }

    #[test]
    fn define_function_name() {
        let mut interner = Interner::new();
        let global = SymbolTable::new();
        let mut local = SymbolTable::new_enclosed(global);
        let sym_my_func = interner.intern("myFunc");

        let fn_sym = local.define_function_name(sym_my_func, Span::default());
        assert_symbol(&interner, fn_sym, "myFunc", SymbolScope::Function, 0);

        // should resolve from same scope
        assert_symbol(
            &interner,
            local.resolve(sym_my_func).unwrap(),
            "myFunc",
            SymbolScope::Function,
            0,
        );
    }

    #[test]
    fn resolve_free() {
        // global:
        //   a, b
        // outer (local):
        //   c, d
        // inner (local):
        //   e, f
        // inner references: a, b (globals), c, d (free), e, f (locals)

        let mut interner = Interner::new();
        let sym_a = interner.intern("a");
        let sym_b = interner.intern("b");
        let sym_c = interner.intern("c");
        let sym_d = interner.intern("d");
        let sym_e = interner.intern("e");
        let sym_f = interner.intern("f");

        let mut global = SymbolTable::new();
        global.define(sym_a, Span::default());
        global.define(sym_b, Span::default());

        let mut outer = SymbolTable::new_enclosed(global);
        outer.define(sym_c, Span::default());
        outer.define(sym_d, Span::default());

        let mut inner = SymbolTable::new_enclosed(outer);
        inner.define(sym_e, Span::default());
        inner.define(sym_f, Span::default());

        // locals
        assert_symbol(
            &interner,
            inner.resolve(sym_e).unwrap(),
            "e",
            SymbolScope::Local,
            0,
        );
        assert_symbol(
            &interner,
            inner.resolve(sym_f).unwrap(),
            "f",
            SymbolScope::Local,
            1,
        );

        // globals
        assert_symbol(
            &interner,
            inner.resolve(sym_a).unwrap(),
            "a",
            SymbolScope::Global,
            0,
        );
        assert_symbol(
            &interner,
            inner.resolve(sym_b).unwrap(),
            "b",
            SymbolScope::Global,
            1,
        );

        // frees (captured from outer scope)
        assert_symbol(
            &interner,
            inner.resolve(sym_c).unwrap(),
            "c",
            SymbolScope::Free,
            0,
        );
        assert_symbol(
            &interner,
            inner.resolve(sym_d).unwrap(),
            "d",
            SymbolScope::Free,
            1,
        );

        assert_eq!(inner.free_symbols.len(), 2);
        assert_eq!(interner.resolve(inner.free_symbols[0].name), "c");
        assert_eq!(interner.resolve(inner.free_symbols[1].name), "d");
    }

    #[test]
    fn resolve_unresolvable_free_is_none() {
        let mut interner = Interner::new();
        let global = SymbolTable::new();
        let mut local = SymbolTable::new_enclosed(global);
        let sym_not_defined = interner.intern("not_defined");

        assert!(local.resolve(sym_not_defined).is_none());
    }

    #[test]
    fn resolve_nested_free() {
        // global: a
        // first local: b
        // second local: c
        //
        // second references: a (global), b (free), c (local)

        let mut interner = Interner::new();
        let sym_a = interner.intern("a");
        let sym_b = interner.intern("b");
        let sym_c = interner.intern("c");

        let mut global = SymbolTable::new();
        global.define(sym_a, Span::default());

        let mut first = SymbolTable::new_enclosed(global);
        first.define(sym_b, Span::default());

        let mut second = SymbolTable::new_enclosed(first);
        second.define(sym_c, Span::default());

        assert_symbol(
            &interner,
            second.resolve(sym_a).unwrap(),
            "a",
            SymbolScope::Global,
            0,
        );
        assert_symbol(
            &interner,
            second.resolve(sym_c).unwrap(),
            "c",
            SymbolScope::Local,
            0,
        );

        // b becomes free in second
        assert_symbol(
            &interner,
            second.resolve(sym_b).unwrap(),
            "b",
            SymbolScope::Free,
            0,
        );
        assert_eq!(second.free_symbols.len(), 1);
        assert_eq!(interner.resolve(second.free_symbols[0].name), "b");
        assert_eq!(second.free_symbols[0].symbol_scope, SymbolScope::Local);
    }

    #[test]
    fn free_symbol_is_not_duplicated() {
        let mut interner = Interner::new();
        let sym_a = interner.intern("a");
        let sym_b = interner.intern("b");

        let mut global = SymbolTable::new();
        global.define(sym_a, Span::default());

        let mut outer = SymbolTable::new_enclosed(global);
        outer.define(sym_b, Span::default());

        let mut inner = SymbolTable::new_enclosed(outer);

        let s1 = inner.resolve(sym_b).unwrap();
        let s2 = inner.resolve(sym_b).unwrap();

        assert_eq!(s1, s2);
        assert_eq!(inner.free_symbols.len(), 1);
    }
}
