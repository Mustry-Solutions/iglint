//! Jython 2.7 code generator: function bodies for Perspective scripts and
//! the project script library. Bodies use only defined names (header
//! parameters, locals, `system`, top-level library packages and builtins),
//! except for seeded undefined-name defects.

use crate::names;
use crate::rng::Rng;
use crate::world::World;

/// What the body may refer to.
pub struct ScriptCtx<'a> {
    /// `self` is a component or view.
    pub has_self: bool,
    /// Header parameters other than `self`.
    pub params: &'a [&'a str],
    pub view_params: &'a [String],
    pub view_custom: &'a [String],
    /// Sibling component names reachable with `getSibling`.
    pub siblings: &'a [String],
    /// The body must end with `return`.
    pub returns: bool,
    /// Logger name used by `system.util.getLogger`.
    pub logger: &'a str,
}

pub struct Output {
    pub text: String,
    /// `params.x` / `custom.y` of the view read by the script.
    pub used: Vec<String>,
}

const LOCAL_NAMES: &[&str] = &[
    "value",
    "result",
    "rows",
    "data",
    "state",
    "count",
    "total",
    "label",
    "path",
    "payload",
    "items",
    "limits",
    "target",
    "status",
    "message",
    "selection",
    "order",
    "speed",
    "line",
    "unit",
    "text",
    "color",
    "duration",
    "params",
    "record",
    "entry",
    "names",
    "values",
];

const UNDEFINED: &[&str] = &[
    "resutl",
    "tagValu",
    "selectedRw",
    "lineNumbr",
    "dataSt",
    "curentValue",
];

struct Gen<'a, 'w> {
    w: &'w World,
    rng: &'a mut Rng,
    ctx: &'a ScriptCtx<'a>,
    lines: Vec<String>,
    locals: Vec<String>,
    /// Locals assigned but not read yet; reads prefer these so that the
    /// code has few unused variables, like real scripts.
    unread: Vec<String>,
    used: Vec<String>,
}

pub fn body(
    w: &World,
    rng: &mut Rng,
    ctx: &ScriptCtx<'_>,
    target_lines: usize,
    base_indent: usize,
    defect: bool,
) -> Output {
    let mut g = Gen {
        w,
        rng,
        ctx,
        lines: Vec::new(),
        locals: ctx.params.iter().map(|s| (*s).to_owned()).collect(),
        unread: Vec::new(),
        used: Vec::new(),
    };
    let budget = target_lines.max(1) - usize::from(ctx.returns);
    let defect_at = if defect {
        g.rng.below(budget.max(1))
    } else {
        usize::MAX
    };
    let mut defect_done = false;
    while g.lines.len() < budget {
        if !defect_done && g.lines.len() >= defect_at {
            let name = g.undefined();
            let var = g.fresh();
            g.emit(base_indent, format!("{var} = {name}"));
            g.define(var);
            defect_done = true;
            continue;
        }
        g.stmt(base_indent, 0, budget);
    }
    if !defect_done && defect {
        let name = g.undefined();
        g.emit(base_indent, format!("system.perspective.print({name})"));
    }
    if ctx.returns {
        let v = g.value_expr();
        g.emit(base_indent, format!("return {v}"));
    }
    let unread: Vec<String> = g
        .unread
        .iter()
        .filter(|u| g.locals.contains(u))
        .cloned()
        .collect();
    if !unread.is_empty() && !ctx.returns {
        let logger = ctx.logger;
        g.emit(
            base_indent,
            format!(
                "system.util.getLogger(\"{logger}\").debug(\"done: %s\" % ([{}],))",
                unread.join(", ")
            ),
        );
    } else if !unread.is_empty() {
        // Read them before the return statement.
        let ret = g.lines.pop().unwrap_or_default();
        let logger = ctx.logger;
        g.emit(
            base_indent,
            format!(
                "system.util.getLogger(\"{logger}\").debug(\"done: %s\" % ([{}],))",
                unread.join(", ")
            ),
        );
        g.lines.push(ret);
    }
    Output {
        text: g.lines.join("\n"),
        used: g.used,
    }
}

