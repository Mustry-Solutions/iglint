//! The span-preserving value tree.

use std::borrow::Cow;

use iglint_text::{Ranged, TextRange, TextSize};

use crate::path::PathSegment;
use crate::string::OffsetMap;

/// A JSON value together with the byte range it occupies in the source.
#[derive(Clone, Debug, PartialEq)]
pub struct Value<'a> {
    pub range: TextRange,
    pub kind: ValueKind<'a>,
}

/// The kind of a [`Value`].
#[derive(Clone, Debug, PartialEq)]
pub enum ValueKind<'a> {
    Null,
    Bool(bool),
    Number(Number<'a>),
    String(JsonString<'a>),
    Array(Array<'a>),
    Object(Object<'a>),
}

impl Ranged for Value<'_> {
    fn range(&self) -> TextRange {
        self.range
    }
}

impl<'a> Value<'a> {
    /// A short name of the value kind, for messages: `"null"`, `"boolean"`,
    /// `"number"`, `"string"`, `"array"` or `"object"`.
    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            ValueKind::Null => "null",
            ValueKind::Bool(_) => "boolean",
            ValueKind::Number(_) => "number",
            ValueKind::String(_) => "string",
            ValueKind::Array(_) => "array",
            ValueKind::Object(_) => "object",
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self.kind, ValueKind::Null)
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self.kind {
            ValueKind::Bool(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<&Number<'a>> {
        match &self.kind {
            ValueKind::Number(n) => Some(n),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        self.as_number().and_then(Number::as_f64)
    }

    pub fn as_i64(&self) -> Option<i64> {
        self.as_number().and_then(Number::as_i64)
    }

    /// The string node, with raw text and offset mapping.
    pub fn as_json_string(&self) -> Option<&JsonString<'a>> {
        match &self.kind {
            ValueKind::String(s) => Some(s),
            _ => None,
        }
    }

    /// The decoded string value.
    pub fn as_str(&self) -> Option<&str> {
        self.as_json_string().map(JsonString::value)
    }

    pub fn as_array(&self) -> Option<&Array<'a>> {
        match &self.kind {
            ValueKind::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&Object<'a>> {
        match &self.kind {
            ValueKind::Object(o) => Some(o),
            _ => None,
        }
    }

    /// Object member value by key (last occurrence wins, as in Gson and most
    /// JSON readers). `None` for non-objects.
    pub fn get(&self, key: &str) -> Option<&Value<'a>> {
        self.as_object().and_then(|o| o.get(key))
    }

    /// Array element by index. `None` for non-arrays.
    pub fn index(&self, index: usize) -> Option<&Value<'a>> {
        self.as_array().and_then(|a| a.get(index))
    }

    /// Follows a path of keys and indexes.
    pub fn get_path(&self, path: &[PathSegment]) -> Option<&Value<'a>> {
        let mut current = self;
        for segment in path {
            current = match segment {
                PathSegment::Key(key) => current.get(key)?,
                PathSegment::Index(index) => current.index(*index)?,
            };
        }
        Some(current)
    }

    /// Follows an RFC 6901 JSON pointer such as `/root/children/0/props`.
    /// The empty pointer returns `self`.
    pub fn pointer(&self, pointer: &str) -> Option<&Value<'a>> {
        let path = crate::path::parse_pointer(pointer)?;
        let mut current = self;
        for segment in &path {
            current = match segment {
                PathSegment::Key(key) => match &current.kind {
                    ValueKind::Object(o) => o.get(key)?,
                    ValueKind::Array(a) => a.get(key.parse::<usize>().ok()?)?,
                    _ => return None,
                },
                PathSegment::Index(index) => current.index(*index)?,
            };
        }
        Some(current)
    }

    /// Calls `f` for this value and every descendant in document order, with
    /// the path from `self`. Return `false` from `f` to skip the children of
    /// that value.
    pub fn walk<F>(&self, mut f: F)
    where
        F: FnMut(&[PathSegment], &Value<'a>) -> bool,
    {
        let mut path = Vec::new();
        walk_inner(self, &mut path, &mut f);
    }

    /// Finds the innermost value whose range contains `offset`, with its path.
    pub fn value_at_offset(&self, offset: TextSize) -> Option<(Vec<PathSegment>, &Value<'a>)> {
        if !self.range.contains(offset) {
            return None;
        }
        let mut path = Vec::new();
        let mut current = self;
        'outer: loop {
            match &current.kind {
                ValueKind::Object(o) => {
                    for m in &o.members {
                        if m.value.range.contains(offset) {
                            path.push(PathSegment::Key(m.key.value().to_owned()));
                            current = &m.value;
                            continue 'outer;
                        }
                    }
                }
                ValueKind::Array(a) => {
                    for (i, e) in a.elements.iter().enumerate() {
                        if e.range.contains(offset) {
                            path.push(PathSegment::Index(i));
                            current = e;
                            continue 'outer;
                        }
                    }
                }
                _ => {}
            }
            return Some((path, current));
        }
    }
}

