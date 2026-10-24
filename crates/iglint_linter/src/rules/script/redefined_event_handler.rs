//! SCR307 redefined-event-handler.

use iglint_project::{EventScriptKind, ScriptKind};
use iglint_python::ast::{Arguments, Expr, Stmt};
use iglint_python_semantic::{BindingKind, ScopeId, SemanticModel};
use iglint_text::{Ranged, TextRange};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{is_builtin_constant, report, semantic};

pub static META: RuleMeta = RuleMeta {
    code: "SCR307",
    name: "redefined-event-handler",
    category: Category::Script,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "Event script file that rebinds its handler to something Ignition cannot call",
    explanation: r#"## What it does

Checks the module-level code after the function of a gateway timer, tag
change, message handler, startup, shutdown or update script file, or of a
WebDev handler. Ignition compiles the file under its canonical header
(`def handleTimerEvent():`, `def handleMessage(payload):`, ...), runs the
module code, then looks the handler up by name and calls it with the
canonical arguments, by position. So the last module-level binding of
the name is what runs. The rule reports when that binding is certainly
one Ignition cannot call:

* a `def` (or `lambda`) that cannot take the canonical number of
  positional arguments, such as `def handleTimerEvent(event):` after the
  header: every event fails with `TypeError: handleTimerEvent() takes
  exactly 1 argument (0 given)`;
* a value that is not callable (`handleTimerEvent = None`, a number, a
  string, a list, tuple, dict or set display or comprehension): Ignition
  reports `The function 'handleTimerEvent' is not defined`. A comparison,
  an operator, a call or a backquote is not followed: each runs code
  that may return a function (`__eq__` may return a lambda, and Jython
  then calls it). `True`, `False` and `None` count only where they are
  Jython's builtins: all three are names a module can rebind
  (`True = lambda: 7`, `import x as None`);
* `del handleTimerEvent`: the same.

Only bindings at module level that always run are followed: one inside
an `if`, a loop, a `try` or a `with`, an import, a class, any other value,
`exec` or `import *` make the outcome unknown, and nothing is reported.
So does any binding of the name that the module-level statements do not
show, found from every scope: a `global` declaration of the name in a
function or class (also in the handler itself, which module code may
call), a list comprehension at module level whose variable is the name
(Python 2 comprehensions leak it), `exec`, `import *` or a write through
`globals()` or `vars()` in any scope, and code that reaches a frame or
the interpreter. Bindings that stay in their scope (a local of a
function, a class attribute, the variable of a generator expression or
of a comprehension inside a function) do not matter. Parameter names do
not matter (Ignition passes the arguments by position), and defaults or
`*args` that make the call fit are fine.

Verified on an Ignition 8.3.3 gateway for timer, tag change, message,
startup, shutdown and update scripts and WebDev handlers. Scheduled
scripts run their first definition whatever is bound later, so for them
only `del` is reported (the script then fails to load with a
`NameError`). Perspective session and page events and Vision client
events are not checked.

## Why is this bad?

The handler never runs. The error is logged on the gateway (for timers
only the first time; later ones are logged at debug level), and for a
message handler the caller of `system.util.sendRequest` gets the error.

## Example

```python
def handleTimerEvent():
	poll()

def handleTimerEvent(event):
	poll()
```

Use instead:

```python
def handleTimerEvent():
	poll()
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Script(&RedefinedEventHandler)],
    options: None,
};

struct RedefinedEventHandler;

/// The event files whose handler lookup was verified on the gateway: the
/// module code runs, then the last binding of the name is called by
/// position with the canonical arguments.
fn checked(kind: EventScriptKind) -> bool {
    use EventScriptKind as E;
    matches!(
        kind,
        E::GatewayStartup
            | E::GatewayShutdown
            | E::GatewayUpdate
            | E::GatewayTimer
            | E::GatewayTagChange
            | E::GatewayMessage
            | E::WebDev(_)
    )
}

/// What the handler name is bound to after a module-level statement.
#[derive(Clone, Copy)]
enum Binding<'a> {
    /// A function or lambda that takes these arguments.
    Callable(&'a Arguments, &'a Stmt),
    /// Certainly not callable.
    NotCallable(&'a Stmt),
    Deleted(&'a Stmt),
    /// Anything else (or a binding that may not run).
    Unknown,
}

/// Whether a call with `n` positional arguments fits `args`.
fn accepts(args: &Arguments, n: usize) -> bool {
    let required = args.args.len() - args.defaults.len();
    required <= n && (args.vararg.is_some() || args.args.len() >= n)
}

/// Whether `value` is certainly not callable: a literal of a type whose
/// instances cannot be called (numbers, strings, lists, tuples, dicts,
/// sets, their comprehensions, generator expressions), or the builtin
/// constants `True`, `False` and `None` where they read the builtins (all
/// three are names that a module can rebind, `True = lambda: 7`).
/// Anything that runs user code to produce the value is unknown, also
/// when the operation usually gives a bool or a string: a comparison
/// calls `__eq__`, `__lt__`, ... which may return a function (Jython
/// calls `handleTimerEvent = C() == 1` when `C.__eq__` returns a lambda),
/// and the same holds for operators, calls and backquotes.
fn not_callable(value: &Expr, constant: &dyn Fn(&Expr) -> bool) -> bool {
    match value {
        Expr::Num(_)
        | Expr::Str(_)
        | Expr::List(_)
        | Expr::Tuple(_)
        | Expr::Dict(_)
        | Expr::Set(_)
        | Expr::ListComp(_)
        | Expr::SetComp(_)
        | Expr::DictComp(_)
        | Expr::GeneratorExp(_) => true,
        Expr::Name(_) => constant(value),
        _ => false,
    }
}

/// Whether `expr` (an assignment target) binds `name`.
fn binds(target: &Expr, name: &str) -> bool {
    match target {
        Expr::Name(n) => &*n.id == name,
        Expr::Tuple(t) => t.elts.iter().any(|e| binds(e, name)),
        Expr::List(l) => l.elts.iter().any(|e| binds(e, name)),
        _ => false,
    }
}

/// Whether a statement nested anywhere in `stmt` (not in a function or
/// class body) may bind `name` or change the module's namespace in ways
/// that are not followed.
fn may_bind(stmt: &Stmt, name: &str) -> bool {
    let suites: Vec<&[Stmt]> = match stmt {
        Stmt::If(s) => vec![&s.body, &s.orelse],
        Stmt::While(s) => vec![&s.body, &s.orelse],
        Stmt::For(s) => vec![&s.body, &s.orelse],
        Stmt::With(s) => vec![&s.body],
        Stmt::TryExcept(s) => {
            let mut out: Vec<&[Stmt]> = vec![&s.body, &s.orelse];
            out.extend(s.handlers.iter().map(|h| h.body.as_slice()));
            out
        }
        Stmt::TryFinally(s) => vec![&s.body, &s.finalbody],
        _ => Vec::new(),
    };
    suites
        .into_iter()
        .flatten()
        .any(|s| statement_binds(s, name) || dynamic(s) || may_bind(s, name))
}

/// A statement that writes the namespace in a way that is not followed.
fn dynamic(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Exec(_) => true,
        Stmt::ImportFrom(s) => s.names.iter().any(|a| a.name.as_str() == "*"),
        _ => false,
    }
}

fn statement_binds(stmt: &Stmt, name: &str) -> bool {
    match stmt {
        Stmt::FunctionDef(f) => f.name.as_str() == name,
        Stmt::ClassDef(c) => c.name.as_str() == name,
        Stmt::Assign(s) => s.targets.iter().any(|t| binds(t, name)),
        Stmt::AugAssign(s) => binds(&s.target, name),
        Stmt::Delete(s) => s.targets.iter().any(|t| binds(t, name)),
        Stmt::For(s) => binds(&s.target, name),
        Stmt::With(s) => s.optional_vars.as_deref().is_some_and(|v| binds(v, name)),
        Stmt::TryExcept(s) => s
            .handlers
            .iter()
            .any(|h| h.name.as_deref().is_some_and(|n| binds(n, name))),
        Stmt::Import(s) => s.names.iter().any(|a| {
            let bound = a.asname.as_ref().unwrap_or(&a.name).as_str();
            bound.split('.').next() == Some(name)
        }),
        Stmt::ImportFrom(s) => s
            .names
            .iter()
            .any(|a| a.asname.as_ref().unwrap_or(&a.name).as_str() == name),
        _ => false,
    }
}

/// The binding of `name` after the module-level statement `stmt`, given
/// the binding before it.
fn after<'a>(
    stmt: &'a Stmt,
    name: &str,
    before: Binding<'a>,
    constant: &dyn Fn(&Expr) -> bool,
) -> Binding<'a> {
    match stmt {
        Stmt::FunctionDef(f) if f.name.as_str() == name => {
            if f.decorator_list.is_empty() {
                Binding::Callable(&f.args, stmt)
            } else {
                Binding::Unknown
            }
        }
        Stmt::Assign(s) if s.targets.iter().any(|t| binds(t, name)) => match s.targets.as_slice() {
            [Expr::Name(_)] => match &*s.value {
                Expr::Lambda(l) => Binding::Callable(&l.args, stmt),
                value if not_callable(value, constant) => Binding::NotCallable(stmt),
                _ => Binding::Unknown,
            },
            _ => Binding::Unknown,
        },
        Stmt::Delete(s) if s.targets.iter().any(|t| binds(t, name)) => match s.targets.as_slice() {
            [Expr::Name(_)] => Binding::Deleted(stmt),
            _ => Binding::Unknown,
        },
        Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {
            if statement_binds(stmt, name) {
                Binding::Unknown
            } else {
                before
            }
        }
        _ if statement_binds(stmt, name) || dynamic(stmt) || may_bind(stmt, name) => {
            Binding::Unknown
        }
        _ => before,
    }
}

impl ScriptRule for RedefinedEventHandler {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let ScriptKind::EventFile(kind) = unit.script.kind else {
            return;
        };
        // Scheduled scripts run their first definition whatever is bound
        // later, but fail to load when the name is deleted (verified).
        let scheduled = kind == EventScriptKind::GatewayScheduled;
        if !(checked(kind) || scheduled) || !unit.parsed.errors.is_empty() {
            return;
        }
        let name = kind.function_name();
        let arity = kind.params().len();
        let body = &unit.parsed.module.body;
        // The canonical header Ignition compiles the file under.
        let Some(Stmt::FunctionDef(header)) = body.first() else {
            return;
        };
        if header.name.as_str() != name {
            return;
        }
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        // A binding the statement walk below does not see (through
        // `global` in a function or class, a list comprehension's
        // variable, `exec`, writes through `globals()` or a frame) cannot
        // be ordered: the outcome is unknown.
        if binds_elsewhere(model, name, body) {
            return;
        }
        let constant = |e: &Expr| is_builtin_constant(model, e);
        let mut binding = Binding::Unknown;
        let mut redefined = false;
        for stmt in &body[1..] {
            let next = after(stmt, name, binding, &constant);
            redefined |= statement_binds(stmt, name) || may_bind(stmt, name);
            binding = next;
        }
        if !redefined {
            return;
        }
        let call = match arity {
            0 => "no arguments".to_owned(),
            1 => "1 argument".to_owned(),
            n => format!("{n} arguments"),
        };
        let header_text = kind.canonical_header();
        let (range, message): (TextRange, String) = match binding {
            Binding::Callable(..) | Binding::NotCallable(_) if scheduled => return,
            Binding::Callable(args, stmt) if !accepts(args, arity) => (
                name_range(stmt),
                format!(
                    "`{name}` is redefined here with parameters that cannot take {call}: Ignition runs \
                     this last definition, not `{header_text}`, and every call fails with a \
                     TypeError"
                ),
            ),
            Binding::NotCallable(stmt) => (
                stmt.range(),
                format!(
                    "`{name}` is rebound here to a value that cannot be called: Ignition finds no \
                     function `{name}` and the script never runs"
                ),
            ),
            Binding::Deleted(stmt) => (
                stmt.range(),
                format!(
                    "`{name}` is deleted here: Ignition finds no function `{name}` and the script \
                     never runs"
                ),
            ),
            _ => return,
        };
        let _ = report(cx, unit, range, message);
    }
}

/// Whether code that the walk over the module's statements does not
/// follow may bind `name` in the module namespace, from the semantic
/// model's bindings of every scope: a `global` declaration of the name in
/// any function or class (a class body runs where it is defined, a
/// function wherever it is called, the handler itself included, since
/// module code may call it), a list comprehension's variable at module
/// level (Python 2 comprehensions leak it: `[0 for handleTimerEvent in
/// fs]`), any binding the model puts in the module from another scope,
/// `exec`, `from m import *` or a write through `globals()` or `vars()` in
/// any scope, and a route to a frame or the interpreter
/// ([`iglint_python_semantic::dynamic::frame_escape`]).
fn binds_elsewhere(model: &SemanticModel<'_>, name: &str, body: &[Stmt]) -> bool {
    let dynamic = model.scopes().any(|(id, scope)| {
        (id != ScopeId::MODULE && scope.declares_global(name))
            || scope.has_exec
            || scope.writes_globals
            || scope.star_import.is_some()
            || scope.unqualified_exec.is_some()
    });
    dynamic
        || model
            .scope(ScopeId::MODULE)
            .bindings_of(name)
            .iter()
            .map(|id| model.binding(*id))
            .any(|b| b.kind == BindingKind::ComprehensionVar || b.site != ScopeId::MODULE)
        || iglint_python_semantic::dynamic::frame_escape(body).is_some()
}

/// The name of a `def`, or the whole statement.
fn name_range(stmt: &Stmt) -> TextRange {
    match stmt {
        Stmt::FunctionDef(f) => f.name.range(),
        other => other.range(),
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    fn timer(code: &str) -> Vec<String> {
        messages(&gateway_timer(code).lint(&["SCR307"]))
    }

    const HEADER: &str = "def handleTimerEvent():\n\tpoll()\n";

    #[test]
    fn reports_what_ignition_cannot_call() {
        for (tail, what) in [
            (
                "def handleTimerEvent(event):\n\tpoll()\n",
                "cannot take no arguments",
            ),
            (
                "def handleTimerEvent(a, b=1):\n\tpass\n",
                "cannot take no arguments",
            ),
            (
                "handleTimerEvent = lambda e: 1\n",
                "cannot take no arguments",
            ),
            ("handleTimerEvent = None\n", "cannot be called"),
            ("handleTimerEvent = False\n", "cannot be called"),
            ("handleTimerEvent = 1\n", "cannot be called"),
            ("handleTimerEvent = [poll]\n", "cannot be called"),
            ("del handleTimerEvent\n", "is deleted"),
            // The last binding counts.
            (
                "def handleTimerEvent():\n\tpass\nhandleTimerEvent = None\n",
                "cannot be called",
            ),
        ] {
            let found = timer(&format!("{HEADER}{tail}"));
            assert_eq!(found.len(), 1, "{tail}: {found:?}");
            assert!(found[0].contains(what), "{tail}: {found:?}");
        }
    }

    #[test]
    fn silent_when_the_call_fits_or_the_outcome_is_unknown() {
        for tail in [
            "",
            // Verified on the gateway: these run.
            "def handleTimerEvent():\n\tpass\n",
            "def handleTimerEvent(event=None):\n\tpass\n",
            "def handleTimerEvent(*args):\n\tpass\n",
            "handleTimerEvent = lambda: 1\n",
            "class handleTimerEvent:\n\tpass\n",
            "from java.lang.System import currentTimeMillis as handleTimerEvent\n",
            // Not followed.
            "if x:\n\tdef handleTimerEvent(event):\n\t\tpass\n",
            "try:\n\thandleTimerEvent = None\nexcept:\n\tpass\n",
            "handleTimerEvent = make()\n",
            "@wrap\ndef handleTimerEvent(event):\n\tpass\n",
            "handleTimerEvent, b = None, 1\n",
            // Reviewer round 16 (Codex): rich comparisons, operators and
            // backquotes run user code that may return a function
            // (verified on Jython: `__eq__` returning a lambda is called).
            "class E(object):\n\tdef __eq__(self, o):\n\t\treturn lambda: 1\nhandleTimerEvent = E() == 1\n",
            "handleTimerEvent = a < b\n",
            "handleTimerEvent = a is not b\n",
            "handleTimerEvent = x + 1\n",
            "handleTimerEvent = not x\n",
            "handleTimerEvent = -x\n",
            "handleTimerEvent = `x`\n",
            "handleTimerEvent = x and None\n",
            "handleTimerEvent = None if x else poll\n",
            // Reviewer round 15 (Codex): `True`, `False` and `None` are
            // names Jython lets a module rebind (verified on 8.3.3).
            "True = lambda: 7\nhandleTimerEvent = True\n",
            "handleTimerEvent = False\ndef f():\n\tglobal False\n\tFalse = 1\n",
            "import helpers as None\nhandleTimerEvent = None\n",
            "handleTimerEvent = None\nexec 'x = 1'\n",
            // Rebound afterwards to something that runs.
            "handleTimerEvent = None\ndef handleTimerEvent():\n\tpass\n",
            "handleTimerEvent = None\nexec 'handleTimerEvent = poll'\n",
            "handleTimerEvent = None\nfrom helpers import *\n",
        ] {
            let found = timer(&format!("{HEADER}{tail}"));
            assert!(found.is_empty(), "{tail}: {found:?}");
        }
        // Bindings in the body do not change what Ignition calls.
        let body = "def handleTimerEvent():\n\tglobal handleTimerEvent\n\tdef handleTimerEvent(event):\n\t\tpass\n";
        assert!(timer(body).is_empty());
    }

    /// Review round 17 (Codex): bindings of the handler name that the
    /// walk over module-level statements does not see. Verified on the
    /// Jython of Ignition 8.3.3 (the module code run, then the handler
    /// looked up by name and called with no arguments): each of these
    /// runs, so nothing may be reported.
    #[test]
    fn bindings_from_other_scopes_make_the_outcome_unknown() {
        for tail in [
            "handleTimerEvent = None\nclass C:\n\tglobal handleTimerEvent\n\thandleTimerEvent = lambda: 1\n",
            "def handleTimerEvent(e):\n\tpass\nclass C:\n\tglobal handleTimerEvent\n\thandleTimerEvent = lambda: 1\n",
            "def g():\n\tglobal handleTimerEvent\n\thandleTimerEvent = lambda: 1\nhandleTimerEvent = None\ng()\n",
            "handleTimerEvent = None\ndef g(f):\n\tglobal handleTimerEvent\n\thandleTimerEvent = f\ng(poll)\n",
            // Python 2 list comprehensions leak their variable.
            "handleTimerEvent = None\n[0 for handleTimerEvent in [poll]]\n",
            "handleTimerEvent = None\nx = [0 for handleTimerEvent in [poll]]\n",
            "def handleTimerEvent(e):\n\tpass\nprint [1 for handleTimerEvent in [poll]]\n",
            "handleTimerEvent = None\ndef g():\n\texec 'global handleTimerEvent; handleTimerEvent = poll'\ng()\n",
            "handleTimerEvent = None\ndef g():\n\tglobals()['handleTimerEvent'] = poll\ng()\n",
            "handleTimerEvent = None\nimport sys\nsys._getframe().f_globals['handleTimerEvent'] = poll\n",
        ] {
            let found = timer(&format!("{HEADER}{tail}"));
            assert!(found.is_empty(), "{tail}: {found:?}");
        }
        // The handler itself rebinds the name when module code calls it.
        let code = "def handleTimerEvent():\n\tglobal handleTimerEvent\n\thandleTimerEvent = poll\nkeep = handleTimerEvent\nhandleTimerEvent = None\nkeep()\n";
        assert!(timer(code).is_empty(), "{:?}", timer(code));
    }

    /// The other direction: bindings in other scopes that do not reach the
    /// module namespace leave the verdict as it was. Verified on Jython
    /// like the test above: each of these fails.
    #[test]
    fn bindings_that_stay_in_their_scope_do_not_hide_the_finding() {
        for (tail, what) in [
            (
                "handleTimerEvent = None\ndef g():\n\thandleTimerEvent = lambda: 1\ng()\n",
                "cannot be called",
            ),
            (
                "handleTimerEvent = None\nclass C:\n\thandleTimerEvent = lambda: 1\n",
                "cannot be called",
            ),
            (
                "handleTimerEvent = None\nx = list(0 for handleTimerEvent in [poll])\n",
                "cannot be called",
            ),
            (
                "handleTimerEvent = None\nx = {0 for handleTimerEvent in [poll]}\n",
                "cannot be called",
            ),
            (
                "handleTimerEvent = None\nx = {0: 1 for handleTimerEvent in [poll]}\n",
                "cannot be called",
            ),
            (
                "handleTimerEvent = None\ndef g():\n\t[0 for handleTimerEvent in [poll]]\ng()\n",
                "cannot be called",
            ),
            (
                "handleTimerEvent = None\ndef g():\n\tglobal other\n\tother = poll\ng()\n",
                "cannot be called",
            ),
            (
                "def handleTimerEvent(e):\n\tpass\nl = lambda: [0 for handleTimerEvent in [poll]]\nl()\n",
                "cannot take no arguments",
            ),
            (
                "def handleTimerEvent(e):\n\tpass\nclass C:\n\tx = [0 for handleTimerEvent in [poll]]\n",
                "cannot take no arguments",
            ),
        ] {
            let found = timer(&format!("{HEADER}{tail}"));
            assert_eq!(found.len(), 1, "{tail}: {found:?}");
            assert!(found[0].contains(what), "{tail}: {found:?}");
        }
    }

    #[test]
    fn every_verified_kind_with_its_arguments() {
        let message = gateway_message(
            "def handleMessage(payload):\n\treturn 1\ndef handleMessage(p):\n\treturn 2\n",
        );
        assert!(message.lint(&["SCR307"]).is_empty(), "names do not matter");
        let message = gateway_message(
            "def handleMessage(payload):\n\treturn 1\ndef handleMessage(payload, extra):\n\treturn 2\n",
        );
        let found = messages(&message.lint(&["SCR307"]));
        assert_eq!(found.len(), 1);
        assert!(found[0].contains("cannot take 1 argument"), "{found:?}");
        let web =
            webdev("def doGet(request, session):\n\treturn {}\ndef doGet(request):\n\treturn {}\n");
        let found = messages(&web.lint(&["SCR307"]));
        assert_eq!(found.len(), 1);
        assert!(found[0].contains("cannot take 2 arguments"), "{found:?}");
        // Scheduled scripts: only a deletion.
        let scheduled = |code: &str| {
            crate::rules::test_support::Fixture::new()
                .resource(
                    "projects/p/ignition/scheduled/s",
                    &[("handleScheduleEvent.py", code)],
                )
                .lint(&["SCR307"])
        };
        let header = "def handleScheduleEvent():\n\tpass\n";
        assert!(scheduled(&format!("{header}def handleScheduleEvent(e):\n\tpass\n")).is_empty());
        assert!(scheduled(&format!("{header}handleScheduleEvent = 1\n")).is_empty());
        assert_eq!(
            scheduled(&format!("{header}del handleScheduleEvent\n")).len(),
            1
        );
        // Perspective session events are not checked.
        let session = session_event("def onStartup(session):\n\tpass\ndef onStartup():\n\tpass\n");
        assert!(session.lint(&["SCR307"]).is_empty());
    }
}
