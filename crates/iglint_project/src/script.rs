//! Embedded scripts: every Python location with the source Ignition
//! compiles and a map back to the file.
//!
//! iglint does not parse Python here. Each script location becomes an
//! [`EmbeddedScript`] whose [`EmbeddedScript::source`] is a complete module
//! (the generated `def` header and the body) and whose
//! [`EmbeddedScript::source_map`] maps any offset of that source to bytes in
//! the file. The Python layer parses `source` and maps its diagnostics back.

use std::borrow::Cow;

use iglint_json::JsonString;
use iglint_text::{TextRange, TextSize};

use crate::ids::{ComponentId, FileId, TagId};
use crate::kinds::{EventScriptKind, RunScope};
use crate::source_map::SourceMap;

/// Names every Ignition script sees as globals without importing them, in
/// addition to the generated function's own name and the project's top-level
/// library packages (format spec 3.2).
pub const IMPLICIT_GLOBALS: &[&str] = &["__builtins__", "system", "fpmi", "sys"];

/// Who owns an embedded script or expression.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Owner {
    /// The view (view-level `params`, `custom`, `propConfig`, `events`).
    View,
    /// A component of the view.
    Component(ComponentId),
    /// Session properties (`session-props/props.json`).
    Session,
    /// A tag or UDT member.
    Tag(TagId),
    /// The resource itself (event script files, library modules).
    Resource,
}

/// A tag event script id (`eventScripts[].eventid`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TagEventKind {
    ValueChanged,
    QualityChanged,
    QualifiedValueChanged,
    AlarmActive,
    AlarmCleared,
    AlarmAcked,
}

impl TagEventKind {
    /// Every tag event.
    pub const ALL: [TagEventKind; 6] = [
        TagEventKind::ValueChanged,
        TagEventKind::QualityChanged,
        TagEventKind::QualifiedValueChanged,
        TagEventKind::AlarmActive,
        TagEventKind::AlarmCleared,
        TagEventKind::AlarmAcked,
    ];

    /// The `eventid` as written, which is also the function name.
    pub fn event_id(self) -> &'static str {
        match self {
            TagEventKind::ValueChanged => "valueChanged",
            TagEventKind::QualityChanged => "qualityChanged",
            TagEventKind::QualifiedValueChanged => "qualifiedValueChanged",
            TagEventKind::AlarmActive => "alarmActive",
            TagEventKind::AlarmCleared => "alarmCleared",
            TagEventKind::AlarmAcked => "alarmAcked",
        }
    }

    /// The event for an `eventid`.
    pub fn from_event_id(id: &str) -> Option<TagEventKind> {
        TagEventKind::ALL.into_iter().find(|k| k.event_id() == id)
    }

    /// The generated parameters (`TagScriptManager.buildSignature`).
    pub fn params(self) -> &'static [&'static str] {
        match self {
            TagEventKind::ValueChanged | TagEventKind::QualityChanged => &[
                "tag",
                "tagPath",
                "previousValue",
                "currentValue",
                "initialChange",
                "missedEvents",
            ],
            TagEventKind::QualifiedValueChanged => &[
                "tag",
                "tagPath",
                "previousValue",
                "currentValue",
                "initialChange",
                "changed",
                "missedEvents",
            ],
            TagEventKind::AlarmActive | TagEventKind::AlarmCleared => &[
                "tag",
                "tagPath",
                "alarmName",
                "alarmEvent",
                "alarmPath",
                "missedEvents",
            ],
            TagEventKind::AlarmAcked => &[
                "tag",
                "tagPath",
                "alarmName",
                "alarmEvent",
                "alarmPath",
                "ackedBy",
                "missedEvents",
            ],
        }
    }

    /// The exact header, without spaces after commas.
    pub fn header(self) -> String {
        format!("def {}({}):", self.event_id(), self.params().join(","))
    }
}

/// What kind of script location this is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScriptKind {
    /// Event script action (`events.*.*` with `type: "script"`):
    /// `def runAction(self, event):`.
    EventAction,
    /// Script transform: `def transform(self, value, quality, timestamp):`.
    Transform,
    /// Property change script (`propConfig.<path>.onChange`):
    /// `def valueChanged(self, previousValue, currentValue, origin, missedEvents):`.
    PropertyChange,
    /// Component message handler: `def onMessageReceived(self, payload):`.
    MessageHandler,
    /// Component custom method: `def name(self,a,b):`.
    CustomMethod,
    /// Component extension function. `declared` is `Some(false)` when the
    /// component type is known and does not declare a function of that
    /// name, `None` when the component type is unknown.
    ExtensionFunction { declared: Option<bool> },
    /// Tag event script.
    TagEvent(TagEventKind),
    /// A `.py` event file (gateway, session, WebDev, Vision client).
    EventFile(EventScriptKind),
    /// A project library module (`code.py`).
    LibraryModule,
}

