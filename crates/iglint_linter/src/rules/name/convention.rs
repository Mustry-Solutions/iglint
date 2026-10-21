//! Naming conventions: patterns, abbreviation handling and suggestions.

use regex::Regex;

/// A known naming convention.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Convention {
    Pascal,
    Camel,
    Snake,
    Kebab,
    Screaming,
    Title,
    Lower,
}

impl Convention {
    pub const ALL: [Convention; 7] = [
        Convention::Pascal,
        Convention::Camel,
        Convention::Snake,
        Convention::Kebab,
        Convention::Screaming,
        Convention::Title,
        Convention::Lower,
    ];

    /// The name used in configuration (ign-lint's spelling).
    pub fn name(self) -> &'static str {
        match self {
            Convention::Pascal => "PascalCase",
            Convention::Camel => "camelCase",
            Convention::Snake => "snake_case",
            Convention::Kebab => "kebab-case",
            Convention::Screaming => "SCREAMING_SNAKE_CASE",
            Convention::Title => "Title Case",
            Convention::Lower => "lower case",
        }
    }

    pub fn parse(s: &str) -> Option<Convention> {
        Convention::ALL.into_iter().find(|c| c.name() == s)
    }

    /// The regex as ign-lint writes it.
    pub fn pattern(self) -> &'static str {
        match self {
            Convention::Pascal => "^[A-Z][a-zA-Z0-9]*$",
            Convention::Camel => "^[a-z][a-zA-Z0-9]*$",
            Convention::Snake => "^[a-z][a-z0-9_]*$",
            Convention::Kebab => "^[a-z][a-z0-9-]*$",
            Convention::Screaming => "^[A-Z][A-Z0-9_]*$",
            Convention::Title => r"^[A-Z][a-z]*(\s[A-Z][a-z]*)*$",
            Convention::Lower => r"^[a-z][a-z\s]*$",
        }
    }

    /// The description used in messages.
    pub fn description(self) -> &'static str {
        match self {
            Convention::Lower => "lower case with spaces (e.g., my button, data table)",
            other => other.name(),
        }
    }

    /// The convention's regex, without digits when `allow_numbers` is false
    /// (ign-lint deletes `0-9` from the pattern).
    pub fn regex_source(self, allow_numbers: bool) -> String {
        if allow_numbers {
            self.pattern().to_owned()
        } else {
            self.pattern().replace("0-9", "")
        }
    }
}

/// Compiles a pattern with Python `re.match` semantics: anchored at the
/// start only.
pub fn compile(pattern: &str) -> Result<Regex, String> {
    Regex::new(&format!(r"\A(?:{pattern})"))
        .map_err(|e| format!("invalid pattern `{pattern}`: {e}"))
}

/// Abbreviations ign-lint knows when `auto_detect_abbreviations` is on.
pub const BUILTIN_ABBREVIATIONS: &[&str] = &[
    "API", "HTTP", "HTTPS", "XML", "JSON", "SQL", "URL", "URI", "UUID", "CPU", "GPU", "RAM", "SSD",
    "HDD", "PDF", "CSV", "ZIP", "GIF", "PNG", "JPG", "JPEG", "SVG", "CSS", "HTML", "JS", "TS",
    "PHP", "ASP", "JSP", "CGI", "FTP", "SSH", "TCP", "UDP", "IP", "DNS", "DHCP", "VPN", "SSL",
    "TLS", "JWT", "CRUD", "REST", "SOAP", "AJAX", "DOM", "UI", "UX", "GUI", "CLI", "OS", "iOS",
    "macOS", "AWS", "GCP", "IBM", "AI", "ML", "NLP", "OCR", "QR", "RFID", "NFC", "GPS", "LED",
    "LCD", "OLED", "CRT", "ID",
];

/// Whether `word` is one of `abbreviations` (compared in upper case).
pub fn is_abbreviation(word: &str, abbreviations: &[String]) -> bool {
    let upper = word.to_uppercase();
    abbreviations.iter().any(|a| a.to_uppercase() == upper)
}

