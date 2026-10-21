//! File-based event script resources: gateway events (timer, scheduled, tag
//! change, message, startup, shutdown, update), Perspective session events
//! and Vision client events (format spec 2 and 5.1), and WebDev resources
//! (format spec 10).

use iglint_expression::{TagPath, parse_tag_path};
use iglint_json::{JsonString, Value};
use iglint_text::TextSize;

use crate::kinds::{EventScriptKind, HttpMethod};
use crate::script::ScriptFile;

/// An event script resource.
#[derive(Debug)]
pub struct EventScriptResource<'a> {
    pub kind: EventScriptKind,
    /// `attributes` of the manifest.
    pub attributes: Option<&'a Value<'a>>,
    /// `attributes.enabled`; absent means enabled.
    pub enabled: Option<bool>,
    /// The script file, when present.
    pub script: Option<ScriptFile<'a>>,
    pub settings: EventSettings<'a>,
}

/// Kind-specific settings from the manifest attributes.
#[derive(Debug)]
pub enum EventSettings<'a> {
    Timer {
        /// `delay` in milliseconds.
        delay: Option<&'a Value<'a>>,
        /// `true`: fixed delay, `false`: fixed rate.
        fixed_delay: Option<bool>,
        shared_thread: Option<bool>,
    },
    Scheduled {
        cron_expression: Option<&'a JsonString<'a>>,
    },
    TagChange {
        /// Each path with its parse.
        paths: Vec<(&'a JsonString<'a>, TagPath)>,
        /// The `paths` value (to report an empty or missing list).
        paths_value: Option<&'a Value<'a>>,
        change_types: Vec<&'a JsonString<'a>>,
    },
    Message {
        /// `Shared` or `Dedicated` (clients also `EDT`).
        thread_type: Option<&'a JsonString<'a>>,
        permission: Option<&'a Value<'a>>,
    },
    KeyEvent {
        event_mode: Option<&'a str>,
        event_bound: Option<&'a str>,
        key: Option<&'a str>,
        code: Option<&'a str>,
    },
    None,
}

pub(crate) fn event_settings<'a>(
    kind: EventScriptKind,
    attributes: Option<&'a Value<'a>>,
) -> EventSettings<'a> {
    let attr = |k: &str| attributes.and_then(|a| a.get(k));
    match kind {
        EventScriptKind::GatewayTimer | EventScriptKind::ClientTimer => EventSettings::Timer {
            delay: attr("delay"),
            fixed_delay: attr("fixedDelay").and_then(Value::as_bool),
            shared_thread: attr("sharedThread").and_then(Value::as_bool),
        },
        EventScriptKind::GatewayScheduled => EventSettings::Scheduled {
            cron_expression: attr("cronExpression").and_then(Value::as_json_string),
        },
        EventScriptKind::GatewayTagChange | EventScriptKind::ClientTagChange => {
            let paths_value = attr("paths");
            let paths = paths_value
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_json_string)
                        .map(|s| (s, parse_tag_path(s.value(), TextSize::ZERO)))
                        .collect()
                })
                .unwrap_or_default();
            EventSettings::TagChange {
                paths,
                paths_value,
                change_types: attr("changeTypes")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_json_string).collect())
                    .unwrap_or_default(),
            }
        }
        EventScriptKind::GatewayMessage
        | EventScriptKind::SessionMessage
        | EventScriptKind::ClientMessage => EventSettings::Message {
            thread_type: attr("threadType").and_then(Value::as_json_string),
            permission: attr("permission"),
        },
        EventScriptKind::KeyEvent => EventSettings::KeyEvent {
            event_mode: attr("eventMode").and_then(Value::as_str),
            event_bound: attr("eventBound").and_then(Value::as_str),
            key: attr("eventBoundKey").and_then(Value::as_str),
            code: attr("eventBoundCode").and_then(Value::as_str),
        },
        _ => EventSettings::None,
    }
}

/// A WebDev resource (`com.inductiveautomation.webdev/resources/<path>`).
#[derive(Debug)]
pub struct WebDevResource<'a> {
    /// `config.json`.
    pub config: Option<&'a Value<'a>>,
    /// `resource-type`: `python-resource`, `text-resource`, `file-resource`,
    /// `mounted-folder-resource`.
    pub resource_type: Option<&'a str>,
    /// One entry per method that has a config entry or a script file.
    pub methods: Vec<WebDevMethod<'a>>,
}

/// One HTTP method of a python resource.
#[derive(Debug)]
pub struct WebDevMethod<'a> {
    pub method: HttpMethod,
    /// `config.json` `do<Method>` object.
    pub config: Option<&'a Value<'a>>,
    pub enabled: Option<bool>,
    pub require_auth: Option<bool>,
    pub require_https: Option<bool>,
    pub required_roles: Option<&'a str>,
    pub script: Option<ScriptFile<'a>>,
}
