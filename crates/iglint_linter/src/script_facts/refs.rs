//! The cross-resource references of a script, for the project index.
//!
//! Collected once per script before the index is built (phase A), so the
//! project's library packages are not known yet: names bound nowhere stand
//! for themselves ([`UnboundNames::AsGlobalEvenIfDynamic`]) and the index
//! decides later which of them are library names. Everything here
//! over-approximates usage: a reference that cannot be followed is
//! recorded as a dynamic reference ([`DynamicReferences`]), which makes
//! "unused" rules stay silent for whatever it may reach.
//!
//! | script | recorded as |
//! |---|---|
//! | `system.perspective.openPopup(id, 'Popups/A')`, `navigate(view=...)` | view |
//! | `system.perspective.navigate('/page')` | page |
//! | `system.db.runNamedQuery('Q')` (and project-scoped, keyword and `exec*` forms) | named query |
//! | `system.perspective.sendMessage('t', {...})`, `system.util.sendMessage(p, 'h')` | message send with payload keys |
//! | `system.tag.readBlocking(['[p]a/b'])` and other tag reads and writes | tag path |
//! | `x.props.style.classes = 'A B'` | style classes |
//! | `core.util.fmt`, `from core.util import fmt`, a module's own top-level names | library name |
//! | a library name used as a value, `from m import *`, non-literal `__import__`/`eval`/`exec` | dynamic library reference |
//! | a non-literal view path, query path or message type | dynamic reference with the literal prefix |

use iglint_project::summary::{MessageSend, MessageTarget, References};
use iglint_python::ast::{Expr, ExprContext, Slice, Stmt};
use iglint_python::visitor::{Visitor, walk_expr, walk_stmt};
use iglint_python_semantic::{BindingKind, Resolution, ScopeId, SemanticModel, UnboundNames};

use super::api::{ApiTarget, api_target};
use super::calls::{
    Arg, call_sites, dict_keys, is_project_name_call, string_prefix, string_values,
};

/// What the script layer knows about where a script lives.
#[derive(Clone, Copy, Debug, Default)]
pub struct FactsContext<'c> {
    /// The project the script belongs to.
    pub project: Option<&'c str>,
    /// Every project of the workspace (to read `runNamedQuery(project, ...)`).
    pub projects: &'c [&'c str],
    /// The dotted module name, for library modules.
    pub module: Option<&'c str>,
}

/// Adds the references of a parsed script to `refs`.
pub fn collect(model: &SemanticModel<'_>, ctx: &FactsContext<'_>, refs: &mut References) {
    let body = &model.module.body;
    for site in call_sites(model, body, UnboundNames::AsGlobalEvenIfDynamic) {
        if let Some(target) = api_target(model, &site) {
            api_reference(model, ctx, target, refs);
        }
        dynamic_code(model, &site.callee, site.call, refs);
    }
    let mut chains = Chains { model, ctx, refs };
    chains.visit_module(model.module);
    // Imports make names attributes of the importing module.
    for (_, b) in model.bindings() {
        if matches!(b.kind, BindingKind::FromImport | BindingKind::Import)
            && let Some(q) = &b.qualified
            && q.contains('.')
        {
            refs.library.push(q.clone());
        }
    }
}

fn values_or_prefix(
    model: &SemanticModel<'_>,
    expr: &Expr,
    literal: &mut Vec<String>,
    dynamic: &mut Vec<String>,
) {
    match string_values(model, expr) {
        Some(values) => literal.extend(values.into_iter().map(|l| l.value)),
        None => dynamic.push(string_prefix(model, expr)),
    }
}

fn payload_keys(model: &SemanticModel<'_>, payload: Arg<'_>) -> Option<Vec<String>> {
    match payload {
        Arg::Absent => Some(Vec::new()),
        Arg::Unknown => None,
        Arg::Given(e) => {
            dict_keys(model, e).map(|keys| keys.into_iter().map(|k| k.value).collect())
        }
    }
}

