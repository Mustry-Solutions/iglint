//! Ignition expression generator.

use crate::names;
use crate::rng::Rng;

/// Property references the expression may use, already in `{...}` form
/// without braces, e.g. `view.params.lineId`.
pub struct ExprCtx<'a> {
    pub refs: &'a [String],
    /// Inside an expression transform `{value}` is available.
    pub has_value: bool,
}

pub struct ExprOut {
    pub text: String,
    /// References used (without braces).
    pub used: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExprDefect {
    None,
    Syntax,
    UnknownFunction,
    Polling,
    UnresolvedRef,
}

pub fn expression(rng: &mut Rng, ctx: &ExprCtx<'_>, defect: ExprDefect) -> ExprOut {
    let mut g = G {
        rng,
        ctx,
        used: Vec::new(),
    };
    let text = match defect {
        ExprDefect::None => g.top(),
        ExprDefect::Syntax => {
            let a = g.atom();
            format!("if({a}, \"On\" \"Off\")")
        }
        ExprDefect::UnknownFunction => {
            let a = g.atom();
            let f = *g
                .rng
                .pick(&["formatValue", "getValueOf", "toLabel", "safeInt"]);
            format!("{f}({a})")
        }
        ExprDefect::Polling => {
            let ms = *g.rng.pick(&[100, 500, 1000, 5000]);
            format!("dateFormat(now({ms}), \"HH:mm:ss\")")
        }
        ExprDefect::UnresolvedRef => {
            let name = format!("{}{}", g.rng.pick(names::EQUIPMENT), "Missing");
            format!("toStr({{../{name}.props.text}})")
        }
    };
    ExprOut { text, used: g.used }
}

struct G<'a, 'r> {
    rng: &'r mut Rng,
    ctx: &'a ExprCtx<'a>,
    used: Vec<String>,
}

impl G<'_, '_> {
    fn reference(&mut self) -> String {
        if self.ctx.has_value && (self.ctx.refs.is_empty() || self.rng.chance(0.6)) {
            return "{value}".to_owned();
        }
        if self.ctx.refs.is_empty() {
            return format!("{}", self.rng.below(100));
        }
        let r = self.rng.pick(self.ctx.refs).clone();
        let out = format!("{{{r}}}");
        self.used.push(r);
        out
    }

    fn atom(&mut self) -> String {
        match self.rng.below(10) {
            0..=5 => self.reference(),
            6 | 7 => format!("{}", self.rng.below(1000)),
            8 => format!("\"{}\"", self.rng.pick(names::ASPECTS)),
            _ => (*self.rng.pick(&["true", "false"])).to_owned(),
        }
    }

    fn top(&mut self) -> String {
        match self.rng.below(16) {
            0 | 1 => self.reference(),
            2 => {
                let a = self.atom();
                let b = self.string();
                let c = self.string();
                format!("if({a}, {b}, {c})")
            }
            3 => {
                let a = self.reference();
                let b = self.string();
                format!("concat({a}, \" \", {b})")
            }
            4 => {
                let a = self.reference();
                format!("numberFormat({a}, \"#,##0.0\")")
            }
            5 => {
                let rate = *self.rng.pick(&[10000, 15000, 30000, 60000]);
                format!("dateFormat(now({rate}), \"HH:mm:ss\")")
            }
            6 => {
                let a = self.reference();
                let b = self.atom();
                format!("try(toInt({a}), {b})")
            }
            7 => {
                let a = self.reference();
                let b = self.reference();
                let op = *self.rng.pick(&["+", "-", "*", "/"]);
                format!("{a} {op} {b}")
            }
            8 => {
                let a = self.reference();
                let n = self.rng.below(100);
                let op = *self.rng.pick(&[">", "<", ">=", "<=", "=", "!="]);
                format!("{a} {op} {n}")
            }
            9 => {
                let a = self.reference();
                let b = self.reference();
                format!("{a} && !{b}")
            }
            10 => {
                let a = self.reference();
                format!("isNull({a}) || len(toStr({a})) = 0")
            }
            11 => {
                let a = self.reference();
                let n = self.rng.range(2, 4);
                let mut s = format!("case({a}");
                for i in 0..n {
                    s.push_str(&format!(", {i}, \"{}\"", self.rng.pick(names::ASPECTS)));
                }
                s.push_str(", \"Unknown\")");
                s
            }
            12 => {
                let a = self.reference();
                let b = self.reference();
                format!("coalesce({a}, {b}, 0)")
            }
            13 => {
                let a = self.reference();
                format!("upper(toStr({a}))")
            }
            14 => {
                let a = self.reference();
                format!("round(toFloat({a}) * 100, 1)")
            }
            _ => {
                let a = self.reference();
                let b = self.reference();
                format!("stringFormat(\"%s (%s)\", {a}, {b})")
            }
        }
    }

    fn string(&mut self) -> String {
        format!("\"{}\"", self.rng.pick(names::ASPECTS))
    }
}
