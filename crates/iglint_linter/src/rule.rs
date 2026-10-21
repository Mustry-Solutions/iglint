//! Rule implementations: traits by context, static registration and the
//! per-settings dispatch tables.
//!
//! A rule is a `'static` value implementing one or more context traits
//! ([`ViewRule`], [`ComponentRule`], [`BindingRule`], [`ExpressionRule`],
//! [`ScriptRule`], [`ResourceRule`], [`ProjectRule`]). Its [`RuleImpl`]
//! lists those implementations as [`Hook`]s, says whether it needs the
//! project index ([`Phase`]) and how to parse its options.
//!
//! For every distinct [`LinterSettings`] the driver builds one [`RuleSet`]:
//! the enabled rules with their parsed options, and per context a plain
//! vector of trait objects. Walking a model then costs one virtual call per
//! enabled rule and node; there are no lookups by rule name or code.

use std::any::Any;
use std::sync::Arc;

use iglint_project::ProjectId;
use iglint_project::model::ModelData;
use iglint_project::view::{Binding, Component, View};
use iglint_project::{EmbeddedExpression, EmbeddedScript, FileId};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::options::OptionReader;
use crate::registry::{RuleCode, RuleMeta};
use crate::settings::LinterSettings;

/// Checks a whole resource model (any resource kind). Also the place for
/// file-level checks such as invalid JSON.
pub trait ResourceRule: Sync {
    fn check_resource(&self, cx: &mut Checker<'_, '_>, data: &ModelData<'_>);
}

/// Checks a Perspective view as a whole, once per view.
pub trait ViewRule: Sync {
    fn check_view(&self, cx: &mut Checker<'_, '_>, view: &View<'_>);
}

/// Checks one component of a view.
pub trait ComponentRule: Sync {
    fn check_component(&self, cx: &mut Checker<'_, '_>, view: &View<'_>, component: &Component<'_>);
}

/// Checks one binding (views and session properties). The host model is
/// available through [`Checker::view`] and [`Checker::data`].
pub trait BindingRule: Sync {
    fn check_binding(&self, cx: &mut Checker<'_, '_>, binding: &Binding<'_>);
}

/// Checks one embedded expression (bindings, transforms, struct leaves,
/// query parameters, tag expressions, ...).
pub trait ExpressionRule: Sync {
    fn check_expression(&self, cx: &mut Checker<'_, '_>, expression: &EmbeddedExpression<'_>);
}

/// One script location with everything script rules may use.
///
/// The Python layer will add the parsed module and its semantic model here
/// (fields `module` and `semantic`), parsed once per script and shared by
/// every script rule. Diagnostics reported while a script rule runs are
/// filtered through the script's `# iglint: ignore[...]` comments
/// ([`crate::suppression`]).
pub struct ScriptUnit<'s, 'a> {
    /// The file the script lives in.
    pub file: FileId,
    pub script: &'s EmbeddedScript<'a>,
}

/// Checks one script location (library modules, event files, embedded
/// Perspective and tag scripts).
pub trait ScriptRule: Sync {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>);
}

/// Checks a project as a whole (its `project.json` and its resources),
/// once per linted project. Always runs in [`Phase::Project`].
pub trait ProjectRule: Sync {
    fn check_project(&self, cx: &mut Checker<'_, '_>, project: ProjectId);
}

/// One context a rule runs in.
#[derive(Clone, Copy)]
pub enum Hook {
    Resource(&'static dyn ResourceRule),
    View(&'static dyn ViewRule),
    Component(&'static dyn ComponentRule),
    Binding(&'static dyn BindingRule),
    Expression(&'static dyn ExpressionRule),
    Script(&'static dyn ScriptRule),
    Project(&'static dyn ProjectRule),
}

/// When a rule runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Per resource, in parallel with loading; the rule only looks at the
    /// resource itself. [`Checker::index`] is `None`.
    Local,
    /// After the project index is built; [`Checker::index`] is available.
    Project,
}

/// Parsed, validated options of one rule.
pub type ParsedOptions = Box<dyn Any + Send + Sync>;

/// Parses a rule's options.
pub type OptionsParser = fn(&OptionReader<'_>) -> Result<ParsedOptions, String>;

/// The implementation of one rule.
pub struct RuleImpl {
    pub meta: &'static RuleMeta,
    pub phase: Phase,
    pub hooks: &'static [Hook],
    /// Parses the options; `None` for rules without options.
    pub options: Option<OptionsParser>,
}

impl RuleImpl {
    pub const fn code(&self) -> RuleCode {
        RuleCode::new(self.meta)
    }
}

/// An enabled rule, ready to run.
pub struct ActiveRule {
    pub rule: RuleCode,
    pub severity: Severity,
    pub options: Option<ParsedOptions>,
}

/// Index of an [`ActiveRule`] in its [`RuleSet`].
pub(crate) type Slot = usize;

/// The hooks of one phase.
#[derive(Default)]
pub(crate) struct PhaseHooks {
    pub resource: Vec<(Slot, &'static dyn ResourceRule)>,
    pub view: Vec<(Slot, &'static dyn ViewRule)>,
    pub component: Vec<(Slot, &'static dyn ComponentRule)>,
    pub binding: Vec<(Slot, &'static dyn BindingRule)>,
    pub expression: Vec<(Slot, &'static dyn ExpressionRule)>,
    pub script: Vec<(Slot, &'static dyn ScriptRule)>,
    pub project: Vec<(Slot, &'static dyn ProjectRule)>,
}

impl PhaseHooks {
    pub fn is_empty(&self) -> bool {
        self.resource.is_empty()
            && self.view.is_empty()
            && self.component.is_empty()
            && self.binding.is_empty()
            && self.expression.is_empty()
            && self.script.is_empty()
            && self.project.is_empty()
    }

    /// Whether any hook walks view internals.
    pub fn walks_views(&self) -> bool {
        !(self.view.is_empty() && self.component.is_empty() && self.binding.is_empty())
    }
}

/// The enabled rules for one settings object.
pub struct RuleSet {
    pub(crate) active: Vec<ActiveRule>,
    pub(crate) local: PhaseHooks,
    pub(crate) project: PhaseHooks,
    /// Option errors, as `(rule, message)`; such a rule runs with defaults.
    pub errors: Vec<(RuleCode, String)>,
}

impl RuleSet {
    /// Builds the dispatch tables for `settings` from `rules`.
    pub fn build(rules: &[&'static RuleImpl], settings: &LinterSettings) -> RuleSet {
        let mut set = RuleSet {
            active: Vec::new(),
            local: PhaseHooks::default(),
            project: PhaseHooks::default(),
            errors: Vec::new(),
        };
        for imp in rules {
            let code = imp.code();
            let Some(rule_settings) = settings.rule(code) else {
                continue;
            };
            let options = match imp.options {
                None => None,
                Some(parse) => {
                    let reader = OptionReader::new(imp.meta, &rule_settings.options);
                    match parse(&reader) {
                        Ok(o) => Some(o),
                        Err(message) => {
                            set.errors.push((code, message));
                            let empty = Default::default();
                            match parse(&OptionReader::new(imp.meta, &empty)) {
                                Ok(o) => Some(o),
                                Err(_) => continue,
                            }
                        }
                    }
                }
            };
            let slot = set.active.len();
            set.active.push(ActiveRule {
                rule: code,
                severity: rule_settings.severity,
                options,
            });
            for hook in imp.hooks {
                if let Hook::Project(r) = *hook {
                    // Project rules always need the index.
                    set.project.project.push((slot, r));
                    continue;
                }
                let hooks = match imp.phase {
                    Phase::Local => &mut set.local,
                    Phase::Project => &mut set.project,
                };
                match *hook {
                    Hook::Resource(r) => hooks.resource.push((slot, r)),
                    Hook::View(r) => hooks.view.push((slot, r)),
                    Hook::Component(r) => hooks.component.push((slot, r)),
                    Hook::Binding(r) => hooks.binding.push((slot, r)),
                    Hook::Expression(r) => hooks.expression.push((slot, r)),
                    Hook::Script(r) => hooks.script.push((slot, r)),
                    Hook::Project(_) => {}
                }
            }
        }
        set
    }

    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }

    /// Whether `rule` is enabled in this set.
    pub fn contains(&self, rule: RuleCode) -> bool {
        self.active.iter().any(|a| a.rule == rule)
    }
}

/// Caches rule sets by settings identity: files without overrides share
/// one settings `Arc`, so a run usually builds one or two sets.
#[derive(Default)]
pub(crate) struct RuleSetCache {
    sets: std::sync::Mutex<Vec<(Arc<LinterSettings>, Arc<RuleSet>)>>,
}

impl RuleSetCache {
    pub fn get(&self, rules: &[&'static RuleImpl], settings: &Arc<LinterSettings>) -> Arc<RuleSet> {
        let mut sets = self.sets.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, set)) = sets
            .iter()
            .find(|(s, _)| Arc::ptr_eq(s, settings) || **s == **settings)
        {
            return Arc::clone(set);
        }
        let set = Arc::new(RuleSet::build(rules, settings));
        sets.push((Arc::clone(settings), Arc::clone(&set)));
        set
    }

    /// Every distinct option error, for reporting once per run.
    pub fn errors(&self) -> Vec<(RuleCode, String)> {
        let sets = self.sets.lock().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<(RuleCode, String)> = Vec::new();
        for (_, set) in sets.iter() {
            for e in &set.errors {
                if !out.contains(e) {
                    out.push(e.clone());
                }
            }
        }
        out
    }
}
