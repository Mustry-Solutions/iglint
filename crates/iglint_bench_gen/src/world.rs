//! Corpus-wide facts decided before any file is written: views and their
//! parameters, the script library, named queries, style classes, tags.
//! Per-file generators draw references from here so that, apart from seeded
//! defects, every reference resolves.

use std::collections::HashSet;

use serde_json::{Value, json};

use crate::names::{self, lower_first};
use crate::rng::Rng;

#[derive(Clone, Debug)]
pub struct Config {
    pub views: usize,
    pub seed: u64,
    /// Probability that a defect opportunity becomes a seeded defect.
    pub defects: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Page,
    Card,
    Popup,
    Dock,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub name: String,
    pub default: Value,
    pub direction: &'static str,
}

#[derive(Clone, Debug)]
pub struct ViewSpec {
    pub index: usize,
    pub path: String,
    pub role: Role,
    pub params: Vec<Param>,
}

#[derive(Clone, Debug)]
pub struct LibFunc {
    /// Dotted module name, `core.format`.
    pub module: String,
    pub name: String,
    pub params: Vec<String>,
}

impl LibFunc {
    pub fn qualified(&self) -> String {
        format!("{}.{}", self.module, self.name)
    }
}

#[derive(Clone, Debug)]
pub struct LibModule {
    pub path: Vec<String>,
    pub functions: Vec<LibFunc>,
}

#[derive(Clone, Debug)]
pub struct NamedQuery {
    pub path: String,
    pub kind: &'static str,
    pub params: Vec<(String, u8)>,
}

#[derive(Clone, Debug)]
pub struct Page {
    pub url: String,
    pub view: usize,
}

pub struct World {
    pub cfg: Config,
    pub views: Vec<ViewSpec>,
    pub modules: Vec<LibModule>,
    pub functions: Vec<LibFunc>,
    pub queries: Vec<NamedQuery>,
    pub style_classes: Vec<String>,
    pub lines: usize,
    pub pages: Vec<Page>,
    pub session_custom: Vec<String>,
    pub cards: Vec<usize>,
    pub popups: Vec<usize>,
    pub docks: Vec<usize>,
}

pub const MOTOR_MEMBERS: &[&str] = &["Speed", "Current", "Running", "Fault", "Hours", "Mode"];
pub const VALVE_MEMBERS: &[&str] = &["Open", "Closed", "Position", "Fault"];
pub const PUMP_MEMBERS: &[&str] = &["Flow", "Pressure", "Running", "Fault"];
pub const LINE_TAGS: &[&str] = &["State", "Good", "Bad", "Total", "Rate", "Target", "Order"];
pub const MOTORS_PER_LINE: usize = 6;
pub const VALVES_PER_LINE: usize = 4;
pub const PUMPS_PER_LINE: usize = 2;

