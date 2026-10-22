use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use iglint_linter::Severity;
use iglint_linter::registry::RuleCode;
use iglint_linter::settings::OptionValue;
use iglint_linter::testing::{self, REGISTRY};

use crate::compat::{self, CompatMode};
use crate::{CliOverrides, ConfigSource, Configuration, LoadOptions, load, migrate};

fn rule(meta: &'static iglint_linter::RuleMeta) -> RuleCode {
    RuleCode::new(meta)
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    path
}

fn options(dir: &Path) -> LoadOptions {
    LoadOptions {
        cwd: dir.to_path_buf(),
        start: Some(dir.to_path_buf()),
        ..LoadOptions::default()
    }
}

fn load_toml(text: &str) -> Result<Configuration, String> {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "iglint.toml", text);
    load(&options(dir.path()), &REGISTRY).map_err(|e| {
        // Paths differ per run; keep the file name only.
        e.to_string()
            .replace(&format!("{}/", dir.path().display()), "")
    })
}

fn enabled(config: &Configuration) -> Vec<&'static str> {
    config
        .settings
        .base
        .rules
        .keys()
        .map(|r| r.code())
        .collect()
}

#[test]
fn defaults_are_the_default_enabled_rules() {
    let config = load_toml("").unwrap();
    assert_eq!(enabled(&config), ["NAME001", "SCR101", "SCR102", "VIEW001"]);
    assert!(config.settings.respect_gitignore);
    assert!(!config.settings.base.strict);
    let undefined = config
        .settings
        .base
        .rule(rule(&testing::UNDEFINED_NAME))
        .unwrap();
    assert_eq!(undefined.severity, Severity::Error);
}

#[test]
fn select_and_ignore_by_specificity() {
    let config = load_toml(
        r#"select = ["SCR", "NAME002"]
ignore = ["SCR1"]
extend-select = ["SCR101"]
"#,
    )
    .unwrap();
    // SCR101 is more specific than the SCR1 ignore; SCR102 is not.
    assert_eq!(enabled(&config), ["NAME002", "SCR101", "SCR900"]);

    let config = load_toml(
        r#"extend-select = ["ALL"]
ignore = ["NAME", "unused-variable"]
"#,
    )
    .unwrap();
    assert_eq!(
        enabled(&config),
        ["BIND001", "REF001", "SCR101", "SCR900", "VIEW001"]
    );
}

#[test]
fn rule_tables_set_severity_options_enabled_and_excludes() {
    let config = load_toml(
        r#"
[rules.unused-variable]
severity = "error"
ignore-names = ["x", "y"]
exclude = ["legacy/**"]

[rules.BIND001]
enabled = true
minimum-interval = 5000

[rules.SCR101]
enabled = false
"#,
    )
    .unwrap();
    assert_eq!(
        enabled(&config),
        ["BIND001", "NAME001", "SCR102", "VIEW001"]
    );
    let unused = config
        .settings
        .base
        .rule(rule(&testing::UNUSED_VARIABLE))
        .unwrap();
    assert_eq!(unused.severity, Severity::Error);
    assert_eq!(
        unused.options["ignore-names"],
        OptionValue::List(vec![
            OptionValue::String("x".into()),
            OptionValue::String("y".into())
        ])
    );
    let polling = config
        .settings
        .base
        .rule(rule(&testing::POLLING_INTERVAL))
        .unwrap();
    assert_eq!(
        polling.options["minimum-interval"],
        OptionValue::Integer(5000)
    );
    let root = &config.settings.root;
    let legacy = config.settings.for_file(&root.join("legacy/a/code.py"));
    assert!(!legacy.is_enabled(rule(&testing::UNUSED_VARIABLE)));
    let other = config.settings.for_file(&root.join("projects/a/code.py"));
    assert!(other.is_enabled(rule(&testing::UNUSED_VARIABLE)));
}

