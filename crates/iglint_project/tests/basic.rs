//! Discovery, inheritance, models and index on the `basic` fixtures (the
//! invented 8.3 projects and gateway config verified on a real gateway).

mod common;

use iglint_project::event_resource::EventSettings;
use iglint_project::index::HandlerSite;
use iglint_project::library::{ModuleSymbols, Signature, Symbol, SymbolKind};
use iglint_project::view::{RefResolution, Unresolved};
use iglint_project::workspace::ParentLink;
use iglint_project::{
    Container, EventScriptKind, Existence, FileKind, HeaderStatus, HttpMethod, LibraryLookup,
    ModelData, ProjectIndex, ResourceKind, Workspace, build_model, summarize_workspace,
};
use iglint_text::TextRange;

fn ws() -> Workspace {
    common::workspace("basic")
}

fn project(ws: &Workspace, name: &str) -> iglint_project::ProjectId {
    ws.project_by_name(name).unwrap().id
}

#[test]
fn discovers_projects_and_collections() {
    let ws = ws();
    assert!(ws.problems.is_empty(), "{:?}", ws.problems);
    let names: Vec<&str> = ws.projects.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["fixture-api", "fixture-base", "fixture-hmi"]);
    let hmi = ws.project_by_name("fixture-hmi").unwrap();
    let base = ws.project_by_name("fixture-base").unwrap();
    assert_eq!(hmi.parent, ParentLink::Found(base.id));
    assert!(base.inheritable());
    assert_eq!(base.children, vec![hmi.id]);
    assert_eq!(
        ws.project_by_name("fixture-api").unwrap().parent,
        ParentLink::None
    );

    let collections: Vec<(&str, String)> = ws
        .collections
        .iter()
        .map(|c| {
            let parent = match &c.parent {
                ParentLink::Found(p) => ws.collection(*p).name.clone(),
                ParentLink::Missing(n) => format!("missing:{n}"),
                other => format!("{other:?}"),
            };
            (c.name.as_str(), parent)
        })
        .collect();
    assert_eq!(
        collections,
        [
            ("core", "external".to_owned()),
            ("external", "missing:system".to_owned()),
            ("local", "production".to_owned()),
            ("production", "core".to_owned())
        ]
    );
    // Every file is classified; nothing is outside a resource.
    assert!(ws.files.iter().all(|f| f.kind != FileKind::Stray));
    assert!(ws.files.iter().all(|f| f.listed != Some(false)));
    assert!(ws.resources.iter().all(|r| r.missing_files.is_empty()));
}

#[test]
fn starting_inside_a_project_finds_the_project() {
    let root = common::fixture("basic");
    let ws = Workspace::discover(&[
        root.join("projects/fixture-hmi/com.inductiveautomation.perspective/views/Main")
    ]);
    let names: Vec<(&str, bool)> = ws
        .projects
        .iter()
        .map(|p| (p.name.as_str(), p.context_only))
        .collect();
    // The parent is loaded as context.
    assert_eq!(names, [("fixture-hmi", false), ("fixture-base", true)]);
    assert!(
        ws.project_by_name("fixture-hmi")
            .unwrap()
            .inheritance
            .complete
    );
}

#[test]
fn inheritance_replaces_whole_resources() {
    let ws = ws();
    let hmi = project(&ws, "fixture-hmi");
    let base = project(&ws, "fixture-base");
    let c = Container::Project(hmi);
    let badge = ws
        .effective_resource(
            c,
            "com.inductiveautomation.perspective",
            "views",
            "Common/StatusBadge",
        )
        .unwrap();
    assert_eq!(ws.resource(badge).container, Container::Project(base));
    let format = ws
        .effective_resource(c, "ignition", "script-python", "core/format")
        .unwrap();
    assert_eq!(ws.resource(format).container, c);
    let tags = ws
        .effective_resource(c, "ignition", "script-python", "core/tags")
        .unwrap();
    assert_eq!(ws.resource(tags).container, Container::Project(base));
    let overrides = &ws.project(hmi).inheritance.overrides;
    assert_eq!(overrides.len(), 1);
    assert_eq!(overrides[0].resource, format);
    assert!(!overrides[0].forbidden);
    assert_eq!(&*ws.resource(overrides[0].replaced).path, "core/format");
    // The parent does not see the child.
    assert!(
        ws.effective_resource(
            Container::Project(base),
            "com.inductiveautomation.perspective",
            "views",
            "Main/Overview"
        )
        .is_none()
    );

    // Gateway collections: the mode overrides core.
    let production = ws
        .collections
        .iter()
        .find(|c| c.name == "production")
        .unwrap();
    let local = ws.collections.iter().find(|c| c.name == "local").unwrap();
    let db = ws
        .effective_resource(
            Container::Collection(local.id),
            "ignition",
            "database-connection",
            "Production",
        )
        .unwrap();
    assert_eq!(
        ws.resource(db).container,
        Container::Collection(production.id)
    );
    assert_eq!(production.inheritance.overrides.len(), 1);
    assert!(!local.inheritance.complete, "`system` is not on disk");
}

