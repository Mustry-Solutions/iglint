//! SEC004 insecure-url.

use iglint_project::view::{Binding, BindingConfig};
use iglint_python::ast::{Expr, Operator};
use iglint_python_semantic::SemanticModel;
use iglint_text::{Ranged, TextRange};

use super::{
    MAX_DEPTH, argument, produced_by, single_assigned_value, string_literal, system_argument,
};
use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{BindingRule, Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{report, semantic, walk_exprs};

pub static META: RuleMeta = RuleMeta {
    code: "SEC004",
    name: "insecure-url",
    category: Category::Sec,
    default_severity: Severity::Info,
    default_enabled: true,
    summary: "Plain `http://` URL in an HTTP binding or `system.net` call",
    explanation: r#"## What it does

Reports literal `http://` URLs:

* in the `request.url` of HTTP bindings;
* passed as the URL of `system.net.httpGet`, `httpPost`, `httpPut`,
  `httpDelete` and `openURL` (first argument or `url` keyword);
* passed to the request methods (`get`, `post`, `put`, `delete`, `patch`,
  `head`, `options`, `trace`, their `Async` forms, and `request`) of a
  client made by `system.net.httpClient()`, called directly or through a
  variable holding the client.

The functions are recognised through imports and aliases. The URL is a
string literal, a name whose single reaching assignment is a literal, or
text built with `+`, `%` or `.format(...)` that starts with such a
literal. Only URLs whose literal part includes the host are reported
(`"http://" + host` is not).

Loopback hosts are not reported: `localhost`, `127.0.0.0/8` and `[::1]`.
Traffic to them does not leave the machine.

## Why is this bad?

Plain HTTP sends the request, its headers (including credentials and
session tokens) and the response unencrypted and unauthenticated over the
network. Anyone on the path can read or change them. Use `https://`; for
a device without TLS, keep it on an isolated network and document why.

## Example

```python
client = system.net.httpClient()
response = client.get("http://erp.example.com/api/orders")
```

Use instead:

```python
client = system.net.httpClient()
response = client.get("https://erp.example.com/api/orders")
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Script(&InsecureUrl), Hook::Binding(&InsecureUrl)],
    options: None,
};

struct InsecureUrl;

/// `system.net` functions that take a URL.
const URL_FUNCTIONS: &[&str] = &[
    "system.net.httpDelete",
    "system.net.httpGet",
    "system.net.httpPost",
    "system.net.httpPut",
    "system.net.openURL",
];

/// Methods of `system.net.httpClient()`: name and URL position.
fn client_method(name: &str) -> Option<usize> {
    let base = name.strip_suffix("Async").unwrap_or(name);
    match base {
        "get" | "post" | "put" | "delete" | "patch" | "head" | "options" | "trace" => Some(0),
        "request" => Some(1),
        _ => None,
    }
}

/// The host of an `http://` URL at the start of `text`, when the URL is
/// plain HTTP and its host is present and not a loopback address.
fn insecure_host(text: &str) -> Option<&str> {
    let t = text.trim_start();
    let rest = match t.split_at_checked(7) {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http://") => rest,
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if let Some(v6) = host_port.strip_prefix('[') {
        v6.split_once(']')?.0
    } else {
        host_port.split(':').next().unwrap_or("")
    };
    if host.is_empty() || is_loopback(host) {
        return None;
    }
    Some(host)
}

fn is_loopback(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") || host == "::1" {
        return true;
    }
    let parts: Vec<&str> = host.split('.').collect();
    parts.len() == 4
        && parts[0] == "127"
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 3 && p.bytes().all(|b| b.is_ascii_digit()))
}

/// The string literal at the start of the URL `expr`: the literal itself,
/// a name whose single reaching assignment is one, or the left operand of
/// `+` and `%`, or the receiver of `.format(...)`.
fn leading_literal<'e>(model: &SemanticModel<'e>, expr: &'e Expr) -> Option<&'e Expr> {
    let mut at = expr;
    for _ in 0..MAX_DEPTH {
        match at {
            Expr::Str(_) => return Some(at),
            Expr::BinOp(b) if matches!(b.op, Operator::Add | Operator::Mod) => at = &b.left,
            Expr::Call(c) => match &*c.func {
                Expr::Attribute(a) if &*a.attr.id == "format" => at = &a.value,
                _ => return None,
            },
            Expr::Name(_) => at = single_assigned_value(model, at)?,
            _ => return None,
        }
    }
    None
}

fn message(host: &str) -> String {
    format!("Plain `http://` URL to `{host}`; use `https://`")
}

impl ScriptRule for InsecureUrl {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        let mut found: Vec<(TextRange, String)> = Vec::new();
        walk_exprs(&unit.parsed.module.body, &mut |e| {
            let Expr::Call(call) = e else { return };
            let url = if let Some(callee) = model.callee(call)
                && URL_FUNCTIONS.contains(&callee.as_str())
            {
                system_argument(call, &callee, "url")
            } else if let Expr::Attribute(a) = &*call.func
                && let Some(position) = client_method(&a.attr.id)
                && produced_by(model, &a.value).as_deref() == Some("system.net.httpClient")
            {
                argument(call, "url", Some(position))
            } else {
                None
            };
            let Some(literal) = url.and_then(|u| leading_literal(model, u)) else {
                return;
            };
            let Some(text) = string_literal(literal) else {
                return;
            };
            if let Some(host) = insecure_host(&text)
                && !found.iter().any(|(r, _)| *r == literal.range())
            {
                found.push((literal.range(), message(host)));
            }
        });
        found.sort_by_key(|(r, _)| r.start());
        for (range, message) in found {
            report(cx, unit, range, message);
        }
    }
}

