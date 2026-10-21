//! VIEW006 excessive-context-data.

use iglint_json::{Value, ValueKind};
use iglint_project::ModelData;
use iglint_project::view::View;
use iglint_text::TextRange;
use rustc_hash::FxHashMap;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::options::OptionReader;
use crate::registry::{Category, OptionMeta, OptionType, RuleMeta};
use crate::rule::{Hook, ParsedOptions, Phase, RuleImpl, ViewRule};

pub static META: RuleMeta = RuleMeta {
    code: "VIEW006",
    name: "excessive-context-data",
    category: Category::View,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "Oversized data stored in view-level custom properties",
    explanation: r#"## What it does

Measures the data stored in the view's `custom` object and reports:

* arrays with more than `max-array-size` items;
* objects (including `custom` itself) with more than
  `max-sibling-properties` members;
* nesting deeper than `max-nesting-depth` levels (one finding, at the first
  deepest value);
* more than `max-data-points` values in total;
* strings longer than `max-value-length` characters.

Counting follows ign-lint: values are the leaves of the JSON tree (an empty
object or array that is a member counts as one value), array indexes do not
add nesting levels, a Java date object (`{"$": ["ts", ...], "$ts": ...}`) is
one value.

## Why is this bad?

Data pasted into a view (lookup tables, query results captured by the
Designer) is sent to every client that opens the view, bloats the resource
and hides data that belongs in a database or a named query.

## Example

```json
{"custom": {"machines": [{"id": 1}, {"id": 2}, "... 500 more ..."]}}
```

Use instead: a binding (`query`, `tag`) that loads the data at run time, or
a named query.

## Options

* `max-array-size` (default 50)
* `max-sibling-properties` (default 50)
* `max-nesting-depth` (default 5)
* `max-data-points` (default 1000)
* `max-value-length` (default 10000, in characters)

All limits are exclusive: exactly the limit is fine.

## ign-lint

Equivalent of `ExcessiveContextDataRule`, same options and messages, with
these corrections: the breadth of the top-level `custom` object is measured
too, arrays nested inside arrays are measured, and arrays of empty objects
are counted by their real length.
"#,
    options: &[
        OptionMeta {
            name: "max-array-size",
            ty: OptionType::Integer,
            default: "50",
            description: "Largest allowed array.",
        },
        OptionMeta {
            name: "max-sibling-properties",
            ty: OptionType::Integer,
            default: "50",
            description: "Largest allowed number of members of one object.",
        },
        OptionMeta {
            name: "max-nesting-depth",
            ty: OptionType::Integer,
            default: "5",
            description: "Deepest allowed nesting.",
        },
        OptionMeta {
            name: "max-data-points",
            ty: OptionType::Integer,
            default: "1000",
            description: "Largest allowed number of values in `custom`.",
        },
        OptionMeta {
            name: "max-value-length",
            ty: OptionType::Integer,
            default: "10000",
            description: "Longest allowed string, in characters.",
        },
    ],
    ign_lint_names: &["ExcessiveContextDataRule"],
    ign_lint_option_map: &[
        ("max_array_size", "max-array-size"),
        ("max_sibling_properties", "max-sibling-properties"),
        ("max_nesting_depth", "max-nesting-depth"),
        ("max_data_points", "max-data-points"),
        ("max_value_length", "max-value-length"),
    ],
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::View(&ExcessiveContextData)],
    options: Some(parse_options),
};

struct Options {
    array: usize,
    siblings: usize,
    depth: usize,
    points: usize,
    length: usize,
}

fn parse_options(o: &OptionReader<'_>) -> Result<ParsedOptions, String> {
    let n = |name: &str| -> Result<usize, String> {
        let v = o.int(name)?;
        usize::try_from(v).map_err(|_| format!("`{name}` must not be negative"))
    };
    Ok(Box::new(Options {
        array: n("max-array-size")?,
        siblings: n("max-sibling-properties")?,
        depth: n("max-nesting-depth")?,
        points: n("max-data-points")?,
        length: n("max-value-length")?,
    }))
}

