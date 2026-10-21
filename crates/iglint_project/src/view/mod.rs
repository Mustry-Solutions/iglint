//! The Perspective view model (format spec 6) and the property model shared
//! with session properties (7.3).
//!
//! Every element keeps references into the parsed `view.json` (`&'a Value`),
//! so rules can reach raw JSON and exact byte ranges, plus an iglint node
//! path and an ign-lint node path (see [`crate::node_path`]).

mod binding;
mod build;
mod resolve;

use iglint_expression::{PropertyRef, TagPath};
use iglint_json::{JsonString, Value};
use iglint_text::TextRange;

use crate::expression::EmbeddedExpression;
use crate::ids::{BindingId, ComponentId, ExprId, PropConfigId, ScriptId};
use crate::script::{EmbeddedScript, Owner};

pub use binding::*;
pub(crate) use build::build_session_props;
pub use build::build_view;
pub use resolve::{RefResolution, RefTarget, ResolvedRef, ScopeRef, Unresolved};

/// A structural problem found while modelling (wrong JSON types, missing
/// names). The model skips the broken part and keeps going.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelProblem {
    pub range: TextRange,
    pub message: String,
    pub node_path: String,
}

/// A parsed `view.json`.
#[derive(Debug)]
pub struct View<'a> {
    /// The whole document.
    pub root: &'a Value<'a>,
    /// View-level `params`, `custom`, `props`, `propConfig`, `events`,
    /// `permissions` objects, when present.
    pub params: Option<&'a Value<'a>>,
    pub custom: Option<&'a Value<'a>>,
    pub props: Option<&'a Value<'a>>,
    pub prop_config: Option<&'a Value<'a>>,
    pub events_value: Option<&'a Value<'a>>,
    pub permissions: Option<&'a Value<'a>>,
    /// Components in document order; `ComponentId(0)` is the root container.
    pub components: Vec<Component<'a>>,
    /// Custom properties and view parameters (view and components).
    pub properties: Vec<PropertyDef<'a>>,
    /// Every `propConfig` entry (view and components).
    pub prop_configs: Vec<PropConfig<'a>>,
    pub bindings: Vec<Binding<'a>>,
    /// Event handlers (view and components).
    pub events: Vec<EventHandler<'a>>,
    pub message_handlers: Vec<MessageHandler<'a>>,
    pub custom_methods: Vec<CustomMethod<'a>>,
    pub extension_functions: Vec<ExtensionFunctionDef<'a>>,
    /// Every Python location of the view.
    pub scripts: Vec<EmbeddedScript<'a>>,
    /// Every expression location of the view.
    pub expressions: Vec<EmbeddedExpression<'a>>,
    /// Literal references to other views.
    pub view_refs: Vec<ViewRef<'a>>,
    /// Literal style class references.
    pub style_refs: Vec<StyleClassRef<'a>>,
    pub problems: Vec<ModelProblem>,
}

