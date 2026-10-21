//! Turning validated configuration files into [`ProjectSettings`].

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use iglint_linter::registry::{Registry, RuleCode, Selector};
use iglint_linter::settings::{LinterSettings, Override, ProjectSettings, RuleSettings};

use crate::compat::{self, CompatMode};
use crate::schema::{self, Layer, RawConfig, RawOverride, RawRule};
use crate::{ConfigError, ConfigSource, Configuration, LoadOptions, absolutize};

/// How specific a selector is; the most specific matching selector decides.
fn specificity(selector: Selector) -> usize {
    match selector {
        Selector::All => 0,
        Selector::Category(_) => 1,
        Selector::Prefix(prefix) => 1 + prefix.len(),
        Selector::Rule(_) => usize::MAX,
    }
}

/// The verdict of the most specific selectors among `enable` and `disable`
/// that match `rule`; `disable` wins ties. `None` if none matches.
fn verdict(rule: RuleCode, enable: &[Selector], disable: &[Selector]) -> Option<bool> {
    let best = |selectors: &[Selector]| {
        selectors
            .iter()
            .filter(|s| Registry::matches(**s, rule))
            .map(|s| specificity(*s))
            .max()
    };
    match (best(enable), best(disable)) {
        (None, None) => None,
        (Some(_), None) => Some(true),
        (None, Some(_)) => Some(false),
        (Some(e), Some(d)) => Some(e > d),
    }
}

/// Applies one layer of `select`/`ignore` to the enabled set.
fn apply_layer(registry: &Registry, enabled: &mut BTreeSet<RuleCode>, layer: &Layer) {
    let mut enable: Vec<Selector> = layer.select.clone().unwrap_or_default();
    enable.extend(layer.extend_select.iter().copied());
    let mut disable = layer.ignore.clone();
    disable.extend(layer.extend_ignore.iter().copied());
    for rule in registry.iter() {
        let on = match verdict(rule, &enable, &disable) {
            Some(on) => on,
            None if layer.select.is_some() => false,
            None => enabled.contains(&rule),
        };
        if on {
            enabled.insert(rule);
        } else {
            enabled.remove(&rule);
        }
    }
}

fn matching(registry: &Registry, selectors: &[Selector]) -> BTreeSet<RuleCode> {
    registry
        .iter()
        .filter(|rule| selectors.iter().any(|s| Registry::matches(*s, *rule)))
        .collect()
}

fn rule_settings(rule: RuleCode, raw: Option<&RawRule>) -> RuleSettings {
    RuleSettings {
        severity: raw
            .and_then(|r| r.severity)
            .unwrap_or(rule.meta().default_severity),
        options: raw.map(|r| r.options.clone()).unwrap_or_default(),
    }
}

/// Reads `path` and the files it extends, base first.
fn read_chain(path: &Path, registry: &Registry) -> Result<Vec<RawConfig>, ConfigError> {
    let mut chain = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut next = Some((path.to_path_buf(), None::<(PathBuf, Option<(u32, u32)>)>));
    while let Some((current, referenced_from)) = next.take() {
        if seen.contains(&current) {
            let mut cycle: Vec<String> = seen.iter().map(|p| p.display().to_string()).collect();
            cycle.push(current.display().to_string());
            let (from, position) = referenced_from.expect("a cycle has a referrer");
            return Err(ConfigError::new(
                &from,
                position,
                format!("circular `extend`: {}", cycle.join(" -> ")),
            ));
        }
        if !current.is_file() {
            let (from, position) = referenced_from.unwrap_or((current.clone(), None));
            return Err(ConfigError::new(
                &from,
                position,
                format!("extended configuration `{}` not found", current.display()),
            ));
        }
        let raw = schema::read_file(&current, registry)?.ok_or_else(|| {
            let (from, position) = referenced_from.clone().unwrap_or((current.clone(), None));
            ConfigError::new(
                &from,
                position,
                format!("`{}` has no [tool.iglint] table", current.display()),
            )
        })?;
        seen.push(current.clone());
        if let Some((base, position)) = &raw.extend {
            next = Some((base.clone(), Some((current.clone(), *position))));
        }
        chain.push(raw);
    }
    chain.reverse();
    Ok(chain)
}

