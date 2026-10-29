//! Shared helpers of the `system.*` rules (`SCR201` to `SCR205`): which
//! scope a script runs in and which `system.*` path an expression names.

use iglint_project::RunScope;
use iglint_python::ast::{Expr, ExprCall};
use iglint_python_semantic::SemanticModel;
use iglint_stubs::{Function, OverloadKind, Scope, Scopes};

use crate::rule::ScriptUnit;

/// The scripting scope a script runs in, or `None` for library modules,
/// which run in the scope of whatever imports them (format spec 3.1).
///
/// Perspective scripts (component, view and session level), gateway
/// events, tag events and WebDev handlers all run in the gateway; Vision
/// client scripts run in the Vision Client.
pub fn script_scope(unit: &ScriptUnit<'_, '_>) -> Option<Scope> {
    match unit.script.run_scope {
        RunScope::PerspectiveComponent
        | RunScope::PerspectiveSession
        | RunScope::Gateway
        | RunScope::Tag => Some(Scope::Gateway),
        RunScope::VisionClient => Some(Scope::Client),
        RunScope::Library => None,
    }
}

/// `scopes` in words: `the gateway`, `the Vision Client and the Designer`.
pub fn scope_names(scopes: Scopes) -> String {
    let names: Vec<&str> = scopes
        .iter()
        .map(|s| match s {
            Scope::Gateway => "the gateway",
            Scope::Client => "the Vision Client",
            Scope::Designer => "the Designer",
        })
        .collect();
    match names.as_slice() {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// The `system.*` path `expr` refers to, following imports and aliases
/// (`db = system.db` makes `db.runQuery` name `system.db.runQuery`). The
/// legacy alias `fpmi` is reported as `system`. `None` when the expression
/// is not (provably) the scripting API: a local `system = ...` that is not
/// an alias of it is not the API.
pub fn system_path(model: &SemanticModel<'_>, expr: &Expr) -> Option<String> {
    if !matches!(expr, Expr::Attribute(_) | Expr::Name(_)) {
        return None;
    }
    let q = model.qualified_name(expr)?;
    if q == "system" || q.starts_with("system.") {
        return Some(q);
    }
    let rest = q.strip_prefix("fpmi")?;
    (rest.is_empty() || rest.starts_with('.')).then(|| format!("system{rest}"))
}

/// The `system.*` function a call calls, with its path as written
/// (resolved through aliases).
pub fn called_function(
    model: &SemanticModel<'_>,
    call: &ExprCall,
) -> Option<(String, &'static Function)> {
    let path = system_path(model, &call.func)?;
    let function = iglint_stubs::system_function(&path)?;
    Some((path, function))
}

/// Whether the call's arguments can be counted: no `*args` or `**kwargs`
/// unpacking.
pub fn is_checkable(call: &ExprCall) -> bool {
    call.starargs.is_none() && call.kwargs.is_none()
}

/// Whether the stubs cannot judge the function's argument counts: an
/// overload accepts anything as far as the stubs know (`Unchecked`), or a
/// plain Java overload ends in an array parameter, which Jython may also
/// accept as Java varargs (a variable number of trailing arguments).
pub fn has_unchecked_overload(function: &Function) -> bool {
    function.variants.iter().any(|v| {
        v.overloads.iter().any(|o| match o.kind {
            OverloadKind::Unchecked => true,
            OverloadKind::Positional => o
                .params
                .last()
                .and_then(|p| p.ty)
                .is_some_and(|t| t.ends_with("[]")),
            _ => false,
        })
    })
}

/// The keyword names of a call.
pub fn keyword_names(call: &ExprCall) -> Vec<&str> {
    call.keywords.iter().map(|k| k.arg.as_str()).collect()
}

/// A call to a `system.*` function that matches none of its overloads.
pub struct ArityProblem {
    /// What to point at: the offending keyword, or the whole call.
    pub range: iglint_text::TextRange,
    pub message: String,
    /// An unknown keyword or keywords passed to a function that takes
    /// none (`SCR203`); otherwise a count problem (`SCR202`).
    pub keyword: bool,
}

/// Checks every countable call to a `system.*` function in the script
/// against the overloads of the scope(s) the script runs in. Library
/// modules run in their caller's scope: a call is only a problem when it
/// fails in every scope where the function exists.
pub fn arity_problems(model: &SemanticModel<'_>, unit: &ScriptUnit<'_, '_>) -> Vec<ArityProblem> {
    use iglint_stubs::ArityError;
    let scope = script_scope(unit);
    let mut out = Vec::new();
    crate::rules::script::walk_exprs(&unit.parsed.module.body, &mut |e| {
        let Expr::Call(call) = e else { return };
        if !is_checkable(call) {
            return;
        }
        let Some((path, function)) = called_function(model, call) else {
            return;
        };
        if has_unchecked_overload(function) {
            return;
        }
        let scopes: Vec<Scope> = match scope {
            Some(s) => vec![s],
            None => function.scopes().iter().collect(),
        };
        let keywords = keyword_names(call);
        let mut first_error = None;
        let mut keyword_overloads = false;
        for s in scopes {
            // Not existing in the scope is `SCR205`'s business.
            let Some(variant) = function.variant(s) else {
                return;
            };
            keyword_overloads |= variant
                .overloads
                .iter()
                .any(|o| matches!(o.kind, iglint_stubs::OverloadKind::Keyword));
            match iglint_stubs::check_call(variant, call.args.len(), &keywords) {
                Ok(()) => return,
                Err(e) => {
                    first_error.get_or_insert(e);
                }
            }
        }
        let Some(error) = first_error else { return };
        let keyword_range = |name: &str| {
            call.keywords
                .iter()
                .find(|k| k.arg.as_str() == name)
                .map(|k| k.arg.range)
        };
        let (keyword, range) = match &error {
            ArityError::NotInScope => return,
            // Some `@KeywordArgs` functions read their arguments into a map
            // (`PyArgumentMap`) that ignores unknown keywords and lets a
            // keyword replace a positional value; only those parsed with
            // `PyArgParser` reject them, and the stubs do not say which.
            ArityError::UnknownKeyword { .. } | ArityError::DuplicateArgument { .. }
                if keyword_overloads =>
            {
                return;
            }
            ArityError::KeywordsNotAccepted { keyword }
            | ArityError::UnknownKeyword { keyword } => (true, keyword_range(keyword)),
            ArityError::DuplicateArgument { name } => (false, keyword_range(name)),
            _ => (false, None),
        };
        out.push(ArityProblem {
            range: range.unwrap_or(call.range),
            message: format!("Call to `{path}`: {error}"),
            keyword,
        });
    });
    out
}