#[test]
fn overrides() {
    let config = load_toml(
        r#"
[[overrides]]
files = ["projects/*/views/Legacy/**"]
ignore = ["NAME"]
select = ["BIND001"]
severity = { SCR = "info" }
strict = true

[overrides.rules.SCR102]
dummy-pattern = "^tmp"

[[overrides]]
files = ["**/Legacy/Keep/**"]
select = ["NAME001"]
"#,
    )
    .unwrap();
    let root = config.settings.root.clone();
    let legacy = config
        .settings
        .for_file(&root.join("projects/p/views/Legacy/A/view.json"));
    let codes: Vec<_> = legacy.rules.keys().map(|r| r.code()).collect();
    assert_eq!(codes, ["BIND001", "SCR101", "SCR102", "VIEW001"]);
    assert!(legacy.strict);
    assert_eq!(
        legacy.rules[&rule(&testing::UNDEFINED_NAME)].severity,
        Severity::Info
    );
    // The override's rule table enables with the override's options; the
    // severity table applies to it as well.
    let unused = &legacy.rules[&rule(&testing::UNUSED_VARIABLE)];
    assert_eq!(
        unused.options["dummy-pattern"],
        OptionValue::String("^tmp".into())
    );
    assert_eq!(unused.severity, Severity::Info);
    // Severity changes never enable a rule.
    assert!(!legacy.is_enabled(rule(&testing::MISSING_VIEW)));

    let keep = config
        .settings
        .for_file(&root.join("projects/p/views/Legacy/Keep/view.json"));
    assert!(keep.is_enabled(rule(&testing::COMPONENT_NAME)));

    let elsewhere = config
        .settings
        .for_file(&root.join("projects/p/views/New/view.json"));
    assert_eq!(*elsewhere, *config.settings.base);
}

#[test]
fn exclude_and_extend_exclude() {
    let config = load_toml(
        r#"exclude = ["build"]
extend-exclude = ["**/Legacy/**"]
respect-gitignore = false
strict = true
baseline = "ci/baseline.json"
cache-dir = ".cache/iglint"
output-format = "github"
projects = ["projects/a"]
"#,
    )
    .unwrap();
    let root = config.settings.root.clone();
    assert_eq!(config.settings.exclude, ["build", "**/Legacy/**"]);
    assert!(config.settings.is_excluded(&root.join("build/x.py")));
    assert!(
        config
            .settings
            .is_excluded(&root.join("p/views/Legacy/v/view.json"))
    );
    assert!(
        !config
            .settings
            .is_excluded(&root.join("p/views/New/v/view.json"))
    );
    assert!(!config.settings.respect_gitignore);
    assert!(config.settings.base.strict);
    assert_eq!(config.baseline, Some(root.join("ci/baseline.json")));
    assert_eq!(config.cache_dir, Some(root.join(".cache/iglint")));
    assert_eq!(config.output_format.as_deref(), Some("github"));
    assert_eq!(config.settings.projects, [root.join("projects/a")]);
}

