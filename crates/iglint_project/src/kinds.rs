//! Resource kinds, file kinds and the event script table.

use std::fmt;

/// Module id of the core platform.
pub const MODULE_IGNITION: &str = "ignition";
/// Module id of Perspective.
pub const MODULE_PERSPECTIVE: &str = "com.inductiveautomation.perspective";
/// Module id of WebDev.
pub const MODULE_WEBDEV: &str = "com.inductiveautomation.webdev";
/// Module id of Vision.
pub const MODULE_VISION: &str = "com.inductiveautomation.vision";

/// What a resource is, from its `<moduleId>/<typeId>` directory.
///
/// Kinds that iglint models in detail have their own variant; others are
/// identified (so that every file is classified) and carry no model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    // ---- project, `ignition/` ----
    /// `ignition/script-python`: a library module (or package folder with a manifest).
    ScriptLibrary,
    /// `ignition/named-query`.
    NamedQuery,
    /// A file-based event script (gateway events, Perspective session events,
    /// Vision client events).
    EventScript(EventScriptKind),
    /// `ignition/global-props`: project properties (`data.bin`).
    GlobalProps,
    /// `ignition/designer-properties`.
    DesignerProperties,
    /// `ignition/script-app-library`: deprecated since 8.3.0.
    ScriptAppLibrary,
    /// `ignition/event-scripts` and Perspective `session-scripts` /
    /// `gateway-scripts`: pre-8.3 aggregate formats. Recognised, not linted.
    LegacyEventScripts,
    // ---- Perspective ----
    /// `views/<path>`.
    View,
    /// `style-classes/<path>`.
    StyleClass,
    /// `stylesheet` singleton.
    Stylesheet,
    /// `page-config` singleton.
    PageConfig,
    /// `session-props` singleton.
    SessionProps,
    /// `general-properties`, `inactivity-properties`, `session-permissions`,
    /// `offline-mode-settings`, `tag-drop-settings`, `symbol-state-settings`.
    PerspectiveSettings,
    /// Gateway-wide `themes`, `icons`, `fonts`.
    PerspectiveAsset,
    // ---- WebDev ----
    /// `com.inductiveautomation.webdev/resources/<path>`.
    WebDev,
    // ---- other project modules, identified only ----
    /// `com.inductiveautomation.reporting/reports`.
    Report,
    /// `com.inductiveautomation.alarm-notification/alarm-pipelines`.
    AlarmPipeline,
    /// `com.inductiveautomation.sfc/charts`.
    SfcChart,
    /// `com.inductiveautomation.vision/windows`.
    VisionWindow,
    /// `com.inductiveautomation.vision/templates`.
    VisionTemplate,
    /// Other Vision project resources (properties, palettes, client tags).
    VisionOther,
    /// `com.inductiveautomation.sqlbridge/transaction-groups`.
    TransactionGroup,
    /// `com.inductiveautomation.eventstream/event-streams`.
    EventStream,
    // ---- gateway configuration ----
    /// `ignition/tag-provider/<provider>`.
    TagProvider,
    /// `ignition/tag-group/<provider>/<group>`.
    TagGroup,
    /// `ignition/tag-definition/<provider>/...`: a tag folder.
    TagDefinition,
    /// `ignition/tag-type-definition/<provider>/...`: a UDT definition folder.
    TagTypeDefinition,
    /// `ignition/database-connection/<name>`.
    DatabaseConnection,
    /// `com.inductiveautomation.historian/historian-provider/<name>`.
    HistorianProvider,
    /// `com.inductiveautomation.opcua/device/<name>`.
    OpcDevice,
    /// `com.inductiveautomation.alarm-notification/alarm-notification-profile`.
    AlarmNotificationProfile,
    /// Any other platform setting (`user-source`, `security-zone`, ...).
    GatewaySetting,
    /// A module/type pair iglint does not know.
    Unknown,
}

