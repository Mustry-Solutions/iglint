//! The safety net of safe fixes on Python code.
//!
//! A safe fix must not change what a script does. Rules decide that per
//! fix; this module re-checks the result centrally, for every script of
//! the fixed file in every storage format (`.py` library modules and event
//! files, and scripts embedded in `view.json` and other resource JSON:
//! event actions, transforms, change scripts, message handlers, custom
//! methods, session and tag event scripts), each compiled with the header
//! Ignition generates for it. A safe fix is dropped when, for any script:
//!
//! * a line moves: the fix changes the number of lines, or the line of any
//!   text it does not edit (so the line on which every remaining statement
//!   starts stays the same, whatever reads line numbers: tracebacks,
//!   `warnings`, `logging`, Java stack traces);
//! * whether the script compiles changes, in either direction (its syntax
//!   and compile errors differ: a fix may neither break a script nor make
//!   a broken one, such as a misplaced `from __future__` import, valid);
//! * a docstring changes (a deletion may promote a later string);
//! * the `from __future__` features differ;
//! * the scopes differ, or a scope's generator status, `global`
//!   declarations or dynamic features (`import *`, `exec`, frame reads);
//! * any remaining name reference resolves differently (local, enclosing
//!   function, global, builtin, injected) or changes whether it may be
//!   unbound. Names an import binds implicitly count too: in a library
//!   module every import that runs binds `__package__`, so removing the
//!   only one changes what a read of `__package__` finds.
//!
//! Scripts are compared in order; a safe fix that adds or removes a script
//! is dropped as well.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use iglint_project::EmbeddedScript;
use iglint_project::script::ScriptBody;
use iglint_python::ast::{Expr, Stmt};
use iglint_python::visitor::{Visitor, walk_stmt};
use iglint_python_semantic::{ImplicitRead, ReferenceKind, SemanticModel, SemanticOptions};
use iglint_text::{TextEdit, TextRange};

/// One script of a file, as Ignition compiles it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptText {
    pub node_path: String,
    /// The source with its generated header.
    pub source: String,
    /// The names the script sees without defining them.
    pub injected: Vec<String>,
    /// The script runs with `__name__` in its globals (a library module),
    /// so its imports bind `__package__`.
    pub module_has_name: bool,
    /// For a script stored in a JSON string: the raw text between the
    /// quotes. `None` for a script that is the file itself.
    pub json_body: Option<TextRange>,
}

impl ScriptText {
    pub fn of(script: &EmbeddedScript<'_>) -> Self {
        ScriptText {
            node_path: script.node_path.clone(),
            source: script.source.to_string(),
            injected: script.implicit_globals().map(str::to_owned).collect(),
            module_has_name: script.has_module_name(),
            json_body: match script.body {
                ScriptBody::Json(s) => Some(s.content_range()),
                ScriptBody::File { .. } => None,
            },
        }
    }
}

/// Tests only: a script containing this text fails the net for every safe
/// fix, so tests can check that every fix path goes through the net.
#[cfg(any(test, feature = "testing"))]
pub const TEST_REJECT_MARKER: &str = "IGLINT_TEST_REJECT_SAFE_FIXES";

/// The scripts of a text of the file being fixed.
pub(crate) type Scripts<'r> = dyn Fn(&str) -> Vec<ScriptText> + 'r;

/// What a script does, as far as the net compares it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScriptFacts {
    errors: Vec<String>,
    docstrings: Vec<String>,
    futures: Vec<String>,
    scopes: Vec<ScopeFacts>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ScopeFacts {
    kind: String,
    name: String,
    generator: bool,
    globals: Vec<String>,
    dynamic: (bool, bool, bool, bool),
    /// Per name read in the scope (run-time spelling, implicit reads apart):
    /// how its references resolve (resolution, where a class body's lookup
    /// continues, never bound, maybe unbound, and the kinds of the bindings
    /// they may read with the kind of scope that holds each).
    refs: BTreeMap<String, BTreeSet<RefFacts>>,
}