/// The children of a container that [`Value::walk`] has yet to visit.
enum Pending<'v, 'a> {
    Members(std::slice::Iter<'v, Member<'a>>),
    Elements(std::iter::Enumerate<std::slice::Iter<'v, Value<'a>>>),
}

impl<'v, 'a> Pending<'v, 'a> {
    /// The children of `value`, when it has any.
    fn of(value: &'v Value<'a>) -> Option<Self> {
        match &value.kind {
            ValueKind::Object(o) if !o.is_empty() => Some(Pending::Members(o.members.iter())),
            ValueKind::Array(a) if !a.is_empty() => {
                Some(Pending::Elements(a.elements.iter().enumerate()))
            }
            _ => None,
        }
    }
}

/// Document order walk with an explicit stack, so that any depth works.
fn walk_inner<'a, F>(value: &Value<'a>, path: &mut Vec<PathSegment>, f: &mut F)
where
    F: FnMut(&[PathSegment], &Value<'a>) -> bool,
{
    if !f(path, value) {
        return;
    }
    let mut stack: Vec<Pending<'_, 'a>> = Pending::of(value).into_iter().collect();
    while let Some(top) = stack.last_mut() {
        let next = match top {
            Pending::Members(members) => members
                .next()
                .map(|m| (PathSegment::Key(m.key.value().to_owned()), &m.value)),
            Pending::Elements(elements) => elements.next().map(|(i, e)| (PathSegment::Index(i), e)),
        };
        match next {
            Some((segment, child)) => {
                path.push(segment);
                let children = if f(path, child) {
                    Pending::of(child)
                } else {
                    None
                };
                match children {
                    Some(children) => stack.push(children),
                    None => {
                        path.pop();
                    }
                }
            }
            None => {
                stack.pop();
                if !stack.is_empty() {
                    path.pop();
                }
            }
        }
    }
}

/// Moves the children of `value` that have children of their own to
/// `pending`.
fn take_children<'a>(value: &mut Value<'a>, pending: &mut Vec<Value<'a>>) {
    match &mut value.kind {
        ValueKind::Array(a) => a.take_nested(pending),
        ValueKind::Object(o) => o.take_nested(pending),
        _ => {}
    }
}

/// Whether the value is an array or object with at least one item.
fn has_children(value: &Value<'_>) -> bool {
    match &value.kind {
        ValueKind::Array(a) => !a.is_empty(),
        ValueKind::Object(o) => !o.is_empty(),
        _ => false,
    }
}

/// Frees the values in `pending` and everything below them without
/// recursion.
fn drop_all(mut pending: Vec<Value<'_>>) {
    while let Some(mut value) = pending.pop() {
        take_children(&mut value, &mut pending);
        // `value` is dropped here with only leaf children left.
    }
}

/// A JSON number. The raw source text is kept; parsing to a Rust number is on
/// demand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Number<'a> {
    raw: &'a str,
}

impl<'a> Number<'a> {
    pub(crate) fn new(raw: &'a str) -> Self {
        Number { raw }
    }