impl ResourceKind {
    /// Whether iglint models resources of this kind (reads their data files
    /// and knows which files the gateway loads), as opposed to kinds that
    /// are only identified.
    pub fn is_modelled(self) -> bool {
        use ResourceKind as K;
        matches!(
            self,
            K::ScriptLibrary
                | K::NamedQuery
                | K::EventScript(_)
                | K::View
                | K::StyleClass
                | K::Stylesheet
                | K::PageConfig
                | K::SessionProps
                | K::WebDev
                | K::TagProvider
                | K::TagGroup
                | K::TagDefinition
                | K::TagTypeDefinition
                | K::DatabaseConnection
                | K::HistorianProvider
                | K::OpcDevice
                | K::AlarmNotificationProfile
                | K::GatewaySetting
        )
    }

    /// Whether resources of this kind use `unary-resource.json` (tag
    /// folders) instead of `resource.json`.
    pub fn has_unary_manifest(self) -> bool {
        matches!(
            self,
            ResourceKind::TagDefinition | ResourceKind::TagTypeDefinition
        )
    }

    /// Classifies a `<moduleId>/<typeId>` pair.
    pub fn from_type(module_id: &str, type_id: &str) -> ResourceKind {
        use ResourceKind as K;
        match (module_id, type_id) {
            (MODULE_IGNITION, t) => match t {
                "script-python" => K::ScriptLibrary,
                "named-query" => K::NamedQuery,
                "global-props" => K::GlobalProps,
                "designer-properties" => K::DesignerProperties,
                "script-app-library" => K::ScriptAppLibrary,
                "event-scripts" => K::LegacyEventScripts,
                "tag-provider" => K::TagProvider,
                "tag-group" => K::TagGroup,
                "tag-definition" => K::TagDefinition,
                "tag-type-definition" => K::TagTypeDefinition,
                "database-connection" => K::DatabaseConnection,
                _ => match EventScriptKind::from_type(module_id, t) {
                    Some(k) => K::EventScript(k),
                    None if IGNITION_SETTINGS.contains(&t) => K::GatewaySetting,
                    None => K::Unknown,
                },
            },
            (MODULE_PERSPECTIVE, t) => match t {
                "views" => K::View,
                "style-classes" => K::StyleClass,
                "stylesheet" => K::Stylesheet,
                "page-config" => K::PageConfig,
                "session-props" => K::SessionProps,
                "general-properties"
                | "inactivity-properties"
                | "session-permissions"
                | "offline-mode-settings"
                | "tag-drop-settings"
                | "symbol-state-settings" => K::PerspectiveSettings,
                "themes" | "icons" | "fonts" => K::PerspectiveAsset,
                "session-scripts" | "gateway-scripts" => K::LegacyEventScripts,
                _ => match EventScriptKind::from_type(module_id, t) {
                    Some(k) => K::EventScript(k),
                    None => K::Unknown,
                },
            },
            (MODULE_WEBDEV, "resources") => K::WebDev,
            ("com.inductiveautomation.reporting", "reports") => K::Report,
            ("com.inductiveautomation.alarm-notification", "alarm-pipelines") => K::AlarmPipeline,
            ("com.inductiveautomation.alarm-notification", "alarm-notification-profile") => {
                K::AlarmNotificationProfile
            }
            ("com.inductiveautomation.sfc", "charts") => K::SfcChart,
            ("com.inductiveautomation.sfc", _) => K::GatewaySetting,
            (MODULE_VISION, "windows") => K::VisionWindow,
            (MODULE_VISION, "templates") => K::VisionTemplate,
            (MODULE_VISION, t) => match EventScriptKind::from_type(module_id, t) {
                Some(k) => K::EventScript(k),
                None => K::VisionOther,
            },
            ("com.inductiveautomation.sqlbridge", "transaction-groups") => K::TransactionGroup,
            ("com.inductiveautomation.eventstream", "event-streams") => K::EventStream,
            ("com.inductiveautomation.historian", "historian-provider") => K::HistorianProvider,
            ("com.inductiveautomation.opcua", "device") => K::OpcDevice,
            ("com.inductiveautomation.opcua", _)
            | ("com.inductiveautomation.eam", _)
            | ("com.inductiveautomation.sip-notification", _) => K::GatewaySetting,
            _ => K::Unknown,
        }
    }

