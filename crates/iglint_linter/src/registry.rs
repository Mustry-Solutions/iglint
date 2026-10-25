//! Rule metadata and lookup.
//!
//! Every rule is described by a `'static` [`RuleMeta`]. The full list lives in
//! [`crate::rules::ALL`]; lookups by code or name go through [`Registry`].

use std::fmt;

use crate::diagnostic::Severity;

/// Address of the documentation page of a rule (the generated
/// `docs/rules/<CODE>.md`); `{code}` stands for the rule code.
pub const RULE_DOCS_URL: &str =
    "https://github.com/Mustry-Solutions/iglint/blob/main/docs/rules/{code}.md";

/// Groups rules for selection (`select = ["SCR"]`) and documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    /// View structure and component tree.
    View,
    /// Naming conventions.
    Name,
    /// Bindings, transforms and expressions.
    Bind,
    /// Script correctness.
    Script,
    /// References between resources.
    Ref,
    /// Performance.
    Perf,
    /// Security.
    Sec,
    /// Accessibility.
    A11y,
    /// Style classes and inline styles.
    Style,
    /// Tags and gateway resources.
    Tag,
}

impl Category {
    pub const ALL: [Category; 10] = [
        Category::View,
        Category::Name,
        Category::Bind,
        Category::Script,
        Category::Ref,
        Category::Perf,
        Category::Sec,
        Category::A11y,
        Category::Style,
        Category::Tag,
    ];

    /// The code prefix, e.g. `SCR` for `SCR101`.
    pub const fn prefix(self) -> &'static str {
        match self {
            Category::View => "VIEW",
            Category::Name => "NAME",
            Category::Bind => "BIND",
            Category::Script => "SCR",
            Category::Ref => "REF",
            Category::Perf => "PERF",
            Category::Sec => "SEC",
            Category::A11y => "A11Y",
            Category::Style => "STYLE",
            Category::Tag => "TAG",
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Category::View => "View structure",
            Category::Name => "Naming",
            Category::Bind => "Bindings and expressions",
            Category::Script => "Scripts",
            Category::Ref => "References",
            Category::Perf => "Performance",
            Category::Sec => "Security",
            Category::A11y => "Accessibility",
            Category::Style => "Styles",
            Category::Tag => "Tags and gateway",
        }
    }

    pub fn from_prefix(prefix: &str) -> Option<Category> {
        Category::ALL
            .into_iter()
            .find(|c| c.prefix().eq_ignore_ascii_case(prefix))
    }
}

/// Whether a rule can produce fixes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixAvailability {
    None,
    Sometimes,
    Always,
}

/// How safe a rule's fixes are, as its documentation says. Every safe fix
/// is also unsafe while code in the workspace may read the files a fix
/// edits (`docs/internal/fix-safety.md`, "File reads").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixSafety {
    /// The rule has no fix.
    None,
    /// Safe.
    Safe,
    /// Always unsafe.
    Unsafe,
    /// Safe or unsafe per finding, by analysis the rule documents (whether
    /// anything can observe what the fix changes).
    Depends,
}

/// The type of a rule option, used to validate configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionType {
    Bool,
    Integer,
    Float,
    String,
    StringList,
    /// A table whose shape the rule validates itself.
    Table,
}

/// Describes one configurable option of a rule.
#[derive(Clone, Copy, Debug)]
pub struct OptionMeta {
    pub name: &'static str,
    pub ty: OptionType,
    /// Default rendered as TOML, for documentation.
    pub default: &'static str,
    pub description: &'static str,
}