fn api_reference(
    model: &SemanticModel<'_>,
    ctx: &FactsContext<'_>,
    target: ApiTarget<'_>,
    refs: &mut References,
) {
    match target {
        ApiTarget::View { path, .. } => {
            values_or_prefix(model, path, &mut refs.views, &mut refs.dynamic.views);
        }
        ApiTarget::Page { url } => {
            if let Some(values) = string_values(model, url) {
                refs.pages.extend(
                    values
                        .into_iter()
                        .map(|l| l.value)
                        .filter(|v| !v.contains("://")),
                );
            }
        }
        ApiTarget::NamedQuery(q) => {
            let mut path = q.path;
            if let Some(project) = q.project {
                if !is_project_name_call(model, project) {
                    match string_values(model, project) {
                        Some(names)
                            if names.iter().all(|n| Some(n.value.as_str()) == ctx.project) => {}
                        // Another project's query (or an unknown project).
                        _ => return,
                    }
                }
            } else if q.ambiguous {
                // `runNamedQuery(first, second)`: `first` is a project when
                // it names one.
                let first = string_values(model, q.path);
                let is_project = first.as_ref().is_some_and(|v| {
                    v.iter()
                        .any(|l| !l.value.contains('/') && ctx.projects.contains(&l.value.as_str()))
                });
                if is_project {
                    let own = first
                        .is_some_and(|v| v.iter().all(|l| Some(l.value.as_str()) == ctx.project));
                    if !own {
                        return;
                    }
                    match q.path_after_project {
                        Some(second) => path = second,
                        None => return,
                    }
                }
            }
            values_or_prefix(
                model,
                path,
                &mut refs.named_queries,
                &mut refs.dynamic.named_queries,
            );
        }
        ApiTarget::PerspectiveMessage {
            message_type,
            payload,
            scope,
        } => match string_values(model, message_type) {
            Some(types) => {
                let scope = scope
                    .given()
                    .and_then(|s| super::calls::string_value(model, s))
                    .map(|l| l.value);
                let keys = payload_keys(model, payload);
                for t in types {
                    refs.messages.push(MessageSend {
                        message_type: t.value,
                        target: MessageTarget::Perspective {
                            scope: scope.clone(),
                        },
                        payload_keys: keys.clone(),
                    });
                }
            }
            None => refs
                .dynamic
                .perspective_messages
                .push(string_prefix(model, message_type)),
        },
        ApiTarget::GatewayMessage {
            project,
            handler,
            payload,
            ..
        } => {
            let project = if is_project_name_call(model, project) {
                ctx.project.map(str::to_owned)
            } else {
                super::calls::string_value(model, project).map(|l| l.value)
            };
            match string_values(model, handler) {
                Some(handlers) => {
                    let keys = payload_keys(model, payload);
                    for h in handlers {
                        refs.messages.push(MessageSend {
                            message_type: h.value,
                            target: MessageTarget::Gateway {
                                project: project.clone(),
                            },
                            payload_keys: keys.clone(),
                        });
                    }
                }
                None => refs
                    .dynamic
                    .gateway_messages
                    .push((project, string_prefix(model, handler))),
            }
        }
        ApiTarget::TagPaths { paths } => {
            let elements: Vec<&Expr> = match paths {
                Expr::List(l) => l.elts.iter().collect(),
                Expr::Tuple(t) => t.elts.iter().collect(),
                other => vec![other],
            };
            for e in elements {
                if let Some(values) = string_values(model, e) {
                    refs.tag_paths
                        .extend(values.into_iter().map(|l| l.value).filter(|p| {
                            p.starts_with('[')
                                && !p.starts_with("[.]")
                                && !p.starts_with("[~]")
                                && !p.contains('{')
                        }));
                }
            }
        }
    }
}

