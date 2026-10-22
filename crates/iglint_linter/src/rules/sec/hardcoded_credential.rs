//! SEC002 hardcoded-credential.

use std::borrow::Cow;

use iglint_json::Value;
use iglint_project::view::{Binding, BindingKind};
use iglint_python::ast::{Expr, Slice, Stmt};
use iglint_text::{Ranged, TextRange};

use super::{string_literal, system_positional_names};
use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{BindingRule, Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{report, semantic, walk_exprs, walk_stmts};

pub static META: RuleMeta = RuleMeta {
    code: "SEC002",
    name: "hardcoded-credential",
    category: Category::Sec,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "Password, token or key written in a script or HTTP binding",
    explanation: r#"## What it does

Reports string literals that hold a password, token, secret or API key:

* in scripts: a literal assigned to a name, attribute or dict key whose
  name ends in a credential word (`password`, `passwd`, `pwd`, `secret`,
  `token`, `passphrase`, `apiKey`, `api_key`, `accessKey`, `secretKey`,
  `privateKey`, `clientSecret`, ...), a dict entry or keyword argument with
  such a name, a positional argument of a `system.*` function whose
  parameter is named `password` in the stubs, the password argument of
  `java.sql.DriverManager.getConnection(url, user, password)`, properties
  and headers set with `setProperty`/`put`/`setHeader`/`addHeader`/
  `setRequestProperty`, and `Authorization` header values;
* in HTTP bindings: the `request.auth` value (for any type other than
  `none`) and `Authorization` or credential-named headers;
* anywhere: a password in a URL (`scheme://user:password@host`).

To keep findings certain, values that cannot be a real credential are
skipped: empty or short strings (fewer than 4 characters), `None`, text
with spaces, placeholders (`<password>`, `{token}`, `${SECRET}`, `%s`,
`****`, `changeme`, `example`, ...), paths and tag paths, values with
property references, and values that merely name a field or key
(`PASSWORD_KEY = "password"`, `token = "access_token"`). Names such as
`passwordField` or `tokenLength`, whose last word is not a credential
word, are not reported.

## Why is this bad?

A credential in a project is readable by everyone who can open the
project, export it, or read the repository and its backups, and it
travels to every client that loads the view. It cannot be rotated
without editing and redeploying the project, and the same value usually
ends up in several places.

Keep secrets out of the project: use the gateway's secret providers
(`secretName` parameters), a database or identity provider connection
configured on the gateway, or a value read at run time from a protected
memory tag or gateway setting.

## Example

```python
def sync():
    system.net.httpPost(URL, "application/json", body, username="svc", password="S3cr3t!x")
```

Use instead:

```python
def sync():
    password = system.tag.readBlocking(["[default]Config/Sync/Password"])[0].value
    system.net.httpPost(URL, "application/json", body, username="svc", password=password)
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[
        Hook::Script(&HardcodedCredential),
        Hook::Binding(&HardcodedCredential),
    ],
    options: None,
};

struct HardcodedCredential;

/// The lowercase words of an identifier or key: split at `_`, `-`, `.`,
/// spaces and camel-case boundaries (`apiKey`, `API_KEY`, `X-Api-Key`).
fn words(name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = name.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            continue;
        }
        let boundary = c.is_uppercase()
            && i > 0
            && (chars[i - 1].is_lowercase()
                || chars[i - 1].is_ascii_digit()
                || (chars[i - 1].is_uppercase()
                    && chars.get(i + 1).is_some_and(|n| n.is_lowercase())));
        if boundary && !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
        current.extend(c.to_lowercase());
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Words that name a credential when they end a name.
const CREDENTIAL_WORDS: &[&str] = &[
    "password",
    "passwd",
    "pwd",
    "passphrase",
    "secret",
    "token",
    "apikey",
    "accesskey",
    "secretkey",
    "privatekey",
    "clientsecret",
];

/// Whether the identifier or key `name` names a credential: its last word
/// is a credential word, or it ends in `api key`, `access key`, `secret
/// key`, `private key` or `auth key`.
pub(crate) fn is_credential_name(name: &str) -> bool {
    let w = words(name);
    match w.as_slice() {
        [.., last] if CREDENTIAL_WORDS.contains(&last.as_str()) => true,
        [.., qualifier, last] => {
            last == "key"
                && matches!(
                    qualifier.as_str(),
                    "api" | "access" | "secret" | "private" | "auth" | "subscription"
                )
        }
        _ => false,
    }
}

fn is_authorization(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization" | "proxy-authorization"
    )
}

/// Substrings of values that are placeholders, not credentials.
const PLACEHOLDERS: &[&str] = &[
    "changeme",
    "change_me",
    "change-me",
    "example",
    "placeholder",
    "dummy",
    "redacted",
    "your",
    "xxxx",
    "****",
    "....",
    "todo",
    "tbd",
    "fixme",
    "replace",
    "insert",
    "enter",
];

/// Whether the literal `value` can be a real credential (see the rule
/// documentation for what is skipped).
pub(crate) fn is_credential_value(value: &str) -> bool {
    let v = value.trim();
    if v.chars().count() < 4 || v.chars().any(char::is_whitespace) {
        return false;
    }
    let lower = v.to_ascii_lowercase();
    if PLACEHOLDERS.iter().any(|p| lower.contains(p))
        || matches!(
            lower.as_str(),
            "none" | "null" | "undefined" | "nil" | "true" | "false"
        )
    {
        return false;
    }
    // Repeated single characters: `aaaa`, `----`.
    if v.chars().all(|c| v.starts_with(c)) {
        return false;
    }
    // Formats, references, templates and paths.
    if v.contains(['{', '}', '$', '%', '/', '\\', '[', ']', '<', '>']) {
        return false;
    }
    // A value that names a field or key (`"password"`, `"api_key"`,
    // `"session.custom.token"`).
    let identifier_like = v
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':'));
    if identifier_like
        && (words(v)
            .iter()
            .any(|w| CREDENTIAL_WORDS.contains(&w.as_str()) || w == "key" || w == "pass")
            || ["password", "passwd", "secret", "token", "apikey"]
                .iter()
                .any(|w| lower.contains(w)))
    {
        return false;
    }
    true
}

