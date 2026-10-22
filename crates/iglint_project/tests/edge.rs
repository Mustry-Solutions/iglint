//! Edge cases: deep inheritance, forbidden overrides, missing parents,
//! malformed and missing files, unlisted files, unicode, CRLF, header
//! variants, every binding/transform/action type, reference resolution and
//! tag/UDT corner cases.

mod common;

use iglint_project::index::Lookup;
use iglint_project::library::is_valid_module_path;
use iglint_project::view::{ActionKind, RefResolution, RefTarget, Unresolved};
use iglint_project::workspace::ParentLink;
use iglint_project::{
    Container, Existence, FileKind, HeaderStatus, LibraryLookup, ModelData, ProjectIndex,
    ResourceKind, ResourceModel, Workspace, build_model, summarize_workspace,
};

fn ws() -> Workspace {
    common::workspace("edge")
}

fn with_model<R>(
    ws: &Workspace,
    project: &str,
    qualified: &str,
    f: impl FnOnce(&ResourceModel<'_>) -> R,
) -> R {
    let r = ws
        .resources
        .iter()
        .find(|r| ws.container_name(r.container) == project && r.qualified_path() == qualified)
        .unwrap_or_else(|| panic!("{project}: {qualified}"));
    let loaded = ws.load_resource(r.id);
    let model = build_model(ws, &loaded);
    f(&model)
}

#[test]
fn deep_inheritance_and_forbidden_override() {
    let ws = ws();
    let root = ws.project_by_name("edge-root").unwrap().id;
    let mid = ws.project_by_name("edge-mid").unwrap();
    let leaf = ws.project_by_name("edge-leaf").unwrap();
    assert_eq!(leaf.parent, ParentLink::Found(mid.id));
    assert_eq!(ws.project_ancestors(leaf.id), vec![mid.id, root]);
    assert_eq!(ws.project_descendants(root).len(), 2);
    assert!(leaf.inheritance.complete);

    let forbidden: Vec<_> = mid
        .inheritance
        .overrides
        .iter()
        .filter(|o| o.forbidden)
        .collect();
    assert_eq!(forbidden.len(), 1);
    assert_eq!(&*ws.resource(forbidden[0].resource).path, "shared/util");
    // The child's resource wins in the effective set (the override is reported, not undone).
    let util = ws
        .effective_resource(
            Container::Project(leaf.id),
            "ignition",
            "script-python",
            "shared/util",
        )
        .unwrap();
    assert_eq!(ws.resource(util).container, Container::Project(mid.id));
    // Inherited from the grandparent.
    let consts = ws
        .effective_resource(
            Container::Project(leaf.id),
            "ignition",
            "script-python",
            "shared/consts",
        )
        .unwrap();
    assert_eq!(ws.resource(consts).container, Container::Project(root));

    let idx = ProjectIndex::build(&ws, summarize_workspace(&ws));
    let Lookup::Found {
        facts: Some(base), ..
    } = idx.view(leaf.id, "Root/Base")
    else {
        panic!()
    };
    let params: Vec<&str> = base.params.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        params,
        ["p1", "p3"],
        "the child's view replaces the parent's whole"
    );
}

#[test]
fn missing_parent_makes_lookups_unknown() {
    let ws = ws();
    let orphan = ws.project_by_name("edge-orphan").unwrap();
    assert_eq!(orphan.parent, ParentLink::Missing("does-not-exist".into()));
    assert!(!orphan.inheritance.complete);
    let idx = ProjectIndex::build(&ws, summarize_workspace(&ws));
    assert_eq!(
        idx.view(orphan.id, "Missing/Elsewhere").existence(),
        Existence::Unknown
    );
    assert_eq!(
        idx.view(orphan.id, "Orphan/View").existence(),
        Existence::Exists
    );
    assert_eq!(
        idx.resolve_library(orphan.id, "nothing.here"),
        LibraryLookup::Unknown
    );
    assert_eq!(
        idx.named_query(orphan.id, "X").existence(),
        Existence::Unknown
    );
    // A bound view path is not a reference.
    with_model(
        &ws,
        "edge-orphan",
        "com.inductiveautomation.perspective/views/Orphan/View",
        |m| {
            let v = m.view().unwrap();
            assert_eq!(v.view_refs.len(), 2);
            assert!(!v.view_refs[0].bound);
            assert!(v.view_refs[1].bound);
        },
    );
    let s = idx
        .summary(
            ws.resources
                .iter()
                .find(|r| &*r.path == "Orphan/View")
                .unwrap()
                .id,
        )
        .unwrap();
    // The bound path's cached literal is not a reference; the binding's
    // literal expression is.
    assert_eq!(s.references.views, ["Missing/Elsewhere", "X/Y"]);

    let core = ws.collections.iter().find(|c| c.name == "core").unwrap();
    assert_eq!(core.parent, ParentLink::Missing("external".into()));
    let dev = ws.collections.iter().find(|c| c.name == "dev").unwrap();
    assert_eq!(dev.inheritance.overrides.len(), 1);
}