#[test]
fn errors_have_positions_and_suggestions() {
    let cases: &[(&str, &str)] = &[
        (
            "selct = [\"SCR\"]\n",
            "iglint.toml:1:1: unknown key `selct`; did you mean `select`?",
        ),
        (
            "extend_select = [\"SCR\"]\n",
            "iglint.toml:1:1: unknown key `extend_select`; did you mean `extend-select`?",
        ),
        ("nonsense = 1\n", "iglint.toml:1:1: unknown key `nonsense`"),
        (
            "select = [\"SCR\", \"SCR1O1\"]\n",
            "iglint.toml:1:18: unknown rule selector `SCR1O1`; did you mean `SCR101`?",
        ),
        (
            "select = \"SCR\"\n",
            "iglint.toml:1:10: `select` must be an array of strings, found a string",
        ),
        (
            "select = [\"SCR\", 3]\n",
            "iglint.toml:1:18: `select` must be an array of strings, found an integer",
        ),
        (
            "strict = \"yes\"\n",
            "iglint.toml:1:10: `strict` must be a boolean, found a string",
        ),
        (
            "\n[rules.undefined-nam]\n",
            "iglint.toml:2:8: unknown rule `undefined-nam`; did you mean `undefined-name`?",
        ),
        (
            "[rules.SCR102]\nignore-name = []\n",
            "iglint.toml:2:1: unknown option `ignore-name` for rule SCR102 (unused-variable); did you mean `ignore-names`?",
        ),
        (
            "[rules.SCR102]\nseverty = \"error\"\n",
            "iglint.toml:2:1: unknown option `severty` for rule SCR102 (unused-variable); did you mean `severity`?",
        ),
        (
            "[rules.SCR102]\ndummy-pattern = 5\n",
            "iglint.toml:2:17: option `dummy-pattern` of rule SCR102 must be a string, found an integer",
        ),
        (
            "[rules.SCR102]\nignore-names = [1]\n",
            "iglint.toml:2:16: option `ignore-names` of rule SCR102 must be an array of strings, found an array",
        ),
        (
            "[rules.SCR102]\nseverity = \"fatal\"\n",
            "iglint.toml:2:12: invalid severity `fatal`; expected `error`, `warning` or `info`",
        ),
        (
            "output-format = \"jsonl\"\n",
            "iglint.toml:1:17: unknown output format `jsonl`; did you mean `json`?",
        ),
        (
            "[[overrides]]\nselect = [\"SCR\"]\n",
            "iglint.toml:1:1: `overrides[0]` needs `files`, the globs it applies to",
        ),
        (
            "[[overrides]]\nfiles = [\"a\"]\nignor = [\"SCR\"]\n",
            "iglint.toml:3:1: unknown key `ignor` in `overrides[0]`; did you mean `ignore`?",
        ),
        (
            "[[overrides]]\nfiles = [\"a\"]\nseverity = { SCR = \"loud\" }\n",
            "iglint.toml:3:20: invalid severity `loud`; expected `error`, `warning` or `info`",
        ),
        (
            "[[overrides]]\nfiles = [\"a\"]\n[overrides.rules.SCR102]\nignore-names = \"x\"\n",
            "iglint.toml:4:16: option `ignore-names` of rule SCR102 must be an array of strings, found a string",
        ),
        (
            "select = [\n",
            "iglint.toml:1:11: invalid TOML: unclosed array, expected `]`",
        ),
    ];
    for (text, expected) in cases {
        let error = load_toml(text).unwrap_err();
        assert!(
            error.starts_with(expected),
            "for {text:?}\n  got: {error}\n want: {expected}"
        );
    }
}

#[test]
fn discovery_order_and_pyproject() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "iglint.toml", "select = [\"SCR101\"]\n");
    write(root, "sub/pyproject.toml", "[project]\nname = \"x\"\n");
    write(
        root,
        "sub/deeper/pyproject.toml",
        "[tool.iglint]\nselect = [\"SCR102\"]\n",
    );
    std::fs::create_dir_all(root.join("sub/deeper/x")).unwrap();

    // A pyproject without [tool.iglint] is skipped.
    let mut opts = options(root);
    opts.start = Some(root.join("sub"));
    let config = load(&opts, &REGISTRY).unwrap();
    assert_eq!(config.source, ConfigSource::Toml(root.join("iglint.toml")));
    assert_eq!(enabled(&config), ["SCR101"]);

    opts.start = Some(root.join("sub/deeper/x"));
    let config = load(&opts, &REGISTRY).unwrap();
    assert_eq!(
        config.source,
        ConfigSource::PyProject(root.join("sub/deeper/pyproject.toml"))
    );
    assert_eq!(enabled(&config), ["SCR102"]);
    assert_eq!(config.settings.root, root.join("sub/deeper"));

    // iglint.toml wins over .iglint.toml in the same directory.
    write(root, "sub/.iglint.toml", "select = [\"NAME\"]\n");
    write(root, "sub/iglint.toml", "select = [\"VIEW\"]\n");
    opts.start = Some(root.join("sub"));
    assert_eq!(enabled(&load(&opts, &REGISTRY).unwrap()), ["VIEW001"]);

    // --isolated ignores all of them, --config picks one.
    opts.isolated = true;
    assert_eq!(
        load(&opts, &REGISTRY).unwrap().source,
        ConfigSource::Defaults
    );
    opts.isolated = false;
    opts.config_file = Some(PathBuf::from("sub/.iglint.toml"));
    assert_eq!(
        enabled(&load(&opts, &REGISTRY).unwrap()),
        ["NAME001", "NAME002"]
    );

    opts.config_file = Some(PathBuf::from("missing.toml"));
    let error = load(&opts, &REGISTRY).unwrap_err();
    assert_eq!(error.message, "configuration file not found");
}