/// Number of lines of a Perspective script body, calibrated on a
/// production project: median 4, p90 about 18, long tail to 120.
pub fn sample_lines(rng: &mut Rng) -> usize {
    let bucket = rng.weighted(&[
        (0usize, 535),
        (5, 234),
        (10, 68),
        (15, 76),
        (20, 35),
        (25, 17),
        (30, 12),
        (40, 8),
        (55, 9),
        (70, 4),
        (100, 3),
    ]);
    let width = match bucket {
        0 => 4,
        30 => 10,
        40 | 55 => 15,
        70 => 30,
        100 => 20,
        _ => 5,
    };
    (bucket + rng.below(width)).max(1)
}

impl Gen<'_, '_> {
    fn emit(&mut self, indent: usize, line: String) {
        self.lines.push(format!("{}{line}", "\t".repeat(indent)));
    }

    fn fresh(&mut self) -> String {
        let base = *self.rng.pick(LOCAL_NAMES);
        if !self.locals.iter().any(|l| l == base) {
            return base.to_owned();
        }
        for i in 2.. {
            let n = format!("{base}{i}");
            if !self.locals.contains(&n) {
                return n;
            }
        }
        unreachable!()
    }

    fn undefined(&mut self) -> String {
        loop {
            let n = *self.rng.pick(UNDEFINED);
            if !self.locals.iter().any(|l| l == n) {
                return n.to_owned();
            }
        }
    }

    fn local(&mut self) -> Option<String> {
        self.unread.retain(|u| self.locals.contains(u));
        if !self.unread.is_empty() && self.rng.chance(0.85) {
            let i = self.rng.below(self.unread.len());
            return Some(self.unread.swap_remove(i));
        }
        if self.locals.is_empty() {
            None
        } else {
            let l = self.rng.pick(&self.locals).clone();
            self.unread.retain(|u| *u != l);
            Some(l)
        }
    }

    /// Leaves a block: locals assigned in it and never read are read once
    /// (logged), then the outer scope's locals are restored.
    fn restore(&mut self, ind: usize, saved: Vec<String>) {
        let pending: Vec<String> = self
            .unread
            .iter()
            .filter(|u| !saved.contains(u) && self.locals.contains(u))
            .cloned()
            .collect();
        if !pending.is_empty() {
            let logger = self.ctx.logger;
            self.emit(
                ind,
                format!(
                    "system.util.getLogger(\"{logger}\").debug(\"%s\" % ([{}],))",
                    pending.join(", ")
                ),
            );
        }
        self.unread.retain(|u| saved.contains(u));
        self.locals = saved;
    }

    fn define(&mut self, name: String) {
        self.unread.push(name.clone());
        self.locals.push(name);
    }

    fn string_lit(&mut self) -> String {
        let w = self.rng.pick(names::ASPECTS);
        format!("\"{w}\"")
    }

    /// A simple expression using defined names.
    fn value_expr(&mut self) -> String {
        match self.rng.below(7) {
            0..=2 => self.local().unwrap_or_else(|| "None".to_owned()),
            3 => format!("{}", self.rng.below(100)),
            4 => self.string_lit(),
            5 => match self.local() {
                Some(l) => format!("str({l})"),
                None => "0".to_owned(),
            },
            _ => match (self.local(), self.local()) {
                (Some(a), Some(b)) if a != b => format!("\"%s / %s\" % ({a}, {b})"),
                (Some(a), _) => format!("{a} is not None"),
                _ => "True".to_owned(),
            },
        }
    }

