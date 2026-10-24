//! Perspective `view.json` generator.

use std::collections::{BTreeMap, HashSet};

use serde_json::{Map, Value, json};

use crate::expr::{self, ExprCtx, ExprDefect};
use crate::names;
use crate::rng::Rng;
use crate::script::{self, ScriptCtx};
use crate::world::{Role, ViewSpec, World};

#[derive(Default, Debug, Clone)]
pub struct Stats {
    pub views: usize,
    pub components: usize,
    pub bindings: usize,
    pub transforms: usize,
    pub scripts: usize,
    pub script_lines: usize,
    pub bytes: usize,
    pub defects: BTreeMap<&'static str, usize>,
}

impl Stats {
    pub fn add(&mut self, o: &Stats) {
        self.views += o.views;
        self.components += o.components;
        self.bindings += o.bindings;
        self.transforms += o.transforms;
        self.scripts += o.scripts;
        self.script_lines += o.script_lines;
        self.bytes += o.bytes;
        for (k, v) in &o.defects {
            *self.defects.entry(k).or_default() += v;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Flex,
    Coord,
    Label,
    Svg,
    Icon,
    View,
    Button,
    TextField,
    Table,
    Checkbox,
    Numeric,
    Dropdown,
    Repeater,
    DateTime,
    Markdown,
    MultiState,
    Image,
    TextArea,
    Chart,
}

impl Kind {
    fn type_id(self) -> &'static str {
        match self {
            Kind::Flex => "ia.container.flex",
            Kind::Coord => "ia.container.coord",
            Kind::Label => "ia.display.label",
            Kind::Svg => "ia.shapes.svg",
            Kind::Icon => "ia.display.icon",
            Kind::View => "ia.display.view",
            Kind::Button => "ia.input.button",
            Kind::TextField => "ia.input.text-field",
            Kind::Table => "ia.display.table",
            Kind::Checkbox => "ia.input.checkbox",
            Kind::Numeric => "ia.input.numeric-entry-field",
            Kind::Dropdown => "ia.input.dropdown",
            Kind::Repeater => "ia.display.flex-repeater",
            Kind::DateTime => "ia.input.date-time-input",
            Kind::Markdown => "ia.display.markdown",
            Kind::MultiState => "ia.input.multi-state-button",
            Kind::Image => "ia.display.image",
            Kind::TextArea => "ia.input.text-area",
            Kind::Chart => "ia.chart.timeseries",
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Kind::Flex | Kind::Coord => "Container",
            Kind::Label => "Label",
            Kind::Svg => "Graphic",
            Kind::Icon => "Icon",
            Kind::View => "View",
            Kind::Button => "Button",
            Kind::TextField => "Input",
            Kind::Table => "Table",
            Kind::Checkbox => "Check",
            Kind::Numeric => "Entry",
            Kind::Dropdown => "Dropdown",
            Kind::Repeater => "Repeater",
            Kind::DateTime => "DatePicker",
            Kind::Markdown => "Notes",
            Kind::MultiState => "Selector",
            Kind::Image => "Image",
            Kind::TextArea => "TextArea",
            Kind::Chart => "Chart",
        }
    }

    fn has_text(self) -> bool {
        matches!(
            self,
            Kind::Label | Kind::Button | Kind::TextField | Kind::TextArea
        )
    }
}

/// Leaf type mix, calibrated on component type counts of a production
/// project.
const LEAVES: &[(Kind, u32)] = &[
    (Kind::Label, 1539),
    (Kind::Svg, 692),
    (Kind::Icon, 271),
    (Kind::View, 227),
    (Kind::Button, 155),
    (Kind::TextField, 72),
    (Kind::Table, 59),
    (Kind::Checkbox, 35),
    (Kind::Numeric, 35),
    (Kind::Dropdown, 32),
    (Kind::Repeater, 28),
    (Kind::DateTime, 20),
    (Kind::Markdown, 10),
    (Kind::MultiState, 9),
    (Kind::Image, 6),
    (Kind::TextArea, 6),
    (Kind::Chart, 6),
];

struct Gen<'w> {
    w: &'w World,
    spec: &'w ViewSpec,
    rng: Rng,
    d: f64,
    param_names: Vec<String>,
    custom_names: Vec<String>,
    used: HashSet<String>,
    stats: Stats,
    logger: String,
}

pub fn generate(w: &World, spec: &ViewSpec) -> (String, Stats) {
    let mut g = Gen {
        w,
        spec,
        rng: w.rng("view", spec.index as u64),
        d: w.cfg.defects,
        param_names: spec.params.iter().map(|p| p.name.clone()).collect(),
        custom_names: Vec::new(),
        used: HashSet::new(),
        stats: Stats {
            views: 1,
            ..Stats::default()
        },
        logger: format!("views.{}", spec.path.replace('/', ".")),
    };
    let value = g.view();
    let text = crate::gson::to_string(&value, true);
    g.stats.bytes = text.len();
    (text, g.stats)
}

