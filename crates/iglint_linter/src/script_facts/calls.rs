//! Calls and literal values of scripts, resolved with the semantic model.
//!
//! Callees are resolved to dotted names through imports and aliases
//! ([`SemanticModel::qualified_name_with`]): `nav = system.perspective.navigate`
//! makes `nav(...)` a call of `system.perspective.navigate`, and the legacy
//! alias `fpmi` stands for `system`. Arguments are found by position or
//! keyword the way Python binds them ([`CallSite::arg`]), and literal values
//! are followed through local names whose every reaching binding assigns a
//! literal ([`string_values`]).

use iglint_python::ast::{Expr, ExprCall, ExprDict, Operator};
use iglint_python_semantic::{BindingKind, Resolution, SemanticModel, UnboundNames};
use iglint_text::{Ranged, TextRange};

/// How deep [`string_values`] follows names and expressions.
const MAX_DEPTH: u32 = 8;

/// A call whose callee resolves to a dotted name.
#[derive(Clone, Debug)]
pub struct CallSite<'a> {
    pub call: &'a ExprCall,
    /// The callee, e.g. `system.perspective.openPopup` (`fpmi.` is
    /// normalised to `system.`).
    pub callee: String,
}

/// An argument as the callee receives it.
#[derive(Clone, Copy, Debug)]
pub enum Arg<'a> {
    /// Passed explicitly (positionally or by keyword).
    Given(&'a Expr),
    /// Not passed.
    Absent,
    /// May be passed through `*args` or `**kwargs`.
    Unknown,
}

impl<'a> Arg<'a> {
    pub fn given(self) -> Option<&'a Expr> {
        match self {
            Arg::Given(e) => Some(e),
            _ => None,
        }
    }

    pub fn is_absent(self) -> bool {
        matches!(self, Arg::Absent)
    }
}

impl<'a> CallSite<'a> {
    /// The argument bound to parameter `name` at position `index`.
    pub fn arg(&self, index: usize, name: &str) -> Arg<'a> {
        arg(self.call, index, name)
    }

    /// Whether the call unpacks `*args` or `**kwargs`.
    pub fn has_unpacking(&self) -> bool {
        self.call.starargs.is_some() || self.call.kwargs.is_some()
    }
}

/// The argument of `call` bound to parameter `name` at position `index`.
pub fn arg<'a>(call: &'a ExprCall, index: usize, name: &str) -> Arg<'a> {
    if let Some(e) = call.args.get(index) {
        return Arg::Given(e);
    }
    if let Some(k) = call.keywords.iter().find(|k| k.arg.as_str() == name) {
        return Arg::Given(&k.value);
    }
    if call.starargs.is_some() || call.kwargs.is_some() {
        return Arg::Unknown;
    }
    Arg::Absent
}

/// The dotted name `expr` stands for, with `fpmi` normalised to `system`.
pub fn qualified(model: &SemanticModel<'_>, expr: &Expr, unbound: UnboundNames) -> Option<String> {
    let q = model.qualified_name_with(expr, unbound)?;
    Some(normalise(q))
}

fn normalise(q: String) -> String {
    match q.strip_prefix("fpmi") {
        Some(rest) if rest.is_empty() || rest.starts_with('.') => format!("system{rest}"),
        _ => q,
    }
}

/// Every call of the module (including nested functions, classes and
/// lambdas) whose callee resolves to a dotted name, in source order.
pub fn call_sites<'a>(
    model: &SemanticModel<'a>,
    body: &'a [iglint_python::ast::Stmt],
    unbound: UnboundNames,
) -> Vec<CallSite<'a>> {
    let mut out = Vec::new();
    crate::rules::script::walk_exprs(body, &mut |e| {
        if let Expr::Call(c) = e
            && let Some(callee) = qualified(model, &c.func, unbound)
        {
            out.push(CallSite { call: c, callee });
        }
    });
    out
}

/// A literal string value and where it is written in the script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Literal {
    pub value: String,
    pub range: TextRange,
}

/// The string values `expr` can have, when every one of them is a literal:
/// a string literal, a conditional expression of such values, or a name
/// whose every reaching binding assigns such a value. `None` when any
/// value may be computed.
pub fn string_values(model: &SemanticModel<'_>, expr: &Expr) -> Option<Vec<Literal>> {
    let mut out = Vec::new();
    collect_strings(model, expr, 0, &mut out)?;
    out.dedup();
    Some(out)
}

/// The single literal string value of `expr` (see [`string_values`]).
pub fn string_value(model: &SemanticModel<'_>, expr: &Expr) -> Option<Literal> {
    let mut values = string_values(model, expr)?;
    let first = values.first()?;
    if values.iter().all(|v| v.value == first.value) {
        values.truncate(1);
        values.pop()
    } else {
        None
    }
}

