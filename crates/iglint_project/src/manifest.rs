//! `resource.json`, `unary-resource.json`, `project.json` and
//! `config-mode.json`.

use iglint_json::Value;
use iglint_text::TextRange;

use crate::document::JsonDocument;
use crate::ids::FileId;

/// An Ignition application scope set as written in `resource.json`
/// (`G`, `D`, `C`, combinations, `A`, `N` or empty).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct AppScope(u8);

impl AppScope {
    /// No scope (`N` or empty).
    pub const NONE: AppScope = AppScope(0);
    /// Gateway (`G`).
    pub const GATEWAY: AppScope = AppScope(1);
    /// Designer (`D`).
    pub const DESIGNER: AppScope = AppScope(2);
    /// Vision client (`C`).
    pub const CLIENT: AppScope = AppScope(4);
    /// All scopes (`A`).
    pub const ALL: AppScope = AppScope(7);

    /// Parses a scope code the way `ApplicationScope.parseScope` does: `A`
    /// is all, `N` or empty is none, otherwise any combination of `G`, `D`
    /// and `C`. `None` for any other character.
    pub fn parse(code: &str) -> Option<AppScope> {
        let mut bits = 0;
        for c in code.chars() {
            bits |= match c {
                'A' => 7,
                'N' => 0,
                'G' => 1,
                'D' => 2,
                'C' => 4,
                _ => return None,
            };
        }
        Some(AppScope(bits))
    }

    /// Whether every scope of `other` is in `self`.
    pub fn contains(self, other: AppScope) -> bool {
        self.0 & other.0 == other.0
    }

    /// The bit set: gateway 1, designer 2, client 4.
    pub fn bits(self) -> u8 {
        self.0
    }
}

/// The interesting keys of a resource manifest, extracted once during
/// discovery. Raw access goes through [`Manifest::doc`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ManifestInfo {
    /// `scope` as written.
    pub scope: Option<String>,
    pub version: Option<i64>,
    pub restricted: Option<bool>,
    /// `overridable`; absent means overridable.
    pub overridable: Option<bool>,
    /// `files`; `None` when the key is absent or not an array of strings.
    pub files: Option<Vec<String>>,
    pub documentation: Option<String>,
    pub description: Option<String>,
}

/// A parsed `resource.json` or `unary-resource.json`.
#[derive(Debug)]
pub struct Manifest {
    pub file: FileId,
    /// True for `unary-resource.json`.
    pub unary: bool,
    pub doc: JsonDocument,
    pub info: ManifestInfo,
    /// Invalid UTF-8 sequences in the file (see
    /// [`crate::workspace::read_text`]).
    pub invalid_utf8: Vec<TextRange>,
    /// The resource directory has no manifest (see
    /// [`crate::workspace::Resource`]): `file` names the file that should
    /// exist, `doc` is an empty object. The gateway does not load such a
    /// resource.
    pub missing: bool,
}

impl Manifest {
    pub(crate) fn new(file: FileId, unary: bool, doc: JsonDocument) -> Self {
        let info = doc.root().map(extract_info).unwrap_or_default();
        Manifest {
            file,
            unary,
            doc,
            info,
            invalid_utf8: Vec::new(),
            missing: false,
        }
    }

    /// Whether a child project or collection may override this resource.
    pub fn overridable(&self) -> bool {
        self.info.overridable.unwrap_or(true)
    }

    /// The parsed `attributes` object.
    pub fn attributes(&self) -> Option<&Value<'_>> {
        self.doc.root()?.get("attributes")
    }

    /// `attributes.enabled`, if present.
    pub fn enabled(&self) -> Option<bool> {
        self.attributes()?.get("enabled")?.as_bool()
    }

    /// True when the manifest lists `name` in `files`, or has no usable
    /// `files` list (then the gateway behaviour is unknown and every file is
    /// treated as listed).
    pub fn lists(&self, name: &str) -> bool {
        match &self.info.files {
            Some(files) => files.iter().any(|f| f == name),
            None => true,
        }
    }
}

fn extract_info(root: &Value<'_>) -> ManifestInfo {
    let string = |key: &str| root.get(key).and_then(Value::as_str).map(str::to_owned);
    ManifestInfo {
        scope: string("scope"),
        version: root.get("version").and_then(Value::as_i64),
        restricted: root.get("restricted").and_then(Value::as_bool),
        overridable: root.get("overridable").and_then(Value::as_bool),
        files: root.get("files").and_then(Value::as_array).and_then(|a| {
            a.iter()
                .map(|v| v.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
        }),
        documentation: string("documentation"),
        description: string("description"),
    }
}

/// A parsed `project.json` or `config-mode.json`. Both have the same shape.
#[derive(Debug)]
pub struct ContainerManifest {
    pub file: FileId,
    pub doc: JsonDocument,
    pub title: Option<String>,
    pub description: Option<String>,
    /// `parent`, `None` when absent or empty.
    pub parent: Option<String>,
    pub enabled: Option<bool>,
    /// `inheritable`; absent means `false`.
    pub inheritable: bool,
    /// Invalid UTF-8 sequences in the file (see
    /// [`crate::workspace::read_text`]).
    pub invalid_utf8: Vec<TextRange>,
}

impl ContainerManifest {
    pub(crate) fn new(file: FileId, doc: JsonDocument) -> Self {
        let (title, description, parent, enabled, inheritable) = match doc.root() {
            Some(root) => {
                let string = |key: &str| root.get(key).and_then(Value::as_str).map(str::to_owned);
                (
                    string("title"),
                    string("description"),
                    string("parent").filter(|p| !p.is_empty()),
                    root.get("enabled").and_then(Value::as_bool),
                    root.get("inheritable")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                )
            }
            None => (None, None, None, None, false),
        };
        ContainerManifest {
            file,
            doc,
            title,
            description,
            parent,
            enabled,
            inheritable,
            invalid_utf8: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_codes() {
        assert_eq!(AppScope::parse("G"), Some(AppScope::GATEWAY));
        assert_eq!(AppScope::parse("A"), Some(AppScope::ALL));
        assert_eq!(AppScope::parse(""), Some(AppScope::NONE));
        assert_eq!(AppScope::parse("N"), Some(AppScope::NONE));
        assert!(AppScope::parse("CG").unwrap().contains(AppScope::CLIENT));
        assert_eq!(AppScope::parse("X"), None);
    }
}
