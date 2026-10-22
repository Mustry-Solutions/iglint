//! SCR001 syntax-error.

use iglint_project::ScriptKind;
use iglint_text::{TextRange, TextSize};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::report;

pub static META: RuleMeta = RuleMeta {
    code: "SCR001",
    name: "syntax-error",
    category: Category::Script,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "Script does not parse with Jython 2.7",
    explanation: r#"## What it does

Reports scripts that Jython 2.7 (Ignition 8.3's interpreter) refuses to
parse, at the first error, with Jython's message. Python 3-only syntax
(`print(x, end="")` without the future import, `f"..."` strings, `nonlocal`,
`except E as e` is fine but `raise ... from ...` is not) gets a hint.

The script is checked as Ignition compiles it: Perspective scripts inside
their generated `def` header (`def runAction(self, event):` and so on),
gateway event files with their canonical header, library modules as
written.

Two Ignition-specific cases, verified on an 8.3 gateway:

* An encoding declaration (`# -*- coding: utf-8 -*-`) in the first two
  lines of the compiled source is a syntax error, because Ignition compiles
  scripts from text (`SyntaxError: encoding declaration in Unicode
  string`). In Perspective scripts and event files the first body line is
  line 2 of the compiled source.
* A UTF-8 byte order mark at the start of a library module or event file is
  not skipped (`no viable alternative at character '\ufeff'`).

Empty Perspective scripts are reported by `VIEW008` instead. Empty
disabled scripts are not reported: the Designer writes a header-only
`do<Method>.py` stub for every disabled WebDev method, and Ignition does
not compile disabled scripts.

## Why is this bad?

A script that does not parse never runs: the event does nothing, the
transform yields an error quality, the library module cannot be imported
by anything.

## Example

```python
def onStartup(session):
    print "starting", session.props.id
    if session.props.auth.authenticated
        system.perspective.navigate("/home")
```

Use instead:

```python
def onStartup(session):
    print "starting", session.props.id
    if session.props.auth.authenticated:
        system.perspective.navigate("/home")
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Script(&SyntaxError)],
    options: None,
};

struct SyntaxError;

impl ScriptRule for SyntaxError {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let script = unit.script;
        let source: &str = &script.source;
        if source.starts_with('\u{feff}') {
            report(
                cx,
                unit,
                TextRange::at(TextSize::ZERO, TextSize::new(3)),
                "SyntaxError: no viable alternative at character '\\ufeff': Ignition does not skip a byte order mark; save the file as UTF-8 without BOM",
            );
            return;
        }
        let Some(error) = unit.parsed.errors.first() else {
            return;
        };
        let reported_by_view_rule = matches!(
            script.kind,
            ScriptKind::EventAction
                | ScriptKind::Transform
                | ScriptKind::PropertyChange
                | ScriptKind::MessageHandler
                | ScriptKind::CustomMethod
                | ScriptKind::ExtensionFunction { .. }
        );
        // Empty view scripts are VIEW008's; disabled empty scripts (the
        // stubs the Designer writes for unused WebDev methods) never compile.
        if script.is_blank() && (reported_by_view_rule || !script.enabled) {
            return;
        }
        let mut message = format!("SyntaxError: {}", error.message);
        if error.message == "encoding declaration in Unicode string" {
            message
                .push_str(": Ignition compiles scripts from text, remove the encoding declaration");
        }
        report(cx, unit, error.range, message);
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn first_error_only() {
        let d = lint_module("SCR001", "x = (\ny = 1\nif x\n    pass\n");
        assert_eq!(d.len(), 1);
        assert!(
            d[0].message.starts_with("SyntaxError: "),
            "{}",
            d[0].message
        );
    }

    #[test]
    fn valid_code_is_clean() {
        assert!(lint_module("SCR001", "print 'a'\nx = `1`\nexec 'y = 1'\n").is_empty());
    }

    #[test]
    fn unknown_character_names() {
        let d = lint_module("SCR001", "x = u'\\N{THIS IS NOT A UNICODE NAME}'\n");
        assert_eq!(d.len(), 1);
        assert!(d[0].message.contains("character name"), "{}", d[0].message);
        assert!(lint_module("SCR001", "x = u'\\N{PILE OF POO}' + '\\N{NOT A NAME}'\n").is_empty());
    }

    #[test]
    fn python3_syntax_gets_a_hint() {
        let d = lint_module("SCR001", "def f():\n    nonlocal x\n");
        assert_eq!(lines(&d), [(2, "x".to_owned())]);
        assert!(d[0].message.contains("Python 3"), "{}", d[0].message);
    }

    #[test]
    fn coding_declarations_and_byte_order_marks() {
        // Library module: lines 1 and 2 count, line 3 does not.
        let d = lint_module("SCR001", "# -*- coding: utf-8 -*-\nx = 1\n");
        assert_eq!(flagged(&d), ["utf-8"]);
        assert!(d[0].message.contains("encoding declaration"));
        assert_eq!(lint_module("SCR001", "# a\n# coding=latin-1\n").len(), 1);
        assert!(lint_module("SCR001", "# a\n# b\n# coding: utf-8\n").is_empty());
        // Perspective script: the first body line is line 2.
        let d = view(ViewScript::Action, "\t# -*- coding: utf-8 -*-\n\tpass").lint(&["SCR001"]);
        assert_eq!(decoded_flagged(&d), ["utf-8"]);
        assert!(
            view(ViewScript::Action, "\tpass\n\t# coding: utf-8")
                .lint(&["SCR001"])
                .is_empty()
        );
        // Event file: the header line is replaced by the canonical one.
        let d = gateway_message("def handleMessage(payload):\n\t# coding: utf-8\n\tpass\n")
            .lint(&["SCR001"]);
        assert_eq!(flagged(&d), ["utf-8"]);
        // Byte order mark.
        let d = lint_module("SCR001", "\u{feff}x = 1\n");
        assert_eq!(flagged(&d), ["\u{feff}"]);
        let d = gateway_message("\u{feff}def handleMessage(payload):\n\tpass\n").lint(&["SCR001"]);
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn errors_in_view_scripts_map_into_the_json_string() {
        let d = view(
            ViewScript::Transform,
            "\tif value \u{3d}\u{3d} 1\n\t\treturn 2",
        )
        .lint(&["SCR001"]);
        assert_eq!(d.len(), 1);
        let flagged = decoded_flagged(&d);
        assert_eq!(flagged, ["\n"], "{:?}", d[0].message);
    }

    #[test]
    fn empty_view_scripts_are_left_to_view008() {
        assert!(view(ViewScript::Action, "  ").lint(&["SCR001"]).is_empty());
        // An event file with only its header does not compile.
        assert_eq!(
            gateway_message("def handleMessage(payload):\n")
                .lint(&["SCR001"])
                .len(),
            1
        );
    }
}
