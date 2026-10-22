//! What calls of the scripting API refer to: views, pages, named queries,
//! message handlers and tags.
//!
//! [`api_target`] reads a resolved call ([`CallSite`]) of a known
//! `system.*` function and returns the arguments that name other
//! resources. Parameter positions and names are those of the Ignition 8.3
//! stubs (`iglint_stubs`); the named query functions accept two calling
//! conventions in the gateway scope (`(project, path, parameters)` and
//! `(path, parameters)`), which [`named_query_call`] tells apart.

use iglint_python::ast::Expr;
use iglint_python_semantic::SemanticModel;

use super::calls::{Arg, CallSite, dict_keys, string_value};

/// A resource reference made by an API call.
#[derive(Clone, Debug)]
pub enum ApiTarget<'a> {
    /// A view path (`openPopup(id, view)`, `navigate(view=...)`), with the
    /// view parameters passed along, if any.
    View { path: &'a Expr, params: Arg<'a> },
    /// A page URL (`navigate(page)`).
    Page { url: &'a Expr },
    /// A named query.
    NamedQuery(NamedQueryCall<'a>),
    /// `system.perspective.sendMessage`.
    PerspectiveMessage {
        message_type: &'a Expr,
        payload: Arg<'a>,
        scope: Arg<'a>,
    },
    /// `system.util.sendMessage`, `sendRequest`, `sendRequestAsync`.
    GatewayMessage {
        project: &'a Expr,
        handler: &'a Expr,
        payload: Arg<'a>,
        /// The `scope` argument of `sendMessage`; requests only reach
        /// gateway handlers (`request` is true).
        scope: Arg<'a>,
        request: bool,
    },
    /// Tag paths read or written: a single path or a list of paths.
    TagPaths { paths: &'a Expr },
}

/// How a named query call names its query.
#[derive(Clone, Debug)]
pub struct NamedQueryCall<'a> {
    /// The project argument, when the call passes one.
    pub project: Option<&'a Expr>,
    pub path: &'a Expr,
    /// The parameters argument (`None` for functions without one).
    pub params: Option<Arg<'a>>,
    /// The call matches both conventions: `(path, parameters)` was assumed,
    /// but the first argument may be a project name.
    pub ambiguous: bool,
    /// For ambiguous calls: the path if the first argument is a project.
    pub path_after_project: Option<&'a Expr>,
}

/// Functions whose first (or `path`) argument names a named query.
const NAMED_QUERY_FUNCTIONS: &[(&str, Option<&str>)] = &[
    ("system.db.runNamedQuery", Some("parameters")),
    ("system.db.runSFNamedQuery", Some("params")),
    ("system.db.clearNamedQueryCache", None),
];

/// Tag functions and the parameter holding the paths (functions that
/// create tags or test for their existence are not listed).
const TAG_FUNCTIONS: &[(&str, &str)] = &[
    ("system.tag.readBlocking", "tagPaths"),
    ("system.tag.readAsync", "tagPaths"),
    ("system.tag.writeBlocking", "tagPaths"),
    ("system.tag.writeAsync", "tagPaths"),
    ("system.tag.read", "tagPath"),
    ("system.tag.readAll", "tagPaths"),
    ("system.tag.write", "tagPath"),
    ("system.tag.writeAll", "tagPaths"),
    ("system.tag.queryTagHistory", "paths"),
];

/// The resource reference a call makes, if it is a call of a known API
/// function with the referencing argument given.
pub fn api_target<'a>(model: &SemanticModel<'_>, site: &CallSite<'a>) -> Option<ApiTarget<'a>> {
    let callee = site.callee.as_str();
    match callee {
        "system.perspective.openPopup" | "system.perspective.togglePopup" => {
            Some(ApiTarget::View {
                path: site.arg(1, "view").given()?,
                params: site.arg(2, "params"),
            })
        }
        "system.perspective.navigate" => {
            if let Some(view) = site.arg(2, "view").given() {
                return Some(ApiTarget::View {
                    path: view,
                    params: site.arg(3, "params"),
                });
            }
            Some(ApiTarget::Page {
                url: site.arg(0, "page").given()?,
            })
        }
        "system.perspective.sendMessage" => Some(ApiTarget::PerspectiveMessage {
            message_type: site.arg(0, "messageType").given()?,
            payload: site.arg(1, "payload"),
            scope: site.arg(2, "scope"),
        }),
        "system.util.sendMessage" => Some(ApiTarget::GatewayMessage {
            project: site.arg(0, "project").given()?,
            handler: site.arg(1, "messageHandler").given()?,
            payload: site.arg(2, "payload"),
            scope: site.arg(3, "scope"),
            request: false,
        }),
        "system.util.sendRequest" | "system.util.sendRequestAsync" => {
            Some(ApiTarget::GatewayMessage {
                project: site.arg(0, "project").given()?,
                handler: site.arg(1, "messageHandler").given()?,
                payload: site.arg(2, "payload"),
                scope: Arg::Absent,
                request: true,
            })
        }
        "system.db.execQuery" | "system.db.execUpdate" => {
            Some(ApiTarget::NamedQuery(NamedQueryCall {
                project: site
                    .call
                    .keywords
                    .iter()
                    .find(|k| k.arg.as_str() == "project")
                    .map(|k| &k.value)
                    .or_else(|| {
                        let index = if callee.ends_with("execQuery") { 3 } else { 4 };
                        site.call.args.get(index)
                    }),
                path: site.arg(0, "path").given()?,
                params: Some(site.arg(1, "parameters")),
                ambiguous: false,
                path_after_project: None,
            }))
        }
        _ => {
            if let Some((_, params)) = NAMED_QUERY_FUNCTIONS.iter().find(|(f, _)| *f == callee) {
                return named_query_call(model, site, *params).map(ApiTarget::NamedQuery);
            }
            if let Some((_, param)) = TAG_FUNCTIONS.iter().find(|(f, _)| *f == callee) {
                return Some(ApiTarget::TagPaths {
                    paths: site.arg(0, param).given()?,
                });
            }
            None
        }
    }
}

/// Reads `runNamedQuery`-style calls: `(path)`, `(path, parameters, ...)`,
/// `(project, path, parameters, ...)` and keyword forms.
fn named_query_call<'a>(
    model: &SemanticModel<'_>,
    site: &CallSite<'a>,
    params_name: Option<&str>,
) -> Option<NamedQueryCall<'a>> {
    let call = site.call;
    let keyword = |name: &str| {
        call.keywords
            .iter()
            .find(|k| k.arg.as_str() == name)
            .map(|k| &k.value)
    };
    let params_kw = params_name.and_then(keyword);
    let positional = &call.args;
    let params_at = |index: usize| -> Option<Arg<'a>> {
        let name = params_name?;
        Some(match (positional.get(index), keyword(name)) {
            (Some(e), _) => Arg::Given(e),
            (None, Some(e)) => Arg::Given(e),
            _ if call.starargs.is_some() || call.kwargs.is_some() => Arg::Unknown,
            _ => Arg::Absent,
        })
    };
    if let Some(path) = keyword("path") {
        return Some(NamedQueryCall {
            project: keyword("project").or_else(|| positional.first()),
            path,
            params: params_name.map(|_| match params_kw {
                Some(e) => Arg::Given(e),
                None if call.starargs.is_some() || call.kwargs.is_some() => Arg::Unknown,
                None => Arg::Absent,
            }),
            ambiguous: false,
            path_after_project: None,
        });
    }
    let first = positional.first()?;
    if let Some(project) = keyword("project") {
        return Some(NamedQueryCall {
            project: Some(project),
            path: first,
            params: params_at(1),
            ambiguous: false,
            path_after_project: None,
        });
    }
    match positional.get(1) {
        None => Some(NamedQueryCall {
            project: None,
            path: first,
            params: params_at(1),
            ambiguous: false,
            path_after_project: None,
        }),
        Some(second) if string_value(model, second).is_some() => Some(NamedQueryCall {
            project: Some(first),
            path: second,
            params: params_at(2),
            ambiguous: false,
            path_after_project: None,
        }),
        Some(second) => Some(NamedQueryCall {
            project: None,
            path: first,
            params: params_at(1),
            ambiguous: dict_keys(model, second).is_none() && !is_mapping_literal(second),
            path_after_project: Some(second),
        }),
    }
}

