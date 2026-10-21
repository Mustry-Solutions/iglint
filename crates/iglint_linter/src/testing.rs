//! Test-only rules and a fake analyzer, for testing configuration, the
//! engine and the CLI before (and independently of) the real rules.
//!
//! Enabled with the `testing` feature.
//!
//! The fake analyzer reports every occurrence of a rule's trigger word in a
//! file, for each enabled rule:
//!
//! | rule | trigger | severity | fix |
//! |---|---|---|---|
//! | `SCR101` undefined-name | `undefined_thing` | error | none |
//! | `SCR102` unused-variable | `unused_var` | warning | safe: replace with `_` |
//! | `VIEW001` unused-custom-property | `unusedProp` | error | unsafe: rename to `usedProp` |
//! | `NAME001` component-name | `badName` | warning | safe: rename to `BadName` |
//! | `NAME002` property-name | `bad_prop` | info | none |
//! | `BIND001` polling-interval | `now(1)` | error | none |
//! | `REF001` missing-view | `missing/view` | error | none |

use std::sync::Arc;

use iglint_text::{TextEdit, TextRange, TextSize};

use crate::diagnostic::{Diagnostic, FileEdit, Fix, Severity, SourceFile};
use crate::discovery::FileKind;
use crate::engine::Analyzer;
use crate::registry::{
    Category, FixAvailability, OptionMeta, OptionType, Registry, RuleCode, RuleMeta,
};
use crate::settings::LinterSettings;

pub static UNDEFINED_NAME: RuleMeta = RuleMeta {
    code: "SCR101",
    name: "undefined-name",
    category: Category::Script,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "Name is not defined",
    explanation: "## What it does\n\nReports names that are never defined.\n",
    ..RuleMeta::DEFAULT
};

pub static UNUSED_VARIABLE: RuleMeta = RuleMeta {
    code: "SCR102",
    name: "unused-variable",
    category: Category::Script,
    default_severity: Severity::Warning,
    default_enabled: true,
    fix: FixAvailability::Always,
    summary: "Local variable is never used",
    explanation: "## What it does\n\nReports unused local variables.\n",
    options: &[
        OptionMeta {
            name: "ignore-names",
            ty: OptionType::StringList,
            default: "[]",
            description: "Names never reported.",
        },
        OptionMeta {
            name: "dummy-pattern",
            ty: OptionType::String,
            default: "\"^_\"",
            description: "Regex of names that may be unused.",
        },
    ],
    ..RuleMeta::DEFAULT
};

pub static UNUSED_CUSTOM_PROPERTY: RuleMeta = RuleMeta {
    code: "VIEW001",
    name: "unused-custom-property",
    category: Category::View,
    default_severity: Severity::Warning,
    default_enabled: true,
    fix: FixAvailability::Sometimes,
    summary: "Custom property is never referenced",
    explanation: "## What it does\n\nReports unused custom properties.\n",
    ign_lint_names: &["UnusedCustomPropertiesRule"],
    ..RuleMeta::DEFAULT
};

pub static COMPONENT_NAME: RuleMeta = RuleMeta {
    code: "NAME001",
    name: "component-name",
    category: Category::Name,
    default_severity: Severity::Warning,
    default_enabled: true,
    fix: FixAvailability::Sometimes,
    summary: "Component name does not follow the convention",
    explanation: "## What it does\n\nChecks component names.\n",
    options: &[
        OptionMeta {
            name: "convention",
            ty: OptionType::String,
            default: "\"PascalCase\"",
            description: "Naming convention.",
        },
        OptionMeta {
            name: "min-length",
            ty: OptionType::Integer,
            default: "1",
            description: "Minimum length.",
        },
        OptionMeta {
            name: "skip-names",
            ty: OptionType::StringList,
            default: "[\"root\"]",
            description: "Names never checked.",
        },
        OptionMeta {
            name: "per-type",
            ty: OptionType::Table,
            default: "{}",
            description: "Per node type settings.",
        },
    ],
    ign_lint_names: &["NamePatternRule"],
    ign_lint_option_map: &[
        ("convention", "convention"),
        ("min_length", "min-length"),
        ("skip_names", "skip-names"),
        ("node_type_specific_rules", "per-type"),
    ],
    ..RuleMeta::DEFAULT
};