struct ExcessiveContextData;

/// A flattened value under `custom`.
struct Leaf<'a> {
    /// Path without array indexes, as segments (`custom`, `a`, `b`).
    stripped: Vec<String>,
    /// Path with indexes (`custom.a[0].b`).
    full: String,
    value: &'a Value<'a>,
    /// Key ranges of the member at each stripped depth (index 0: `custom`).
    ancestors: Vec<TextRange>,
}

fn is_java_date(value: &Value<'_>) -> bool {
    let Some(o) = value.as_object() else {
        return false;
    };
    o.len() == 2
        && o.get("$ts").is_some_and(|t| t.as_f64().is_some())
        && o.get("$").and_then(Value::as_array).is_some_and(|a| {
            a.len() == 3
                && a.get(0).and_then(Value::as_str) == Some("ts")
                && a.get(1).is_some_and(|v| v.as_f64().is_some())
                && a.get(2).is_some_and(|v| v.as_f64().is_some())
        })
}

#[allow(clippy::too_many_arguments)]
fn flatten<'a>(
    value: &'a Value<'a>,
    in_array: bool,
    stripped: &mut Vec<String>,
    full: &mut String,
    ancestors: &mut Vec<TextRange>,
    out: &mut Vec<Leaf<'a>>,
) {
    let leaf = |stripped: &Vec<String>,
                full: &String,
                ancestors: &Vec<TextRange>,
                out: &mut Vec<Leaf<'a>>| {
        out.push(Leaf {
            stripped: stripped.clone(),
            full: full.clone(),
            value,
            ancestors: ancestors.clone(),
        });
    };
    match &value.kind {
        ValueKind::Object(o) if is_java_date(value) => {
            let _ = o;
            stripped.push("_JavaDate".to_owned());
            let len = full.len();
            full.push_str("._JavaDate");
            let last = *ancestors.last().unwrap_or(&value.range);
            ancestors.push(last);
            leaf(stripped, full, ancestors, out);
            ancestors.pop();
            full.truncate(len);
            stripped.pop();
        }
        ValueKind::Object(o) if !o.is_empty() => {
            for m in o.iter() {
                stripped.push(m.key().to_owned());
                ancestors.push(m.key.range());
                let len = full.len();
                full.push('.');
                full.push_str(m.key());
                flatten(&m.value, false, stripped, full, ancestors, out);
                full.truncate(len);
                ancestors.pop();
                stripped.pop();
            }
        }
        ValueKind::Array(a) if !a.is_empty() => {
            for (i, e) in a.iter().enumerate() {
                let len = full.len();
                full.push_str(&format!("[{i}]"));
                flatten(e, true, stripped, full, ancestors, out);
                full.truncate(len);
            }
        }
        ValueKind::Object(_) | ValueKind::Array(_) => {
            // Empty containers are values only as members.
            if !in_array {
                leaf(stripped, full, ancestors, out);
            }
        }
        _ => leaf(stripped, full, ancestors, out),
    }
}

/// Arrays anywhere under `custom`, with their indexed paths and key ranges.
fn arrays<'a>(
    value: &'a Value<'a>,
    path: &mut String,
    key: TextRange,
    out: &mut Vec<(String, &'a Value<'a>, TextRange)>,
) {
    match &value.kind {
        ValueKind::Object(o) => {
            for m in o.iter() {
                let len = path.len();
                path.push('.');
                path.push_str(m.key());
                arrays(&m.value, path, m.key.range(), out);
                path.truncate(len);
            }
        }
        ValueKind::Array(a) => {
            out.push((path.clone(), value, key));
            for (i, e) in a.iter().enumerate() {
                let len = path.len();
                path.push_str(&format!("[{i}]"));
                arrays(e, path, key, out);
                path.truncate(len);
            }
        }
        _ => {}
    }
}

