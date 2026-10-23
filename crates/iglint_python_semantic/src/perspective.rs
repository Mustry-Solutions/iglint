//! The members of the objects Perspective hands to scripts, measured
//! inside the Ignition 8.3.3 image by `stubs-gen/gen_vetted.py` (see
//! [`crate::generated`]).
//!
//! `self` in a view script, `self.view`, `self.page`, `self.session` and
//! the `session` and `page` parameters of session and page events are
//! `PyDelegateObject`s over a `SafetyWrapper`. Python sees, in order: the
//! property trees named by `PropertyType` (`props`, `custom`, ...),
//! custom methods and the script-callable methods of the component's
//! model delegate (components only), then the public Java members of the
//! wrapper (method and bean spellings). The linter's access analysis
//! classifies every measured member; its tests fail when a regenerated
//! table holds a member it does not classify.

/// A Perspective script object: `component`, `view`, `page` or `session`.
pub type Wrapper = &'static str;

/// The wrapper classes measured, by short name. The regeneration test
/// (`generated_tables_match_vetted_inputs`) fails when the generator
/// measures other classes.
pub const WRAPPER_CLASSES: &[(Wrapper, &str)] = &[
    (
        "component",
        "com.inductiveautomation.perspective.gateway.script.ComponentModelScriptWrapper$SafetyWrapper",
    ),
    (
        "page",
        "com.inductiveautomation.perspective.gateway.script.PageScriptWrapper$SafetyWrapper",
    ),
    (
        "session",
        "com.inductiveautomation.perspective.gateway.script.SessionScriptWrapper$SafetyWrapper",
    ),
    (
        "view",
        "com.inductiveautomation.perspective.gateway.script.ViewModelScriptWrapper$SafetyWrapper",
    ),
];

/// Every measured member: wrapper, member name (Java method or bean
/// spelling), and the kind of its declared type: `host` (a page or
/// session), `element` (views and components, or collections of them),
/// `tree` (a property tree), `object` (`Object` or `PyObject`) or `value`.
pub fn members() -> &'static [(Wrapper, &'static str, &'static str)] {
    crate::generated::PERSPECTIVE_MEMBERS
}

/// The kind of member `name` of `wrapper`, if it is a member.
pub fn member_kind(wrapper: &str, name: &str) -> Option<&'static str> {
    members()
        .iter()
        .find(|(w, n, _)| *w == wrapper && *n == name)
        .map(|(_, _, k)| *k)
}

/// The property tree names (`PropertyType`), resolved before members.
pub fn property_types() -> &'static [&'static str] {
    crate::generated::PERSPECTIVE_PROPERTY_TYPES
}

/// Whether `name` is a property tree name.
pub fn is_property_type(name: &str) -> bool {
    property_types().contains(&name)
}

/// The script-callable members of every component model delegate, with
/// the kind of their declared type.
pub fn delegate_members() -> &'static [(&'static str, &'static str)] {
    crate::generated::PERSPECTIVE_DELEGATE_MEMBERS
}