    /// Short stable name for dumps and messages.
    pub fn name(self) -> &'static str {
        use ResourceKind as K;
        match self {
            K::ScriptLibrary => "script-library",
            K::NamedQuery => "named-query",
            K::EventScript(k) => k.name(),
            K::GlobalProps => "global-props",
            K::DesignerProperties => "designer-properties",
            K::ScriptAppLibrary => "script-app-library",
            K::LegacyEventScripts => "legacy-event-scripts",
            K::View => "view",
            K::StyleClass => "style-class",
            K::Stylesheet => "stylesheet",
            K::PageConfig => "page-config",
            K::SessionProps => "session-props",
            K::PerspectiveSettings => "perspective-settings",
            K::PerspectiveAsset => "perspective-asset",
            K::WebDev => "webdev",
            K::Report => "report",
            K::AlarmPipeline => "alarm-pipeline",
            K::SfcChart => "sfc-chart",
            K::VisionWindow => "vision-window",
            K::VisionTemplate => "vision-template",
            K::VisionOther => "vision-other",
            K::TransactionGroup => "transaction-group",
            K::EventStream => "event-stream",
            K::TagProvider => "tag-provider",
            K::TagGroup => "tag-group",
            K::TagDefinition => "tag-definition",
            K::TagTypeDefinition => "tag-type-definition",
            K::DatabaseConnection => "database-connection",
            K::HistorianProvider => "historian-provider",
            K::OpcDevice => "opc-device",
            K::AlarmNotificationProfile => "alarm-notification-profile",
            K::GatewaySetting => "gateway-setting",
            K::Unknown => "unknown",
        }
    }
}

impl fmt::Display for ResourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Platform setting types under `ignition/` (Appendix A of the format spec)
/// that iglint only needs by name.
const IGNITION_SETTINGS: &[&str] = &[
    "alarm-journal",
    "api-token",
    "audit-profile",
    "cobranding",
    "database-driver",
    "database-translator",
    "edge-sync-settings",
    "edge-system-properties",
    "email-profile",
    "gateway-network-incoming",
    "gateway-network-outgoing",
    "gateway-network-proxy-rules",
    "gateway-network-queue-settings",
    "gateway-network-settings",
    "general-alarm-settings",
    "holiday",
    "identity-provider",
    "images",
    "keyboard_layout",
    "local-system-properties",
    "metrics-dashboard",
    "oauth2-client",
    "opc-connection",
    "quickstart",
    "roster-config",
    "schedule",
    "secret-provider",
    "security-levels",
    "security-properties",
    "security-zone",
    "service-connector",
    "smtp.classic",
    "store-and-forward-engine",
    "system-properties",
    "tag-report-search",
    "tags",
    "translations",
    "user-source",
    "web-server",
];

/// HTTP methods of a WebDev python resource.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Head,
    Options,
    Trace,
    Patch,
}

impl HttpMethod {
    /// All methods in the order Ignition writes them in `config.json`.
    pub const ALL: [HttpMethod; 8] = [
        HttpMethod::Get,
        HttpMethod::Post,
        HttpMethod::Put,
        HttpMethod::Delete,
        HttpMethod::Head,
        HttpMethod::Options,
        HttpMethod::Trace,
        HttpMethod::Patch,
    ];

