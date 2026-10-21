//! Node paths: iglint's own format and the ign-lint compatible format.
//!
//! # iglint node paths
//!
//! A node path names an element of a resource for messages, configuration
//! (`exclude` patterns) and baseline fingerprints. It is built from names a
//! user sees in the Designer, so it survives reordering of siblings:
//!
//! * A component is its `meta.name` chain from the root container joined
//!   with `/`, the same as Perspective's absolute component paths without
//!   the leading slash: `root`, `root/Header/Title`.
//! * The view itself is `view`; session properties are `session`.
//! * A property appends `.` and its property path: `root/Title.props.text`,
//!   `view.params.line`, `root/Chart.props.series[0].data`.
//! * Everything that is not a property value is introduced by `@`, so it can
//!   never collide with a property name:
//!   * binding: `root/Title.props.text@binding`; its transforms
//!     `...@binding.transforms[0]`; struct leaves
//!     `...@binding.struct.count`; query parameters
//!     `...@binding.parameters.line`;
//!   * change script: `view.custom.total@onChange`;
//!   * event actions: `root/Save@events.component.onActionPerformed`, with
//!     `[i]` when the event holds a list of actions;
//!   * component scripts: `root@messageHandlers.line-changed`,
//!     `root@customMethods.describe`, `root/Alarms@extensionFunctions.filterAlarm`.
//! * Tags use Ignition tag paths: `[default]Plant/Line1/Counter`, with
//!   `@eventScripts.valueChanged`, `@alarms.Overflow` or `.expression` for
//!   tag elements. UDT definitions live under `_types_`.
//! * Other resources use their resource path, for example the named query
//!   `Machines/ByLine` or the timer `poll-weather`.
//!
//! Duplicate sibling names make a component path ambiguous; the component's
//! JSON pointer ([`crate::Component::pointer`]) is always exact.
//!
//! # ign-lint node paths
//!
//! ign-lint names nodes by flattening the JSON (compat spec 2 and 3): keys
//! joined with `.`, array indexes as `[i]`, and the `meta.name` of any object
//! that has one injected after that object's path. The root component is
//! therefore `root.root`, a child `root.root.children[0].Button`. Bindings are
//! named by their property path (`...Button.propConfig.props.text`), scripts
//! by their location (`...scripts.messageHandlers[0]`,
//! `...events.component.onActionPerformed[1]`), and property nodes with every
//! `[i]` removed ([`strip_indices`]).

use iglint_json::Value;

/// `base.key`, or `key` when `base` is empty.
pub(crate) fn ign_key(base: &str, key: &str) -> String {
    if base.is_empty() {
        key.to_owned()
    } else {
        let mut s = String::with_capacity(base.len() + 1 + key.len());
        s.push_str(base);
        s.push('.');
        s.push_str(key);
        s
    }
}

/// `base[index]`.
pub(crate) fn ign_index(base: &str, index: usize) -> String {
    format!("{base}[{index}]")
}

/// Appends the injected component name when `object` has a `meta` object
/// with a truthy `name`, as ign-lint's flattening does.
pub(crate) fn ign_inject(path: String, object: &Value<'_>) -> String {
    let Some(name) = object.get("meta").and_then(|m| m.get("name")) else {
        return path;
    };
    let text = match &name.kind {
        iglint_json::ValueKind::String(s) if !s.value().is_empty() => s.value().to_owned(),
        iglint_json::ValueKind::Number(n) if n.as_f64().is_some_and(|f| f != 0.0) => {
            n.raw().to_owned()
        }
        iglint_json::ValueKind::Bool(true) => "True".to_owned(),
        _ => return path,
    };
    ign_key(&path, &text)
}

/// Removes every `[i]` index from an ign-lint path, as ign-lint does for
/// property nodes.
pub fn strip_indices(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut rest = path;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        match after.find(']') {
            Some(close) if after[..close].bytes().all(|b| b.is_ascii_digit()) && close > 0 => {
                out.push_str(&rest[..open]);
                rest = &after[close + 1..];
            }
            _ => {
                out.push_str(&rest[..=open]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `owner.property` in iglint format (`root/Title` + `props.text`).
pub(crate) fn prop_path(owner: &str, property: &str) -> String {
    let mut s = String::with_capacity(owner.len() + 1 + property.len());
    s.push_str(owner);
    s.push('.');
    s.push_str(property);
    s
}

/// `base@element` in iglint format.
pub(crate) fn at(base: &str, element: &str) -> String {
    let mut s = String::with_capacity(base.len() + 1 + element.len());
    s.push_str(base);
    s.push('@');
    s.push_str(element);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip() {
        assert_eq!(
            strip_indices("root.root.children[0].B.custom.x[1]"),
            "root.root.children.B.custom.x"
        );
        assert_eq!(strip_indices("a[x]"), "a[x]");
        assert_eq!(strip_indices("a[]"), "a[]");
    }

    #[test]
    fn inject() {
        let v = iglint_json::parse(r#"{"meta": {"name": "Btn"}}"#).unwrap();
        assert_eq!(
            ign_inject("root.root.children[0]".into(), &v),
            "root.root.children[0].Btn"
        );
        let v = iglint_json::parse(r#"{"meta": {"name": ""}}"#).unwrap();
        assert_eq!(ign_inject("x".into(), &v), "x");
    }
}