impl ScriptFacts {
    pub(crate) fn of(script: &ScriptText) -> ScriptFacts {
        let parsed = iglint_python::parse_module(&script.source);
        let mut errors: Vec<String> = parsed.errors.iter().map(|e| e.message.clone()).collect();
        if let Some(e) = iglint_python_semantic::mark_limit_error(&script.source) {
            errors.push(e.message);
        }
        let mut facts = ScriptFacts {
            errors: Vec::new(),
            docstrings: docstrings(&parsed.module.body),
            futures: futures(&parsed.module.body),
            scopes: Vec::new(),
        };
        if parsed.errors.is_empty() {
            errors.extend(
                iglint_python_semantic::size_errors(&parsed.module, &script.source)
                    .into_iter()
                    .map(|e| e.message),
            );
            let options = SemanticOptions {
                injected: script.injected.iter().map(String::as_str).collect(),
                unknown_globals: true,
                module_has_name: script.module_has_name,
            };
            let model = SemanticModel::build(&parsed.module, &options);
            errors.extend(
                iglint_python_semantic::compile_errors(&model)
                    .into_iter()
                    .map(|e| e.message),
            );
            facts.scopes = scopes(&model);
        }
        errors.sort();
        facts.errors = errors;
        facts
    }
}

fn scopes(model: &SemanticModel<'_>) -> Vec<ScopeFacts> {
    let mut out: Vec<ScopeFacts> = model
        .scopes()
        .map(|(_, s)| {
            let mut globals: Vec<String> = s.globals.keys().map(|g| (*g).to_owned()).collect();
            globals.sort();
            ScopeFacts {
                kind: format!("{:?}", s.kind),
                name: s.name.to_owned(),
                generator: s.is_generator,
                globals,
                dynamic: (
                    s.star_import.is_some(),
                    s.unqualified_exec.is_some(),
                    s.reads_locals,
                    s.writes_globals,
                ),
                refs: BTreeMap::new(),
            }
        })
        .collect();
    for (_, r) in model.references() {
        let mut bindings: Vec<String> = r
            .bindings
            .iter()
            .map(|id| {
                let b = model.binding(*id);
                format!("{:?}:{}", model.scope(b.scope).kind, b.kind.name())
            })
            .collect();
        bindings.sort();
        bindings.dedup();
        let key = match r.kind {
            // The reads an import statement makes to find its package go
            // with the statement (removing an import removes them), and
            // what they find decides nothing the script sees but the
            // `__package__` the statement binds, which is compared as a
            // binding.
            ReferenceKind::Implicit(ImplicitRead::Import) => continue,
            ReferenceKind::Implicit(read) => format!("{} (implicit {read:?})", r.name),
            _ => r.name.to_owned(),
        };
        out[r.scope.index()].refs.entry(key).or_default().insert((
            format!("{:?}", r.resolution),
            format!("{:?}", r.fallback),
            r.unbound,
            r.maybe_unbound,
            bindings,
        ));
    }
    out
}

/// See [`ScopeFacts::refs`].
type RefFacts = (String, String, bool, bool, Vec<String>);

/// The docstrings of the module and of every function and class body.
fn docstrings(body: &[Stmt]) -> Vec<String> {
    fn first_doc(body: &[Stmt]) -> Option<String> {
        match body.first() {
            Some(Stmt::Expr(e)) => match &*e.value {
                Expr::Str(s) => Some(s.s.to_string_lossy().into_owned()),
                _ => None,
            },
            _ => None,
        }
    }
    struct Walk(Vec<String>);
    impl<'a> Visitor<'a> for Walk {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            match stmt {
                Stmt::FunctionDef(f) => self.0.extend(first_doc(&f.body)),
                Stmt::ClassDef(c) => self.0.extend(first_doc(&c.body)),
                _ => {}
            }
            walk_stmt(self, stmt);
        }
    }
    let mut w = Walk(first_doc(body).into_iter().collect());
    for stmt in body {
        w.visit_stmt(stmt);
    }
    w.0.sort();
    w.0
}