    /// The handler function name, e.g. `doGet`. Also the `config.json` key.
    pub fn function_name(self) -> &'static str {
        match self {
            HttpMethod::Get => "doGet",
            HttpMethod::Post => "doPost",
            HttpMethod::Put => "doPut",
            HttpMethod::Delete => "doDelete",
            HttpMethod::Head => "doHead",
            HttpMethod::Options => "doOptions",
            HttpMethod::Trace => "doTrace",
            HttpMethod::Patch => "doPatch",
        }
    }

    /// The script file name, e.g. `doGet.py`.
    pub fn file_name(self) -> &'static str {
        match self {
            HttpMethod::Get => "doGet.py",
            HttpMethod::Post => "doPost.py",
            HttpMethod::Put => "doPut.py",
            HttpMethod::Delete => "doDelete.py",
            HttpMethod::Head => "doHead.py",
            HttpMethod::Options => "doOptions.py",
            HttpMethod::Trace => "doTrace.py",
            HttpMethod::Patch => "doPatch.py",
        }
    }

    /// The method for a script file name.
    pub fn from_file_name(name: &str) -> Option<HttpMethod> {
        HttpMethod::ALL.into_iter().find(|m| m.file_name() == name)
    }

    /// True for methods that change state (`POST`, `PUT`, `DELETE`, `PATCH`).
    pub fn is_mutating(self) -> bool {
        matches!(
            self,
            HttpMethod::Post | HttpMethod::Put | HttpMethod::Delete | HttpMethod::Patch
        )
    }
}

/// Where a script runs, which decides the names and `system.*` functions it
/// can use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RunScope {
    /// Perspective view and component scripts (gateway, inside a session,
    /// with `self` a component or view).
    PerspectiveComponent,
    /// Perspective session-level events (gateway, inside a session or page,
    /// no `self`).
    PerspectiveSession,
    /// Gateway event scripts and WebDev handlers.
    Gateway,
    /// Tag event scripts (gateway, tag provider).
    Tag,
    /// Vision client scripts.
    VisionClient,
    /// Project library modules: every scope that imports them.
    Library,
}

impl RunScope {
    /// Short stable name for dumps.
    pub fn name(self) -> &'static str {
        match self {
            RunScope::PerspectiveComponent => "perspective-component",
            RunScope::PerspectiveSession => "perspective-session",
            RunScope::Gateway => "gateway",
            RunScope::Tag => "tag",
            RunScope::VisionClient => "vision-client",
            RunScope::Library => "library",
        }
    }
}

/// The event scripts stored as `.py` files that contain a full `def`
/// (format spec 5.1 and 5.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventScriptKind {
    GatewayStartup,
    GatewayShutdown,
    GatewayUpdate,
    GatewayTimer,
    GatewayScheduled,
    GatewayTagChange,
    GatewayMessage,
    SessionStartup,
    SessionShutdown,
    PageStartup,
    SessionMessage,
    KeyEvent,
    Barcode,
    Bluetooth,
    NfcScan,
    Accelerometer,
    AuthChallenge,
    FormSubmission,
    /// A WebDev python resource handler.
    WebDev(HttpMethod),
    // Vision client events. File names follow the gateway events; the
    // resources are identified, their exact on-disk names are not verified.
    ClientStartup,
    ClientShutdown,
    ClientTimer,
    ClientTagChange,
    ClientMessage,
    ClientKeystroke,
    ClientShutdownIntercept,
}

struct EventScriptInfo {
    kind: EventScriptKind,
    module: &'static str,
    type_id: &'static str,
    /// True when resources live in named subdirectories (`timer/<name>/`).
    named: bool,
    function: &'static str,
    params: &'static [&'static str],
    scope: RunScope,
    name: &'static str,
}

const TAG_CHANGE_PARAMS: &[&str] = &[
    "initialChange",
    "newValue",
    "previousValue",
    "event",
    "executionCount",
];

