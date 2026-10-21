//! What the script layer tells the rest of the linter about scripts.
//!
//! Rules outside the `SCR` family need two kinds of facts that only Python
//! analysis can provide:
//!
//! * **Cross-resource references** made by scripts: literal view paths
//!   (`system.perspective.openPopup(..., 'Popups/Detail')`), named query
//!   paths (`system.db.runNamedQuery('Machines/ByLine')`), library calls,
//!   message sends. They are added to the resource's
//!   [`References`](iglint_project::summary::References) before the project
//!   index is built, so `REF001`, `REF004`, `REF011`, `REF013`, ... see them
//!   like references from bindings.
//! * **Property accesses** of view scripts (`self.view.custom.x`,
//!   `self.getSibling('Label').props.text = ...`), so `VIEW015
//!   unused-custom-property` and `VIEW019 output-param-never-written` can
//!   count script reads and writes.
//!
//! [`ScriptFacts`] is the interface; [`provider`] returns the
//! implementation in use. Until the script layer installs one,
//! [`NoScriptFacts`] reports nothing and says so through
//! [`ScriptFacts::complete`], and rules that depend on script facts treat
//! every script as possibly reading everything (they stay silent for views
//! that have scripts).

use iglint_project::ComponentId;
use iglint_project::EmbeddedScript;
use iglint_project::summary::References;
use iglint_project::view::{PropCategory, View};
use iglint_text::TextRange;

use crate::rule::ScriptUnit;

/// Which properties a view script reads or writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropertyAccess {
    /// `None` for view-level properties (`self.view.custom.x`), else the
    /// component whose property is accessed.
    pub component: Option<ComponentId>,
    pub category: PropCategory,
    /// The top-level property name; `None` when the script accesses the
    /// whole category or a computed key (`self.view.custom[name]`), which
    /// counts as accessing every property of that category.
    pub name: Option<String>,
    pub write: bool,
    /// Where in the file the access is.
    pub range: TextRange,
}

/// Facts extracted from parsed scripts.
pub trait ScriptFacts: Sync {
    /// True when [`ScriptFacts::property_accesses`] finds every access a
    /// script makes, except those it reports as `name: None`. Rules that
    /// count script reads only report when this is true.
    fn complete(&self) -> bool;

    /// Whether [`ScriptFacts::references`] contributes anything. When false
    /// the driver does not parse context-only scripts for it.
    fn provides_references(&self) -> bool;

    /// Adds the literal cross-resource references of one script to its
    /// resource's references (views, named queries, library names, message
    /// sends, pages).
    fn references(&self, unit: &ScriptUnit<'_, '_>, refs: &mut References);

    /// The property reads and writes of one script of `view`. Unresolvable
    /// component lookups are left out (the access goes to an unknown
    /// component); dynamic keys are reported with `name: None`.
    fn property_accesses(
        &self,
        unit: &ScriptUnit<'_, '_>,
        view: &View<'_>,
        out: &mut Vec<PropertyAccess>,
    );
}

/// Reports nothing; used until the script layer provides facts.
pub struct NoScriptFacts;

impl ScriptFacts for NoScriptFacts {
    fn complete(&self) -> bool {
        false
    }

    fn provides_references(&self) -> bool {
        false
    }

    fn references(&self, _unit: &ScriptUnit<'_, '_>, _refs: &mut References) {}

    fn property_accesses(
        &self,
        _unit: &ScriptUnit<'_, '_>,
        _view: &View<'_>,
        _out: &mut Vec<PropertyAccess>,
    ) {
    }
}

/// The script facts implementation the driver uses. The script layer
/// replaces [`NoScriptFacts`] here.
pub fn provider() -> &'static dyn ScriptFacts {
    &NoScriptFacts
}

/// Parses a script location the way Ignition compiles it: the generated
/// source (header and body) as a unicode string.
pub fn parse_script(script: &EmbeddedScript<'_>) -> iglint_python::Parsed {
    iglint_python::parse_module(&script.source)
}
