//! Which fields of Perspective binding and transform configurations
//! Ignition parses as expressions or property paths, and which members of
//! the component script object return the component's name.
//!
//! Both are measured inside the Ignition 8.3.3 image by
//! `stubs-gen/gen_perspective_fields.py`, not read from documentation:
//! every binding and transform type registered by Perspective is built
//! from its own JSON schema, one field at a time replaced by a marker
//! reference, and a field is an expression (or a property path) when the
//! marker reaches Perspective's property reference parser. Every
//! Perspective method that calls an expression or property parser must be
//! reached by the probe or be classified, so a field of an event action or
//! of a component configuration cannot be parsed without being listed.
//! The name members come from a return-value flow analysis of the
//! bytecode (a value derived from `PerspectiveElement.getName()`).
//!
//! The view model ([`crate::view`]) models every listed expression field
//! as an [`crate::EmbeddedExpression`], so that the `BIND` rules,
//! `VIEW015` and `NAME001` see its references
//! (`every_measured_field_is_modelled`).

use iglint_json::{JsonString, Value, ValueKind};

/// What holds a configuration field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FieldOwner {
    /// A binding; paths are relative to its `config`.
    Binding,
    /// A transform; paths are relative to the transform object.
    Transform,
}

impl FieldOwner {
    fn parse(s: &str) -> FieldOwner {
        match s {
            "binding" => FieldOwner::Binding,
            "transform" => FieldOwner::Transform,
            other => unreachable!("generated owner {other}"),
        }
    }
}

/// How Perspective parses a field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FieldSink {
    /// An Ignition expression.
    Expression,
    /// A property path (`this.props.text`, `../Label.props.text`).
    PropertyPath,
}

/// One measured configuration field.
#[derive(Clone, Copy, Debug)]
pub struct ConfigField {
    pub owner: FieldOwner,
    /// The binding or transform `type`.
    pub type_name: &'static str,
    /// `request.header[].value`: `*` is any key of a free-form object,
    /// `[]` any item of a list, a final `**` any value at any depth below.
    pub path: &'static str,
    pub sink: FieldSink,
    /// The field is parsed only when the value at this path (relative to
    /// the same root) is one of `values`.
    pub condition: Option<(&'static str, &'static [&'static str])>,
}

/// Every measured field.
pub fn config_fields() -> impl Iterator<Item = ConfigField> {
    crate::generated::CONFIG_FIELDS
        .iter()
        .map(
            |&(owner, type_name, path, sink, field, values)| ConfigField {
                owner: FieldOwner::parse(owner),
                type_name,
                path,
                sink: match sink {
                    "expression" => FieldSink::Expression,
                    "property" => FieldSink::PropertyPath,
                    other => unreachable!("generated sink {other}"),
                },
                condition: (!field.is_empty()).then_some((field, values)),
            },
        )
}

/// The measured fields of one binding or transform type.
pub fn fields_of(owner: FieldOwner, type_name: &str) -> impl Iterator<Item = ConfigField> + '_ {
    config_fields().filter(move |f| f.owner == owner && f.type_name == type_name)
}

/// Members of the component script object (Java and bean spellings,
/// including the script-callable methods of component model delegates)
/// whose value may derive from the component's name: `name`, `getName`.
pub fn component_name_members() -> &'static [&'static str] {
    crate::generated::COMPONENT_NAME_MEMBERS
}

/// Whether `name` is a member of the component script object that returns
/// the component's name.
pub fn is_component_name_member(name: &str) -> bool {
    component_name_members().contains(&name)
}

impl ConfigField {
    /// Whether the field is parsed in the configuration `root`.
    pub fn applies(&self, root: &Value<'_>) -> bool {
        let Some((field, values)) = self.condition else {
            return true;
        };
        let mut at = Some(root);
        for key in field.split('.') {
            at = at.and_then(|v| v.get(key));
        }
        values
            .iter()
            .any(|expected| match (expected, at.map(|v| &v.kind)) {
                (&"absent", None) => true,
                (&"true", Some(ValueKind::Bool(true))) => true,
                (&"false", Some(ValueKind::Bool(false))) => true,
                (e, Some(ValueKind::String(s))) => {
                    e.strip_prefix('"').and_then(|e| e.strip_suffix('"')) == Some(s.value())
                }
                _ => false,
            })
    }