/// The name with every *whole-word* upper-case abbreviation written the way
/// `convention` writes ordinary words, so that `read_API_data` counts as
/// snake_case and `API Key` as Title Case. Unlike ign-lint, which replaces
/// abbreviation substrings anywhere (so `idleTimer` fails camelCase because
/// of `ID`), this only ever makes a name more acceptable.
pub fn normalize_abbreviations(
    name: &str,
    convention: Convention,
    abbreviations: &[String],
) -> Option<String> {
    if abbreviations.is_empty() {
        return None;
    }
    let is_abbr = |w: &str| {
        !w.is_empty()
            && abbreviations
                .iter()
                .any(|a| w == a || (w == a.to_uppercase() && w.chars().any(char::is_uppercase)))
    };
    let rewrite = |separators: &[char], f: &dyn Fn(&str) -> String| -> Option<String> {
        let mut out = String::with_capacity(name.len());
        let mut word = String::new();
        let mut changed = false;
        let flush = |word: &mut String, out: &mut String, changed: &mut bool| {
            if is_abbr(word) {
                let replaced = f(word);
                *changed |= replaced != *word;
                out.push_str(&replaced);
            } else {
                out.push_str(word);
            }
            word.clear();
        };
        for c in name.chars() {
            if separators.contains(&c) || (separators.is_empty() && c.is_whitespace()) {
                flush(&mut word, &mut out, &mut changed);
                out.push(c);
            } else {
                word.push(c);
            }
        }
        flush(&mut word, &mut out, &mut changed);
        changed.then_some(out)
    };
    match convention {
        Convention::Snake | Convention::Kebab => rewrite(&['_', '-'], &|w| w.to_lowercase()),
        Convention::Lower => rewrite(&[], &|w| w.to_lowercase()),
        Convention::Title => rewrite(&[], &|w| capitalize(w)),
        // Upper-case abbreviations already match these patterns.
        Convention::Pascal | Convention::Camel | Convention::Screaming => None,
    }
}

/// First character upper case, the rest lower case (Python `capitalize`).
pub fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first
            .to_uppercase()
            .chain(chars.flat_map(char::to_lowercase))
            .collect(),
        None => String::new(),
    }
}