fn collect_strings(
    model: &SemanticModel<'_>,
    expr: &Expr,
    depth: u32,
    out: &mut Vec<Literal>,
) -> Option<()> {
    if depth > MAX_DEPTH {
        return None;
    }
    match expr {
        Expr::Str(s) => {
            out.push(Literal {
                value: s.s.as_str()?.to_owned(),
                range: s.range,
            });
            Some(())
        }
        Expr::IfExp(e) => {
            collect_strings(model, &e.body, depth + 1, out)?;
            collect_strings(model, &e.orelse, depth + 1, out)
        }
        Expr::Name(n) => {
            for value in single_values(model, n.range)? {
                collect_strings(model, value, depth + 1, out)?;
            }
            Some(())
        }
        _ => None,
    }
}

/// The values assigned by every binding that may reach the name reference
/// at `range`, when all of them are plain `name = value` assignments.
fn single_values<'a>(model: &SemanticModel<'a>, range: TextRange) -> Option<Vec<&'a Expr>> {
    let reference = model.reference_at(range)?;
    if reference.bindings.is_empty()
        || reference.maybe_unbound
        || !matches!(
            reference.resolution,
            Resolution::Local | Resolution::Global | Resolution::Free(_)
        )
    {
        return None;
    }
    reference
        .bindings
        .iter()
        .map(|id| {
            let b = model.binding(*id);
            match b.kind {
                BindingKind::Assignment if !b.unpacked => b.value,
                _ => None,
            }
        })
        .collect()
}

/// The literal text a computed string starts with: `'Popups/' + name`,
/// `'Popups/%s' % name` and `'Popups/{}'.format(name)` all start with
/// `Popups/`. Empty when nothing is known.
pub fn string_prefix(model: &SemanticModel<'_>, expr: &Expr) -> String {
    prefix(model, expr, 0).unwrap_or_default()
}

fn prefix(model: &SemanticModel<'_>, expr: &Expr, depth: u32) -> Option<String> {
    if depth > MAX_DEPTH {
        return None;
    }
    match expr {
        Expr::Str(s) => s.s.as_str().map(str::to_owned),
        Expr::BinOp(b) if b.op == Operator::Add => prefix(model, &b.left, depth + 1),
        Expr::BinOp(b) if b.op == Operator::Mod => {
            let format = prefix(model, &b.left, depth + 1)?;
            Some(format.split('%').next().unwrap_or("").to_owned())
        }
        Expr::Call(c) => match &*c.func {
            Expr::Attribute(a) if a.attr.as_str() == "format" => {
                let format = prefix(model, &a.value, depth + 1)?;
                Some(format.split(['{', '}']).next().unwrap_or("").to_owned())
            }
            _ => None,
        },
        Expr::Name(n) => {
            let values = single_values(model, n.range)?;
            let prefixes: Vec<String> = values
                .into_iter()
                .map(|v| prefix(model, v, depth + 1))
                .collect::<Option<_>>()?;
            Some(common_prefix(&prefixes))
        }
        Expr::IfExp(e) => {
            let a = prefix(model, &e.body, depth + 1)?;
            let b = prefix(model, &e.orelse, depth + 1)?;
            Some(common_prefix(&[a, b]))
        }
        _ => None,
    }
}

fn common_prefix(values: &[String]) -> String {
    let Some(first) = values.first() else {
        return String::new();
    };
    let mut len = first.len();
    for v in &values[1..] {
        len = first
            .char_indices()
            .zip(v.chars())
            .take_while(|((_, a), b)| a == b)
            .last()
            .map_or(0, |((i, a), _)| i + a.len_utf8())
            .min(len);
    }
    // `len` is the end of a character found by `char_indices` (or 0).
    first.get(..len).unwrap_or_default().to_owned()
}

/// The keys of a dict literal (`{'a': 1}`) or `dict(a=1)` call, when every
/// key is a literal string. `None` for anything else, including names
/// bound to dicts (keys may be added later).
pub fn dict_keys(model: &SemanticModel<'_>, expr: &Expr) -> Option<Vec<Literal>> {
    match expr {
        Expr::Dict(d) => literal_keys(d),
        Expr::Call(c)
            if c.args.is_empty()
                && c.starargs.is_none()
                && c.kwargs.is_none()
                && crate::rules::script::is_builtin_reference(model, &c.func, "dict") =>
        {
            Some(
                c.keywords
                    .iter()
                    .map(|k| Literal {
                        value: k.arg.as_str().to_owned(),
                        range: k.arg.range,
                    })
                    .collect(),
            )
        }
        _ => None,
    }
}

fn literal_keys(d: &ExprDict) -> Option<Vec<Literal>> {
    d.keys
        .iter()
        .map(|k| match k {
            Expr::Str(s) => Some(Literal {
                value: s.s.as_str()?.to_owned(),
                range: s.range,
            }),
            _ => None,
        })
        .collect()
}

/// Whether `expr` is `system.util.getProjectName()` (the project the
/// script runs in), directly or through a local name.
pub fn is_project_name_call(model: &SemanticModel<'_>, expr: &Expr) -> bool {
    is_project_name(model, expr, 0)
}