#[test]
fn broken_resources() {
    let ws = ws();
    let idx = ProjectIndex::build(&ws, summarize_workspace(&ws));
    let leaf = ws.project_by_name("edge-leaf").unwrap().id;

    with_model(
        &ws,
        "edge-leaf",
        "com.inductiveautomation.perspective/views/Broken/Malformed",
        |m| {
            assert_eq!(m.errors.len(), 1);
            assert!(matches!(m.data, ModelData::None));
        },
    );
    let Lookup::Found { facts, .. } = idx.view(leaf, "Broken/Malformed") else {
        panic!()
    };
    assert!(facts.is_none(), "exists, content unknown");

    let missing = ws
        .resources
        .iter()
        .find(|r| &*r.path == "Broken/MissingFile")
        .unwrap();
    assert_eq!(missing.missing_files, ["view.json", "thumbnail.png"]);
    let Lookup::Found { facts, .. } = idx.view(leaf, "Broken/MissingFile") else {
        panic!()
    };
    assert!(facts.is_none());

    let unlisted = ws
        .files
        .iter()
        .find(|f| f.rel_path.ends_with("Broken/Unlisted/view.json"))
        .unwrap();
    assert_eq!(unlisted.listed, Some(false));
    let stray: Vec<&str> = ws
        .files
        .iter()
        .filter(|f| f.kind == FileKind::Stray)
        .map(|f| f.rel_path.as_str())
        .collect();
    assert_eq!(
        stray,
        [
            "README.md",
            "com.inductiveautomation.perspective/views/NoManifest/view.json"
        ]
    );
    assert_eq!(idx.view(leaf, "NoManifest").existence(), Existence::Missing);
}

#[test]
fn every_action_type() {
    let ws = ws();
    with_model(
        &ws,
        "edge-leaf",
        "com.inductiveautomation.perspective/views/Events/All",
        |m| {
            let v = m.view().unwrap();
            let actions: Vec<_> = v.events.iter().flat_map(|e| &e.actions).collect();
            assert_eq!(actions.len(), 23);
            let unknown = actions
                .iter()
                .filter(|a| matches!(a.kind, ActionKind::Unknown))
                .count();
            assert_eq!(unknown, 2);
            let other = actions
                .iter()
                .filter(|a| matches!(a.kind, ActionKind::Other))
                .count();
            assert_eq!(other, 3);
            // `{"type": "script"}` without a script string has no script.
            let no_script = actions
                .iter()
                .filter(|a| matches!(a.kind, ActionKind::Script { script: None }))
                .count();
            assert_eq!(no_script, 1);
            // A blank script is kept (it fails to compile in Ignition).
            assert!(v.scripts.iter().any(|s| s.is_blank()));
            assert!(v.scripts.iter().any(|s| !s.enabled));
            assert_eq!(v.events.iter().filter(|e| e.group == "model").count(), 0);
        },
    );
}

