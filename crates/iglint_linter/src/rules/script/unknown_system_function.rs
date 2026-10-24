//! SCR201 unknown-system-function.

use iglint_python::ast::{Expr, ExprContext};
use iglint_stubs::{Member, system_member, system_members, system_package};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::options::OptionReader;
use crate::registry::{Category, OptionMeta, OptionType, RuleMeta};
use crate::rule::{Hook, ParsedOptions, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::helpers::wildcard_match;
use crate::rules::script::system_api::system_path;
use crate::rules::script::{report, semantic, walk_exprs};
use crate::suggest::{closest, levenshtein};

pub static META: RuleMeta = RuleMeta {
    code: "SCR201",
    name: "unknown-system-function",
    category: Category::Script,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "`system.*` member that does not exist in Ignition 8.3",
    explanation: r#"## What it does

Reports `system.x.y` (and the legacy alias `fpmi.x.y`) where `y` is not a
function, constant, object or package of the package `system.x` in any
scope of Ignition 8.3 (gateway, Vision Client or Designer). Aliases are
followed: after `db = system.db`, `db.runQery(...)` is checked as
`system.db.runQery`. The chain is checked up to the first member that is
not a package: what follows a function, constant or object
(`system.net.httpClient().get`) is not checked. A local variable named
`system` that is not an alias of the scripting API is not checked.

The member list comes from the scripting API of a real 8.3 gateway with
Ignition's standard modules. Third-party modules add their own top-level
packages (for example a MES module's `system.mes`) and sometimes add
functions to Ignition's own packages, so by default a name is only
reported when it is a near miss of a known member of the same package:
the same name in other letter case, one edit away for names of four or
more letters, or two edits away for names of eight or more
(`system.tag.readBlockng`, `system.Tag`, `system.datset`). The message
names the member it probably means. In strict mode every unknown member
of a known package is reported; list the members your modules add in
`allow`.

## Why is this bad?

The script raises `AttributeError` when it reaches the name: usually a
typo or a function that only existed in an older Ignition version.

## Example

```python
def runAction(self, event):
    values = system.tag.readBlockng(["[default]Line/Speed"])
```

Use instead:

```python
def runAction(self, event):
    values = system.tag.readBlocking(["[default]Line/Speed"])
```

## Options

* `allow` (default `[]`): extra `system.*` paths that exist on your
  gateways (from third-party or in-house modules). `*` matches any
  characters, `?` one character, and a path also allows everything below
  it, for example `["system.mes", "system.util.myCompanyHelper"]`.
"#,
    options: &[OptionMeta {
        name: "allow",
        ty: OptionType::StringList,
        default: "[]",
        description: "Extra `system.*` paths (wildcards `*`, `?`) that exist on your gateways.",
    }],
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Script(&UnknownSystemFunction)],
    options: Some(parse_options),
};

struct Options {
    allow: Vec<String>,
}

fn parse_options(r: &OptionReader<'_>) -> Result<ParsedOptions, String> {
    Ok(Box::new(Options {
        allow: r.string_list("allow")?,
    }))
}

impl Options {
    /// Whether `path` or one of its parent paths is allowed.
    fn allows(&self, path: &str) -> bool {
        // Each `i` is the offset of a `.` from `match_indices`.
        let prefixes = path
            .match_indices('.')
            .map(|(i, _)| path.split_at(i).0)
            .chain(std::iter::once(path))
            .collect::<Vec<_>>();
        prefixes
            .iter()
            .any(|p| self.allow.iter().any(|pattern| wildcard_match(pattern, p)))
    }
}

struct UnknownSystemFunction;

impl ScriptRule for UnknownSystemFunction {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        let options: &Options = cx.options();
        let strict = cx.strict();
        let mut found = Vec::new();
        walk_exprs(&unit.parsed.module.body, &mut |e| {
            let Expr::Attribute(a) = e else { return };
            if a.ctx != ExprContext::Load {
                return;
            }
            let Some(path) = system_path(model, e) else {
                return;
            };
            let Some((parent, name)) = path.rsplit_once('.') else {
                return;
            };
            // Dunder attributes (`__name__`, `__dict__`) are Python's.
            if name.starts_with("__") {
                return;
            }
            // Only members of packages are checked: what follows a
            // function, constant or object is not the API's.
            if parent != "system" && system_package(parent).is_none() {
                return;
            }
            if system_member(&path).is_some() || options.allows(&path) {
                return;
            }
            let names: Vec<&str> = system_members(parent)
                .map(|m| member_name(&m))
                .filter(|n| !n.starts_with("__"))
                .collect();
            let suggestion = closest(name, names.iter().copied()).filter(|s| is_near_miss(name, s));
            let what = if parent == "system" {
                "is not a scripting package of Ignition 8.3"
            } else {
                "does not exist in Ignition 8.3"
            };
            let message = match suggestion {
                Some(s) => format!("`{path}` {what}; did you mean `{parent}.{s}`?"),
                None if strict => format!(
                    "`{path}` {what} (if a module on your gateways adds it, list it in `allow`)"
                ),
                None => return,
            };
            found.push((a.range, message));
        });
        for (range, message) in found {
            report(cx, unit, range, message);
        }
    }
}

/// Whether `name` is plausibly a misspelling of `candidate`: the same name
/// in other letter case, one edit away for names of four or more
/// characters, or two edits away for names of eight or more.
fn is_near_miss(name: &str, candidate: &str) -> bool {
    let distance = levenshtein(&name.to_ascii_lowercase(), &candidate.to_ascii_lowercase());
    let len = name.chars().count();
    distance == 0 || (len >= 4 && distance <= 1) || (len >= 8 && distance <= 2)
}

