//! Loading a resource's data files and building its typed model.

use std::sync::Arc;

use iglint_json::Value;
use iglint_text::TextRange;

use crate::document::JsonDocument;
use crate::event_resource::{EventScriptResource, WebDevMethod, WebDevResource, event_settings};
use crate::expression::EmbeddedExpression;
use crate::gateway::{GatewayResource, build_gateway_resource};
use crate::ids::{FileId, ResourceId};
use crate::kinds::{EventScriptKind, FileKind, HttpMethod, ResourceKind};
use crate::library::LibraryModule;
use crate::named_query::{NamedQuery, build_named_query};
use crate::perspective::{PageConfig, StyleClass, build_page_config, build_style_class};
use crate::script::{EmbeddedScript, ScriptFile, library_script};
use crate::tags::{TagBuilder, TagFolder};
use crate::view::{ModelProblem, SessionProps, View, build_session_props, build_view};
use crate::workspace::{Resource, Workspace, read_text};

/// A data file of a resource, read and (for JSON files) parsed.
#[derive(Debug)]
pub struct LoadedFile {
    pub file: FileId,
    pub kind: FileKind,
    /// The text; `None` for binary files and files that could not be read.
    pub text: Option<Arc<str>>,
    /// The parse tree of JSON files.
    pub json: Option<JsonDocument>,
    /// Why the file could not be read.
    pub error: Option<String>,
}

impl LoadedFile {
    /// The text of a text file (from the JSON document for JSON files).
    pub fn text(&self) -> Option<&str> {
        match &self.json {
            Some(doc) => Some(doc.text()),
            None => self.text.as_deref(),
        }
    }
}

/// The data files of one resource (the manifest is already parsed in the
/// workspace).
#[derive(Debug)]
pub struct LoadedResource {
    pub resource: ResourceId,
    pub files: Vec<LoadedFile>,
}

impl LoadedResource {
    /// A loaded file by id.
    pub fn file(&self, id: FileId) -> Option<&LoadedFile> {
        self.files.iter().find(|f| f.file == id)
    }

    fn by_name<'s>(&'s self, ws: &Workspace, name: &str) -> Option<&'s LoadedFile> {
        self.files.iter().find(|f| ws.file(f.file).name() == name)
    }
}

impl Workspace {
    /// Reads every text data file of a resource once. JSON files are parsed.
    pub fn load_resource(&self, id: ResourceId) -> LoadedResource {
        let files = self
            .data_files(id)
            .filter(|f| f.kind.is_text())
            .map(|f| {
                let (text, error) = match read_text(&f.path) {
                    Ok(t) => (Some(t), None),
                    Err(e) => (None, Some(e)),
                };
                let json = match (&text, f.kind.is_json()) {
                    (Some(t), true) => Some(JsonDocument::parse(t.clone())),
                    _ => None,
                };
                LoadedFile {
                    file: f.id,
                    kind: f.kind,
                    text: if json.is_some() { None } else { text },
                    json,
                    error,
                }
            })
            .collect();
        LoadedResource {
            resource: id,
            files,
        }
    }
}

/// An error that prevents (part of) a resource from being modelled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileError {
    pub file: FileId,
    /// Range of the JSON syntax error; empty for I/O errors.
    pub range: TextRange,
    pub message: String,
}

/// The model of one resource.
#[derive(Debug)]
pub struct ResourceModel<'a> {
    pub resource: &'a Resource,
    pub data: ModelData<'a>,
    /// Unreadable files and JSON syntax errors.
    pub errors: Vec<FileError>,
}