#[test]
fn classifies_files() {
    let ws = ws();
    let kind = |rel: &str| ws.files.iter().find(|f| f.rel_path == rel).map(|f| f.kind);
    assert_eq!(kind("project.json"), Some(FileKind::ProjectManifest));
    assert_eq!(
        kind("com.inductiveautomation.perspective/views/Main/Overview/view.json"),
        Some(FileKind::View)
    );
    assert_eq!(
        kind("com.inductiveautomation.perspective/views/Main/Overview/thumbnail.png"),
        Some(FileKind::Thumbnail)
    );
    assert_eq!(
        kind("ignition/timer/poll-weather/handleTimerEvent.py"),
        Some(FileKind::EventScript(EventScriptKind::GatewayTimer))
    );
    assert_eq!(
        kind("com.inductiveautomation.webdev/resources/api/status/doPatch.py"),
        Some(FileKind::EventScript(EventScriptKind::WebDev(
            HttpMethod::Patch
        )))
    );
    assert_eq!(
        kind("ignition/tag-definition/default/Plant/Line1/udts.json"),
        Some(FileKind::UdtsJson { definitions: false })
    );
    assert_eq!(
        kind("ignition/tag-type-definition/default/Motors/udts.json"),
        Some(FileKind::UdtsJson { definitions: true })
    );
    assert_eq!(
        kind("ignition/tag-definition/default/Plant/unary-resource.json"),
        Some(FileKind::UnaryResourceManifest)
    );
    assert_eq!(
        kind("ignition/database-connection/Production/config.json"),
        Some(FileKind::GatewayConfig)
    );
    assert_eq!(
        kind("com.inductiveautomation.perspective/stylesheet/stylesheet.css"),
        Some(FileKind::Stylesheet)
    );
}

#[test]
fn event_scripts_webdev_and_settings() {
    let ws = ws();
    let mut seen = Vec::new();
    for r in &ws.resources {
        let loaded = ws.load_resource(r.id);
        let model = build_model(&ws, &loaded);
        assert!(model.errors.is_empty());
        assert!(model.problems().is_empty(), "{:?}", model.problems());
        match &model.data {
            ModelData::EventScript(e) => {
                let script = e.script.as_ref().expect("script file");
                assert_eq!(
                    script.header,
                    HeaderStatus::Canonical,
                    "{}",
                    r.qualified_path()
                );
                assert_eq!(
                    script.runtime.source,
                    script.written.source.replace("\r\n", "\n")
                );
                seen.push(e.kind);
                match (&e.settings, e.kind) {
                    (
                        EventSettings::Timer {
                            delay,
                            fixed_delay,
                            shared_thread,
                        },
                        EventScriptKind::GatewayTimer,
                    ) => {
                        assert_eq!(delay.and_then(|d| d.as_i64()), Some(60000));
                        assert_eq!((*fixed_delay, *shared_thread), (Some(true), Some(false)));
                    }
                    (
                        EventSettings::TagChange {
                            paths,
                            change_types,
                            ..
                        },
                        _,
                    ) => {
                        assert_eq!(paths.len(), 2);
                        assert!(paths.iter().all(|(_, p)| p.is_valid()));
                        assert_eq!(change_types.len(), 2);
                    }
                    (EventSettings::Scheduled { cron_expression }, _) => {
                        assert_eq!(cron_expression.unwrap().value(), "0 2 * * *");
                    }
                    (
                        EventSettings::Message { thread_type, .. },
                        EventScriptKind::GatewayMessage,
                    ) => {
                        assert_eq!(thread_type.unwrap().value(), "Dedicated");
                    }
                    _ => {}
                }
            }
            ModelData::WebDev(w) => {
                assert_eq!(w.resource_type, Some("python-resource"));
                assert_eq!(w.methods.len(), 8);
                let post = w
                    .methods
                    .iter()
                    .find(|m| m.method == HttpMethod::Post)
                    .unwrap();
                assert_eq!(post.require_auth, Some(true));
                assert_eq!(post.required_roles, Some("ApiWriter"));
                for m in &w.methods {
                    let s = m.script.as_ref().unwrap();
                    assert_eq!(
                        s.runtime.header,
                        format!("def {}(request, session):", m.method.function_name())
                    );
                }
            }
            _ => {}
        }
    }
    seen.sort_by_key(|k| k.name());
    assert_eq!(seen.len(), 14, "{seen:?}");
}