#[test]
fn pyproject_errors_point_into_the_pyproject() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "pyproject.toml",
        "[project]\nname = \"x\"\n\n[tool.iglint]\nselekt = []\n",
    );
    let error = load(&options(dir.path()), &REGISTRY).unwrap_err();
    assert_eq!(error.position, Some((5, 1)));
    assert_eq!(
        error.message,
        "unknown key `selekt` in `tool.iglint`; did you mean `select`?"
    );
}

#[test]
fn extend_inherits_and_detects_cycles() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "base.toml",
        "select = [\"SCR\"]\nexclude = [\"build\"]\nbaseline = \"base-baseline.json\"\n[rules.SCR102]\nignore-names = [\"a\"]\nseverity = \"error\"\n",
    );
    write(
        root,
        "project/iglint.toml",
        "extend = \"../base.toml\"\nextend-select = [\"VIEW001\"]\nextend-exclude = [\"tmp\"]\n[rules.SCR102]\nignore-names = [\"b\"]\n",
    );
    let config = load(&options(&root.join("project")), &REGISTRY).unwrap();
    assert_eq!(enabled(&config), ["SCR101", "SCR102", "SCR900", "VIEW001"]);
    assert_eq!(config.settings.exclude, ["build", "tmp"]);
    // Paths are relative to the file that sets them.
    assert_eq!(config.baseline, Some(root.join("base-baseline.json")));
    let unused = config
        .settings
        .base
        .rule(rule(&testing::UNUSED_VARIABLE))
        .unwrap();
    assert_eq!(unused.severity, Severity::Error);
    assert_eq!(
        unused.options["ignore-names"],
        OptionValue::List(vec![OptionValue::String("b".into())])
    );

    write(root, "a.toml", "extend = \"b.toml\"\n");
    write(root, "b.toml", "extend = \"a.toml\"\n");
    let mut opts = options(root);
    opts.config_file = Some(PathBuf::from("a.toml"));
    let error = load(&opts, &REGISTRY).unwrap_err();
    assert!(error.message.starts_with("circular `extend`: "), "{error}");
    assert_eq!(error.path, Some(root.join("b.toml")));
    assert_eq!(error.position, Some((1, 10)));

    write(root, "c.toml", "\nextend = \"nope.toml\"\n");
    opts.config_file = Some(PathBuf::from("c.toml"));
    let error = load(&opts, &REGISTRY).unwrap_err();
    assert_eq!(error.position, Some((2, 10)));
    assert!(error.message.contains("not found"), "{error}");
}

#[test]
fn command_line_overrides_come_last() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "iglint.toml",
        "[rules.REF001]\nenabled = true\n",
    );
    let mut opts = options(dir.path());
    opts.cli = CliOverrides {
        select: Some(vec!["REF".into(), "SCR".into()]),
        ignore: vec!["SCR102".into()],
        strict: Some(true),
        ..CliOverrides::default()
    };
    let config = load(&opts, &REGISTRY).unwrap();
    assert_eq!(enabled(&config), ["REF001", "SCR101", "SCR900"]);
    assert!(config.settings.base.strict);

    opts.cli.ignore = vec!["SCR10O".into()];
    let error = load(&opts, &REGISTRY).unwrap_err();
    assert_eq!(
        error.to_string(),
        "unknown rule selector `SCR10O`; did you mean `SCR101`? (in --ignore)"
    );
}

