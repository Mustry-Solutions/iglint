//! Minimal, formatting-preserving edits for autofixes.
//!
//! Every builder returns [`TextEdit`]s against the original source. Text that
//! the edit does not need to touch is left byte-for-byte as it was. Layout
//! rules follow Ignition's writer (Gson pretty printing): one member or
//! element per line, indentation in steps of two spaces, `": "` between key
//! and value, empty containers written as `{}` and `[]`.

use std::fmt;

use iglint_text::{Ranged, TextEdit, TextRange, TextSize};

use crate::string::{EscapeStyle, detect_escape_style, escape_into, write_json_string};
use crate::value::{JsonString, Member, Value, ValueKind, compare_utf16};

/// Why an edit could not be built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditError {
    NotAnObject,
    NotAnArray,
    IndexOutOfBounds { index: usize, len: usize },
    KeyNotFound(String),
    KeyExists(String),
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EditError::NotAnObject => f.write_str("value is not an object"),
            EditError::NotAnArray => f.write_str("value is not an array"),
            EditError::IndexOutOfBounds { index, len } => {
                write!(f, "index {index} out of bounds for length {len}")
            }
            EditError::KeyNotFound(k) => write!(f, "key `{k}` not found"),
            EditError::KeyExists(k) => write!(f, "key `{k}` already exists"),
        }
    }
}

impl std::error::Error for EditError {}

/// Where [`JsonEditor::insert_member`] puts a new member.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InsertPosition {
    /// Before the member at this index (`len` appends).
    Index(usize),
    /// After the last member.
    End,
    /// At its sorted position when the existing keys are sorted (as Ignition
    /// writes them), otherwise at the end.
    Sorted,
}

/// Builds edits for one source text.
#[derive(Clone, Debug)]
pub struct JsonEditor<'s> {
    source: &'s str,
    indent_unit: String,
    key_separator: String,
    escape_style: EscapeStyle,
    multiline: bool,
    newline: &'static str,
}

impl<'s> JsonEditor<'s> {
    /// An editor for `source` whose parsed root is `root`. Indentation unit,
    /// key separator and escape style are detected from the document.
    pub fn new(source: &'s str, root: &Value<'_>) -> Self {
        let mut editor = JsonEditor {
            source,
            indent_unit: "  ".to_owned(),
            key_separator: ": ".to_owned(),
            escape_style: detect_escape_style(root),
            multiline: source.contains('\n'),
            newline: if source.contains("\r\n") {
                "\r\n"
            } else {
                "\n"
            },
        };
        if let Some(unit) = editor.detect_indent_unit(root) {
            editor.indent_unit = unit;
        }
        if let Some(sep) = editor.detect_key_separator(root) {
            editor.key_separator = sep;
        }
        editor
    }

    /// Overrides the escape style used for new strings and keys.
    pub fn with_escape_style(mut self, style: EscapeStyle) -> Self {
        self.escape_style = style;
        self
    }

    /// Overrides the indentation unit used for new nested lines.
    pub fn with_indent_unit(mut self, unit: impl Into<String>) -> Self {
        self.indent_unit = unit.into();
        self
    }

