//! Parsing and validating one configuration file.
//!
//! The TOML is parsed with spans and walked by hand, so every problem is
//! reported at its exact position with a "did you mean" hint.

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use iglint_linter::Severity;
use iglint_linter::registry::{Registry, RuleCode, Selector};
use iglint_linter::settings::OptionValue;
use iglint_linter::suggest::closest;
use iglint_text::{LineIndex, TextSize};
use toml::Spanned;
use toml::de::{DeTable, DeValue};

use crate::compat::{check_option_type, option_type_name};
use crate::{ConfigError, OUTPUT_FORMATS, absolutize};

pub(crate) const TOP_KEYS: &[&str] = &[
    "extend",
    "select",
    "extend-select",
    "ignore",
    "extend-ignore",
    "unfixable",
    "exclude",
    "extend-exclude",
    "strict",
    "projects",
    "gateways",
    "baseline",
    "respect-gitignore",
    "cache-dir",
    "output-format",
    "rules",
    "overrides",
];

pub(crate) const OVERRIDE_KEYS: &[&str] = &[
    "files",
    "select",
    "extend-select",
    "ignore",
    "extend-ignore",
    "severity",
    "strict",
    "rules",
];

pub(crate) const RULE_KEYS: &[&str] = &["severity", "enabled", "exclude"];

/// `select`/`ignore` of one configuration layer.
#[derive(Clone, Debug, Default)]
pub(crate) struct Layer {
    pub select: Option<Vec<Selector>>,
    pub extend_select: Vec<Selector>,
    pub ignore: Vec<Selector>,
    pub extend_ignore: Vec<Selector>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RawRule {
    pub severity: Option<Severity>,
    pub enabled: Option<bool>,
    pub exclude: Option<Vec<String>>,
    pub options: BTreeMap<String, OptionValue>,
}

impl RawRule {
    /// `self` with `other`'s settings taking precedence.
    pub fn merged(&self, other: &RawRule) -> RawRule {
        let mut options = self.options.clone();
        options.extend(other.options.clone());
        RawRule {
            severity: other.severity.or(self.severity),
            enabled: other.enabled.or(self.enabled),
            exclude: other.exclude.clone().or_else(|| self.exclude.clone()),
            options,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RawOverride {
    pub files: Vec<String>,
    pub select: Vec<Selector>,
    pub ignore: Vec<Selector>,
    pub severity: Vec<(Selector, Severity)>,
    pub rules: BTreeMap<RuleCode, RawRule>,
    pub strict: Option<bool>,
}

/// One configuration file, validated, with paths made absolute.
#[derive(Clone, Debug, Default)]
pub(crate) struct RawConfig {
    pub extend: Option<(PathBuf, Option<(u32, u32)>)>,
    pub layer: Layer,
    pub unfixable: Option<Vec<Selector>>,
    pub exclude: Option<Vec<String>>,
    pub extend_exclude: Vec<String>,
    pub strict: Option<bool>,
    pub projects: Option<Vec<PathBuf>>,
    pub gateways: Option<Vec<PathBuf>>,
    pub baseline: Option<PathBuf>,
    pub respect_gitignore: Option<bool>,
    pub cache_dir: Option<PathBuf>,
    pub output_format: Option<String>,
    pub rules: BTreeMap<RuleCode, RawRule>,
    pub overrides: Vec<RawOverride>,
}

struct Ctx<'a> {
    path: &'a Path,
    dir: PathBuf,
    index: LineIndex,
    text: &'a str,
    registry: &'a Registry,
}

type Entry<'i> = (
    &'i Spanned<std::borrow::Cow<'i, str>>,
    &'i Spanned<DeValue<'i>>,
);

impl Ctx<'_> {
    fn position(&self, span: &Range<usize>) -> (u32, u32) {
        let offset = TextSize::from(span.start.min(self.text.len()));
        let lc = self.index.line_column(offset, self.text);
        (lc.line, lc.column)
    }