impl<'a> View<'a> {
    /// A component by id.
    pub fn component(&self, id: ComponentId) -> &Component<'a> {
        &self.components[id.index()]
    }

    /// The root container, if the view has one.
    pub fn root_component(&self) -> Option<&Component<'a>> {
        self.components.first()
    }

    /// A script by id.
    pub fn script(&self, id: ScriptId) -> &EmbeddedScript<'a> {
        &self.scripts[id.index()]
    }

    /// An expression by id.
    pub fn expression(&self, id: ExprId) -> &EmbeddedExpression<'a> {
        &self.expressions[id.index()]
    }

    /// A binding by id.
    pub fn binding(&self, id: BindingId) -> &Binding<'a> {
        &self.bindings[id.index()]
    }

    /// A `propConfig` entry by id.
    pub fn prop_config_entry(&self, id: PropConfigId) -> &PropConfig<'a> {
        &self.prop_configs[id.index()]
    }

    /// View parameters (names in `params` and `propConfig.params.*`).
    pub fn params(&self) -> impl Iterator<Item = &PropertyDef<'a>> {
        self.properties
            .iter()
            .filter(|p| p.owner == Owner::View && p.category == PropCategory::Params)
    }

    /// A custom property or parameter definition.
    pub fn property(
        &self,
        owner: Owner,
        category: PropCategory,
        name: &str,
    ) -> Option<&PropertyDef<'a>> {
        self.properties
            .iter()
            .find(|p| p.owner == owner && p.category == category && p.name == name)
    }

    /// The `propConfig` entry of `owner` for exactly `path` (e.g. `props.text`).
    pub fn prop_config_for(&self, owner: Owner, path: &str) -> Option<&PropConfig<'a>> {
        self.prop_configs
            .iter()
            .rev()
            .find(|c| c.owner == owner && c.key.value() == path)
    }

    /// True when `path` of `owner`, or a property containing it, is bound:
    /// the literal value in the file is only the Designer's cached result.
    pub fn is_bound(&self, owner: Owner, path: &str) -> bool {
        self.prop_configs
            .iter()
            .any(|c| c.owner == owner && c.binding.is_some() && path_covers(c.key.value(), path))
    }

    /// True when some property inside `path` of `owner` is bound.
    pub fn has_binding_within(&self, owner: Owner, path: &str) -> bool {
        self.prop_configs.iter().any(|c| {
            c.owner == owner
                && c.binding.is_some()
                && (path_covers(c.key.value(), path) || path_covers(path, c.key.value()))
        })
    }

    /// Children of a component, in order.
    pub fn children(&self, id: ComponentId) -> impl Iterator<Item = &Component<'a>> {
        self.components[id.index()]
            .children
            .iter()
            .map(|c| &self.components[c.index()])
    }

    /// The iglint node path of an owner (`view`, `session` or the component
    /// path).
    pub fn owner_path(&self, owner: Owner) -> &str {
        match owner {
            Owner::Component(id) => &self.components[id.index()].path,
            Owner::Session => "session",
            _ => "view",
        }
    }
}

/// True when `outer` is `inner` or a property path containing it
/// (`props.style` covers `props.style.classes` and `props.style[0]`).
pub(crate) fn path_covers(outer: &str, inner: &str) -> bool {
    inner == outer
        || (inner.starts_with(outer)
            && matches!(inner.as_bytes().get(outer.len()), Some(b'.') | Some(b'[')))
}

/// A component of a view.
#[derive(Debug)]
pub struct Component<'a> {
    pub id: ComponentId,
    pub parent: Option<ComponentId>,
    pub children: Vec<ComponentId>,
    /// 0 for the root container.
    pub depth: u32,
    /// Index in the parent's `children` array.
    pub index_in_parent: usize,
    /// The component object.
    pub value: &'a Value<'a>,
    /// `meta.name`, empty when missing.
    pub name: &'a str,
    /// The `meta.name` value.
    pub name_value: Option<&'a Value<'a>>,
    /// `type`, e.g. `ia.display.label`.
    pub type_id: Option<&'a str>,
    pub type_value: Option<&'a Value<'a>>,
    pub meta: Option<&'a Value<'a>>,
    pub props: Option<&'a Value<'a>>,
    pub custom: Option<&'a Value<'a>>,
    pub position: Option<&'a Value<'a>>,
    pub prop_config: Option<&'a Value<'a>>,
    pub events: Option<&'a Value<'a>>,
    pub scripts: Option<&'a Value<'a>>,
    /// iglint node path, e.g. `root/Header/Title`.
    pub path: String,
    /// JSON pointer, e.g. `/root/children/0/children/1`.
    pub pointer: String,
    /// ign-lint node path, e.g. `root.root.children[0].Header`.
    pub ign_lint_path: String,
}

impl Component<'_> {
    /// `meta.visible` when it is a literal boolean.
    pub fn visible(&self) -> Option<bool> {
        self.meta?.get("visible")?.as_bool()
    }

    /// The component type descriptor from the stubs, if the type is known.
    pub fn component_type(&self) -> Option<&'static iglint_stubs::ComponentType> {
        iglint_stubs::component_type(self.type_id?)
    }

    /// Range of the name value, or of the whole component when it has none.
    pub fn name_range(&self) -> TextRange {
        self.name_value.map_or(self.value.range, |v| v.range)
    }
}

/// `custom` or `params`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PropCategory {
    Custom,
    Params,
}

impl PropCategory {
    /// `custom` or `params`.
    pub fn name(self) -> &'static str {
        match self {
            PropCategory::Custom => "custom",
            PropCategory::Params => "params",
        }
    }
}

