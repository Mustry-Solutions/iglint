//! TAG: tags and gateway resources.

pub mod alarm_bind_type;
pub mod invalid_cron;
pub mod tag_change_paths;
pub mod undefined_udt_parameter;
pub mod unknown_history_provider;
pub mod unknown_tag_path;
pub mod unknown_udt_type;

/// A `typeId` as a definition path below `_types_`: `[default]_types_/A/B`
/// and `_types_/A/B` become `A/B`. Returns the provider when the id names
/// one.
pub(crate) fn definition_path(type_id: &str) -> (Option<&str>, &str) {
    let (provider, rest) = match type_id.strip_prefix('[').and_then(|r| r.split_once(']')) {
        Some((p, rest)) => (Some(p), rest),
        None => (None, type_id),
    };
    let rest = rest.trim_start_matches('/');
    let rest = rest
        .strip_prefix("_types_/")
        .or_else(|| rest.strip_prefix("_types_"))
        .unwrap_or(rest);
    (provider, rest.trim_matches('/'))
}

#[cfg(test)]
pub(crate) mod fixtures {
    use crate::rules::test_support::Fixture;

    /// A gateway collection `core` with tag provider `default`.
    pub fn gateway() -> Fixture {
        Fixture::new()
            .file("gw/core/config-mode.json", r#"{"title": "core"}"#)
            .resource(
                "gw/core/ignition/tag-provider/default",
                &[(
                    "config.json",
                    r#"{"profile": {"type": "STANDARD"}, "settings": {}}"#,
                )],
            )
    }

    fn unary(file: &str) -> String {
        format!(
            r#"{{"scope": "G", "version": 1, "restricted": false, "overridable": true,
                "files": [{file}], "attributes": {{"config": {{}}}}}}"#
        )
    }

    /// Adds UDT definitions (`udts.json` of the provider's `_types_` root).
    pub fn udts(f: Fixture, udts: &str) -> Fixture {
        let dir = "gw/core/ignition/tag-type-definition/default";
        f.file(
            &format!("{dir}/unary-resource.json"),
            &unary("\"udts.json\""),
        )
        .file(&format!("{dir}/udts.json"), udts)
    }

    /// Adds tags (`tags.json`) and UDT instances (`udts.json`) at the
    /// provider root.
    pub fn tags(f: Fixture, tags: &str, instances: &str) -> Fixture {
        let dir = "gw/core/ignition/tag-definition/default";
        f.file(
            &format!("{dir}/unary-resource.json"),
            &unary("\"tags.json\", \"udts.json\""),
        )
        .file(&format!("{dir}/tags.json"), tags)
        .file(&format!("{dir}/udts.json"), instances)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn definition_paths() {
        assert_eq!(
            super::definition_path("Motors/Motor"),
            (None, "Motors/Motor")
        );
        assert_eq!(
            super::definition_path("[default]_types_/Motors/Motor"),
            (Some("default"), "Motors/Motor")
        );
        assert_eq!(super::definition_path("_types_/A"), (None, "A"));
    }
}