/// Settings fields that are plain values: the last file that sets one wins.
#[derive(Default)]
struct Merged {
    exclude: Vec<String>,
    unfixable: Option<Vec<Selector>>,
    strict: Option<bool>,
    projects: Option<Vec<PathBuf>>,
    gateways: Option<Vec<PathBuf>>,
    baseline: Option<PathBuf>,
    respect_gitignore: Option<bool>,
    cache_dir: Option<PathBuf>,
    output_format: Option<String>,
    rules: BTreeMap<RuleCode, RawRule>,
    overrides: Vec<RawOverride>,
    layers: Vec<Layer>,
}

fn merge(chain: Vec<RawConfig>) -> Merged {
    let mut merged = Merged::default();
    for raw in chain {
        if let Some(exclude) = raw.exclude {
            merged.exclude = exclude;
        }
        merged.exclude.extend(raw.extend_exclude);
        merged.unfixable = raw.unfixable.or(merged.unfixable);
        merged.strict = raw.strict.or(merged.strict);
        merged.projects = raw.projects.or(merged.projects);
        merged.gateways = raw.gateways.or(merged.gateways);
        merged.baseline = raw.baseline.or(merged.baseline);
        merged.respect_gitignore = raw.respect_gitignore.or(merged.respect_gitignore);
        merged.cache_dir = raw.cache_dir.or(merged.cache_dir);
        merged.output_format = raw.output_format.or(merged.output_format);
        for (rule, settings) in raw.rules {
            let combined = merged
                .rules
                .get(&rule)
                .map_or(settings.clone(), |base| base.merged(&settings));
            merged.rules.insert(rule, combined);
        }
        merged.overrides.extend(raw.overrides);
        merged.layers.push(raw.layer);
    }
    merged
}

fn cli_layer(options: &LoadOptions, registry: &Registry) -> Result<Layer, ConfigError> {
    let parse = |flag: &str, values: &[String]| -> Result<Vec<Selector>, ConfigError> {
        values
            .iter()
            .map(|v| {
                schema::parse_selector(registry, v)
                    .map_err(|message| ConfigError::cli(format!("{message} (in {flag})")))
            })
            .collect()
    };
    let cli = &options.cli;
    Ok(Layer {
        select: cli
            .select
            .as_deref()
            .map(|s| parse("--select", s))
            .transpose()?,
        extend_select: parse("--extend-select", &cli.extend_select)?,
        ignore: parse("--ignore", &cli.ignore)?,
        extend_ignore: parse("--extend-ignore", &cli.extend_ignore)?,
    })
}

fn build_override(
    registry: &Registry,
    base_rules: &BTreeMap<RuleCode, RawRule>,
    raw: &RawOverride,
) -> Override {
    let mut enable = BTreeMap::new();
    let mut disable = Vec::new();
    for rule in registry.iter() {
        let table = raw.rules.get(&rule);
        let on = match table.and_then(|t| t.enabled) {
            Some(on) => Some(on),
            None => verdict(rule, &raw.select, &raw.ignore).or(table.map(|_| true)),
        };
        match on {
            Some(true) => {
                let combined = match (base_rules.get(&rule), table) {
                    (Some(base), Some(table)) => Some(base.merged(table)),
                    (Some(base), None) => Some(base.clone()),
                    (None, Some(table)) => Some(table.clone()),
                    (None, None) => None,
                };
                enable.insert(rule, rule_settings(rule, combined.as_ref()));
            }
            Some(false) => disable.push(rule),
            None => {}
        }
    }
    let mut severity = BTreeMap::new();
    for (selector, level) in &raw.severity {
        for rule in matching(registry, &[*selector]) {
            severity.insert(rule, *level);
        }
    }
    Override {
        files: raw.files.clone(),
        disable,
        enable,
        strict: raw.strict,
        severity,
    }
}

fn whitelist(options: &LoadOptions) -> Result<Vec<PathBuf>, ConfigError> {
    let Some(path) = &options.cli.whitelist else {
        return Ok(Vec::new());
    };
    let path = absolutize(&options.cwd, path);
    compat::read_whitelist(&path, &options.cwd)
        .map_err(|e| ConfigError::new(&path, None, format!("cannot read whitelist: {e}")))
}

