//! Gateway configuration resources with a `config.json` (tag providers, tag
//! groups, database connections, historian providers, devices, settings).

use iglint_json::Value;

use crate::ids::FileId;
use crate::kinds::ResourceKind;

/// A gateway config resource.
#[derive(Debug)]
pub struct GatewayResource<'a> {
    pub kind: ResourceKind,
    pub config_file: Option<FileId>,
    pub config: Option<&'a Value<'a>>,
    /// `attributes.enabled` of the manifest.
    pub enabled: Option<bool>,
    pub details: GatewayDetails<'a>,
}

/// Kind-specific settings.
#[derive(Debug)]
pub enum GatewayDetails<'a> {
    Database {
        connect_url: Option<&'a str>,
        driver: Option<&'a str>,
        translator: Option<&'a str>,
        /// `password.type`: `Embedded` or `Referenced`.
        password_type: Option<&'a str>,
    },
    TagProvider {
        /// `profile.type`: `STANDARD`, `MANAGED`, `REMOTE`, ...
        provider_type: Option<&'a str>,
    },
    TagGroup {
        mode: Option<&'a str>,
        rate: Option<&'a Value<'a>>,
    },
    None,
}

pub(crate) fn build_gateway_resource<'a>(
    kind: ResourceKind,
    enabled: Option<bool>,
    config_file: Option<FileId>,
    config: Option<&'a Value<'a>>,
) -> GatewayResource<'a> {
    let get = |path: &[&str]| -> Option<&'a Value<'a>> {
        let mut v = config?;
        for k in path {
            v = v.get(k)?;
        }
        Some(v)
    };
    let s = |path: &[&str]| get(path).and_then(Value::as_str);
    let details = match kind {
        ResourceKind::DatabaseConnection => GatewayDetails::Database {
            connect_url: s(&["connectURL"]),
            driver: s(&["driver"]),
            translator: s(&["translator"]),
            password_type: s(&["password", "type"]),
        },
        ResourceKind::TagProvider => GatewayDetails::TagProvider {
            provider_type: s(&["profile", "type"]),
        },
        ResourceKind::TagGroup => GatewayDetails::TagGroup {
            mode: s(&["config", "mode"]),
            rate: get(&["config", "rate"]),
        },
        _ => GatewayDetails::None,
    };
    GatewayResource {
        kind,
        config_file,
        config,
        enabled,
        details,
    }
}