/// The `from __future__` imports of the module's own statements, with
/// their position among them.
fn futures(body: &[Stmt]) -> Vec<String> {
    body.iter()
        .enumerate()
        .filter_map(|(i, s)| match s {
            Stmt::ImportFrom(f) if f.module.as_ref().is_some_and(|m| &*m.id == "__future__") => {
                let names: Vec<&str> = f.names.iter().map(|a| a.name.as_str()).collect();
                Some(format!("{i}:{}", names.join(",")))
            }
            _ => None,
        })
        .collect()
}

/// Why a safe fix that turns `before` into `after` with `edits` (ranges in
/// `before`) changes a script, if it does.
pub(crate) fn safe_fix_problem(
    path: &Path,
    before: &str,
    before_scripts: &[ScriptText],
    before_facts: &[ScriptFacts],
    edits: &[TextEdit],
    after_scripts: &[ScriptText],
) -> Option<String> {
    if before_scripts.is_empty() && after_scripts.is_empty() {
        return None;
    }
    #[cfg(any(test, feature = "testing"))]
    if before_scripts
        .iter()
        .any(|s| s.source.contains(TEST_REJECT_MARKER))
    {
        return Some("the test marker rejects safe fixes".to_owned());
    }
    if before_scripts.len() != after_scripts.len() {
        return Some("it would add or remove a script".to_owned());
    }
    if let Some(problem) = line_problem(path, before, before_scripts, edits) {
        return Some(problem);
    }
    for ((b, facts), a) in before_scripts.iter().zip(before_facts).zip(after_scripts) {
        if b.source == a.source {
            continue;
        }
        let after = ScriptFacts::of(a);
        if let Some(problem) = compare(facts, &after) {
            return Some(problem);
        }
    }
    None
}

/// The first difference between the facts of a script before and after a
/// fix.
fn compare(before: &ScriptFacts, after: &ScriptFacts) -> Option<String> {
    if before.errors != after.errors {
        return Some("it would change whether a script compiles".to_owned());
    }
    if before.docstrings != after.docstrings {
        return Some("it would change a docstring".to_owned());
    }
    if before.futures != after.futures {
        return Some("it would change the `__future__` imports".to_owned());
    }
    if before.scopes.len() != after.scopes.len() {
        return Some("it would change the scopes of a script".to_owned());
    }
    for (b, a) in before.scopes.iter().zip(&after.scopes) {
        if (&b.kind, &b.name, b.generator, &b.globals, b.dynamic)
            != (&a.kind, &a.name, a.generator, &a.globals, a.dynamic)
        {
            return Some(format!("it would change scope `{}`", b.name));
        }
        for (name, resolutions) in &a.refs {
            if b.refs.get(name) != Some(resolutions) {
                return Some(format!("it would change what `{name}` refers to"));
            }
        }
    }
    None
}

/// Whether `edits` keep every line of Python source where it is: in a
/// `.py` file the file's lines, in a JSON file the decoded lines of every
/// embedded script an edit touches.
fn line_problem(
    path: &Path,
    before: &str,
    scripts: &[ScriptText],
    edits: &[TextEdit],
) -> Option<String> {
    const MOVED: &str = "it would move lines of a script";
    if path.extension().and_then(|e| e.to_str()) == Some("py") {
        let edits: Vec<(usize, usize, String)> = edits
            .iter()
            .map(|e| {
                (
                    e.range.start().to_usize(),
                    e.range.end().to_usize(),
                    e.content.clone(),
                )
            })
            .collect();
        return (!lines_preserved(before, &edits)).then(|| MOVED.to_owned());
    }
    for script in scripts {
        let Some(body) = script.json_body else {
            continue;
        };
        let (start, end) = (body.start().to_usize(), body.end().to_usize());
        let touching: Vec<&TextEdit> = edits
            .iter()
            .filter(|e| {
                let (s, t) = (e.range.start().to_usize(), e.range.end().to_usize());
                if s == t {
                    // An insertion inside the quotes.
                    (start..=end).contains(&s)
                } else {
                    // Overlaps the string literal, quotes included.
                    s <= end && t >= start
                }
            })
            .collect();
        if touching.is_empty() {
            continue;
        }
        let Some(decoded) = before.get(start..end).and_then(|raw| decode(raw, start)) else {
            return Some(MOVED.to_owned());
        };
        let mut local = Vec::with_capacity(touching.len());
        for e in touching {
            let (s, t) = (e.range.start().to_usize(), e.range.end().to_usize());
            if s < start || t > end {
                // The fix replaces or removes the script itself.
                return Some(MOVED.to_owned());
            }
            let (Some(ds), Some(dt)) = (decoded.index_of(s), decoded.index_of(t)) else {
                return Some(MOVED.to_owned());
            };
            let Some(content) = decode(&e.content, 0) else {
                return Some(MOVED.to_owned());
            };
            local.push((ds, dt, content.text));
        }
        if !lines_preserved(&decoded.text, &local) {
            return Some(MOVED.to_owned());
        }
    }
    None
}