#[test]
fn legacy_rule_config_is_detected_with_its_whitelist() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "rule_config.json",
        r#"{"PollingIntervalRule": {"kwargs": {"minimum_interval": 2000}}, "UnusedCustomPropertiesRule": {"enabled": false}, "Nope": {}}"#,
    );
    write(
        root,
        ".ignition-lint-whitelist.txt",
        "# legacy\nviews/Old/view.json\n",
    );
    std::fs::create_dir_all(root.join("views")).unwrap();
    let mut opts = options(root);
    opts.start = Some(root.join("views"));
    let config = load(&opts, &REGISTRY).unwrap();
    assert_eq!(
        config.source,
        ConfigSource::IgnLint(root.join("rule_config.json"))
    );
    // Defaults, minus the disabled rule; on views (the files ign-lint
    // lints) also ign-lint's PollingIntervalRule and PylintScriptRule.
    assert_eq!(enabled(&config), ["NAME001", "SCR101", "SCR102"]);
    let view: Vec<&str> = config
        .settings
        .for_file(&root.join("views/A/view.json"))
        .rules
        .keys()
        .map(|r| r.code())
        .collect();
    assert_eq!(
        view,
        [
            "BIND001", "NAME001", "NAME002", "SCR101", "SCR102", "SCR900"
        ]
    );
    let view_settings = config.settings.for_file(&root.join("views/A/view.json"));
    let polling = view_settings
        .rule(rule(&testing::POLLING_INTERVAL))
        .unwrap();
    assert_eq!(
        polling.options["minimum-interval"],
        OptionValue::Integer(2000)
    );
    assert_eq!(
        config.settings.whitelist,
        [root.join("views/Old/view.json")]
    );
    assert!(
        config.notices[0].contains("run `iglint migrate`"),
        "{:?}",
        config.notices
    );
    assert!(
        config
            .notices
            .contains(&"Unknown rule in config: Nope".to_owned())
    );
    assert!(
        config
            .settings
            .is_excluded(&root.join("views/Old/view.json"))
    );

    // An iglint configuration anywhere above wins over rule_config.json.
    write(root, "../iglint.toml", "");
    let config = load(&opts, &REGISTRY).unwrap();
    assert!(matches!(config.source, ConfigSource::Toml(_)));
    std::fs::remove_file(root.join("../iglint.toml")).unwrap();
}

fn variants() -> serde_json::Map<String, serde_json::Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../iglint_linter/tests/fixtures/ign-lint/variants.json");
    let text = std::fs::read_to_string(path).unwrap();
    serde_json::from_str(&text).unwrap()
}

#[test]
fn every_ign_lint_fixture_variant_loads() {
    for (name, variant) in variants() {
        if name.starts_with('_') {
            continue;
        }
        let text = variant.to_string();
        let config = compat::parse_rule_config(Path::new("rule_config.json"), &text).unwrap();
        assert!(
            config.messages.is_empty(),
            "variant {name}: {:?}",
            config.messages
        );
        assert!(config.loaded_rules().count() >= 1, "variant {name}");
        let resolved = compat::to_settings(
            &config,
            &REGISTRY,
            Path::new("/repo"),
            CompatMode::IgnLintOnly,
        );
        // ign-lint runs every rule that is not disabled; their iglint
        // equivalents in the test registry are BIND001, NAME001, NAME002,
        // VIEW001 and, for PylintScriptRule, SCR900 and the default SCR
        // rules.
        let codes: Vec<_> = resolved
            .settings
            .base
            .rules
            .keys()
            .map(|r| r.code())
            .collect();
        assert_eq!(
            codes,
            [
                "BIND001", "NAME001", "NAME002", "SCR101", "SCR102", "SCR900", "VIEW001"
            ],
            "variant {name}"
        );
    }
}

