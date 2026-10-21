//! Perspective component types and their property schemas.
//!
//! Schemas are the structural part of the JSON schemas Perspective registers
//! for each component (types, properties, items, enums, small defaults,
//! references). Descriptions and examples are not stored.

use crate::{generated, shared_schema};

/// Prefix of references to shared schemas, see [`crate::shared_schema`].
pub const SHARED_SCHEMA_PREFIX: &str = "urn:ignition-schema:schemas/";

/// JSON-schema `type` values, as a bit set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct TypeSet(pub u16);

impl TypeSet {
    pub const OBJECT: TypeSet = TypeSet(1);
    pub const ARRAY: TypeSet = TypeSet(1 << 1);
    pub const STRING: TypeSet = TypeSet(1 << 2);
    pub const NUMBER: TypeSet = TypeSet(1 << 3);
    pub const INTEGER: TypeSet = TypeSet(1 << 4);
    pub const BOOLEAN: TypeSet = TypeSet(1 << 5);
    pub const NULL: TypeSet = TypeSet(1 << 6);
    /// Perspective extension: a date.
    pub const DATE: TypeSet = TypeSet(1 << 7);
    /// Perspective extension: a dataset.
    pub const DATASET: TypeSet = TypeSet(1 << 8);
    /// Perspective extension: a color.
    pub const COLOR: TypeSet = TypeSet(1 << 9);
    /// Any other type name.
    pub const OTHER: TypeSet = TypeSet(1 << 15);

    /// Whether no type is declared (anything goes).
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether all types in `other` are in `self`.
    pub const fn contains(self, other: TypeSet) -> bool {
        self.0 & other.0 == other.0
    }
}

/// The `additionalProperties` of an object schema.
#[derive(Clone, Copy, Debug)]
pub enum Additional {
    /// Not specified; JSON schema allows extra keys.
    Unspecified,
    /// `true`.
    Allowed,
    /// `false`.
    Forbidden,
    /// A schema for extra keys.
    Schema(&'static Schema),
}

/// A JSON-schema node.
#[derive(Debug)]
pub struct Schema {
    pub types: TypeSet,
    /// Declared properties, sorted by name.
    pub properties: &'static [(&'static str, &'static Schema)],
    /// `definitions`, sorted by name (only on document roots).
    pub definitions: &'static [(&'static str, &'static Schema)],
    pub required: &'static [&'static str],
    pub items: Option<&'static Schema>,
    pub additional_properties: Additional,
    /// `$ref`, either `#/...` within the same document or
    /// `urn:ignition-schema:schemas/<name>`.
    pub reference: Option<&'static str>,
    pub one_of: &'static [&'static Schema],
    pub any_of: &'static [&'static Schema],
    pub all_of: &'static [&'static Schema],
    /// Allowed values as JSON text.
    pub enum_values: &'static [&'static str],
    /// Default value as JSON text (only small defaults are kept).
    pub default: Option<&'static str>,
    pub format: Option<&'static str>,
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
    pub pattern: Option<&'static str>,
    pub deprecated: bool,
}

impl Schema {
    pub(crate) const EMPTY: Schema = Schema {
        types: TypeSet(0),
        properties: &[],
        definitions: &[],
        required: &[],
        items: None,
        additional_properties: Additional::Unspecified,
        reference: None,
        one_of: &[],
        any_of: &[],
        all_of: &[],
        enum_values: &[],
        default: None,
        format: None,
        minimum: None,
        maximum: None,
        pattern: None,
        deprecated: false,
    };

    /// A declared property of this node (not following references).
    pub fn property(&self, name: &str) -> Option<&'static Schema> {
        lookup(self.properties, name)
    }

    /// A definition of this document root.
    pub fn definition(&self, name: &str) -> Option<&'static Schema> {
        lookup(self.definitions, name)
    }
}

fn lookup(
    table: &'static [(&'static str, &'static Schema)],
    name: &str,
) -> Option<&'static Schema> {
    table
        .binary_search_by(|(n, _)| (*n).cmp(name))
        .ok()
        .map(|i| table[i].1)
}

/// The schema that accepts anything.
static ANY: Schema = Schema::EMPTY;

/// A schema together with the document it belongs to, needed to resolve
/// `#/...` references.
#[derive(Clone, Copy)]
struct Located {
    root: &'static Schema,
    node: &'static Schema,
}

/// Follows `$ref` chains (at most a few levels).
fn resolve(mut at: Located) -> Located {
    for _ in 0..16 {
        let Some(reference) = at.node.reference else {
            return at;
        };
        let Some(next) = resolve_reference(at.root, reference) else {
            return at;
        };
        at = next;
    }
    at
}