/// Whether replacing `edits` (sorted, non-overlapping byte ranges of
/// `text` and their new text) keeps every unedited character on its line
/// and the number of lines. A line ends at `\n`, `\r\n` or `\r`.
pub(crate) fn lines_preserved(text: &str, edits: &[(usize, usize, String)]) -> bool {
    let mut sorted: Vec<&(usize, usize, String)> = edits.iter().collect();
    sorted.sort_by_key(|e| (e.0, e.1));
    let mut after = String::with_capacity(text.len());
    let mut anchors = Vec::with_capacity(sorted.len() * 2 + 1);
    let mut prev = 0;
    for (s, t, content) in sorted {
        if *s < prev || *t > text.len() || !text.is_char_boundary(*s) || !text.is_char_boundary(*t)
        {
            return false;
        }
        // Both ends were checked to be character boundaries.
        after.push_str(text.get(prev..*s).unwrap_or_default());
        anchors.push((*s, after.len()));
        after.push_str(content);
        prev = *t;
        anchors.push((*t, after.len()));
    }
    after.push_str(text.get(prev..).unwrap_or_default());
    anchors.push((text.len(), after.len()));
    let (b, a) = (break_ends(text), break_ends(&after));
    let line = |ends: &[usize], at: usize| ends.partition_point(|&e| e <= at);
    anchors.into_iter().all(|(x, y)| line(&b, x) == line(&a, y))
}

/// The offsets just after each line break of `text`.
fn break_ends(text: &str) -> Vec<usize> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    for (i, &c) in bytes.iter().enumerate() {
        if c == b'\n' || (c == b'\r' && bytes.get(i + 1) != Some(&b'\n')) {
            out.push(i + 1);
        }
    }
    out
}

/// A decoded JSON string fragment with the file offset of every decoded
/// byte boundary.
struct Decoded {
    text: String,
    /// `(raw offset, decoded offset)` of every character start, and of the
    /// end.
    offsets: Vec<(usize, usize)>,
}

impl Decoded {
    fn index_of(&self, raw: usize) -> Option<usize> {
        self.offsets
            .binary_search_by_key(&raw, |(r, _)| *r)
            .ok()
            .map(|i| self.offsets[i].1)
    }
}