#[test]
fn ign_lint_kwargs_map_to_options() {
    let variants = variants();
    let load_variant = |name: &str| {
        let config =
            compat::parse_rule_config(Path::new("rule_config.json"), &variants[name].to_string())
                .unwrap();
        compat::to_settings(
            &config,
            &REGISTRY,
            Path::new("/repo"),
            CompatMode::IgnLintOnly,
        )
    };
    let poll = load_variant("poll-1000");
    let polling = &poll.settings.base.rules[&rule(&testing::POLLING_INTERVAL)];
    assert_eq!(
        polling.options["minimum-interval"],
        OptionValue::Integer(1000)
    );
    assert_eq!(polling.severity, Severity::Error);

    let naming = load_variant("np-camel-component-error");
    let component = &naming.settings.base.rules[&rule(&testing::COMPONENT_NAME)];
    assert_eq!(component.severity, Severity::Error);
    assert_eq!(
        component.options["convention"],
        OptionValue::String("camelCase".into())
    );
    assert_eq!(component.options["min-length"], OptionValue::Integer(1));
    let property = &naming.settings.base.rules[&rule(&testing::PROPERTY_NAME)];
    assert_eq!(
        property.options["convention"],
        OptionValue::String("camelCase".into())
    );
    // target_node_types and allow_numbers have no equivalent in the test rules.
    assert!(
        naming.notices.iter().any(|n| n
            == "NamePatternRule: kwarg `allow_numbers` has no iglint equivalent and is ignored"),
        "{:?}",
        naming.notices
    );
    // Default severity in ign-lint-only mode is ign-lint's.
    let unused = &naming.settings.base.rules[&rule(&testing::UNUSED_CUSTOM_PROPERTY)];
    assert_eq!(unused.severity, Severity::Error);

    let split = load_variant("np-standard-component-property");
    let component = &split.settings.base.rules[&rule(&testing::COMPONENT_NAME)];
    let OptionValue::Table(per_type) = &component.options["per-type"] else {
        panic!("per-type is a table");
    };
    assert!(per_type.contains_key("property"));
}

#[test]
fn allow_fix_false_makes_rules_unfixable() {
    let config = compat::parse_rule_config(
        Path::new("rule_config.json"),
        r#"{"UnusedCustomPropertiesRule": {"allow_fix": false}}"#,
    )
    .unwrap();
    let resolved =
        compat::to_settings(&config, &REGISTRY, Path::new("/r"), CompatMode::IgnLintOnly);
    assert!(
        resolved
            .settings
            .unfixable
            .contains(&rule(&testing::UNUSED_CUSTOM_PROPERTY))
    );
}

#[test]
fn migration_keeps_comments_and_behaviour() {
    let source = r#"{
  "_comment": "Project lint rules.\nKeep in sync with CI.",
  "NamePatternRule": {
    "_comment": "Components only",
    "kwargs": {"_comment": "PascalCase everywhere", "convention": "PascalCase", "min_length": 3, "target_node_types": ["component"], "severity": "error"}
  },
  "UnusedCustomPropertiesRule": {"enabled": false},
  "PylintScriptRule": {"kwargs": {"debug": true}},
  "PollingIntervalRule": {"kwargs": {"minimum_interval": 5000}, "allow_fix": true},
  "MysteryRule": {}
}"#;
    let config = compat::parse_rule_config(Path::new("/repo/rule_config.json"), source).unwrap();
    let migration = migrate::migrate(&config, Path::new("/repo/rule_config.json"), &REGISTRY);
    let expected = r#"# iglint configuration, migrated from rule_config.json by `iglint migrate`.
#
# Project lint rules.
# Keep in sync with CI.

[rules.NAME001]
# component-name: Component name does not follow the convention, from ign-lint NamePatternRule.
# Components only
# PascalCase everywhere
severity = "error"
convention = "PascalCase"
min-length = 3
# ign-lint kwarg without iglint equivalent: target_node_types = ["component"]

[rules.NAME002]
# property-name: Property name does not follow the convention, from ign-lint NamePatternRule.
# Components only
# PascalCase everywhere
severity = "error"
convention = "PascalCase"
# ign-lint kwarg without iglint equivalent: min_length = 3
# ign-lint kwarg without iglint equivalent: target_node_types = ["component"]