impl Gen<'_> {
    fn defect(&mut self, kind: &'static str, p: f64) -> bool {
        if self.rng.chance(p) {
            *self.stats.defects.entry(kind).or_default() += 1;
            true
        } else {
            false
        }
    }

    fn view(&mut self) -> Value {
        // View custom properties: median 1, mean about 2.
        let ncustom = self.rng.lognormal(0.2, 1.0, 0, 26);
        let mut custom = Map::new();
        while self.custom_names.len() < ncustom {
            let n = names::prop_name(&mut self.rng);
            if !self.custom_names.contains(&n) && !self.param_names.contains(&n) {
                custom.insert(n.clone(), self.literal());
                self.custom_names.push(n);
            }
        }
        let mut prop_config = Map::new();
        for p in &self.spec.params {
            prop_config.insert(
                format!("params.{}", p.name),
                json!({"paramDirection": p.direction, "persistent": true}),
            );
        }
        // Seeded: a custom property nobody reads.
        if self.defect("unused-custom-property", self.d) {
            let n = format!("unused{}", names::pascal(&mut self.rng));
            custom.insert(n, json!(""));
        }
        // Seeded: oversized context data.
        if self.defect("excessive-context-data", self.d / 4.0) {
            let rows: Vec<Value> = (0..80)
                .map(|i| json!({"id": i, "label": format!("Row {i}"), "value": i * 3}))
                .collect();
            custom.insert("cachedRows".to_owned(), Value::Array(rows));
            self.custom_names.push("cachedRows".to_owned());
        }
        let names_snapshot = self.custom_names.clone();
        for n in &names_snapshot {
            let mut cfg = Map::new();
            cfg.insert("persistent".to_owned(), json!(true));
            if self.rng.chance(0.5) {
                let b = self.binding(&[], &[], false, n);
                cfg.insert("binding".to_owned(), b);
            }
            if self.rng.chance(0.12) {
                let s = self.script_body(
                    &["previousValue", "currentValue", "origin", "missedEvents"],
                    false,
                    &[],
                    "change-script",
                );
                cfg.insert("onChange".to_owned(), json!({"enabled": true, "script": s}));
            }
            prop_config.insert(format!("custom.{n}"), Value::Object(cfg));
        }
        for p in self.spec.params.clone() {
            if self.rng.chance(0.08)
                && let Some(Value::Object(cfg)) = prop_config.get_mut(&format!("params.{}", p.name))
            {
                let s = self.script_body(
                    &["previousValue", "currentValue", "origin", "missedEvents"],
                    false,
                    &[],
                    "change-script",
                );
                cfg.insert("onChange".to_owned(), json!({"enabled": true, "script": s}));
            }
        }

        let target = match self.spec.role {
            Role::Page => self.rng.lognormal(3.0, 0.8, 4, 400),
            Role::Card => self.rng.lognormal(1.8, 0.8, 1, 60),
            Role::Popup => self.rng.lognormal(2.4, 0.8, 2, 150),
            Role::Dock => self.rng.lognormal(1.8, 0.6, 2, 40),
        };
        let root_kind = match self.spec.role {
            Role::Popup | Role::Card if self.rng.chance(0.35) => Kind::Coord,
            _ => Kind::Flex,
        };
        let mut budget = target.saturating_sub(1);
        let mut root = self.container(root_kind, None, 0, &mut budget, "root".to_owned(), true);

        // Every declared property is read somewhere (unless seeded unused).
        let leftovers: Vec<String> = self
            .param_names
            .iter()
            .map(|p| format!("params.{p}"))
            .chain(self.custom_names.iter().map(|c| format!("custom.{c}")))
            .filter(|k| !self.used.contains(k))
            .collect();
        if !leftovers.is_empty()
            && let Some(Value::Array(children)) = root
                .as_object_mut()
                .map(|r| r.entry("children").or_insert_with(|| json!([])))
        {
            let mut st = Map::new();
            for k in &leftovers {
                let key = k.rsplit('.').next().unwrap_or(k).to_owned();
                st.insert(key, json!(format!("{{view.{k}}}")));
            }
            self.stats.components += 1;
            self.stats.bindings += 1;
            children.push(json!({
                "meta": {"name": "StateSummary", "visible": false},
                "position": if root_kind == Kind::Coord { json!({"height": 20, "width": 120, "x": 0, "y": 0}) } else { json!({"basis": "20px"}) },
                "propConfig": {"props.text": {"binding": {"config": {"struct": st, "waitOnAll": true}, "type": "expr-struct"}}},
                "props": {"text": ""},
                "type": "ia.display.label"
            }));
        }

        let mut view = Map::new();
        view.insert("custom".to_owned(), Value::Object(custom));
        let params: Map<String, Value> = self
            .spec
            .params
            .iter()
            .map(|p| (p.name.clone(), p.default.clone()))
            .collect();
        view.insert("params".to_owned(), Value::Object(params));
        if !prop_config.is_empty() {
            view.insert("propConfig".to_owned(), Value::Object(prop_config));
        }
        let (w, h) = match self.spec.role {
            Role::Page => (1920, 1080),
            Role::Card => (240, 160),
            Role::Popup => (640, 480),
            Role::Dock => (1920, 64),
        };
        view.insert(
            "props".to_owned(),
            json!({"defaultSize": {"height": h, "width": w}}),
        );
        if self.rng.chance(0.06) {
            let s = self.script_body(&["event"], true, &[], "view-event");
            view.insert(
                "events".to_owned(),
                json!({"system": {"onStartup": {"config": {"script": s}, "scope": "G", "type": "script"}}}),
            );
        }
        view.insert("root".to_owned(), root);
        Value::Object(view)
    }

    fn literal(&mut self) -> Value {
        match self.rng.below(5) {
            0 => json!(0),
            1 => json!(false),
            2 => json!(""),
            3 => json!(null),
            _ => json!({"label": "", "value": 0}),
        }
    }

    fn mark(&mut self, refs: &[String]) {
        for r in refs {
            if let Some(rest) = r.strip_prefix("view.") {
                let mut parts = rest.splitn(3, '.');
                if let (Some(a), Some(b)) = (parts.next(), parts.next()) {
                    self.used.insert(format!("{a}.{b}"));
                }
            }
        }
    }

    /// Reference candidates (without braces); unused view properties first.
    fn refs(&mut self, siblings: &[String], own_custom: &[String]) -> Vec<String> {
        let mut out = Vec::new();
        let unused: Vec<String> = self
            .param_names
            .iter()
            .map(|p| format!("params.{p}"))
            .chain(self.custom_names.iter().map(|c| format!("custom.{c}")))
            .filter(|k| !self.used.contains(k))
            .collect();
        if let Some(u) = unused.first() {
            out.push(format!("view.{u}"));
        }
        for p in &self.param_names {
            out.push(format!("view.params.{p}"));
        }
        for c in &self.custom_names {
            out.push(format!("view.custom.{c}"));
        }
        for c in own_custom {
            out.push(format!("this.custom.{c}"));
        }
        if self.rng.chance(0.2) {
            out.push(format!(
                "session.custom.{}",
                self.rng.pick(&self.w.session_custom)
            ));
        }
        if !siblings.is_empty() && self.rng.chance(0.25) {
            out.push(format!("../{}.props.text", self.rng.pick(siblings)));
        }
        if out.is_empty() {
            out.push("session.props.locale".to_owned());
        }
        out
    }

    fn expression(&mut self, siblings: &[String], own: &[String], has_value: bool) -> String {
        let refs = self.refs(siblings, own);
        let defect = if self.defect("expression-syntax", self.d / 4.0) {
            ExprDefect::Syntax
        } else if self.defect("unknown-expression-function", self.d / 4.0) {
            ExprDefect::UnknownFunction
        } else if self.defect("polling-interval", self.d / 4.0) {
            ExprDefect::Polling
        } else if self.defect("unresolved-reference", self.d / 4.0) {
            ExprDefect::UnresolvedRef
        } else {
            ExprDefect::None
        };
        let ctx = ExprCtx {
            refs: &refs,
            has_value,
        };
        let out = expr::expression(&mut self.rng, &ctx, defect);
        self.mark(&out.used);
        out.text
    }

    fn script_body(
        &mut self,
        params: &[&str],
        has_self: bool,
        siblings: &[String],
        kind: &'static str,
    ) -> String {
        let lines = script::sample_lines(&mut self.rng);
        let defect = self.defect("undefined-name", self.d / 2.0);
        let returns = kind == "transform";
        let view_custom = self.custom_names.clone();
        let view_params = self.param_names.clone();
        let logger = self.logger.clone();
        let ctx = ScriptCtx {
            has_self,
            params,
            view_params: &view_params,
            view_custom: &view_custom,
            siblings,
            returns,
            logger: &logger,
        };
        let out = script::body(self.w, &mut self.rng, &ctx, lines, 1, defect);
        for u in out.used {
            self.used.insert(u);
        }
        self.stats.scripts += 1;
        self.stats.script_lines += out.text.lines().count();
        out.text
    }

    /// A binding config with optional transforms. `target` names the bound
    /// property (used for tag member choice only).
    fn binding(
        &mut self,
        siblings: &[String],
        own: &[String],
        bidirectional: bool,
        _target: &str,
    ) -> Value {
        self.stats.bindings += 1;
        let ty = if bidirectional {
            "property"
        } else {
            self.rng.weighted(&[
                ("property", 450),
                ("expr", 310),
                ("tag", 210),
                ("expr-struct", 20),
                ("tag-history", 5),
                ("query", 5),
            ])
        };
        let mut b = Map::new();
        let config = match ty {
            "property" => {
                let refs = self.refs(siblings, own);
                let path = self.rng.pick(&refs).clone();
                self.mark(std::slice::from_ref(&path));
                if bidirectional {
                    json!({"bidirectional": true, "path": path})
                } else {
                    json!({"path": path})
                }
            }
            "expr" => json!({"expression": self.expression(siblings, own, false)}),
            "expr-struct" => {
                let n = self.rng.range(2, 4);
                let mut st = Map::new();
                for i in 0..n {
                    st.insert(
                        format!(
                            "{}{i}",
                            names::lower_first(self.rng.pick::<&str>(names::ASPECTS))
                        ),
                        json!(self.expression(siblings, own, false)),
                    );
                }
                json!({"struct": st, "waitOnAll": true})
            }
            "tag" => {
                let has_tag_param = self.param_names.iter().any(|p| p == "tagPath");
                if has_tag_param && self.rng.chance(0.7) {
                    self.used.insert("params.tagPath".to_owned());
                    let member = *self.rng.pick(crate::world::MOTOR_MEMBERS);
                    if self.rng.chance(0.8) {
                        json!({"fallbackDelay": 2.5, "mode": "indirect", "references": {"0": "{view.params.tagPath}"}, "tagPath": format!("{{0}}/{member}")})
                    } else {
                        json!({"fallbackDelay": 2.5, "mode": "expression", "tagPath": format!("{{view.params.tagPath}} + \"/{member}\"")})
                    }
                } else {
                    json!({"fallbackDelay": 2.5, "mode": "direct", "tagPath": self.w.tag_path(&mut self.rng)})
                }
            }
            "tag-history" => {
                let tags: Vec<Value> = (0..self.rng.range(1, 4))
                    .map(|_| json!({"aggregate": "Average", "alias": names::lower_first(self.rng.pick::<&str>(names::ASPECTS)), "path": self.w.tag_path(&mut self.rng)}))
                    .collect();
                json!({
                    "aggregate": "MinMax",
                    "avoidScanClassValidation": true,
                    "dateRange": {"mostRecent": "8", "mostRecentUnits": "HOUR"},
                    "enableValueCache": true,
                    "ignoreBadQuality": false,
                    "polling": {"enabled": true, "rate": "30"},
                    "preventInterpolation": false,
                    "returnFormat": "Wide",
                    "returnSize": {"delay": 1, "delayUnits": "MIN", "type": "INTERVAL"},
                    "tags": tags,
                    "valueFormat": "DATASET"
                })
            }
            _ => {
                let q = self.rng.pick(&self.w.queries).clone();
                let path = if self.defect("unknown-named-query", self.d) {
                    format!("{}Old", q.path)
                } else {
                    q.path.clone()
                };
                let mut params = Map::new();
                for (n, _) in &q.params {
                    params.insert(n.clone(), json!(self.expression(siblings, own, false)));
                }
                json!({"parameters": params, "polling": {"enabled": false, "rate": "0"}, "queryPath": path, "returnFormat": "json"})
            }
        };
        b.insert("config".to_owned(), config);
        let mut transforms = Vec::new();
        if !bidirectional && self.rng.chance(0.4) {
            let n = if self.rng.chance(0.12) { 2 } else { 1 };
            for _ in 0..n {
                transforms.push(self.transform(siblings, own));
            }
        }
        if !transforms.is_empty() {
            b.insert("transforms".to_owned(), Value::Array(transforms));
        }
        b.insert("type".to_owned(), json!(ty));
        Value::Object(b)
    }

    fn transform(&mut self, siblings: &[String], own: &[String]) -> Value {
        self.stats.transforms += 1;
        match self.rng.weighted(&[(0, 660), (1, 270), (2, 66), (3, 4)]) {
            0 => {
                let style = self.rng.chance(0.4);
                let n = self.rng.range(2, 5);
                let mappings: Vec<Value> = (0..n)
                    .map(|i| {
                        let out = if style {
                            json!(self.style_class_ref())
                        } else {
                            json!(format!("#{:06X}", self.rng.below(0xFFFFFF)))
                        };
                        json!({"input": i, "output": out})
                    })
                    .collect();
                let fallback = if style { json!("") } else { json!("#000000") };
                json!({
                    "fallback": fallback,
                    "inputType": "scalar",
                    "mappings": mappings,
                    "outputType": if style { "style-list" } else { "color" },
                    "type": "map"
                })
            }
            1 => {
                let code = self.script_body(
                    &["value", "quality", "timestamp"],
                    true,
                    siblings,
                    "transform",
                );
                json!({"code": code, "type": "script"})
            }
            2 => json!({"expression": self.expression(siblings, own, true), "type": "expression"}),
            _ => json!({"formatType": "numeric", "formatValue": "#,##0.00", "type": "format"}),
        }
    }

    fn style_class_ref(&mut self) -> String {
        if self.defect("unknown-style-class", self.d / 3.0) {
            format!("Legacy/{}", self.rng.pick(names::STYLE_WORDS))
        } else {
            self.rng.pick(&self.w.style_classes).clone()
        }
    }

    fn view_ref(&mut self, pool: &[usize]) -> Option<&ViewSpec> {
        let w = self.w;
        if pool.is_empty() {
            return None;
        }
        let i = *self.rng.pick(pool);
        if i == self.spec.index {
            return None;
        }
        Some(&w.views[i])
    }

    fn view_path(&mut self, spec: &ViewSpec) -> String {
        if self.defect("unknown-view", self.d / 2.0) {
            format!("{}Old", spec.path)
        } else {
            spec.path.clone()
        }
    }

    fn component_name(&mut self, kind: Kind, taken: &mut HashSet<String>) -> String {
        let base = format!("{}{}", self.rng.pick(names::EQUIPMENT), kind.suffix());
        if self.defect("name-pattern", self.d / 4.0) {
            let bad = format!(
                "{}_{}",
                names::lower_first(self.rng.pick::<&str>(names::EQUIPMENT)),
                kind.suffix().to_lowercase()
            );
            taken.insert(bad.clone());
            return bad;
        }
        if self.defect("duplicate-component-name", self.d / 20.0) && !taken.is_empty() {
            return taken.iter().min().cloned().unwrap_or(base);
        }
        let mut name = base.clone();
        let mut i = 1;
        while !taken.insert(name.clone()) {
            i += 1;
            name = format!("{base}{i}");
        }
        name
    }

    fn position(&mut self, parent: Option<Kind>) -> Option<Value> {
        match parent {
            Some(Kind::Coord) => Some(json!({
                "height": 20 * self.rng.range(1, 12),
                "width": 20 * self.rng.range(2, 30),
                "x": 10 * self.rng.below(80),
                "y": 10 * self.rng.below(60)
            })),
            Some(_) => {
                if self.rng.chance(0.3) {
                    Some(json!({"basis": format!("{}px", 20 * self.rng.range(1, 20))}))
                } else if self.rng.chance(0.5) {
                    Some(json!({"grow": 1, "shrink": 0}))
                } else {
                    Some(json!({"basis": "auto"}))
                }
            }
            None => None,
        }
    }

    fn container(
        &mut self,
        kind: Kind,
        parent: Option<Kind>,
        depth: usize,
        budget: &mut usize,
        name: String,
        root: bool,
    ) -> Value {
        self.stats.components += 1;
        let mut c = Map::new();
        c.insert("type".to_owned(), json!(kind.type_id()));
        c.insert("meta".to_owned(), json!({"name": name}));
        if let Some(p) = self.position(parent) {
            c.insert("position".to_owned(), p);
        }
        let mut props = Map::new();
        if kind == Kind::Flex {
            props.insert(
                "direction".to_owned(),
                json!(if self.rng.chance(0.6) {
                    "column"
                } else {
                    "row"
                }),
            );
            if self.rng.chance(0.3) {
                props.insert("justify".to_owned(), json!("space-between"));
            }
        }
        if self.rng.chance(0.35) {
            let cls = self.style_class_ref();
            props.insert("style".to_owned(), json!({"classes": cls}));
        }
        c.insert("props".to_owned(), Value::Object(props));
        if !root && self.rng.chance(0.25) {
            let b = self.binding(&[], &[], false, "visible");
            c.insert(
                "propConfig".to_owned(),
                json!({"meta.visible": {"binding": b}}),
            );
        }
        if !root && self.rng.chance(0.08) {
            let a = self.action(&[]);
            c.insert("events".to_owned(), json!({"dom": {"onClick": a}}));
        }

        // Children: median 2, mean about 3.7, long tail.
        let want = if root {
            (*budget).min(self.rng.lognormal(1.5, 0.7, 1, 40).max(2))
        } else {
            (*budget).min(self.rng.lognormal(0.9, 0.7, 1, 150))
        };
        let mut children = Vec::new();
        let mut taken = HashSet::new();
        let mut text_siblings: Vec<String> = Vec::new();
        while *budget > 0 && (root || children.len() < want) {
            *budget -= 1;
            let p_nested = if root && *budget > 3 * want.max(1) {
                0.6
            } else if depth < 2 {
                0.3
            } else {
                0.15
            };
            let nested = depth < 7 && *budget > 2 && self.rng.chance(p_nested);
            let child = if nested {
                let ck = if self.rng.chance(0.84) {
                    Kind::Flex
                } else {
                    Kind::Coord
                };
                let n = self.component_name(ck, &mut taken);
                self.container(ck, Some(kind), depth + 1, budget, n, false)
            } else {
                let lk = self.rng.weighted(LEAVES);
                let n = self.component_name(lk, &mut taken);
                let v = self.leaf(lk, kind, &n, &text_siblings);
                if lk.has_text() {
                    text_siblings.push(n);
                }
                v
            };
            children.push(child);
        }
        if !children.is_empty() {
            c.insert("children".to_owned(), Value::Array(children));
        }
        if self.rng.chance(if root { 0.15 } else { 0.03 }) {
            // Scripts on a container reach its children with getChild, not
            // getSibling; keep them free of tree navigation.
            self.add_scripts(&mut c, &[]);
        }
        Value::Object(c)
    }

    fn add_scripts(&mut self, c: &mut Map<String, Value>, siblings: &[String]) {
        let mut handlers = Vec::new();
        let mut methods = Vec::new();
        if self.rng.chance(0.75) {
            let s = self.script_body(&["payload"], true, siblings, "message-handler");
            let listens = !self.defect("handler-without-scope", self.d / 4.0);
            handlers.push(json!({
                "messageType": self.rng.pick(names::MESSAGES),
                "pageScope": listens,
                "script": s,
                "sessionScope": false,
                "viewScope": false
            }));
        }
        if handlers.is_empty() || self.rng.chance(0.3) {
            let name = format!(
                "{}{}",
                self.rng.pick(names::VERBS),
                self.rng.pick(names::NOUNS)
            );
            let n = self.rng.below(3);
            let params: Vec<String> = (0..n).map(|i| format!("arg{i}")).collect();
            let refs: Vec<&str> = params.iter().map(String::as_str).collect();
            let s = self.script_body(&refs, true, siblings, "custom-method");
            methods.push(json!({"name": name, "params": params, "script": s}));
        }
        c.insert(
            "scripts".to_owned(),
            json!({"customMethods": methods, "extensionFunctions": null, "messageHandlers": handlers}),
        );
    }

    fn action(&mut self, siblings: &[String]) -> Value {
        match self.rng.weighted(&[(0, 69), (1, 27), (2, 4)]) {
            0 => {
                let s = self.script_body(&["event"], true, siblings, "event-script");
                json!({"config": {"script": s}, "scope": "G", "type": "script"})
            }
            1 => {
                let pool = self.w.popups.clone();
                match self.view_ref(&pool).cloned() {
                    Some(target) => {
                        let path = self.view_path(&target);
                        let mut vp = Map::new();
                        for p in &target.params {
                            if p.name == "tagPath" {
                                vp.insert(
                                    p.name.clone(),
                                    json!(self.w.equipment_path(&mut self.rng).0),
                                );
                            } else if self.rng.chance(0.5) {
                                vp.insert(p.name.clone(), p.default.clone());
                            }
                        }
                        let id =
                            names::lower_first(target.path.rsplit('/').next().unwrap_or("popup"));
                        json!({"config": {
                            "draggable": true, "id": id, "modal": self.rng.chance(0.5), "overlayDismiss": true,
                            "position": {"height": 480, "width": 640}, "positionType": "exact", "resizable": true,
                            "showCloseIcon": true, "title": "", "type": "open", "viewParams": vp, "viewPath": path,
                            "viewportBound": false
                        }, "scope": "C", "type": "popup"})
                    }
                    None => {
                        json!({"config": {"type": "close", "id": ""}, "scope": "C", "type": "popup"})
                    }
                }
            }
            _ => {
                let page = self.rng.pick(&self.w.pages).url.replace(":lineId", "1");
                json!({"config": {"page": page}, "scope": "C", "type": "nav"})
            }
        }
    }

    fn leaf(&mut self, kind: Kind, parent: Kind, name: &str, siblings: &[String]) -> Value {
        self.stats.components += 1;
        let mut c = Map::new();
        c.insert("type".to_owned(), json!(kind.type_id()));
        c.insert("meta".to_owned(), json!({"name": name}));
        if let Some(p) = self.position(Some(parent)) {
            c.insert("position".to_owned(), p);
        }
        let mut own_custom = Vec::new();
        if self.rng.chance(0.04) {
            let n = names::prop_name(&mut self.rng);
            c.insert("custom".to_owned(), json!({n.clone(): 0}));
            own_custom.push(n);
        }
        let mut props = Map::new();
        let mut pc = Map::new();
        let mut events = Map::new();
        let bind_p = |s: &mut Self, p: f64| s.rng.chance(p);
        match kind {
            Kind::Label => {
                props.insert("text".to_owned(), json!(self.rng.pick(names::ASPECTS)));
                if bind_p(self, 0.75) {
                    let b = self.binding(siblings, &own_custom, false, "text");
                    pc.insert("props.text".to_owned(), json!({"binding": b}));
                }
                if bind_p(self, 0.12) {
                    let b = self.binding(siblings, &own_custom, false, "classes");
                    pc.insert("props.style.classes".to_owned(), json!({"binding": b}));
                } else if bind_p(self, 0.3) {
                    let cls = self.style_class_ref();
                    props.insert("style".to_owned(), json!({"classes": cls}));
                }
                if bind_p(self, 0.12) {
                    let a = self.action(siblings);
                    events.insert("dom".to_owned(), json!({"onClick": a}));
                }
            }
            Kind::Svg => {
                let n = self.rng.range(2, 14);
                let elements: Vec<Value> = (0..n)
                    .map(|i| {
                        let mut d = String::from("M 0 0");
                        for _ in 0..self.rng.range(4, 24) {
                            d.push_str(&format!(" L {} {}", self.rng.below(200), self.rng.below(200)));
                        }
                        d.push_str(" Z");
                        json!({"d": d, "fill": {"paint": format!("#{:06X}", self.rng.below(0xFFFFFF))}, "name": format!("path{i}"), "stroke": {"paint": "#000000", "width": 1}, "type": "path"})
                    })
                    .collect();
                props.insert("elements".to_owned(), Value::Array(elements));
                props.insert("viewBox".to_owned(), json!("0 0 200 200"));
                if bind_p(self, 0.35) {
                    let b = self.binding(siblings, &own_custom, false, "fill");
                    pc.insert(
                        "props.elements[0].fill.paint".to_owned(),
                        json!({"binding": b}),
                    );
                }
            }
            Kind::Icon => {
                props.insert("path".to_owned(), json!(self.rng.pick(names::ICONS)));
                props.insert("color".to_owned(), json!("#4A4A4A"));
                if bind_p(self, 0.4) {
                    let b = self.binding(siblings, &own_custom, false, "color");
                    pc.insert("props.color".to_owned(), json!({"binding": b}));
                }
                if bind_p(self, 0.3) {
                    let a = self.action(siblings);
                    events.insert("dom".to_owned(), json!({"onClick": a}));
                }
            }
            Kind::View => {
                let pool = self.w.cards.clone();
                if let Some(target) = self.view_ref(&pool).cloned() {
                    let path = self.view_path(&target);
                    props.insert("path".to_owned(), json!(path));
                    let mut params = Map::new();
                    for p in &target.params {
                        let v = if p.name == "tagPath" {
                            json!(self.w.equipment_path(&mut self.rng).0)
                        } else if p.name == "title" {
                            json!(self.rng.pick(names::EQUIPMENT))
                        } else {
                            p.default.clone()
                        };
                        params.insert(p.name.clone(), v);
                    }
                    if self.defect("unknown-view-param", self.d / 4.0) {
                        params.insert("legacyMode".to_owned(), json!(true));
                    }
                    if let Some(first) = target.params.first()
                        && bind_p(self, 0.4)
                    {
                        let b = self.binding(siblings, &own_custom, false, "param");
                        pc.insert(
                            format!("props.params.{}", first.name),
                            json!({"binding": b}),
                        );
                    }
                    props.insert("params".to_owned(), Value::Object(params));
                }
            }
            Kind::Button => {
                props.insert("text".to_owned(), json!(self.rng.pick(names::ASPECTS)));
                let a = self.action(siblings);
                events.insert("component".to_owned(), json!({"onActionPerformed": a}));
                if bind_p(self, 0.3) {
                    let b = self.binding(siblings, &own_custom, false, "enabled");
                    pc.insert("props.enabled".to_owned(), json!({"binding": b}));
                }
            }
            Kind::TextField
            | Kind::TextArea
            | Kind::Numeric
            | Kind::Checkbox
            | Kind::Dropdown
            | Kind::DateTime
            | Kind::MultiState => {
                let (prop, default) = match kind {
                    Kind::TextField | Kind::TextArea => ("text", json!("")),
                    Kind::Checkbox => ("selected", json!(false)),
                    Kind::MultiState => ("controlValue", json!(0)),
                    _ => ("value", json!(null)),
                };
                props.insert(prop.to_owned(), default);
                if kind == Kind::Dropdown {
                    let opts: Vec<Value> = (0..self.rng.range(2, 10))
                        .map(|i| json!({"label": format!("{} {i}", self.rng.pick(names::EQUIPMENT)), "value": i}))
                        .collect();
                    props.insert("options".to_owned(), Value::Array(opts));
                }
                if bind_p(self, 0.6) {
                    let b = self.binding(siblings, &own_custom, true, prop);
                    pc.insert(format!("props.{prop}"), json!({"binding": b}));
                }
            }
            Kind::Table => {
                let cols: Vec<&str> = (0..self.rng.range(3, 8))
                    .map(|_| *self.rng.pick(names::PROP_WORDS))
                    .collect();
                let rows: Vec<Value> = (0..self.rng.range(0, 30))
                    .map(|i| {
                        let mut r = Map::new();
                        for c in &cols {
                            r.insert((*c).to_owned(), json!(i * 7 % 13));
                        }
                        Value::Object(r)
                    })
                    .collect();
                props.insert("data".to_owned(), Value::Array(rows));
                let columns: Vec<Value> = cols
                    .iter()
                    .map(|c| json!({"field": c, "header": {"title": c}, "sortable": true, "visible": true}))
                    .collect();
                props.insert("columns".to_owned(), Value::Array(columns));
                if bind_p(self, 0.7) {
                    let b = self.binding(siblings, &own_custom, false, "data");
                    pc.insert("props.data".to_owned(), json!({"binding": b}));
                }
                if bind_p(self, 0.4) {
                    let s = self.script_body(&["event"], true, siblings, "event-script");
                    events.insert("component".to_owned(), json!({"onRowClick": {"config": {"script": s}, "scope": "G", "type": "script"}}));
                }
            }
            Kind::Repeater => {
                let pool = self.w.cards.clone();
                if let Some(target) = self.view_ref(&pool).cloned() {
                    let path = self.view_path(&target);
                    props.insert("path".to_owned(), json!(path));
                    let instances: Vec<Value> = (0..self.rng.range(2, 12))
                        .map(|_| {
                            let mut m = Map::new();
                            for p in &target.params {
                                let v = if p.name == "tagPath" {
                                    json!(self.w.equipment_path(&mut self.rng).0)
                                } else {
                                    p.default.clone()
                                };
                                m.insert(p.name.clone(), v);
                            }
                            Value::Object(m)
                        })
                        .collect();
                    props.insert("instances".to_owned(), Value::Array(instances));
                    if bind_p(self, 0.4) {
                        let b = self.binding(siblings, &own_custom, false, "instances");
                        pc.insert("props.instances".to_owned(), json!({"binding": b}));
                    }
                }
            }
            Kind::Markdown => {
                props.insert(
                    "source".to_owned(),
                    json!("## Notes\n\nOperator notes for this area."),
                );
            }
            Kind::Image => {
                props.insert("source".to_owned(), json!("/system/images/logo.png"));
                if !self.defect("image-without-alt", self.d) {
                    props.insert("alt".to_owned(), json!("Logo"));
                }
            }
            Kind::Chart => {
                if bind_p(self, 0.8) {
                    let b = self.binding(siblings, &own_custom, false, "series");
                    pc.insert("props.series[0].data".to_owned(), json!({"binding": b}));
                }
            }
            Kind::Flex | Kind::Coord => {}
        }
        if bind_p(self, 0.15) {
            let b = self.binding(siblings, &own_custom, false, "visible");
            pc.insert("meta.visible".to_owned(), json!({"binding": b}));
        }
        for n in &own_custom {
            let used = pc
                .values()
                .any(|v| v.to_string().contains(&format!("this.custom.{n}")));
            if !used {
                let mut cfg = Map::new();
                let b = json!({"config": {"path": format!("this.custom.{n}")}, "type": "property"});
                self.stats.bindings += 1;
                cfg.insert("binding".to_owned(), b);
                pc.insert("props.style.opacity".to_owned(), Value::Object(cfg));
            }
        }
        c.insert("props".to_owned(), Value::Object(props));
        if !pc.is_empty() {
            c.insert("propConfig".to_owned(), Value::Object(pc));
        }
        if !events.is_empty() {
            c.insert("events".to_owned(), Value::Object(events));
        }
        if self.rng.chance(0.015) {
            self.add_scripts(&mut c, siblings);
        }
        Value::Object(c)
    }
}