fn index(ws: &Workspace) -> ProjectIndex<'_> {
    ProjectIndex::build(ws, summarize_workspace(ws))
}

#[test]
fn index_views_queries_styles_pages() {
    let ws = ws();
    let idx = index(&ws);
    let hmi = project(&ws, "fixture-hmi");
    let base = project(&ws, "fixture-base");

    let detail = idx.view(hmi, "Popups/MotorDetail");
    let iglint_project::Lookup::Found {
        facts: Some(facts), ..
    } = detail
    else {
        panic!("{detail:?}")
    };
    assert_eq!(facts.params.len(), 1);
    assert_eq!(facts.params[0].name, "tagPath");
    assert_eq!(
        idx.view(hmi, "Common/StatusBadge").existence(),
        Existence::Exists
    );
    assert_eq!(
        idx.view(hmi, "/Common/StatusBadge").existence(),
        Existence::Exists
    );
    assert_eq!(
        idx.view(hmi, "Does/Not/Exist").existence(),
        Existence::Missing
    );
    // An inheritable project may rely on views of its children.
    assert_eq!(
        idx.view(base, "Main/Overview").existence(),
        Existence::Unknown
    );
    assert_eq!(
        idx.view(base, "Nowhere/AtAll").existence(),
        Existence::Missing
    );

    let iglint_project::Lookup::Found { facts: Some(q), .. } =
        idx.named_query(hmi, "Machines/SetState")
    else {
        panic!()
    };
    assert_eq!(q.query_type.as_deref(), Some("UpdateQuery"));
    assert_eq!(q.parameters.len(), 3);
    assert_eq!(
        idx.named_query(hmi, "Machines/Nope").existence(),
        Existence::Missing
    );

    assert_eq!(
        idx.style_class(hmi, "Brand/Primary").existence(),
        Existence::Exists
    );
    assert_eq!(
        idx.style_class(hmi, "Brand/DoesNotExist").existence(),
        Existence::Missing
    );
    // No gateway themes in the workspace: a class could come from one.
    assert_eq!(
        idx.style_class_reference(hmi, "Brand/DoesNotExist"),
        Existence::Unknown
    );
    assert_eq!(
        idx.style_class_reference(hmi, "Brand/Muted"),
        Existence::Exists
    );

    let page = idx.match_page(hmi, "/motors/Line1").unwrap();
    assert_eq!(page.url, "/motors/:line");
    assert_eq!(page.params, vec![("line".to_owned(), "Line1".to_owned())]);
    assert_eq!(
        idx.match_page(hmi, "/").unwrap().view_path.as_deref(),
        Some("Main/Overview")
    );
    assert!(idx.match_page(hmi, "/nope").is_none());
    assert_eq!(
        idx.page_for_url(hmi, "/nope").existence(),
        Existence::Missing
    );

    assert_eq!(
        idx.session_property(hmi, "custom", "selectedLine"),
        Existence::Exists
    );
    assert_eq!(
        idx.session_property(hmi, "custom", "nope"),
        Existence::Missing
    );
    assert_eq!(
        idx.session_property(hmi, "props", "auth"),
        Existence::Exists
    );
    assert_eq!(
        idx.session_property(hmi, "props", "somethingElse"),
        Existence::Unknown
    );
}