/// Code run from strings may reach any library name.
fn dynamic_code(
    model: &SemanticModel<'_>,
    callee: &str,
    call: &iglint_python::ast::ExprCall,
    refs: &mut References,
) {
    if !matches!(callee, "eval" | "execfile" | "__import__" | "compile") {
        return;
    }
    if !crate::rules::script::is_builtin_reference(model, &call.func, callee) {
        return;
    }
    match call.args.first() {
        Some(e) if callee == "__import__" => match string_values(model, e) {
            Some(values) => refs.library.extend(values.into_iter().map(|l| l.value)),
            None => refs.dynamic.library.push(string_prefix(model, e)),
        },
        Some(Expr::Str(_)) => {}
        _ => refs.dynamic.library.push(String::new()),
    }
}

/// How an attribute chain is used.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    /// Called.
    Call,
    /// The base of an attribute or subscript that is not followed.
    Base,
    /// Anything else: the object escapes.
    Value,
}

struct Chains<'m, 'a, 'c, 'r> {
    model: &'m SemanticModel<'a>,
    ctx: &'c FactsContext<'c>,
    refs: &'r mut References,
}

fn is_chain(e: &Expr) -> bool {
    match e {
        Expr::Name(_) => true,
        Expr::Attribute(a) => is_chain(&a.value),
        _ => false,
    }
}

impl Chains<'_, '_, '_, '_> {
    /// The dotted name of a chain whose root may be a library name or a
    /// top-level name of the module itself.
    fn dotted(&self, e: &Expr) -> Option<String> {
        if let Some(q) = super::calls::qualified(self.model, e, UnboundNames::AsGlobalEvenIfDynamic)
        {
            let root = q.split('.').next().unwrap_or("");
            if matches!(root, "system" | "sys" | "__builtins__") || is_builtin_root(self.model, e) {
                return None;
            }
            return Some(q);
        }
        let module = self.ctx.module?;
        match e {
            Expr::Name(n) => {
                let r = self.model.reference_at(n.range)?;
                let module_level = match r.resolution {
                    Resolution::Global => true,
                    Resolution::Local => r.scope == ScopeId::MODULE,
                    _ => false,
                };
                (module_level
                    && r.bindings
                        .iter()
                        .all(|b| self.model.binding(*b).scope == ScopeId::MODULE))
                .then(|| format!("{module}.{}", n.id))
            }
            Expr::Attribute(a) => {
                let mut base = self.dotted(&a.value)?;
                base.push('.');
                base.push_str(a.attr.as_str());
                Some(base)
            }
            _ => None,
        }
    }

    fn record(&mut self, e: &Expr, role: Role) {
        let Some(q) = self.dotted(e) else {
            return;
        };
        if role == Role::Value {
            self.refs.dynamic.library.push(q.clone());
        }
        self.refs.library.push(q);
    }

    fn expr(&mut self, e: &Expr, role: Role) {
        match e {
            Expr::Name(n) => {
                if matches!(n.ctx, ExprContext::Load | ExprContext::AugLoad) {
                    self.record(e, role);
                }
            }
            Expr::Attribute(a) if is_chain(e) => {
                if matches!(a.ctx, ExprContext::Load | ExprContext::AugLoad) {
                    self.record(e, role);
                } else {
                    self.record(&a.value, Role::Base);
                }
            }
            Expr::Attribute(a) => self.expr(&a.value, Role::Base),
            Expr::Call(c) => {
                self.expr(&c.func, Role::Call);
                for a in &c.args {
                    self.expr(a, Role::Value);
                }
                for k in &c.keywords {
                    self.expr(&k.value, Role::Value);
                }
                if let Some(s) = &c.starargs {
                    self.expr(s, Role::Value);
                }
                if let Some(k) = &c.kwargs {
                    self.expr(k, Role::Value);
                }
            }
            Expr::Subscript(s) => {
                self.expr(&s.value, Role::Base);
                self.visit_slice(&s.slice);
            }
            other => walk_expr(self, other),
        }
    }
}