#[test]
fn unicode_crlf_and_structure_problems() {
    let ws = ws();
    with_model(
        &ws,
        "edge-leaf",
        "com.inductiveautomation.perspective/views/Edge/Unicode",
        |m| {
            let v = m.view().unwrap();
            // The root has no `meta`: Perspective names it `root`.
            assert_eq!(v.components[0].name, "root");
            assert_eq!(v.components[0].ign_lint_path, "root");
            let names: Vec<&str> = v.components.iter().map(|c| c.path.as_str()).collect();
            assert!(names.contains(&"root/Café"));
            assert!(names.contains(&"root/日本"));
            assert_eq!(names.iter().filter(|n| **n == "root/Dup").count(), 2);
            let messages: Vec<&str> = v.problems.iter().map(|p| p.message.as_str()).collect();
            assert_eq!(
                messages,
                [
                    "component has no `meta.name`",
                    "component has no `meta.name`",
                    "`meta` is not an object",
                    "`children` is not an array",
                    "child component is not an object",
                    "custom method has no `name`"
                ]
            );
            // Surrogate pair escapes map back to the twelve escape bytes.
            let s = &v.scripts[0];
            let emoji = s.source.find('😀').unwrap();
            let range = s.file_range(iglint_text::TextRange::at(
                iglint_text::TextSize::new(emoji as u32),
                iglint_text::TextSize::new(4),
            ));
            let file = m
                .resource
                .files
                .iter()
                .find(|f| ws.file(**f).kind == FileKind::View)
                .unwrap();
            let text = std::fs::read_to_string(&ws.file(*file).path).unwrap();
            assert_eq!(range.slice(&text).to_lowercase(), "\\ud83d\\ude00");
            // Extension functions: declared, not declared, unknown component type.
            let declared: Vec<Option<bool>> =
                v.extension_functions.iter().map(|f| f.declared).collect();
            assert_eq!(declared, [Some(true), Some(false), None]);
            assert_eq!(
                v.scripts[3].header,
                "def filterShelvedAlarm(self, shelvedAlarmEvent):"
            );
            assert_eq!(v.scripts[4].header, "def notDeclared(self):");
            assert_eq!(v.scripts[2].header, "def noArgs(self,):");
            let deaf = &v.message_handlers[0];
            assert!(!deaf.page_scope && !deaf.session_scope && !deaf.view_scope);
            // A custom key with dots.
            assert_eq!(v.properties[0].name, "k.with.dots");
        },
    );
}

#[test]
fn event_file_headers() {
    let ws = ws();
    let check = |q: &str, status: HeaderStatus, runtime: &str| {
        with_model(&ws, "edge-leaf", q, |m| {
            let ModelData::EventScript(e) = &m.data else {
                panic!()
            };
            let f = e.script.as_ref().unwrap();
            assert_eq!(f.header, status, "{q}");
            assert_eq!(f.runtime.source, runtime, "{q}");
        })
    };
    check(
        "ignition/timer/bad-header",
        HeaderStatus::Replaced,
        "def handleTimerEvent():\n\tlog(bogus)",
    );
    check(
        "ignition/message/headerless",
        HeaderStatus::Missing,
        "def handleMessage(payload):\nx = payload\n",
    );
    check(
        "ignition/scheduled/one-line",
        HeaderStatus::Replaced,
        "def handleScheduleEvent():\n",
    );
    check(
        "ignition/tag-change/crlf",
        HeaderStatus::Canonical,
        "def onTagChange(initialChange, newValue, previousValue, event, executionCount):\n\tif initialChange:\r\n\t\treturn\r\n",
    );
    check(
        "ignition/startup",
        HeaderStatus::Replaced,
        "def onStartup():\n  pass",
    );
}

#[test]
fn bindings_and_transforms() {
    let ws = ws();
    with_model(
        &ws,
        "edge-leaf",
        "com.inductiveautomation.perspective/views/Bindings/All",
        |m| {
            let v = m.view().unwrap();
            let kinds: Vec<&str> = v.bindings.iter().map(|b| b.kind.name()).collect();
            assert_eq!(
                kinds,
                [
                    "expr",
                    "expr-struct",
                    "tag",
                    "tag",
                    "tag",
                    "query",
                    "tag-history",
                    "tag-history",
                    "http",
                    "property",
                    "unknown",
                    "expr",
                    "property",
                    "expr"
                ]
            );
            let a = &v.bindings[0];
            assert_eq!(a.transforms.len(), 6);
            assert_eq!(a.expression_ids().len(), 4);
            assert!(!v.bindings[11].is_enabled());
            assert!(
                !v.expression(v.bindings[11].expression_ids()[0])
                    .parsed
                    .is_valid()
            );
            // Literal tag history paths and URLs are not expressions.
            let h = &v.bindings[7];
            assert_eq!(h.expression_ids().len(), 1);
            // Style lists from map outputs and a bound `classes`.
            let refs: Vec<(Vec<&str>, bool)> = v
                .style_refs
                .iter()
                .map(|s| (s.classes.iter().map(|c| c.0).collect(), s.bound))
                .collect();
            assert_eq!(
                refs,
                [
                    (vec!["Root/Accent", "Legacy/Old"], false),
                    (vec!["Root/Missing"], false),
                    (vec!["Root/Accent", "Legacy/Old", "missing-one"], false),
                    (vec!["Root/Accent"], true)
                ]
            );
            // A property binding with a relative path from the view resolves to nothing.
            let j = &v.bindings[9];
            let iglint_project::view::BindingConfig::Property(p) = &j.typed else {
                panic!()
            };
            assert_eq!(
                v.resolve_ref(j.owner, p.reference.as_ref().unwrap()),
                RefResolution::Unresolved(Unresolved::NoComponent)
            );
        },
    );
    let idx = ProjectIndex::build(&ws, summarize_workspace(&ws));
    let leaf = ws.project_by_name("edge-leaf").unwrap().id;
    let Lookup::Found { facts: Some(q), .. } = idx.named_query(leaf, "Root/Q") else {
        panic!()
    };
    assert_eq!(q.parameters.len(), 3);
    assert_eq!(
        idx.style_class(leaf, "Legacy/Old").existence(),
        Existence::Exists
    );
    assert_eq!(
        idx.style_class(leaf, "Root/Missing").existence(),
        Existence::Missing
    );
}

