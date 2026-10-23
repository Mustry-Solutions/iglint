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
//! implementation in use, [`AstScriptFacts`]: property accesses are found
//! on the Jython syntax tree (see [`mod@access`]), cross-resource
//! references and library symbols with the semantic model (see
//! [`mod@refs`] and [`mod@symbols`]). [`NoScriptFacts`] reports nothing and is
//! honest about it through [`ScriptFacts::complete`]: rules that depend on
//! script facts then treat every script as possibly reading everything.

use iglint_project::ComponentId;
use iglint_project::EmbeddedScript;
use iglint_project::summary::ResourceSummary;
use iglint_project::view::{PropCategory, View};
use iglint_text::TextRange;

use crate::rule::ScriptUnit;

pub mod access;
pub mod api;
pub mod calls;
pub mod code_sources;
pub mod refs;
pub mod symbols;

pub use access::AstScriptFacts;
pub use refs::FactsContext;

/// Whose property a script accesses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AccessTarget {
    /// The view (`self.view.custom.x`, or `self.custom.x` in a view-level
    /// script).
    View,
    /// One component (`self.custom.x`, `self.getSibling('A').custom.x`).
    Component(ComponentId),
    /// A component that cannot be determined (`self.getSibling(name)`,
    /// `self.children[i]`): the access may concern any component.
    AnyComponent,
}

/// Which properties a view script reads or writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropertyAccess {
    pub target: AccessTarget,
    pub category: PropCategory,
    /// The top-level property name; `None` when the script accesses the
    /// whole category or a computed key (`self.view.custom[name]`), which
    /// counts as accessing every property of that category.
    pub name: Option<String>,
    pub read: bool,
    pub write: bool,
    /// Where in the file the access is.
    pub range: TextRange,
    /// Set when the access is assumed because a followed value is passed
    /// to a call of a dotted name (`lib.util.handle(self)`): rules with the
    /// project index can resolve the callee and replace the assumption by
    /// what the function does (see [`access::LibraryCall`]).
    pub via: Option<std::sync::Arc<access::LibraryCall>>,
}

impl PropertyAccess {
    /// Whether the access is only assumed until the project index shows
    /// that a name the script calls is not a frame builtin
    /// ([`access::LibraryCall::is_callee_check`]). Rules that do not
    /// resolve library calls skip these.
    pub fn is_callee_check(&self) -> bool {
        self.via.as_ref().is_some_and(|c| c.is_callee_check())
    }
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

    /// Adds the cross-resource references of one script to its resource's
    /// summary (views, named queries, library names, message sends, pages,
    /// dynamic references) and, for library modules, the module's
    /// top-level symbols.
    fn summarize(
        &self,
        unit: &ScriptUnit<'_, '_>,
        ctx: &FactsContext<'_>,
        summary: &mut ResourceSummary,
    );

    /// The property reads and writes of one script of `view`. Lookups of
    /// components that cannot be resolved statically are reported as
    /// [`AccessTarget::AnyComponent`]; lookups of components that do not
    /// exist are left out (they fail at run time); dynamic keys are
    /// reported with `name: None`.
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

    fn summarize(
        &self,
        _unit: &ScriptUnit<'_, '_>,
        _ctx: &FactsContext<'_>,
        _summary: &mut ResourceSummary,
    ) {
    }

    fn property_accesses(
        &self,
        _unit: &ScriptUnit<'_, '_>,
        _view: &View<'_>,
        _out: &mut Vec<PropertyAccess>,
    ) {
    }
}

/// The script facts implementation the driver uses.
pub fn provider() -> &'static dyn ScriptFacts {
    &AstScriptFacts
}

/// Parses a script location the way Ignition compiles it: the generated
/// source (header and body) as a unicode string.
pub fn parse_script(script: &EmbeddedScript<'_>) -> iglint_python::Parsed {
    iglint_python::parse_module(&script.source)
}