impl ScriptKind {
    /// Short stable name for dumps.
    pub fn name(self) -> &'static str {
        match self {
            ScriptKind::EventAction => "event-action",
            ScriptKind::Transform => "transform",
            ScriptKind::PropertyChange => "property-change",
            ScriptKind::MessageHandler => "message-handler",
            ScriptKind::CustomMethod => "custom-method",
            ScriptKind::ExtensionFunction { .. } => "extension-function",
            ScriptKind::TagEvent(_) => "tag-event",
            ScriptKind::EventFile(_) => "event-file",
            ScriptKind::LibraryModule => "library-module",
        }
    }
}

/// Where the body of a script comes from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScriptBody<'a> {
    /// The decoded value of a JSON string.
    Json(&'a JsonString<'a>),
    /// A slice of a text file starting at `start`.
    File { text: &'a str, start: TextSize },
}

/// One Python script location, wrapped the way Ignition compiles it.
#[derive(Clone, Debug, PartialEq)]
pub struct EmbeddedScript<'a> {
    pub kind: ScriptKind,
    /// The generated function's name (`runAction`, `transform`, the custom
    /// method name, ...). Empty for library modules.
    pub function_name: Cow<'a, str>,
    /// The generated function's parameters, in order.
    pub params: Vec<Cow<'a, str>>,
    /// The generated header line without newline, e.g.
    /// `def runAction(self, event):`. Empty when the source is a file as
    /// written.
    pub header: String,
    /// The complete source to parse: `header + "\n" + body`, or the file
    /// itself.
    pub source: Cow<'a, str>,
    /// Maps offsets of `source` to the file.
    pub source_map: SourceMap,
    pub body: ScriptBody<'a>,
    pub owner: Owner,
    /// iglint node path of the script's location.
    pub node_path: String,
    /// ign-lint node path of the script, for locations ign-lint knows.
    pub ign_lint_path: Option<String>,
    /// False for disabled change scripts, extension functions, tag events
    /// and event resources with `enabled: false`.
    pub enabled: bool,
    pub run_scope: RunScope,
}

impl<'a> EmbeddedScript<'a> {
    /// The body part of `source` (after the header line).
    pub fn body_text(&self) -> &str {
        &self.source[self.source_map.body_start().to_usize()..]
    }

    /// True when the body has nothing but whitespace (Ignition fails to
    /// compile such a script).
    pub fn is_blank(&self) -> bool {
        self.body_text().trim().is_empty()
    }

    /// Maps a range of `source` to the file.
    pub fn file_range(&self, range: TextRange) -> TextRange {
        self.source_map.file_range(range)
    }

    /// The file range that stands for the whole script (the key of the
    /// script property, or the header line of a file).
    pub fn anchor(&self) -> TextRange {
        self.source_map.anchor()
    }

    /// Names the script sees as globals without importing them, besides the
    /// project's top-level library packages: [`IMPLICIT_GLOBALS`] and the
    /// generated function's own name.
    pub fn implicit_globals(&self) -> impl Iterator<Item = &str> {
        IMPLICIT_GLOBALS
            .iter()
            .copied()
            .chain((!self.function_name.is_empty()).then_some(self.function_name.as_ref()))
    }

    /// Whether the script runs with `__name__` in its globals: a project
    /// library module does (and an import there binds `__package__`), the
    /// generated function of an event script does not.
    pub fn has_module_name(&self) -> bool {
        self.kind == ScriptKind::LibraryModule
    }