    fn view_prop_read(&mut self) -> Option<String> {
        if !self.ctx.has_self {
            return None;
        }
        let pick_params = !self.ctx.view_params.is_empty()
            && (self.ctx.view_custom.is_empty() || self.rng.chance(0.6));
        if pick_params {
            let p = self.rng.pick(self.ctx.view_params).clone();
            self.used.push(format!("params.{p}"));
            Some(format!("self.view.params.{p}"))
        } else if !self.ctx.view_custom.is_empty() {
            let c = self.rng.pick(self.ctx.view_custom).clone();
            self.used.push(format!("custom.{c}"));
            Some(format!("self.view.custom.{c}"))
        } else {
            None
        }
    }

    fn lib_call(&mut self) -> String {
        let f = self.rng.pick(&self.w.functions).clone();
        let args: Vec<String> = f.params.iter().map(|_| self.value_expr()).collect();
        format!("{}({})", f.qualified(), args.join(", "))
    }

    fn stmt(&mut self, ind: usize, depth: usize, budget: usize) {
        let remaining = budget.saturating_sub(self.lines.len());
        let kind = self.rng.weighted(&[
            (0, 18),                                              // read view prop
            (1, 18),                                              // library call
            (2, 8),                                               // tag read
            (3, 4),                                               // tag write
            (4, 8),                                               // logger
            (5, 6),                                               // send message
            (6, 3),                                               // popup
            (7, 4),                                               // named query loop
            (8, if remaining > 3 && depth < 3 { 14 } else { 0 }), // if/else
            (9, if remaining > 4 && depth < 2 { 5 } else { 0 }),  // try/except
            (10, 7),                                              // write a prop
            (11, 4),                                              // dates
            (12, 4),                                              // comprehension
            (13, 3),                                              // sibling access
        ]);
        match kind {
            0 => {
                if let Some(r) = self.view_prop_read() {
                    let v = self.fresh();
                    self.emit(ind, format!("{v} = {r}"));
                    self.define(v);
                } else {
                    let v = self.fresh();
                    let c = self.lib_call();
                    self.emit(ind, format!("{v} = {c}"));
                    self.define(v);
                }
            }
            1 => {
                let c = self.lib_call();
                if self.rng.chance(0.7) {
                    let v = self.fresh();
                    self.emit(ind, format!("{v} = {c}"));
                    self.define(v);
                } else {
                    self.emit(ind, c);
                }
            }
            2 => {
                let p = self.w.tag_path(self.rng);
                let v = self.fresh();
                self.emit(
                    ind,
                    format!("{v} = system.tag.readBlocking([\"{p}\"])[0].value"),
                );
                self.define(v);
            }
            3 => {
                let p = self.w.tag_path(self.rng);
                let v = self.value_expr();
                self.emit(ind, format!("system.tag.writeBlocking([\"{p}\"], [{v}])"));
            }
            4 => {
                let v = self.value_expr();
                let logger = self.ctx.logger;
                let level = *self.rng.pick(&["info", "debug", "warn"]);
                self.emit(
                    ind,
                    format!("system.util.getLogger(\"{logger}\").{level}(\"value: %s\" % ({v},))"),
                );
            }
            5 => {
                let m = *self.rng.pick(names::MESSAGES);
                let v = self.value_expr();
                let scope = *self.rng.pick(&["page", "session", "view"]);
                self.emit(
                    ind,
                    format!(
                        "system.perspective.sendMessage(\"{m}\", payload={{\"value\": {v}}}, scope=\"{scope}\")"
                    ),
                );
            }
            6 => {
                if self.w.popups.is_empty() {
                    return;
                }
                let target = &self.w.views[*self.rng.pick(&self.w.popups)];
                let args: Vec<String> = target
                    .params
                    .iter()
                    .take(2)
                    .map(|p| format!("\"{}\": {}", p.name, self.value_expr()))
                    .collect();
                let id = names::lower_first(target.path.rsplit('/').next().unwrap_or("popup"));
                self.emit(
                    ind,
                    format!(
                        "system.perspective.openPopup(\"{id}\", \"{}\", params={{{}}})",
                        target.path,
                        args.join(", ")
                    ),
                );
            }
            7 => {
                let q = self.rng.pick(&self.w.queries).clone();
                let args: Vec<String> = q
                    .params
                    .iter()
                    .map(|(n, _)| format!("\"{n}\": {}", self.value_expr()))
                    .collect();
                let v = self.fresh();
                self.emit(
                    ind,
                    format!(
                        "{v} = system.db.runNamedQuery(\"{}\", {{{}}})",
                        q.path,
                        args.join(", ")
                    ),
                );
                self.define(v.clone());
                if remaining > 2 && depth < 3 {
                    self.unread.retain(|u| *u != v);
                    let row = if self.locals.contains(&"row".to_owned()) {
                        "row2"
                    } else {
                        "row"
                    };
                    self.emit(
                        ind,
                        format!("for {row} in system.dataset.toPyDataSet({v}):"),
                    );
                    let saved = self.locals.clone();
                    self.locals.push(row.to_owned());
                    let t = self.fresh();
                    self.emit(ind + 1, format!("{t} = {row}[0]"));
                    self.define(t);
                    if self.rng.chance(0.5) {
                        self.stmt(ind + 1, depth + 1, budget);
                    }
                    self.restore(ind + 1, saved);
                }
            }
            8 => {
                let cond = match self.local() {
                    Some(l) => match self.rng.below(4) {
                        0 => format!("{l} is None"),
                        1 => format!("not {l}"),
                        2 => format!("{l} > {}", self.rng.below(50)),
                        _ => format!("{l} == {}", self.string_lit()),
                    },
                    None => "True".to_owned(),
                };
                self.emit(ind, format!("if {cond}:"));
                let saved = self.locals.clone();
                let n = self.rng.range(1, 3);
                for _ in 0..n {
                    self.stmt(ind + 1, depth + 1, budget);
                }
                self.restore(ind + 1, saved.clone());
                if self.rng.chance(0.4) {
                    self.emit(ind, "else:".to_owned());
                    self.stmt(ind + 1, depth + 1, budget);
                    self.restore(ind + 1, saved);
                }
            }
            9 => {
                self.emit(ind, "try:".to_owned());
                let saved = self.locals.clone();
                self.stmt(ind + 1, depth + 1, budget);
                self.stmt(ind + 1, depth + 1, budget);
                self.restore(ind + 1, saved);
                self.emit(ind, "except Exception as e:".to_owned());
                let logger = self.ctx.logger;
                self.emit(
                    ind + 1,
                    format!("system.util.getLogger(\"{logger}\").error(\"failed: %s\" % e)"),
                );
            }
            10 => {
                let v = self.value_expr();
                if self.ctx.has_self {
                    if !self.ctx.view_custom.is_empty() && self.rng.chance(0.4) {
                        let c = self.rng.pick(self.ctx.view_custom).clone();
                        self.emit(ind, format!("self.view.custom.{c} = {v}"));
                    } else {
                        self.emit(ind, format!("self.props.text = {v}"));
                    }
                } else {
                    let t = self.fresh();
                    self.emit(ind, format!("{t} = {v}"));
                    self.define(t);
                }
            }
            11 => {
                let v = self.fresh();
                self.emit(
                    ind,
                    format!("{v} = system.date.format(system.date.now(), \"yyyy-MM-dd HH:mm\")"),
                );
                self.define(v);
            }
            12 => {
                let src = self.local().unwrap_or_else(|| "[]".to_owned());
                let v = self.fresh();
                self.emit(ind, format!("{v} = [str(x) for x in ({src} or [])]"));
                self.define(v);
            }
            _ => {
                if self.ctx.has_self && !self.ctx.siblings.is_empty() {
                    let s = self.rng.pick(self.ctx.siblings).clone();
                    let v = self.fresh();
                    self.emit(ind, format!("{v} = self.getSibling(\"{s}\").props.text"));
                    self.define(v);
                } else {
                    let c = self.lib_call();
                    self.emit(ind, c);
                }
            }
        }
    }
}