fn is_builtin_root(model: &SemanticModel<'_>, e: &Expr) -> bool {
    let mut root = e;
    while let Expr::Attribute(a) = root {
        root = &a.value;
    }
    let Expr::Name(n) = root else { return false };
    model
        .reference_at(n.range)
        .is_some_and(|r| r.resolution == Resolution::Builtin)
}

impl<'a> Visitor<'a> for Chains<'_, '_, '_, '_> {
    fn visit_expr(&mut self, expr: &'a Expr) {
        self.expr(expr, Role::Value);
    }

    fn visit_slice(&mut self, slice: &'a Slice) {
        iglint_python::visitor::walk_slice(self, slice);
    }

    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::ImportFrom(i) if i.names.iter().any(|a| a.name.as_str() == "*") => {
                if let Some(m) = &i.module
                    && i.level == 0
                {
                    self.refs.dynamic.library.push(m.as_str().to_owned());
                }
            }
            Stmt::Exec(e) => {
                if !matches!(&*e.body, Expr::Str(_)) {
                    self.refs.dynamic.library.push(String::new());
                }
            }
            Stmt::Assign(a) => {
                for t in &a.targets {
                    style_classes(self.model, t, &a.value, self.refs);
                }
            }
            _ => {}
        }
        walk_stmt(self, stmt);
    }
}

/// `x.style.classes = 'A B'`, `x.style['classes'] = ...` and
/// `x.style = {'classes': ...}`.
fn style_classes(model: &SemanticModel<'_>, target: &Expr, value: &Expr, refs: &mut References) {
    let is_style = |e: &Expr| matches!(e, Expr::Attribute(a) if a.attr.as_str() == "style");
    let classes_value = match target {
        Expr::Attribute(a) if a.attr.as_str() == "classes" && is_style(&a.value) => Some(value),
        Expr::Subscript(s) if is_style(&s.value) && slice_is(&s.slice, "classes") => Some(value),
        Expr::Attribute(a) if a.attr.as_str() == "style" => match value {
            Expr::Dict(d) => d
                .keys
                .iter()
                .position(|k| matches!(k, Expr::Str(s) if s.s.as_str() == Some("classes")))
                .map(|i| &d.values[i]),
            _ => None,
        },
        _ => None,
    };
    if let Some(v) = classes_value
        && let Some(values) = string_values(model, v)
    {
        for l in values {
            refs.style_classes
                .extend(l.value.split_whitespace().map(str::to_owned));
        }
    }
}