    /// Every JSON string at the field's path below `root`, with the path
    /// of each (`request.header[0].value`).
    pub fn strings<'a>(&self, root: &'a Value<'a>) -> Vec<(String, &'a JsonString<'a>)> {
        let steps = steps(self.path);
        let mut out = Vec::new();
        walk(root, &steps, String::new(), &mut out);
        out
    }
}

#[derive(Debug, PartialEq)]
enum Step<'p> {
    Key(&'p str),
    AnyKey,
    Items,
    Recursive,
}

fn steps(path: &str) -> Vec<Step<'_>> {
    let mut out = Vec::new();
    for part in path.split('.') {
        let name = part.trim_end_matches("[]");
        let items = (part.len() - name.len()) / 2;
        out.push(match name {
            "*" => Step::AnyKey,
            "**" => Step::Recursive,
            key => Step::Key(key),
        });
        out.extend((0..items).map(|_| Step::Items));
    }
    out
}

fn join(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        key.to_owned()
    } else {
        format!("{prefix}.{key}")
    }
}

fn walk<'a>(
    v: &'a Value<'a>,
    steps: &[Step<'_>],
    path: String,
    out: &mut Vec<(String, &'a JsonString<'a>)>,
) {
    let Some((first, rest)) = steps.split_first() else {
        if let ValueKind::String(s) = &v.kind {
            out.push((path, s));
        }
        return;
    };
    match (first, &v.kind) {
        (Step::Key(k), ValueKind::Object(o)) => {
            if let Some(child) = o.get(k) {
                walk(child, rest, join(&path, k), out);
            }
        }
        (Step::AnyKey, ValueKind::Object(o)) => {
            for m in o.iter() {
                walk(&m.value, rest, join(&path, m.key()), out);
            }
        }
        (Step::Items, ValueKind::Array(a)) => {
            for (i, item) in a.iter().enumerate() {
                walk(item, rest, format!("{path}[{i}]"), out);
            }
        }
        (Step::Recursive, ValueKind::Object(o)) => {
            for m in o.iter() {
                walk(&m.value, &[], join(&path, m.key()), out);
                walk(&m.value, steps, join(&path, m.key()), out);
            }
        }
        (Step::Recursive, ValueKind::Array(a)) => {
            for (i, item) in a.iter().enumerate() {
                walk(item, &[], format!("{path}[{i}]"), out);
                walk(item, steps, format!("{path}[{i}]"), out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fnv1a64(data: &[u8]) -> String {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in data {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{h:016x}")
    }

    /// The generated table was produced by the generator as it is now.
    #[test]
    fn generated_fields_match_the_generator() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../stubs-gen/gen_perspective_fields.py"
        );
        let source = std::fs::read_to_string(path).expect("generator");
        let source = source.replace("\r\n", "\n");
        assert_eq!(
            fnv1a64(source.as_bytes()),
            crate::generated::GENERATOR_HASH,
            "stubs-gen/gen_perspective_fields.py changed; rerun \
             `stubs-gen/perspective-fields.sh` to regenerate \
             src/generated/perspective_fields.rs"
        );
    }

    #[test]
    fn generated_tables_are_sorted_and_well_formed() {
        let rows = crate::generated::CONFIG_FIELDS;
        assert!(rows.windows(2).all(|w| w[0] < w[1]));
        for (_, _, path, _, field, values) in rows {
            assert!(!steps(path).is_empty());
            assert_eq!(field.is_empty(), values.is_empty());
            for v in *values {
                assert!(
                    matches!(*v, "absent" | "true" | "false")
                        || v.strip_prefix('"')
                            .and_then(|v| v.strip_suffix('"'))
                            .is_some_and(|v| !v.contains(['"', '\\'])),
                    "condition value {v}"
                );
            }
        }
        let names = component_name_members();
        assert!(names.windows(2).all(|w| w[0] < w[1]));
        // The measurement must at least see the obvious ones.
        assert!(is_component_name_member("name") && is_component_name_member("getName"));
    }

    /// A JSON object with `value` at `path` (`*` and `**` as key `k1`,
    /// `[]` as the first item).
    fn put(root: &mut serde_json::Value, path: &str, value: serde_json::Value) {
        let mut at = root;
        let steps = steps(path);
        for (i, step) in steps.iter().enumerate() {
            let last = i + 1 == steps.len();
            let next = if last {
                value.clone()
            } else if matches!(steps[i + 1], Step::Items) {
                serde_json::json!([])
            } else {
                serde_json::json!({})
            };
            at = match step {
                Step::Key(k) => at.as_object_mut().unwrap().entry(*k).or_insert(next),
                Step::AnyKey | Step::Recursive => {
                    at.as_object_mut().unwrap().entry("k1").or_insert(next)
                }
                Step::Items => {
                    let a = at.as_array_mut().unwrap();
                    if a.is_empty() {
                        a.push(next);
                    }
                    &mut a[0]
                }
            };
        }
    }

    /// Every measured expression field of every binding and transform
    /// type is an expression of the view model, and every property path
    /// a property binding path: the BIND rules, VIEW015 and NAME001 see
    /// the references in all of them.
    #[test]
    fn every_measured_field_is_modelled() {
        for f in config_fields() {
            let marker = "{this.custom.iglintMarker}";
            let mut root = serde_json::json!({});
            put(&mut root, f.path, serde_json::json!(marker));
            if let Some((field, values)) = f.condition {
                let v = values[0];
                if v != "absent" {
                    put(&mut root, field, serde_json::from_str(v).unwrap());
                }
            }
            let binding = match f.owner {
                FieldOwner::Binding => {
                    serde_json::json!({"type": f.type_name, "config": root})
                }
                FieldOwner::Transform => {
                    let mut t = root.clone();
                    t.as_object_mut()
                        .unwrap()
                        .insert("type".into(), serde_json::json!(f.type_name));
                    serde_json::json!({"type": "expr", "config": {"expression": "1"},
                        "transforms": [t]})
                }
            };
            let view = serde_json::json!({
                "custom": {"iglintMarker": 1},
                "root": {"type": "ia.container.flex", "meta": {"name": "root"},
                    "propConfig": {"props.text": {"binding": binding}}}
            });
            let text = view.to_string();
            let parsed = iglint_json::parse(&text).unwrap();
            let model = crate::view::build_view(&parsed);
            let found = match f.sink {
                FieldSink::Expression => model.expressions.iter().any(|e| e.text() == marker),
                FieldSink::PropertyPath => model.bindings.iter().any(|b| {
                    matches!(&b.typed, crate::view::BindingConfig::Property(p)
                        if p.path.map(|p| p.value()) == Some(marker))
                }),
            };
            assert!(found, "{f:?} is not modelled: {text}");
        }
    }

    #[test]
    fn patterns_find_strings() {
        let src = r#"{"request": {"header": [{"key": "a", "value": "x"}, {"value": "y"}]},
            "struct": {"a": "1", "b": {"c": "2", "d": ["3", {"e": "4"}]}}}"#;
        let root = &iglint_json::parse(src).unwrap();
        let field = |path| ConfigField {
            owner: FieldOwner::Binding,
            type_name: "t",
            path,
            sink: FieldSink::Expression,
            condition: None,
        };
        let got: Vec<String> = field("request.header[].value")
            .strings(root)
            .into_iter()
            .map(|(p, s)| format!("{p}={}", s.value()))
            .collect();
        assert_eq!(
            got,
            ["request.header[0].value=x", "request.header[1].value=y"]
        );
        let got: Vec<String> = field("struct.**")
            .strings(root)
            .into_iter()
            .map(|(p, s)| format!("{p}={}", s.value()))
            .collect();
        assert_eq!(
            got,
            [
                "struct.a=1",
                "struct.b.c=2",
                "struct.b.d[0]=3",
                "struct.b.d[1].e=4"
            ]
        );
    }

    #[test]
    fn conditions() {
        let root =
            &iglint_json::parse(r#"{"mode": "expression", "polling": {"enabled": true}}"#).unwrap();
        let f = |field, values| ConfigField {
            owner: FieldOwner::Binding,
            type_name: "t",
            path: "x",
            sink: FieldSink::Expression,
            condition: Some((field, values)),
        };
        assert!(f("mode", &["\"expression\""]).applies(root));
        assert!(!f("mode", &["\"indirect\""]).applies(root));
        assert!(f("polling.enabled", &["true"]).applies(root));
        assert!(!f("polling.rate", &["true"]).applies(root));
        assert!(f("polling.rate", &["absent"]).applies(root));
    }
}
