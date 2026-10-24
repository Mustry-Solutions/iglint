//! Named queries (format spec 8).

use iglint_json::{JsonString, Value};
use iglint_text::{TextRange, TextSize};

use crate::ids::FileId;

/// `attributes.parameters[].type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NamedQueryParamKind {
    /// Bound with `:name` in the SQL.
    Parameter,
    /// Textual substitution with `{name}` (SQL injection risk).
    QueryString,
    /// Selects the database connection.
    Database,
    /// Anything else.
    Other,
}

impl NamedQueryParamKind {
    /// Parses `attributes.parameters[].type`.
    pub fn parse(s: &str) -> NamedQueryParamKind {
        match s {
            "Parameter" => NamedQueryParamKind::Parameter,
            "QueryString" => NamedQueryParamKind::QueryString,
            "Database" => NamedQueryParamKind::Database,
            _ => NamedQueryParamKind::Other,
        }
    }

    /// The type as written.
    pub fn name(self) -> &'static str {
        match self {
            NamedQueryParamKind::Parameter => "Parameter",
            NamedQueryParamKind::QueryString => "QueryString",
            NamedQueryParamKind::Database => "Database",
            NamedQueryParamKind::Other => "other",
        }
    }
}

/// One declared parameter.
#[derive(Debug)]
pub struct NamedQueryParam<'a> {
    pub value: &'a Value<'a>,
    pub identifier: Option<&'a JsonString<'a>>,
    pub kind: NamedQueryParamKind,
    pub sql_type: Option<i64>,
}

/// A named query: settings from `resource.json` attributes and the SQL text.
#[derive(Debug)]
pub struct NamedQuery<'a> {
    pub attributes: Option<&'a Value<'a>>,
    /// `Query`, `ScalarQuery` or `UpdateQuery`.
    pub query_type: Option<&'a str>,
    /// Connection name; empty means the project default.
    pub database: Option<&'a str>,
    pub parameters: Vec<NamedQueryParam<'a>>,
    pub enabled: Option<bool>,
    pub cache_enabled: Option<bool>,
    pub use_max_return_size: Option<bool>,
    pub documentation: Option<&'a str>,
    pub sql_file: Option<FileId>,
    pub sql: Option<&'a str>,
}

impl<'a> NamedQuery<'a> {
    /// Parameter placeholders used in the SQL.
    pub fn placeholders(&self) -> Vec<SqlPlaceholder<'a>> {
        self.sql.map(sql_placeholders).unwrap_or_default()
    }

    /// A declared parameter by identifier.
    pub fn parameter(&self, name: &str) -> Option<&NamedQueryParam<'a>> {
        self.parameters
            .iter()
            .find(|p| p.identifier.is_some_and(|i| i.value() == name))
    }
}

pub(crate) fn build_named_query<'a>(
    attributes: Option<&'a Value<'a>>,
    documentation: Option<&'a str>,
    sql_file: Option<FileId>,
    sql: Option<&'a str>,
) -> NamedQuery<'a> {
    let attr = |k: &str| attributes.and_then(|a| a.get(k));
    let parameters = attr("parameters")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|p| NamedQueryParam {
                    value: p,
                    identifier: p.get("identifier").and_then(Value::as_json_string),
                    kind: p
                        .get("type")
                        .and_then(Value::as_str)
                        .map_or(NamedQueryParamKind::Other, NamedQueryParamKind::parse),
                    sql_type: p.get("sqlType").and_then(Value::as_i64),
                })
                .collect()
        })
        .unwrap_or_default();
    NamedQuery {
        attributes,
        query_type: attr("type").and_then(Value::as_str),
        database: attr("database").and_then(Value::as_str),
        parameters,
        enabled: attr("enabled").and_then(Value::as_bool),
        cache_enabled: attr("cacheEnabled").and_then(Value::as_bool),
        use_max_return_size: attr("useMaxReturnSize").and_then(Value::as_bool),
        documentation,
        sql_file,
        sql,
    }
}

/// How a parameter is used in SQL.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlaceholderKind {
    /// `:name`, outside string literals and comments.
    Colon,
    /// `{name}`, anywhere outside comments (textual substitution).
    Brace,
}

/// A parameter placeholder in named query SQL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SqlPlaceholder<'a> {
    pub name: &'a str,
    pub kind: PlaceholderKind,
    /// Range of the whole placeholder (`:name` or `{name}`) in the SQL file.
    pub range: TextRange,
}

/// Finds `:name` and `{name}` placeholders. `--` and `/* */` comments are
/// skipped (Ignition removes `--` comments before parsing parameters);
/// `:name` inside single-quoted strings and `::` casts are not parameters.
pub fn sql_placeholders(sql: &str) -> Vec<SqlPlaceholder<'_>> {
    let b = sql.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let is_start = |c: u8| c.is_ascii_alphabetic() || c == b'_';
    let is_part = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut in_string = false;
    while i < b.len() {
        let c = b[i];
        if !in_string && c == b'-' && b.get(i + 1) == Some(&b'-') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if !in_string && c == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                i += 1;
            }
            i = (i + 2).min(b.len());
            continue;
        }
        if c == b'\'' {
            in_string = !in_string;
            i += 1;
            continue;
        }
        if c == b'{' {
            // `i` is at the ASCII `{`, so `i + 1` is a char boundary, and
            // `close` comes from `find` on that suffix.
            #[allow(clippy::string_slice)]
            if let Some(close) = sql[i + 1..].find('}') {
                // Both ends are boundaries: see above.
                #[allow(clippy::string_slice)]
                let name = &sql[i + 1..i + 1 + close];
                if !name.is_empty() && name.bytes().all(|c| is_part(c) || c == b' ' || c == b'-') {
                    out.push(SqlPlaceholder {
                        name,
                        kind: PlaceholderKind::Brace,
                        range: TextRange::new(
                            TextSize::new(i as u32),
                            TextSize::new((i + close + 2) as u32),
                        ),
                    });
                    i += close + 2;
                    continue;
                }
            }
            i += 1;
            continue;
        }
        if !in_string && c == b':' {
            if b.get(i + 1) == Some(&b':') {
                i += 2;
                continue;
            }
            let prev_colon = i > 0 && b[i - 1] == b':';
            if !prev_colon && b.get(i + 1).copied().is_some_and(is_start) {
                let mut j = i + 1;
                while j < b.len() && is_part(b[j]) {
                    j += 1;
                }
                // `i` is the ASCII `:` and `j` stops at the first byte that
                // is not ASCII alphanumeric or `_`, so both are boundaries.
                #[allow(clippy::string_slice)]
                let name = &sql[i + 1..j];
                out.push(SqlPlaceholder {
                    name,
                    kind: PlaceholderKind::Colon,
                    range: TextRange::new(TextSize::new(i as u32), TextSize::new(j as u32)),
                });
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders() {
        let sql = "UPDATE {table} SET a = :a, b = 'x:y' -- :c\nWHERE id = :id::int /* :d */";
        let p = sql_placeholders(sql);
        let names: Vec<_> = p.iter().map(|p| (p.name, p.kind)).collect();
        assert_eq!(
            names,
            vec![
                ("table", PlaceholderKind::Brace),
                ("a", PlaceholderKind::Colon),
                ("id", PlaceholderKind::Colon)
            ]
        );
        assert_eq!(p[0].range.slice(sql), "{table}");
        assert_eq!(p[2].range.slice(sql), ":id");
    }
}