#[test]
fn index_message_handlers_and_usage() {
    let ws = ws();
    let idx = index(&ws);
    let hmi = project(&ws, "fixture-hmi");
    let api = project(&ws, "fixture-api");
    let line = idx.message_handlers(hmi, "line-changed");
    assert_eq!(line.len(), 2);
    assert!(line.iter().any(|h| matches!(h, HandlerSite::Session(_))));
    assert!(line.iter().any(|h| matches!(
        h,
        HandlerSite::Component {
            session_scope: true,
            view_scope: true,
            page_scope: false,
            ..
        }
    )));
    assert!(matches!(
        idx.message_handlers(hmi, "motor-refresh"),
        [HandlerSite::Component {
            page_scope: true,
            ..
        }]
    ));
    assert!(matches!(
        idx.message_handlers(api, "rebuild-report"),
        [HandlerSite::Gateway(_)]
    ));
    assert!(idx.message_handlers(api, "nope").is_empty());

    let c = Container::Project(hmi);
    let badge = ws
        .effective_resource(
            c,
            "com.inductiveautomation.perspective",
            "views",
            "Common/StatusBadge",
        )
        .unwrap();
    let samples = ws
        .effective_resource(
            c,
            "com.inductiveautomation.perspective",
            "views",
            "Sandbox/LintSamples",
        )
        .unwrap();
    let by_line = ws
        .effective_resource(c, "ignition", "named-query", "Machines/ByLine")
        .unwrap();
    let primary = ws
        .effective_resource(
            c,
            "com.inductiveautomation.perspective",
            "style-classes",
            "Brand/Primary",
        )
        .unwrap();
    let running = ws
        .effective_resource(
            c,
            "com.inductiveautomation.perspective",
            "style-classes",
            "State/Running",
        )
        .unwrap();
    assert!(idx.usage_count(badge) >= 3);
    assert_eq!(idx.usage_count(samples), 0);
    assert_eq!(idx.usage_count(by_line), 1);
    assert!(idx.usage_count(primary) >= 2);
    // Used through a `style-list` map transform output, seen from both
    // the parent and the child project.
    assert_eq!(idx.usage_count(running), 2);
    // `runScript("hmi.motors.listMotors", ...)` in an expression.
    let motors = ws
        .effective_resource(c, "ignition", "script-python", "hmi/motors")
        .unwrap();
    assert_eq!(
        idx.usage_count(motors),
        0,
        "symbols unknown: no symbol usage counted"
    );
}

#[test]
fn index_library() {
    let ws = ws();
    let hmi = project(&ws, "fixture-hmi");
    let api = project(&ws, "fixture-api");
    let mut summaries = summarize_workspace(&ws);
    let format = ws
        .effective_resource(
            Container::Project(hmi),
            "ignition",
            "script-python",
            "core/format",
        )
        .unwrap();
    let sym = |name: &str| Symbol {
        name: name.into(),
        kind: SymbolKind::Function(Signature::default()),
        range: TextRange::default(),
    };
    for s in &mut summaries {
        if s.resource == format {
            s.set_module_symbols(ModuleSymbols {
                symbols: vec![sym("withUnit"), sym("percent")],
                dynamic: false,
                constants: Vec::new(),
            });
        }
    }
    let idx = ProjectIndex::build(&ws, summaries);
    assert_eq!(idx.library_globals(hmi), ["core", "hmi"]);
    assert_eq!(idx.library_globals(api), ["api", "legacy"]);
    assert!(
        matches!(idx.resolve_library(hmi, "core.format.withUnit"), LibraryLookup::Symbol { resource, .. } if resource == format)
    );
    assert!(matches!(
        idx.resolve_library(hmi, "core.format.nope"),
        LibraryLookup::MissingAttribute { index: 2, .. }
    ));
    assert!(matches!(
        idx.resolve_library(hmi, "core.tags"),
        LibraryLookup::Module { .. }
    ));
    assert!(matches!(
        idx.resolve_library(hmi, "core.nope.x"),
        LibraryLookup::Missing { index: 1, .. }
    ));
    assert!(matches!(
        idx.resolve_library(hmi, "system.tag.readBlocking"),
        LibraryLookup::NotLibrary
    ));
    assert!(matches!(
        idx.resolve_library(api, "legacy.helpers"),
        LibraryLookup::HiddenByModule { index: 1, .. }
    ));
    assert_eq!(
        idx.library(api).shadowed_packages(),
        vec![("legacy", vec!["legacy.helpers"])]
    );
    assert_eq!(idx.symbol_usage_count(format, "withUnit"), 0);
}