fn slice_is(slice: &Slice, key: &str) -> bool {
    matches!(slice, Slice::Index(i) if matches!(&*i.value, Expr::Str(s) if s.s.as_str() == Some(key)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use iglint_python_semantic::SemanticOptions;

    fn refs_of(src: &str, module: Option<&str>) -> References {
        let parsed = iglint_python::parse_module(src);
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let model = SemanticModel::build(
            &parsed.module,
            &SemanticOptions {
                injected: vec!["system", "fpmi", "sys"],
                ..SemanticOptions::default()
            },
        );
        let mut refs = References::default();
        let ctx = FactsContext {
            project: Some("p"),
            projects: &["p", "other"],
            module,
        };
        collect(&model, &ctx, &mut refs);
        refs
    }

    #[test]
    fn views_pages_queries() {
        let r = refs_of(
            "V = 'Popups/B'\nsystem.perspective.openPopup('id', 'Popups/A')\nsystem.perspective.openPopup('id', V)\nsystem.perspective.openPopup('id', 'Dyn/' + x)\nsystem.perspective.navigate('/home')\nsystem.perspective.navigate(view='Main')\nsystem.db.runNamedQuery('Q/A', {})\nsystem.db.runNamedQuery('other', 'Q/B', {})\nsystem.db.runNamedQuery('p', 'Q/C', {})\nsystem.db.runNamedQuery(system.util.getProjectName(), 'Q/D')\nsystem.db.runNamedQuery('Q/' + n)\nsystem.db.runNamedQuery('other', params)\nsystem.db.runNamedQuery('Q/E', params)\n",
            None,
        );
        assert_eq!(r.views, ["Popups/A", "Popups/B", "Main"]);
        assert_eq!(r.dynamic.views, ["Dyn/"]);
        assert_eq!(r.pages, ["/home"]);
        assert_eq!(r.named_queries, ["Q/A", "Q/C", "Q/D", "Q/E"]);
        assert_eq!(r.dynamic.named_queries, ["Q/"]);
    }

    #[test]
    fn messages() {
        let r = refs_of(
            "system.perspective.sendMessage('a', {'x': 1}, scope='session')\nsystem.perspective.sendMessage(t)\nsystem.util.sendMessage(system.util.getProjectName(), 'h', payload)\nsystem.util.sendRequest('other', 'r')\nsystem.util.sendMessage('p', 'pre-' + k)\n",
            None,
        );
        assert_eq!(
            r.messages,
            [
                MessageSend {
                    message_type: "a".into(),
                    target: MessageTarget::Perspective {
                        scope: Some("session".into())
                    },
                    payload_keys: Some(vec!["x".into()]),
                },
                MessageSend {
                    message_type: "h".into(),
                    target: MessageTarget::Gateway {
                        project: Some("p".into())
                    },
                    payload_keys: None,
                },
                MessageSend {
                    message_type: "r".into(),
                    target: MessageTarget::Gateway {
                        project: Some("other".into())
                    },
                    payload_keys: Some(vec![]),
                },
            ]
        );
        assert_eq!(r.dynamic.perspective_messages, [""]);
        assert_eq!(
            r.dynamic.gateway_messages,
            [(Some("p".to_owned()), "pre-".to_owned())]
        );
    }

    #[test]
    fn library_names_and_escapes() {
        let r = refs_of(
            "from core.fmt import unit\nimport core.tags as t\ncore.util.go(1)\nt.read()\nsystem.util.invokeAsynchronous(core.jobs.run)\nhandle(core.mod)\nlen(x)\n",
            None,
        );
        for name in [
            "core.util.go",
            "core.tags.read",
            "core.jobs.run",
            "core.mod",
            "core.fmt.unit",
        ] {
            assert!(
                r.library.iter().any(|n| n == name),
                "{name}: {:?}",
                r.library
            );
        }
        assert!(
            !r.library
                .iter()
                .any(|n| n.starts_with("system") || n == "len")
        );
        // `x` is bound nowhere: it may be a library package passed as a value.
        assert_eq!(r.dynamic.library, ["core.jobs.run", "core.mod", "x"]);
        let r = refs_of("from core.util import *\nexec code\n", None);
        assert_eq!(r.dynamic.library, ["core.util", "", "code"]);
    }

    #[test]
    fn module_self_references() {
        let r = refs_of(
            "def helper(a):\n    return a\ndef api(x):\n    return helper(x)\nLOOKUP = {'k': helper}\n",
            Some("core.util"),
        );
        assert_eq!(
            r.library
                .iter()
                .filter(|n| *n == "core.util.helper")
                .count(),
            2
        );
        assert_eq!(r.dynamic.library, ["core.util.helper"]);
    }

    #[test]
    fn style_classes_and_tags() {
        let r = refs_of(
            "self.props.style.classes = 'A B/C'\nself.props.style = {'classes': 'D'}\nc.style['classes'] = 'E' if x else 'F'\nsystem.tag.readBlocking(['[default]a/b', '[.]rel', p])\nsystem.tag.writeBlocking('[default]c', 1)\n",
            None,
        );
        assert_eq!(r.style_classes, ["A", "B/C", "D", "E", "F"]);
        assert_eq!(r.tag_paths, ["[default]a/b", "[default]c"]);
    }
}
