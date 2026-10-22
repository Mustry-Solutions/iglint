//! JSON writer that reproduces Ignition's Gson output: 2-space indent,
//! HTML-safe escapes (`'` `=` `<` `>` `&` as `\u00XX`), no trailing newline.
//! Views are written with keys sorted at every level (Designer style);
//! manifests and tag files keep insertion order.

use serde_json::Value;

pub fn to_string(value: &Value, sort_keys: bool) -> String {
    let mut out = String::with_capacity(4096);
    write_value(value, sort_keys, 0, &mut out);
    out
}

fn indent(depth: usize, out: &mut String) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn write_value(value: &Value, sort: bool, depth: usize, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(",\n");
                }
                indent(depth + 1, out);
                write_value(item, sort, depth + 1, out);
            }
            out.push('\n');
            indent(depth, out);
            out.push(']');
        }
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            if sort {
                entries.sort_by(|a, b| a.0.cmp(b.0));
            }
            out.push_str("{\n");
            for (i, (k, v)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push_str(",\n");
                }
                indent(depth + 1, out);
                write_string(k, out);
                out.push_str(": ");
                write_value(v, sort, depth + 1, out);
            }
            out.push('\n');
            indent(depth, out);
            out.push('}');
        }
    }
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '<' | '>' | '&' | '=' | '\'' | '\u{2028}' | '\u{2029}' => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn designer_style() {
        let v = json!({"b": [1, {"z": "a='x'", "a": 2.5}], "a": {}, "c": []});
        assert_eq!(
            super::to_string(&v, true),
            "{\n  \"a\": {},\n  \"b\": [\n    1,\n    {\n      \"a\": 2.5,\n      \"z\": \"a\\u003d\\u0027x\\u0027\"\n    }\n  ],\n  \"c\": []\n}"
        );
    }

    #[test]
    fn insertion_order_and_escapes() {
        let v = json!({"z": "\tif a < b:\n\t\tpass", "a": true});
        assert_eq!(
            super::to_string(&v, false),
            "{\n  \"z\": \"\\tif a \\u003c b:\\n\\t\\tpass\",\n  \"a\": true\n}"
        );
    }
}