fn resolve_reference(root: &'static Schema, reference: &str) -> Option<Located> {
    let (doc, pointer) = match reference.split_once('#') {
        Some((doc, pointer)) => (doc, pointer),
        None => (reference, ""),
    };
    let root = if doc.is_empty() {
        root
    } else {
        shared_schema(doc.strip_prefix(SHARED_SCHEMA_PREFIX)?)?
    };
    let mut node = root;
    let mut segments = pointer.split('/').filter(|s| !s.is_empty());
    while let Some(segment) = segments.next() {
        node = match segment {
            "definitions" => node.definition(segments.next()?)?,
            "properties" => node.property(segments.next()?)?,
            "items" => node.items?,
            "additionalProperties" => match node.additional_properties {
                Additional::Schema(s) => s,
                _ => return None,
            },
            "oneOf" => node.one_of.get(segments.next()?.parse::<usize>().ok()?)?,
            "anyOf" => node.any_of.get(segments.next()?.parse::<usize>().ok()?)?,
            "allOf" => node.all_of.get(segments.next()?.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(Located { root, node })
}

/// Finds `name` as a property of `at`, looking through references and
/// `allOf` / `anyOf` / `oneOf` alternatives, then `additionalProperties`.
fn child(at: Located, name: &str) -> Option<Located> {
    let at = resolve(at);
    if let Some(node) = at.node.property(name) {
        return Some(Located {
            root: at.root,
            node,
        });
    }
    for alt in at
        .node
        .all_of
        .iter()
        .chain(at.node.any_of)
        .chain(at.node.one_of)
    {
        if let Some(found) = child(
            Located {
                root: at.root,
                node: alt,
            },
            name,
        ) {
            return Some(found);
        }
    }
    match at.node.additional_properties {
        Additional::Schema(node) => Some(Located {
            root: at.root,
            node,
        }),
        Additional::Forbidden => None,
        Additional::Allowed => Some(Located {
            root: at.root,
            node: &ANY,
        }),
        Additional::Unspecified => None,
    }
}

fn element(at: Located) -> Option<Located> {
    let at = resolve(at);
    if let Some(node) = at.node.items {
        return Some(Located {
            root: at.root,
            node,
        });
    }
    at.node.any_of.iter().chain(at.node.one_of).find_map(|alt| {
        element(Located {
            root: at.root,
            node: alt,
        })
    })
}

/// Walks a dotted property path (`style.classes`, `items.0.text`,
/// `items[0].text`) from `root`.
fn walk(root: &'static Schema, path: &str) -> Option<&'static Schema> {
    let mut at = Located { root, node: root };
    for raw in path.split('.').filter(|s| !s.is_empty()) {
        let (name, indexes) = match raw.find('[') {
            Some(i) => (&raw[..i], &raw[i..]),
            None => (raw, ""),
        };
        if !name.is_empty() {
            at = if name.bytes().all(|b| b.is_ascii_digit()) {
                element(at)?
            } else {
                child(at, name)?
            };
        }
        for _ in indexes.matches('[') {
            at = element(at)?;
        }
    }
    Some(resolve(at).node)
}

/// A Perspective component event (`onActionPerformed`, `onRowClick`, ...).
#[derive(Debug)]
pub struct ComponentEvent {
    pub name: &'static str,
    /// Schema of the `event` object passed to the script.
    pub schema: Option<&'static Schema>,
}

/// An extension function a component lets you implement, e.g. `filterAlarm`.
#[derive(Debug)]
pub struct ExtensionFunction {
    pub name: &'static str,
    /// Python signature, e.g. `filterAlarm(self, alarmEvent)`.
    pub signature: &'static str,
}

/// A registered Perspective component type.
#[derive(Debug)]
pub struct ComponentType {
    /// Type id as stored in `view.json`, e.g. `ia.display.label`.
    pub id: &'static str,
    /// Display name, e.g. `Label`.
    pub name: &'static str,
    pub palette_category: Option<&'static str>,
    pub module_id: &'static str,
    pub deprecated: bool,
    /// Schema of `props`.
    pub props: &'static Schema,
    /// Schema of `position` for children of this container.
    pub child_position: Option<&'static Schema>,
    pub events: &'static [ComponentEvent],
    pub extension_functions: &'static [ExtensionFunction],
}

impl ComponentType {
    /// Schema of a property below `props`, e.g. `text`, `style.classes` or
    /// `columns[0].field`. References are resolved. Returns `None` when the
    /// schema does not declare the property (which does not always make it
    /// invalid: objects without `additionalProperties` accept extra keys).
    pub fn prop(&self, path: &str) -> Option<&'static Schema> {
        walk(self.props, path)
    }

    /// Schema of a property below a child's `position` in this container.
    pub fn child_position_prop(&self, path: &str) -> Option<&'static Schema> {
        walk(self.child_position?, path)
    }

    /// A component event by name.
    pub fn event(&self, name: &str) -> Option<&'static ComponentEvent> {
        self.events.iter().find(|e| e.name == name)
    }

    /// Whether the top level of `props` forbids undeclared keys
    /// (`additionalProperties: false`).
    pub fn props_closed(&self) -> bool {
        matches!(self.props.additional_properties, Additional::Forbidden)
    }
}

/// Schema of a property of every component's `meta` (`name`, `visible`, ...).
pub fn meta_prop(path: &str) -> Option<&'static Schema> {
    walk(shared_schema("meta-schema.json")?, path)
}

/// Resolves a property path inside any shared schema, e.g.
/// `shared_prop("style-properties.schema.json", "classes")`.
pub fn shared_prop(schema: &str, path: &str) -> Option<&'static Schema> {
    walk(shared_schema(schema)?, path)
}

/// All shared schemas, sorted by name.
pub fn shared_schemas() -> &'static [(&'static str, &'static Schema)] {
    generated::SHARED_SCHEMAS
}
