//! Code that runs from places other than scripts: the Python that
//! expressions hand to `runScript`, and resources whose code iglint does
//! not read. They feed the closed world (`References::unknown_code`) and
//! the library references of the project index.
//!
//! `runScript(script, [pollRate], [args...])` is the only expression
//! function of Ignition 8.3 that runs Python (measured over every
//! expression function class of the 8.3.3 image, gateway and modules:
//! `ScriptFunction` in common, Perspective's `PerspectiveRunScriptFunction`;
//! Vision's `objectScript` exists only in Vision clients, whose windows
//! iglint does not read). Measured on 8.3.3 (`oracle/run-oracle.sh
//! --library-routes`): it compiles `"__RESULT = " + script` in exec mode,
//! runs it with the project's library packages, `system` and `sys` as
//! locals (Perspective's adds `self`, the element the expression belongs
//! to, and its `view`, `page` and `session`), and calls `__RESULT` with
//! the remaining arguments when it is callable, also when there are none.
//! So ([`run_script_source`]):
//!
//! * a literal dotted name (`"lib.util.fn"`, `"eval"`, `"open"`,
//!   `"m.f.func_globals.get"`) is analysed as the code `name(*args)` with
//!   arguments that are unknown values: through the same vetting, frame
//!   routes, file reads and references as a script (review round 27). The
//!   summary layer records it as a library reference as well;
//! * any other literal is the code `__RESULT = <literal>`, parsed and
//!   analysed like a script; if it does not parse it is unknown code;
//! * a computed first argument is unknown code;
//! * in a Perspective expression, code that names `self`, `view`, `page` or
//!   `session` reaches the Perspective objects, which the view analyses
//!   do not follow there: unknown code, and `VIEW015` counts every
//!   property of the view as read and written
//!   ([`reaches_perspective_objects`]).
//!
//! Resources of kinds that hold code or expressions iglint does not model
//! (reports, alarm pipelines, SFC charts, Vision windows and templates,
//! transaction groups, event streams, the deprecated app library, pre-8.3
//! aggregate event scripts, unknown module resources) are unknown code.
//! So is any JSON string of a modelled resource that mentions `runScript`
//! but is not an expression or script iglint reads.

use iglint_expression::ExprKind;
use iglint_json::ValueKind;
use iglint_project::EmbeddedExpression;
use iglint_project::model::{LoadedResource, ResourceModel};
use iglint_project::script::{IMPLICIT_GLOBALS, ScriptBody};
use iglint_project::summary::{References, ResourceSummary, UnknownSite};
use iglint_project::{FileId, ResourceKind};
use iglint_text::{TextRange, TextSize};
use rustc_hash::FxHashSet;

use super::FactsContext;

