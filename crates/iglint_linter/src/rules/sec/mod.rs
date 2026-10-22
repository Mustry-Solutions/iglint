//! SEC: security.
//!
//! The script rules here share the semantic model of the `SCR` rules
//! ([`crate::rules::script::semantic`]): callees are resolved through
//! imports and aliases (`db = system.db`, `from java.lang import Runtime`,
//! `import os as o`), never by name alone, and a local name is followed to
//! the values assigned to it when every assignment that reaches the use is
//! a plain one.

use std::borrow::Cow;

use iglint_python::ast::{Expr, ExprCall, Stmt, StringValue};
use iglint_python_semantic::{BindingId, BindingKind, SemanticModel};

pub mod eval_exec;
pub mod hardcoded_credential;
pub mod insecure_url;
pub mod os_command;
pub mod querystring_parameter;
pub mod sql_string_building;

/// How many assignments a lookup follows (`a = b`, `b = c`, ...).
pub(crate) const MAX_DEPTH: u32 = 8;

/// The value of a string literal.
pub fn string_literal(expr: &Expr) -> Option<Cow<'_, str>> {
    match expr {
        Expr::Str(s) => Some(match &s.s {
            StringValue::Unicode(u) => Cow::Borrowed(&**u),
            StringValue::Bytes(b) => String::from_utf8_lossy(b),
        }),
        _ => None,
    }
}

/// The values assigned to the name `expr` by every binding that reaches
/// it, when each of them is a plain single-target assignment (`q = ...`).
/// `None` for anything else: parameters, loop variables, imports, tuple
/// unpacking, augmented assignments, names that may be unbound, ...
pub fn assigned_values<'m>(model: &SemanticModel<'m>, expr: &Expr) -> Option<Vec<&'m Expr>> {
    let Expr::Name(n) = expr else { return None };
    let reference = model.reference_at(n.range)?;
    if reference.bindings.is_empty() || reference.maybe_unbound {
        return None;
    }
    reference
        .bindings
        .iter()
        .map(|id| {
            let binding = model.binding(*id);
            match binding.kind {
                BindingKind::Assignment if !binding.unpacked => binding.value,
                _ => None,
            }
        })
        .collect()
}

/// The value assigned to the name `expr` when exactly one plain assignment
/// reaches it (see [`assigned_values`]).
pub fn single_assigned_value<'m>(model: &SemanticModel<'m>, expr: &Expr) -> Option<&'m Expr> {
    match assigned_values(model, expr)?.as_slice() {
        [one] => Some(one),
        _ => None,
    }
}

/// The value of a string literal, or of a name whose single reaching
/// assignment is (a chain of names ending in) a string literal.
pub fn constant_string<'e>(model: &SemanticModel<'e>, expr: &'e Expr) -> Option<Cow<'e, str>> {
    let mut at = expr;
    for _ in 0..MAX_DEPTH {
        if let Some(s) = string_literal(at) {
            return Some(s);
        }
        at = single_assigned_value(model, at)?;
    }
    None
}

/// Whether the value of `expr` is made only of literals, so no outside
/// input can reach it: string and number literals, names whose reaching
/// assignments (plain, or `+=`/`*=`/... of such values) are all such
/// values, `+` and `%` of such values, repetition of a literal (`"?" *
/// n`, `["?"] * len(ids)`), `.join`, `.format` and case or whitespace
/// methods of such values, `str(...)` of such values, comprehensions
/// producing them, and numbers (`int(x)`, `len(x)`, arithmetic,
/// comparisons).
pub fn is_fixed(model: &SemanticModel<'_>, expr: &Expr) -> bool {
    Fixed {
        model,
        visiting: Vec::new(),
    }
    .expr(expr, 0)
}

struct Fixed<'m, 'a> {
    model: &'m SemanticModel<'a>,
    /// Bindings being checked: a binding that reaches itself (`pos +=
    /// step` in a loop) is fixed when its other sources are.
    visiting: Vec<BindingId>,
}