#[test]
fn index_tags_and_gateway() {
    let ws = ws();
    let idx = index(&ws);
    let mut providers: Vec<&str> = idx.tag_providers().collect();
    providers.sort_unstable();
    assert_eq!(providers, ["default"]);
    for (path, expected) in [
        ("[default]Plant/Line1/State", Existence::Exists),
        ("[default]plant/LINE1/state", Existence::Exists),
        ("[default]Plant/Line1/Total.Quality", Existence::Exists),
        ("[default]Plant", Existence::Exists),
        ("[default]Plant/Line1/Motor1/Speed", Existence::Exists),
        ("[default]Plant/Line1/Motor1/Cmd/Start", Existence::Exists),
        (
            "[default]Plant/Line1/Motor2/Setpoints/MaxSpeed",
            Existence::Exists,
        ),
        ("[default]Plant/Line1/Motor1/Nope", Existence::Missing),
        ("[default]Plant/Line2/State", Existence::Missing),
        ("[default]_types_/Motors/Motor/Speed", Existence::Exists),
        ("[default]_types_/Motors/FastMotor", Existence::Exists),
        ("[default]_types_/Motors/Nope", Existence::Missing),
        ("[other]Plant/Line1/State", Existence::Unknown),
        ("[default]Plant/{line}/State", Existence::Unknown),
        ("[.]Motor1/Speed", Existence::Unknown),
        ("Plant/Line1/State", Existence::Unknown),
    ] {
        assert_eq!(idx.tag(path), expected, "{path}");
    }
    let mut params = idx.udt_parameters("default", "Motors/FastMotor").unwrap();
    params.sort_unstable();
    assert_eq!(
        params,
        ["BaseAddress", "DeviceName", "MaxSpeed", "MaxSpeed"]
    );
    assert_eq!(
        idx.udt_existence("default", "Motors/Motor"),
        Existence::Exists
    );
    assert_eq!(
        idx.udt_existence("default", "Motors/Nope"),
        Existence::Missing
    );
    assert_eq!(
        idx.udt_existence("other", "Motors/Motor"),
        Existence::Unknown
    );
    assert_eq!(idx.history_provider("Core Historian"), Existence::Unknown);
    assert_eq!(idx.database("Production"), Existence::Exists);
    assert_eq!(idx.database("Nope"), Existence::Missing);
}

#[test]
fn tag_models() {
    let ws = ws();
    let udt_res = ws
        .resources
        .iter()
        .find(|r| r.kind == ResourceKind::TagTypeDefinition && &*r.path == "default/Motors")
        .unwrap();
    let loaded = ws.load_resource(udt_res.id);
    let model = build_model(&ws, &loaded);
    let ModelData::Tags(t) = &model.data else {
        panic!()
    };
    assert!(t.definitions);
    let speed = t
        .tags
        .iter()
        .find(|t| t.path == "Motors/Motor/Speed")
        .unwrap();
    assert!(speed.in_definition);
    assert_eq!(speed.full_path, "[default]_types_/Motors/Motor/Speed");
    let opc = speed
        .bound_properties
        .iter()
        .find(|b| b.property.value() == "opcItemPath")
        .unwrap();
    let names: Vec<&str> = opc.placeholders.iter().map(|p| p.0).collect();
    assert_eq!(names, ["DeviceName", "BaseAddress"]);
    let text = loaded.file(t.files[0].0).unwrap().text().unwrap();
    assert_eq!(opc.placeholders[0].1.slice(text), "DeviceName");
    let alarm = &speed.alarms[0];
    assert_eq!(alarm.bindings.len(), 2);
    assert!(alarm.bindings.iter().any(|b| b.expression.is_some()));
    assert_eq!(alarm.pipelines.len(), 1);
    let running = t
        .tags
        .iter()
        .find(|t| t.path == "Motors/Motor/Running")
        .unwrap();
    assert_eq!(running.event_scripts.len(), 2);
    let script = &t.scripts[running.event_scripts[0].script.unwrap().index()];
    assert_eq!(
        script.header,
        "def valueChanged(tag,tagPath,previousValue,currentValue,initialChange,missedEvents):"
    );
    assert_eq!(
        script.node_path,
        "[default]_types_/Motors/Motor/Running@eventScripts.valueChanged"
    );
}

#[test]
fn view_reference_resolution() {
    let ws = ws();
    for r in ws.resources.iter().filter(|r| r.kind == ResourceKind::View) {
        let loaded = ws.load_resource(r.id);
        let model = build_model(&ws, &loaded);
        let view = model.view().unwrap();
        for b in &view.bindings {
            if let iglint_project::view::BindingConfig::Property(p) = &b.typed {
                let res = view.resolve_ref(b.owner, p.reference.as_ref().unwrap());
                match p.path.unwrap().value() {
                    "view.params.missing" => assert!(matches!(
                        res,
                        RefResolution::Unresolved(Unresolved::MissingProperty { ref name, .. }) if name == "missing"
                    )),
                    "session.custom.selectedLine" | "session.props.auth.user.userName" => {
                        assert!(matches!(res, RefResolution::Scope(_)))
                    }
                    other => assert!(
                        matches!(res, RefResolution::Resolved(_)),
                        "{other}: {res:?}"
                    ),
                }
            }
        }
        for e in &view.expressions {
            for (_, r) in e.references() {
                if let iglint_expression::Reference::Property(p) = r {
                    let res = view.resolve_ref(e.owner, &p);
                    assert!(!res.is_unresolved(), "{}: {res:?}", e.text());
                }
            }
        }
    }
}