/// A custom property or view parameter: a top-level key of `custom` /
/// `params`, or the first segment of a `propConfig` key `custom.<name>...`
/// / `params.<name>...` (properties that exist only through their config).
#[derive(Debug)]
pub struct PropertyDef<'a> {
    pub owner: Owner,
    pub category: PropCategory,
    pub name: &'a str,
    /// The key in the `custom`/`params` object.
    pub key: Option<&'a JsonString<'a>>,
    /// The stored value.
    pub value: Option<&'a Value<'a>>,
    /// The `propConfig` entry for exactly `<category>.<name>`.
    pub config: Option<PropConfigId>,
    /// `paramDirection` of that entry.
    pub direction: Option<ParamDirection>,
    pub node_path: String,
    /// ign-lint property path (indexes stripped).
    pub ign_lint_path: String,
}

impl PropertyDef<'_> {
    /// The best range to report: the key, else the config key.
    pub fn range(&self) -> TextRange {
        self.key
            .map(|k| k.range())
            .or(self.value.map(|v| v.range))
            .unwrap_or_default()
    }
}

/// `propConfig.<path>.paramDirection`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParamDirection {
    Input,
    Output,
    InOut,
}

impl ParamDirection {
    /// Parses `paramDirection`.
    pub fn parse(s: &str) -> Option<ParamDirection> {
        match s {
            "input" => Some(ParamDirection::Input),
            "output" => Some(ParamDirection::Output),
            "inout" => Some(ParamDirection::InOut),
            _ => None,
        }
    }

    /// The direction as written.
    pub fn name(self) -> &'static str {
        match self {
            ParamDirection::Input => "input",
            ParamDirection::Output => "output",
            ParamDirection::InOut => "inout",
        }
    }
}

/// A property path split into its category and the rest:
/// `props.series[0].data` is category `props`, name `series`, rest
/// `series[0].data`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PropPath<'a> {
    pub raw: &'a str,
    pub category: &'a str,
    /// The first segment after the category (empty when there is none).
    pub name: &'a str,
    /// Everything after `category.`.
    pub rest: &'a str,
}

impl<'a> PropPath<'a> {
    /// Splits a property path.
    pub fn parse(raw: &'a str) -> PropPath<'a> {
        let (category, rest) = match raw.find(['.', '[']) {
            Some(i) if raw.as_bytes()[i] == b'.' => (&raw[..i], &raw[i + 1..]),
            Some(i) => (&raw[..i], &raw[i..]),
            None => (raw, ""),
        };
        let name_end = rest.find(['.', '[']).unwrap_or(rest.len());
        PropPath {
            raw,
            category,
            name: &rest[..name_end],
            rest,
        }
    }
}

/// One `propConfig` entry.
#[derive(Debug)]
pub struct PropConfig<'a> {
    pub id: PropConfigId,
    pub owner: Owner,
    /// The property path key, e.g. `props.text`.
    pub key: &'a JsonString<'a>,
    pub path: PropPath<'a>,
    /// The entry object.
    pub value: &'a Value<'a>,
    pub binding: Option<BindingId>,
    pub on_change: Option<OnChange<'a>>,
    pub persistent: Option<bool>,
    pub param_direction: Option<ParamDirection>,
    pub param_direction_value: Option<&'a Value<'a>>,
    /// `access`: `PRIVATE`, `SYSTEM` or absent (public).
    pub access: Option<&'a str>,
    pub node_path: String,
    pub ign_lint_path: String,
}

/// `propConfig.<path>.onChange`.
#[derive(Debug)]
pub struct OnChange<'a> {
    pub value: &'a Value<'a>,
    /// `enabled`; absent means enabled.
    pub enabled: Option<bool>,
    pub script: Option<ScriptId>,
}

/// An event handler: `events.<group>.<event>` holding one action or a list.
#[derive(Debug)]
pub struct EventHandler<'a> {
    pub owner: Owner,
    /// `dom`, `component`, `system`, `model`, ...
    pub group: &'a str,
    /// `onClick`, `onActionPerformed`, ...
    pub name: &'a str,
    pub key: &'a JsonString<'a>,
    pub value: &'a Value<'a>,
    /// True when the value is a list of actions.
    pub is_list: bool,
    pub actions: Vec<Action<'a>>,
    pub node_path: String,
    pub ign_lint_path: String,
}

