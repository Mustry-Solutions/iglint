//! Non-view resources: manifests, script library, named queries, style
//! classes, page configuration, session properties, gateway events and the
//! gateway configuration tree with tags and UDTs.

use serde_json::{Map, Value, json};

use crate::gson;
use crate::names;
use crate::script::{self, ScriptCtx};
use crate::world::{self, Role, World};

pub const BASE: &str = "bench-base";
pub const HMI: &str = "bench-hmi";
pub const PERSPECTIVE: &str = "com.inductiveautomation.perspective";

/// A file to write, relative to the corpus root.
pub struct File {
    pub path: String,
    pub text: String,
}

fn file(path: String, text: String) -> File {
    File { path, text }
}

pub fn manifest(scope: &str, version: u32, files: &[&str], attributes: Value) -> String {
    let mut m = Map::new();
    m.insert("scope".to_owned(), json!(scope));
    m.insert("version".to_owned(), json!(version));
    m.insert("restricted".to_owned(), json!(false));
    m.insert("overridable".to_owned(), json!(true));
    m.insert("files".to_owned(), json!(files));
    m.insert("attributes".to_owned(), attributes);
    gson::to_string(&Value::Object(m), false)
}

fn modified() -> Value {
    json!({"lastModification": {"actor": "designer", "timestamp": "2026-01-15T08:00:00Z"}})
}

pub fn view_manifest() -> String {
    manifest("G", 1, &["view.json"], modified())
}

pub fn projects() -> Vec<File> {
    vec![
        file(
            format!("projects/{BASE}/project.json"),
            gson::to_string(
                &json!({"title": "Bench Base", "description": "Shared library", "parent": "", "enabled": true, "inheritable": true}),
                false,
            ),
        ),
        file(
            format!("projects/{HMI}/project.json"),
            gson::to_string(
                &json!({"title": "Bench HMI", "description": "Operator screens", "parent": BASE, "enabled": true, "inheritable": false}),
                false,
            ),
        ),
    ]
}

/// One script library module (`code.py` and manifest).
pub fn library_module(w: &World, index: usize) -> (Vec<File>, usize) {
    let m = &w.modules[index];
    let mut rng = w.rng("library", index as u64);
    let dotted = m.path.join(".");
    let mut out = String::new();
    out.push_str(&format!("\"\"\"Helpers for {}.\"\"\"\n", m.path.join(" ")));
    out.push_str(&format!("LOGGER = system.util.getLogger(\"{dotted}\")\n"));
    out.push_str(&format!("DEFAULT_TIMEOUT = {}\n", 5 * rng.range(1, 12)));
    let mut lines = 3;
    for f in &m.functions {
        out.push_str(&format!("\n\ndef {}({}):\n", f.name, f.params.join(", ")));
        out.push_str(&format!(
            "\t\"\"\"{} {}.\"\"\"\n",
            f.name,
            f.params.join(", ")
        ));
        let params: Vec<&str> = f.params.iter().map(String::as_str).collect();
        let ctx = ScriptCtx {
            has_self: false,
            params: &params,
            view_params: &[],
            view_custom: &[],
            siblings: &[],
            returns: true,
            logger: &dotted,
        };
        let n = rng.lognormal(2.4, 0.6, 2, 80);
        let defect = rng.chance(w.cfg.defects / 4.0);
        let body = script::body(w, &mut rng, &ctx, n, 1, defect);
        lines += 4 + body.text.lines().count();
        out.push_str(&body.text);
        out.push('\n');
    }
    let dir = format!(
        "projects/{BASE}/ignition/script-python/{}",
        m.path.join("/")
    );
    (
        vec![
            file(format!("{dir}/code.py"), out),
            file(
                format!("{dir}/resource.json"),
                manifest("A", 1, &["code.py"], json!({"hintScope": 2})),
            ),
        ],
        lines,
    )
}

