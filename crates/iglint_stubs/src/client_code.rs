//! What can run JavaScript in a Perspective browser client.
//!
//! JavaScript in a Perspective page can read the page's DOM and the
//! client's property store, write the properties the client may write,
//! open other pages of the session's origin and call the gateway with the
//! session's credentials. So a component that can run code the view does
//! not show (raw HTML, `javascript:` URLs, embedded documents) is code
//! iglint does not read. Measured on the client bundles of the Ignition
//! 8.3.3 image (`Perspective-module.modl`, `app-3.3.3.jar`,
//! `mounted/js/*.js`, React 18.2, which renders `javascript:` URLs as
//! written); see `docs/internal/fix-safety.md`, "Client-side code".

/// How a component type relates to client-side code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientCode {
    /// No route from the component's properties to code was found: the
    /// component reads named properties (`getPropsReducer`) and renders
    /// them as text, numbers, styles or paths of the project.
    None,
    /// The component can run code from its properties; the text says how.
    Always(&'static str),
    /// `ia.display.markdown`: raw HTML unless `props.markdown.escapeHtml`
    /// is `true` ([`markdown_is_plain`]).
    Markdown,
    /// `ia.navigation.link`: `props.url` is the anchor's `href`
    /// ([`url_runs_no_script`]).
    Link,
    /// `ia.shapes.svg`: it renders the element type and every attribute
    /// that `props.elements` names ([`svg_element_type_is_plain`],
    /// [`svg_attribute_is_plain`]).
    Svg,
}

/// Component types whose properties can run code in the browser, with how
/// (measured, see the module documentation). Sorted.
const ALWAYS: &[(&str, &str)] = &[
    (
        "ia.chart.chartrangeselector",
        "its client bundle writes innerHTML (not reviewed further)",
    ),
    (
        "ia.chart.gauge",
        "it configures amCharts from its properties, and amCharts renders HTML (not reviewed further)",
    ),
    (
        "ia.chart.pie",
        "it configures amCharts from its properties, and amCharts renders HTML (not reviewed further)",
    ),
    (
        "ia.chart.powerchart",
        "its client bundle writes innerHTML (not reviewed further)",
    ),
    (
        "ia.chart.simple-gauge",
        "it configures amCharts from its properties, and amCharts renders HTML (not reviewed further)",
    ),
    (
        "ia.chart.timeseries",
        "its client bundle writes innerHTML (not reviewed further)",
    ),
    (
        "ia.chart.xy",
        "it configures amCharts from its properties, and amCharts renders HTML (not reviewed further)",
    ),
    (
        "ia.display.google-map",
        "its map library renders popup and tooltip texts as HTML (not reviewed further)",
    ),
    (
        "ia.display.iframe",
        "it shows a document from src or srcDoc, which runs with the page's origin",
    ),
    (
        "ia.display.map",
        "its map library renders popup and tooltip texts as HTML (not reviewed further)",
    ),
    (
        "ia.display.pdf-viewer",
        "its PDF renderer can evaluate code (not reviewed further)",
    ),
    (
        "ia.navigation.horizontalmenu",
        "it opens item targets that are not project paths with window.open",
    ),
    (
        "ia.navigation.menutree",
        "it opens item targets that are not project paths with window.open",
    ),
    (
        "ia.navigation.navlinks",
        "it opens item targets that are not project paths (not reviewed further)",
    ),
    (
        "ia.reporting.report-viewer",
        "it shows reports, which iglint does not read (not reviewed further)",
    ),
];

/// How the component type `type_id` relates to client-side code. A type
/// that is not in the measured 8.3 component list (a third-party module's,
/// or a missing type) is [`ClientCode::Always`]: its client code is not
/// known.
pub fn component_client_code(type_id: Option<&str>) -> ClientCode {
    let Some(id) = type_id else {
        return ClientCode::Always("its type is missing");
    };
    if crate::component_type(id).is_none() {
        return ClientCode::Always(
            "it is not a component of Ignition 8.3, so its client code is not known",
        );
    }
    if let Ok(i) = ALWAYS.binary_search_by(|(t, _)| (*t).cmp(id)) {
        return ClientCode::Always(ALWAYS[i].1);
    }
    match id {
        "ia.display.markdown" => ClientCode::Markdown,
        "ia.navigation.link" => ClientCode::Link,
        "ia.shapes.svg" => ClientCode::Svg,
        _ => ClientCode::None,
    }
}