#[rustfmt::skip]
const EVENT_SCRIPTS: &[EventScriptInfo] = {
    use EventScriptKind as E;
    use RunScope as S;
    &[
        EventScriptInfo { kind: E::GatewayStartup, module: MODULE_IGNITION, type_id: "startup", named: false, function: "onStartup", params: &[], scope: S::Gateway, name: "gateway-startup" },
        EventScriptInfo { kind: E::GatewayShutdown, module: MODULE_IGNITION, type_id: "shutdown", named: false, function: "onShutdown", params: &[], scope: S::Gateway, name: "gateway-shutdown" },
        EventScriptInfo { kind: E::GatewayUpdate, module: MODULE_IGNITION, type_id: "update", named: false, function: "onUpdate", params: &["actor", "resources"], scope: S::Gateway, name: "gateway-update" },
        EventScriptInfo { kind: E::GatewayTimer, module: MODULE_IGNITION, type_id: "timer", named: true, function: "handleTimerEvent", params: &[], scope: S::Gateway, name: "gateway-timer" },
        EventScriptInfo { kind: E::GatewayScheduled, module: MODULE_IGNITION, type_id: "scheduled", named: true, function: "handleScheduleEvent", params: &[], scope: S::Gateway, name: "gateway-scheduled" },
        EventScriptInfo { kind: E::GatewayTagChange, module: MODULE_IGNITION, type_id: "tag-change", named: true, function: "onTagChange", params: TAG_CHANGE_PARAMS, scope: S::Gateway, name: "gateway-tag-change" },
        EventScriptInfo { kind: E::GatewayMessage, module: MODULE_IGNITION, type_id: "message", named: true, function: "handleMessage", params: &["payload"], scope: S::Gateway, name: "gateway-message" },
        EventScriptInfo { kind: E::SessionStartup, module: MODULE_PERSPECTIVE, type_id: "startup", named: false, function: "onStartup", params: &["session"], scope: S::PerspectiveSession, name: "session-startup" },
        EventScriptInfo { kind: E::SessionShutdown, module: MODULE_PERSPECTIVE, type_id: "shutdown", named: false, function: "onShutdown", params: &["session"], scope: S::PerspectiveSession, name: "session-shutdown" },
        EventScriptInfo { kind: E::PageStartup, module: MODULE_PERSPECTIVE, type_id: "page-startup", named: false, function: "onPageStartup", params: &["page"], scope: S::PerspectiveSession, name: "page-startup" },
        EventScriptInfo { kind: E::SessionMessage, module: MODULE_PERSPECTIVE, type_id: "message", named: true, function: "handleMessage", params: &["session", "payload"], scope: S::PerspectiveSession, name: "session-message" },
        EventScriptInfo { kind: E::KeyEvent, module: MODULE_PERSPECTIVE, type_id: "key-event", named: true, function: "onKeyEvent", params: &["page", "event"], scope: S::PerspectiveSession, name: "key-event" },
        EventScriptInfo { kind: E::Barcode, module: MODULE_PERSPECTIVE, type_id: "barcode", named: false, function: "onBarcodeDataReceived", params: &["session", "data", "context"], scope: S::PerspectiveSession, name: "barcode" },
        EventScriptInfo { kind: E::Bluetooth, module: MODULE_PERSPECTIVE, type_id: "bluetooth", named: false, function: "onBluetoothReceived", params: &["session", "data"], scope: S::PerspectiveSession, name: "bluetooth" },
        EventScriptInfo { kind: E::NfcScan, module: MODULE_PERSPECTIVE, type_id: "nfc-scan", named: false, function: "onNdefDataReceived", params: &["session", "data", "context"], scope: S::PerspectiveSession, name: "nfc-scan" },
        EventScriptInfo { kind: E::Accelerometer, module: MODULE_PERSPECTIVE, type_id: "accelerometer", named: false, function: "onAccelerometerDataReceived", params: &["session", "data", "context"], scope: S::PerspectiveSession, name: "accelerometer" },
        EventScriptInfo { kind: E::AuthChallenge, module: MODULE_PERSPECTIVE, type_id: "auth-challenge", named: false, function: "onAuthChallengeCompleted", params: &["session", "payload", "result"], scope: S::PerspectiveSession, name: "auth-challenge" },
        EventScriptInfo { kind: E::FormSubmission, module: MODULE_PERSPECTIVE, type_id: "form-submission-handler", named: true, function: "handleSubmission", params: &["session", "name", "data", "formContext", "sessionContext", "retry"], scope: S::PerspectiveSession, name: "form-submission" },
        EventScriptInfo { kind: E::ClientStartup, module: MODULE_VISION, type_id: "startup", named: false, function: "onStartup", params: &[], scope: S::VisionClient, name: "client-startup" },
        EventScriptInfo { kind: E::ClientShutdown, module: MODULE_VISION, type_id: "shutdown", named: false, function: "onShutdown", params: &[], scope: S::VisionClient, name: "client-shutdown" },
        EventScriptInfo { kind: E::ClientTimer, module: MODULE_VISION, type_id: "timer", named: true, function: "handleTimerEvent", params: &[], scope: S::VisionClient, name: "client-timer" },
        EventScriptInfo { kind: E::ClientTagChange, module: MODULE_VISION, type_id: "tag-change", named: true, function: "onTagChange", params: TAG_CHANGE_PARAMS, scope: S::VisionClient, name: "client-tag-change" },
        EventScriptInfo { kind: E::ClientMessage, module: MODULE_VISION, type_id: "message", named: true, function: "handleMessage", params: &["payload"], scope: S::VisionClient, name: "client-message" },
        EventScriptInfo { kind: E::ClientKeystroke, module: MODULE_VISION, type_id: "keystroke", named: true, function: "onKeystroke", params: &[], scope: S::VisionClient, name: "client-keystroke" },
        EventScriptInfo { kind: E::ClientShutdownIntercept, module: MODULE_VISION, type_id: "shutdown-intercept", named: false, function: "isShutdownPrevented", params: &["event"], scope: S::VisionClient, name: "client-shutdown-intercept" },
    ]
};