    pub fn source(&self) -> &'s str {
        self.source
    }

    pub fn indent_unit(&self) -> &str {
        &self.indent_unit
    }

    pub fn key_separator(&self) -> &str {
        &self.key_separator
    }

    pub fn escape_style(&self) -> EscapeStyle {
        self.escape_style
    }

    /// Writes `value` as a JSON string literal in the document's escape style.
    pub fn quote(&self, value: &str) -> String {
        let mut out = String::with_capacity(value.len() + 2);
        write_json_string(&mut out, value, self.escape_style);
        out
    }

    // ----- removal -----

    /// Removes the member at `index`, with its comma and, when it sits on
    /// lines of its own, those whole lines.
    pub fn remove_member(&self, object: &Value<'_>, index: usize) -> Result<TextEdit, EditError> {
        let mut edits = self.remove_members(object, &[index])?;
        Ok(edits.pop().expect("one removal yields one edit"))
    }

    /// Removes the last member with `key`.
    pub fn remove_member_by_key(
        &self,
        object: &Value<'_>,
        key: &str,
    ) -> Result<TextEdit, EditError> {
        let o = object.as_object().ok_or(EditError::NotAnObject)?;
        let index = o
            .position(key)
            .ok_or_else(|| EditError::KeyNotFound(key.to_owned()))?;
        self.remove_member(object, index)
    }

    /// Removes several members of one object. Indexes may be in any order and
    /// may repeat. The returned edits do not overlap.
    pub fn remove_members(
        &self,
        object: &Value<'_>,
        indices: &[usize],
    ) -> Result<Vec<TextEdit>, EditError> {
        let o = object.as_object().ok_or(EditError::NotAnObject)?;
        let items: Vec<TextRange> = o.members.iter().map(Member::range).collect();
        self.remove_items(object.range, &items, indices)
    }

    /// Removes the element at `index` of an array.
    pub fn remove_element(&self, array: &Value<'_>, index: usize) -> Result<TextEdit, EditError> {
        let mut edits = self.remove_elements(array, &[index])?;
        Ok(edits.pop().expect("one removal yields one edit"))
    }

    /// Removes several elements of one array.
    pub fn remove_elements(
        &self,
        array: &Value<'_>,
        indices: &[usize],
    ) -> Result<Vec<TextEdit>, EditError> {
        let a = array.as_array().ok_or(EditError::NotAnArray)?;
        let items: Vec<TextRange> = a.elements.iter().map(|e| e.range).collect();
        self.remove_items(array.range, &items, indices)
    }

    fn remove_items(
        &self,
        container: TextRange,
        items: &[TextRange],
        indices: &[usize],
    ) -> Result<Vec<TextEdit>, EditError> {
        let n = items.len();
        let mut remove = vec![false; n];
        for &i in indices {
            if i >= n {
                return Err(EditError::IndexOutOfBounds { index: i, len: n });
            }
            remove[i] = true;
        }
        if remove.iter().all(|&r| r) {
            if n == 0 {
                return Ok(Vec::new());
            }
            // Empty container: `{}` / `[]`, as Ignition writes it.
            let inner = TextRange::new(
                container.start() + TextSize::new(1),
                container.end() - TextSize::new(1),
            );
            return Ok(vec![TextEdit::delete(inner)]);
        }
        let mut edits = Vec::new();
        let mut i = 0;
        while i < n {
            if !remove[i] {
                i += 1;
                continue;
            }
            let first = i;
            while i < n && remove[i] {
                i += 1;
            }
            let last = i - 1;
            if last + 1 < n {
                // A kept item follows: delete up to its start.
                let next = items[last + 1].start();
                let start = items[first].start();
                let range = match (self.own_line_start(start), self.own_line_start(next)) {
                    (Some(a), Some(b)) => TextRange::new(a, b),
                    _ => TextRange::new(start, next),
                };
                edits.push(TextEdit::delete(range));
            } else {
                // Run at the end: delete from the end of the previous kept
                // item, which takes its comma.
                let prev = items[first - 1].end();
                edits.push(TextEdit::delete(TextRange::new(prev, items[last].end())));
            }
        }
        Ok(edits)
    }

    // ----- replacement -----

    /// Replaces a value with `json`, raw JSON text written as if it started
    /// at column 0. Continuation lines are indented to the value's line.
    pub fn replace_value(&self, value: &Value<'_>, json: &str) -> TextEdit {
        let indent = self.line_indent(value.range.start());
        TextEdit::replace(value.range, self.reindent(json, indent))
    }

    /// Replaces a value with a JSON string holding `text`.
    pub fn replace_with_string(&self, value: &Value<'_>, text: &str) -> TextEdit {
        TextEdit::replace(value.range, self.quote(text))
    }

    /// Replaces a range of a string's decoded value with `text`, touching only
    /// the raw bytes of that range.
    pub fn replace_in_string(
        &self,
        string: &JsonString<'_>,
        decoded: TextRange,
        text: &str,
    ) -> TextEdit {
        let raw = string.raw_range(decoded);
        let mut out = String::with_capacity(text.len());
        escape_into(&mut out, text, self.escape_style);
        TextEdit::replace(raw, out)
    }

    /// Renames a member's key, keeping its value and position.
    pub fn rename_key(&self, member: &Member<'_>, new_key: &str) -> TextEdit {
        TextEdit::replace(member.key.range(), self.quote(new_key))
    }

    // ----- insertion -----

    /// Inserts `"key": json` into an object. `json` is raw JSON text written
    /// as if it started at column 0.
    pub fn insert_member(
        &self,
        object: &Value<'_>,
        position: InsertPosition,
        key: &str,
        json: &str,
    ) -> Result<TextEdit, EditError> {
        let o = object.as_object().ok_or(EditError::NotAnObject)?;
        if o.contains_key(key) {
            return Err(EditError::KeyExists(key.to_owned()));
        }
        let n = o.members.len();
        let index = match position {
            InsertPosition::Index(i) if i > n => {
                return Err(EditError::IndexOutOfBounds { index: i, len: n });
            }
            InsertPosition::Index(i) => i,
            InsertPosition::End => n,
            InsertPosition::Sorted => {
                if o.is_sorted_like_ignition() {
                    o.members
                        .iter()
                        .position(|m| {
                            compare_utf16(m.key.value(), key) == std::cmp::Ordering::Greater
                        })
                        .unwrap_or(n)
                } else {
                    n
                }
            }
        };
        let separator = o
            .members
            .first()
            .map(|m| &self.source[m.key.range().end().to_usize()..m.value.range.start().to_usize()])
            .unwrap_or(&self.key_separator);
        let items: Vec<TextRange> = o.members.iter().map(Member::range).collect();
        let quoted = self.quote(key);
        Ok(self.insert_item(object.range, &items, index, |indent| {
            let mut s = String::with_capacity(quoted.len() + separator.len() + json.len());
            s.push_str(&quoted);
            s.push_str(separator);
            s.push_str(&self.reindent(json, indent));
            s
        }))
    }

    /// Inserts an element into an array before `index` (`len` appends).
    pub fn insert_element(
        &self,
        array: &Value<'_>,
        index: usize,
        json: &str,
    ) -> Result<TextEdit, EditError> {
        let a = array.as_array().ok_or(EditError::NotAnArray)?;
        let n = a.elements.len();
        if index > n {
            return Err(EditError::IndexOutOfBounds { index, len: n });
        }
        let items: Vec<TextRange> = a.elements.iter().map(|e| e.range).collect();
        Ok(self.insert_item(array.range, &items, index, |indent| {
            self.reindent(json, indent)
        }))
    }

    fn insert_item(
        &self,
        container: TextRange,
        items: &[TextRange],
        index: usize,
        render: impl Fn(&str) -> String,
    ) -> TextEdit {
        let n = items.len();
        if n == 0 {
            let inner = TextRange::new(
                container.start() + TextSize::new(1),
                container.end() - TextSize::new(1),
            );
            if !self.multiline {
                return TextEdit::replace(inner, render(""));
            }
            let outer = self.line_indent(container.start());
            let indent = format!("{outer}{}", self.indent_unit);
            let nl = self.newline;
            return TextEdit::replace(inner, format!("{nl}{indent}{}{nl}{outer}", render(&indent)));
        }
        let multiline = items
            .iter()
            .any(|r| self.own_line_start(r.start()).is_some());
        let neighbour = items[index.min(n - 1)];
        let indent = self.line_indent(neighbour.start());
        if index < n {
            let at = items[index].start();
            let text = if multiline && self.own_line_start(at).is_some() {
                format!("{},{}{indent}", render(indent), self.newline)
            } else {
                format!("{}{}", render(indent), self.item_separator(items))
            };
            TextEdit::insert(at, text)
        } else {
            let at = items[n - 1].end();
            let text = if multiline && self.own_line_start(items[n - 1].start()).is_some() {
                format!(",{}{indent}{}", self.newline, render(indent))
            } else {
                format!("{}{}", self.item_separator(items), render(indent))
            };
            TextEdit::insert(at, text)
        }
    }

    /// `", "` or `","`, as used between the first two items, for inline
    /// containers.
    fn item_separator(&self, items: &[TextRange]) -> &str {
        if items.len() >= 2 {
            let between = &self.source[items[0].end().to_usize()..items[1].start().to_usize()];
            if !between.contains('\n') {
                return between;
            }
        }
        ", "
    }

    // ----- layout helpers -----

    /// [`reindent`], also converting `\n` to the document's line ending.
    fn reindent(&self, json: &str, indent: &str) -> String {
        let text = reindent(json, indent);
        if self.newline == "\r\n" && text.contains('\n') {
            text.replace("\r\n", "\n").replace('\n', "\r\n")
        } else {
            text
        }
    }

    /// When only spaces or tabs precede `offset` on its line, the offset of
    /// the line start.
    fn own_line_start(&self, offset: TextSize) -> Option<TextSize> {
        let before = &self.source.as_bytes()[..offset.to_usize()];
        let mut i = before.len();
        while i > 0 && matches!(before[i - 1], b' ' | b'\t') {
            i -= 1;
        }
        if i == 0 || before[i - 1] == b'\n' || before[i - 1] == b'\r' {
            Some(TextSize::from(i))
        } else {
            None
        }
    }

    /// Leading whitespace of the line containing `offset`.
    fn line_indent(&self, offset: TextSize) -> &'s str {
        let bytes = self.source.as_bytes();
        let mut start = offset.to_usize().min(bytes.len());
        while start > 0 && bytes[start - 1] != b'\n' && bytes[start - 1] != b'\r' {
            start -= 1;
        }
        let mut end = start;
        while end < bytes.len() && matches!(bytes[end], b' ' | b'\t') {
            end += 1;
        }
        &self.source[start..end]
    }

    fn detect_indent_unit(&self, root: &Value<'_>) -> Option<String> {
        let mut found = None;
        root.walk(|_, v| {
            if found.is_some() {
                return false;
            }
            let first_child = match &v.kind {
                ValueKind::Object(o) => o.members.first().map(|m| m.key.range().start()),
                ValueKind::Array(a) => a.elements.first().map(|e| e.range.start()),
                _ => None,
            };
            if let Some(child) = first_child
                && self.own_line_start(child).is_some()
            {
                let outer = self.line_indent(v.range.start());
                let inner = self.line_indent(child);
                if let Some(unit) = inner.strip_prefix(outer)
                    && !unit.is_empty()
                {
                    found = Some(unit.to_owned());
                }
            }
            true
        });
        found
    }

    fn detect_key_separator(&self, root: &Value<'_>) -> Option<String> {
        let mut found = None;
        root.walk(|_, v| {
            if found.is_some() {
                return false;
            }
            if let ValueKind::Object(o) = &v.kind
                && let Some(m) = o.members.first()
            {
                found = Some(
                    self.source[m.key.range().end().to_usize()..m.value.range.start().to_usize()]
                        .to_owned(),
                );
            }
            true
        });
        found
    }
}

/// Indents every line of `json` after the first with `indent`.
pub fn reindent(json: &str, indent: &str) -> String {
    if indent.is_empty() || !json.contains('\n') {
        return json.to_owned();
    }
    let mut out = String::with_capacity(json.len() + indent.len() * 8);
    for (i, line) in json.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
            if !line.is_empty() {
                out.push_str(indent);
            }
        }
        out.push_str(line);
    }
    out
}
