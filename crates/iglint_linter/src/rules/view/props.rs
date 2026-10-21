//! Helpers shared by the custom property rules (VIEW006, VIEW012, VIEW013,
//! VIEW015, VIEW019).

use iglint_json::Value;
use iglint_project::Owner;
use iglint_project::view::{PropertyDef, View};
use iglint_text::TextRange;

/// The ign-lint path of an owner: `""` for the view, else the component's
/// ign-lint path (`root.root.children[0].Button`).
pub fn owner_ign<'v>(view: &'v View<'_>, owner: Owner) -> &'v str {
    match owner {
        Owner::Component(id) => &view.component(id).ign_lint_path,
        _ => "",
    }
}

/// `owner.key` in ign-lint's notation (`custom.x` for the view).
pub fn ign_full_path(view: &View<'_>, owner: Owner, key: &str) -> String {
    let base = owner_ign(view, owner);
    if base.is_empty() {
        key.to_owned()
    } else {
        format!("{base}.{key}")
    }
}

/// The JSON object of an owner: the component, or the view document.
pub fn owner_object<'a>(view: &View<'a>, owner: Owner) -> Option<&'a Value<'a>> {
    match owner {
        Owner::Component(id) => Some(view.component(id).value),
        Owner::View => Some(view.root),
        _ => None,
    }
}

/// The `propConfig` object of an owner.
pub fn owner_prop_config<'a>(view: &View<'a>, owner: Owner) -> Option<&'a Value<'a>> {
    match owner {
        Owner::Component(id) => view.component(id).prop_config,
        Owner::View => view.prop_config,
        _ => None,
    }
}

/// The range to report for a property definition: its key in
/// `custom`/`params`, else the key of its first `propConfig` entry.
pub fn def_range(view: &View<'_>, def: &PropertyDef<'_>) -> TextRange {
    if let Some(key) = def.key {
        return key.range();
    }
    let prefix = def.category.name();
    view.prop_configs
        .iter()
        .find(|c| c.owner == def.owner && c.path.category == prefix && c.path.name == def.name)
        .map_or_else(|| def.range(), |c| c.key.range())
}

/// Follows a property path (`custom.a.b[0]`) from `object`.
pub fn lookup<'a>(object: &'a Value<'a>, path: &str) -> Option<&'a Value<'a>> {
    let mut at = object;
    for segment in path.split('.') {
        let (name, mut rest) = match segment.find('[') {
            Some(i) => (&segment[..i], &segment[i..]),
            None => (segment, ""),
        };
        if !name.is_empty() {
            at = at.get(name)?;
        }
        while let Some(stripped) = rest.strip_prefix('[') {
            let end = stripped.find(']')?;
            at = at.index(stripped[..end].parse().ok()?)?;
            rest = &stripped[end + 1..];
        }
    }
    Some(at)
}

/// Whether `pattern` (with `*` and `?` wildcards; everything else, `[`
/// included, literal) matches all of `text`, like ign-lint's exemptions.
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut star_t) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            star_t = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            star_t += 1;
            ti = star_t;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards() {
        assert!(wildcard_match(
            "root.root.children[?].*",
            "root.root.children[0].A.custom.x"
        ));
        assert!(!wildcard_match(
            "root.root.children[?].*",
            "root.root.children[10].A.custom.x"
        ));
        assert!(wildcard_match("data", "data"));
        assert!(!wildcard_match("data", "data2"));
        assert!(wildcard_match("d*", "data2"));
    }

    #[test]
    fn lookups() {
        let v = iglint_json::parse(r#"{"custom": {"a": {"b": [1, {"c": 2}]}}}"#).unwrap();
        assert_eq!(
            lookup(&v, "custom.a.b[1].c").and_then(Value::as_i64),
            Some(2)
        );
        assert!(lookup(&v, "custom.x").is_none());
    }
}