const WEBDEV_PARAMS: &[&str] = &["request", "session"];

impl EventScriptKind {
    fn info(self) -> Option<&'static EventScriptInfo> {
        EVENT_SCRIPTS.iter().find(|i| i.kind == self)
    }

    /// The event script kind of a `<moduleId>/<typeId>` pair, if any. WebDev
    /// resources hold several scripts and are not matched here.
    pub fn from_type(module_id: &str, type_id: &str) -> Option<EventScriptKind> {
        EVENT_SCRIPTS
            .iter()
            .find(|i| i.module == module_id && i.type_id == type_id)
            .map(|i| i.kind)
    }

    /// The generated function name, e.g. `handleTimerEvent`.
    pub fn function_name(self) -> &'static str {
        match self {
            EventScriptKind::WebDev(m) => m.function_name(),
            k => k.info().map_or("", |i| i.function),
        }
    }

    /// The canonical parameters, e.g. `["session", "payload"]`.
    pub fn params(self) -> &'static [&'static str] {
        match self {
            EventScriptKind::WebDev(_) => WEBDEV_PARAMS,
            k => k.info().map_or(&[], |i| i.params),
        }
    }

    /// The script file name, e.g. `handleTimerEvent.py`.
    pub fn file_name(self) -> String {
        format!("{}.py", self.function_name())
    }

    /// The canonical header line Ignition compiles the body under, e.g.
    /// `def handleMessage(session, payload):`.
    pub fn canonical_header(self) -> String {
        format!(
            "def {}({}):",
            self.function_name(),
            self.params().join(", ")
        )
    }

    /// Where the script runs.
    pub fn run_scope(self) -> RunScope {
        match self {
            EventScriptKind::WebDev(_) => RunScope::Gateway,
            k => k.info().map_or(RunScope::Gateway, |i| i.scope),
        }
    }

    /// True when resources of this kind live in named subdirectories.
    pub fn is_named(self) -> bool {
        match self {
            EventScriptKind::WebDev(_) => true,
            k => k.info().is_some_and(|i| i.named),
        }
    }

    /// Short stable name, e.g. `gateway-timer`.
    pub fn name(self) -> &'static str {
        match self {
            EventScriptKind::WebDev(_) => "webdev",
            k => k.info().map_or("event", |i| i.name),
        }
    }
}

