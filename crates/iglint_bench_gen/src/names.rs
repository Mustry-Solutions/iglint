//! Vocabulary for generated names. Generic process-industry terms only.

pub const EQUIPMENT: &[&str] = &[
    "Motor",
    "Pump",
    "Valve",
    "Conveyor",
    "Tank",
    "Mixer",
    "Station",
    "Oven",
    "Press",
    "Filler",
    "Capper",
    "Labeler",
    "Palletizer",
    "Compressor",
    "Boiler",
    "Chiller",
    "Fan",
    "Heater",
    "Scale",
    "Robot",
    "Feeder",
    "Hopper",
    "Silo",
    "Dryer",
    "Cooler",
    "Washer",
    "Sorter",
    "Packer",
    "Extruder",
    "Grinder",
    "Cutter",
    "Wrapper",
    "Elevator",
    "Crane",
    "Kiln",
];

pub const ASPECTS: &[&str] = &[
    "Status",
    "Speed",
    "Temperature",
    "Pressure",
    "Level",
    "Flow",
    "Alarm",
    "Mode",
    "Setpoint",
    "Counter",
    "Runtime",
    "Overview",
    "Detail",
    "Summary",
    "Trend",
    "History",
    "Settings",
    "Header",
    "Footer",
    "Card",
    "Panel",
    "List",
    "Badge",
    "Indicator",
    "State",
    "Recipe",
    "Batch",
    "Order",
    "Shift",
    "Energy",
    "Quality",
    "Downtime",
    "Maintenance",
    "Throughput",
    "Target",
    "Actual",
    "Limit",
    "Command",
    "Feedback",
    "Selection",
];

pub const PROP_WORDS: &[&str] = &[
    "selected", "current", "target", "line", "motor", "speed", "state", "mode", "filter", "start",
    "end", "range", "unit", "count", "total", "active", "visible", "enabled", "title", "label",
    "value", "limit", "order", "batch", "recipe", "shift", "operator", "zone", "area", "station",
    "device", "product", "message", "refresh", "page", "index",
];

pub const PROP_SUFFIXES: &[&str] = &[
    "Value", "Index", "Path", "Name", "Count", "Date", "Time", "State", "Mode", "List", "Text",
    "Rate", "Limit", "Id", "Data", "Flag", "Color", "Label", "Filter", "Items",
];

pub const PACKAGES: &[&str] = &[
    "core",
    "hmi",
    "utils",
    "reporting",
    "quality",
    "maintenance",
    "production",
    "alarms",
    "recipes",
    "integration",
    "energy",
    "logistics",
    "shared",
    "plant",
    "mes",
    "lab",
];

pub const MODULE_WORDS: &[&str] = &[
    "format",
    "tags",
    "nav",
    "db",
    "dates",
    "popups",
    "messages",
    "users",
    "audit",
    "export",
    "counters",
    "states",
    "orders",
    "batches",
    "recipes",
    "limits",
    "alarms",
    "trends",
    "colors",
    "units",
    "config",
    "cache",
    "validation",
    "labels",
    "printing",
    "shifts",
    "schedule",
    "kpi",
    "oee",
    "downtime",
    "energy",
    "inventory",
    "materials",
    "assets",
];

pub const VERBS: &[&str] = &[
    "get", "set", "load", "save", "format", "build", "update", "compute", "resolve", "read",
    "write", "check", "parse", "convert", "find", "list", "create", "delete", "apply", "send",
];

pub const NOUNS: &[&str] = &[
    "Value",
    "State",
    "Order",
    "Batch",
    "Recipe",
    "Limits",
    "Rows",
    "Summary",
    "Label",
    "Color",
    "Path",
    "Status",
    "Counters",
    "Duration",
    "Report",
    "Message",
    "Target",
    "Shift",
    "Operator",
    "Settings",
    "History",
    "Totals",
    "Payload",
    "Selection",
    "Units",
];

pub const MESSAGES: &[&str] = &[
    "refreshData",
    "lineChanged",
    "orderSelected",
    "batchStarted",
    "batchStopped",
    "recipeLoaded",
    "alarmAcknowledged",
    "shiftChanged",
    "filterChanged",
    "closePopup",
    "showDetails",
    "updateCounters",
    "modeChanged",
    "reloadTable",
    "userChanged",
];

pub const STYLE_GROUPS: &[&str] = &[
    "Brand", "State", "Text", "Card", "Layout", "Button", "Table", "Alarm", "Header", "Icon",
];

pub const STYLE_WORDS: &[&str] = &[
    "Primary",
    "Secondary",
    "Running",
    "Stopped",
    "Faulted",
    "Idle",
    "Warning",
    "Title",
    "Subtitle",
    "Body",
    "Small",
    "Large",
    "Muted",
    "Highlight",
    "Shadow",
    "Border",
    "Rounded",
    "Compact",
    "Wide",
    "Active",
    "Inactive",
    "Selected",
    "Disabled",
    "Success",
    "Danger",
];

pub const ICONS: &[&str] = &[
    "material/settings",
    "material/warning",
    "material/play_arrow",
    "material/stop",
    "material/refresh",
    "material/info",
    "material/close",
    "material/check_circle",
    "material/error",
    "material/search",
    "material/edit",
    "material/delete",
];

/// `motorSpeed`-style property name.
pub fn prop_name(rng: &mut crate::rng::Rng) -> String {
    let w = rng.pick(PROP_WORDS);
    let s = rng.pick(PROP_SUFFIXES);
    format!("{w}{s}")
}

/// `MotorStatus`-style component name.
pub fn pascal(rng: &mut crate::rng::Rng) -> String {
    format!("{}{}", rng.pick(EQUIPMENT), rng.pick(ASPECTS))
}

pub fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