    /// Builds a script whose body is a JSON string, compiled under `header`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_json(
        kind: ScriptKind,
        function_name: Cow<'a, str>,
        params: Vec<Cow<'a, str>>,
        header: String,
        body: &'a JsonString<'a>,
        anchor: TextRange,
        owner: Owner,
        node_path: String,
        ign_lint_path: Option<String>,
        enabled: bool,
        run_scope: RunScope,
    ) -> Self {
        let mut source = String::with_capacity(header.len() + 1 + body.value().len());
        source.push_str(&header);
        source.push('\n');
        source.push_str(body.value());
        let source_map = SourceMap::json(
            TextSize::of(header.as_str()) + TextSize::new(1),
            anchor,
            body,
        );
        EmbeddedScript {
            kind,
            function_name,
            params,
            header,
            source: Cow::Owned(source),
            source_map,
            body: ScriptBody::Json(body),
            owner,
            node_path,
            ign_lint_path,
            enabled,
            run_scope,
        }
    }
}

/// The standard Perspective header for a fixed script kind.
pub(crate) fn perspective_header(kind: ScriptKind) -> (&'static str, &'static [&'static str]) {
    match kind {
        ScriptKind::EventAction => ("runAction", &["self", "event"]),
        ScriptKind::Transform => ("transform", &["self", "value", "quality", "timestamp"]),
        ScriptKind::PropertyChange => (
            "valueChanged",
            &[
                "self",
                "previousValue",
                "currentValue",
                "origin",
                "missedEvents",
            ],
        ),
        ScriptKind::MessageHandler => ("onMessageReceived", &["self", "payload"]),
        _ => ("", &[]),
    }
}

/// `def name(p1, p2):`.
pub(crate) fn header_line(name: &str, params: &[&str], separator: &str) -> String {
    format!("def {name}({}):", params.join(separator))
}

/// How line 1 of a `.py` event file relates to the canonical header.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HeaderStatus {
    /// Line 1 is exactly the canonical header (a trailing `\r` is ignored).
    Canonical,
    /// Line 1 starts with `def <name>(` but differs from the canonical
    /// header. Ignition drops it and compiles the body under the canonical
    /// header, so parameters written here have no effect.
    Replaced,
    /// Line 1 does not start with `def <name>(`. Ignition treats the whole
    /// file as the body (legacy, header-less style).
    Missing,
}

/// A `.py` file with a full `def` (gateway events, session events, WebDev):
/// the file as written and the script Ignition actually runs.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptFile<'a> {
    pub file: FileId,
    pub kind: EventScriptKind,
    /// The file as written, parsed as a module (what the user edits).
    pub written: EmbeddedScript<'a>,
    /// What runs: the canonical header followed by the file without its
    /// first line (or the whole file when line 1 is not a matching `def`).
    pub runtime: EmbeddedScript<'a>,
    pub header: HeaderStatus,
    /// Range of line 1 without its line break.
    pub first_line: TextRange,
}

impl<'a> ScriptFile<'a> {
    /// Models `text` the way `ExtensionFunctionDescriptor.extractUserScript`
    /// and `buildMethod` handle it.
    pub(crate) fn new(
        file: FileId,
        kind: EventScriptKind,
        text: &'a str,
        node_path: String,
        enabled: bool,
    ) -> Self {
        let function = kind.function_name();
        let canonical = kind.canonical_header();
        let line_end = text.find('\n');
        let first_line_text = &text[..line_end.unwrap_or(text.len())];
        let first_line_trimmed = first_line_text
            .strip_suffix('\r')
            .unwrap_or(first_line_text);
        let first_line = TextRange::at(TextSize::ZERO, TextSize::of(first_line_trimmed));
        let drops = first_line_text
            .strip_prefix("def ")
            .and_then(|r| r.strip_prefix(function))
            .is_some_and(|r| r.starts_with('('));
        let header = if !drops {
            HeaderStatus::Missing
        } else if first_line_trimmed == canonical {
            HeaderStatus::Canonical
        } else {
            HeaderStatus::Replaced
        };
        let params: Vec<Cow<'a, str>> = kind.params().iter().map(|p| Cow::Borrowed(*p)).collect();
        let written = EmbeddedScript {
            kind: ScriptKind::EventFile(kind),
            function_name: Cow::Borrowed(function),
            params: params.clone(),
            header: String::new(),
            source: Cow::Borrowed(text),
            source_map: SourceMap::identity(TextSize::ZERO, TextSize::of(text)),
            body: ScriptBody::File {
                text,
                start: TextSize::ZERO,
            },
            owner: Owner::Resource,
            node_path: node_path.clone(),
            ign_lint_path: None,
            enabled,
            run_scope: kind.run_scope(),
        };
        let body_start = if drops {
            line_end.map_or(text.len(), |e| e + 1)
        } else {
            0
        };
        let body = &text[body_start..];
        let mut source = String::with_capacity(canonical.len() + 1 + body.len());
        source.push_str(&canonical);
        source.push('\n');
        source.push_str(body);
        let anchor = if drops {
            first_line
        } else {
            TextRange::empty(TextSize::ZERO)
        };
        let runtime = EmbeddedScript {
            kind: ScriptKind::EventFile(kind),
            function_name: Cow::Borrowed(function),
            params,
            source_map: SourceMap::file_with_prefix(
                TextSize::of(canonical.as_str()) + TextSize::new(1),
                anchor,
                TextSize::of(&text[..body_start]),
                TextSize::of(body),
            ),
            header: canonical,
            source: Cow::Owned(source),
            body: ScriptBody::File {
                text,
                start: TextSize::of(&text[..body_start]),
            },
            owner: Owner::Resource,
            node_path,
            ign_lint_path: None,
            enabled,
            run_scope: kind.run_scope(),
        };
        ScriptFile {
            file,
            kind,
            written,
            runtime,
            header,
            first_line,
        }
    }
}

