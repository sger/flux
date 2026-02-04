#[cfg(test)]
mod tests {
    use flux::bytecode::{symbol::Symbol, symbol_scope::SymbolScope, symbol_table::SymbolTable};
    use flux::frontend::position::Span;

    fn assert_symbol(sym: Symbol, name: &str, scope: SymbolScope, index: usize) {
        assert_eq!(sym.name, name);
        assert_eq!(sym.symbol_scope, scope);
        assert_eq!(sym.index, index);
    }

    #[test]
    fn define_resolve() {
        let mut global = SymbolTable::new();
        global.define("a", Span::default());
        global.define("b", Span::default());

        assert_eq!(global.resolve("a").unwrap().index, 0);
        assert_eq!(global.resolve("b").unwrap().index, 1);
    }

    #[test]
    fn nested_scopes() {
        let mut global = SymbolTable::new();
        global.define("a", Span::default());

        let mut local = SymbolTable::new_enclosed(global);
        local.define("b", Span::default());

        assert_eq!(
            local.resolve("a").unwrap().symbol_scope,
            SymbolScope::Global
        );
        assert_eq!(local.resolve("b").unwrap().symbol_scope, SymbolScope::Local);
    }

    #[test]
    fn define_global() {
        let mut global = SymbolTable::new();

        let a = global.define("a", Span::default());
        let b = global.define("b", Span::default());

        assert_symbol(a, "a", SymbolScope::Global, 0);
        assert_symbol(b, "b", SymbolScope::Global, 1);
    }

    #[test]
    fn define_local() {
        let global = SymbolTable::new();
        let mut local = SymbolTable::new_enclosed(global);

        let a = local.define("a", Span::default());
        let b = local.define("b", Span::default());

        assert_symbol(a, "a", SymbolScope::Local, 0);
        assert_symbol(b, "b", SymbolScope::Local, 1);
    }

    #[test]
    fn resolve_global() {
        let mut global = SymbolTable::new();
        global.define("a", Span::default());
        global.define("b", Span::default());

        let a = global.resolve("a").unwrap();
        let b = global.resolve("b").unwrap();

        assert_symbol(a, "a", SymbolScope::Global, 0);
        assert_symbol(b, "b", SymbolScope::Global, 1);
    }

    #[test]
    fn resolve_local_and_global() {
        let mut global = SymbolTable::new();
        global.define("a", Span::default());
        global.define("b", Span::default());

        let mut local = SymbolTable::new_enclosed(global);
        local.define("c", Span::default());
        local.define("d", Span::default());

        // locals
        assert_symbol(local.resolve("c").unwrap(), "c", SymbolScope::Local, 0);
        assert_symbol(local.resolve("d").unwrap(), "d", SymbolScope::Local, 1);

        // globals should still resolve as globals
        assert_symbol(local.resolve("a").unwrap(), "a", SymbolScope::Global, 0);
        assert_symbol(local.resolve("b").unwrap(), "b", SymbolScope::Global, 1);
    }

    #[test]
    fn define_and_resolve_builtin() {
        let mut global = SymbolTable::new();
        global.define_builtin(0, "len");
        global.define_builtin(1, "first");

        assert_symbol(
            global.resolve("len").unwrap(),
            "len",
            SymbolScope::Builtin,
            0,
        );
        assert_symbol(
            global.resolve("first").unwrap(),
            "first",
            SymbolScope::Builtin,
            1,
        );

        // builtins should resolve through enclosed scopes too
        let mut local = SymbolTable::new_enclosed(global);
        assert_symbol(
            local.resolve("len").unwrap(),
            "len",
            SymbolScope::Builtin,
            0,
        );
    }

    #[test]
    fn define_function_name() {
        let global = SymbolTable::new();
        let mut local = SymbolTable::new_enclosed(global);

        let fn_sym = local.define_function_name("myFunc", Span::default());
        assert_symbol(fn_sym, "myFunc", SymbolScope::Function, 0);

        // should resolve from same scope
        assert_symbol(
            local.resolve("myFunc").unwrap(),
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

        let mut global = SymbolTable::new();
        global.define("a", Span::default());
        global.define("b", Span::default());

        let mut outer = SymbolTable::new_enclosed(global);
        outer.define("c", Span::default());
        outer.define("d", Span::default());

        let mut inner = SymbolTable::new_enclosed(outer);
        inner.define("e", Span::default());
        inner.define("f", Span::default());

        // locals
        assert_symbol(inner.resolve("e").unwrap(), "e", SymbolScope::Local, 0);
        assert_symbol(inner.resolve("f").unwrap(), "f", SymbolScope::Local, 1);

        // globals
        assert_symbol(inner.resolve("a").unwrap(), "a", SymbolScope::Global, 0);
        assert_symbol(inner.resolve("b").unwrap(), "b", SymbolScope::Global, 1);

        // frees (captured from outer scope)
        assert_symbol(inner.resolve("c").unwrap(), "c", SymbolScope::Free, 0);
        assert_symbol(inner.resolve("d").unwrap(), "d", SymbolScope::Free, 1);

        assert_eq!(inner.free_symbols.len(), 2);
        assert_eq!(inner.free_symbols[0].name, "c");
        assert_eq!(inner.free_symbols[1].name, "d");
    }

    #[test]
    fn resolve_unresolvable_free_is_none() {
        let global = SymbolTable::new();
        let mut local = SymbolTable::new_enclosed(global);

        assert!(local.resolve("not_defined").is_none());
    }

    #[test]
    fn resolve_nested_free() {
        // global: a
        // first local: b
        // second local: c
        //
        // second references: a (global), b (free), c (local)

        let mut global = SymbolTable::new();
        global.define("a", Span::default());

        let mut first = SymbolTable::new_enclosed(global);
        first.define("b", Span::default());

        let mut second = SymbolTable::new_enclosed(first);
        second.define("c", Span::default());

        assert_symbol(second.resolve("a").unwrap(), "a", SymbolScope::Global, 0);
        assert_symbol(second.resolve("c").unwrap(), "c", SymbolScope::Local, 0);

        // b becomes free in second
        assert_symbol(second.resolve("b").unwrap(), "b", SymbolScope::Free, 0);
        assert_eq!(second.free_symbols.len(), 1);
        assert_eq!(second.free_symbols[0].name, "b");
        assert_eq!(second.free_symbols[0].symbol_scope, SymbolScope::Local);
    }

    #[test]
    fn free_symbol_is_not_duplicated() {
        let mut global = SymbolTable::new();
        global.define("a", Span::default());

        let mut outer = SymbolTable::new_enclosed(global);
        outer.define("b", Span::default());

        let mut inner = SymbolTable::new_enclosed(outer);

        let s1 = inner.resolve("b").unwrap();
        let s2 = inner.resolve("b").unwrap();

        assert_eq!(s1, s2);
        assert_eq!(inner.free_symbols.len(), 1);
    }
}