pub(crate) fn load_source(
    source: ConfigSource,
    options: &LoadOptions,
    registry: &Registry,
) -> Result<Configuration, ConfigError> {
    let cli = cli_layer(options, registry)?;
    let mut whitelist = whitelist(options)?;
    match &source {
        ConfigSource::IgnLint(path) => {
            let config = compat::read_rule_config(path)
                .map_err(|e| ConfigError::new(path, None, e.message))?;
            let root = path.parent().map(Path::to_path_buf).unwrap_or_default();
            let mut resolved =
                compat::to_settings(&config, registry, &root, CompatMode::ExtendDefaults);
            let mut notices = vec![format!(
                "using the ign-lint configuration `{}`; ign-lint configuration support is deprecated, run `iglint migrate` to convert it to iglint.toml",
                path.display()
            )];
            notices.extend(config.messages.iter().cloned());
            notices.append(&mut resolved.notices);
            if options.cli.whitelist.is_none() {
                for name in compat::WHITELIST_FILES {
                    let candidate = root.join(name);
                    if candidate.is_file() {
                        whitelist = compat::read_whitelist(&candidate, &root).map_err(|e| {
                            ConfigError::new(
                                &candidate,
                                None,
                                format!("cannot read whitelist: {e}"),
                            )
                        })?;
                        notices.push(format!(
                            "skipping the {} files listed in the ign-lint whitelist `{}`",
                            whitelist.len(),
                            candidate.display()
                        ));
                        break;
                    }
                }
            }
            let mut settings = resolved.settings;
            let mut enabled: BTreeSet<RuleCode> = settings.base.rules.keys().copied().collect();
            apply_layer(registry, &mut enabled, &cli);
            let mut base = (*settings.base).clone();
            base.rules.retain(|rule, _| enabled.contains(rule));
            for rule in enabled {
                base.rules
                    .entry(rule)
                    .or_insert_with(|| rule_settings(rule, None));
            }
            if let Some(strict) = options.cli.strict {
                base.strict = strict;
            }
            settings.base = Arc::new(base);
            settings.whitelist = whitelist;
            Ok(Configuration {
                source,
                settings,
                baseline: None,
                cache_dir: None,
                output_format: None,
                notices,
            })
        }
        ConfigSource::Defaults | ConfigSource::Toml(_) | ConfigSource::PyProject(_) => {
            let (chain, root) = match source.path() {
                Some(path) => (
                    read_chain(path, registry)?,
                    path.parent().map(Path::to_path_buf).unwrap_or_default(),
                ),
                None => (Vec::new(), options.cwd.clone()),
            };
            let merged = merge(chain);

            let mut enabled: BTreeSet<RuleCode> = registry
                .iter()
                .filter(|r| r.meta().default_enabled)
                .collect();
            for layer in &merged.layers {
                apply_layer(registry, &mut enabled, layer);
            }
            for (rule, raw) in &merged.rules {
                match raw.enabled {
                    Some(true) => {
                        enabled.insert(*rule);
                    }
                    Some(false) => {
                        enabled.remove(rule);
                    }
                    None => {}
                }
            }
            // Command-line selection beats `enabled` in files.
            apply_layer(registry, &mut enabled, &cli);
            let rules = enabled
                .iter()
                .map(|rule| (*rule, rule_settings(*rule, merged.rules.get(rule))))
                .collect();
            let rule_exclude = merged
                .rules
                .iter()
                .filter_map(|(rule, raw)| raw.exclude.clone().map(|globs| (*rule, globs)))
                .filter(|(_, globs)| !globs.is_empty())
                .collect();
            let overrides = merged
                .overrides
                .iter()
                .map(|o| build_override(registry, &merged.rules, o))
                .collect();
            let settings = ProjectSettings {
                root: root.clone(),
                base: Arc::new(LinterSettings {
                    rules,
                    strict: options.cli.strict.or(merged.strict).unwrap_or(false),
                }),
                overrides,
                exclude: merged.exclude,
                rule_exclude,
                whitelist,
                projects: merged.projects.unwrap_or_default(),
                gateways: merged.gateways.unwrap_or_default(),
                respect_gitignore: merged.respect_gitignore.unwrap_or(true),
                unfixable: matching(registry, &merged.unfixable.unwrap_or_default()),
            };
            Ok(Configuration {
                source,
                settings,
                baseline: merged.baseline,
                cache_dir: merged.cache_dir,
                output_format: merged.output_format,
                notices: Vec::new(),
            })
        }
    }
}