/// Splits a name into words: at `-`, `_`, whitespace and other punctuation,
/// at lower-to-upper transitions (`myButton`), before the last capital of a
/// capital run followed by a lower-case letter (`HTTPServer` is `HTTP`,
/// `Server`) and between letters and digits (`Pump12` is `Pump`, `12`).
pub fn words(name: &str) -> Vec<String> {
    #[derive(Clone, Copy, PartialEq)]
    enum K {
        Upper,
        Lower,
        Digit,
        Other,
    }
    let kind = |c: char| {
        if c.is_uppercase() {
            K::Upper
        } else if c.is_lowercase() || (c.is_alphabetic() && !c.is_uppercase()) {
            K::Lower
        } else if c.is_ascii_digit() {
            K::Digit
        } else {
            K::Other
        }
    };
    let chars: Vec<char> = name.chars().collect();
    let mut out = Vec::new();
    let mut current = String::new();
    for i in 0..chars.len() {
        let c = chars[i];
        let k = kind(c);
        if k == K::Other {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            continue;
        }
        if let Some(&prev) = current.chars().last().as_ref() {
            let pk = kind(prev);
            let split = match (pk, k) {
                (K::Lower, K::Upper) => true,
                (K::Digit, K::Upper | K::Lower) | (K::Upper | K::Lower, K::Digit) => true,
                (K::Upper, K::Upper) => chars.get(i + 1).is_some_and(|n| kind(*n) == K::Lower),
                _ => false,
            };
            if split {
                out.push(std::mem::take(&mut current));
            }
        }
        current.push(c);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Splits an all-capitals word that is a run of known abbreviations
/// (`XMLHTTP` is `XML`, `HTTP`); other words are returned as they are.
fn split_abbreviation_run(word: &str, abbreviations: &[String]) -> Vec<String> {
    fn go(rest: &str, abbreviations: &[String], out: &mut Vec<String>) -> bool {
        if rest.is_empty() {
            return true;
        }
        let mut candidates: Vec<&String> = abbreviations
            .iter()
            .filter(|a| rest.starts_with(a.to_uppercase().as_str()))
            .collect();
        candidates.sort_by_key(|a| std::cmp::Reverse(a.len()));
        for a in candidates {
            out.push(a.to_uppercase());
            if go(&rest[a.len()..], abbreviations, out) {
                return true;
            }
            out.pop();
        }
        false
    }
    if word.len() > 1
        && word.chars().all(|c| c.is_ascii_uppercase())
        && !is_abbreviation(word, abbreviations)
    {
        let mut out = Vec::new();
        if go(word, abbreviations, &mut out) && out.len() > 1 {
            return out;
        }
    }
    vec![word.to_owned()]
}

/// A suggestion for `name` in `convention`. Digit-only words are dropped
/// when numbers are not allowed.
pub fn suggest(
    name: &str,
    convention: Convention,
    abbreviations: &[String],
    allow_numbers: bool,
) -> String {
    let mut parts: Vec<String> = words(name)
        .iter()
        .flat_map(|w| split_abbreviation_run(w, abbreviations))
        .collect();
    if !allow_numbers {
        parts.retain(|w| !w.chars().all(|c| c.is_ascii_digit()));
    }
    let pascal_word = |w: &str| {
        if is_abbreviation(w, abbreviations) {
            w.to_uppercase()
        } else {
            capitalize(w)
        }
    };
    match convention {
        Convention::Pascal => parts.iter().map(|w| pascal_word(w)).collect(),
        Convention::Camel => parts
            .iter()
            .enumerate()
            .map(|(i, w)| {
                if i == 0 {
                    w.to_lowercase()
                } else {
                    pascal_word(w)
                }
            })
            .collect(),
        Convention::Snake => lower_join(&parts, "_"),
        Convention::Kebab => lower_join(&parts, "-"),
        Convention::Screaming => lower_join(&parts, "_").to_uppercase(),
        Convention::Title => parts
            .iter()
            .filter(|w| !w.chars().all(|c| c.is_ascii_digit()))
            .map(|w| pascal_word(w))
            .collect::<Vec<_>>()
            .join(" "),
        Convention::Lower => parts
            .iter()
            .filter(|w| !w.chars().all(|c| c.is_ascii_digit()))
            .map(|w| w.to_lowercase())
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn lower_join(parts: &[String], sep: &str) -> String {
    parts
        .iter()
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join(sep)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn abbreviations() -> Vec<String> {
        BUILTIN_ABBREVIATIONS
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    }

    #[test]
    fn splits_words() {
        assert_eq!(words("HTTPServer"), ["HTTP", "Server"]);
        assert_eq!(words("myButton1"), ["my", "Button", "1"]);
        assert_eq!(
            words("badEmbeddedPotato_0"),
            ["bad", "Embedded", "Potato", "0"]
        );
        assert_eq!(words("DAT_02_TT_123_A"), ["DAT", "02", "TT", "123", "A"]);
        assert_eq!(words("Zone Dropdown"), ["Zone", "Dropdown"]);
        assert_eq!(words("eq_path_url"), ["eq", "path", "url"]);
    }

    #[test]
    fn suggestions() {
        let a = abbreviations();
        assert_eq!(
            suggest("eq_path_url", Convention::Camel, &a, true),
            "eqPathURL"
        );
        assert_eq!(
            suggest("HTTPServer", Convention::Snake, &a, true),
            "http_server"
        );
        assert_eq!(
            suggest("DAT_02_TT_123_A", Convention::Pascal, &a, true),
            "Dat02Tt123A"
        );
        assert_eq!(
            suggest("badEmbeddedPotato_0", Convention::Pascal, &a, true),
            "BadEmbeddedPotato0"
        );
        assert_eq!(suggest("Pump12", Convention::Pascal, &a, false), "Pump");
        assert_eq!(
            suggest("URL_Params", Convention::Camel, &a, true),
            "urlParams"
        );
        assert_eq!(
            suggest("XMLHTTPRequest", Convention::Camel, &a, true),
            "xmlHTTPRequest"
        );
        assert_eq!(
            suggest("iconCamelCase", Convention::Lower, &a, true),
            "icon camel case"
        );
        assert_eq!(
            suggest("ActionButton", Convention::Screaming, &a, true),
            "ACTION_BUTTON"
        );
        assert_eq!(
            suggest("icon-kebab-case", Convention::Title, &a, true),
            "Icon Kebab Case"
        );
    }

    #[test]
    fn abbreviations_only_widen() {
        let a = abbreviations();
        assert_eq!(
            normalize_abbreviations("read_API_data", Convention::Snake, &a).as_deref(),
            Some("read_api_data")
        );
        assert_eq!(
            normalize_abbreviations("API Key", Convention::Title, &a).as_deref(),
            Some("Api Key")
        );
        // Substrings are not abbreviations.
        assert_eq!(
            normalize_abbreviations("idle_timer", Convention::Snake, &a),
            None
        );
        assert_eq!(
            normalize_abbreviations("idleTimer", Convention::Camel, &a),
            None
        );
    }
}
