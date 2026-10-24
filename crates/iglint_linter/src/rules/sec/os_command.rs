//! SEC006 os-command.

use iglint_python::ast::Expr;
use iglint_text::{Ranged, TextRange};

use super::produced_by;
use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{report, semantic, walk_exprs};

pub static META: RuleMeta = RuleMeta {
    code: "SEC006",
    name: "os-command",
    category: Category::Sec,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "Script starts an operating system process",
    explanation: r#"## What it does

Reports calls that start operating system processes:

* `os.system`, `os.popen`, `os.popen2`/`3`/`4`, `os.exec*`, `os.spawn*`,
  `os.startfile`;
* `subprocess.call`, `check_call`, `check_output` and `Popen`;
* `commands.getoutput`, `getstatusoutput` and `getstatus`, the `popen2`
  module, `platform.popen`;
* `java.lang.Runtime.getRuntime().exec(...)`, also through `from
  java.lang import Runtime` or a variable holding the runtime;
* `java.lang.ProcessBuilder(...)`;
* Ignition's `system.util.execute`.

Calls are recognised through imports and aliases (`import os as o`,
`from subprocess import Popen`, `rt = Runtime.getRuntime()`), never by
the function name alone: a library function named `system` or a method
`exec` of another object is not reported.

## Why is this bad?

The process runs on the gateway (or the client) with the operating system
account of the Ignition service. A command line built from tag values,
payloads or user input lets that input run arbitrary commands. Even fixed
commands make the project depend on the server's operating system, its
`PATH` and installed tools, and block a gateway thread while they run.

Prefer Ignition's own functions (`system.file.*`, `system.net.*`) or a
service with its own API. When a process is really needed, use a fixed program with a
list of arguments, never a shell command line, and suppress the finding
with `# iglint: ignore[SEC006]` on that line.

## Example

```python
import os

def onMessageReceived(self, payload):
    os.system("ping -c 1 " + payload["host"])
```

Use instead:

```python
def onMessageReceived(self, payload):
    reachable = system.net.httpClient().get(payload["url"]).good
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Script(&OsCommand)],
    options: None,
};

struct OsCommand;

/// Python functions that start a process.
fn is_process_function(name: &str) -> bool {
    if let Some(f) = name.strip_prefix("os.") {
        return matches!(
            f,
            "system" | "popen" | "popen2" | "popen3" | "popen4" | "startfile"
        ) || f
            .strip_prefix("exec")
            .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b"lpev".contains(&b)))
            || f.strip_prefix("spawn")
                .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b"lpev".contains(&b)));
    }
    matches!(
        name,
        "subprocess.call"
            | "subprocess.check_call"
            | "subprocess.check_output"
            | "subprocess.Popen"
            | "commands.getoutput"
            | "commands.getstatusoutput"
            | "commands.getstatus"
            | "popen2.popen2"
            | "popen2.popen3"
            | "popen2.popen4"
            | "popen2.Popen3"
            | "popen2.Popen4"
            | "platform.popen"
            | "java.lang.ProcessBuilder"
            | "system.util.execute"
    )
}

impl ScriptRule for OsCommand {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        let mut found: Vec<(TextRange, String)> = Vec::new();
        walk_exprs(&unit.parsed.module.body, &mut |e| {
            let Expr::Call(call) = e else { return };
            if let Some(callee) = model.callee(call)
                && is_process_function(&callee)
            {
                found.push((
                    call.func.range(),
                    format!("`{callee}` starts an operating system process"),
                ));
                return;
            }
            // `Runtime.getRuntime().exec(...)`, `rt.exec(...)`.
            if let Expr::Attribute(a) = &*call.func
                && &*a.attr.id == "exec"
                && produced_by(model, &a.value).as_deref() == Some("java.lang.Runtime.getRuntime")
            {
                found.push((
                    call.func.range(),
                    "`Runtime.exec` starts an operating system process".to_owned(),
                ));
            }
        });
        for (range, message) in found {
            report(cx, unit, range, message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_process_function;
    use crate::rules::script::testing::*;

    #[test]
    fn process_functions() {
        for f in [
            "os.system",
            "os.execvpe",
            "os.spawnl",
            "os.popen3",
            "subprocess.Popen",
        ] {
            assert!(is_process_function(f), "{f}");
        }
        for f in [
            "os.path",
            "os.execute",
            "os.spawn",
            "os.listdir",
            "subprocess.PIPE",
        ] {
            assert!(!is_process_function(f), "{f}");
        }
    }

    #[test]
    fn reports_process_calls_through_aliases() {
        let code = r#"import os as o
import subprocess
from subprocess import check_output as co
from java.lang import Runtime, ProcessBuilder
import java.lang
def f(cmd):
    o.system(cmd)
    subprocess.Popen([cmd])
    co(["ls"])
    Runtime.getRuntime().exec(cmd)
    rt = Runtime.getRuntime()
    rt.exec(["ls", "-l"])
    ProcessBuilder(["ls"]).start()
    java.lang.Runtime.getRuntime().exec(cmd)
    o.popen(cmd).read()
    system.util.execute(["ls"])
"#;
        assert_eq!(
            flagged(&lint_module("SEC006", code)),
            [
                "o.system",
                "subprocess.Popen",
                "co",
                "Runtime.getRuntime().exec",
                "rt.exec",
                "ProcessBuilder",
                "java.lang.Runtime.getRuntime().exec",
                "o.popen",
                "system.util.execute",
            ]
        );
    }

    #[test]
    fn names_alone_are_not_enough() {
        let code = r#"from java.lang import Runtime
def system(x):
    return x
def f(os, cmd, engine):
    os.system(cmd)
    system(cmd)
    engine.exec(cmd)
    Runtime.getRuntime().totalMemory()
    Runtime.getRuntime().availableProcessors()
    subprocess.call(cmd)
"#;
        let d = lint_module("SEC006", code);
        assert!(d.is_empty(), "{d:?}");
    }

    #[test]
    fn everywhere() {
        assert_eq!(
            flagged_everywhere("SEC006", "\timport os\n\tos.system(\"echo \\\"x\\\"\")"),
            ["os.system"]
        );
        let f = gateway_message(
            "def handleMessage(payload):\n\timport subprocess as sp\n\tsp.check_call(payload['cmd'])\n",
        );
        assert_eq!(flagged(&f.lint(&["SEC006"])), ["sp.check_call"]);
    }
}