/// `{...}` literals (including dict comprehensions) are parameters, never
/// project names.
fn is_mapping_literal(e: &Expr) -> bool {
    matches!(e, Expr::Dict(_) | Expr::DictComp(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script_facts::calls::{call_sites, string_value};
    use iglint_python_semantic::{SemanticOptions, UnboundNames};

    fn targets(src: &str) -> Vec<String> {
        let parsed = iglint_python::parse_module(src);
        assert!(parsed.errors.is_empty());
        let model = SemanticModel::build(
            &parsed.module,
            &SemanticOptions {
                injected: vec!["system"],
                ..SemanticOptions::default()
            },
        );
        let lit = |e: &Expr| string_value(&model, e).map_or("?".to_owned(), |l| l.value);
        call_sites(&model, &parsed.module.body, UnboundNames::Reject)
            .iter()
            .filter_map(|s| api_target(&model, s))
            .map(|t| match t {
                ApiTarget::View { path, params } => {
                    format!("view {} {}", lit(path), params.given().is_some())
                }
                ApiTarget::Page { url } => format!("page {}", lit(url)),
                ApiTarget::NamedQuery(q) => format!(
                    "query {} project={} params={} ambiguous={}",
                    lit(q.path),
                    q.project.map_or("-".to_owned(), lit),
                    match q.params {
                        Some(Arg::Given(_)) => "given",
                        Some(Arg::Absent) => "absent",
                        Some(Arg::Unknown) => "unknown",
                        None => "none",
                    },
                    q.ambiguous
                ),
                ApiTarget::PerspectiveMessage { message_type, .. } => {
                    format!("message {}", lit(message_type))
                }
                ApiTarget::GatewayMessage {
                    project, handler, ..
                } => format!("handler {} {}", lit(project), lit(handler)),
                ApiTarget::TagPaths { .. } => "tags".to_owned(),
            })
            .collect()
    }

    #[test]
    fn perspective_calls() {
        assert_eq!(
            targets(
                "system.perspective.openPopup('id', 'Popups/A', {'x': 1})\nsystem.perspective.navigate('/page')\nsystem.perspective.navigate(view='V')\nsystem.perspective.sendMessage('m', {})\n"
            ),
            [
                "view Popups/A true",
                "page /page",
                "view V false",
                "message m"
            ]
        );
    }

    #[test]
    fn named_query_conventions() {
        assert_eq!(
            targets(
                "system.db.runNamedQuery('A/B')\nsystem.db.runNamedQuery('A/B', {'p': 1})\nsystem.db.runNamedQuery('proj', 'A/B', {})\nsystem.db.runNamedQuery('A', params)\nsystem.db.runNamedQuery(path='Q', parameters=p)\nsystem.db.execQuery('Q', project='x')\n"
            ),
            [
                "query A/B project=- params=absent ambiguous=false",
                "query A/B project=- params=given ambiguous=false",
                "query A/B project=proj params=given ambiguous=false",
                "query A project=- params=given ambiguous=true",
                "query Q project=- params=given ambiguous=false",
                "query Q project=x params=absent ambiguous=false",
            ]
        );
    }

    #[test]
    fn messages_and_tags() {
        assert_eq!(
            targets(
                "system.util.sendMessage('p', 'h', {})\nsystem.util.sendRequest(project='p', messageHandler='h')\nsystem.tag.readBlocking(['[default]a'])\nsystem.tag.exists('[default]a')\n"
            ),
            ["handler p h", "handler p h", "tags"]
        );
    }
}