fn member_name(member: &Member) -> &'static str {
    let path = member.path();
    path.rsplit('.').next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn unknown_members_of_known_packages() {
        let d = lint_module(
            "SCR201",
            "def f():\n    return system.tag.readBlockng(['a'])\n",
        );
        assert_eq!(flagged(&d), ["system.tag.readBlockng"]);
        assert_eq!(
            messages(&d),
            [
                "`system.tag.readBlockng` does not exist in Ignition 8.3; did you mean `system.tag.readBlocking`?"
            ]
        );
        // No near miss: possibly added by a module, reported in strict
        // mode only.
        let f = library("m", "x = system.db.frobnicate\n");
        assert!(f.lint(&["SCR201"]).is_empty());
        assert_eq!(
            messages(&f.lint_strict(&["SCR201"])),
            [
                "`system.db.frobnicate` does not exist in Ignition 8.3 (if a module on your gateways adds it, list it in `allow`)"
            ]
        );
    }

    #[test]
    fn known_members_and_what_follows_them() {
        let code = "a = system.tag.readBlocking(['x'])[0].value\nb = system.db.INTEGER\nc = system.net.httpClient().get('u')\nd = system.util.getLogger('x').infof('a')\ne = system.perspective.workstation.exit\nf = system.tag.__name__\ng = system.bacnet.enumerated\n";
        assert!(lint_module("SCR201", code).is_empty());
    }

    #[test]
    fn aliases_and_fpmi() {
        let code = "import system.util\ndb = system.db\nfrom system import tag as t\ndef f():\n    db.runQery('q')\n    t.raed(['a'])\n    fpmi.gui.messageBx('m')\n    system.util.getLoger('x')\n";
        let d = lint_module("SCR201", code);
        assert_eq!(
            flagged(&d),
            [
                "db.runQery",
                "t.raed",
                "fpmi.gui.messageBx",
                "system.util.getLoger"
            ]
        );
        assert!(messages(&d)[2].contains("`system.gui.messageBx`"));
    }

    #[test]
    fn local_system_is_not_the_api() {
        let code = "def f(system):\n    return system.tag.nope\ndef g():\n    system = Thing()\n    return system.db.nope\n";
        assert!(lint_module("SCR201", code).is_empty());
    }

    #[test]
    fn stores_are_not_checked() {
        assert!(lint_module("SCR201", "system.util.myHelper = 1\n").is_empty());
    }

    #[test]
    fn unknown_top_level_packages() {
        // Possibly a third-party module: silent by default.
        assert!(lint_module("SCR201", "x = system.mes.getThing()\n").is_empty());
        // Near misses of known packages are reported.
        let d = lint_module(
            "SCR201",
            "x = system.Tag.readBlocking\ny = system.datset.toDataSet\nz = system.dtabase\nw = system.tga\n",
        );
        assert_eq!(flagged(&d), ["system.Tag", "system.datset"]);
        assert!(messages(&d)[0].ends_with("did you mean `system.tag`?"));
        // Strict mode reports them all.
        let d = library("m", "x = system.mes.getThing()\n").lint_strict(&["SCR201"]);
        assert_eq!(flagged(&d), ["system.mes"]);
    }

    #[test]
    fn allow_option() {
        let f = library(
            "m",
            "a = system.Mes.x\nb = system.util.companyHelper()\nc = system.util.getLoger\nd = system.util.getLogge\n",
        );
        let d = f.lint_with(&[(
            "SCR201",
            r#"allow = ["system.Mes", "system.util.company*", "system.util.getLogge"]"#,
        )]);
        assert_eq!(flagged(&d), ["system.util.getLoger"]);
    }

    #[test]
    fn every_location() {
        assert_eq!(
            flagged_everywhere("SCR201", "\tx \u{3d} system.date.addDay(d, 1)"),
            ["system.date.addDay"]
        );
        let d =
            gateway_message("def handleMessage(payload):\n\tsystem.util.sendMesage('p', 'm')\n")
                .lint(&["SCR201"]);
        assert_eq!(flagged(&d), ["system.util.sendMesage"]);
        let d = session_event("def onStartup(session):\n\tsystem.perspective.navigat('/')\n")
            .lint(&["SCR201"]);
        assert_eq!(flagged(&d), ["system.perspective.navigat"]);
        let d = tag_event("\tsystem.tag.writeBlockin(['a'], [1])").lint(&["SCR201"]);
        assert_eq!(decoded_flagged(&d), ["system.tag.writeBlockin"]);
        let d = webdev("def doGet(request, session):\n\treturn system.util.jsonEncod({})\n")
            .lint(&["SCR201"]);
        assert_eq!(flagged(&d), ["system.util.jsonEncod"]);
    }

    #[test]
    fn json_escaped_scripts_point_at_the_exact_bytes() {
        let f = view(
            ViewScript::Action,
            "\tx \u{3d} '<&>'\n\tsystem.tag.raed(['a'])",
        );
        let d = f.lint(&["SCR201"]);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].range.slice(d[0].file.text()), "system.tag.raed");
    }

    #[test]
    fn unparsable_scripts_are_left_to_syntax_error() {
        assert!(lint_module("SCR201", "x = system.tag.nope(\n").is_empty());
    }
}
