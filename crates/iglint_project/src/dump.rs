//! A deterministic text dump of resource models, for snapshot tests and
//! debugging.

use std::fmt::Write;

use iglint_expression::Reference;
use iglint_json::Value;

use crate::event_resource::EventSettings;
use crate::expression::EmbeddedExpression;
use crate::model::{ModelData, ResourceModel};
use crate::script::EmbeddedScript;
use crate::view::{
    ActionKind, BindingConfig, NavTarget, PropertyDef, SessionProps, TagHistoryTags, TransformKind,
    View,
};
use crate::workspace::Workspace;

fn compact(v: Option<&Value<'_>>) -> String {
    match v {
        None => "-".to_owned(),
        Some(v) => {
            let mut s = String::new();
            write_compact(&mut s, v);
            if s.len() > 80 {
                let mut cut = 77;
                while !s.is_char_boundary(cut) {
                    cut -= 1;
                }
                s.truncate(cut);
                s.push_str("...");
            }
            s
        }
    }
}

fn write_compact(out: &mut String, v: &Value<'_>) {
    use iglint_json::ValueKind as K;
    match &v.kind {
        K::Null => out.push_str("null"),
        K::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        K::Number(n) => out.push_str(n.raw()),
        K::String(s) => {
            let _ = write!(out, "{:?}", s.value());
        }
        K::Array(a) => {
            out.push('[');
            for (i, e) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_compact(out, e);
            }
            out.push(']');
        }
        K::Object(o) => {
            out.push('{');
            for (i, m) in o.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                let _ = write!(out, "{:?}:", m.key.value());
                write_compact(out, &m.value);
            }
            out.push('}');
        }
    }
}

fn script(out: &mut String, indent: &str, s: &EmbeddedScript<'_>) {
    let _ = writeln!(
        out,
        "{indent}script {} {} scope={} enabled={} anchor={} ign={}",
        s.kind.name(),
        s.node_path,
        s.run_scope.name(),
        s.enabled,
        s.anchor(),
        s.ign_lint_path.as_deref().unwrap_or("-"),
    );
    for line in s.source.lines() {
        let _ = writeln!(out, "{indent}  | {}", line.replace('\t', "→"));
    }
}

fn expression(out: &mut String, indent: &str, e: &EmbeddedExpression<'_>) {
    let refs: Vec<String> = e
        .references()
        .into_iter()
        .map(|(r, c)| match c {
            Reference::Property(_) => format!("prop:{}", r.path),
            Reference::Tag(_) => format!("tag:{}", r.path),
            Reference::TransformValue => "value".to_owned(),
            Reference::Other(o) => format!("other:{o}"),
        })
        .collect();
    let calls: Vec<String> = e
        .parsed
        .expr
        .calls()
        .iter()
        .map(|c| c.name.clone())
        .collect();
    let _ = writeln!(
        out,
        "{indent}expr {} {} {:?} valid={} tentative={} refs={refs:?} calls={calls:?} ign={}",
        e.kind.name(),
        e.node_path,
        e.text(),
        e.parsed.is_valid(),
        e.tentative,
        e.ign_lint_path.as_deref().unwrap_or("-"),
    );
}