/// One event action.
#[derive(Debug)]
pub struct Action<'a> {
    /// Index in the list, `None` for a single action.
    pub index: Option<usize>,
    pub value: &'a Value<'a>,
    pub type_value: Option<&'a JsonString<'a>>,
    /// `scope`: `G` (gateway) or `C` (client).
    pub scope: Option<&'a str>,
    /// `enabled`; absent means enabled.
    pub enabled: Option<bool>,
    pub config: Option<&'a Value<'a>>,
    pub kind: ActionKind<'a>,
    pub node_path: String,
    pub ign_lint_path: String,
}

impl Action<'_> {
    /// The action type string, if any.
    pub fn type_name(&self) -> Option<&str> {
        self.type_value.map(|t| t.value())
    }
}

/// Action types and their typed configs (format spec 6.8).
#[derive(Debug)]
pub enum ActionKind<'a> {
    /// `script`: `config.script`.
    Script {
        script: Option<ScriptId>,
    },
    /// `nav`.
    Navigate(NavTarget<'a>),
    /// `popup`.
    Popup(PopupAction<'a>),
    /// `dock`.
    Dock(DockAction<'a>),
    /// `alter-dock`.
    AlterDock {
        view: Option<&'a JsonString<'a>>,
        view_params: Option<&'a Value<'a>>,
    },
    Login,
    Logout,
    AuthChallenge,
    Theme {
        name: Option<&'a JsonString<'a>>,
    },
    /// A known action type without a typed model (`refresh`, `fullscreen`,
    /// `native/barcode`, ...).
    Other,
    /// An action type Ignition does not know (or a missing `type`).
    Unknown,
}

/// The known action type strings.
pub const ACTION_TYPES: &[&str] = &[
    "script",
    "auth-challenge",
    "nav",
    "popup",
    "dock",
    "alter-dock",
    "login",
    "logout",
    "theme",
    "refresh",
    "debug",
    "request-print",
    "console-log",
    "fullscreen",
    "alter-logging",
    "workstation/mode",
    "native/barcode",
    "native/bluetooth",
    "native/ndef",
    "native/accelerometer",
];

/// Target of a `nav` action.
#[derive(Debug)]
pub enum NavTarget<'a> {
    /// `config.page`: a page URL.
    Page(&'a JsonString<'a>),
    /// `config.url`: an external URL.
    Url(&'a JsonString<'a>),
    /// `config.view` with optional `params`.
    View {
        view: &'a JsonString<'a>,
        params: Option<&'a Value<'a>>,
    },
    /// Anything else (browser back/forward, empty config).
    Other,
}

/// `popup` action config.
#[derive(Debug)]
pub struct PopupAction<'a> {
    /// `type`: `open`, `close` or `toggle`.
    pub operation: Option<&'a str>,
    pub id: Option<&'a JsonString<'a>>,
    pub view_path: Option<&'a JsonString<'a>>,
    pub view_params: Option<&'a Value<'a>>,
}

/// `dock` action config.
#[derive(Debug)]
pub struct DockAction<'a> {
    pub operation: Option<&'a str>,
    pub id: Option<&'a JsonString<'a>>,
    pub view_params: Option<&'a Value<'a>>,
}

/// A component message handler.
#[derive(Debug)]
pub struct MessageHandler<'a> {
    pub owner: Owner,
    pub index: usize,
    pub value: &'a Value<'a>,
    pub message_type: Option<&'a JsonString<'a>>,
    pub page_scope: bool,
    pub session_scope: bool,
    pub view_scope: bool,
    pub script: Option<ScriptId>,
    pub node_path: String,
    pub ign_lint_path: String,
}

/// A component custom method.
#[derive(Debug)]
pub struct CustomMethod<'a> {
    pub owner: Owner,
    pub index: usize,
    pub value: &'a Value<'a>,
    pub name: Option<&'a JsonString<'a>>,
    pub params: Vec<&'a JsonString<'a>>,
    pub script: Option<ScriptId>,
    pub node_path: String,
    pub ign_lint_path: String,
}