/// Whether an `Authorization` header value holds a literal credential:
/// `Basic <...>`, `Bearer <...>` or a bare value.
fn is_authorization_value(value: &str) -> bool {
    let v = value.trim();
    let credential = match v.split_once(char::is_whitespace) {
        Some((scheme, rest))
            if scheme.chars().all(|c| c.is_ascii_alphabetic()) && !rest.trim().is_empty() =>
        {
            rest.trim()
        }
        Some(_) => return false,
        None => v,
    };
    credential.len() >= 8 && is_credential_value(credential)
}

/// The password of a URL with user information (`scheme://user:pw@host`),
/// as a byte range of `text`.
fn url_password(text: &str) -> Option<std::ops::Range<usize>> {
    let scheme_end = text.find("://")? + 3;
    let rest = &text[scheme_end..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let at = authority.rfind('@')?;
    let userinfo = &authority[..at];
    let colon = userinfo.find(':')?;
    let password = &userinfo[colon + 1..];
    if !is_credential_value(password) {
        return None;
    }
    let start = scheme_end + colon + 1;
    Some(start..start + password.len())
}

/// Methods that set a named property or header: `(name, value)`.
const SETTERS: &[&str] = &[
    "setProperty",
    "put",
    "setHeader",
    "addHeader",
    "setRequestProperty",
    "header",
];

/// What a literal is for: the name it is assigned to, or `Authorization`.
enum Target<'a> {
    Name(&'a str),
    Authorization,
}

fn target_of(name: &str) -> Option<Target<'_>> {
    if is_authorization(name) {
        Some(Target::Authorization)
    } else if is_credential_name(name) {
        Some(Target::Name(name))
    } else {
        None
    }
}

fn literal_credential(target: &Target<'_>, value: &Expr) -> bool {
    let Some(text) = string_literal(value) else {
        return false;
    };
    match target {
        Target::Authorization => is_authorization_value(&text),
        Target::Name(name) => is_credential_value(&text) && !text.eq_ignore_ascii_case(name),
    }
}

fn message(what: &str) -> String {
    format!("Hardcoded credential in {what}; load it from a secret provider or a protected setting")
}

