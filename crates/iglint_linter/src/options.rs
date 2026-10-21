//! Typed access to rule options.
//!
//! Configuration stores options as [`OptionValue`]s keyed by name. A rule
//! turns them into its own options struct once per settings object (not per
//! file or node) through an [`OptionReader`], which validates types and
//! falls back to the default declared in [`RuleMeta::options`].

use std::collections::BTreeMap;

use crate::registry::{OptionMeta, OptionType, RuleMeta};
use crate::settings::OptionValue;

/// Reads the options of one rule, with defaults from its metadata.
pub struct OptionReader<'a> {
    meta: &'static RuleMeta,
    values: &'a BTreeMap<String, OptionValue>,
}

impl<'a> OptionReader<'a> {
    pub fn new(meta: &'static RuleMeta, values: &'a BTreeMap<String, OptionValue>) -> Self {
        OptionReader { meta, values }
    }

    /// The configured value, or the parsed default of a declared option;
    /// `None` when neither exists (defaults written as `none`).
    fn value_opt(&self, name: &str) -> Result<Option<OptionValue>, String> {
        if let Some(value) = self.values.get(name) {
            return Ok(Some(value.clone()));
        }
        let option = self.option(name)?;
        let default = option.default.trim();
        if default.is_empty() || default == "none" {
            return Ok(None);
        }
        parse_default(option).map(Some)
    }

    fn value(&self, name: &str) -> Result<OptionValue, String> {
        self.value_opt(name)?
            .ok_or_else(|| format!("option `{name}` of {} has no value", self.meta.code))
    }

    fn option(&self, name: &str) -> Result<&'static OptionMeta, String> {
        self.meta
            .option(name)
            .ok_or_else(|| format!("{}: `{name}` is not a declared option", self.meta.code))
    }

    fn wrong(&self, name: &str, expected: &str, found: &OptionValue) -> String {
        format!(
            "option `{name}` of {} must be {expected}, found {}",
            self.meta.code,
            found.type_name()
        )
    }

    /// Whether the option was set in the configuration (as opposed to using
    /// its default).
    pub fn is_set(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }

    pub fn bool(&self, name: &str) -> Result<bool, String> {
        let v = self.value(name)?;
        v.as_bool().ok_or_else(|| self.wrong(name, "a boolean", &v))
    }

    pub fn int(&self, name: &str) -> Result<i64, String> {
        let v = self.value(name)?;
        v.as_i64().ok_or_else(|| self.wrong(name, "an integer", &v))
    }

    /// An integer option whose default is `none`.
    pub fn opt_int(&self, name: &str) -> Result<Option<i64>, String> {
        match self.value_opt(name)? {
            None => Ok(None),
            Some(v) => v
                .as_i64()
                .map(Some)
                .ok_or_else(|| self.wrong(name, "an integer", &v)),
        }
    }

    pub fn float(&self, name: &str) -> Result<f64, String> {
        let v = self.value(name)?;
        v.as_f64().ok_or_else(|| self.wrong(name, "a number", &v))
    }

    pub fn string(&self, name: &str) -> Result<String, String> {
        let v = self.value(name)?;
        v.as_str()
            .map(str::to_owned)
            .ok_or_else(|| self.wrong(name, "a string", &v))
    }

    /// A string option whose default is `none`.
    pub fn opt_string(&self, name: &str) -> Result<Option<String>, String> {
        match self.value_opt(name)? {
            None => Ok(None),
            Some(v) => v
                .as_str()
                .map(|s| Some(s.to_owned()))
                .ok_or_else(|| self.wrong(name, "a string", &v)),
        }
    }

    pub fn string_list(&self, name: &str) -> Result<Vec<String>, String> {
        let v = self.value(name)?;
        v.as_string_list()
            .map(|l| l.into_iter().map(str::to_owned).collect())
            .ok_or_else(|| self.wrong(name, "an array of strings", &v))
    }

    /// A string list option whose default is `none`.
    pub fn opt_string_list(&self, name: &str) -> Result<Option<Vec<String>>, String> {
        match self.value_opt(name)? {
            None => Ok(None),
            Some(v) => v
                .as_string_list()
                .map(|l| Some(l.into_iter().map(str::to_owned).collect()))
                .ok_or_else(|| self.wrong(name, "an array of strings", &v)),
        }
    }

    /// A table option (empty when unset and without default).
    pub fn table(&self, name: &str) -> Result<BTreeMap<String, OptionValue>, String> {
        match self.value_opt(name)? {
            None => Ok(BTreeMap::new()),
            Some(OptionValue::Table(t)) => Ok(t),
            Some(other) => Err(self.wrong(name, "a table", &other)),
        }
    }

    /// The configured value, for options whose shape the rule validates
    /// itself.
    pub fn raw(&self, name: &str) -> Option<&OptionValue> {
        self.values.get(name)
    }
}

/// Parses an option's documented default (a TOML value). An empty default
/// means "unset" and is an error for typed getters.
pub fn parse_default(option: &OptionMeta) -> Result<OptionValue, String> {
    let text = option.default.trim();
    if text.is_empty() || text == "none" {
        return Err(format!("option `{}` has no default", option.name));
    }
    let doc: toml::Table = format!("v = {text}")
        .parse()
        .map_err(|e| format!("invalid default of option `{}`: {e}", option.name))?;
    let value = doc.get("v").cloned().unwrap_or(toml::Value::Boolean(false));
    let converted = from_toml(value);
    let ok = match option.ty {
        OptionType::Bool => matches!(converted, OptionValue::Bool(_)),
        OptionType::Integer => matches!(converted, OptionValue::Integer(_)),
        OptionType::Float => matches!(converted, OptionValue::Integer(_) | OptionValue::Float(_)),
        OptionType::String => matches!(converted, OptionValue::String(_)),
        OptionType::StringList => converted.as_string_list().is_some(),
        OptionType::Table => matches!(converted, OptionValue::Table(_)),
    };
    if ok {
        Ok(converted)
    } else {
        Err(format!(
            "default of option `{}` does not match its type",
            option.name
        ))
    }
}

fn from_toml(value: toml::Value) -> OptionValue {
    match value {
        toml::Value::String(s) => OptionValue::String(s),
        toml::Value::Integer(i) => OptionValue::Integer(i),
        toml::Value::Float(f) => OptionValue::Float(f),
        toml::Value::Boolean(b) => OptionValue::Bool(b),
        toml::Value::Datetime(d) => OptionValue::String(d.to_string()),
        toml::Value::Array(a) => OptionValue::List(a.into_iter().map(from_toml).collect()),
        toml::Value::Table(t) => {
            OptionValue::Table(t.into_iter().map(|(k, v)| (k, from_toml(v))).collect())
        }
    }
}