[rules.BIND001]
# polling-interval: now() polls too often, from ign-lint PollingIntervalRule.
minimum-interval = 5000

[rules.SCR900]
# pylint: Message reported by pylint, from ign-lint PylintScriptRule.
severity = "error"
# ign-lint kwarg without iglint equivalent: debug = true

[rules.VIEW001]
# unused-custom-property: Custom property is never referenced, from ign-lint UnusedCustomPropertiesRule.
enabled = false

# MysteryRule: unknown to ign-lint 0.6.4; not migrated.

# ign-lint runs every rule that is not disabled, on `view.json` files only.
[[overrides]]
files = ["**/view.json"]
select = ["BIND001", "NAME002", "SCR900"]
"#;
    assert_eq!(migration.toml, expected);
    assert!(
        migration
            .summary
            .contains(&"UnusedCustomPropertiesRule: disabled (VIEW001)".to_owned())
    );
    assert!(
        migration
            .summary
            .contains(&"PylintScriptRule -> SCR101, SCR102, SCR900".to_owned())
    );
    assert!(
        migration
            .summary
            .contains(&"NamePatternRule -> NAME001, NAME002".to_owned())
    );
}

#[test]
fn migrated_configurations_reproduce_ign_lint_settings() {
    for (name, variant) in variants() {
        if name.starts_with('_') {
            continue;
        }
        let path = Path::new("/repo/rule_config.json");
        let config = compat::parse_rule_config(path, &variant.to_string()).unwrap();
        let migration = migrate::migrate(&config, path, &REGISTRY);
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "iglint.toml", &migration.toml);
        let migrated = load(&options(dir.path()), &REGISTRY)
            .unwrap_or_else(|e| panic!("variant {name}: {e}\n{}", migration.toml));
        let ign_lint = compat::to_settings(&config, &REGISTRY, dir.path(), CompatMode::IgnLintOnly);
        // On views, the files ign-lint lints, the settings are ign-lint's.
        let view = migrated
            .settings
            .for_file(&dir.path().join("projects/p/views/A/view.json"));
        for (rule, settings) in &ign_lint.settings.base.rules {
            assert_eq!(
                view.rules.get(rule),
                Some(settings),
                "variant {name}, rule {rule}\n{}",
                migration.toml
            );
        }
        // Rules iglint adds on top of ign-lint are its own defaults.
        let extra: BTreeMap<_, _> = view
            .rules
            .iter()
            .filter(|(r, _)| !ign_lint.settings.base.rules.contains_key(r))
            .collect();
        assert!(
            extra.keys().all(|r| r.meta().default_enabled),
            "variant {name}"
        );
        // Elsewhere only iglint's defaults run.
        let module = migrated.settings.for_file(
            &dir.path()
                .join("projects/p/ignition/script-python/m/code.py"),
        );
        assert!(
            module.rules.keys().all(|r| r.meta().default_enabled),
            "variant {name}\n{}",
            migration.toml
        );
    }
}