/// Typed resource content.
#[derive(Debug)]
pub enum ModelData<'a> {
    View {
        file: FileId,
        view: View<'a>,
    },
    StyleClass(StyleClass<'a>),
    PageConfig(PageConfig<'a>),
    SessionProps {
        file: FileId,
        props: SessionProps<'a>,
    },
    Stylesheet {
        file: FileId,
        text: &'a str,
    },
    /// A gateway Perspective theme (or other asset) with its CSS files.
    Theme {
        css: Vec<(FileId, &'a str)>,
    },
    NamedQuery(NamedQuery<'a>),
    EventScript(EventScriptResource<'a>),
    WebDev(WebDevResource<'a>),
    Library(LibraryModule<'a>),
    Tags(TagFolder<'a>),
    Gateway(GatewayResource<'a>),
    /// Nothing to model (identified-only kinds, missing or invalid main
    /// file).
    None,
}

impl<'a> ResourceModel<'a> {
    /// Every script of the resource with the file it lives in. For `.py`
    /// event files this is the runtime form ([`ScriptFile::runtime`]),
    /// which maps back to the file exactly.
    pub fn scripts(&self) -> Vec<(FileId, &EmbeddedScript<'a>)> {
        match &self.data {
            ModelData::View { file, view } => view.scripts.iter().map(|s| (*file, s)).collect(),
            ModelData::SessionProps { file, props } => {
                props.scripts.iter().map(|s| (*file, s)).collect()
            }
            ModelData::EventScript(e) => e.script.iter().map(|s| (s.file, &s.runtime)).collect(),
            ModelData::WebDev(w) => w
                .methods
                .iter()
                .filter_map(|m| m.script.as_ref())
                .map(|s| (s.file, &s.runtime))
                .collect(),
            ModelData::Library(l) => vec![(l.file, &l.script)],
            ModelData::Tags(t) => t
                .scripts
                .iter()
                .map(|s| {
                    let file = match s.owner {
                        crate::script::Owner::Tag(id) => t.tag(id).file,
                        _ => t.files.first().map_or(FileId(u32::MAX), |f| f.0),
                    };
                    (file, s)
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Every expression of the resource with the file it lives in.
    pub fn expressions(&self) -> Vec<(FileId, &EmbeddedExpression<'a>)> {
        match &self.data {
            ModelData::View { file, view } => view.expressions.iter().map(|e| (*file, e)).collect(),
            ModelData::SessionProps { file, props } => {
                props.expressions.iter().map(|e| (*file, e)).collect()
            }
            ModelData::Tags(t) => t
                .expressions
                .iter()
                .map(|e| {
                    let file = match e.owner {
                        crate::script::Owner::Tag(id) => t.tag(id).file,
                        _ => FileId(u32::MAX),
                    };
                    (file, e)
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Structural problems found while modelling.
    pub fn problems(&self) -> &[ModelProblem] {
        match &self.data {
            ModelData::View { view, .. } => &view.problems,
            ModelData::SessionProps { props, .. } => &props.problems,
            ModelData::StyleClass(s) => &s.problems,
            ModelData::PageConfig(p) => &p.problems,
            ModelData::Tags(t) => &t.problems,
            _ => &[],
        }
    }

    /// The view, for view resources.
    pub fn view(&self) -> Option<&View<'a>> {
        match &self.data {
            ModelData::View { view, .. } => Some(view),
            _ => None,
        }
    }
}

/// The node path prefix of a resource: its path, or its type id for
/// singletons.
pub(crate) fn display_path(r: &Resource) -> &str {
    if r.path.is_empty() {
        &r.type_id
    } else {
        &r.path
    }
}

/// Builds the typed model of a loaded resource.
pub fn build_model<'a>(ws: &'a Workspace, loaded: &'a LoadedResource) -> ResourceModel<'a> {
    let resource = ws.resource(loaded.resource);
    let mut errors = Vec::new();
    for f in &loaded.files {
        if let Some(e) = &f.error {
            errors.push(FileError {
                file: f.file,
                range: TextRange::default(),
                message: e.clone(),
            });
        }
        if let Some(e) = f.json.as_ref().and_then(JsonDocument::error) {
            errors.push(FileError {
                file: f.file,
                range: e.range,
                message: e.to_string(),
            });
        }
    }
    let json = |name: &str| -> Option<(FileId, &'a Value<'a>)> {
        let f = loaded.by_name(ws, name)?;
        Some((f.file, f.json.as_ref()?.root()?))
    };
    let text = |name: &str| -> Option<(FileId, &'a str)> {
        let f = loaded.by_name(ws, name)?;
        Some((f.file, f.text()?))
    };
    let attributes = resource.manifest.attributes();
    let enabled = resource.manifest.enabled();
    let data = match resource.kind {
        ResourceKind::View => match json("view.json") {
            Some((file, root)) => ModelData::View {
                file,
                view: build_view(root),
            },
            None => ModelData::None,
        },
        ResourceKind::StyleClass => match json("style.json") {
            Some((file, root)) => ModelData::StyleClass(build_style_class(file, root, false)),
            None => match json("data.bin") {
                Some((file, root)) => ModelData::StyleClass(build_style_class(file, root, true)),
                None => ModelData::None,
            },
        },
        ResourceKind::PageConfig => match json("config.json") {
            Some((file, root)) => ModelData::PageConfig(build_page_config(file, root)),
            None => ModelData::None,
        },
        ResourceKind::SessionProps => match json("props.json") {
            Some((file, root)) => ModelData::SessionProps {
                file,
                props: build_session_props(root),
            },
            None => ModelData::None,
        },
        ResourceKind::Stylesheet => match text("stylesheet.css") {
            Some((file, text)) => ModelData::Stylesheet { file, text },
            None => ModelData::None,
        },
        ResourceKind::PerspectiveAsset => ModelData::Theme {
            css: loaded
                .files
                .iter()
                .filter(|f| f.kind == FileKind::ThemeCss)
                .filter_map(|f| Some((f.file, f.text()?)))
                .collect(),
        },
        ResourceKind::NamedQuery => {
            let sql = text("query.sql");
            ModelData::NamedQuery(build_named_query(
                attributes,
                resource
                    .manifest
                    .doc
                    .root()
                    .and_then(|r| r.get("documentation"))
                    .and_then(Value::as_str),
                sql.map(|s| s.0),
                sql.map(|s| s.1),
            ))
        }
        ResourceKind::EventScript(kind) => {
            let script = text(&kind.file_name()).map(|(file, t)| {
                ScriptFile::new(
                    file,
                    kind,
                    t,
                    display_path(resource).to_owned(),
                    enabled != Some(false),
                )
            });
            ModelData::EventScript(EventScriptResource {
                kind,
                attributes,
                enabled,
                script,
                settings: event_settings(kind, attributes),
            })
        }
        ResourceKind::WebDev => {
            let config = json("config.json").map(|c| c.1);
            let mut methods = Vec::new();
            for m in HttpMethod::ALL {
                let mconfig = config.and_then(|c| c.get(m.function_name()));
                let menabled = mconfig
                    .and_then(|c| c.get("enabled"))
                    .and_then(Value::as_bool);
                let script = text(m.file_name()).map(|(file, t)| {
                    ScriptFile::new(
                        file,
                        EventScriptKind::WebDev(m),
                        t,
                        format!("{}@{}", display_path(resource), m.function_name()),
                        menabled != Some(false),
                    )
                });
                if mconfig.is_none() && script.is_none() {
                    continue;
                }
                methods.push(WebDevMethod {
                    method: m,
                    config: mconfig,
                    enabled: menabled,
                    require_auth: mconfig
                        .and_then(|c| c.get("require-auth"))
                        .and_then(Value::as_bool),
                    require_https: mconfig
                        .and_then(|c| c.get("require-https"))
                        .and_then(Value::as_bool),
                    required_roles: mconfig
                        .and_then(|c| c.get("required-roles"))
                        .and_then(Value::as_str),
                    script,
                });
            }
            ModelData::WebDev(WebDevResource {
                config,
                resource_type: config
                    .and_then(|c| c.get("resource-type"))
                    .and_then(Value::as_str),
                methods,
            })
        }
        ResourceKind::ScriptLibrary => match text("code.py") {
            Some((file, t)) => {
                let module_path = resource.path.replace('/', ".");
                ModelData::Library(LibraryModule {
                    script: library_script(t, module_path.clone()),
                    module_path,
                    file,
                    hint_scope: attributes
                        .and_then(|a| a.get("hintScope"))
                        .and_then(Value::as_i64),
                })
            }
            None => ModelData::None,
        },
        ResourceKind::TagDefinition | ResourceKind::TagTypeDefinition => {
            let (provider, folder) = resource
                .path
                .split_once('/')
                .unwrap_or((&resource.path, ""));
            let mut b = TagBuilder::new(
                provider,
                folder,
                resource.kind == ResourceKind::TagTypeDefinition,
                attributes.and_then(|a| a.get("config")),
            );
            for f in &loaded.files {
                if !matches!(f.kind, FileKind::TagsJson | FileKind::UdtsJson { .. }) {
                    continue;
                }
                if let Some(root) = f.json.as_ref().and_then(JsonDocument::root) {
                    let listed = ws.file(f.file).listed != Some(false);
                    b.add_file(f.file, root, listed);
                }
            }
            ModelData::Tags(b.folder)
        }
        ResourceKind::TagProvider
        | ResourceKind::TagGroup
        | ResourceKind::DatabaseConnection
        | ResourceKind::HistorianProvider
        | ResourceKind::OpcDevice
        | ResourceKind::AlarmNotificationProfile
        | ResourceKind::GatewaySetting => {
            let config = json("config.json");
            ModelData::Gateway(build_gateway_resource(
                resource.kind,
                enabled,
                config.map(|c| c.0),
                config.map(|c| c.1),
            ))
        }
        _ => ModelData::None,
    };
    ResourceModel {
        resource,
        data,
        errors,
    }
}