impl BindingRule for InsecureUrl {
    fn check_binding(&self, cx: &mut Checker<'_, '_>, binding: &Binding<'_>) {
        let BindingConfig::Http(http) = &binding.typed else {
            return;
        };
        let Some(url) = http.url else { return };
        // A literal URL, or an expression starting with a quoted literal.
        let text = url.value().trim_start();
        let text = text.strip_prefix(['\'', '"']).unwrap_or(text);
        if let Some(host) = insecure_host(text) {
            cx.report(url.range(), message(host))
                .node_path(binding.node_path.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::insecure_host;
    use crate::rules::script::testing::*;
    use crate::rules::test_support::Fixture;

    fn flagged_in(code: &str) -> Vec<String> {
        flagged(&lint_module("SEC004", code))
    }

    #[test]
    fn hosts() {
        assert_eq!(
            insecure_host("http://erp.example.com/x"),
            Some("erp.example.com")
        );
        assert_eq!(insecure_host("HTTP://10.0.0.5:8080"), Some("10.0.0.5"));
        assert_eq!(insecure_host("http://u:p@h.example/x"), Some("h.example"));
        assert_eq!(insecure_host("http://[fe80::1]:80/"), Some("fe80::1"));
        for url in [
            "https://erp.example.com",
            "http://localhost:8088/x",
            "http://127.0.0.1/x",
            "http://127.1.2.3:80",
            "http://[::1]:8088/",
            "http://",
            "http://:80/",
            "ftp://x.example",
        ] {
            assert_eq!(insecure_host(url), None, "{url}");
        }
    }

    #[test]
    fn reports_system_net_calls_and_clients() {
        let code = r#"BASE = "http://erp.example.com/api"
net = system.net
from system.net import httpClient as hc
def f(path, id):
    system.net.httpGet("http://a.example/x")
    net.httpPost(url="http://b.example/x", postData="")
    system.net.httpGet(BASE + path)
    client = system.net.httpClient()
    client.get("http://c.example/%s" % id)
    client.postAsync("http://d.example/{}".format(id))
    hc().request("GET", "http://e.example/x")
    system.net.openURL("http://f.example")
"#;
        assert_eq!(
            flagged_in(code),
            [
                r#""http://erp.example.com/api""#,
                r#""http://a.example/x""#,
                r#""http://b.example/x""#,
                r#""http://c.example/%s""#,
                r#""http://d.example/{}""#,
                r#""http://e.example/x""#,
                r#""http://f.example""#,
            ]
        );
    }

    #[test]
    fn secure_loopback_and_unknown_are_fine() {
        let code = r#"def f(host, url, other, system2):
    system.net.httpGet("https://a.example/x")
    system.net.httpGet("http://localhost:8088/system/x")
    system.net.httpGet("http://127.0.0.1/x")
    system.net.httpGet("http://" + host + "/x")
    system.net.httpGet(url)
    other.get("http://a.example/x")
    client = system.net.httpClient()
    client.get(url)
    client.getTimeout()
    requests.get("http://a.example/x")
    x = "http://a.example/x"
    if other:
        x = "https://a.example"
    client.get(x)
"#;
        let found = flagged_in(code);
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn shadowed_system_is_not_the_api() {
        let code = "def f(system):\n    system.net.httpGet('http://a.example/x')\n";
        assert!(flagged_in(code).is_empty());
    }

    #[test]
    fn everywhere_and_gateway_events() {
        assert_eq!(
            flagged_everywhere(
                "SEC004",
                "\tc = system.net.httpClient()\n\tr = c.get(\"http://a.example/\\\"q\\\"\")"
            ),
            ["\"http://a.example/\\\"q\\\"\""]
        );
        let f = gateway_message(
            "def handleMessage(payload):\n\tsystem.net.httpGet('http://a.example/x')\n",
        );
        assert_eq!(flagged(&f.lint(&["SEC004"])), ["'http://a.example/x'"]);
    }

    fn http_view(url: &str) -> Fixture {
        Fixture::view(&format!(
            r#"{{"custom": {{"x": 1}}, "propConfig": {{"custom.x": {{"binding": {{"type": "http", "config": {{"request": {{"url": {url}, "method": "GET"}}}}}}}}}},
               "root": {{"type": "ia.container.flex", "meta": {{"name": "root"}}}}}}"#
        ))
    }

    #[test]
    fn http_bindings() {
        let f = http_view(r#""http://a.example/x""#);
        assert_eq!(flagged(&f.lint(&["SEC004"])), [r#""http://a.example/x""#]);
        let f = http_view(r#""'http://a.example/' + {view.params.id}""#);
        assert_eq!(
            flagged(&f.lint(&["SEC004"])),
            [r#""'http://a.example/' + {view.params.id}""#]
        );
        for ok in [
            r#""https://a.example/x""#,
            r#""http://localhost/x""#,
            r#""{view.params.url}""#,
        ] {
            assert!(http_view(ok).lint(&["SEC004"]).is_empty(), "{ok}");
        }
    }
}