pub fn named_queries(w: &World) -> Vec<File> {
    let mut files = Vec::new();
    for q in &w.queries {
        let mut sql = String::new();
        sql.push_str(&format!("-- {}\n", q.path));
        match q.kind {
            "UpdateQuery" => {
                sql.push_str("UPDATE line_state\nSET state = 1, updated_at = CURRENT_TIMESTAMP\nWHERE 1 = 1");
            }
            "ScalarQuery" => sql.push_str("SELECT COUNT(*)\nFROM production_events\nWHERE 1 = 1"),
            _ => sql.push_str("SELECT id, line_id, state, quantity, created_at\nFROM production_events\nWHERE 1 = 1"),
        }
        for (p, _) in &q.params {
            sql.push_str(&format!("\n  AND {} = :{p}", names::lower_first(p)));
        }
        let params: Vec<Value> = q
            .params
            .iter()
            .map(|(p, t)| json!({"type": "Parameter", "identifier": p, "sqlType": t}))
            .collect();
        let attrs = json!({
            "cacheAmount": 1, "cacheEnabled": false, "database": "", "useMaxReturnSize": false,
            "fallbackEnabled": false, "autoBatchEnabled": false, "permissions": [{}],
            "maxReturnSize": 100, "cacheUnit": "SEC", "type": q.kind, "parameters": params, "enabled": true
        });
        let dir = format!("projects/{BASE}/ignition/named-query/{}", q.path);
        files.push(file(format!("{dir}/query.sql"), sql));
        files.push(file(
            format!("{dir}/resource.json"),
            manifest("A", 2, &["query.sql"], attrs),
        ));
    }
    files
}

pub fn style_classes(w: &World) -> Vec<File> {
    let mut files = Vec::new();
    for (i, s) in w.style_classes.iter().enumerate() {
        let mut rng = w.rng("style", i as u64);
        let mut style = Map::new();
        style.insert(
            "color".to_owned(),
            json!(format!("#{:06X}", rng.below(0xFFFFFF))),
        );
        if rng.chance(0.5) {
            style.insert("fontWeight".to_owned(), json!("bold"));
        }
        if rng.chance(0.4) {
            style.insert(
                "borderRadius".to_owned(),
                json!(format!("{}px", rng.range(2, 12))),
            );
        }
        let mut v = json!({"base": {"style": style}});
        if rng.chance(0.3) {
            v["variants"] = json!([{"pseudo": "hover", "style": {"opacity": 0.8}}]);
        }
        let dir = format!("projects/{BASE}/{PERSPECTIVE}/style-classes/{s}");
        files.push(file(format!("{dir}/style.json"), gson::to_string(&v, true)));
        files.push(file(
            format!("{dir}/resource.json"),
            manifest("G", 1, &["style.json"], modified()),
        ));
    }
    files
}

pub fn hmi_resources(w: &World) -> Vec<File> {
    let mut files = Vec::new();
    let mut pages = Map::new();
    for p in &w.pages {
        pages.insert(
            p.url.clone(),
            json!({"title": w.views[p.view].path.rsplit('/').next().unwrap_or(""), "viewPath": w.views[p.view].path}),
        );
    }
    let docks: Vec<Value> = w
        .docks
        .iter()
        .take(2)
        .map(|&i| json!({"anchor": "fixed", "autoBreakpoint": 480, "content": "auto", "handle": "hide", "iconUrl": "", "id": names::lower_first(w.views[i].path.rsplit('/').next().unwrap_or("dock")), "modal": false, "resizable": false, "show": "visible", "size": 64, "viewParams": {}, "viewPath": w.views[i].path}))
        .collect();
    let config =
        json!({"pages": pages, "sharedDocks": {"cornerPriority": "top-bottom", "top": docks}});
    let dir = format!("projects/{HMI}/{PERSPECTIVE}/page-config");
    files.push(file(
        format!("{dir}/config.json"),
        gson::to_string(&config, true),
    ));
    files.push(file(
        format!("{dir}/resource.json"),
        manifest("G", 1, &["config.json"], modified()),
    ));

    let mut custom = Map::new();
    let mut pc = Map::new();
    for c in &w.session_custom {
        custom.insert(c.clone(), json!(""));
        pc.insert(format!("custom.{c}"), json!({"persistent": true}));
    }
    let props =
        json!({"custom": custom, "propConfig": pc, "props": {"locale": "en-US", "theme": "light"}});
    let dir = format!("projects/{HMI}/{PERSPECTIVE}/session-props");
    files.push(file(
        format!("{dir}/props.json"),
        gson::to_string(&props, true),
    ));
    files.push(file(
        format!("{dir}/resource.json"),
        manifest("G", 1, &["props.json"], modified()),
    ));

    // Gateway events.
    let events: [(&str, &str, &str, &[&str], Value); 4] = [
        (
            "startup",
            "onStartup.py",
            "onStartup",
            &[],
            json!({"enabled": true}),
        ),
        (
            "timer/refreshCounters",
            "handleTimerEvent.py",
            "handleTimerEvent",
            &[],
            json!({"delay": 10000, "fixedDelay": true, "sharedThread": true, "enabled": true}),
        ),
        (
            "message/rebuildReport",
            "handleMessage.py",
            "handleMessage",
            &["payload"],
            json!({"threadType": "Shared", "enabled": true}),
        ),
        (
            "tag-change/lineState",
            "onTagChange.py",
            "onTagChange",
            &[
                "initialChange",
                "newValue",
                "previousValue",
                "event",
                "executionCount",
            ],
            json!({"paths": ["[default]Plant/Line1/State"], "changeTypes": ["ValueChange"], "enabled": true}),
        ),
    ];
    for (i, (path, fname, func, params, attrs)) in events.into_iter().enumerate() {
        let mut rng = w.rng("gateway-event", i as u64);
        let ctx = ScriptCtx {
            has_self: false,
            params,
            view_params: &[],
            view_custom: &[],
            siblings: &[],
            returns: false,
            logger: "gateway.events",
        };
        let n = rng.range(4, 30);
        let body = script::body(w, &mut rng, &ctx, n, 1, false);
        let text = format!("def {func}({}):\n{}", params.join(", "), body.text);
        let dir = format!("projects/{HMI}/ignition/{path}");
        files.push(file(format!("{dir}/{fname}"), text));
        files.push(file(
            format!("{dir}/resource.json"),
            manifest("G", 1, &[fname], attrs),
        ));
    }
    files
}

