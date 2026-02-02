use flux::frontend::error_codes_registry::{get_enhanced, ERROR_CODES};

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
        let found = get_enhanced(item.code).expect("code missing from registry");
        assert_eq!(found.title, item.title);
    }
}