impl World {
    pub fn new(cfg: Config) -> World {
        let mut rng = Rng::derive(cfg.seed, "world", 0);
        let n = cfg.views.max(4);

        // Views: roles calibrated on typical HMI projects (many reusable
        // cards, popups, fewer pages and docks).
        let mut used_paths = HashSet::new();
        let mut views = Vec::with_capacity(n);
        for index in 0..n {
            let role = if index < 3 {
                Role::Page
            } else if index < 5 {
                Role::Dock
            } else {
                rng.weighted(&[(Role::Card, 52), (Role::Popup, 26), (Role::Page, 18), (Role::Dock, 4)])
            };
            let folder = match role {
                Role::Page => format!("Pages/Area{}", 1 + index % (1 + n / 60)),
                Role::Card => format!("Components/{}", rng.pick(names::EQUIPMENT)),
                Role::Popup => format!("Popups/{}", rng.pick(names::EQUIPMENT)),
                Role::Dock => "Docks".to_owned(),
            };
            let mut name = names::pascal(&mut rng);
            if !used_paths.insert(format!("{folder}/{name}")) {
                name = format!("{name}{index}");
                used_paths.insert(format!("{folder}/{name}"));
            }
            let params = view_params(&mut rng, role);
            views.push(ViewSpec {
                index,
                path: format!("{folder}/{name}"),
                role,
                params,
            });
        }
        let by_role = |r: Role| -> Vec<usize> {
            views.iter().filter(|v| v.role == r).map(|v| v.index).collect()
        };
        let cards = by_role(Role::Card);
        let popups = by_role(Role::Popup);
        let docks = by_role(Role::Dock);

        // Script library: about 0.75 modules per view in small projects,
        // levelling off for very large ones.
        let module_count = ((n as f64 * 0.75) as usize).clamp(8, 1500);
        let package_count = (module_count / 12).clamp(3, names::PACKAGES.len());
        let top_packages: Vec<String> = names::PACKAGES[..package_count]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let mut modules = Vec::with_capacity(module_count);
        let mut functions = Vec::new();
        let mut module_paths = HashSet::new();
        for index in 0..module_count {
            let pkg = top_packages[index % top_packages.len()].clone();
            let mut path = vec![pkg];
            if rng.chance(0.35) {
                path.push(names::lower_first(rng.pick(names::EQUIPMENT)));
            }
            let mut leaf = rng.pick(names::MODULE_WORDS).to_string();
            if !module_paths.insert(format!("{}/{leaf}", path.join("/"))) {
                leaf = format!("{leaf}{index}");
                module_paths.insert(format!("{}/{leaf}", path.join("/")));
            }
            path.push(leaf);
            let dotted = path.join(".");
            let fcount = rng.lognormal(2.3, 0.7, 1, 60);
            let mut seen = HashSet::new();
            let mut funcs = Vec::new();
            for _ in 0..fcount {
                let name = format!("{}{}", rng.pick(names::VERBS), rng.pick(names::NOUNS));
                if !seen.insert(name.clone()) {
                    continue;
                }
                let arity = rng.weighted(&[(0usize, 10), (1, 35), (2, 35), (3, 15), (4, 5)]);
                let mut params: Vec<String> = Vec::new();
                while params.len() < arity {
                    let p = lower_first(rng.pick(names::NOUNS));
                    if !params.contains(&p) {
                        params.push(p);
                    }
                }
                funcs.push(LibFunc {
                    module: dotted.clone(),
                    name,
                    params,
                });
            }
            functions.extend(funcs.iter().cloned());
            modules.push(LibModule {
                path,
                functions: funcs,
            });
        }

        let query_count = (n / 5).clamp(6, 800);
        let mut queries = Vec::with_capacity(query_count);
        let mut qseen = HashSet::new();
        for i in 0..query_count {
            let folder = rng.pick(names::EQUIPMENT);
            let mut name = format!("{}{}", capital(rng.pick(names::VERBS)), rng.pick(names::NOUNS));
            if !qseen.insert(format!("{folder}/{name}")) {
                name = format!("{name}{i}");
            }
            let kind = rng.weighted(&[("Query", 70), ("ScalarQuery", 15), ("UpdateQuery", 15)]);
            let pcount = rng.weighted(&[(0usize, 15), (1, 40), (2, 30), (3, 15)]);
            let mut params: Vec<(String, u8)> = Vec::new();
            while params.len() < pcount {
                let p = names::prop_name(&mut rng);
                if params.iter().all(|(q, _)| *q != p) {
                    params.push((p, *rng.pick(&[2u8, 3, 5, 7, 8])));
                }
            }
            queries.push(NamedQuery {
                path: format!("{folder}/{name}"),
                kind,
                params,
            });
        }

        let style_count = ((n as f64 * 0.4) as usize).clamp(12, 500);
        let mut style_classes = Vec::with_capacity(style_count);
        let mut sseen = HashSet::new();
        for i in 0..style_count {
            let mut s = format!("{}/{}", rng.pick(names::STYLE_GROUPS), rng.pick(names::STYLE_WORDS));
            if !sseen.insert(s.clone()) {
                s = format!("{s}{i}");
            }
            style_classes.push(s);
        }

        let lines = (n / 20).clamp(2, 300);
        let pages = views
            .iter()
            .filter(|v| v.role == Role::Page)
            .enumerate()
            .map(|(i, v)| Page {
                url: if i == 0 {
                    "/".to_owned()
                } else if v.params.iter().any(|p| p.name == "lineId") {
                    format!("/area/{i}/:lineId")
                } else {
                    format!("/area/{i}")
                },
                view: v.index,
            })
            .collect();
        let session_custom = vec![
            "selectedLine".to_owned(),
            "selectedOrder".to_owned(),
            "unitSystem".to_owned(),
            "shiftStart".to_owned(),
            "darkMode".to_owned(),
        ];

        World {
            cfg,
            views,
            modules,
            functions,
            queries,
            style_classes,
            lines,
            pages,
            session_custom,
            cards,
            popups,
            docks,
        }
    }