/// Decodes the raw text between the quotes of a JSON string (`base` is its
/// file offset). `None` on an invalid escape.
fn decode(raw: &str, base: usize) -> Option<Decoded> {
    let bytes = raw.as_bytes();
    let mut text = String::with_capacity(raw.len());
    let mut offsets = Vec::with_capacity(raw.len() + 1);
    let mut i = 0;
    while i < bytes.len() {
        offsets.push((base + i, text.len()));
        if bytes[i] != b'\\' {
            let c = raw.get(i..)?.chars().next()?;
            text.push(c);
            i += c.len_utf8();
            continue;
        }
        let e = *bytes.get(i + 1)?;
        i += 2;
        let c = match e {
            b'"' => '"',
            b'\\' => '\\',
            b'/' => '/',
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'u' => {
                let hex = |at: usize| u32::from_str_radix(raw.get(at..at + 4)?, 16).ok();
                let first = hex(i)?;
                i += 4;
                if (0xD800..0xDC00).contains(&first) && raw.get(i..i + 2) == Some("\\u") {
                    let second = hex(i + 2)?;
                    if (0xDC00..0xE000).contains(&second) {
                        i += 6;
                        char::from_u32(0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00))
                            .unwrap_or('\u{fffd}')
                    } else {
                        '\u{fffd}'
                    }
                } else {
                    char::from_u32(first).unwrap_or('\u{fffd}')
                }
            }
            _ => return None,
        };
        text.push(c);
    }
    offsets.push((base + bytes.len(), text.len()));
    Some(Decoded { text, offsets })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(s: usize, t: usize, c: &str) -> (usize, usize, String) {
        (s, t, c.to_owned())
    }

    #[test]
    fn line_preservation() {
        let text = "import os\nimport sys\nx = 1\n";
        // Content removed, line break kept.
        assert!(lines_preserved(text, &[e(0, 9, "")]));
        // The whole line removed.
        assert!(!lines_preserved(text, &[e(0, 10, "")]));
        // A line inserted.
        assert!(!lines_preserved(text, &[e(0, 0, "# c\n")]));
        // One removed, one inserted: the lines between move.
        assert!(!lines_preserved(text, &[e(0, 10, ""), e(20, 20, "\n")]));
        // A multi-line statement replaced by as many line breaks.
        let text = "from a import (b,\n  c)\nx = 1\n";
        assert!(lines_preserved(text, &[e(0, 21, "\n")]));
        // CRLF: `\r\n` is one line break.
        let text = "import os\r\nx = 1\r\n";
        assert!(lines_preserved(text, &[e(0, 9, "")]));
        assert!(!lines_preserved(text, &[e(0, 11, "")]));
        // A lone `\r` is a line break too.
        assert!(!lines_preserved("a\rb\n", &[e(1, 2, "")]));
    }

    #[test]
    fn json_fragments_decode_with_offsets() {
        let d = decode(r"a\n\tbé😀", 10).unwrap();
        assert_eq!(d.text, "a\n\tb\u{e9}\u{1f600}");
        assert_eq!(d.index_of(10), Some(0));
        assert_eq!(d.index_of(11), Some(1));
        assert_eq!(d.index_of(13), Some(2));
        assert_eq!(d.index_of(12), None);
        assert!(decode(r"\x", 0).is_none());
    }

    /// Every check of the net runs for every script storage format:
    /// library modules, `.py` event files (gateway message handler and
    /// timer, Perspective session event, WebDev), the five view script
    /// locations (JSON strings) and tag event scripts (gateway JSON). Each
    /// body is mutated so that exactly one property changes. (Reviewer
    /// repro R6-E2.)
    #[test]
    fn every_check_runs_for_every_script_kind() {
        use crate::rules::script::testing::*;
        use crate::rules::test_support::{Fixture, PERSPECTIVE};
        use std::sync::Arc;

        struct Kind {
            name: &'static str,
            /// The `def` line of an event file, whose body is indented.
            header: &'static str,
            indent: &'static str,
            make: fn(&str) -> Fixture,
            path: &'static str,
        }
        let kinds = [
            Kind {
                name: "library",
                header: "",
                indent: "",
                make: |c| library("m", c),
                path: "projects/p/ignition/script-python/m/code.py",
            },
            Kind {
                name: "gateway message",
                header: "def handleMessage(payload):\n",
                indent: "\t",
                make: gateway_message,
                path: "projects/p/ignition/message/m/handleMessage.py",
            },
            Kind {
                name: "gateway timer",
                header: "def handleTimerEvent():\n",
                indent: "\t",
                make: gateway_timer,
                path: "projects/p/ignition/timer/t/handleTimerEvent.py",
            },
            Kind {
                name: "session event",
                header: "def onStartup(session):\n",
                indent: "\t",
                make: session_event,
                path: "",
            },
            Kind {
                name: "webdev",
                header: "def doGet(request, session):\n",
                indent: "\t",
                make: webdev,
                path: "projects/p/com.inductiveautomation.webdev/resources/api/doGet.py",
            },
            Kind {
                name: "event action",
                header: "",
                indent: "\t",
                make: |c| view(ViewScript::Action, c),
                path: "",
            },
            Kind {
                name: "transform",
                header: "",
                indent: "\t",
                make: |c| view(ViewScript::Transform, c),
                path: "",
            },
            Kind {
                name: "change script",
                header: "",
                indent: "\t",
                make: |c| view(ViewScript::Change, c),
                path: "",
            },
            Kind {
                name: "message handler",
                header: "",
                indent: "\t",
                make: |c| view(ViewScript::Handler, c),
                path: "",
            },
            Kind {
                name: "custom method",
                header: "",
                indent: "\t",
                make: |c| view(ViewScript::Method, c),
                path: "",
            },
            Kind {
                name: "tag event",
                header: "",
                indent: "\t",
                make: tag_event,
                path: "",
            },
        ];
        // (edits as (old snippet, new snippet), expected problem).
        type Mutation<'a> = (&'a [(&'a str, &'a str)], Option<&'a str>);
        let mutations: &[Mutation<'_>] = &[
            (&[("import os", "")], None),
            (&[("import os", "import os as len")], Some("refers to")),
            (&[("\timport os\n", "")], Some("move lines")),
            (&[("import os", ""), ("import sys", "")], Some("docstring")),
            (&[("return len([1])", "return len([1]")], Some("compiles")),
            (&[("return len([1])", "yield len([1])")], Some("scope")),
        ];
        for kind in &kinds {
            let i = kind.indent;
            let header = kind.header;
            let body = format!(
                "{header}{i}def g():\n{i}\timport os\n{i}\timport sys\n{i}\t'doc'\n{i}\treturn len([1])\n{i}print g()\n"
            );
            let f = (kind.make)(&body);
            let path = match kind.path {
                "" if kind.name == "tag event" => walk(f.root())
                    .into_iter()
                    .find(|p| std::fs::read_to_string(p).is_ok_and(|t| t.contains("eventScripts")))
                    .expect("tags file"),
                "" if kind.name == "session event" => {
                    f.path(&format!("projects/p/{PERSPECTIVE}/startup/onStartup.py"))
                }
                "" => view_path(&f),
                p => f.path(p),
            };
            let text = std::fs::read_to_string(&path).unwrap();
            let json = path.extension().is_some_and(|e| e == "json");
            let raw = |snippet: &str| -> String {
                if json {
                    let quoted = crate::rules::script::testing::json(snippet);
                    quoted
                        .strip_prefix('"')
                        .and_then(|q| q.strip_suffix('"'))
                        .unwrap()
                        .to_owned()
                } else {
                    snippet.to_owned()
                }
            };
            let ws = iglint_project::Workspace::discover(&[f.root()]);
            let file = ws.file_by_path(&path).expect("file").id;
            let scripts = |t: &str| {
                crate::analyze::resource_scripts(&ws, &Default::default(), file, Arc::from(t))
            };
            let before = scripts(&text);
            assert_eq!(before.len(), 1, "{}: {before:?}", kind.name);
            let facts: Vec<ScriptFacts> = before.iter().map(ScriptFacts::of).collect();
            for (edits, expected) in mutations {
                let mut text_edits: Vec<TextEdit> = edits
                    .iter()
                    .map(|(old, new)| {
                        let old = raw(&old.replace('\t', &format!("{i}\t")));
                        let at = text
                            .find(&old)
                            .unwrap_or_else(|| panic!("{}: {old}", kind.name));
                        TextEdit::replace(
                            TextRange::new(
                                iglint_text::TextSize::from(at),
                                iglint_text::TextSize::from(at + old.len()),
                            ),
                            raw(new),
                        )
                    })
                    .collect();
                text_edits.sort_by_key(|e| e.range);
                let after = iglint_text::apply_edits(&text, &text_edits).unwrap();
                let problem =
                    safe_fix_problem(&path, &text, &before, &facts, &text_edits, &scripts(&after));
                match expected {
                    None => assert_eq!(problem, None, "{}: {edits:?}", kind.name),
                    Some(want) => assert!(
                        problem.as_deref().is_some_and(|p| p.contains(want)),
                        "{}: {edits:?}: {problem:?}",
                        kind.name
                    ),
                }
            }
        }
        // A misplaced `from __future__` import becomes valid when the
        // statement before it goes (reviewer repro R6-C).
        let f = library("m", "import os\nfrom __future__ import division\nx = 1/2\n");
        let path = f.path("projects/p/ignition/script-python/m/code.py");
        let text = std::fs::read_to_string(&path).unwrap();
        let ws = iglint_project::Workspace::discover(&[f.root()]);
        let file = ws.file_by_path(&path).expect("file").id;
        let scripts = |t: &str| {
            crate::analyze::resource_scripts(&ws, &Default::default(), file, Arc::from(t))
        };
        let before = scripts(&text);
        let facts: Vec<ScriptFacts> = before.iter().map(ScriptFacts::of).collect();
        let edits = [TextEdit::replace(TextRange::from(0..9), "")];
        let after = iglint_text::apply_edits(&text, &edits).unwrap();
        let problem = safe_fix_problem(&path, &text, &before, &facts, &edits, &scripts(&after));
        assert!(problem.is_some_and(|p| p.contains("compiles")));
    }

    /// The net sees the `__package__` an import binds in a library module:
    /// removing the only import changes what a read of it finds (reviewer
    /// round 12, K1; Ignition 8.3.3 returns `None` before and `'ABSENT'`
    /// after). Event scripts run without `__name__`, so there the same
    /// removal changes nothing.
    #[test]
    fn the_net_sees_the_package_an_import_binds() {
        use crate::rules::script::testing::*;
        use std::sync::Arc;
        let repro = "import math\n\ndef get_package():\n    try:\n        return __package__\n    except NameError:\n        return 'ABSENT'\n";
        let problem = |f: &crate::rules::test_support::Fixture, path: &Path, old: &str| {
            let text = std::fs::read_to_string(path).unwrap();
            let ws = iglint_project::Workspace::discover(&[f.root()]);
            let file = ws.file_by_path(path).expect("file").id;
            let scripts = |t: &str| {
                crate::analyze::resource_scripts(&ws, &Default::default(), file, Arc::from(t))
            };
            let before = scripts(&text);
            let facts: Vec<ScriptFacts> = before.iter().map(ScriptFacts::of).collect();
            let at = text.find(old).unwrap();
            let edits = [TextEdit::replace(
                TextRange::new(
                    iglint_text::TextSize::from(at),
                    iglint_text::TextSize::from(at + old.len()),
                ),
                String::new(),
            )];
            let after = iglint_text::apply_edits(&text, &edits).unwrap();
            safe_fix_problem(path, &text, &before, &facts, &edits, &scripts(&after))
        };
        let f = library("m", repro);
        let p = problem(&f, &library_path(&f, "m"), "import math");
        assert!(
            p.as_deref() == Some("it would change what `__package__` refers to"),
            "{p:?}"
        );
        // A read at module level after the import, and a function-local
        // import.
        let f = library("m", "import math\nx = __package__\n");
        assert!(problem(&f, &library_path(&f, "m"), "import math").is_some());
        let f = library("m", "def g():\n    import math\nprint __package__\n");
        assert!(problem(&f, &library_path(&f, "m"), "import math").is_some());
        // Another module-level import before the read still binds it.
        let f = library(
            "m",
            &repro.replacen("import math", "import os\nimport math", 1),
        );
        assert_eq!(problem(&f, &library_path(&f, "m"), "import math"), None);
        // An event script.
        let f = view(
            ViewScript::Action,
            "\timport math\n\tdef get():\n\t\treturn __package__\n",
        );
        assert_eq!(problem(&f, &view_path(&f), "import math"), None);
    }

    /// Every path that applies or offers a fix goes through
    /// `engine::fix_file` and so through the net: `--fix`, `--fix
    /// --unsafe-fixes`, `--diff` (with and without unsafe fixes; `iglint
    /// ign-lint --fix` runs `engine::check` the same way), the language
    /// server's fix-all (also the `source.fixAll` code action on save and
    /// the fix-all command, both `Live::fix_all`) and its single quick
    /// fixes (`Live::fix_one`). A test marker makes the net reject every
    /// safe fix of the script; the unsafe fix still applies where unsafe
    /// fixes are wanted.
    #[test]
    fn every_fix_path_goes_through_the_net() {
        use crate::engine::FixMode;
        use crate::rules::script::testing::*;

        let code = format!(
            "# {TEST_REJECT_MARKER}\nx = 1  # iglint: ignore[SCR101]\ny = 2  # iglint: ignore[SCR101] legacy\n"
        );
        let safe_kept = |fixed: &str| fixed.contains("x = 1  # iglint: ignore[SCR101]\n");
        let unsafe_applied = |fixed: &str| !fixed.contains("legacy");
        let rules = [("SCR101", ""), ("SCR123", "")];
        let f = library("m", &code);
        let path = library_path(&f, "m");
        // The control: without the marker the safe fix applies.
        let control = library("m", &code.replacen(TEST_REJECT_MARKER, "marker", 1));
        let control_path = library_path(&control, "m");
        let result = control.run(
            &rules,
            FixMode::DiffOnly {
                unsafe_fixes: false,
            },
        );
        let fixed = &result
            .fixed
            .iter()
            .find(|s| s.path == control_path)
            .unwrap()
            .fixed;
        assert!(!safe_kept(fixed), "{fixed}");

        for (mode, wants_unsafe) in [
            (
                FixMode::DiffOnly {
                    unsafe_fixes: false,
                },
                false,
            ),
            (FixMode::DiffOnly { unsafe_fixes: true }, true),
            (FixMode::Safe, false),
            (FixMode::Unsafe, true),
        ] {
            let g = library("m", &code);
            let p = library_path(&g, "m");
            let result = g.run(&rules, mode);
            let fixed = result
                .fixed
                .iter()
                .find(|s| s.path == p)
                .map_or_else(|| code.clone(), |s| s.fixed.clone());
            assert!(safe_kept(&fixed), "{mode:?}: {fixed}");
            assert_eq!(unsafe_applied(&fixed), wants_unsafe, "{mode:?}: {fixed}");
            if mode.writes() {
                assert_eq!(std::fs::read_to_string(&p).unwrap(), fixed, "{mode:?}");
            }
        }

        let settings = crate::rules::test_support::settings(f.root(), &rules);
        let mut live = crate::live::LiveWorkspace::new(settings, vec![f.root().to_path_buf()]);
        for unsafe_fixes in [false, true] {
            let fixed = live
                .fix_all(&path, unsafe_fixes)
                .map_or_else(|| code.clone(), |f| f.fixed);
            assert!(safe_kept(&fixed), "fix-all {unsafe_fixes}: {fixed}");
            assert_eq!(unsafe_applied(&fixed), unsafe_fixes, "fix-all: {fixed}");
        }
        let diagnostics = live
            .check(std::slice::from_ref(&path))
            .pop()
            .unwrap()
            .diagnostics;
        let quick: Vec<(crate::Applicability, bool)> = diagnostics
            .iter()
            .filter(|d| d.fix.is_some())
            .map(|d| {
                (
                    d.fix.as_ref().unwrap().applicability,
                    live.fix_one(&path, d).is_some(),
                )
            })
            .collect();
        assert_eq!(
            quick,
            [
                (crate::Applicability::Safe, false),
                (crate::Applicability::Unsafe, true)
            ]
        );
    }

    fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let p = entry.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
        out
    }
}