fn strip_indices(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut depth = 0;
    for c in path.chars() {
        match c {
            '[' => depth += 1,
            ']' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

impl ViewRule for ExcessiveContextData {
    fn check_view(&self, cx: &mut Checker<'_, '_>, view: &View<'_>) {
        let (Some(custom), ModelData::View { file, .. }) = (view.custom, cx.data()) else {
            return;
        };
        let file = *file;
        let custom_key = view
            .root
            .as_object()
            .and_then(|o| o.member("custom"))
            .map_or(custom.range, |m| m.key.range());
        let o = cx.options::<Options>();
        let (max_array, max_siblings, max_depth, max_points, max_length) =
            (o.array, o.siblings, o.depth, o.points, o.length);

        let mut leaves = Vec::new();
        flatten(
            custom,
            false,
            &mut vec!["custom".to_owned()],
            &mut "custom".to_owned(),
            &mut vec![custom_key],
            &mut leaves,
        );
        // `custom` itself is not a value (an empty `custom` has none).
        leaves.retain(|l| l.stripped.len() > 1);

        // 1. Arrays.
        let mut found = Vec::new();
        arrays(custom, &mut "custom".to_owned(), custom_key, &mut found);
        for (path, array, key) in found {
            let size = array.as_array().map_or(0, |a| a.len());
            if size > max_array {
                let name = path.replace("custom.", "");
                let text = format!(
                    "Custom property '{name}' contains {size} items. Large datasets should be stored in databases, not view JSON. Maximum recommended: {max_array} items."
                );
                cx.report_in(file, key, text.clone())
                    .node_path(format!("view.{path}"))
                    .ign_lint_path(Some(path))
                    .ign_lint_message(text);
            }
        }

        // 2. Breadth: distinct child names per (index-free) object path.
        let mut children: FxHashMap<Vec<String>, (Vec<String>, TextRange)> = FxHashMap::default();
        let mut order: Vec<Vec<String>> = Vec::new();
        for leaf in &leaves {
            for len in 1..leaf.stripped.len() {
                let prefix = leaf.stripped[..len].to_vec();
                let next = &leaf.stripped[len];
                let entry = children.entry(prefix.clone()).or_insert_with(|| {
                    order.push(prefix.clone());
                    (Vec::new(), leaf.ancestors[len - 1])
                });
                if !entry.0.contains(next) {
                    entry.0.push(next.clone());
                }
            }
        }
        for prefix in &order {
            let (names, range) = &children[prefix];
            if names.len() > max_siblings {
                let parent = prefix.join(".");
                let text = format!(
                    "Contains {} sibling properties. Large flat structures should be stored in databases, not view JSON. Maximum recommended: {max_siblings} siblings.",
                    names.len()
                );
                cx.report_in(file, *range, text.clone())
                    .node_path(format!("view.{parent}"))
                    .ign_lint_path(Some(parent))
                    .ign_lint_message(text);
            }
        }

        // 3. Depth: the first deepest value.
        if let Some(deepest) = leaves.iter().fold(None::<&Leaf<'_>>, |best, l| match best {
            Some(b) if b.stripped.len() >= l.stripped.len() => Some(b),
            _ => Some(l),
        }) {
            let depth = deepest.stripped.len() - 1;
            if depth > max_depth {
                let path = deepest.stripped.join(".");
                let text = format!(
                    "Custom properties are nested {depth} levels deep. Deeply nested structures indicate complex data that should be in databases. Maximum recommended: {max_depth} levels."
                );
                let range = *deepest.ancestors.last().unwrap_or(&custom_key);
                cx.report_in(file, range, text.clone())
                    .node_path(format!("view.{}", deepest.full))
                    .ign_lint_path(Some(path))
                    .ign_lint_message(text);
            }
        }

        // 4. Data points.
        if leaves.len() > max_points {
            let text = format!(
                "Contains {} total data points across all custom properties. Large volumes of data should be stored in databases, not view JSON. Maximum recommended: {max_points} data points.",
                leaves.len()
            );
            cx.report_in(file, custom_key, text.clone())
                .node_path("view.custom")
                .ign_lint_path(Some("custom"))
                .ign_lint_message(text);
        }

        // 5. Long strings.
        for leaf in &leaves {
            let Some(s) = leaf.value.as_str() else {
                continue;
            };
            let length = s.chars().count();
            if length > max_length {
                let stripped = strip_indices(&leaf.full);
                let name = stripped.strip_prefix("custom.").unwrap_or(&stripped);
                let text = format!(
                    "Custom property '{name}' contains a string value of {length} characters. Large values should be stored in external resources or databases, not view JSON. Maximum recommended: {max_length} characters."
                );
                cx.report_in(file, leaf.value.range, text.clone())
                    .node_path(format!("view.{}", leaf.full))
                    .ign_lint_path(Some(leaf.full.clone()))
                    .ign_lint_message(text);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    const TIGHT: &str = "max-array-size = 2\nmax-sibling-properties = 2\nmax-nesting-depth = 2\nmax-data-points = 6\nmax-value-length = 5";

    fn lint(custom: &str, options: &str) -> Vec<(String, String)> {
        let view = format!(
            r#"{{"custom": {custom}, "root": {{"meta": {{"name": "root"}}, "custom": {{"big": [1, 2, 3, 4]}}}}}}"#
        );
        let d = Fixture::view(&view).lint_with(&[("VIEW006", options)]);
        d.iter()
            .map(|d| {
                (
                    d.ign_lint_path.clone().unwrap(),
                    flagged(std::slice::from_ref(d))[0].clone(),
                )
            })
            .collect()
    }

    #[test]
    fn small_data_is_fine() {
        assert!(lint(r#"{"a": [1, 2], "b": "short"}"#, TIGHT).is_empty());
        assert!(lint("{}", TIGHT).is_empty());
        // Defaults are generous.
        assert!(lint(r#"{"a": [1, 2, 3], "b": {"c": {"d": 1}}}"#, "").is_empty());
    }

    #[test]
    fn every_detector() {
        let found = lint(
            r#"{"list": [1, 2, 3], "deep": {"x": {"y": {"z": 1}}}, "text": "toolong"}"#,
            TIGHT,
        );
        let paths: Vec<&str> = found.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            paths,
            vec!["custom", "custom.list", "custom.deep.x.y.z", "custom.text"],
            "{found:?}"
        );
        let data_points = lint(r#"{"a": [1, 2], "b": [3, 4], "c": [5, 6], "d": 7}"#, TIGHT);
        assert!(
            data_points
                .iter()
                .any(|(p, f)| p == "custom" && f == "\"custom\"")
        );
    }

    #[test]
    fn nested_arrays_and_empty_objects_are_counted() {
        // ign-lint misses both (bug 24).
        let found = lint(
            r#"{"m": [[1, 2, 3]], "e": [{}, {}, {}]}"#,
            "max-array-size = 2",
        );
        let paths: Vec<&str> = found.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths, vec!["custom.m[0]", "custom.e"]);
    }

    #[test]
    fn top_level_breadth_is_measured() {
        let found = lint(r#"{"a": 1, "b": 2, "c": 3}"#, "max-sibling-properties = 2");
        assert_eq!(found, vec![("custom".to_owned(), "\"custom\"".to_owned())]);
    }

    #[test]
    fn component_custom_is_not_checked() {
        assert!(lint("{}", "max-array-size = 1").is_empty());
    }

    #[test]
    fn java_dates_are_single_values() {
        let found = lint(
            r#"{"d": {"$": ["ts", 192, 1700000000000], "$ts": 1700000000000}}"#,
            "max-nesting-depth = 1\nmax-sibling-properties = 1",
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].0, "custom.d._JavaDate");
    }
}
