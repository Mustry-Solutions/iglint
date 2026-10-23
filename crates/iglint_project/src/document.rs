//! Owned, parsed JSON documents.

use std::sync::Arc;

use iglint_json::{ParseError, ParseOptions, Value};

/// The deepest nesting of arrays and objects that iglint analyses. A
/// deeper file is a [`iglint_json::ParseErrorKind::NestingTooDeep`] error.
///
/// Gson parses any depth, but Perspective cannot use such a view: its view
/// loader builds components recursively and its property trees copy values
/// recursively, and both overflow a gateway thread's 1 MiB stack at 1,200
/// to 3,300 levels (measured on 8.3.3, see
/// `docs/internal/ignition-format.md`). The limit is far above that, so
/// that only views Ignition cannot open are reported, and it keeps the
/// recursive consumers of the tree within their stacks.
pub const MAX_JSON_DEPTH: usize = 8_192;

type JsonResult<'a> = Result<Value<'a>, ParseError>;

self_cell::self_cell!(
    struct JsonCell {
        owner: Arc<str>,
        #[covariant]
        dependent: JsonResult,
    }
    impl {Debug}
);

/// A JSON file's text together with its span-preserving parse tree.
///
/// The tree borrows from the text; this type owns both, so documents can be
/// stored in the workspace, sent between threads and kept for the whole run.
/// Each file is parsed exactly once.
#[derive(Debug)]
pub struct JsonDocument(JsonCell);

impl JsonDocument {
    /// Parses `text`. Parse errors are kept, not returned: an invalid file is
    /// still a document of the workspace. Nesting deeper than
    /// [`MAX_JSON_DEPTH`] is an error.
    pub fn parse(text: Arc<str>) -> Self {
        let options = ParseOptions {
            max_depth: MAX_JSON_DEPTH,
            ..ParseOptions::default()
        };
        JsonDocument(JsonCell::new(text, |t| iglint_json::parse_with(t, options)))
    }

    /// The file text.
    pub fn text(&self) -> &str {
        self.0.borrow_owner()
    }

    /// The shared file text.
    pub fn shared_text(&self) -> &Arc<str> {
        self.0.borrow_owner()
    }

    /// The parse tree, or the first syntax error.
    pub fn value(&self) -> Result<&Value<'_>, &ParseError> {
        self.0.borrow_dependent().as_ref()
    }

    /// The parse tree, if the file is valid JSON.
    pub fn root(&self) -> Option<&Value<'_>> {
        self.value().ok()
    }

    /// The syntax error, if any.
    pub fn error(&self) -> Option<&ParseError> {
        self.value().err()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn document_is_send_and_sync() {
        assert_send_sync::<JsonDocument>();
    }

    #[test]
    fn keeps_errors() {
        let doc = JsonDocument::parse(Arc::from("{\"a\": }"));
        assert!(doc.error().is_some());
        let doc = JsonDocument::parse(Arc::from("{\"a\": 1}"));
        assert_eq!(doc.root().unwrap().get("a").unwrap().as_i64(), Some(1));
    }

    #[test]
    fn depth_limit() {
        let nested = |n: usize| Arc::from("[".repeat(n) + &"]".repeat(n));
        assert!(JsonDocument::parse(nested(MAX_JSON_DEPTH)).root().is_some());
        let doc = JsonDocument::parse(nested(MAX_JSON_DEPTH + 1));
        let error = doc.error().unwrap();
        assert_eq!(error.kind, iglint_json::ParseErrorKind::NestingTooDeep);
        assert_eq!(error.range.start().to_usize(), MAX_JSON_DEPTH);
    }
}