/// Whether `text` is a plain dotted name (`lib.util.fn`), which `runScript`
/// resolves like a library reference.
pub fn is_dotted_name(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty()
        && text.split('.').all(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// The Python `runScript` runs for the literal script `script` (see the
/// module docs): `"__RESULT = " + script` as Ignition compiles it, or for a
/// dotted name the call of it with arguments that are unknown values (the
/// expression's other arguments, which Ignition passes when the value is
/// callable).
pub fn run_script_source(script: &str) -> String {
    if is_dotted_name(script) {
        format!("def runScript(*args):\n    {}(*args)\n", script.trim())
    } else {
        format!("__RESULT = {script}")
    }
}

/// The names Perspective's `runScript` adds to the locals of the code it
/// runs (measured on 8.3.3: `PerspectiveRunScriptFunction.initLocalsMap`).
pub const PERSPECTIVE_RUN_SCRIPT_LOCALS: &[&str] = &["self", "view", "page", "session"];

/// Whether `module` names one of [`PERSPECTIVE_RUN_SCRIPT_LOCALS`]
/// anywhere (default deny: a binding of the name does not count as hiding
/// it).
fn names_perspective_locals(module: &iglint_python::ast::Module) -> bool {
    struct Names(bool);
    impl<'a> iglint_python::visitor::Visitor<'a> for Names {
        fn visit_expr(&mut self, expr: &'a iglint_python::ast::Expr) {
            if let iglint_python::ast::Expr::Name(n) = expr
                && PERSPECTIVE_RUN_SCRIPT_LOCALS.contains(&&*n.id)
            {
                self.0 = true;
            }
            iglint_python::visitor::walk_expr(self, expr);
        }
    }
    let mut names = Names(false);
    iglint_python::visitor::Visitor::visit_module(&mut names, module);
    names.0
}

/// Whether a `runScript` call of the Perspective expression `e` may reach
/// the Perspective objects Ignition hands its code (`self`, `view`, `page`,
/// `session`): its script is computed, or its code names one of them. The
/// first such call, by its range in the expression.
pub fn reaches_perspective_objects(e: &EmbeddedExpression<'_>) -> Option<TextRange> {
    if e.function_context() != iglint_stubs::ExpressionContext::Perspective {
        return None;
    }
    e.parsed.expr.calls().into_iter().find_map(|call| {
        if !call.is("runScript") {
            return None;
        }
        let first = call.args.first()?;
        let reaches = match &first.unparenthesized().kind {
            ExprKind::String(code) => {
                let parsed = iglint_python::parse_module(&run_script_source(code));
                // Code that does not parse never runs.
                parsed.errors.is_empty() && names_perspective_locals(&parsed.module)
            }
            _ => true,
        };
        reaches.then_some(call.name_range)
    })
}

fn unknown(summary: &mut ResourceSummary, file: FileId, range: TextRange, what: String) {
    let refs = &mut summary.references;
    let site = UnknownSite { file, range, what };
    // Code iglint does not read may read files too.
    refs.unknown_code = true;
    refs.reads_files = true;
    if refs.file_read_site.is_none() {
        refs.file_read_site = Some(site.clone());
    }
    if refs.unknown_site.is_none() {
        refs.unknown_site = Some(site);
    }
}

fn one_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    let mut out: String = line.chars().take(60).collect();
    if out.len() < line.len() || text.trim_end().contains('\n') {
        out.push_str("...");
    }
    out
}

/// Adds what `runScript` calls of the resource's expressions run.
pub fn run_script_code(
    model: &ResourceModel<'_>,
    ctx: &FactsContext<'_>,
    summary: &mut ResourceSummary,
) {
    for (file, e) in model.expressions() {
        for call in e.parsed.expr.calls() {
            if !call.is("runScript") {
                continue;
            }
            let call_range = {
                let end = call
                    .args
                    .last()
                    .map_or(call.name_range.end(), |a| a.range.end());
                // Through the closing parenthesis.
                let close = e
                    .text()
                    .get(end.to_usize()..)
                    .and_then(|rest| rest.find(')'))
                    .map_or(end, |i| end + TextSize::new(i as u32 + 1));
                TextRange::new(call.name_range.start(), close)
            };
            let what = || {
                one_line(
                    e.text()
                        .get(call_range.start().to_usize()..call_range.end().to_usize())
                        .unwrap_or("runScript"),
                )
            };
            let Some(first) = call.args.first() else {
                continue;
            };
            let at = e.file_range(first.range);
            let ExprKind::String(code) = &first.unparenthesized().kind else {
                // A computed script: any code.
                unknown(summary, file, e.file_range(call_range), what());
                continue;
            };
            if is_dotted_name(code) {
                // What the name holds becomes the expression's value when it
                // is not callable (`runScript("m")` puts the module object in
                // a property or a tag): a use of the object as a value, which
                // the index judges (a function or class is called instead,
                // and only its result leaves).
                let name = code.trim().to_owned();
                let refs = &mut summary.references;
                refs.dynamic.library.push(name.clone());
                refs.library_values.push((
                    name,
                    UnknownSite {
                        file,
                        range: at,
                        what: what(),
                    },
                ));
            }
            let parsed = iglint_python::parse_module(&run_script_source(code));
            if !parsed.errors.is_empty() {
                unknown(summary, file, at, what());
                continue;
            }
            if e.function_context() == iglint_stubs::ExpressionContext::Perspective
                && names_perspective_locals(&parsed.module)
            {
                // The component, view, page or session, which nothing
                // follows here.
                unknown(summary, file, at, what());
            }
            let model = iglint_python_semantic::SemanticModel::build(
                &parsed.module,
                &iglint_python_semantic::SemanticOptions {
                    injected: IMPLICIT_GLOBALS.to_vec(),
                    unknown_globals: false,
                    ..Default::default()
                },
            );
            let mut inner = References::default();
            let ctx = FactsContext {
                module: None,
                ..*ctx
            };
            super::refs::collect(&model, &ctx, &mut inner);
            // Sites inside the code are reported at the literal.
            for site in inner
                .unknown_site
                .iter_mut()
                .chain(&mut inner.file_read_site)
            {
                *site = UnknownSite {
                    file,
                    range: at,
                    what: what(),
                };
            }
            for site in inner
                .unvetted_imports
                .iter_mut()
                .map(|(_, s)| s)
                .chain(inner.unvetted_names.iter_mut().map(|(_, s)| s))
                .chain(inner.library_changes.iter_mut().map(|(_, s)| s))
                .chain(inner.library_values.iter_mut().map(|(_, s)| s))
            {
                *site = UnknownSite {
                    file,
                    range: at,
                    what: what(),
                };
            }
            summary.references.extend(inner);
        }
    }
}

/// Whether resources of `kind` hold code or expressions that iglint does
/// not read.
pub fn holds_unread_code(kind: ResourceKind) -> bool {
    matches!(
        kind,
        ResourceKind::Report
            | ResourceKind::AlarmPipeline
            | ResourceKind::SfcChart
            | ResourceKind::VisionWindow
            | ResourceKind::VisionTemplate
            | ResourceKind::VisionOther
            | ResourceKind::TransactionGroup
            | ResourceKind::EventStream
            | ResourceKind::ScriptAppLibrary
            | ResourceKind::LegacyEventScripts
            | ResourceKind::Unknown
    )
}

/// Marks code iglint does not read as unknown: resources of kinds it does
/// not model ([`holds_unread_code`]), and JSON strings that mention
/// `runScript` outside the expressions and scripts it models.
pub fn unread_code(
    model: &ResourceModel<'_>,
    loaded: &LoadedResource,
    summary: &mut ResourceSummary,
) {
    let resource = model.resource;
    if resource.manifest.missing {
        return;
    }
    if holds_unread_code(resource.kind) {
        let file = resource
            .files
            .first()
            .copied()
            .unwrap_or(resource.manifest.file);
        unknown(
            summary,
            file,
            TextRange::empty(TextSize::ZERO),
            format!(
                "{} resource {} (not analysed)",
                resource.type_id, resource.path
            ),
        );
        return;
    }
    let mut read: FxHashSet<(FileId, u32)> = FxHashSet::default();
    for (file, e) in model.expressions() {
        read.insert((file, e.source.range().start().to_u32()));
    }
    for (file, s) in model.data.scripts() {
        if let ScriptBody::Json(body) = &s.body {
            read.insert((file, body.range().start().to_u32()));
        }
    }
    for f in &loaded.files {
        let Some(root) = f.json.as_ref().and_then(|d| d.root()) else {
            continue;
        };
        let mut found: Option<(TextRange, String)> = None;
        root.walk(|_, v| {
            if found.is_none()
                && let ValueKind::String(s) = &v.kind
                && s.value().to_ascii_lowercase().contains("runscript")
                && !read.contains(&(f.file, s.range().start().to_u32()))
            {
                found = Some((s.range(), one_line(s.value())));
            }
            found.is_none()
        });
        if let Some((range, what)) = found {
            unknown(summary, f.file, range, what);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Applicability;
    use crate::rules::script::testing::{flagged, library};

    /// A view whose label text is bound to `expression`.
    fn expression_view(
        f: crate::rules::test_support::Fixture,
        expression: &str,
    ) -> crate::rules::test_support::Fixture {
        let expression = serde_json::to_string(expression).unwrap();
        f.with_view(
            "Expr",
            &format!(
                r#"{{"root": {{"type": "ia.display.label", "meta": {{"name": "root"}}, "props": {{"text": ""}},
                "propConfig": {{"props.text": {{"binding": {{"type": "expr", "config": {{"expression": {expression}}}}}}}}}}}}}"#
            ),
        )
    }

    const CLEAN: &str = "def g():\n    import os\n    return 1\n";

    /// The SCR102 fix of the clean module `c`: safe only in a closed world
    /// without unknown code.
    fn clean_fix(f: &crate::rules::test_support::Fixture) -> (Applicability, Option<String>) {
        let d = f.lint(&["SCR102"]);
        let d = d
            .iter()
            .find(|d| d.message.contains("`os`"))
            .expect("SCR102 on c");
        let fix = d.fix.as_ref().unwrap();
        (
            fix.applicability,
            fix.unsafe_reason
                .as_ref()
                .and_then(|r| r.site.as_ref())
                .map(|s| s.what.clone()),
        )
    }

    #[test]
    fn run_script_names_are_library_references() {
        // `runScript('lib.util.fn')` uses `fn`, and a dotted path through a
        // module attribute (`lib.util.json.dumps`) uses `json`.
        let util = "import json\ndef fn():\n    pass\ndef unused():\n    pass\n";
        let f = expression_view(library("lib/util", util), "runScript('lib.util.fn')");
        assert_eq!(flagged(&f.lint(&["REF012"])), ["unused"]);
        let f = expression_view(
            library("lib/util", util),
            "runScript('lib.util.json.dumps', 0, 1)",
        );
        assert!(f.lint(&["SCR102"]).is_empty());
        let f = expression_view(library("lib/util", util), "runScript('lib.util.fn')");
        assert_eq!(f.lint(&["SCR102"]).len(), 1);
        // Code that calls a library function references it too.
        let f = expression_view(library("lib/util", util), "runScript('lib.util.fn() + 1')");
        assert_eq!(flagged(&f.lint(&["REF012"])), ["unused"]);
        // A plain call keeps the closed world.
        let f = expression_view(library("c", CLEAN), "runScript('len([1, 2])')");
        assert_eq!(clean_fix(&f).0, Applicability::Safe);
    }

    #[test]
    fn run_script_code_that_is_unknown_closes_the_world() {
        for (expression, what) in [
            // computed script
            (
                "runScript({view.custom.name})",
                "runScript({view.custom.name})",
            ),
            ("runScript('lib.' + 'f')", "runScript('lib.' + 'f')"),
            // code that is not vetted or reads frames
            (
                "runScript(\"__import__('os')\")",
                "runScript(\"__import__('os')\")",
            ),
            (
                "runScript('sys._getframe(0)')",
                "runScript('sys._getframe(0)')",
            ),
            // code that does not parse
            ("runScript('lib.(')", "runScript('lib.(')"),
        ] {
            let f = expression_view(library("c", CLEAN), expression);
            assert_eq!(
                clean_fix(&f),
                (Applicability::Unsafe, Some(what.to_owned())),
                "{expression}"
            );
        }
    }

    #[test]
    fn dotted_run_script_targets_are_code() {
        // Review round 27: Ignition evaluates the name and calls it with
        // the other arguments, so a builtin or `system.*` function named
        // there runs like a call in a script: frame routes, code that is
        // not vetted and file reads keep every fix unsafe, in a view and
        // in a tag expression.
        for expression in [
            "runScript('eval', 0, {view.custom.code})",
            "runScript('eval')",
            "runScript('execfile', 0, 'x.py')",
            "runScript('locals')",
            "runScript('getattr', 0, {view.custom.o}, 'f_locals')",
            "runScript('sys._getframe', 0, 0)",
            "runScript('system.util.execute', 0, 'true')",
            "runScript('system.tag.configure', 0, '[default]', '[]')",
            "runScript('system.nonexistent.fn', 0)",
            "runScript('open', 0, {view.custom.p})",
            "runScript('file', 0, '/tmp/x')",
            "runScript('system.file.readFileAsString', 0, {view.custom.p})",
            "runScript(' eval ', 0, '1')",
        ] {
            let f = expression_view(library("c", CLEAN), expression);
            assert_eq!(
                clean_fix(&f),
                (Applicability::Unsafe, Some(super::one_line(expression))),
                "{expression}"
            );
            let tag = expression.replace("{view.custom.code}", "'1'");
            let tag = tag
                .replace("{view.custom.o}", "'x'")
                .replace("{view.custom.p}", "'/tmp/x'");
            let f = crate::rules::tag::fixtures::gateway()
                .resource("projects/p/ignition/script-python/c", &[("code.py", CLEAN)]);
            let f = crate::rules::tag::fixtures::tags(
                f,
                &format!(
                    r#"[{{"name": "T", "tagType": "AtomicTag", "valueSource": "expr", "expression": {}}}]"#,
                    serde_json::to_string(&tag).unwrap()
                ),
                "[]",
            );
            assert_eq!(clean_fix(&f).0, Applicability::Unsafe, "{tag}");
        }
        // A library function or a vetted builtin keeps the closed world.
        for expression in [
            "runScript('lib.util.fn', 0, {view.custom.p}, 1)",
            "runScript('len', 0, 'abc')",
        ] {
            let f = expression_view(
                library("c", CLEAN).resource(
                    "projects/p/ignition/script-python/lib/util",
                    &[("code.py", "def fn(*a):\n    return 1\n")],
                ),
                expression,
            );
            assert_eq!(clean_fix(&f).0, Applicability::Safe, "{expression}");
        }
    }

    #[test]
    fn perspective_run_script_code_reaches_the_perspective_objects() {
        // Perspective's runScript runs its code with `self`, `view`,
        // `page` and `session` (measured on 8.3.3): code that names them
        // is unknown code; in a tag expression they are plain names.
        for expression in [
            "runScript('self.view.custom.x')",
            "runScript('view.custom.get', 0, 'x')",
            "runScript('page.id')",
            "runScript('str(session)')",
            "runScript('[c for c in self.getChildren()]', 1000)",
        ] {
            let f = expression_view(library("c", CLEAN), expression);
            assert_eq!(
                clean_fix(&f),
                (Applicability::Unsafe, Some(expression.to_owned())),
                "{expression}"
            );
        }
        let f = crate::rules::tag::fixtures::gateway()
            .resource("projects/p/ignition/script-python/c", &[("code.py", CLEAN)]);
        let f = crate::rules::tag::fixtures::tags(
            f,
            r#"[{"name": "T", "tagType": "AtomicTag", "valueSource": "expr", "expression": "runScript('str(self)')"}]"#,
            "[]",
        );
        assert_eq!(clean_fix(&f).0, Applicability::Safe);
    }

    #[test]
    fn run_script_values_leave_the_expression() {
        // A name that is not callable becomes the expression's value:
        // `runScript('lib.util')` puts the module in a property, where any
        // script can read its names (review round 27).
        let util = "import json\ndef fn():\n    pass\n";
        let f = expression_view(library("lib/util", util), "runScript('lib.util')");
        assert!(f.lint(&["SCR102"]).is_empty());
        let f = expression_view(library("lib/util", util), "runScript('lib.util.json')");
        assert!(f.lint(&["SCR102"]).is_empty());
        // A function is called, and only its result leaves.
        let f = expression_view(library("lib/util", util), "runScript('lib.util.fn')");
        assert_eq!(f.lint(&["SCR102"]).len(), 1);
    }

    #[test]
    fn unread_code_closes_the_world() {
        // A Vision window: iglint does not read its scripts.
        let f = library("c", CLEAN).resource(
            "projects/p/com.inductiveautomation.vision/windows/Main",
            &[("window.bin", "")],
        );
        assert_eq!(clean_fix(&f).0, Applicability::Unsafe);
        // A view string that mentions runScript outside any expression.
        let f = library("c", CLEAN).with_view(
            "Other",
            r#"{"root": {"type": "ia.display.label", "meta": {"name": "root"}, "props": {"text": "runScript('x')"}}}"#,
        );
        assert_eq!(clean_fix(&f).0, Applicability::Unsafe);
        // A tag expression is read: a dotted runScript is a reference.
        let f = crate::rules::tag::fixtures::gateway()
            .resource("projects/p/ignition/script-python/c", &[("code.py", CLEAN)])
            .resource(
                "projects/p/ignition/script-python/lib/util",
                &[("code.py", "def fn():\n    pass\n")],
            );
        let f = crate::rules::tag::fixtures::tags(
            f,
            r#"[{"name": "T", "tagType": "AtomicTag", "valueSource": "expr", "expression": "runScript('lib.util.fn')"}]"#,
            "[]",
        );
        assert_eq!(clean_fix(&f).0, Applicability::Safe);
        assert!(!flagged(&f.lint(&["REF012"])).contains(&"fn".to_owned()));
    }
}
