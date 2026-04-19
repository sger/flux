#[path = "support/semantic_core_dump.rs"]
mod semantic_core_dump;

use semantic_core_dump::dump_core_debug_fixture;

#[test]
fn debug_core_preserves_polymorphic_function_types_without_dynamic() {
    let dump = dump_core_debug_fixture("functions/polymorphism.flx");
    assert!(
        dump.contains("def id : forall"),
        "expected explicit type-variable residue in Core dump:\n{dump}"
    );
    assert!(
        !dump.contains("Dynamic"),
        "Core dump should not contain semantic Dynamic placeholders:\n{dump}"
    );
}

#[test]
fn debug_core_preserves_generic_adt_shapes_without_dynamic() {
    let dump = dump_core_debug_fixture("adts/generic_adts.flx");
    assert!(
        dump.contains("letrec wrap"),
        "expected generic ADT function in Core dump:\n{dump}"
    );
    assert!(
        dump.contains("Box"),
        "expected named ADT shape to survive lowering:\n{dump}"
    );
    assert!(
        !dump.contains("Dynamic"),
        "Core dump should not contain semantic Dynamic placeholders:\n{dump}"
    );
}

#[test]
fn debug_core_preserves_effectful_function_types_without_dynamic() {
    let dump = dump_core_debug_fixture("effects/basic_handle.flx");
    assert!(
        dump.contains("letrec twice"),
        "expected effectful function in Core dump:\n{dump}"
    );
    assert!(
        !dump.contains("Dynamic"),
        "Core dump should not contain semantic Dynamic placeholders:\n{dump}"
    );
}