/// What a file is. Derived from its resource kind and file name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileKind {
    /// `project.json`.
    ProjectManifest,
    /// `config-mode.json` of a gateway config collection.
    CollectionManifest,
    /// `resource.json`.
    ResourceManifest,
    /// `unary-resource.json` (tag folders).
    UnaryResourceManifest,
    /// Perspective `view.json`.
    View,
    /// `thumbnail.png` of a view or window.
    Thumbnail,
    /// Perspective `style.json` (or legacy `data.bin`).
    StyleClass,
    /// Perspective `stylesheet.css`.
    Stylesheet,
    /// A CSS file of a gateway Perspective theme.
    ThemeCss,
    /// Perspective `page-config/config.json`.
    PageConfig,
    /// Perspective `session-props/props.json`.
    SessionProps,
    /// Library `code.py`.
    LibraryModule,
    /// Named query `query.sql`.
    NamedQuerySql,
    /// An event script `.py` with a full `def`.
    EventScript(EventScriptKind),
    /// WebDev `config.json`.
    WebDevConfig,
    /// Other WebDev data (text or file resource content).
    WebDevContent,
    /// `tags.json` (`definitions: false`) of a tag folder.
    TagsJson,
    /// `udts.json`: UDT instances in a tag folder (`definitions: false`) or UDT
    /// definitions in a type folder (`definitions: true`).
    UdtsJson { definitions: bool },
    /// `config.json` of a gateway config resource.
    GatewayConfig,
    /// Settings data (`data.bin` and similar) of settings singletons.
    SettingsData,
    /// A data file of a resource kind iglint does not model.
    OtherResourceData,
    /// A file in a project or collection that is not inside any resource
    /// directory.
    Stray,
}

impl FileKind {
    /// Classifies a data file of a resource.
    pub fn of_resource_file(kind: ResourceKind, file_name: &str) -> FileKind {
        use FileKind as F;
        use ResourceKind as K;
        match (kind, file_name) {
            (_, "resource.json") => F::ResourceManifest,
            (_, "unary-resource.json") => F::UnaryResourceManifest,
            (_, "thumbnail.png") => F::Thumbnail,
            (K::View, "view.json") => F::View,
            (K::StyleClass, "style.json" | "data.bin") => F::StyleClass,
            (K::Stylesheet, "stylesheet.css") => F::Stylesheet,
            (K::PerspectiveAsset, name) if name.ends_with(".css") => F::ThemeCss,
            (K::PageConfig, "config.json") => F::PageConfig,
            (K::SessionProps, "props.json") => F::SessionProps,
            (K::ScriptLibrary, "code.py") => F::LibraryModule,
            (K::NamedQuery, "query.sql") => F::NamedQuerySql,
            (K::EventScript(e), name) if name == e.file_name() => F::EventScript(e),
            (K::WebDev, "config.json") => F::WebDevConfig,
            (K::WebDev, name) => match HttpMethod::from_file_name(name) {
                Some(m) => F::EventScript(EventScriptKind::WebDev(m)),
                None => F::WebDevContent,
            },
            (K::TagDefinition, "tags.json") => F::TagsJson,
            (K::TagDefinition, "udts.json") => F::UdtsJson { definitions: false },
            (K::TagTypeDefinition, "udts.json") => F::UdtsJson { definitions: true },
            (K::TagTypeDefinition, "tags.json") => F::TagsJson,
            (
                K::TagProvider
                | K::TagGroup
                | K::DatabaseConnection
                | K::HistorianProvider
                | K::OpcDevice
                | K::AlarmNotificationProfile
                | K::GatewaySetting,
                "config.json",
            ) => F::GatewayConfig,
            (
                K::GlobalProps
                | K::DesignerProperties
                | K::PerspectiveSettings
                | K::LegacyEventScripts,
                _,
            ) => F::SettingsData,
            _ => F::OtherResourceData,
        }
    }

    /// True for the main data file of a resource kind iglint models (a
    /// `view.json` below `views/`, a library `code.py`, an event script,
    /// ...): such a file in a directory without a manifest is a resource
    /// whose manifest is missing, not a stray file.
    pub fn implies_resource(self) -> bool {
        use FileKind as F;
        matches!(
            self,
            F::View
                | F::StyleClass
                | F::Stylesheet
                | F::PageConfig
                | F::SessionProps
                | F::LibraryModule
                | F::NamedQuerySql
                | F::EventScript(_)
                | F::WebDevConfig
                | F::TagsJson
                | F::UdtsJson { .. }
                | F::GatewayConfig
        )
    }