fn is_project_name(model: &SemanticModel<'_>, expr: &Expr, depth: u32) -> bool {
    match expr {
        Expr::Call(c) => {
            c.args.is_empty()
                && c.keywords.is_empty()
                && qualified(model, &c.func, UnboundNames::Reject).as_deref()
                    == Some("system.util.getProjectName")
        }
        Expr::Name(n) if depth < MAX_DEPTH => single_values(model, n.range)
            .is_some_and(|values| values.iter().all(|v| is_project_name(model, v, depth + 1))),
        _ => false,
    }
}

/// The range of an expression.
pub fn range(e: &Expr) -> TextRange {
    e.range()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iglint_python::ast::Stmt;
    use iglint_python_semantic::SemanticOptions;

    fn with_calls(src: &str, f: impl FnOnce(&SemanticModel<'_>, &[CallSite<'_>])) {
        let parsed = iglint_python::parse_module(src);
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let model = SemanticModel::build(
            &parsed.module,
            &SemanticOptions {
                injected: vec!["system", "fpmi"],
                ..SemanticOptions::default()
            },
        );
        let calls = call_sites(&model, &parsed.module.body, UnboundNames::AsGlobal);
        f(&model, &calls);
    }

    #[test]
    fn callees_follow_aliases() {
        with_calls(
            "nav = system.perspective.navigate\nnav(view='A')\nfpmi.db.runQuery('q')\nfrom core.util import go\ngo()\ncore.x.f()\n",
            |_, calls| {
                let names: Vec<&str> = calls.iter().map(|c| c.callee.as_str()).collect();
                assert_eq!(
                    names,
                    [
                        "system.perspective.navigate",
                        "system.db.runQuery",
                        "core.util.go",
                        "core.x.f"
                    ]
                );
            },
        );
    }

    #[test]
    fn arguments_by_position_and_keyword() {
        with_calls("f(1, b=2)\ng(*a)\n", |_, calls| {
            assert!(matches!(calls[0].arg(0, "a"), Arg::Given(_)));
            assert!(matches!(calls[0].arg(1, "b"), Arg::Given(_)));
            assert!(calls[0].arg(2, "c").is_absent());
            assert!(matches!(calls[1].arg(0, "a"), Arg::Unknown));
        });
    }

    #[test]
    fn literal_values_through_names() {
        let src = "A = 'x'\nB = 'y' if c else 'z'\ndef f():\n    v = 'w'\n    use(A, B, v, other, 'Pre/' + v, 'P/%s' % v, 'Q/{}'.format(v))\n";
        let parsed = iglint_python::parse_module(src);
        let model = SemanticModel::build(&parsed.module, &SemanticOptions::default());
        let Stmt::FunctionDef(f) = &parsed.module.body[2] else {
            panic!()
        };
        let Stmt::Expr(e) = &f.body[1] else { panic!() };
        let Expr::Call(c) = &*e.value else { panic!() };
        let values = |i: usize| {
            string_values(&model, &c.args[i])
                .map(|v| v.into_iter().map(|l| l.value).collect::<Vec<_>>())
        };
        assert_eq!(values(0), Some(vec!["x".to_owned()]));
        assert_eq!(values(1), Some(vec!["y".to_owned(), "z".to_owned()]));
        assert_eq!(values(2), Some(vec!["w".to_owned()]));
        assert_eq!(values(3), None);
        assert_eq!(string_prefix(&model, &c.args[4]), "Pre/");
        assert_eq!(string_prefix(&model, &c.args[5]), "P/");
        assert_eq!(string_prefix(&model, &c.args[6]), "Q/");
        assert_eq!(string_prefix(&model, &c.args[3]), "");
        assert_eq!(string_prefix(&model, &c.args[1]), "");
    }

    #[test]
    fn reassigned_names_are_not_literals() {
        let src = "def f(c):\n    v = 'a'\n    v = v + c\n    use(v)\n";
        let parsed = iglint_python::parse_module(src);
        let model = SemanticModel::build(&parsed.module, &SemanticOptions::default());
        let Stmt::FunctionDef(f) = &parsed.module.body[0] else {
            panic!()
        };
        let Stmt::Expr(e) = &f.body[2] else { panic!() };
        let Expr::Call(c) = &*e.value else { panic!() };
        assert_eq!(string_values(&model, &c.args[0]), None);
        assert_eq!(string_prefix(&model, &c.args[0]), "a");
    }

    #[test]
    fn dict_literal_keys() {
        with_calls(
            "f({'a': 1, 'b': 2})\nf(dict(c=1))\nf({k: 1})\nf(d)\n",
            |m, calls| {
                let calls: Vec<&CallSite<'_>> = calls.iter().filter(|c| c.callee == "f").collect();
                let keys = |i: usize| {
                    dict_keys(m, &calls[i].call.args[0])
                        .map(|v| v.into_iter().map(|l| l.value).collect::<Vec<_>>())
                };
                assert_eq!(keys(0), Some(vec!["a".to_owned(), "b".to_owned()]));
                assert_eq!(keys(1), Some(vec!["c".to_owned()]));
                assert_eq!(keys(2), None);
                assert_eq!(keys(3), None);
            },
        );
    }
}
