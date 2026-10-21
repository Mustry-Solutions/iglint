//! The iglint linter: diagnostics, rule registry, rules and the check driver.

pub mod analyze;
pub mod baseline;
mod cache;
pub mod diagnostic;
pub mod discovery;
pub mod engine;
mod fix;
pub mod registry;
pub mod rules;
pub mod settings;
pub mod suggest;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use diagnostic::{Applicability, Diagnostic, FileEdit, Fix, Label, Severity, SourceFile};
pub use discovery::FileKind;
pub use engine::{CheckOptions, CheckResult, FixMode, check};
pub use registry::{
    Category, FixAvailability, OptionMeta, OptionType, Registry, RuleCode, RuleMeta, Selector,
};