/// SVG element types that run no script and navigate nowhere: shapes,
/// containers, paint servers, text, clipping, markers and filter
/// primitives. Not `script`, `a`, `foreignObject` (HTML inside), `image`,
/// the animation elements (`set` and `animate` change attributes) or any
/// other type. Sorted.
const SVG_PLAIN_TYPES: &[&str] = &[
    "circle",
    "clipPath",
    "defs",
    "desc",
    "ellipse",
    "feBlend",
    "feColorMatrix",
    "feComponentTransfer",
    "feComposite",
    "feConvolveMatrix",
    "feDiffuseLighting",
    "feDisplacementMap",
    "feDistantLight",
    "feDropShadow",
    "feFlood",
    "feFuncA",
    "feFuncB",
    "feFuncG",
    "feFuncR",
    "feGaussianBlur",
    "feMerge",
    "feMergeNode",
    "feMorphology",
    "feOffset",
    "fePointLight",
    "feSpecularLighting",
    "feSpotLight",
    "feTile",
    "feTurbulence",
    "filter",
    "g",
    "group",
    "line",
    "linearGradient",
    "marker",
    "mask",
    "path",
    "pattern",
    "polygon",
    "polyline",
    "radialGradient",
    "rect",
    "stop",
    "symbol",
    "text",
    "textPath",
    "title",
    "tspan",
    "use",
];

/// Whether an element of `ia.shapes.svg`'s `elements` with type `t` runs no
/// script and navigates nowhere (the drawing passes the type to React as
/// the element to create).
pub fn svg_element_type_is_plain(t: &str) -> bool {
    SVG_PLAIN_TYPES.binary_search(&t).is_ok()
}

/// Whether the attribute `key` of an element of `ia.shapes.svg` with the
/// literal string value `value` (`None` when it is not a string) cannot
/// load or run code: the drawing passes every key it does not know to the
/// element. A link (`href`, `xlink:href`, `xlinkHref`, any spelling) must
/// be a literal reference into the drawing (`#id`); keys that start with
/// `on` (event handlers) count even though React drops them.
pub fn svg_attribute_is_plain(key: &str, value: Option<&str>) -> bool {
    let lower = key.to_ascii_lowercase();
    if lower.starts_with("on") {
        return false;
    }
    if lower.contains("href") {
        return value.is_some_and(|v| v.starts_with('#'));
    }
    true
}

/// The keys of `props.markdown` that may be present in a plain Markdown
/// component (the component passes every other key of `props.markdown`
/// to its renderer, `react-markdown`, so `allowDangerousHtml` or
/// `transformLinkUri` would turn its HTML and URL guards off).
pub const MARKDOWN_PLAIN_KEYS: &[&str] = &[
    "allowedTypes",
    "disallowedTypes",
    "escapeHtml",
    "renderers",
    "skipHtml",
    "sourcePos",
    "unwrapDisallowed",
];

/// A literal value of a key of `props.markdown`, as far as
/// [`markdown_is_plain`] needs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkdownValue {
    True,
    EmptyObject,
    Other,
}

/// Whether a literal `props.markdown` object, given as its members,
/// renders no HTML and no unchecked URL: its renderer gets
/// `allowDangerousHtml: !escapeHtml`, so `escapeHtml` must be present and
/// `true` (an absent key renders HTML), every key must be one of
/// [`MARKDOWN_PLAIN_KEYS`], and `renderers` (element types for Markdown
/// nodes: `{"image": "iframe"}` loads the image URL as a document) must be
/// empty. Every member counts, duplicate keys included.
pub fn markdown_is_plain<'k>(members: impl IntoIterator<Item = (&'k str, MarkdownValue)>) -> bool {
    let mut escaped = false;
    for (key, value) in members {
        let plain = match key {
            "escapeHtml" => value == MarkdownValue::True,
            "renderers" => value == MarkdownValue::EmptyObject,
            _ => MARKDOWN_PLAIN_KEYS.contains(&key),
        };
        if !plain {
            return false;
        }
        escaped |= key == "escapeHtml";
    }
    escaped
}