    /// True for files whose content iglint reads (JSON, Python, SQL, CSS).
    pub fn is_text(self) -> bool {
        !matches!(
            self,
            FileKind::Thumbnail
                | FileKind::SettingsData
                | FileKind::OtherResourceData
                | FileKind::Stray
        )
    }

    /// True for files whose content is JSON.
    pub fn is_json(self) -> bool {
        matches!(
            self,
            FileKind::ProjectManifest
                | FileKind::CollectionManifest
                | FileKind::ResourceManifest
                | FileKind::UnaryResourceManifest
                | FileKind::View
                | FileKind::StyleClass
                | FileKind::PageConfig
                | FileKind::SessionProps
                | FileKind::WebDevConfig
                | FileKind::TagsJson
                | FileKind::UdtsJson { .. }
                | FileKind::GatewayConfig
        )
    }

    /// Short stable name for dumps.
    pub fn name(self) -> &'static str {
        match self {
            FileKind::ProjectManifest => "project-manifest",
            FileKind::CollectionManifest => "collection-manifest",
            FileKind::ResourceManifest => "resource-manifest",
            FileKind::UnaryResourceManifest => "unary-resource-manifest",
            FileKind::View => "view",
            FileKind::Thumbnail => "thumbnail",
            FileKind::StyleClass => "style-class",
            FileKind::Stylesheet => "stylesheet",
            FileKind::ThemeCss => "theme-css",
            FileKind::PageConfig => "page-config",
            FileKind::SessionProps => "session-props",
            FileKind::LibraryModule => "library-module",
            FileKind::NamedQuerySql => "named-query-sql",
            FileKind::EventScript(EventScriptKind::WebDev(_)) => "webdev-script",
            FileKind::EventScript(_) => "event-script",
            FileKind::WebDevConfig => "webdev-config",
            FileKind::WebDevContent => "webdev-content",
            FileKind::TagsJson => "tags-json",
            FileKind::UdtsJson { definitions: false } => "udt-instances-json",
            FileKind::UdtsJson { definitions: true } => "udt-definitions-json",
            FileKind::GatewayConfig => "gateway-config",
            FileKind::SettingsData => "settings-data",
            FileKind::OtherResourceData => "other-resource-data",
            FileKind::Stray => "stray",
        }
    }
}

impl fmt::Display for FileKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_headers() {
        assert_eq!(
            EventScriptKind::GatewayTagChange.canonical_header(),
            "def onTagChange(initialChange, newValue, previousValue, event, executionCount):"
        );
        assert_eq!(
            EventScriptKind::FormSubmission.canonical_header(),
            "def handleSubmission(session, name, data, formContext, sessionContext, retry):"
        );
        assert_eq!(
            EventScriptKind::WebDev(HttpMethod::Patch).canonical_header(),
            "def doPatch(request, session):"
        );
        assert_eq!(
            EventScriptKind::GatewayStartup.canonical_header(),
            "def onStartup():"
        );
    }

    #[test]
    fn classification() {
        assert_eq!(
            ResourceKind::from_type("ignition", "timer"),
            ResourceKind::EventScript(EventScriptKind::GatewayTimer)
        );
        assert_eq!(
            ResourceKind::from_type(MODULE_PERSPECTIVE, "startup"),
            ResourceKind::EventScript(EventScriptKind::SessionStartup)
        );
        assert_eq!(
            ResourceKind::from_type("ignition", "user-source"),
            ResourceKind::GatewaySetting
        );
        assert_eq!(ResourceKind::from_type("x.y", "z"), ResourceKind::Unknown);
        assert_eq!(
            FileKind::of_resource_file(ResourceKind::WebDev, "doGet.py"),
            FileKind::EventScript(EventScriptKind::WebDev(HttpMethod::Get))
        );
        assert_eq!(
            FileKind::of_resource_file(
                ResourceKind::EventScript(EventScriptKind::GatewayTimer),
                "handleTimerEvent.py"
            ),
            FileKind::EventScript(EventScriptKind::GatewayTimer)
        );
    }
}
