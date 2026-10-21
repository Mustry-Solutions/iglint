//! Paths into a JSON tree.

use std::fmt;

/// One step of a [`JsonPath`]: an object key or an array index.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PathSegment {
    Key(String),
    Index(usize),
}

impl From<&str> for PathSegment {
    fn from(value: &str) -> Self {
        PathSegment::Key(value.to_owned())
    }
}

impl From<String> for PathSegment {
    fn from(value: String) -> Self {
        PathSegment::Key(value)
    }
}

impl From<usize> for PathSegment {
    fn from(value: usize) -> Self {
        PathSegment::Index(value)
    }
}

impl fmt::Display for PathSegment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathSegment::Key(k) => f.write_str(k),
            PathSegment::Index(i) => write!(f, "{i}"),
        }
    }
}

/// A path from a root value to a descendant.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct JsonPath(pub Vec<PathSegment>);

impl JsonPath {
    pub fn new() -> Self {
        JsonPath(Vec::new())
    }

    pub fn push(&mut self, segment: impl Into<PathSegment>) {
        self.0.push(segment.into());
    }

    pub fn pop(&mut self) -> Option<PathSegment> {
        self.0.pop()
    }

    /// A new path with `segment` appended.
    pub fn join(&self, segment: impl Into<PathSegment>) -> JsonPath {
        let mut p = self.clone();
        p.push(segment);
        p
    }

    pub fn segments(&self) -> &[PathSegment] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Parses an RFC 6901 JSON pointer. Every token becomes a
    /// [`PathSegment::Key`]; [`crate::Value::pointer`] resolves numeric keys on
    /// arrays.
    pub fn from_pointer(pointer: &str) -> Option<JsonPath> {
        parse_pointer(pointer).map(JsonPath)
    }

    /// The RFC 6901 JSON pointer for this path, for example `/root/children/0`.
    pub fn to_pointer(&self) -> String {
        let mut out = String::new();
        for s in &self.0 {
            out.push('/');
            match s {
                PathSegment::Key(k) => {
                    for c in k.chars() {
                        match c {
                            '~' => out.push_str("~0"),
                            '/' => out.push_str("~1"),
                            c => out.push(c),
                        }
                    }
                }
                PathSegment::Index(i) => out.push_str(&i.to_string()),
            }
        }
        out
    }

    /// Dotted form used in messages: keys joined by `.`, indexes as `[i]`,
    /// for example `root.children[0].props.text`.
    pub fn to_dotted(&self) -> String {
        let mut out = String::new();
        for s in &self.0 {
            match s {
                PathSegment::Key(k) => {
                    if !out.is_empty() {
                        out.push('.');
                    }
                    out.push_str(k);
                }
                PathSegment::Index(i) => {
                    out.push('[');
                    out.push_str(&i.to_string());
                    out.push(']');
                }
            }
        }
        out
    }
}

impl fmt::Display for JsonPath {
    /// Displays the JSON pointer form.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_pointer())
    }
}

impl From<Vec<PathSegment>> for JsonPath {
    fn from(value: Vec<PathSegment>) -> Self {
        JsonPath(value)
    }
}

impl std::ops::Deref for JsonPath {
    type Target = [PathSegment];
    fn deref(&self) -> &[PathSegment] {
        &self.0
    }
}

/// Parses an RFC 6901 pointer into key segments. `None` when the pointer is
/// not empty and does not start with `/`, or contains a bad `~` escape.
pub(crate) fn parse_pointer(pointer: &str) -> Option<Vec<PathSegment>> {
    if pointer.is_empty() {
        return Some(Vec::new());
    }
    let rest = pointer.strip_prefix('/')?;
    rest.split('/')
        .map(|token| {
            let mut out = String::with_capacity(token.len());
            let mut chars = token.chars();
            while let Some(c) = chars.next() {
                if c == '~' {
                    match chars.next() {
                        Some('0') => out.push('~'),
                        Some('1') => out.push('/'),
                        _ => return None,
                    }
                } else {
                    out.push(c);
                }
            }
            Some(PathSegment::Key(out))
        })
        .collect()
}