    pub fn rng(&self, stream: &str, index: u64) -> Rng {
        Rng::derive(self.cfg.seed, stream, index)
    }

    /// A tag path that exists in the generated tag provider.
    pub fn tag_path(&self, rng: &mut Rng) -> String {
        format!("[default]{}", self.tag_relative(rng))
    }

    pub fn equipment_path(&self, rng: &mut Rng) -> (String, &'static [&'static str]) {
        let line = 1 + rng.below(self.lines);
        match rng.weighted(&[(0, 60), (1, 25), (2, 15)]) {
            0 => (
                format!("[default]Plant/Line{line}/Motor{}", 1 + rng.below(MOTORS_PER_LINE)),
                MOTOR_MEMBERS,
            ),
            1 => (
                format!("[default]Plant/Line{line}/Valve{}", 1 + rng.below(VALVES_PER_LINE)),
                VALVE_MEMBERS,
            ),
            _ => (
                format!("[default]Plant/Line{line}/Pump{}", 1 + rng.below(PUMPS_PER_LINE)),
                PUMP_MEMBERS,
            ),
        }
    }

    fn tag_relative(&self, rng: &mut Rng) -> String {
        let line = 1 + rng.below(self.lines);
        if rng.chance(0.3) {
            format!("Plant/Line{line}/{}", rng.pick(LINE_TAGS))
        } else {
            let (p, members) = self.equipment_path(rng);
            let p = p.trim_start_matches("[default]").to_owned();
            format!("{p}/{}", rng.pick(members))
        }
    }
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn view_params(rng: &mut Rng, role: Role) -> Vec<Param> {
    let mut params = Vec::new();
    let push = |params: &mut Vec<Param>, name: &str, default: Value, direction| {
        if params.iter().all(|p: &Param| p.name != name) {
            params.push(Param {
                name: name.to_owned(),
                default,
                direction,
            });
        }
    };
    match role {
        Role::Card => {
            push(&mut params, "tagPath", json!(""), "input");
            push(&mut params, "title", json!("Equipment"), "input");
            if rng.chance(0.3) {
                push(&mut params, "selected", json!(false), "inout");
            }
        }
        Role::Popup => {
            push(&mut params, "tagPath", json!(""), "input");
            if rng.chance(0.5) {
                push(&mut params, "lineId", json!(1), "input");
            }
        }
        Role::Page => {
            if rng.chance(0.5) {
                push(&mut params, "lineId", json!(1), "input");
            }
        }
        Role::Dock => {}
    }
    // Extra parameters: median two per view overall.
    let extra = rng.lognormal(0.3, 0.8, 0, 12).saturating_sub(1);
    for _ in 0..extra {
        let name = names::prop_name(rng);
        let default = match rng.below(4) {
            0 => json!(0),
            1 => json!(false),
            2 => json!(""),
            _ => json!("Line1"),
        };
        push(&mut params, &name, default, "input");
    }
    params
}
