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

/// Review round 34 (docs): the bullets of a rule's `## Options` section
/// name options the rule has, and every page shows each option's default
/// there: from the option metadata (`RuleMeta::documented_explanation`),
/// or, where a bullet states it itself, the same as the metadata's.
#[test]
fn option_defaults_on_the_pages_are_the_metadata_defaults() {
    let normal = |s: &str| -> String {
        s.chars()
            .filter(|c| !c.is_whitespace() && *c != '`' && *c != '"')
            .collect()
    };
    let (mut generated, mut by_hand) = (0, 0);
    for rule in iglint_linter::rules::REGISTRY.iter() {
        let meta = rule.meta();
        // Bullets joined with their continuation lines.
        let mut bullets: Vec<String> = Vec::new();
        let mut in_options = false;
        for line in meta.explanation.lines() {
            if line.starts_with("## ") {
                in_options = line == "## Options";
            } else if in_options && line.starts_with("* ") {
                bullets.push(line.to_owned());
            } else if in_options
                && line.starts_with("  ")
                && let Some(last) = bullets.last_mut()
            {
                last.push(' ');
                last.push_str(line.trim());
            }
        }
        let page = iglint::docs::rule_page(rule);
        for bullet in &bullets {
            let Some((names, rest)) = meta.option_bullet(bullet) else {
                continue;
            };
            for name in &names {
                assert!(
                    meta.option(name).is_some(),
                    "{}: `{name}` in ## Options is not an option",
                    meta.code
                );
            }
            if let Some(paren) = rest.strip_prefix(" (") {
                // `(default X)`, `(regex, default X)`, `(default X, ...)`.
                let inside = paren.split("):").next().unwrap();
                let inside = inside.strip_suffix(')').unwrap_or(inside);
                let stated = inside
                    .split_once("default ")
                    .map(|(_, d)| d)
                    .unwrap_or_else(|| panic!("{}: {bullet}", meta.code));
                let stated = match stated.split_once(", ") {
                    Some((first, _)) if !stated.starts_with('`') => first,
                    _ => stated,
                };
                assert_eq!(names.len(), 1, "{}: {bullet}", meta.code);
                let option = meta.option(names[0]).unwrap();
                assert_eq!(
                    normal(stated),
                    normal(option.default),
                    "{}: `{}` states another default than its metadata",
                    meta.code,
                    option.name
                );
                by_hand += 1;
            } else {
                for name in names {
                    let option = meta.option(name).unwrap();
                    let shown = match option.default.trim() {
                        "" | "none" => format!("`{name}` (unset by default)"),
                        default => format!("`{name}` (default `{default}`)"),
                    };
                    assert!(page.contains(&shown), "{}: {shown}", meta.code);
                    generated += 1;
                }
            }
        }
    }
    assert!(generated > 20 && by_hand > 10, "{generated} {by_hand}");
}
