//! All rules known to iglint.
//!
//! Every rule lives in its own module with a `static META: RuleMeta` (code,
//! name, documentation, options) and a `static RULE: RuleImpl` (the hooks
//! it runs in). [`rules!`] lists them once, in code order, and produces both
//! the metadata list [`ALL`] (configuration, documentation) and the
//! implementation list [`IMPLS`] (the driver).

use crate::registry::{Registry, RuleMeta};
use crate::rule::RuleImpl;

pub mod helpers;
pub mod script;
#[cfg(test)]
pub mod test_support;
pub mod view;

macro_rules! rules {
    ($($($segment:ident)::+),* $(,)?) => {
        /// Every rule, in code order.
        pub static ALL: &[&RuleMeta] = &[$(&$($segment)::+::META),*];
        /// Every rule implementation, in the order of [`ALL`].
        pub static IMPLS: &[&RuleImpl] = &[$(&$($segment)::+::RULE),*];
    };
}

rules![
    script::syntax_error,
    script::compile_error,
    script::undefined_name,
    script::unused_import,
    script::unused_variable,
    script::redefined_unused,
    script::unreachable_code,
    script::bare_except,
    script::mutable_default,
    script::duplicate_dict_key,
    script::self_assignment,
    script::literal_identity,
    script::assert_on_tuple,
    script::format_string,
    script::except_order,
    script::pointless_statement,
    script::except_tuple,
    script::raise_string,
    script::redefined_builtin,
    script::used_before_assignment,
    script::return_in_finally,
    script::dangerous_loop_closure,
    script::global_at_module_level,
    script::print_statement,
    script::unused_suppression,
    view::invalid_json,
];

pub static REGISTRY: Registry = Registry::new(ALL);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_are_sorted_unique_and_consistent() {
        for pair in ALL.windows(2) {
            assert!(
                pair[0].code < pair[1].code,
                "{} before {}",
                pair[0].code,
                pair[1].code
            );
        }
        let mut names: Vec<&str> = ALL.iter().map(|m| m.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ALL.len(), "rule names are unique");
        assert_eq!(ALL.len(), IMPLS.len());
        for (meta, imp) in ALL.iter().zip(IMPLS) {
            assert!(
                std::ptr::eq(*meta, imp.meta),
                "{} registered twice",
                meta.code
            );
            assert!(
                meta.code.starts_with(meta.category.prefix()),
                "{}",
                meta.code
            );
            assert!(
                !meta.summary.is_empty() && !meta.explanation.is_empty(),
                "{}",
                meta.code
            );
            for section in ["## What it does", "## Why", "## Example"] {
                assert!(
                    meta.explanation.contains(section),
                    "{}: missing {section}",
                    meta.code
                );
            }
            for option in meta.options {
                if !matches!(option.default.trim(), "" | "none") {
                    crate::options::parse_default(option)
                        .unwrap_or_else(|e| panic!("{}: {e}", meta.code));
                }
                assert!(
                    meta.explanation.contains(&format!("`{}`", option.name)),
                    "{}: option {} undocumented",
                    meta.code,
                    option.name
                );
            }
            // Every rule's options parse with defaults only.
            if let Some(parse) = imp.options {
                let empty = Default::default();
                parse(&crate::options::OptionReader::new(meta, &empty))
                    .unwrap_or_else(|e| panic!("{}: {e}", meta.code));
            }
        }
    }
}
