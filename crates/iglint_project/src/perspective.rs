//! Perspective project resources other than views: style classes, page
//! configuration (format spec 7.1, 7.2). Session properties share the view
//! property model ([`crate::view::SessionProps`]).

use iglint_json::{JsonString, Value, ValueKind};

use crate::ids::FileId;
use crate::view::ModelProblem;

/// A style class (`style-classes/<path>/style.json`).
#[derive(Debug)]
pub struct StyleClass<'a> {
    pub file: FileId,
    pub root: &'a Value<'a>,
    /// True for the legacy `data.bin` storage.
    pub legacy: bool,
    pub base: Option<&'a Value<'a>>,
    /// `base.style`.
    pub base_style: Option<&'a Value<'a>>,
    /// `base.animation`.
    pub animation: Option<&'a Value<'a>>,
    pub variants: Vec<StyleVariant<'a>>,
    pub problems: Vec<ModelProblem>,
}

/// One entry of `variants`.
#[derive(Debug)]
pub struct StyleVariant<'a> {
    pub value: &'a Value<'a>,
    pub pseudo: Option<&'a str>,
    pub media_query: Option<&'a str>,
    pub style: Option<&'a Value<'a>>,
    pub animation: Option<&'a Value<'a>>,
}

fn non_empty(v: Option<&Value<'_>>) -> bool {
    match v.map(|v| &v.kind) {
        Some(ValueKind::Object(o)) => !o.is_empty(),
        Some(ValueKind::Array(a)) => !a.is_empty(),
        Some(ValueKind::Null) | None => false,
        Some(_) => true,
    }
}

impl StyleClass<'_> {
    /// True when neither the base nor any variant defines a style or an
    /// animation.
    pub fn is_empty(&self) -> bool {
        !non_empty(self.base_style)
            && !non_empty(self.animation)
            && self
                .variants
                .iter()
                .all(|v| !non_empty(v.style) && !non_empty(v.animation))
    }
}

pub(crate) fn build_style_class<'a>(
    file: FileId,
    root: &'a Value<'a>,
    legacy: bool,
) -> StyleClass<'a> {
    let mut problems = Vec::new();
    if root.as_object().is_none() {
        problems.push(ModelProblem {
            range: root.range,
            message: "style class is not a JSON object".into(),
            node_path: String::new(),
        });
    }
    let base = root.get("base");
    let variants = match root.get("variants") {
        Some(v) => match v.as_array() {
            Some(list) => list
                .iter()
                .map(|v| StyleVariant {
                    value: v,
                    pseudo: v.get("pseudo").and_then(Value::as_str),
                    media_query: v.get("mediaQuery").and_then(Value::as_str),
                    style: v.get("style"),
                    animation: v.get("animation"),
                })
                .collect(),
            None => {
                if !v.is_null() {
                    problems.push(ModelProblem {
                        range: v.range,
                        message: "`variants` is not an array".into(),
                        node_path: String::new(),
                    });
                }
                Vec::new()
            }
        },
        None => Vec::new(),
    };
    StyleClass {
        file,
        root,
        legacy,
        base,
        base_style: base.and_then(|b| b.get("style")),
        animation: base.and_then(|b| b.get("animation")),
        variants,
        problems,
    }
}

/// `page-config/config.json`.
#[derive(Debug)]
pub struct PageConfig<'a> {
    pub file: FileId,
    pub root: &'a Value<'a>,
    pub pages: Vec<Page<'a>>,
    /// `sharedDocks`, all sides.
    pub shared_docks: Vec<Dock<'a>>,
    pub problems: Vec<ModelProblem>,
}

/// One page (`pages[url]`).
#[derive(Debug)]
pub struct Page<'a> {
    pub url: &'a JsonString<'a>,
    pub value: &'a Value<'a>,
    pub view_path: Option<&'a JsonString<'a>>,
    pub title: Option<&'a str>,
    pub docks: Vec<Dock<'a>>,
}

impl<'a> Page<'a> {
    /// `:name` segments of the URL, which become params of the page view.
    pub fn url_params(&self) -> Vec<&'a str> {
        let url: &'a str = self.url.value();
        url.split('/').filter_map(|s| s.strip_prefix(':')).collect()
    }
}

/// A dock of a page or of `sharedDocks`.
#[derive(Debug)]
pub struct Dock<'a> {
    /// `top`, `left`, `right` or `bottom`.
    pub side: &'a str,
    pub value: &'a Value<'a>,
    pub id: Option<&'a JsonString<'a>>,
    pub view_path: Option<&'a JsonString<'a>>,
    pub view_params: Option<&'a Value<'a>>,
    pub show: Option<&'a str>,
}

