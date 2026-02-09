use flux::syntax::diagnostics::{ERROR_CODES, lookup_error_code};

#[test]
fn registry_has_unique_codes() {
    let mut codes = std::collections::HashSet::new();
    for item in ERROR_CODES {
        assert!(
            codes.insert(item.code),
            "duplicate error code in registry: {}",
            item.code
        );
    }
}

#[test]
fn registry_get_finds_codes() {
    for item in ERROR_CODES {
        let found = lookup_error_code(item.code).expect("code missing from registry");
        assert_eq!(found.title, item.title);
    }
}