    fn error(&self, span: &Range<usize>, message: String) -> ConfigError {
        ConfigError::new(self.path, Some(self.position(span)), message)
    }

    fn unknown_key(
        &self,
        key: &Spanned<std::borrow::Cow<'_, str>>,
        table: &str,
        known: &[&str],
    ) -> ConfigError {
        let name = key.get_ref();
        let mut message = if table.is_empty() {
            format!("unknown key `{name}`")
        } else {
            format!("unknown key `{name}` in `{table}`")
        };
        if let Some(suggestion) = closest(name, known.iter().copied()) {
            message.push_str(&format!("; did you mean `{suggestion}`?"));
        }
        self.error(&key.span(), message)
    }

    fn type_error(&self, key: &str, value: &Spanned<DeValue<'_>>, expected: &str) -> ConfigError {
        self.error(
            &value.span(),
            format!(
                "`{key}` must be {expected}, found {}",
                type_name(value.get_ref())
            ),
        )
    }

    fn bool(&self, key: &str, value: &Spanned<DeValue<'_>>) -> Result<bool, ConfigError> {
        value
            .get_ref()
            .as_bool()
            .ok_or_else(|| self.type_error(key, value, "a boolean"))
    }

    fn string(&self, key: &str, value: &Spanned<DeValue<'_>>) -> Result<String, ConfigError> {
        value
            .get_ref()
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| self.type_error(key, value, "a string"))
    }

    fn strings(
        &self,
        key: &str,
        value: &Spanned<DeValue<'_>>,
    ) -> Result<Vec<(String, Range<usize>)>, ConfigError> {
        let array = value
            .get_ref()
            .as_array()
            .ok_or_else(|| self.type_error(key, value, "an array of strings"))?;
        array
            .iter()
            .map(|item| {
                item.get_ref()
                    .as_str()
                    .map(|s| (s.to_owned(), item.span()))
                    .ok_or_else(|| self.type_error(key, item, "an array of strings"))
            })
            .collect()
    }

    fn path(&self, key: &str, value: &Spanned<DeValue<'_>>) -> Result<PathBuf, ConfigError> {
        Ok(absolutize(&self.dir, Path::new(&self.string(key, value)?)))
    }

    fn paths(&self, key: &str, value: &Spanned<DeValue<'_>>) -> Result<Vec<PathBuf>, ConfigError> {
        Ok(self
            .strings(key, value)?
            .into_iter()
            .map(|(s, _)| absolutize(&self.dir, Path::new(&s)))
            .collect())
    }

    fn selector(&self, text: &str, span: &Range<usize>) -> Result<Selector, ConfigError> {
        parse_selector(self.registry, text).map_err(|message| self.error(span, message))
    }

    fn selectors(
        &self,
        key: &str,
        value: &Spanned<DeValue<'_>>,
    ) -> Result<Vec<Selector>, ConfigError> {
        self.strings(key, value)?
            .iter()
            .map(|(text, span)| self.selector(text, span))
            .collect()
    }

    fn severity(&self, key: &str, value: &Spanned<DeValue<'_>>) -> Result<Severity, ConfigError> {
        let text = self.string(key, value)?;
        parse_severity(&text).map_err(|message| self.error(&value.span(), message))
    }

    fn rule(&self, key: &Spanned<std::borrow::Cow<'_, str>>) -> Result<RuleCode, ConfigError> {
        let name = key.get_ref();
        self.registry.find(name).ok_or_else(|| {
            let mut message = format!("unknown rule `{name}`");
            if let Some(suggestion) = self.registry.suggest(name) {
                message.push_str(&format!("; did you mean `{suggestion}`?"));
            }
            self.error(&key.span(), message)
        })
    }

    fn table<'v, 'i>(
        &self,
        key: &str,
        value: &'v Spanned<DeValue<'i>>,
    ) -> Result<&'v DeTable<'i>, ConfigError> {
        value
            .get_ref()
            .as_table()
            .ok_or_else(|| self.type_error(key, value, "a table"))
    }
}