#[test]
fn relative_references() {
    let ws = ws();
    with_model(
        &ws,
        "edge-leaf",
        "com.inductiveautomation.perspective/views/Edge/Relative",
        |m| {
            let v = m.view().unwrap();
            let e = &v.expressions[1];
            let results: Vec<String> = e
                .references()
                .into_iter()
                .filter_map(|(r, c)| match c {
                    iglint_expression::Reference::Property(p) => {
                        Some((r.path.clone(), v.resolve_ref(e.owner, &p)))
                    }
                    _ => None,
                })
                .map(|(path, res)| {
                    let short = match res {
                        RefResolution::Resolved(r) => match r.target {
                            RefTarget::View => "view".to_owned(),
                            RefTarget::Component(c) => v.component(c).path.clone(),
                        },
                        RefResolution::Unresolved(Unresolved::MissingComponent {
                            name, ..
                        }) => format!("missing component {name}"),
                        RefResolution::Unresolved(Unresolved::MissingProperty { name, .. }) => {
                            format!("missing property {name}")
                        }
                        RefResolution::Unresolved(Unresolved::AboveRoot { up }) => {
                            format!("above root {up}")
                        }
                        RefResolution::Unresolved(Unresolved::NoComponent) => {
                            "no component".to_owned()
                        }
                        RefResolution::Scope(s) => format!("scope {}", s.scope),
                        RefResolution::Dynamic => "dynamic".to_owned(),
                    };
                    format!("{path} -> {short}")
                })
                .collect();
            assert_eq!(
                results,
                [
                    "../B.custom.c -> root/A/B",
                    ".../A.custom.a -> root/A",
                    "parent.custom.a -> root/A",
                    "this.custom.missing -> missing property missing",
                    "view.params.p -> view",
                    "view.params.nope -> missing property nope",
                    "../../X.props.y -> root/X",
                    "page.props.path -> scope page",
                    "./Kid.props.text -> missing component Kid",
                    "this.props.anything -> root/A/B",
                ]
            );
            // View-level: `this` is the view; absolute paths start at the root.
            let view_expr = &v.expressions[0];
            let unresolved = view_expr
                .references()
                .into_iter()
                .filter_map(|(_, c)| match c {
                    iglint_expression::Reference::Property(p) => {
                        Some(v.resolve_ref(view_expr.owner, &p))
                    }
                    _ => None,
                })
                .filter(RefResolution::is_unresolved)
                .count();
            assert_eq!(unresolved, 0);
            // `custom.onlyConfig` exists only through its propConfig entry.
            assert!(
                v.properties
                    .iter()
                    .any(|p| p.name == "onlyConfig" && p.key.is_none())
            );
        },
    );
}

#[test]
fn library_names_and_shadowing() {
    let ws = ws();
    let idx = ProjectIndex::build(&ws, summarize_workspace(&ws));
    let leaf = ws.project_by_name("edge-leaf").unwrap().id;
    assert!(matches!(
        idx.resolve_library(leaf, "pkg.child.f"),
        LibraryLookup::HiddenByModule { index: 1, .. }
    ));
    assert!(!is_valid_module_path("bad-name"));
    assert_eq!(idx.library_globals(leaf), ["bad-name", "pkg", "shared"]);
}