/// A Python file as written, modelled as a library-style module script
/// (no generated header). Used for project library modules and for Python
/// files outside any resource.
pub fn library_script<'a>(text: &'a str, node_path: String) -> EmbeddedScript<'a> {
    EmbeddedScript {
        kind: ScriptKind::LibraryModule,
        function_name: Cow::Borrowed(""),
        params: Vec::new(),
        header: String::new(),
        source: Cow::Borrowed(text),
        source_map: SourceMap::identity(TextSize::ZERO, TextSize::of(text)),
        body: ScriptBody::File {
            text,
            start: TextSize::ZERO,
        },
        owner: Owner::Resource,
        node_path,
        ign_lint_path: None,
        enabled: true,
        run_scope: RunScope::Library,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(text: &str) -> ScriptFile<'_> {
        ScriptFile::new(
            FileId(0),
            EventScriptKind::GatewayMessage,
            text,
            "m".into(),
            true,
        )
    }

    #[test]
    fn canonical_header_is_replaced_by_itself() {
        let f = file("def handleMessage(payload):\n\treturn payload");
        assert_eq!(f.header, HeaderStatus::Canonical);
        assert_eq!(
            f.runtime.source,
            "def handleMessage(payload):\n\treturn payload"
        );
        let r = f.runtime.source.find("return").unwrap() as u32;
        let range = f
            .runtime
            .file_range(TextRange::at(TextSize::new(r), TextSize::new(6)));
        assert_eq!(range.slice(f.written.source.as_ref()), "return");
    }

    #[test]
    fn non_canonical_header_is_dropped() {
        let text = "def handleMessage(payload=None, extra=5):\n\tprint extra";
        let f = file(text);
        assert_eq!(f.header, HeaderStatus::Replaced);
        assert_eq!(
            f.runtime.source,
            "def handleMessage(payload):\n\tprint extra"
        );
        // Header maps to the dropped line.
        let r = f
            .runtime
            .file_range(TextRange::at(TextSize::new(0), TextSize::new(3)));
        assert_eq!(r.slice(text), "def handleMessage(payload=None, extra=5):");
    }

    #[test]
    fn one_line_def_loses_body() {
        let f = file("def handleMessage(payload): doWork()");
        assert_eq!(f.header, HeaderStatus::Replaced);
        assert_eq!(f.runtime.source, "def handleMessage(payload):\n");
    }

    #[test]
    fn headerless_file_is_the_body() {
        let f = file("x = 1\n");
        assert_eq!(f.header, HeaderStatus::Missing);
        assert_eq!(f.runtime.source, "def handleMessage(payload):\nx = 1\n");
        assert_eq!(
            f.runtime.source_map.anchor(),
            TextRange::empty(TextSize::ZERO)
        );
    }

    #[test]
    fn crlf_header() {
        let f = file("def handleMessage(payload):\r\n\tpass\r\n");
        assert_eq!(f.header, HeaderStatus::Canonical);
        assert_eq!(f.runtime.source, "def handleMessage(payload):\n\tpass\r\n");
    }

    #[test]
    fn tag_headers() {
        assert_eq!(
            TagEventKind::AlarmAcked.header(),
            "def alarmAcked(tag,tagPath,alarmName,alarmEvent,alarmPath,ackedBy,missedEvents):"
        );
    }
}