pub(crate) fn parse_severity(text: &str) -> Result<Severity, String> {
    match text {
        "error" => Ok(Severity::Error),
        "warning" => Ok(Severity::Warning),
        "info" => Ok(Severity::Info),
        other => Err(format!(
            "invalid severity `{other}`; expected `error`, `warning` or `info`"
        )),
    }
}

pub(crate) fn parse_selector(registry: &Registry, text: &str) -> Result<Selector, String> {
    registry.parse_selector(text).ok_or_else(|| {
        let mut message = format!("unknown rule selector `{text}`");
        if let Some(suggestion) = registry.suggest(text) {
            message.push_str(&format!("; did you mean `{suggestion}`?"));
        }
        message
    })
}

fn type_name(value: &DeValue<'_>) -> &'static str {
    match value {
        DeValue::String(_) => "a string",
        DeValue::Integer(_) => "an integer",
        DeValue::Float(_) => "a float",
        DeValue::Boolean(_) => "a boolean",
        DeValue::Datetime(_) => "a datetime",
        DeValue::Array(_) => "an array",
        DeValue::Table(_) => "a table",
    }
}

/// A TOML value as a rule option value.
fn option_value(ctx: &Ctx<'_>, value: &Spanned<DeValue<'_>>) -> Result<OptionValue, ConfigError> {
    Ok(match value.get_ref() {
        DeValue::String(s) => OptionValue::String(s.to_string()),
        DeValue::Integer(i) => {
            let digits = i.as_str().replace('_', "");
            OptionValue::Integer(
                i64::from_str_radix(&digits, i.radix())
                    .map_err(|_| ctx.error(&value.span(), "integer out of range".to_owned()))?,
            )
        }
        DeValue::Float(f) => {
            let digits = f.as_str().replace('_', "");
            OptionValue::Float(
                digits
                    .parse()
                    .map_err(|_| ctx.error(&value.span(), "invalid float".to_owned()))?,
            )
        }
        DeValue::Boolean(b) => OptionValue::Bool(*b),
        DeValue::Datetime(_) => {
            return Err(ctx.error(
                &value.span(),
                "datetimes are not valid option values".to_owned(),
            ));
        }
        DeValue::Array(items) => OptionValue::List(
            items
                .iter()
                .map(|item| option_value(ctx, item))
                .collect::<Result<_, _>>()?,
        ),
        DeValue::Table(table) => OptionValue::Table(
            table
                .iter()
                .map(|(k, v)| Ok((k.get_ref().to_string(), option_value(ctx, v)?)))
                .collect::<Result<_, ConfigError>>()?,
        ),
    })
}

fn parse_rule_table(
    ctx: &Ctx<'_>,
    rule: RuleCode,
    table_name: &str,
    value: &Spanned<DeValue<'_>>,
) -> Result<RawRule, ConfigError> {
    let table = ctx.table(table_name, value)?;
    let meta = rule.meta();
    let mut raw = RawRule::default();
    for (key, value) in table.iter() {
        match key.get_ref().as_ref() {
            "severity" => raw.severity = Some(ctx.severity("severity", value)?),
            "enabled" => raw.enabled = Some(ctx.bool("enabled", value)?),
            "exclude" => {
                raw.exclude = Some(
                    ctx.strings("exclude", value)?
                        .into_iter()
                        .map(|(s, _)| s)
                        .collect(),
                );
            }
            name => {
                let Some(option) = meta.option(name) else {
                    let mut message = format!(
                        "unknown option `{name}` for rule {} ({})",
                        meta.code, meta.name
                    );
                    let candidates = meta
                        .options
                        .iter()
                        .map(|o| o.name)
                        .chain(RULE_KEYS.iter().copied());
                    if let Some(suggestion) = closest(name, candidates) {
                        message.push_str(&format!("; did you mean `{suggestion}`?"));
                    }
                    return Err(ctx.error(&key.span(), message));
                };
                let converted = option_value(ctx, value)?;
                if check_option_type(&converted, option.ty).is_err() {
                    return Err(ctx.error(
                        &value.span(),
                        format!(
                            "option `{name}` of rule {} must be {}, found {}",
                            meta.code,
                            option_type_name(option.ty),
                            type_name(value.get_ref())
                        ),
                    ));
                }
                raw.options.insert(name.to_owned(), converted);
            }
        }
    }
    Ok(raw)
}

fn parse_rules(
    ctx: &Ctx<'_>,
    prefix: &str,
    value: &Spanned<DeValue<'_>>,
) -> Result<BTreeMap<RuleCode, RawRule>, ConfigError> {
    let table = ctx.table(&format!("{prefix}rules"), value)?;
    let mut rules: BTreeMap<RuleCode, RawRule> = BTreeMap::new();
    for (key, value) in table.iter() {
        let rule = ctx.rule(key)?;
        let parsed = parse_rule_table(
            ctx,
            rule,
            &format!("{prefix}rules.{}", key.get_ref()),
            value,
        )?;
        // `[rules.SCR101]` and `[rules.undefined-name]` may both appear.
        let merged = rules
            .get(&rule)
            .map_or(parsed.clone(), |r| r.merged(&parsed));
        rules.insert(rule, merged);
    }
    Ok(rules)
}

fn parse_override(
    ctx: &Ctx<'_>,
    index: usize,
    value: &Spanned<DeValue<'_>>,
) -> Result<RawOverride, ConfigError> {
    let name = format!("overrides[{index}]");
    let table = ctx.table(&name, value)?;
    let mut raw = RawOverride::default();
    let mut has_files = false;
    for entry in table.iter() {
        let (key, value): Entry<'_> = entry;
        match key.get_ref().as_ref() {
            "files" => {
                has_files = true;
                raw.files = ctx
                    .strings("files", value)?
                    .into_iter()
                    .map(|(s, _)| s)
                    .collect();
            }
            "select" | "extend-select" => raw.select.extend(ctx.selectors(key.get_ref(), value)?),
            "ignore" | "extend-ignore" => raw.ignore.extend(ctx.selectors(key.get_ref(), value)?),
            "strict" => raw.strict = Some(ctx.bool("strict", value)?),
            "severity" => {
                let table = ctx.table("severity", value)?;
                for (selector, severity) in table.iter() {
                    let parsed = ctx.selector(selector.get_ref(), &selector.span())?;
                    raw.severity
                        .push((parsed, ctx.severity("severity", severity)?));
                }
            }
            "rules" => raw.rules = parse_rules(ctx, &format!("{name}."), value)?,
            _ => return Err(ctx.unknown_key(key, &name, OVERRIDE_KEYS)),
        }
    }
    if !has_files {
        return Err(ctx.error(
            &value.span(),
            format!("`{name}` needs `files`, the globs it applies to"),
        ));
    }
    Ok(raw)
}

fn parse_table(
    ctx: &Ctx<'_>,
    table: &DeTable<'_>,
    table_name: &str,
) -> Result<RawConfig, ConfigError> {
    let mut raw = RawConfig::default();
    for (key, value) in table.iter() {
        let name: &str = key.get_ref();
        match name {
            "extend" => {
                raw.extend = Some((
                    ctx.path("extend", value)?,
                    Some(ctx.position(&value.span())),
                ));
            }
            "select" => raw.layer.select = Some(ctx.selectors(name, value)?),
            "extend-select" => raw.layer.extend_select = ctx.selectors(name, value)?,
            "ignore" => raw.layer.ignore = ctx.selectors(name, value)?,
            "extend-ignore" => raw.layer.extend_ignore = ctx.selectors(name, value)?,
            "unfixable" => raw.unfixable = Some(ctx.selectors(name, value)?),
            "exclude" => {
                raw.exclude = Some(
                    ctx.strings(name, value)?
                        .into_iter()
                        .map(|(s, _)| s)
                        .collect(),
                );
            }
            "extend-exclude" => {
                raw.extend_exclude = ctx
                    .strings(name, value)?
                    .into_iter()
                    .map(|(s, _)| s)
                    .collect();
            }
            "strict" => raw.strict = Some(ctx.bool(name, value)?),
            "projects" => raw.projects = Some(ctx.paths(name, value)?),
            "gateways" => raw.gateways = Some(ctx.paths(name, value)?),
            "baseline" => raw.baseline = Some(ctx.path(name, value)?),
            "respect-gitignore" => raw.respect_gitignore = Some(ctx.bool(name, value)?),
            "cache-dir" => raw.cache_dir = Some(ctx.path(name, value)?),
            "output-format" => {
                let format = ctx.string(name, value)?;
                if !OUTPUT_FORMATS.contains(&format.as_str()) {
                    let mut message = format!("unknown output format `{format}`");
                    if let Some(suggestion) = closest(&format, OUTPUT_FORMATS) {
                        message.push_str(&format!("; did you mean `{suggestion}`?"));
                    } else {
                        message
                            .push_str(&format!("; expected one of {}", OUTPUT_FORMATS.join(", ")));
                    }
                    return Err(ctx.error(&value.span(), message));
                }
                raw.output_format = Some(format);
            }
            "rules" => raw.rules = parse_rules(ctx, "", value)?,
            "overrides" => {
                let array = value
                    .get_ref()
                    .as_array()
                    .ok_or_else(|| ctx.type_error("overrides", value, "an array of tables"))?;
                for (index, item) in array.iter().enumerate() {
                    raw.overrides.push(parse_override(ctx, index, item)?);
                }
            }
            _ => return Err(ctx.unknown_key(key, table_name, TOP_KEYS)),
        }
    }
    Ok(raw)
}

fn syntax_error(path: &Path, text: &str, error: &toml::de::Error) -> ConfigError {
    let position = error.span().map(|span| {
        let index = LineIndex::new(text);
        let lc = index.line_column(TextSize::from(span.start.min(text.len())), text);
        (lc.line, lc.column)
    });
    ConfigError::new(
        path,
        position,
        format!("invalid TOML: {}", error.message().trim()),
    )
}

/// Parses `text` read from `path`. For `pyproject.toml` only `[tool.iglint]`
/// is read; `Ok(None)` means it has none.
pub(crate) fn parse_file(
    path: &Path,
    text: &str,
    registry: &Registry,
) -> Result<Option<RawConfig>, ConfigError> {
    let document = DeTable::parse(text).map_err(|e| syntax_error(path, text, &e))?;
    let ctx = Ctx {
        path,
        dir: path.parent().map(Path::to_path_buf).unwrap_or_default(),
        index: LineIndex::new(text),
        text,
        registry,
    };
    let is_pyproject = path.file_name().is_some_and(|n| n == "pyproject.toml");
    if is_pyproject {
        let Some(tool) = document.get_ref().get("tool") else {
            return Ok(None);
        };
        let tool = ctx.table("tool", tool)?;
        let Some(iglint) = tool.get("iglint") else {
            return Ok(None);
        };
        let table = ctx.table("tool.iglint", iglint)?;
        return parse_table(&ctx, table, "tool.iglint").map(Some);
    }
    parse_table(&ctx, document.get_ref(), "").map(Some)
}

pub(crate) fn read_file(
    path: &Path,
    registry: &Registry,
) -> Result<Option<RawConfig>, ConfigError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| ConfigError::new(path, None, format!("cannot read configuration: {e}")))?;
    parse_file(path, &text, registry)
}

pub(crate) fn pyproject_has_iglint(path: &Path) -> Result<bool, ConfigError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| ConfigError::new(path, None, format!("cannot read configuration: {e}")))?;
    let document = DeTable::parse(&text).map_err(|e| syntax_error(path, &text, &e))?;
    Ok(document
        .get_ref()
        .get("tool")
        .and_then(|tool| tool.get_ref().as_table())
        .is_some_and(|tool| tool.get("iglint").is_some()))
}