fn properties(out: &mut String, props: &[PropertyDef<'_>]) {
    for p in props {
        let _ = writeln!(
            out,
            "property {} value={} direction={} config={} ign={}",
            p.node_path,
            compact(p.value),
            p.direction.map_or("-", |d| d.name()),
            p.config.is_some(),
            p.ign_lint_path
        );
    }
}

fn prop_configs_and_bindings(
    out: &mut String,
    configs: &[crate::view::PropConfig<'_>],
    bindings: &[crate::view::Binding<'_>],
) {
    for c in configs {
        let _ = writeln!(
            out,
            "propConfig {} persistent={:?} direction={} access={} binding={} onChange={} ign={}",
            c.node_path,
            c.persistent,
            c.param_direction.map_or("-", |d| d.name()),
            c.access.unwrap_or("-"),
            c.binding.is_some(),
            c.on_change
                .as_ref()
                .map_or("-".to_owned(), |o| format!("enabled={:?}", o.enabled)),
            c.ign_lint_path
        );
    }
    for b in bindings {
        let _ = write!(
            out,
            "binding {} type={} enabled={:?}",
            b.node_path,
            b.kind.name(),
            b.enabled
        );
        match &b.typed {
            BindingConfig::Property(p) => {
                let _ = write!(
                    out,
                    " path={:?} parsed={} bidirectional={}",
                    p.path.map(|p| p.value()),
                    p.reference.is_some(),
                    p.bidirectional
                );
            }
            BindingConfig::Expr { expression } => {
                let _ = write!(out, " expression={expression:?}");
            }
            BindingConfig::ExprStruct {
                leaves,
                wait_on_all,
            } => {
                let keys: Vec<&str> = leaves.iter().map(|l| l.key.as_str()).collect();
                let _ = write!(out, " leaves={keys:?} waitOnAll={wait_on_all:?}");
            }
            BindingConfig::Tag(t) => {
                let placeholders: Vec<&str> = t.placeholders.iter().map(|p| p.0.as_str()).collect();
                let refs: Vec<String> = t
                    .references
                    .iter()
                    .map(|r| {
                        format!(
                            "{}={}{}",
                            r.key.value(),
                            compact(Some(r.value)),
                            if r.reference.is_some() { "(ref)" } else { "" }
                        )
                    })
                    .collect();
                let _ = write!(
                    out,
                    " mode={:?} tagPath={:?} parsed={} expression={:?} placeholders={placeholders:?} references={refs:?} bidirectional={} fallbackDelay={:?}",
                    t.mode,
                    t.tag_path.map(|p| p.value()),
                    t.parsed_path.as_ref().is_some_and(|p| p.is_valid()),
                    t.path_expression,
                    t.bidirectional,
                    t.fallback_delay
                );
            }
            BindingConfig::Query(q) => {
                let params: Vec<String> = q
                    .parameters
                    .iter()
                    .map(|p| format!("{}={:?}", p.name.value(), p.expression))
                    .collect();
                let _ = write!(
                    out,
                    " queryPath={:?} returnFormat={:?} params={params:?} polling={:?}/{:?}",
                    q.query_path.map(|p| p.value()),
                    q.return_format,
                    q.polling.enabled,
                    q.polling.literal_rate()
                );
            }
            BindingConfig::TagHistory(h) => {
                let tags = match &h.tags {
                    TagHistoryTags::List(l) => l
                        .iter()
                        .map(|t| {
                            format!(
                                "{:?}(expr={:?}) alias={:?} agg={:?}",
                                t.path.map(|p| p.value()),
                                t.path_expression,
                                t.alias,
                                t.aggregate
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", "),
                    TagHistoryTags::Expression(e) => format!("expression {e:?}"),
                    TagHistoryTags::None => "-".to_owned(),
                };
                let _ = write!(
                    out,
                    " tags=[{tags}] dateRange={} returnFormat={:?} polling={:?}/{:?}",
                    compact(h.date_range),
                    h.return_format,
                    h.polling.enabled,
                    h.polling.literal_rate()
                );
            }
            BindingConfig::Http(h) => {
                let _ = write!(
                    out,
                    " url={:?} urlExpression={:?} method={:?} auth={:?} polling={:?}",
                    h.url.map(|u| u.value()),
                    h.url_expression,
                    h.method,
                    h.auth_type,
                    h.polling.enabled
                );
            }
            BindingConfig::None => out.push_str(" config=-"),
        }
        let _ = writeln!(out, " ign={}", b.ign_lint_path);
        for t in &b.transforms {
            let detail = match &t.kind {
                TransformKind::Script { script } => format!("script {script:?}"),
                TransformKind::Expression { expression } => format!("expression {expression:?}"),
                TransformKind::Map(m) => format!(
                    "map in={:?} out={:?} mappings={} fallback={}",
                    m.input_type,
                    m.output_type,
                    m.mappings
                        .iter()
                        .map(|x| format!("{}->{}", compact(x.input), compact(x.output)))
                        .collect::<Vec<_>>()
                        .join(" "),
                    compact(m.fallback)
                ),
                TransformKind::Format(f) => {
                    format!("format {:?} {:?}", f.format_type, f.format_value)
                }
                TransformKind::Unknown => format!("unknown {:?}", t.type_name),
            };
            let _ = writeln!(
                out,
                "  transform {} {detail} ign={}",
                t.node_path, t.ign_lint_path
            );
        }
    }
}

fn view(out: &mut String, v: &View<'_>) {
    for c in &v.components {
        let _ = writeln!(
            out,
            "component {}{} type={} depth={} ptr={} ign={}",
            "  ".repeat(c.depth as usize),
            c.path,
            c.type_id.unwrap_or("-"),
            c.depth,
            c.pointer,
            c.ign_lint_path
        );
    }
    properties(out, &v.properties);
    prop_configs_and_bindings(out, &v.prop_configs, &v.bindings);
    for e in &v.events {
        let _ = writeln!(
            out,
            "event {} list={} ign={}",
            e.node_path, e.is_list, e.ign_lint_path
        );
        for a in &e.actions {
            let detail = match &a.kind {
                ActionKind::Script { script } => format!("script {script:?}"),
                ActionKind::Navigate(NavTarget::Page(p)) => format!("nav page={:?}", p.value()),
                ActionKind::Navigate(NavTarget::Url(u)) => format!("nav url={:?}", u.value()),
                ActionKind::Navigate(NavTarget::View { view, params }) => {
                    format!("nav view={:?} params={}", view.value(), compact(*params))
                }
                ActionKind::Navigate(NavTarget::Other) => "nav other".to_owned(),
                ActionKind::Popup(p) => format!(
                    "popup op={:?} id={:?} view={:?} params={}",
                    p.operation,
                    p.id.map(|i| i.value()),
                    p.view_path.map(|v| v.value()),
                    compact(p.view_params)
                ),
                ActionKind::Dock(d) => {
                    format!("dock op={:?} id={:?}", d.operation, d.id.map(|i| i.value()))
                }
                ActionKind::AlterDock { view, .. } => {
                    format!("alter-dock view={:?}", view.map(|v| v.value()))
                }
                ActionKind::Login => "login".to_owned(),
                ActionKind::Logout => "logout".to_owned(),
                ActionKind::AuthChallenge => "auth-challenge".to_owned(),
                ActionKind::Theme { name } => format!("theme {:?}", name.map(|n| n.value())),
                ActionKind::Other => format!("other {:?}", a.type_name()),
                ActionKind::Unknown => format!("unknown {:?}", a.type_name()),
            };
            let _ = writeln!(
                out,
                "  action {} scope={:?} enabled={:?} {detail} ign={}",
                a.node_path, a.scope, a.enabled, a.ign_lint_path
            );
        }
    }
    for h in &v.message_handlers {
        let _ = writeln!(
            out,
            "messageHandler {} type={:?} page={} session={} view={} script={:?} ign={}",
            h.node_path,
            h.message_type.map(|t| t.value()),
            h.page_scope,
            h.session_scope,
            h.view_scope,
            h.script,
            h.ign_lint_path
        );
    }
    for m in &v.custom_methods {
        let params: Vec<&str> = m.params.iter().map(|p| p.value()).collect();
        let _ = writeln!(
            out,
            "customMethod {} params={params:?} script={:?} ign={}",
            m.node_path, m.script, m.ign_lint_path
        );
    }
    for f in &v.extension_functions {
        let _ = writeln!(
            out,
            "extensionFunction {} enabled={:?} declared={:?} script={:?} ign={}",
            f.node_path, f.enabled, f.declared, f.script, f.ign_lint_path
        );
    }
    for r in &v.view_refs {
        let _ = writeln!(
            out,
            "viewRef {:?} {} path={:?} params={} bound={} paramsBound={}",
            r.kind,
            r.node_path,
            r.path.value(),
            compact(r.params),
            r.bound,
            r.params_bound
        );
    }
    for s in &v.style_refs {
        let classes: Vec<&str> = s.classes.iter().map(|c| c.0).collect();
        let _ = writeln!(
            out,
            "styleRef {:?} {} classes={classes:?} bound={}",
            s.origin, s.node_path, s.bound
        );
    }
    for (i, s) in v.scripts.iter().enumerate() {
        let _ = write!(out, "[{i}] ");
        script(out, "", s);
    }
    for (i, e) in v.expressions.iter().enumerate() {
        let _ = write!(out, "[{i}] ");
        expression(out, "", e);
    }
    for p in &v.problems {
        let _ = writeln!(out, "problem {} {} {}", p.node_path, p.range, p.message);
    }
}

fn session(out: &mut String, s: &SessionProps<'_>) {
    properties(out, &s.properties);
    prop_configs_and_bindings(out, &s.prop_configs, &s.bindings);
    for (i, sc) in s.scripts.iter().enumerate() {
        let _ = write!(out, "[{i}] ");
        script(out, "", sc);
    }
    for (i, e) in s.expressions.iter().enumerate() {
        let _ = write!(out, "[{i}] ");
        expression(out, "", e);
    }
}

/// Dumps a resource model as text.
pub fn dump_model(ws: &Workspace, model: &ResourceModel<'_>) -> String {
    let r = model.resource;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# {} {} in {}{} (overridable={}, files={:?}, missing={:?})",
        r.kind,
        r.qualified_path(),
        ws.container_name(r.container),
        if r.manifest.missing {
            " without manifest"
        } else {
            ""
        },
        r.manifest.overridable(),
        r.files
            .iter()
            .map(|f| {
                let f = ws.file(*f);
                format!(
                    "{}:{}{}",
                    f.name(),
                    f.kind,
                    if f.listed == Some(false) {
                        "(unlisted)"
                    } else {
                        ""
                    }
                )
            })
            .collect::<Vec<_>>(),
        r.missing_files,
    );
    for e in &model.errors {
        let _ = writeln!(
            out,
            "error {} {} {}",
            ws.file(e.file).name(),
            e.range,
            e.message
        );
    }
    match &model.data {
        ModelData::View { view: v, .. } => view(&mut out, v),
        ModelData::SessionProps { props, .. } => session(&mut out, props),
        ModelData::StyleClass(s) => {
            let _ = writeln!(
                out,
                "styleClass legacy={} empty={} base={} animation={} variants={}",
                s.legacy,
                s.is_empty(),
                compact(s.base_style),
                compact(s.animation),
                s.variants
                    .iter()
                    .map(|v| format!("{:?}/{:?}:{}", v.pseudo, v.media_query, compact(v.style)))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        ModelData::PageConfig(p) => {
            for page in &p.pages {
                let _ = writeln!(
                    out,
                    "page {:?} view={:?} title={:?} params={:?}",
                    page.url.value(),
                    page.view_path.map(|v| v.value()),
                    page.title,
                    page.url_params()
                );
                for d in &page.docks {
                    let _ = writeln!(
                        out,
                        "  dock {} id={:?} view={:?} show={:?}",
                        d.side,
                        d.id.map(|i| i.value()),
                        d.view_path.map(|v| v.value()),
                        d.show
                    );
                }
            }
            for d in &p.shared_docks {
                let _ = writeln!(
                    out,
                    "sharedDock {} id={:?} view={:?} show={:?}",
                    d.side,
                    d.id.map(|i| i.value()),
                    d.view_path.map(|v| v.value()),
                    d.show
                );
            }
        }
        ModelData::Stylesheet { text, .. } => {
            let _ = writeln!(
                out,
                "stylesheet classes={:?}",
                crate::perspective::css_classes(text)
            );
        }
        ModelData::Theme { css } => {
            let _ = writeln!(out, "theme css files={}", css.len());
        }
        ModelData::NamedQuery(q) => {
            let params: Vec<String> = q
                .parameters
                .iter()
                .map(|p| {
                    format!(
                        "{}:{}:{:?}",
                        p.identifier.map_or("?", |i| i.value()),
                        p.kind.name(),
                        p.sql_type
                    )
                })
                .collect();
            let placeholders: Vec<String> = q
                .placeholders()
                .iter()
                .map(|p| format!("{:?}{}@{:?}", p.kind, p.name, p.range))
                .collect();
            let _ = writeln!(
                out,
                "namedQuery type={:?} database={:?} enabled={:?} cache={:?} doc={:?} params={params:?} placeholders={placeholders:?}",
                q.query_type, q.database, q.enabled, q.cache_enabled, q.documentation
            );
        }
        ModelData::EventScript(e) => {
            let settings = match &e.settings {
                EventSettings::Timer {
                    delay,
                    fixed_delay,
                    shared_thread,
                } => {
                    format!(
                        "timer delay={} fixedDelay={fixed_delay:?} sharedThread={shared_thread:?}",
                        compact(*delay)
                    )
                }
                EventSettings::Scheduled { cron_expression } => {
                    format!("scheduled cron={:?}", cron_expression.map(|c| c.value()))
                }
                EventSettings::TagChange {
                    paths,
                    change_types,
                    ..
                } => format!(
                    "tagChange paths={:?} changeTypes={:?}",
                    paths
                        .iter()
                        .map(|(s, p)| format!("{}(valid={})", s.value(), p.is_valid()))
                        .collect::<Vec<_>>(),
                    change_types.iter().map(|c| c.value()).collect::<Vec<_>>()
                ),
                EventSettings::Message { thread_type, .. } => {
                    format!("message threadType={:?}", thread_type.map(|t| t.value()))
                }
                EventSettings::KeyEvent {
                    event_mode,
                    event_bound,
                    key,
                    code,
                } => {
                    format!(
                        "keyEvent mode={event_mode:?} bound={event_bound:?} key={key:?} code={code:?}"
                    )
                }
                EventSettings::None => "-".to_owned(),
            };
            let _ = writeln!(
                out,
                "event {} enabled={:?} {settings}",
                e.kind.name(),
                e.enabled
            );
            if let Some(f) = &e.script {
                let _ = writeln!(out, "header {:?} firstLine={}", f.header, f.first_line);
                script(&mut out, "", &f.runtime);
            }
        }
        ModelData::WebDev(w) => {
            let _ = writeln!(out, "webdev type={:?}", w.resource_type);
            for m in &w.methods {
                let _ = writeln!(
                    out,
                    "method {} enabled={:?} requireAuth={:?} requireHttps={:?} roles={:?} header={:?}",
                    m.method.function_name(),
                    m.enabled,
                    m.require_auth,
                    m.require_https,
                    m.required_roles,
                    m.script.as_ref().map(|s| s.header)
                );
                if let Some(s) = &m.script {
                    script(&mut out, "  ", &s.runtime);
                }
            }
        }
        ModelData::Library(l) => {
            let _ = writeln!(out, "module {} hintScope={:?}", l.module_path, l.hint_scope);
            script(&mut out, "", &l.script);
        }
        ModelData::Tags(t) => {
            let _ = writeln!(
                out,
                "tags provider={} folder={:?} definitions={} base={} config={}",
                t.provider,
                t.folder,
                t.definitions,
                t.base_path(),
                compact(t.config)
            );
            for tag in &t.tags {
                let depth = tag.path.matches('/').count();
                let _ = writeln!(
                    out,
                    "tag {}{} type={:?} typeId={:?} source={:?} dataType={:?} inDefinition={} params={:?} history={:?}",
                    "  ".repeat(depth),
                    tag.full_path,
                    tag.tag_type,
                    tag.type_id.map(|s| s.value()),
                    tag.value_source,
                    tag.data_type,
                    tag.in_definition,
                    tag.parameters
                        .iter()
                        .map(|p| p.name.value())
                        .collect::<Vec<_>>(),
                    tag.history_provider.map(|h| h.value()),
                );
                for b in &tag.bound_properties {
                    let _ = writeln!(
                        out,
                        "  bound {} {:?} {:?} placeholders={:?} expression={:?}",
                        b.property.value(),
                        b.bind_type,
                        b.binding.map(|s| s.value()),
                        b.placeholders.iter().map(|p| p.0).collect::<Vec<_>>(),
                        b.expression
                    );
                }
                for a in &tag.alarms {
                    let _ = writeln!(
                        out,
                        "  alarm {} mode={:?} priority={:?} bindings={:?} pipelines={:?}",
                        a.node_path,
                        a.mode,
                        a.priority,
                        a.bindings
                            .iter()
                            .map(|b| format!(
                                "{}:{:?}",
                                b.property.value(),
                                b.bind_type.map(|t| t.value())
                            ))
                            .collect::<Vec<_>>(),
                        a.pipelines
                            .iter()
                            .map(|(k, v)| format!("{k}={}", v.value()))
                            .collect::<Vec<_>>()
                    );
                }
                for e in &tag.event_scripts {
                    let _ = writeln!(
                        out,
                        "  eventScript {:?} enabled={:?} script={:?}",
                        e.event_id.map(|i| i.value()),
                        e.enabled,
                        e.script
                    );
                }
            }
            for (i, s) in t.scripts.iter().enumerate() {
                let _ = write!(out, "[{i}] ");
                script(&mut out, "", s);
            }
            for (i, e) in t.expressions.iter().enumerate() {
                let _ = write!(out, "[{i}] ");
                expression(&mut out, "", e);
            }
            for p in &t.problems {
                let _ = writeln!(out, "problem {} {} {}", p.node_path, p.range, p.message);
            }
        }
        ModelData::Gateway(g) => {
            let _ = writeln!(
                out,
                "gateway {} enabled={:?} details={:?}",
                g.kind, g.enabled, g.details
            );
        }
        ModelData::None => out.push_str("(no model)\n"),
    }
    out
}