/// A component extension function implementation.
#[derive(Debug)]
pub struct ExtensionFunctionDef<'a> {
    pub owner: Owner,
    pub index: usize,
    pub value: &'a Value<'a>,
    pub name: Option<&'a JsonString<'a>>,
    pub enabled: Option<bool>,
    /// `Some(true)` when the component type declares this function,
    /// `Some(false)` when it does not, `None` when the type is unknown.
    pub declared: Option<bool>,
    pub script: Option<ScriptId>,
    pub node_path: String,
    pub ign_lint_path: String,
}

/// Where a view path was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ViewRefKind {
    /// `ia.display.view` `props.path`.
    EmbeddedView,
    /// `ia.display.flex-repeater` `props.path`.
    FlexRepeater,
    /// `ia.display.viewcanvas` `props.instances[].viewPath`.
    ViewCanvas,
    /// `ia.display.carousel` `props.views[].viewPath`.
    Carousel,
    /// `ia.display.accordion` item body or header view.
    Accordion,
    /// `ia.display.dashboard` widgets.
    Dashboard,
    /// `ia.display.table` row subview.
    TableSubview,
    /// `ia.display.table` column view.
    TableColumn,
    /// `popup` action `viewPath`.
    PopupAction,
    /// `nav` action `view`.
    NavigateAction,
    /// `alter-dock` action `view`.
    AlterDockAction,
}

/// A literal view path in a view.
#[derive(Debug)]
pub struct ViewRef<'a> {
    pub owner: Owner,
    pub kind: ViewRefKind,
    pub path: &'a JsonString<'a>,
    /// Literal parameters passed to the view, if any.
    pub params: Option<&'a Value<'a>>,
    /// The JSON property path of `path` relative to the owner (e.g.
    /// `props.path`), or the action's node path for actions.
    pub property: String,
    /// True when the path property is bound (the literal is a cached
    /// value).
    pub bound: bool,
    /// True when (part of) the params is bound.
    pub params_bound: bool,
    pub node_path: String,
}

/// Where a style class list was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StyleRefOrigin {
    /// A `classes` key of a style object (`props.style.classes`, ...).
    Property,
    /// A `style-list` map transform output.
    MapOutput,
}

/// A literal, space-separated style class list.
#[derive(Debug)]
pub struct StyleClassRef<'a> {
    pub owner: Owner,
    pub origin: StyleRefOrigin,
    pub source: &'a JsonString<'a>,
    /// Each class path with its file range.
    pub classes: Vec<(&'a str, TextRange)>,
    /// Property path of the list (e.g. `props.style.classes`), or the
    /// binding path for map outputs.
    pub property: String,
    pub bound: bool,
    pub node_path: String,
}

/// Re-exported here because tag bindings parse their paths.
pub type ParsedTagPath = TagPath;

/// Re-exported here because property bindings parse their paths.
pub type ParsedPropertyRef = PropertyRef;

/// A parsed `session-props/props.json`: the same property model as a view.
#[derive(Debug)]
pub struct SessionProps<'a> {
    pub root: &'a Value<'a>,
    pub custom: Option<&'a Value<'a>>,
    pub props: Option<&'a Value<'a>>,
    pub prop_config: Option<&'a Value<'a>>,
    pub properties: Vec<PropertyDef<'a>>,
    pub prop_configs: Vec<PropConfig<'a>>,
    pub bindings: Vec<Binding<'a>>,
    pub scripts: Vec<EmbeddedScript<'a>>,
    pub expressions: Vec<EmbeddedExpression<'a>>,
    pub style_refs: Vec<StyleClassRef<'a>>,
    pub problems: Vec<ModelProblem>,
}

/// Built-in session `props` keys (`session-props.json` schema).
pub const SESSION_PROPS: &[&str] = &[
    "id",
    "host",
    "theme",
    "locale",
    "timeZoneId",
    "lastActivity",
    "auth",
    "gateway",
    "device",
    "bluetooth",
    "geolocation",
    "appBar",
    "pipes",
    "symbols",
    "googleMapsApiKey",
    "offline",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prop_paths() {
        let p = PropPath::parse("props.series[0].data");
        assert_eq!(
            (p.category, p.name, p.rest),
            ("props", "series", "series[0].data")
        );
        let p = PropPath::parse("custom");
        assert_eq!((p.category, p.name, p.rest), ("custom", "", ""));
        assert!(path_covers("props.style", "props.style.classes"));
        assert!(path_covers("props.items", "props.items[0]"));
        assert!(!path_covers("props.style", "props.styles"));
    }
}
