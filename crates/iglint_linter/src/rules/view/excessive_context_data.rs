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

/// An index-free object path (`custom.a.b`) in the trie of paths that lead
/// to values.
struct PathNode {
    parent: usize,
    name: String,
    /// Whether a value lies at or below the path (its name is counted).
    used: bool,
    /// Whether a value lies below the path (it is a parent).
    prefix: bool,
    /// Distinct names of the children with values below, in first-seen
    /// order.
    children: usize,
    /// Key range of the member on the first path that reached a value.
    range: TextRange,
}

/// The path trie: node 0 is `custom`.
struct Paths {
    nodes: Vec<PathNode>,
    index: FxHashMap<(usize, String), usize>,
    /// Parent nodes, in the order their first value was seen.
    order: Vec<usize>,
}

impl Paths {
    fn child(&mut self, parent: usize, name: &str) -> usize {
        if let Some(&id) = self.index.get(&(parent, name.to_owned())) {
            return id;
        }
        let id = self.nodes.len();
        self.nodes.push(PathNode {
            parent,
            name: name.to_owned(),
            used: false,
            prefix: false,
            children: 0,
            range: TextRange::default(),
        });
        self.index.insert((parent, name.to_owned()), id);
        id
    }

    /// Records a value at `node`; `ranges[i]` is the key range of the
    /// member at index-free depth `i` on the current path. Amortized
    /// constant: the walk up stops at the first known prefix.
    fn add_value(&mut self, node: usize, ranges: &[TextRange]) {
        let mut fresh = Vec::new();
        let mut id = node;
        let mut depth = ranges.len() - 1;
        if !self.nodes[id].used {
            self.nodes[id].used = true;
            if id != 0 {
                let parent = self.nodes[id].parent;
                self.nodes[parent].children += 1;
            }
        }
        while id != 0 {
            let parent = self.nodes[id].parent;
            depth -= 1;
            let known = self.nodes[parent].prefix;
            if !known {
                self.nodes[parent].prefix = true;
                self.nodes[parent].range = ranges[depth];
                fresh.push(parent);
            }
            let used = self.nodes[parent].used;
            if !used {
                self.nodes[parent].used = true;
                if parent != 0 {
                    let grandparent = self.nodes[parent].parent;
                    self.nodes[grandparent].children += 1;
                }
            }
            if known && used {
                break;
            }
            id = parent;
        }
        // Shorter prefixes first.
        self.order.extend(fresh.iter().rev());
    }

    /// `custom.a.b` for a node.
    fn dotted(&self, mut id: usize) -> String {
        let mut names = vec![self.nodes[id].name.as_str()];
        while id != 0 {
            id = self.nodes[id].parent;
            names.push(self.nodes[id].name.as_str());
        }
        names.reverse();
        names.join(".")
    }
}

/// A container under `custom` whose items are being walked.
enum Items<'v, 'a> {
    Members(std::slice::Iter<'v, iglint_json::Member<'a>>),
    Elements(std::iter::Enumerate<std::slice::Iter<'v, Value<'a>>>),
}

struct Frame<'v, 'a> {
    items: Items<'v, 'a>,
    /// Length of the indexed path of the container.
    full: usize,
    /// Its index-free trie node and depth (`ranges` length).
    node: usize,
    depth: usize,
}

/// Everything VIEW006 measures, collected in one walk without recursion
/// and without copying a path per value.
struct Measures<'a> {
    /// Arrays above the size limit: indexed path, array, key range.
    arrays: Vec<(String, &'a Value<'a>, TextRange)>,
    paths: Paths,
    /// The first deepest value: index-free depth, dotted path, indexed
    /// path, key range.
    deepest: Option<(usize, String, String, TextRange)>,
    points: usize,
    /// Strings above the length limit: indexed path, value, length.
    long: Vec<(String, &'a Value<'a>, usize)>,
}