fn collection(name: &str, parent: &str, inheritable: bool) -> File {
    file(
        format!("services/gateway/config/resources/{name}/config-mode.json"),
        gson::to_string(
            &json!({"title": name, "description": "", "enabled": true, "inheritable": inheritable, "parent": parent}),
            false,
        ),
    )
}

fn folder_manifest(files: &[&str]) -> String {
    manifest("G", 1, files, json!({"config": {}}))
}

fn opc(member: &str, data_type: &str) -> Value {
    json!({
        "valueSource": "opc",
        "opcServer": "Ignition OPC UA Server",
        "dataType": data_type,
        "name": member,
        "tagType": "AtomicTag",
        "opcItemPath": {"bindType": "parameter", "binding": format!("ns=1;s=[{{DeviceName}}]{{BaseAddress}}.{member}")}
    })
}

fn data_type(member: &str) -> &'static str {
    match member {
        "Running" | "Fault" | "Open" | "Closed" => "Boolean",
        "Mode" | "Hours" => "Int4",
        _ => "Float8",
    }
}

pub fn gateway(w: &World) -> Vec<File> {
    let root = "services/gateway/config/resources/core/ignition";
    let mut files = vec![
        collection("external", "system", true),
        collection("core", "external", true),
        collection("production", "core", true),
        collection("local", "production", false),
    ];
    files.push(file(
        format!("{root}/tag-provider/default/config.json"),
        gson::to_string(&json!({"profile": {"allowBackfill": false, "enableTagReferenceStore": true, "type": "STANDARD"}, "settings": {"readOnly": false, "valuePersistence": "Database"}}), true),
    ));
    files.push(file(
        format!("{root}/tag-provider/default/resource.json"),
        manifest(
            "A",
            1,
            &["config.json"],
            json!({"uuid": "00000000-0000-4000-8000-000000000001", "enabled": true}),
        ),
    ));
    files.push(file(
        format!("{root}/tag-group/default/Default/config.json"),
        gson::to_string(
            &json!({"config": {"mode": "Direct", "opcDataMode": "Subscribed", "rate": 1000}}),
            true,
        ),
    ));
    files.push(file(
        format!("{root}/tag-group/default/Default/resource.json"),
        manifest(
            "A",
            1,
            &["config.json"],
            json!({"uuid": "00000000-0000-4000-8000-000000000002", "enabled": true}),
        ),
    ));

    // UDT definitions.
    let types = format!("{root}/tag-type-definition/default");
    files.push(file(
        format!("{types}/unary-resource.json"),
        folder_manifest(&[]),
    ));
    let udts: [(&str, &str, &[&str]); 3] = [
        ("Motors", "Motor", world::MOTOR_MEMBERS),
        ("Valves", "Valve", world::VALVE_MEMBERS),
        ("Pumps", "Pump", world::PUMP_MEMBERS),
    ];
    for (folder, name, members) in udts {
        let mut tags: Vec<Value> = members.iter().map(|m| opc(m, data_type(m))).collect();
        if let Some(Value::Object(first)) = tags.first_mut() {
            first.insert(
                "engHigh".to_owned(),
                json!({"bindType": "parameter", "binding": "{MaxValue}"}),
            );
            first.insert(
                "alarms".to_owned(),
                json!([{"name": "High", "mode": "AboveValue", "priority": "High", "setpointA": {"bindType": "Tag", "value": "[.]../Limits/High"}, "displayPath": "{InstanceName}", "activePipeline": ""}]),
            );
        }
        if let Some(Value::Object(t)) = tags.iter_mut().find(|t| t["dataType"] == "Boolean") {
            t.insert(
                "eventScripts".to_owned(),
                json!([{"eventid": "valueChanged", "script": "\tif not initialChange and currentValue.value:\n\t\tsystem.util.getLogger(\"tags\").info(str(tagPath))"}]),
            );
        }
        tags.push(json!({"name": "Limits", "tagType": "Folder", "tags": [{"name": "High", "tagType": "AtomicTag", "valueSource": "memory", "dataType": "Float8", "value": 100.0}]}));
        let udt = json!([{
            "name": name,
            "tagType": "UdtType",
            "parameters": {
                "DeviceName": {"dataType": "String", "value": ""},
                "BaseAddress": {"dataType": "String", "value": ""},
                "MaxValue": {"dataType": "Integer", "value": 100}
            },
            "tags": tags
        }]);
        files.push(file(
            format!("{types}/{folder}/udts.json"),
            gson::to_string(&udt, false),
        ));
        files.push(file(
            format!("{types}/{folder}/unary-resource.json"),
            folder_manifest(&["udts.json"]),
        ));
    }

    // Tag instances: one folder per line.
    let defs = format!("{root}/tag-definition/default");
    files.push(file(
        format!("{defs}/unary-resource.json"),
        folder_manifest(&[]),
    ));
    files.push(file(
        format!("{defs}/Plant/unary-resource.json"),
        folder_manifest(&[]),
    ));
    for line in 1..=w.lines {
        let mut rng = w.rng("tags", line as u64);
        let atomics: Vec<Value> = world::LINE_TAGS
            .iter()
            .map(|t| {
                let dt = if *t == "Order" { "String" } else { "Int4" };
                json!({"name": t, "tagType": "AtomicTag", "valueSource": "memory", "dataType": dt, "value": if dt == "String" { json!("") } else { json!(0) }})
            })
            .collect();
        let mut instances = Vec::new();
        let mut plc =
            |kind: &str, folder: &str, count: usize, base: usize, rng: &mut crate::rng::Rng| {
                for i in 1..=count {
                    let type_id = if rng.chance(w.cfg.defects / 10.0) {
                        format!("{folder}/{kind}Legacy")
                    } else {
                        format!("{folder}/{kind}")
                    };
                    instances.push(json!({
                    "name": format!("{kind}{i}"),
                    "typeId": type_id,
                    "tagType": "UdtInstance",
                    "parameters": {
                        "DeviceName": {"dataType": "String", "value": format!("PLC{line}")},
                        "BaseAddress": {"dataType": "String", "value": format!("DB{}", base + i)}
                    }
                }));
                }
            };
        plc("Motor", "Motors", world::MOTORS_PER_LINE, 100, &mut rng);
        plc("Valve", "Valves", world::VALVES_PER_LINE, 200, &mut rng);
        plc("Pump", "Pumps", world::PUMPS_PER_LINE, 300, &mut rng);
        let dir = format!("{defs}/Plant/Line{line}");
        files.push(file(
            format!("{dir}/tags.json"),
            gson::to_string(&Value::Array(atomics), false),
        ));
        files.push(file(
            format!("{dir}/udts.json"),
            gson::to_string(&Value::Array(instances), false),
        ));
        files.push(file(
            format!("{dir}/unary-resource.json"),
            folder_manifest(&["tags.json", "udts.json"]),
        ));
    }
    files
}

/// Role counts for the summary.
pub fn roles(w: &World) -> [(Role, usize); 4] {
    let count = |r| w.views.iter().filter(|v| v.role == r).count();
    [
        (Role::Page, count(Role::Page)),
        (Role::Card, count(Role::Card)),
        (Role::Popup, count(Role::Popup)),
        (Role::Dock, count(Role::Dock)),
    ]
}