/// Whether the browser runs no code when it navigates to `url` (the
/// `href` of a link, `window.location.href`, `window.open`): a relative
/// URL, or one whose scheme is `http`, `https`, `mailto`, `tel` or `ftp`.
/// Parsed as browsers do: leading and trailing spaces and control
/// characters are ignored, tabs and line breaks anywhere are removed, and
/// the scheme is case-insensitive (`JaVa\tScRiPt:` is `javascript:`).
pub fn url_runs_no_script(url: &str) -> bool {
    let trimmed = url.trim_matches(|c: char| c <= ' ');
    let cleaned: String = trimmed
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    let Some((scheme, _)) = cleaned.split_once(':') else {
        return true;
    };
    let is_scheme = scheme
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !is_scheme {
        // `/a:b`, `?x=a:b`: a relative URL.
        return true;
    }
    matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http" | "https" | "mailto" | "tel" | "ftp"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_sorted_and_names_real_components() {
        assert!(ALWAYS.windows(2).all(|w| w[0].0 < w[1].0));
        for (id, _) in ALWAYS {
            assert!(crate::component_type(id).is_some(), "{id}");
        }
        assert!(crate::component_type("ia.display.markdown").is_some());
        assert!(crate::component_type("ia.navigation.link").is_some());
    }

    #[test]
    fn svg() {
        assert!(SVG_PLAIN_TYPES.windows(2).all(|w| w[0] < w[1]));
        assert!(svg_element_type_is_plain("path"));
        for t in [
            "script",
            "a",
            "foreignObject",
            "image",
            "set",
            "animate",
            "iframe",
        ] {
            assert!(!svg_element_type_is_plain(t), "{t}");
        }
        assert!(svg_attribute_is_plain("fill", None));
        assert!(svg_attribute_is_plain("href", Some("#g1")));
        assert!(!svg_attribute_is_plain("href", Some("javascript:x")));
        assert!(!svg_attribute_is_plain("xlinkHref", None));
        assert!(!svg_attribute_is_plain("onClick", Some("x")));
    }

    #[test]
    fn components() {
        assert_eq!(
            component_client_code(Some("ia.display.label")),
            ClientCode::None
        );
        assert!(matches!(
            component_client_code(Some("ia.display.iframe")),
            ClientCode::Always(_)
        ));
        assert!(matches!(
            component_client_code(Some("embr.periscope.embedding.javascript")),
            ClientCode::Always(_)
        ));
        assert!(matches!(component_client_code(None), ClientCode::Always(_)));
        assert_eq!(
            component_client_code(Some("ia.display.markdown")),
            ClientCode::Markdown
        );
    }

    #[test]
    fn urls() {
        for safe in [
            "",
            "/page",
            "page",
            "#x",
            "https://example.com/a:b",
            "HTTP://x",
            "mailto:a@b",
            "/a:b",
            "?q=a:b",
        ] {
            assert!(url_runs_no_script(safe), "{safe}");
        }
        for unsafe_ in [
            "javascript:alert(1)",
            " JavaScript:alert(1)",
            "java\tscript:alert(1)",
            "jav\nascript:x",
            "\u{1}javascript:x",
            "data:text/html,<script>x</script>",
            "vbscript:x",
            "blob:x",
        ] {
            assert!(!url_runs_no_script(unsafe_), "{unsafe_:?}");
        }
    }

    #[test]
    fn markdown() {
        use MarkdownValue::*;
        assert!(markdown_is_plain([
            ("escapeHtml", True),
            ("skipHtml", Other)
        ]));
        assert!(markdown_is_plain([
            ("escapeHtml", True),
            ("renderers", EmptyObject)
        ]));
        assert!(!markdown_is_plain([("skipHtml", Other)]));
        assert!(!markdown_is_plain([("escapeHtml", Other)]));
        assert!(!markdown_is_plain([
            ("escapeHtml", True),
            ("allowDangerousHtml", True)
        ]));
        assert!(!markdown_is_plain([
            ("escapeHtml", True),
            ("renderers", Other)
        ]));
        assert!(!markdown_is_plain([
            ("escapeHtml", True),
            ("escapeHtml", Other)
        ]));
    }
}