/// Static description of a rule.
#[derive(Debug)]
pub struct RuleMeta {
    /// Stable code, e.g. `SCR101`.
    pub code: &'static str,
    /// Stable kebab-case name, e.g. `undefined-name`.
    pub name: &'static str,
    pub category: Category,
    pub default_severity: Severity,
    /// Whether the rule runs when the user has not selected rules explicitly.
    pub default_enabled: bool,
    pub fix: FixAvailability,
    /// How safe the fixes are ([`FixSafety::None`] when there is none).
    pub fix_safety: FixSafety,
    /// One line, shown in rule lists.
    pub summary: &'static str,
    /// Markdown: what it does, why, examples, configuration.
    pub explanation: &'static str,
    pub options: &'static [OptionMeta],
    /// Names under which ign-lint configures the equivalent rule, if any.
    pub ign_lint_names: &'static [&'static str],
    /// How ign-lint `kwargs` of the rules in `ign_lint_names` map to this
    /// rule's options: `(ign-lint kwarg, iglint option)`. A kwarg named
    /// `severity` maps to the rule severity unless it is listed here. Empty
    /// for rules without ign-lint options.
    pub ign_lint_option_map: &'static [(&'static str, &'static str)],
    /// String-list options that an ign-lint configuration implies for this
    /// rule, whatever its kwargs say: `(option, values)`. Set when the rule
    /// is configured from a `rule_config.json` and the option is not set by
    /// a kwarg.
    pub ign_lint_implied_options: &'static [(&'static str, &'static [&'static str])],
}

impl RuleMeta {
    /// Neutral values for every field, to fill in the fields a rule does not
    /// care about: `RuleMeta { code: "SCR101", ..RuleMeta::DEFAULT }`.
    pub const DEFAULT: RuleMeta = RuleMeta {
        code: "",
        name: "",
        category: Category::View,
        default_severity: Severity::Warning,
        default_enabled: false,
        fix: FixAvailability::None,
        fix_safety: FixSafety::None,
        summary: "",
        explanation: "",
        options: &[],
        ign_lint_names: &[],
        ign_lint_option_map: &[],
        ign_lint_implied_options: &[],
    };

    pub fn option(&self, name: &str) -> Option<&'static OptionMeta> {
        self.options.iter().find(|o| o.name == name)
    }

    /// [`RuleMeta::explanation`] with the default of every option named at
    /// the start of a bullet of its `## Options` section, from
    /// [`OptionMeta::default`]: `` * `max-depth`: ... `` becomes
    /// `` * `max-depth` (default `12`): ... ``. A bullet that states the
    /// default itself (`` * `max-depth` (default `12`): ``) is kept as it
    /// is; a test checks that it states the metadata's.
    pub fn documented_explanation(&self) -> String {
        let mut out = String::with_capacity(self.explanation.len() + 64);
        let mut in_options = false;
        for line in self.explanation.split_inclusive('\n') {
            let bare = line.trim_end_matches(['\n', '\r']);
            if bare.starts_with("## ") {
                in_options = bare == "## Options";
            }
            match self.option_bullet(line).filter(|_| in_options) {
                Some((names, rest)) if !rest.starts_with(" (") => {
                    out.push_str("* ");
                    for (k, name) in names.iter().enumerate() {
                        if k > 0 {
                            out.push_str(", ");
                        }
                        out.push('`');
                        out.push_str(name);
                        out.push('`');
                        if let Some(option) = self.option(name) {
                            match option.default.trim() {
                                "" | "none" => out.push_str(" (unset by default)"),
                                default => {
                                    out.push_str(" (default `");
                                    out.push_str(default);
                                    out.push_str("`)");
                                }
                            }
                        }
                    }
                    out.push_str(rest);
                }
                _ => out.push_str(line),
            }
        }
        out
    }

    /// The option names a bullet starts with (`` * `a`, `b`: ... ``) and
    /// the rest of the line.
    pub fn option_bullet<'l>(&self, line: &'l str) -> Option<(Vec<&'l str>, &'l str)> {
        let mut rest = line.strip_prefix("* ")?;
        let mut names = Vec::new();
        while let Some((name, after)) = rest.strip_prefix('`').and_then(|r| r.split_once('`')) {
            names.push(name);
            rest = after;
            match rest.strip_prefix(", ") {
                Some(r) if r.starts_with('`') => rest = r,
                _ => break,
            }
        }
        (!names.is_empty()).then_some((names, rest))
    }

    /// The iglint option an ign-lint kwarg maps to, if any.
    pub fn option_for_ign_lint_kwarg(&self, kwarg: &str) -> Option<&'static str> {
        self.ign_lint_option_map
            .iter()
            .find(|(from, _)| *from == kwarg)
            .map(|(_, to)| *to)
    }
}