impl ScriptRule for HardcodedCredential {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        let mut found: Vec<(TextRange, String)> = Vec::new();
        let mut push = |range: TextRange, what: String| {
            if !found.iter().any(|(r, _)| *r == range) {
                found.push((range, message(&what)));
            }
        };
        walk_stmts(&unit.parsed.module.body, &mut |s| {
            let Stmt::Assign(assign) = s else { return };
            for target in &assign.targets {
                let name: Cow<'_, str> = match target {
                    Expr::Name(n) => Cow::Borrowed(&*n.id),
                    Expr::Attribute(a) => Cow::Borrowed(&*a.attr.id),
                    Expr::Subscript(sub) => match &*sub.slice {
                        Slice::Index(i) => match string_literal(&i.value) {
                            Some(key) => key,
                            None => continue,
                        },
                        _ => continue,
                    },
                    _ => continue,
                };
                let name = &*name;
                if let Some(t) = target_of(name)
                    && literal_credential(&t, &assign.value)
                {
                    push(assign.value.range(), format!("`{name}`"));
                }
            }
        });
        walk_exprs(&unit.parsed.module.body, &mut |e| match e {
            Expr::Dict(d) => {
                for (k, v) in d.keys.iter().zip(&d.values) {
                    let Some(key) = string_literal(k) else {
                        continue;
                    };
                    if let Some(t) = target_of(&key)
                        && literal_credential(&t, v)
                    {
                        push(v.range(), format!("`{key}`"));
                    }
                }
            }
            Expr::Str(_) => {
                let Some(text) = string_literal(e) else {
                    return;
                };
                if url_password(&text).is_some() {
                    push(e.range(), "a URL".to_owned());
                }
            }
            Expr::Call(call) => {
                for k in &call.keywords {
                    let name = &*k.arg.id;
                    if let Some(t) = target_of(name)
                        && literal_credential(&t, &k.value)
                    {
                        push(k.value.range(), format!("`{name}`"));
                    }
                }
                let callee = model.callee(call);
                if let Some(callee) = callee.as_deref() {
                    if callee.starts_with("system.") {
                        let names = system_positional_names(callee, call);
                        for (arg, name) in call.args.iter().zip(names) {
                            if let Some(name) = name
                                && is_credential_name(name)
                                && literal_credential(&Target::Name(name), arg)
                            {
                                push(arg.range(), format!("`{name}` of `{callee}`"));
                            }
                        }
                    }
                    if callee == "java.sql.DriverManager.getConnection"
                        && call.args.len() == 3
                        && literal_credential(&Target::Name("password"), &call.args[2])
                    {
                        push(call.args[2].range(), format!("the password of `{callee}`"));
                    }
                }
                if let Expr::Attribute(a) = &*call.func
                    && SETTERS.contains(&&*a.attr.id)
                    && call.args.len() == 2
                    && let Some(name) = string_literal(&call.args[0])
                    && let Some(t) = target_of(&name)
                    && literal_credential(&t, &call.args[1])
                {
                    push(call.args[1].range(), format!("`{name}`"));
                }
            }
            _ => {}
        });
        found.sort_by_key(|(r, _)| r.start());
        for (range, message) in found {
            report(cx, unit, range, message);
        }
    }
}

/// The text of a binding value that is a literal: strips the quotes of an
/// expression string literal (`"'Bearer abc'"`). `None` when it holds
/// property references or expression code.
fn binding_literal(value: &Value<'_>) -> Option<String> {
    let s = value.as_str()?.trim();
    if s.contains('{') {
        return None;
    }
    for q in ['\'', '"'] {
        if let Some(inner) = s.strip_prefix(q).and_then(|r| r.strip_suffix(q)) {
            if inner.contains(q) {
                return None;
            }
            return Some(inner.to_owned());
        }
    }
    Some(s.to_owned())
}