    /// The number exactly as written, for example `1.0`, `-2.5E-4` or `NaN`.
    pub fn raw(&self) -> &'a str {
        self.raw
    }

    /// True when the literal has no fraction and no exponent.
    pub fn is_integer_literal(&self) -> bool {
        !self.raw.bytes().any(|b| matches!(b, b'.' | b'e' | b'E')) && !self.is_special()
    }

    /// True for the non-RFC literals `NaN`, `Infinity` and `-Infinity`.
    pub fn is_special(&self) -> bool {
        matches!(self.raw, "NaN" | "Infinity" | "-Infinity")
    }

    /// The value as `f64`. Always `Some` for numbers produced by the parser.
    pub fn as_f64(&self) -> Option<f64> {
        match self.raw {
            "NaN" => Some(f64::NAN),
            "Infinity" => Some(f64::INFINITY),
            "-Infinity" => Some(f64::NEG_INFINITY),
            raw => raw.parse().ok(),
        }
    }

    /// The value as `i64` when the literal is an integer literal that fits.
    pub fn as_i64(&self) -> Option<i64> {
        if self.is_integer_literal() {
            self.raw.parse().ok()
        } else {
            None
        }
    }

    /// The value as `u64` when the literal is a non-negative integer literal
    /// that fits.
    pub fn as_u64(&self) -> Option<u64> {
        if self.is_integer_literal() {
            self.raw.parse().ok()
        } else {
            None
        }
    }
}

/// A JSON string: its decoded value, the raw text between the quotes and the
/// source range.
///
/// Diagnostics inside embedded code (scripts, expressions) are computed on the
/// decoded value. [`JsonString::raw_offset`] and [`JsonString::raw_range`] map
/// such positions back to the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonString<'a> {
    /// Range including the quotes.
    range: TextRange,
    /// Text between the quotes, as written.
    raw: &'a str,
    /// Decoded value. Borrowed from the source when there are no escapes.
    value: Cow<'a, str>,
}

impl Ranged for JsonString<'_> {
    fn range(&self) -> TextRange {
        self.range
    }
}

impl<'a> JsonString<'a> {
    pub(crate) fn new(range: TextRange, raw: &'a str, value: Cow<'a, str>) -> Self {
        JsonString { range, raw, value }
    }

    /// The decoded value.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// The decoded value as a `Cow` (borrowed from the source when possible).
    pub fn value_cow(&self) -> &Cow<'a, str> {
        &self.value
    }

    /// The raw text between the quotes.
    pub fn raw(&self) -> &'a str {
        self.raw
    }

    /// Range of the whole string literal, including the quotes.
    pub fn range(&self) -> TextRange {
        self.range
    }

    /// Range of the raw text between the quotes.
    pub fn content_range(&self) -> TextRange {
        TextRange::new(
            self.range.start() + TextSize::new(1),
            self.range.end() - TextSize::new(1),
        )
    }

    /// True when the raw text contains at least one escape sequence.
    pub fn has_escapes(&self) -> bool {
        matches!(self.value, Cow::Owned(_))
    }

    /// Maps a byte offset in the decoded value to the byte offset in the file.
    ///
    /// Offsets inside the bytes of a decoded character that came from an
    /// escape map to the start of that escape. Offsets past the end clamp to
    /// the closing quote. For many lookups on one string build an
    /// [`OffsetMap`] once with [`JsonString::offset_map`].
    pub fn raw_offset(&self, decoded: TextSize) -> TextSize {
        if !self.has_escapes() {
            let clamped = decoded.min(TextSize::of(self.raw));
            return self.content_range().start() + clamped;
        }
        self.offset_map().raw_offset(decoded)
    }

    /// Maps a range in the decoded value to the range of raw file bytes that
    /// produce it.
    pub fn raw_range(&self, decoded: TextRange) -> TextRange {
        if !self.has_escapes() {
            return TextRange::new(
                self.raw_offset(decoded.start()),
                self.raw_offset(decoded.end()),
            );
        }
        self.offset_map().raw_range(decoded)
    }

    /// Builds the decoded-to-raw offset map for this string.
    pub fn offset_map(&self) -> OffsetMap {
        OffsetMap::build(self.raw, self.content_range().start())
    }
}

/// A JSON array.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Array<'a> {
    pub elements: Vec<Value<'a>>,
}

impl<'a> Array<'a> {
    pub fn len(&self) -> usize {
        self.elements.len()
    }

    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&Value<'a>> {
        self.elements.get(index)
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Value<'a>> {
        self.elements.iter()
    }
}

