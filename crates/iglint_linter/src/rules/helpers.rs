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