impl BindingRule for HardcodedCredential {
    fn check_binding(&self, cx: &mut Checker<'_, '_>, binding: &Binding<'_>) {
        if binding.kind != BindingKind::Http {
            return;
        }
        let Some(request) = binding.config.and_then(|c| c.get("request")) else {
            return;
        };
        let mut found: Vec<(TextRange, String)> = Vec::new();
        if let Some(auth) = request.get("auth") {
            let kind = auth.get("type").and_then(Value::as_str).unwrap_or("");
            if !matches!(kind.trim().to_ascii_lowercase().as_str(), "" | "none")
                && let Some(value) = auth.get("value")
            {
                if let Some(object) = value.as_object() {
                    for member in object.iter() {
                        if is_credential_name(member.key())
                            && binding_literal(&member.value)
                                .is_some_and(|v| is_credential_value(&v))
                        {
                            found.push((
                                member.value.range,
                                format!("the HTTP binding's `auth.value.{}`", member.key()),
                            ));
                        }
                    }
                } else if let Some(text) = binding_literal(value) {
                    // Basic authentication is `user:password`.
                    let secret = match text.split_once(':') {
                        Some((_, password)) => password.to_owned(),
                        None => text.clone(),
                    };
                    if is_credential_value(&secret) {
                        found.push((value.range, "the HTTP binding's `auth`".to_owned()));
                    }
                }
            }
        }
        if let Some(headers) = request.get("header").and_then(Value::as_array) {
            for header in headers {
                let (Some(key), Some(value)) = (
                    header.get("key").and_then(Value::as_str),
                    header.get("value"),
                ) else {
                    continue;
                };
                let Some(text) = binding_literal(value) else {
                    continue;
                };
                let credential = if is_authorization(key) {
                    is_authorization_value(&text)
                } else {
                    is_credential_name(key) && is_credential_value(&text)
                };
                if credential {
                    found.push((value.range, format!("the HTTP binding's `{key}` header")));
                }
            }
        }
        if let Some(url) = request.get("url")
            && url.as_str().is_some_and(|u| url_password(u).is_some())
        {
            found.push((url.range, "the HTTP binding's URL".to_owned()));
        }
        found.sort_by_key(|(r, _)| r.start());
        for (range, what) in found {
            cx.report(range, message(&what))
                .node_path(binding.node_path.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{is_credential_name, is_credential_value, words};
    use crate::rules::script::testing::*;
    use crate::rules::test_support::Fixture;

    fn flagged_in(code: &str) -> Vec<String> {
        flagged(&lint_module("SEC002", code))
    }

    #[test]
    fn splits_words() {
        assert_eq!(words("apiKey"), ["api", "key"]);
        assert_eq!(words("API_KEY"), ["api", "key"]);
        assert_eq!(words("X-Api-Key"), ["x", "api", "key"]);
        assert_eq!(words("HTTPPassword"), ["http", "password"]);
        assert_eq!(words("db2Password"), ["db2", "password"]);
    }

    #[test]
    fn credential_names() {
        for n in [
            "password",
            "DB_PASSWORD",
            "userPwd",
            "apiKey",
            "api_key",
            "X-API-Key",
            "clientSecret",
            "access_token",
            "secret",
        ] {
            assert!(is_credential_name(n), "{n}");
        }
        for n in [
            "passwordField",
            "PASSWORD_KEY",
            "tokenLength",
            "key",
            "user",
            "bypass",
            "passwordMode",
            "tokens_seen",
            "secretName",
        ] {
            assert!(!is_credential_name(n), "{n}");
        }
    }

    #[test]
    fn credential_values() {
        for v in ["S3cr3t!x", "hunter22", "abcd1234efgh"] {
            assert!(is_credential_value(v), "{v}");
        }
        for v in [
            "",
            "   ",
            "abc",
            "password",
            "Password",
            "api_key",
            "session.custom.token",
            "<password>",
            "{token}",
            "${SECRET}",
            "%s",
            "********",
            "changeme",
            "your-token-here",
            "[default]Config/Password",
            "Wrong password",
            "None",
            "xxxxxxx",
        ] {
            assert!(!is_credential_value(v), "{v:?}");
        }
    }

    #[test]
    fn reports_literal_credentials_in_scripts() {
        let code = r#"DB_PASSWORD = "S3cr3t!x"
class C(object):
    def __init__(self):
        self.apiToken = 'tok_8f2a9c'
def f(url, body, cfg):
    cfg["password"] = "hunter22"
    headers = {"Authorization": "Bearer abcdef123456", "X-Api-Key": "k3y-9f8e7d"}
    system.net.httpPost(url, "application/json", body, username="svc", password="pa55word!")
    system.net.httpGet(url, 5000, 60000, "svc", "pa55word?")
    db = system.db
    from java.sql import DriverManager
    DriverManager.getConnection("jdbc:x", "sa", "Sa_P4ss")
    props.setProperty("password", "pr0p-S3cr!")
    conn.setRequestProperty("Authorization", "Basic dXNlcjpwYXNz")
    open_url("ftp://user:Pa55w0rd@host/file")
"#;
        assert_eq!(
            flagged_in(code),
            [
                r#""S3cr3t!x""#,
                "'tok_8f2a9c'",
                r#""hunter22""#,
                r#""Bearer abcdef123456""#,
                r#""k3y-9f8e7d""#,
                r#""pa55word!""#,
                r#""pa55word?""#,
                r#""Sa_P4ss""#,
                r#""pr0p-S3cr!""#,
                r#""Basic dXNlcjpwYXNz""#,
                r#""ftp://user:Pa55w0rd@host/file""#,
            ]
        );
    }

    #[test]
    fn non_credentials_are_fine() {
        let code = r#"PASSWORD_KEY = 'password'
password = ''
token = None
TOKEN_SEPARATOR = ','
passwordField = "S3cr3t!x"
api_key = "<your api key>"
secret = "{secret}"
pwd = "C:/temp"
def f(user, password, url, token):
    system.security.validateUser(user, password)
    system.net.httpGet(url, username=user, password=password)
    headers = {"Authorization": "Bearer " + token, "Accept": "application/json"}
    msg = {"password": "Wrong password"}
    self.password = self.view.params.password
    params = {"token": "access_token", "pwd": "pwd"}
    system.tag.writeBlocking(["[default]Config/Password"], [password])
    x = "http://host:8088/data"
    y = "http://user@host/data"
"#;
        let found = flagged_in(code);
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn everywhere_and_json_ranges() {
        assert_eq!(
            flagged_everywhere(
                "SEC002",
                "\theaders = {\"Authorization\": \"Bearer \\\"abcdef123456\"}\n\tpassword = \"S3cr3t!x\""
            ),
            ["\"Bearer \\\"abcdef123456\"", "\"S3cr3t!x\""]
        );
        let f = gateway_message("def handleMessage(payload):\n\ttoken = 'tok_8f2a9c'\n");
        assert_eq!(flagged(&f.lint(&["SEC002"])), ["'tok_8f2a9c'"]);
    }

    #[test]
    fn shadowed_system_is_not_the_api() {
        let code =
            "def f(system, url):\n    system.net.httpGet(url, 5000, 60000, 'svc', 'pa55word?')\n";
        assert!(flagged_in(code).is_empty());
    }

    fn http_view(request: &str) -> Fixture {
        Fixture::view(&format!(
            r#"{{"custom": {{"x": 1}}, "propConfig": {{"custom.x": {{"binding": {{"type": "http", "config": {{"request": {request}}}}}}}}},
               "root": {{"type": "ia.container.flex", "meta": {{"name": "root"}}}}}}"#
        ))
    }

    #[test]
    fn http_bindings() {
        let f = http_view(
            r#"{"url": "https://api.example.invalid/x", "method": "GET",
                "auth": {"type": "basic", "value": "svc:pa55word!"},
                "header": [{"key": "Authorization", "value": "Bearer abcdef123456"},
                           {"key": "Accept", "value": "application/json"},
                           {"key": "X-Api-Key", "value": "{view.params.key}"}]}"#,
        );
        assert_eq!(
            flagged(&f.lint(&["SEC002"])),
            ["\"svc:pa55word!\"", "\"Bearer abcdef123456\""]
        );
        let f = http_view(
            r#"{"url": "https://svc:pa55word!@api.example.invalid/x",
                "auth": {"type": "bearer", "value": {"token": "abcdef123456"}}}"#,
        );
        assert_eq!(
            flagged(&f.lint(&["SEC002"])),
            [
                "\"https://svc:pa55word!@api.example.invalid/x\"",
                "\"abcdef123456\""
            ]
        );
        let f = http_view(
            r#"{"url": "https://api.example.invalid/x",
                "auth": {"type": "none", "value": "svc:pa55word!"},
                "header": [{"key": "Authorization", "value": "'Bearer ' + {session.custom.token}"}]}"#,
        );
        assert!(f.lint(&["SEC002"]).is_empty());
    }
}