fn measure<'a>(custom: &'a Value<'a>, custom_key: TextRange, o: &Options) -> Measures<'a> {
    let mut m = Measures {
        arrays: Vec::new(),
        paths: Paths {
            nodes: vec![PathNode {
                parent: 0,
                name: "custom".to_owned(),
                used: false,
                prefix: false,
                children: 0,
                range: custom_key,
            }],
            index: FxHashMap::default(),
            order: Vec::new(),
        },
        deepest: None,
        points: 0,
        long: Vec::new(),
    };
    let mut full = "custom".to_owned();
    let mut ranges = vec![custom_key];
    let mut stack: Vec<Frame<'_, 'a>> = Vec::new();
    // The value to visit, whether it is an array element, its trie node.
    let mut next: Option<(&'a Value<'a>, bool, usize)> = Some((custom, false, 0));
    loop {
        if let Some((value, in_array, node)) = next.take() {
            let depth = ranges.len();
            let leaf = |m: &mut Measures<'a>, node: usize, full: &str, ranges: &[TextRange]| {
                // `custom` itself is not a value.
                if node == 0 {
                    return;
                }
                m.points += 1;
                m.paths.add_value(node, ranges);
                let stripped = ranges.len();
                if m.deepest.as_ref().is_none_or(|d| d.0 < stripped) {
                    m.deepest = Some((
                        stripped,
                        m.paths.dotted(node),
                        full.to_owned(),
                        *ranges.last().unwrap_or(&custom_key),
                    ));
                }
                if let Some(s) = value.as_str() {
                    let length = s.chars().count();
                    if length > o.length {
                        m.long.push((full.to_owned(), value, length));
                    }
                }
            };
            match &value.kind {
                ValueKind::Object(object) if is_java_date(value) => {
                    if let Some(dollar) = object.member("$")
                        && dollar.value.as_array().is_some_and(|a| a.len() > o.array)
                    {
                        m.arrays
                            .push((format!("{full}.$"), &dollar.value, dollar.key.range()));
                    }
                    let len = full.len();
                    full.push_str("._JavaDate");
                    let last = *ranges.last().unwrap_or(&value.range);
                    ranges.push(last);
                    let date = m.paths.child(node, "_JavaDate");
                    leaf(&mut m, date, &full, &ranges);
                    ranges.pop();
                    full.truncate(len);
                }
                ValueKind::Object(object) if !object.is_empty() => stack.push(Frame {
                    items: Items::Members(object.iter()),
                    full: full.len(),
                    node,
                    depth,
                }),
                ValueKind::Array(array) if !array.is_empty() => {
                    if array.len() > o.array {
                        m.arrays.push((full.clone(), value, ranges[depth - 1]));
                    }
                    stack.push(Frame {
                        items: Items::Elements(array.iter().enumerate()),
                        full: full.len(),
                        node,
                        depth,
                    });
                }
                // Empty containers are values only as members.
                ValueKind::Object(_) | ValueKind::Array(_) => {
                    if !in_array {
                        leaf(&mut m, node, &full, &ranges);
                    }
                }
                _ => leaf(&mut m, node, &full, &ranges),
            }
        }
        let Some(frame) = stack.last_mut() else {
            return m;
        };
        full.truncate(frame.full);
        ranges.truncate(frame.depth);
        match &mut frame.items {
            Items::Members(members) => {
                if let Some(member) = members.next() {
                    full.push('.');
                    full.push_str(member.key());
                    ranges.push(member.key.range());
                    let node = frame.node;
                    let child = m.paths.child(node, member.key());
                    next = Some((&member.value, false, child));
                    continue;
                }
            }
            Items::Elements(elements) => {
                if let Some((i, element)) = elements.next() {
                    use std::fmt::Write as _;
                    let _ = write!(full, "[{i}]");
                    next = Some((element, true, frame.node));
                    continue;
                }
            }
        }
        stack.pop();
    }
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
        let m = measure(custom, custom_key, o);

        // 1. Arrays.
        for (path, array, key) in m.arrays {
            let size = array.as_array().map_or(0, |a| a.len());
            let name = path.replace("custom.", "");
            let text = format!(
                "Custom property '{name}' contains {size} items. Large datasets should be stored in databases, not view JSON. Maximum recommended: {max_array} items."
            );
            cx.report_in(file, key, text.clone())
                .node_path(format!("view.{path}"))
                .ign_lint_path(Some(path))
                .ign_lint_message(text);
        }

        // 2. Breadth: distinct child names per (index-free) object path.
        for &id in &m.paths.order {
            let node = &m.paths.nodes[id];
            if node.children > max_siblings {
                let parent = m.paths.dotted(id);
                let text = format!(
                    "Contains {} sibling properties. Large flat structures should be stored in databases, not view JSON. Maximum recommended: {max_siblings} siblings.",
                    node.children
                );
                cx.report_in(file, node.range, text.clone())
                    .node_path(format!("view.{parent}"))
                    .ign_lint_path(Some(parent))
                    .ign_lint_message(text);
            }
        }

        // 3. Depth: the first deepest value.
        if let Some((stripped, path, full, range)) = m.deepest {
            let depth = stripped - 1;
            if depth > max_depth {
                let text = format!(
                    "Custom properties are nested {depth} levels deep. Deeply nested structures indicate complex data that should be in databases. Maximum recommended: {max_depth} levels."
                );
                cx.report_in(file, range, text.clone())
                    .node_path(format!("view.{full}"))
                    .ign_lint_path(Some(path))
                    .ign_lint_message(text);
            }
        }

        // 4. Data points.
        if m.points > max_points {
            let text = format!(
                "Contains {} total data points across all custom properties. Large volumes of data should be stored in databases, not view JSON. Maximum recommended: {max_points} data points.",
                m.points
            );
            cx.report_in(file, custom_key, text.clone())
                .node_path("view.custom")
                .ign_lint_path(Some("custom"))
                .ign_lint_message(text);
        }

        // 5. Long strings.
        for (full, value, length) in m.long {
            let stripped = strip_indices(&full);
            let name = stripped.strip_prefix("custom.").unwrap_or(&stripped);
            let text = format!(
                "Custom property '{name}' contains a string value of {length} characters. Large values should be stored in external resources or databases, not view JSON. Maximum recommended: {max_length} characters."
            );
            cx.report_in(file, value.range, text.clone())
                .node_path(format!("view.{full}"))
                .ign_lint_path(Some(full))
                .ign_lint_message(text);
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