#[test]
fn pylint_script_rule_maps_to_pylint_and_the_native_script_rules() {
    let parse =
        |text: &str| compat::parse_rule_config(Path::new("rule_config.json"), text).unwrap();
    let config = parse(
        r#"{"PerspectiveScriptPylintRule": {"kwargs": {
            "pylintrc": "rc/.pylintrc", "category_mapping": {"E": "warning", "C": "info"},
            "debug": true, "batch_mode": true, "debug_dir": "d", "severity": "warning"}}}"#,
    );
    assert!(config.messages.is_empty(), "{:?}", config.messages);
    let resolved =
        compat::to_settings(&config, &REGISTRY, Path::new("/r"), CompatMode::IgnLintOnly);
    let codes: Vec<&str> = resolved.rule_map["PerspectiveScriptPylintRule"]
        .iter()
        .map(|r| r.code())
        .collect();
    assert_eq!(codes, ["SCR101", "SCR102", "SCR900"]);
    let rules = &resolved.settings.base.rules;
    let pylint = &rules[&rule(&testing::PYLINT)];
    assert_eq!(pylint.severity, Severity::Warning);
    assert_eq!(
        pylint.options["pylintrc"],
        OptionValue::String("rc/.pylintrc".into())
    );
    let OptionValue::Table(mapping) = &pylint.options["category-mapping"] else {
        panic!("category-mapping is a table");
    };
    assert_eq!(mapping["C"], OptionValue::String("info".into()));
    // The native rules take no kwargs; `category_mapping` maps their
    // severity like their pylint counterparts' (`E` for errors).
    let undefined = &rules[&rule(&testing::UNDEFINED_NAME)];
    assert_eq!(undefined.severity, Severity::Warning);
    assert!(undefined.options.is_empty());
    assert_eq!(
        rules[&rule(&testing::UNUSED_VARIABLE)].severity,
        Severity::Warning
    );
    let mut notices = resolved.notices.clone();
    notices.sort();
    assert_eq!(
        notices,
        [
            "PerspectiveScriptPylintRule: kwarg `batch_mode` has no iglint equivalent and is ignored",
            "PerspectiveScriptPylintRule: kwarg `debug_dir` has no iglint equivalent and is ignored",
            "PerspectiveScriptPylintRule: kwarg `debug` has no iglint equivalent and is ignored",
        ]
    );

    // With defaults (ign-lint runs it), `iglint ign-lint` runs SCR900 with
    // ign-lint's rule severity and the native rules with their own.
    let resolved = compat::to_settings(
        &parse("{}"),
        &REGISTRY,
        Path::new("/r"),
        CompatMode::IgnLintOnly,
    );
    let rules = &resolved.settings.base.rules;
    assert_eq!(rules[&rule(&testing::PYLINT)].severity, Severity::Error);
    assert_eq!(
        rules[&rule(&testing::UNDEFINED_NAME)].severity,
        Severity::Error
    );

    // Disabling it in a legacy configuration used by `iglint check` turns
    // off pylint and the native script rules it stands for.
    let disabled = parse(r#"{"PylintScriptRule": {"enabled": false}}"#);
    for mode in [CompatMode::IgnLintOnly, CompatMode::ExtendDefaults] {
        let resolved = compat::to_settings(&disabled, &REGISTRY, Path::new("/r"), mode);
        let codes: Vec<&str> = resolved
            .settings
            .base
            .rules
            .keys()
            .map(|r| r.code())
            .collect();
        assert!(
            codes.iter().all(|c| !c.starts_with("SCR")),
            "{mode:?}: {codes:?}"
        );
    }
    let resolved = compat::to_settings(
        &parse("{}"),
        &REGISTRY,
        Path::new("/r"),
        CompatMode::ExtendDefaults,
    );
    let codes: Vec<&str> = resolved
        .settings
        .for_file(Path::new("/r/views/A/view.json"))
        .rules
        .keys()
        .map(|r| r.code())
        .collect();
    assert!(
        codes.contains(&"SCR900") && codes.contains(&"SCR101"),
        "{codes:?}"
    );
    // pylint runs on views only, like in ign-lint.
    let module = resolved
        .settings
        .for_file(Path::new("/r/ignition/script-python/m/code.py"));
    assert!(module.rules.keys().any(|r| r.code() == "SCR101"));
    assert!(!module.rules.keys().any(|r| r.code() == "SCR900"));

    // The migration disables the native rules explicitly.
    let migration = migrate::migrate(&disabled, Path::new("/repo/rule_config.json"), &REGISTRY);
    assert!(
        migration.toml.contains("[rules.SCR101]\n# undefined-name: Name is not defined, from ign-lint PylintScriptRule.\nenabled = false\n"),
        "{}",
        migration.toml
    );
    assert!(
        migration
            .summary
            .contains(&"PylintScriptRule: disabled (SCR101, SCR102, SCR900)".to_owned())
    );
}
