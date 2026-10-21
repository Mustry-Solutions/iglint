//! The iglint linter: diagnostics, rule registry, rules and the check driver.

pub mod diagnostic;
pub mod registry;
pub mod rules;

pub use diagnostic::{Applicability, Diagnostic, FileEdit, Fix, Label, Severity, SourceFile};
pub use registry::{
    Category, FixAvailability, OptionMeta, OptionType, Registry, RuleCode, RuleMeta, Selector,
};