impl Fixed<'_, '_> {
    fn expr(&mut self, expr: &Expr, depth: u32) -> bool {
        use iglint_python::ast::Operator;
        if depth > MAX_DEPTH {
            return false;
        }
        let model = self.model;
        let d = depth + 1;
        match expr {
            Expr::Str(_) | Expr::Num(_) | Expr::Compare(_) | Expr::UnaryOp(_) => true,
            Expr::Name(n) => {
                if matches!(&*n.id, "None" | "True" | "False")
                    && crate::rules::script::is_builtin_reference(model, expr, &n.id)
                {
                    return true;
                }
                let Some(reference) = model.reference_at(n.range) else {
                    return false;
                };
                if reference.bindings.is_empty() || reference.maybe_unbound {
                    return false;
                }
                let bindings = reference.bindings.clone();
                bindings.iter().all(|id| self.binding(*id, d))
            }
            Expr::Tuple(t) => t.elts.iter().all(|e| self.expr(e, d)),
            Expr::List(l) => l.elts.iter().all(|e| self.expr(e, d)),
            Expr::BinOp(b) => match b.op {
                Operator::Add | Operator::Mod => self.expr(&b.left, d) && self.expr(&b.right, d),
                // Repetition: the repeated value is all that ends up in the text.
                Operator::Mult => self.expr(&b.left, d) || self.expr(&b.right, d),
                // Arithmetic and bit operations on numbers.
                _ => true,
            },
            Expr::BoolOp(b) => b.values.iter().all(|e| self.expr(e, d)),
            Expr::IfExp(i) => self.expr(&i.body, d) && self.expr(&i.orelse, d),
            Expr::ListComp(c) => self.expr(&c.elt, d),
            Expr::GeneratorExp(c) => self.expr(&c.elt, d),
            Expr::Call(c) => {
                if c.starargs.is_some() || c.kwargs.is_some() {
                    return false;
                }
                match &*c.func {
                    Expr::Name(n) => {
                        let builtin =
                            crate::rules::script::is_builtin_reference(model, &c.func, &n.id);
                        match &*n.id {
                            "int" | "long" | "float" | "len" | "round" | "abs" | "bool" => builtin,
                            "str" | "unicode" => builtin && self.args(c, d),
                            _ => false,
                        }
                    }
                    Expr::Attribute(a) => {
                        matches!(
                            &*a.attr.id,
                            "join"
                                | "format"
                                | "upper"
                                | "lower"
                                | "strip"
                                | "lstrip"
                                | "rstrip"
                                | "replace"
                        ) && self.args(c, d)
                            && self.expr(&a.value, d)
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn args(&mut self, c: &ExprCall, d: u32) -> bool {
        c.args.iter().all(|a| self.expr(a, d)) && c.keywords.iter().all(|k| self.expr(&k.value, d))
    }

    fn binding(&mut self, id: BindingId, depth: u32) -> bool {
        if self.visiting.contains(&id) {
            return true;
        }
        if depth > MAX_DEPTH {
            return false;
        }
        let binding = self.model.binding(id);
        self.visiting.push(id);
        let fixed = match binding.kind {
            BindingKind::Assignment if !binding.unpacked => {
                binding.value.is_some_and(|v| self.expr(v, depth + 1))
            }
            BindingKind::AugAssignment => match binding.stmt {
                Some(Stmt::AugAssign(s)) => {
                    !binding.shadowed.is_empty()
                        && self.expr(&s.value, depth + 1)
                        && binding.shadowed.iter().all(|b| self.binding(*b, depth + 1))
                }
                _ => false,
            },
            _ => false,
        };
        self.visiting.pop();
        fixed
    }
}

/// The qualified name of the function whose call produced `expr`: `expr`
/// is a call, or a name whose single reaching assignment is such a call
/// (`client = system.net.httpClient()`, `rt = Runtime.getRuntime()`).
pub fn produced_by(model: &SemanticModel<'_>, expr: &Expr) -> Option<String> {
    let mut at = expr;
    for _ in 0..MAX_DEPTH {
        if let Expr::Call(c) = at {
            return model.callee(c);
        }
        at = single_assigned_value(model, at)?;
    }
    None
}

/// The argument passed for parameter `name` (at `position` when passed
/// positionally). `None` when it is not passed or cannot be located
/// (`*args` before that position, `**kwargs`).
pub fn argument<'c>(call: &'c ExprCall, name: &str, position: Option<usize>) -> Option<&'c Expr> {
    if let Some(k) = call.keywords.iter().find(|k| &*k.arg.id == name) {
        return Some(&k.value);
    }
    call.args.get(position?)
}

/// The position of parameter `name` of the `system.*` function `path`,
/// when every overload that names its parameters puts it at the same
/// position.
pub fn system_param_position(path: &str, name: &str) -> Option<usize> {
    let function = iglint_stubs::system_function(path)?;
    let mut position = None;
    for variant in function.variants {
        for overload in variant.overloads {
            if overload.params.iter().all(|p| p.name.is_none()) {
                continue;
            }
            let Some(at) = overload.params.iter().position(|p| p.name == Some(name)) else {
                continue;
            };
            match position {
                None => position = Some(at),
                Some(p) if p == at => {}
                Some(_) => return None,
            }
        }
    }
    position
}

/// The argument passed for parameter `name` of the `system.*` function
/// `path` (by keyword or by its position in the stubs).
pub fn system_argument<'c>(call: &'c ExprCall, path: &str, name: &str) -> Option<&'c Expr> {
    argument(call, name, system_param_position(path, name))
}

/// The parameter names of a `system.*` function, by position, for the
/// positional arguments of `call`: one entry per positional argument, the
/// name when every overload that accepts that many arguments agrees.
pub fn system_positional_names(path: &str, call: &ExprCall) -> Vec<Option<&'static str>> {
    let Some(function) = iglint_stubs::system_function(path) else {
        return vec![None; call.args.len()];
    };
    (0..call.args.len())
        .map(|i| {
            let mut name: Option<&'static str> = None;
            for variant in function.variants {
                for overload in variant.overloads {
                    let Some(param) = overload.params.get(i) else {
                        continue;
                    };
                    match (name, param.name) {
                        (_, None) => return None,
                        (None, Some(n)) => name = Some(n),
                        (Some(a), Some(b)) if a == b => {}
                        _ => return None,
                    }
                }
            }
            name
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_positions() {
        assert_eq!(
            system_param_position("system.db.runPrepQuery", "query"),
            Some(0)
        );
        assert_eq!(
            system_param_position("system.net.httpGet", "password"),
            Some(4)
        );
        assert_eq!(system_param_position("system.db.nothing", "query"), None);
    }
}
