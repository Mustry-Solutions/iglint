//! SCR303 module-level-side-effect.

use iglint_python::ast::{CmpOp, Expr, Stmt};
use iglint_python::visitor::{Visitor, walk_expr, walk_stmt};
use iglint_python_semantic::SemanticModel;
use iglint_text::TextRange;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::sleep_in_event_script::SLEEPS;
use crate::rules::script::system_api::system_path;
use crate::rules::script::{is_builtin_reference, report, semantic};
use iglint_project::ScriptKind;
use iglint_project::model::ModelData;

pub static META: RuleMeta = RuleMeta {
    code: "SCR303",
    name: "module-level-side-effect",
    category: Category::Script,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "Library module calls `system.*` or does I/O when it is imported",
    explanation: r#"## What it does

Reports calls in a project library module that run when the module is
imported: at module level and in class bodies, including decorators and
default argument values, but not inside function bodies, lambdas or an
`if __name__ == '__main__':` block. Reported calls are `system.*`
functions (also through aliases and `fpmi`) and the I/O builtins `open`,
`file` and `execfile`, and `time.sleep` / `java.lang.Thread.sleep`.

These `system.*` calls are considered harmless and are not reported:
`system.util.getLogger`, `system.util.getGlobals` (the gateway-wide
dictionary modules use to keep state across reloads),
`system.util.getProjectName`,
`system.util.jsonEncode`, `system.util.jsonDecode`, every `system.date.*`
and `system.math.*` function, and the `system.dataset.*` functions except
the `export*` ones (they build or convert datasets without side effects).
Creating objects such as `system.net.httpClient()` is not reported either:
only functions are.

## Why is this bad?

Ignition imports a library module lazily, the first time any script of
any scope touches it, and again after every project save. Module-level
code then runs at an unpredictable moment, in whichever scope imported it
(gateway, Perspective session, Vision Client, Designer), possibly where
the function does not exist or with the wrong permissions; a tag read or
query that fails there makes the whole module fail to import. A sleep
blocks the importing script.

## Example

```python
setpoints = system.tag.readBlocking(["[default]Line/Setpoint"])

def target():
    return setpoints[0].value
```

Use instead:

```python
def target():
    return system.tag.readBlocking(["[default]Line/Setpoint"])[0].value
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Script(&ModuleLevelSideEffect)],
    options: None,
};

struct ModuleLevelSideEffect;

/// `system.*` functions without side effects that are fine at import.
fn is_benign(path: &str) -> bool {
    matches!(
        path,
        "system.util.getLogger"
            | "system.util.getGlobals"
            | "system.util.getProjectName"
            | "system.util.jsonEncode"
            | "system.util.jsonDecode"
    ) || path.starts_with("system.date.")
        || path.starts_with("system.math.")
        || path
            .strip_prefix("system.dataset.")
            .is_some_and(|name| !name.starts_with("export"))
}

impl ScriptRule for ModuleLevelSideEffect {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        // Project library modules only: a `.py` file outside any project is
        // not imported by Ignition.
        if unit.script.kind != ScriptKind::LibraryModule
            || !matches!(cx.data(), ModelData::Library(_))
            || cx.resource().is_none()
        {
            return;
        }
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        let mut v = ImportTimeCalls {
            model,
            found: Vec::new(),
        };
        for stmt in &unit.parsed.module.body {
            v.visit_stmt(stmt);
        }
        for (range, message) in v.found {
            report(cx, unit, range, message);
        }
    }
}

/// `__name__ == '__main__'` in either order.
fn is_main_guard(test: &Expr) -> bool {
    let Expr::Compare(c) = test else { return false };
    if c.ops.len() != 1 || c.ops[0] != CmpOp::Eq {
        return false;
    }
    let is_name = |e: &Expr| matches!(e, Expr::Name(n) if &*n.id == "__name__");
    let is_main = |e: &Expr| matches!(e, Expr::Str(s) if s.s.as_str() == Some("__main__"));
    (is_name(&c.left) && is_main(&c.comparators[0]))
        || (is_main(&c.left) && is_name(&c.comparators[0]))
}

struct ImportTimeCalls<'m, 's> {
    model: &'m SemanticModel<'s>,
    found: Vec<(TextRange, String)>,
}

impl<'a> Visitor<'a> for ImportTimeCalls<'_, '_> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::FunctionDef(f) => {
                for d in &f.decorator_list {
                    self.visit_expr(d);
                }
                for d in &f.args.defaults {
                    self.visit_expr(d);
                }
            }
            Stmt::If(i) if is_main_guard(&i.test) => {
                for s in &i.orelse {
                    self.visit_stmt(s);
                }
            }
            _ => walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Lambda(l) => {
                for d in &l.args.defaults {
                    self.visit_expr(d);
                }
                return;
            }
            Expr::Call(call) => {
                let message = if let Some(path) = system_path(self.model, &call.func)
                    && iglint_stubs::system_function(&path).is_some()
                {
                    (!is_benign(&path)).then(|| {
                        format!("`{path}` runs whenever the module is imported, in the importing script's scope")
                    })
                } else if let Some(name) = ["open", "file", "execfile"]
                    .into_iter()
                    .find(|n| is_builtin_reference(self.model, &call.func, n))
                {
                    Some(format!("`{name}` does I/O whenever the module is imported"))
                } else {
                    self.model
                        .callee(call)
                        .filter(|c| SLEEPS.contains(&c.as_str()))
                        .map(|c| format!("`{c}` blocks every script that imports the module"))
                };
                if let Some(message) = message {
                    self.found.push((call.range, message));
                }
            }
            _ => {}
        }
        walk_expr(self, expr);
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn module_level_calls() {
        let code = "import time\nvalues = system.tag.readBlocking(['a'])\nfpmi.db.runPrepUpdate('q', [])\nf = open('x')\ntime.sleep(1)\nclass C:\n    rows = system.db.runPrepQuery('q', [])\n";
        let d = lint_module("SCR303", code);
        assert_eq!(
            flagged(&d),
            [
                "system.tag.readBlocking(['a'])",
                "fpmi.db.runPrepUpdate('q', [])",
                "open('x')",
                "time.sleep(1)",
                "system.db.runPrepQuery('q', [])"
            ]
        );
        assert_eq!(
            messages(&d)[0],
            "`system.tag.readBlocking` runs whenever the module is imported, in the importing script's scope"
        );
        assert!(messages(&d)[1].starts_with("`system.db.runPrepUpdate`"));
    }

    #[test]
    fn decorators_and_defaults_run_at_import() {
        let d = lint_module(
            "SCR303",
            "def f(x=system.tag.readBlocking(['a'])):\n    return x\n@cache(system.security.getUserRoles('a', 'b'))\ndef g():\n    pass\n",
        );
        assert_eq!(
            flagged(&d),
            [
                "system.tag.readBlocking(['a'])",
                "system.security.getUserRoles('a', 'b')"
            ]
        );
    }

    #[test]
    fn deferred_code_is_not_reported() {
        let code = "def f():\n    return system.tag.readBlocking(['a'])\ng = lambda: open('x')\nclass C:\n    def m(self):\n        system.db.runPrepQuery('q', [])\nif __name__ == '__main__':\n    system.util.sendMessage('p', 'm')\nif '__main__' == __name__:\n    open('x')\nelse:\n    open('y')\n";
        let d = lint_module("SCR303", code);
        assert_eq!(flagged(&d), ["open('y')"]);
    }

    #[test]
    fn benign_calls() {
        let code = "g = system.util.getGlobals()\nlog = system.util.getLogger('x')\np = system.util.getProjectName()\nnow = system.date.now()\nds = system.dataset.toDataSet(['a'], [[1]])\nj = system.util.jsonDecode('{}')\nm = system.math.mean([1])\nclient = system.net.httpClient()\n";
        assert!(lint_module("SCR303", code).is_empty());
        let d = lint_module("SCR303", "system.dataset.exportCSV('f', True, ds)\n");
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn local_names_are_not_the_api_or_builtins() {
        let code = "def open(x):\n    return x\nopen('x')\nsystem = Fake()\nsystem.tag.readBlocking(['a'])\n";
        assert!(lint_module("SCR303", code).is_empty());
    }

    #[test]
    fn only_library_modules() {
        let f = crate::rules::test_support::Fixture::new()
            .file("tools/build.py", "data = open('x').read()\n");
        assert!(f.lint(&["SCR303"]).is_empty());
        assert!(
            view(ViewScript::Action, "\tsystem.tag.readBlocking(['a'])")
                .lint(&["SCR303"])
                .is_empty()
        );
        assert!(
            gateway_timer("def handleTimerEvent():\n\tpass\nsystem.tag.readBlocking(['a'])\n")
                .lint(&["SCR303"])
                .is_empty()
        );
    }
}