/// A cheap handle to a rule's metadata. Equality is by code.
#[derive(Clone, Copy)]
pub struct RuleCode(&'static RuleMeta);

impl RuleCode {
    pub const fn new(meta: &'static RuleMeta) -> Self {
        RuleCode(meta)
    }

    pub const fn meta(self) -> &'static RuleMeta {
        self.0
    }

    pub const fn code(self) -> &'static str {
        self.0.code
    }

    pub const fn name(self) -> &'static str {
        self.0.name
    }

    /// The address of the rule's documentation page ([`RULE_DOCS_URL`]).
    pub fn docs_url(self) -> String {
        RULE_DOCS_URL.replace("{code}", self.0.code)
    }
}

impl PartialEq for RuleCode {
    fn eq(&self, other: &Self) -> bool {
        self.0.code == other.0.code
    }
}

impl Eq for RuleCode {}

impl std::hash::Hash for RuleCode {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.code.hash(state);
    }
}

impl PartialOrd for RuleCode {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RuleCode {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.code.cmp(other.0.code)
    }
}

impl fmt::Debug for RuleCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.code)
    }
}

impl fmt::Display for RuleCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.code)
    }
}

/// What a selector in configuration refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selector {
    All,
    Category(Category),
    /// A code prefix such as `SCR1`.
    Prefix(&'static str),
    Rule(RuleCode),
}

/// Lookup over a fixed list of rules.
pub struct Registry {
    rules: &'static [&'static RuleMeta],
}

impl Registry {
    pub const fn new(rules: &'static [&'static RuleMeta]) -> Self {
        Registry { rules }
    }

    pub fn iter(&self) -> impl Iterator<Item = RuleCode> + '_ {
        self.rules.iter().map(|meta| RuleCode::new(meta))
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Rules that ign-lint configures under `name` (a rule class name).
    pub fn find_ign_lint<'a>(&'a self, name: &'a str) -> impl Iterator<Item = RuleCode> + 'a {
        self.rules
            .iter()
            .filter(move |m| m.ign_lint_names.contains(&name))
            .map(|meta| RuleCode::new(meta))
    }

    /// The closest rule code or name to a misspelled `key`, for "did you
    /// mean" hints.
    pub fn suggest(&self, key: &str) -> Option<&'static str> {
        let candidates = self
            .rules
            .iter()
            .flat_map(|m| [m.code, m.name])
            .chain(Category::ALL.iter().map(|c| c.prefix()));
        crate::suggest::closest(key, candidates)
    }

    /// Finds a rule by code (case-insensitive), kebab-case name, or ign-lint
    /// rule class name.
    pub fn find(&self, key: &str) -> Option<RuleCode> {
        self.rules
            .iter()
            .find(|m| {
                m.code.eq_ignore_ascii_case(key) || m.name == key || m.ign_lint_names.contains(&key)
            })
            .map(|meta| RuleCode::new(meta))
    }

    /// Parses a selector: `ALL`, a category prefix (`SCR`), a code prefix
    /// (`SCR1`), a full code (`SCR101`) or a rule name (`undefined-name`).
    pub fn parse_selector(&self, key: &str) -> Option<Selector> {
        if key.eq_ignore_ascii_case("ALL") {
            return Some(Selector::All);
        }
        if let Some(rule) = self.find(key) {
            return Some(Selector::Rule(rule));
        }
        if let Some(category) = Category::from_prefix(key) {
            return Some(Selector::Category(category));
        }
        // A code prefix must match at least one rule and start with a category.
        let upper = key.to_ascii_uppercase();
        let matched = self.rules.iter().find(|m| m.code.starts_with(&upper))?;
        let prefix = matched.code.get(..upper.len())?;
        Category::ALL
            .into_iter()
            .any(|c| prefix.starts_with(c.prefix()))
            .then_some(Selector::Prefix(prefix))
    }

    pub fn matches(selector: Selector, rule: RuleCode) -> bool {
        match selector {
            Selector::All => true,
            Selector::Category(category) => rule.meta().category == category,
            Selector::Prefix(prefix) => rule.code().starts_with(prefix),
            Selector::Rule(other) => other == rule,
        }
    }
}