fn docks<'a>(value: Option<&'a Value<'a>>, out: &mut Vec<Dock<'a>>) {
    let Some(obj) = value.and_then(Value::as_object) else {
        return;
    };
    for m in obj.iter() {
        let Some(list) = m.value.as_array() else {
            continue;
        };
        let side: &'a JsonString<'a> = &m.key;
        for d in list.iter() {
            out.push(Dock {
                side: side.value(),
                value: d,
                id: d.get("id").and_then(Value::as_json_string),
                view_path: d
                    .get("viewPath")
                    .and_then(Value::as_json_string)
                    .filter(|s| !s.value().is_empty()),
                view_params: d.get("viewParams"),
                show: d.get("show").and_then(Value::as_str),
            });
        }
    }
}

pub(crate) fn build_page_config<'a>(file: FileId, root: &'a Value<'a>) -> PageConfig<'a> {
    let mut problems = Vec::new();
    let mut pages = Vec::new();
    if let Some(p) = root.get("pages") {
        match p.as_object() {
            Some(obj) => {
                for m in obj.iter() {
                    let mut page_docks = Vec::new();
                    docks(m.value.get("docks"), &mut page_docks);
                    pages.push(Page {
                        url: &m.key,
                        value: &m.value,
                        view_path: m
                            .value
                            .get("viewPath")
                            .and_then(Value::as_json_string)
                            .filter(|s| !s.value().is_empty()),
                        title: m.value.get("title").and_then(Value::as_str),
                        docks: page_docks,
                    });
                }
            }
            None => problems.push(ModelProblem {
                range: p.range,
                message: "`pages` is not an object".into(),
                node_path: String::new(),
            }),
        }
    }
    let mut shared_docks = Vec::new();
    docks(root.get("sharedDocks"), &mut shared_docks);
    PageConfig {
        file,
        root,
        pages,
        shared_docks,
        problems,
    }
}

/// Matches a URL against a page URL pattern with `:name` segments.
/// Returns the captured parameters, or `None` when it does not match.
pub fn match_page_url<'u>(pattern: &str, url: &'u str) -> Option<Vec<(String, &'u str)>> {
    let url = url.split(['?', '#']).next().unwrap_or(url);
    let norm = |s: &str| -> String {
        let t = s.trim_end_matches('/');
        if t.is_empty() {
            "/".to_owned()
        } else {
            t.to_owned()
        }
    };
    let pattern = norm(pattern);
    let url_norm = norm(url);
    let p: Vec<&str> = pattern.split('/').collect();
    let mut captures = Vec::new();
    let u_parts: Vec<&'u str> = url.trim_end_matches('/').split('/').collect();
    let u_norm: Vec<&str> = url_norm.split('/').collect();
    if p.len() != u_norm.len() {
        return None;
    }
    for (i, (ps, us)) in p.iter().zip(u_norm.iter()).enumerate() {
        if let Some(name) = ps.strip_prefix(':') {
            if us.is_empty() {
                return None;
            }
            captures.push((name.to_owned(), u_parts.get(i).copied().unwrap_or("")));
        } else if ps != us {
            return None;
        }
    }
    Some(captures)
}

/// Class names used in CSS selectors (`.name`), outside comments and
/// strings, with CSS escapes resolved (`.psc-Brand\\/Primary` gives
/// `psc-Brand/Primary`). Over-approximates (any `.name` outside a string
/// counts), which is the safe direction for "unknown class" checks.
pub fn css_classes(css: &str) -> Vec<String> {
    let b = css.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                    i += 1;
                }
                i += 2;
            }
            q @ (b'"' | b'\'') => {
                i += 1;
                while i < b.len() && b[i] != q {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i += 1;
            }
            b'.' => {
                let mut j = i + 1;
                let mut name = String::new();
                while j < b.len() {
                    let c = b[j];
                    if c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_') || c >= 0x80 {
                        let ch = css[j..].chars().next().unwrap_or(' ');
                        name.push(ch);
                        j += ch.len_utf8();
                    } else if c == b'\\' && j + 1 < b.len() && b[j + 1] != b'\n' {
                        let ch = css[j + 1..].chars().next().unwrap_or(' ');
                        name.push(ch);
                        j += 1 + ch.len_utf8();
                    } else {
                        break;
                    }
                }
                if name.chars().next().is_some_and(|c| !c.is_ascii_digit())
                    && !name.starts_with("--")
                {
                    out.push(name);
                }
                i = j.max(i + 1);
            }
            _ => i += 1,
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css() {
        let css = "/* .no */ .a-b, div.c_d:hover { width: .5em; background: url(\"x.png\"); }\n@media (x) { .e { } .psc-Brand\\/Primary {} }";
        assert_eq!(
            css_classes(css),
            vec!["a-b", "c_d", "e", "psc-Brand/Primary"]
        );
    }

    #[test]
    fn page_urls() {
        assert_eq!(match_page_url("/", "/"), Some(vec![]));
        assert_eq!(
            match_page_url("/motors/:line", "/motors/Line1"),
            Some(vec![("line".to_owned(), "Line1")])
        );
        assert_eq!(match_page_url("/motors/:line", "/motors"), None);
        assert_eq!(match_page_url("/a", "/a/"), Some(vec![]));
        assert_eq!(match_page_url("/a", "/a?x=1"), Some(vec![]));
        assert_eq!(match_page_url("/a", "/b"), None);
    }
}