#[test]
fn tags_and_udts() {
    let ws = ws();
    let idx = ProjectIndex::build(&ws, summarize_workspace(&ws));
    for (path, expected) in [
        ("[edge]Area/Value", Existence::Exists),
        ("[edge]Area/Unit1/Value", Existence::Exists),
        ("[edge]Area/Unit1/Own", Existence::Exists),
        ("[edge]Area/Unit1/Sub/Deep/Deeper", Existence::Exists),
        ("[edge]Area/Unit1/Sub/Nope", Existence::Missing),
        ("[edge]Area/Unit1/Nope", Existence::Missing),
        ("[edge]Area/Ghost/Anything", Existence::Unknown),
        ("[edge]Area/Folder/Inner", Existence::Exists),
        ("[edge]Hidden/Secret", Existence::Missing),
        ("[edge]_types_/Types/Child/Own", Existence::Exists),
        ("[edge]_types_/Types/Child/Value", Existence::Exists),
        ("[edge]_types_/Types/Nested/Inside", Existence::Exists),
        ("[edge]_types_/Types/Nested", Existence::Exists),
        ("[edge]_types_/Nested/Inside", Existence::Missing),
        ("[remote]Anything", Existence::Unknown),
    ] {
        assert_eq!(idx.tag(path), expected, "{path}");
    }
    // Inheritance cycles terminate.
    assert!(idx.udt_parameters("edge", "Types/CycA").is_some());
    assert_eq!(idx.udt_existence("edge", "Types/CycA"), Existence::Exists);
    let mut params = idx.udt_parameters("edge", "Types/Child").unwrap();
    params.sort_unstable();
    assert_eq!(params, ["Extra", "Max"]);
    assert_eq!(idx.history_provider("Hist"), Existence::Exists);
    assert_eq!(idx.history_provider("NoSuchHistorian"), Existence::Missing);
    assert_eq!(idx.database("db"), Existence::Exists);

    with_model(&ws, "core", "ignition/tag-definition/edge/Area", |m| {
        let ModelData::Tags(t) = &m.data else {
            panic!()
        };
        let inner = t
            .tags
            .iter()
            .find(|t| t.path == "Area/Folder/Inner")
            .unwrap();
        assert_eq!(inner.event_scripts.len(), 3);
        assert!(inner.event_scripts[2].kind.is_none());
        assert!(inner.event_scripts[2].script.is_none());
        assert!(!t.scripts[inner.event_scripts[1].script.unwrap().index()].enabled);
        let alarm = &inner.alarms[0];
        let types: Vec<&str> = alarm
            .bindings
            .iter()
            .map(|b| b.bind_type.unwrap().value())
            .collect();
        assert_eq!(types, ["parameter", "Expression", "Tag"]);
        assert_eq!(alarm.pipelines.len(), 2);
        let weird = t.tags.iter().find(|t| t.name == "Weird").unwrap();
        assert_eq!(weird.tag_type, iglint_project::tags::TagType::Other);
        assert_eq!(t.problems.len(), 1);
        let total = t.tags.iter().find(|t| t.name == "Total").unwrap();
        let e = &t.expressions[total.expression.unwrap().index()];
        assert_eq!(e.references().len(), 2);
    });
    with_model(&ws, "core", "ignition/tag-definition/edge/Bad", |m| {
        assert_eq!(m.problems()[0].message, "tag file is not a JSON array");
    });
    let hidden = ws
        .resources
        .iter()
        .find(|r| r.kind == ResourceKind::TagDefinition && &*r.path == "edge/Hidden")
        .unwrap();
    assert!(ws.data_files(hidden.id).all(|f| f.listed == Some(false)));
}

#[test]
fn session_props_and_pages() {
    let ws = ws();
    let idx = ProjectIndex::build(&ws, summarize_workspace(&ws));
    let leaf = ws.project_by_name("edge-leaf").unwrap().id;
    assert_eq!(
        idx.session_property(leaf, "custom", "configOnly"),
        Existence::Exists
    );
    assert_eq!(
        idx.session_property(leaf, "props", "custom-key"),
        Existence::Exists
    );
    // The exact page wins over the parameterised one.
    assert_eq!(
        idx.match_page(leaf, "/edge/fixed").unwrap().url,
        "/edge/fixed"
    );
    assert_eq!(
        idx.match_page(leaf, "/edge/42").unwrap().params,
        [("id".to_owned(), "42".to_owned())]
    );
    // An inheritable project without page config does not know.
    let mid = ws.project_by_name("edge-mid").unwrap().id;
    assert_eq!(idx.page_for_url(mid, "/x").existence(), Existence::Unknown);
}
