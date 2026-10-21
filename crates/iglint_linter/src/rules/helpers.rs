//! Small utilities shared by rules.

use iglint_json::Value;
use iglint_text::TextRange;

/// The range of the key of `object.key`, falling back to the value.
pub fn key_range(object: &Value<'_>, key: &str) -> Option<TextRange> {
    let member = object.as_object()?.member(key)?;
    Some(member.key.range())
}

/// `n` with its noun, pluralised: `1 item`, `2 items`.
pub fn plural(n: usize, singular: &str, plural: &str) -> String {
    if n == 1 {
        format!("{n} {singular}")
    } else {
        format!("{n} {plural}")
    }
}

/// Matches `text` against a glob with `*` (any run of characters) and `?`
/// (one character).
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// True for a JSON value that is absent, `null`, or a blank string.
pub fn is_blank_text(value: Option<&Value<'_>>) -> bool {
    match value {
        None => true,
        Some(v) if v.is_null() => true,
        Some(v) => v.as_str().is_some_and(|s| s.trim().is_empty()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards() {
        assert!(wildcard_match("embr.*", "embr.chart.x"));
        assert!(wildcard_match("a?c", "abc"));
        assert!(!wildcard_match("a?c", "abbc"));
        assert!(wildcard_match("*", ""));
        assert!(!wildcard_match("embr.*", "ia.display.label"));
    }
}
