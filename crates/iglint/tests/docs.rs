//! The committed rule reference in `docs/rules/` matches the rule metadata.

use std::path::Path;

#[test]
fn rule_docs_are_up_to_date() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/rules");
    let problems = iglint::docs::stale(&iglint_linter::rules::REGISTRY, &dir);
    assert!(
        problems.is_empty(),
        "docs/rules is stale; run `cargo run -p iglint -- generate-docs`:\n  {}",
        problems.join("\n  ")
    );
}

#[test]
fn every_rule_has_a_page_and_an_index_entry() {
    let registry = &iglint_linter::rules::REGISTRY;
    let files = iglint::docs::generate(registry);
    assert_eq!(files.len(), registry.len() + 1);
    let index = iglint::docs::index(registry);
    for rule in registry.iter() {
        assert!(index.contains(&format!("[{0}]({0}.md)", rule.code())));
    }
}