pub static PROPERTY_NAME: RuleMeta = RuleMeta {
    code: "NAME002",
    name: "property-name",
    category: Category::Name,
    default_severity: Severity::Info,
    default_enabled: false,
    summary: "Property name does not follow the convention",
    explanation: "## What it does\n\nChecks property names.\n",
    options: &[OptionMeta {
        name: "convention",
        ty: OptionType::String,
        default: "\"camelCase\"",
        description: "Naming convention.",
    }],
    ign_lint_names: &["NamePatternRule"],
    ign_lint_option_map: &[("convention", "convention")],
    ..RuleMeta::DEFAULT
};

pub static POLLING_INTERVAL: RuleMeta = RuleMeta {
    code: "BIND001",
    name: "polling-interval",
    category: Category::Bind,
    default_severity: Severity::Error,
    default_enabled: false,
    summary: "now() polls too often",
    explanation: "## What it does\n\nFlags fast `now()` polling.\n",
    options: &[OptionMeta {
        name: "minimum-interval",
        ty: OptionType::Integer,
        default: "10000",
        description: "Minimum interval in milliseconds.",
    }],
    ign_lint_names: &["PollingIntervalRule"],
    ign_lint_option_map: &[("minimum_interval", "minimum-interval")],
    ..RuleMeta::DEFAULT
};

pub static MISSING_VIEW: RuleMeta = RuleMeta {
    code: "REF001",
    name: "missing-view",
    category: Category::Ref,
    default_severity: Severity::Error,
    default_enabled: false,
    summary: "Referenced view does not exist",
    explanation: "## What it does\n\nReports references to views that do not exist.\n",
    ..RuleMeta::DEFAULT
};

pub static RULES: &[&RuleMeta] = &[
    &POLLING_INTERVAL,
    &COMPONENT_NAME,
    &PROPERTY_NAME,
    &MISSING_VIEW,
    &UNDEFINED_NAME,
    &UNUSED_VARIABLE,
    &UNUSED_CUSTOM_PROPERTY,
];

pub static REGISTRY: Registry = Registry::new(RULES);

struct Trigger {
    rule: &'static RuleMeta,
    word: &'static str,
    fix: Option<(&'static str, bool)>,
}

const TRIGGERS: &[Trigger] = &[
    Trigger {
        rule: &UNDEFINED_NAME,
        word: "undefined_thing",
        fix: None,
    },
    Trigger {
        rule: &UNUSED_VARIABLE,
        word: "unused_var",
        fix: Some(("_", true)),
    },
    Trigger {
        rule: &UNUSED_CUSTOM_PROPERTY,
        word: "unusedProp",
        fix: Some(("usedProp", false)),
    },
    Trigger {
        rule: &COMPONENT_NAME,
        word: "badName",
        fix: Some(("BadName", true)),
    },
    Trigger {
        rule: &PROPERTY_NAME,
        word: "bad_prop",
        fix: None,
    },
    Trigger {
        rule: &POLLING_INTERVAL,
        word: "now(1)",
        fix: None,
    },
    Trigger {
        rule: &MISSING_VIEW,
        word: "missing/view",
        fix: None,
    },
];

/// The fake per-file analysis described in the module documentation.
pub fn analyze(file: &SourceFile, _kind: FileKind, settings: &LinterSettings) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for trigger in TRIGGERS {
        let rule = RuleCode::new(trigger.rule);
        let Some(rule_settings) = settings.rule(rule) else {
            continue;
        };
        for (offset, _) in file.text().match_indices(trigger.word) {
            let range = TextRange::at(TextSize::from(offset), TextSize::of(trigger.word));
            let mut diagnostic = Diagnostic::new(
                rule,
                rule_settings.severity,
                format!("`{}` triggers {}", trigger.word, trigger.rule.name),
                file.clone(),
                range,
            );
            if let Some((replacement, safe)) = trigger.fix {
                let edits = vec![FileEdit {
                    path: file.path().to_path_buf(),
                    edit: TextEdit::replace(range, replacement),
                }];
                let message = format!("Replace with `{replacement}`");
                diagnostic.fix = Some(if safe {
                    Fix::safe(message, edits)
                } else {
                    Fix::unsafe_edit(message, edits)
                });
            }
            diagnostics.push(diagnostic);
        }
    }
    diagnostics
}

pub fn analyzer() -> Analyzer {
    Arc::new(analyze)
}
