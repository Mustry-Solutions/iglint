//! REF002 unknown-library-member.

use iglint_project::library::LibraryLookup;
use iglint_project::script::IMPLICIT_GLOBALS;
use iglint_project::{ProjectId, ProjectIndex};

use super::library_uses::{LibraryUse, library_uses};
use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};

pub static META: RuleMeta = RuleMeta {
    code: "REF002",
    name: "unknown-library-member",
    category: Category::Ref,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "Library module or function does not exist",
    explanation: r#"## What it does

Resolves every use of the project script library in scripts against the
library the project sees (its own modules and inherited ones):
attribute chains such as `core.util.fmt(...)`, chains through aliases
(`u = core.util`, `from core.util import fmt`) and `import` statements.
Paths through another library module's import continue in the module
the import leads to: `m0.util.nope` is checked in `lib.util` when `m0`
does `from lib import util` or `import lib.util as util`, and so is
`m0.lib.util.nope` when `m0` does `import lib.util` (which binds `lib`),
at any depth (measured with Ignition's ScriptManager on 8.3.3). It
reports:

* a package without the module (`core.nope` where `core/` has no `nope`);
* a module without the attribute (`core.util.nope` where `core/util/code.py`
  defines no top-level `nope`);
* a child module hidden by its parent's own `code.py`: a folder with a
  `code.py` is a module, and Ignition cannot reach the modules below it
  (see `SCR304`).

A module's top-level names are those its module-level code binds,
including names bound through `global` in functions, names it imports,
and the globals every script sees (`system`, `fpmi`, `sys` and the
library's top-level packages).

The rule stays silent where it cannot be sure: modules whose namespace is
dynamic (`from x import *`, `exec`, writes through `globals()`, code that
reads or writes a frame's or a module's names by itself such as
`sys._getframe()`, `.f_globals` or `e = eval`; reflection that is only a
first hop, such as a `.type` Java member or `getattr` with a computed
name, does not count, see `docs/internal/fix-safety.md`, "Frame routes:
findings and fixes"), modules
that do not parse, paths through a name the module binds otherwise too
(`try: from lib import util` / `except: util = None`), through a star
import or around a cycle of imports, uses inside a `try` that catches
`AttributeError`,
`ImportError`, `NameError` or `Exception`, dunder attributes, the members
of Ignition's library module and package objects that the module does
not bind (`core.util.dict`, `core.util.getName()`, `core.clear()`; see
`docs/internal/fix-safety.md`, "Library module objects"), scripts of
projects whose parent project is not in the workspace, and names that a
project inheriting from the script's project defines, and names a
resource of the project stores, through any path (`core.util.flag =
True` counts for `m0.util.flag` when `m0` imports `core.util` as `util`,
and `m0.util.flag = True` for `core.util.flag`). Attribute stores
themselves are not checked.

## Why is this bad?

The script raises `AttributeError` (or `ImportError`) when it reaches the
name. A missing library name is almost always a renamed or moved function
or module.

## Example

```python
core.util.formatUnits(value)
```

Use instead (the function is called `formatUnit`):

```python
core.util.formatUnit(value)
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Script(&UnknownLibraryMember)],
    options: None,
};

struct UnknownLibraryMember;

/// Whether `lookup` names something that does not exist.
pub(crate) fn is_miss(lookup: &LibraryLookup<'_>) -> bool {
    matches!(
        lookup,
        LibraryLookup::Missing { .. }
            | LibraryLookup::MissingAttribute { .. }
            | LibraryLookup::HiddenByModule { .. }
    )
}

/// Resolves `dotted` in `project`; a miss becomes `Unknown` when a project
/// inheriting from it resolves the name (its scripts may run there), and
/// any result becomes `Unknown` when a script of the project or of a
/// project inheriting from it stores the name
/// ([`ProjectIndex::library_name_stored`]): the definition then says
/// nothing about what runs. Callers that must also exclude rebinding by
/// code they cannot see check [`ProjectIndex::library_name_rebound`].
pub(crate) fn resolve<'i>(
    index: &'i ProjectIndex<'_>,
    project: ProjectId,
    dotted: &str,
) -> LibraryLookup<'i> {
    resolve_with(index, project, dotted, false)
}

/// [`resolve`] for findings: modules whose only escape routes do not
/// observe names are not dynamic
/// ([`ProjectIndex::resolve_library_for_findings`]).
pub(crate) fn resolve_for_findings<'i>(
    index: &'i ProjectIndex<'_>,
    project: ProjectId,
    dotted: &str,
) -> LibraryLookup<'i> {
    resolve_with(index, project, dotted, true)
}

fn resolve_with<'i>(
    index: &'i ProjectIndex<'_>,
    project: ProjectId,
    dotted: &str,
    findings: bool,
) -> LibraryLookup<'i> {
    let lookup_in = |p: ProjectId| {
        if findings {
            index.resolve_library_for_findings(p, dotted)
        } else {
            index.resolve_library(p, dotted)
        }
    };
    let stored = std::iter::once(project)
        .chain(index.workspace().project_descendants(project))
        .any(|p| index.library_name_stored(p, dotted));
    if stored {
        return LibraryLookup::Unknown;
    }
    let lookup = lookup_in(project);
    if is_miss(&lookup) && index.workspace().project(project).inheritable() {
        let found_below = index
            .workspace()
            .project_descendants(project)
            .into_iter()
            .any(|d| !is_miss(&lookup_in(d)));
        if found_below {
            return LibraryLookup::Unknown;
        }
    }
    lookup
}

impl ScriptRule for UnknownLibraryMember {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(uses) = library_uses(cx, unit) else {
            return;
        };
        let Some((index, project)) = cx.project_index() else {
            return;
        };
        let mut reported = Vec::new();
        for u in uses {
            if u.guarded {
                continue;
            }
            let Some((at, message)) = problem(index, project, &u) else {
                continue;
            };
            if u.aliased > 0 && at <= u.aliased {
                // Reported where the alias or import names it.
                continue;
            }
            let range = u.segments[at];
            if reported.contains(&range) {
                continue;
            }
            reported.push(range);
            crate::rules::script::report(cx, unit, range, message);
        }
    }
}

/// The failing segment and the message, if `u` names nothing.
fn problem(
    index: &ProjectIndex<'_>,
    project: ProjectId,
    u: &LibraryUse<'_>,
) -> Option<(usize, String)> {
    let segments: Vec<&str> = u.dotted.split('.').collect();
    let prefix = |i: usize| segments[..=i].join(".");
    match resolve_for_findings(index, project, &u.dotted) {
        LibraryLookup::Missing { index: i, parent } => Some((
            i,
            format!(
                "`{}` does not exist: package `{parent}` has no module `{}`",
                prefix(i),
                segments[i]
            ),
        )),
        LibraryLookup::HiddenByModule { index: i, module } => Some((
            i,
            format!(
                "`{}` cannot be reached: module `{module}` has its own code.py, which hides the modules below it",
                prefix(i)
            ),
        )),
        LibraryLookup::MissingAttribute { index: i, .. } => {
            let name = segments[i];
            if (name.starts_with("__") && name.ends_with("__"))
                || IMPLICIT_GLOBALS.contains(&name)
                || index.library(project).is_global(name)
            {
                return None;
            }
            Some((
                i,
                format!(
                    "Module `{}` has no attribute `{name}`",
                    segments[..i].join(".")
                ),
            ))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::{
        ViewScript, decoded_flagged, flagged, library, messages, view, with_library,
    };
    use crate::rules::test_support::Fixture;

    const UTIL: &str = "import json\nLIMIT = 3\ndef fmt(value, digits=2):\n    return value\ndef _setup():\n    global cache\n    cache = {}\nclass Unit(object):\n    pass\n";

    fn project(f: Fixture) -> Fixture {
        let f = with_library(f, "core/util", UTIL);
        let f = with_library(f, "core/legacy", "X = 1\n");
        let f = with_library(f, "core/legacy/inner", "Y = 1\n");
        with_library(f, "dyn/star", "from java.util import *\n")
    }

    #[test]
    fn missing_modules_attributes_and_hidden_modules() {
        let f = project(library(
            "app/main",
            "import core.util\nfrom core.util import fmt, nope\nimport core.gone\ndef go():\n    core.util.fmt(1)\n    core.util.fmtt(1)\n    core.util.json.dumps(1)\n    core.util.cache\n    core.util.system\n    core.util.__name__\n    core.missing.f()\n    core.legacy.inner.Y\n    u = core.util\n    u.LIMITS\n    dyn.star.anything\n    core.util.LIMIT.real\n    core.util.Unit()\n    core.util.flag = 1\n",
        ));
        let d = f.lint(&["REF002"]);
        assert_eq!(
            flagged(&d),
            vec!["nope", "core.gone", "fmtt", "missing", "inner", "LIMITS"]
        );
        assert_eq!(
            messages(&d),
            vec![
                "Module `core.util` has no attribute `nope`",
                "`core.gone` does not exist: package `core` has no module `gone`",
                "Module `core.util` has no attribute `fmtt`",
                "`core.missing` does not exist: package `core` has no module `missing`",
                "`core.legacy.inner` cannot be reached: module `core.legacy` has its own code.py, which hides the modules below it",
                "Module `core.util` has no attribute `LIMITS`",
            ]
        );
    }

    /// A name another script creates (`core.util.extra = ...`) exists once
    /// that script ran.
    #[test]
    fn stored_names_are_not_missing() {
        for store in [
            "core.util.extra = 1\n",
            "setattr(core.util, 'extra', 1)\n",
            "core.util.__dict__['extra'] = 1\n",
            "exec 'core.util.extra = 1'\n",
        ] {
            let f = project(library(
                "app/main",
                "def go():\n    return core.util.extra\n",
            ));
            let f = with_library(f, "app/setup", store);
            assert!(f.lint(&["REF002"]).is_empty(), "{store}");
        }
        let f = project(library(
            "app/main",
            "def go():\n    return core.util.extra\n",
        ));
        let f = with_library(f, "app/setup", "core.util.other = 1\n");
        assert_eq!(flagged(&f.lint(&["REF002"])), vec!["extra"]);
    }

    /// Review round 29 (M1): a store through another library module's
    /// import lands in the import's target, and a store of the target
    /// counts for the paths through the import (`m0.util.h = f` and then
    /// `lib.util.h()` returns what was stored on 8.3.3, `chains.cases`).
    #[test]
    fn stores_through_library_imports_are_not_missing() {
        for (store, read) in [
            ("m0.util.h = lambda: 1", "core.util.h()"),
            ("m0.m1.util.h = 1", "core.util.h"),
            ("core.util.h = 1", "m0.util.h"),
            ("core.util.h = 1", "m0.m1.util.h"),
            ("m0.util.h = 1", "m0.m1.util.h"),
            ("setattr(m0.util, 'h', 1)", "core.util.h"),
            ("import m0.util as u\n    u.h = 1", "core.util.h"),
        ] {
            let f = project(library(
                "app/main",
                &format!("def go():\n    {store}\n    return {read}\n"),
            ));
            let f = with_library(f, "m0", "from core import util\nimport m1\n");
            let f = with_library(f, "m1", "import core.util as util\n");
            assert!(f.lint(&["REF002"]).is_empty(), "{store}; {read}");
        }
        // A store into another module of the chain is not one of `util`.
        let f = project(library(
            "app/main",
            "def go():\n    m0.h = 1\n    return core.util.h\n",
        ));
        let f = with_library(f, "m0", "from core import util\n");
        assert_eq!(flagged(&f.lint(&["REF002"])), vec!["h"]);
    }

    /// Review round 28: a path through another library module's import
    /// is checked in the import's target (`m0.util.nothere` when `m0` does
    /// `from lib import util` raises AttributeError on 8.3.3), however
    /// long the chain; through a name bound otherwise too, or a cycle,
    /// nothing is reported.
    #[test]
    fn names_reached_through_library_imports() {
        let lib = |f| with_library(f, "lib/util", "import json\ndef f():\n    return 1\n");
        let mut chain = Fixture::new();
        for i in 0..10 {
            chain = with_library(
                chain,
                &format!("c{i}"),
                &format!("import c{} as a\n", i + 1),
            );
        }
        let chain = with_library(chain, "c10", "from lib import util\n");
        for (f, path) in [
            (
                with_library(Fixture::new(), "m0", "from lib import util\n"),
                "m0.util",
            ),
            (
                with_library(Fixture::new(), "m0", "import lib.util as util\n"),
                "m0.util",
            ),
            (
                with_library(Fixture::new(), "m0", "import lib.util\n"),
                "m0.lib.util",
            ),
            (chain, "c0.a.a.a.a.a.a.a.a.a.a.util"),
        ] {
            let f = with_library(
                lib(f),
                "app/main",
                &format!(
                    "def go():\n    {path}.f()\n    {path}.json.dumps(1)\n    {path}.nothere()\n"
                ),
            );
            let d = f.lint(&["REF002"]);
            assert_eq!(flagged(&d), vec!["nothere"], "{path}");
            assert_eq!(
                messages(&d),
                vec![format!("Module `{path}` has no attribute `nothere`")]
            );
        }
        for (m0, m1) in [
            (
                "try:\n    from lib import util\nexcept ImportError:\n    util = None\n",
                "",
            ),
            ("from m1 import *\n", "from lib import util\n"),
            ("from m1 import util\n", "from m0 import util\n"),
        ] {
            let f = with_library(with_library(lib(Fixture::new()), "m0", m0), "m1", m1);
            let f = with_library(f, "app/main", "def go():\n    m0.util.nothere()\n");
            let d = f.lint(&["REF002"]);
            assert!(!flagged(&d).contains(&"nothere".to_owned()), "{m0}");
        }
    }

    #[test]
    fn aliases_are_reported_once_where_they_are_made() {
        let f = project(library(
            "app/main",
            "from core.util import gone\ndef go():\n    gone()\n    gone.x\n",
        ));
        assert_eq!(flagged(&f.lint(&["REF002"])), vec!["gone"]);
    }

    #[test]
    fn guarded_shadowed_and_view_scripts() {
        let f = project(library(
            "app/main",
            "try:\n    core.util.maybe()\nexcept AttributeError:\n    pass\ndef go(core):\n    core.util.whatever\n",
        ));
        assert!(f.lint(&["REF002"]).is_empty());
        let f = project(view(
            ViewScript::Action,
            "\tcore.util.fmt(self.props.text)\n\tcore.utill.fmt(1)\n\tsystem.tag.readBlocking([])",
        ));
        let d = f.lint(&["REF002"]);
        assert_eq!(decoded_flagged(&d), vec!["utill"]);
    }

    #[test]
    fn inherited_and_descendant_libraries() {
        let f = Fixture::new()
            .file(
                "projects/base/project.json",
                r#"{"title": "base", "enabled": true, "inheritable": true}"#,
            )
            .resource(
                "projects/base/ignition/script-python/core/base",
                &[(
                    "code.py",
                    "def go():\n    core.child.only()\n    core.nowhere.f()\n",
                )],
            )
            .file(
                "projects/p/project.json",
                r#"{"title": "p", "parent": "base", "enabled": true, "inheritable": false}"#,
            )
            .resource(
                "projects/p/ignition/script-python/core/child",
                &[("code.py", "def only():\n    core.base.go()\n")],
            );
        assert_eq!(flagged(&f.lint(&["REF002"])), vec!["nowhere"]);
    }
}