impl<'a> Array<'a> {
    /// Moves the elements out when one of them has children.
    fn take_nested(&mut self, pending: &mut Vec<Value<'a>>) {
        if self.elements.iter().any(has_children) {
            pending.append(&mut self.elements);
        }
    }
}

/// Arrays and objects are freed without recursion, so that a tree of any
/// depth can be dropped.
impl Drop for Array<'_> {
    fn drop(&mut self) {
        let mut pending = Vec::new();
        self.take_nested(&mut pending);
        drop_all(pending);
    }
}

impl<'s, 'a> IntoIterator for &'s Array<'a> {
    type Item = &'s Value<'a>;
    type IntoIter = std::slice::Iter<'s, Value<'a>>;
    fn into_iter(self) -> Self::IntoIter {
        self.elements.iter()
    }
}

/// A JSON object. Members keep source order; duplicate keys are kept.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Object<'a> {
    pub members: Vec<Member<'a>>,
}

/// One `"key": value` pair of an object.
#[derive(Clone, Debug, PartialEq)]
pub struct Member<'a> {
    pub key: JsonString<'a>,
    pub value: Value<'a>,
}

impl Ranged for Member<'_> {
    /// From the opening quote of the key to the end of the value.
    fn range(&self) -> TextRange {
        TextRange::new(self.key.range().start(), self.value.range.end())
    }
}

impl Member<'_> {
    /// The decoded key.
    pub fn key(&self) -> &str {
        self.key.value()
    }
}

impl<'a> Object<'a> {
    pub fn len(&self) -> usize {
        self.members.len()
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// Value of the last member with this key.
    pub fn get(&self, key: &str) -> Option<&Value<'a>> {
        self.member(key).map(|m| &m.value)
    }

    /// The last member with this key.
    pub fn member(&self, key: &str) -> Option<&Member<'a>> {
        self.members.iter().rev().find(|m| m.key.value() == key)
    }

    /// Index of the last member with this key.
    pub fn position(&self, key: &str) -> Option<usize> {
        self.members.iter().rposition(|m| m.key.value() == key)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.member(key).is_some()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Member<'a>> {
        self.members.iter()
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.members.iter().map(|m| m.key.value())
    }

    /// Groups of member indexes that share a key, for every key that occurs
    /// more than once. Groups are ordered by first occurrence.
    pub fn duplicate_keys(&self) -> Vec<Vec<usize>> {
        let mut groups: Vec<Vec<usize>> = Vec::new();
        if self.members.len() < 2 {
            return groups;
        }
        let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for (i, m) in self.members.iter().enumerate() {
            match seen.get(m.key.value()) {
                Some(&g) => groups[g].push(i),
                None => {
                    seen.insert(m.key.value(), groups.len());
                    groups.push(vec![i]);
                }
            }
        }
        groups.retain(|g| g.len() > 1);
        groups
    }

    /// True when keys are in ascending order by UTF-16 code units, which is
    /// how Ignition's serializer orders them.
    pub fn is_sorted_like_ignition(&self) -> bool {
        self.members.windows(2).all(|w| {
            compare_utf16(w[0].key.value(), w[1].key.value()) != std::cmp::Ordering::Greater
        })
    }
}

impl<'a> Object<'a> {
    /// Moves the member values out when one of them has children.
    fn take_nested(&mut self, pending: &mut Vec<Value<'a>>) {
        if self.members.iter().any(|m| has_children(&m.value)) {
            pending.extend(self.members.drain(..).map(|m| m.value));
        }
    }
}

impl Drop for Object<'_> {
    fn drop(&mut self) {
        let mut pending = Vec::new();
        self.take_nested(&mut pending);
        drop_all(pending);
    }
}

impl<'s, 'a> IntoIterator for &'s Object<'a> {
    type Item = &'s Member<'a>;
    type IntoIter = std::slice::Iter<'s, Member<'a>>;
    fn into_iter(self) -> Self::IntoIter {
        self.members.iter()
    }
}

/// Compares two strings by UTF-16 code units, like Java's `String.compareTo`.
pub fn compare_utf16(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}
