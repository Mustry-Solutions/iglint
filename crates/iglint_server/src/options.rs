//! Client settings: `initializationOptions` and the `iglint` section of
//! `workspace/didChangeConfiguration`.

use serde::Deserialize;

/// When files are analysed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunTrigger {
    /// After every change, debounced.
    #[default]
    OnType,
    /// When a file is opened or saved.
    OnSave,
}

/// Which files get diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    /// Open documents only.
    #[default]
    Open,
    /// Every lintable file of the workspace folders.
    Workspace,
}

/// Settings of the server.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ServerOptions {
    pub run: RunTrigger,
    pub scope: Scope,
    /// Quiet time after the last change before analysis starts.
    pub debounce_ms: u64,
    /// Whether "fix all" also applies unsafe fixes. Quick fixes always offer
    /// unsafe fixes, marked as such.
    pub unsafe_fixes: bool,
    /// Link of a rule's documentation; `{code}` and `{name}` are replaced.
    pub rule_docs_url: String,
}

impl Default for ServerOptions {
    fn default() -> Self {
        ServerOptions {
            run: RunTrigger::OnType,
            scope: Scope::Open,
            debounce_ms: 10,
            unsafe_fixes: false,
            rule_docs_url: iglint_linter::registry::RULE_DOCS_URL.to_owned(),
        }
    }
}

impl ServerOptions {
    /// Reads settings from `initializationOptions` or a configuration
    /// change: either the settings object itself or `{"iglint": {...}}`.
    /// Unknown keys are ignored; invalid values keep the current setting.
    pub fn updated(&self, value: &serde_json::Value) -> ServerOptions {
        let value = value.get("iglint").unwrap_or(value);
        let Some(object) = value.as_object() else {
            return self.clone();
        };
        let mut current =
            serde_json::to_value(SerializedOptions::from(self)).unwrap_or(serde_json::Value::Null);
        if let Some(target) = current.as_object_mut() {
            for (key, value) in object {
                if target.contains_key(key) && !value.is_null() {
                    let mut candidate = target.clone();
                    candidate.insert(key.clone(), value.clone());
                    if serde_json::from_value::<ServerOptions>(candidate.clone().into()).is_ok() {
                        *target = candidate;
                    }
                }
            }
        }
        serde_json::from_value(current).unwrap_or_else(|_| self.clone())
    }

    /// The documentation link of a rule.
    pub fn rule_docs_url(&self, code: &str, name: &str) -> String {
        self.rule_docs_url
            .replace("{code}", code)
            .replace("{name}", name)
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SerializedOptions<'a> {
    run: &'static str,
    scope: &'static str,
    debounce_ms: u64,
    unsafe_fixes: bool,
    rule_docs_url: &'a str,
}

impl<'a> From<&'a ServerOptions> for SerializedOptions<'a> {
    fn from(o: &'a ServerOptions) -> Self {
        SerializedOptions {
            run: match o.run {
                RunTrigger::OnType => "onType",
                RunTrigger::OnSave => "onSave",
            },
            scope: match o.scope {
                Scope::Open => "open",
                Scope::Workspace => "workspace",
            },
            debounce_ms: o.debounce_ms,
            unsafe_fixes: o.unsafe_fixes,
            rule_docs_url: &o.rule_docs_url,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_nested_and_flat_settings() {
        let o = ServerOptions::default()
            .updated(&json!({"iglint": {"run": "onSave", "debounceMs": 5}}));
        assert_eq!(o.run, RunTrigger::OnSave);
        assert_eq!(o.debounce_ms, 5);
        let o = o.updated(&json!({"scope": "workspace", "unknown": 1}));
        assert_eq!(o.scope, Scope::Workspace);
        assert_eq!(o.run, RunTrigger::OnSave);
    }

    #[test]
    fn invalid_values_keep_the_current_setting() {
        let o = ServerOptions::default().updated(&json!({"run": "sometimes", "debounceMs": 7}));
        assert_eq!(o.run, RunTrigger::OnType);
        assert_eq!(o.debounce_ms, 7);
        assert_eq!(
            ServerOptions::default().updated(&json!(null)),
            ServerOptions::default()
        );
    }

    #[test]
    fn docs_url_template() {
        let o =
            ServerOptions::default().updated(&json!({"ruleDocsUrl": "https://x/{name}#{code}"}));
        assert_eq!(
            o.rule_docs_url("VIEW001", "invalid-json"),
            "https://x/invalid-json#VIEW001"
        );
    }
}
