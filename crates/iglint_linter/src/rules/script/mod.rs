//! SCR: script correctness (Jython 2.7).
//!
//! Every script location (library modules, gateway and session event
//! files, WebDev handlers, Perspective scripts, tag event scripts) is
//! parsed once by the driver. The rules here share one semantic model per
//! script ([`semantic`]), built with the names the location provides:
//! Jython's builtins, Ignition's implicit globals (`system`, `fpmi`, `sys`,
//! `__builtins__`, the generated function's name) and the project's
//! top-level library packages (format spec 3.2). Scripts that do not parse
//! are left to `SCR001`; the semantic rules stay silent on them.
//!
//! Findings are reported in script coordinates and mapped to the file
//! ([`report`]); fixes are text edits on the script source, mapped and
//! JSON-escaped for scripts stored in `view.json` strings ([`fix`]).

use iglint_json::EscapeStyle;
use iglint_project::EmbeddedScript;
use iglint_project::script::ScriptBody;
use iglint_python::ast::{Expr, Stmt};
use iglint_python_semantic::{SemanticModel, SemanticOptions};
use iglint_text::{TextEdit, TextRange, TextSize};

use crate::checker::{Checker, Report};
use crate::diagnostic::{Applicability, Fix, SuiteGuard};
use crate::rule::ScriptUnit;

pub mod assert_on_tuple;
pub mod bare_except;
pub mod compile_error;
pub mod compile_warning;
pub mod dangerous_loop_closure;
pub mod deprecated_system_function;
pub mod duplicate_dict_key;
pub mod except_order;
pub mod except_tuple;
pub mod format_string;
pub mod global_at_module_level;
pub mod invalid_event_script_header;
pub mod legacy_script_prefix;
pub mod literal_identity;
pub mod message_handler_payload_misuse;
pub mod module_level_side_effect;
pub mod mutable_default;
pub mod pointless_statement;
pub mod print_statement;
pub mod pylint;
pub mod raise_string;
pub mod redefined_builtin;
pub mod redefined_event_handler;
pub mod redefined_unused;
pub mod return_in_finally;
pub mod self_assignment;
pub mod shadowed_library_package;
pub mod sleep_in_event_script;
pub mod syntax_error;
pub mod system_api;
pub mod system_function_arity;
pub mod trailing_whitespace;
pub mod undefined_name;
pub mod unknown_system_function;
pub mod unknown_system_keyword;
pub mod unreachable_code;
pub mod unresolved_component_reference;
pub mod unused_import;
pub mod unused_suppression;
pub mod unused_variable;
pub mod used_before_assignment;
pub mod wrong_scope_system_function;

#[cfg(test)]
mod layout_oracle;
#[cfg(test)]
mod locations_tests;
#[cfg(test)]
pub(crate) mod testing;

/// The semantic model of the script, built on first use and shared by
/// every script rule. `None` when the script does not parse.
pub fn semantic<'u, 's>(
    cx: &Checker<'_, '_>,
    unit: &'u ScriptUnit<'s, '_>,
) -> Option<&'u SemanticModel<'s>> {
    if !unit.parsed.errors.is_empty() {
        return None;
    }
    Some(unit.semantic.get_or_init(|| {
        let (injected, unknown_globals) = injected_names(cx, unit.script);
        let options = SemanticOptions {
            injected,
            unknown_globals,
            module_has_name: unit.script.has_module_name(),
        };
        SemanticModel::build(&unit.parsed.module, &options)
    }))
}

/// The names `script` sees as globals without defining them (Ignition's
/// implicit globals, the generated function's name, the project's
/// top-level library packages), and whether that list may be incomplete
/// (scripts outside a project, or a project whose ancestry is not in the
/// workspace, see library names iglint does not know).
pub fn injected_names<'n>(
    cx: &'n Checker<'_, '_>,
    script: &'n EmbeddedScript<'_>,
) -> (Vec<&'n str>, bool) {
    let mut injected: Vec<&str> = script.implicit_globals().collect();
    let unknown_globals = match cx.project_index() {
        Some((index, project)) => {
            let (globals, complete) = index.script_library_globals(project);
            injected.extend(globals);
            !complete
        }
        // Scripts of a resource without a project (gateway collections:
        // tag events run with the gateway scripting project's library)
        // and `.py` files outside any project see library names we do
        // not know.
        None => true,
    };
    if cx.resource().is_none() {
        // A file outside any project is a plain module run from disk: its
        // loader sets `__file__` and `__package__`. (In a library module,
        // `__package__` appears once an import runs: the semantic model
        // binds it there.)
        injected.push("__file__");
        injected.push("__package__");
    }
    (injected, unknown_globals)
}

/// The script source the parse and the model refer to.
pub fn source<'s>(unit: &ScriptUnit<'s, '_>) -> &'s str {
    &unit.script.source
}

/// Reports a range of the script source, mapped to the file.
pub fn report<'c>(
    cx: &'c mut Checker<'_, '_>,
    unit: &ScriptUnit<'_, '_>,
    range: TextRange,
    message: impl Into<String>,
) -> Report<'c> {
    cx.report_script(unit.file, unit.script, range, message)
        .node_path(unit.script.node_path.clone())
        .ign_lint_path(unit.script.ign_lint_path.clone())
}

/// Whether `range` touches the generated header (text Ignition adds that
/// is not in the file).
pub fn in_header(unit: &ScriptUnit<'_, '_>, range: TextRange) -> bool {
    unit.script.source_map.is_synthetic(range.start())
        || (range.end() > TextSize::ZERO
            && unit
                .script
                .source_map
                .is_synthetic(range.end() - TextSize::new(1)))
}

/// A fix made of edits to the script source. Each edit is mapped to the
/// file and, for scripts stored in JSON strings, its new text is escaped
/// in the string's style. `None` when an edit touches the generated
/// header or does not map back exactly.
pub fn fix(
    cx: &Checker<'_, '_>,
    unit: &ScriptUnit<'_, '_>,
    message: impl Into<String>,
    applicability: Applicability,
    edits: Vec<(TextRange, String)>,
) -> Option<Fix> {
    let mut file_edits = Vec::with_capacity(edits.len());
    for (range, text) in edits {
        file_edits.push(file_edit(unit, range, &text)?);
    }
    let edits = cx.file_edits(unit.file, file_edits);
    if edits.is_empty() {
        return None;
    }
    Some(Fix {
        message: message.into(),
        applicability,
        edits,
        suite: None,
        unsafe_reason: None,
        layout: false,
    })
}

/// One script-source edit as a file edit.
pub fn file_edit(unit: &ScriptUnit<'_, '_>, range: TextRange, text: &str) -> Option<TextEdit> {
    let script = unit.script;
    let source: &str = &script.source;
    if range.start() < script.source_map.body_start() || range.end().to_usize() > source.len() {
        return None;
    }
    let file_range = script.file_range(range);
    let content = match script.body {
        ScriptBody::Json(s) => {
            // The replaced file text must decode to exactly the replaced
            // source text (no partial escapes).
            let raw = s.raw();
            let style = if raw
                .bytes()
                .any(|b| matches!(b, b'<' | b'>' | b'&' | b'=' | b'\''))
            {
                EscapeStyle::Minimal
            } else {
                EscapeStyle::Ignition
            };
            let mut out = String::with_capacity(text.len());
            iglint_json::escape_into(&mut out, text, style);
            out
        }
        ScriptBody::File { .. } => text.to_owned(),
    };
    Some(TextEdit::replace(file_range, content))
}

/// The range of a whole statement for deletion: from the start of its
/// line through its line break, when the statement is alone on its lines.
/// `None` when other code shares the lines (`a = 1; b = 2`).
pub fn statement_line_range(source: &str, stmt: TextRange) -> Option<TextRange> {
    let end = stmt.end().to_usize();
    let before = source.get(..stmt.start().to_usize())?;
    let rest = source.get(end..)?;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    // `line_start` is just after a `\n` found by `rfind`.
    if !before.split_at(line_start).1.trim().is_empty() {
        return None;
    }
    let tail_len = rest.find('\n').map_or(rest.len(), |i| i + 1);
    let line_end = end + tail_len;
    // `tail_len` is the end of the text or just after a `\n` found by `find`.
    let tail = rest.split_at(tail_len).0.trim();
    if !(tail.is_empty() || tail.starts_with('#')) {
        return None;
    }
    Some(TextRange::from(line_start..line_end))
}

/// The indentation of the line `offset` is on.
pub fn indentation_at(source: &str, offset: TextSize) -> &str {
    let before = TextRange::new(TextSize::ZERO, offset).slice(source);
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    // `line_start` is just after a `\n` found by `rfind`.
    let line = source.split_at(line_start).1;
    line.strip_suffix(line.trim_start_matches([' ', '\t']))
        .unwrap_or_default()
}

/// An edit of the script source that may delete a statement of a suite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatementEdit {
    pub range: TextRange,
    pub content: String,
    /// When the edit deletes a statement: the start of the suite's first
    /// statement and the number of statements in the suite.
    pub suite: Option<(TextSize, u32)>,
}

impl StatementEdit {
    pub fn replace(range: TextRange, content: impl Into<String>) -> Self {
        StatementEdit {
            range,
            content: content.into(),
            suite: None,
        }
    }
}

/// The text of `range` of `source` without its last line break, and the
/// line breaks inside it: the edit that empties the lines of `range` but
/// keeps them (so no other line moves).
fn blank_lines(source: &str, range: TextRange) -> (TextRange, String) {
    let text = range.slice(source);
    let body = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .or_else(|| text.strip_suffix('\r'))
        .unwrap_or(text);
    let mut breaks = String::new();
    for b in body.bytes() {
        match b {
            b'\n' | b'\r' => breaks.push(char::from(b)),
            _ => {}
        }
    }
    (TextRange::at(range.start(), TextSize::of(body)), breaks)
}

/// Edits that delete `stmt`, which is an element of `body`, when the
/// statement is alone on its lines; replaced by `pass` when it is the only
/// statement of its block. With `keep_lines` (what safe fixes do) the
/// statement's text goes and its lines stay, empty, so that no line
/// number changes; otherwise the lines go too. Deletions carry their
/// suite, so that fixes deleting several statements of one suite never
/// empty it in one pass (the last one is replaced by `pass` in the next
/// pass).
pub fn delete_statement(
    source: &str,
    body: &[Stmt],
    stmt: &Stmt,
    keep_lines: bool,
) -> Option<StatementEdit> {
    let range = iglint_text::Ranged::range(stmt);
    let lines = statement_line_range(source, range)?;
    if body.len() == 1 {
        Some(StatementEdit::replace(range, "pass"))
    } else if body
        .iter()
        .position(|s| std::ptr::eq(s, stmt))
        .is_some_and(|i| i == 0 && is_string_expr(&body[1]))
    {
        // Deleting the first statement would promote the next statement,
        // a bare string literal, to the body's docstring (changing
        // `__doc__`, which code can read). Keep a `pass` so the string
        // stays a plain expression. A `pass` is never itself a docstring.
        Some(StatementEdit::replace(range, "pass"))
    } else {
        let first = iglint_text::Ranged::range(&body[0]).start();
        let (range, content) = if keep_lines {
            blank_lines(source, lines)
        } else {
            (lines, String::new())
        };
        Some(StatementEdit {
            range,
            content,
            suite: Some((first, u32::try_from(body.len()).ok()?)),
        })
    }
}

/// A fix made of one [`StatementEdit`]; see [`fix`].
pub fn statement_fix(
    cx: &Checker<'_, '_>,
    unit: &ScriptUnit<'_, '_>,
    message: impl Into<String>,
    applicability: Applicability,
    edit: StatementEdit,
) -> Option<Fix> {
    let fixed = fix(
        cx,
        unit,
        message,
        applicability,
        vec![(edit.range, edit.content)],
    )?;
    Some(match edit.suite {
        Some((first, statements)) => {
            let key = unit.script.file_range(TextRange::empty(first)).start();
            fixed.deleting_from(SuiteGuard {
                key: key.to_u32(),
                statements,
            })
        }
        None => fixed,
    })
}

/// Python's `str.isspace` (what `str.rstrip()` removes).
pub fn is_python_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

/// Trailing whitespace of the lines of `source` from offset `from`: per
/// line (split at `\n`), the run of characters at its end for which
/// `is_space` holds. With `keep_cr`, a `\r` right before the `\n` is the
/// line break and not part of the run.
pub fn trailing_whitespace(
    source: &str,
    from: TextSize,
    keep_cr: bool,
    is_space: fn(char) -> bool,
) -> Vec<TextRange> {
    let mut out = Vec::new();
    let mut start = from.to_usize();
    while let Some(rest) = source.get(start..) {
        let end = rest.find('\n').map_or(source.len(), |i| start + i);
        // `end` is the end of the text or a `\n` found by `find`.
        let mut line = rest.split_at(end - start).0;
        if keep_cr && end < source.len() {
            line = line.strip_suffix('\r').unwrap_or(line);
        }
        let kept = line.trim_end_matches(is_space).len();
        if kept < line.len() {
            out.push(TextRange::from(start + kept..start + line.len()));
        }
        start = end + 1;
    }
    out
}

/// The ranges of the string literals of a parsed script (whitespace in a
/// multi-line string is data, not trailing whitespace).
pub fn string_ranges(parsed: &iglint_python::Parsed) -> Vec<TextRange> {
    let mut out = Vec::new();
    walk_exprs(&parsed.module.body, &mut |e| {
        if let Expr::Str(s) = e {
            out.push(s.range);
        }
    });
    out
}

/// Whether the whitespace `range` follows a backslash inside a string
/// literal that Jython's lexer drops (a single-quoted string that a line
/// break ends before its closing quote, [`iglint_python::DropReason`]).
/// The backslash escapes the first whitespace character, so the line break
/// still ends the string and the lexer drops it with the line break; without
/// the whitespace the backslash continues the string on the next line
/// (review round 25, B1: `v = 'x  \ ` then `y'` reads `y`, without the
/// space it is the string `'x  y'`). Such whitespace is part of the string.
pub fn escaped_in_dropped_string(
    parsed: &iglint_python::Parsed,
    source: &str,
    range: TextRange,
) -> bool {
    parsed.dropped.iter().any(|d| {
        d.reason == iglint_python::DropReason::UnterminatedString
            && d.range.start() < range.start()
            && range.start() < d.range.end()
            && TextRange::new(d.range.start(), range.start())
                .slice(source)
                .bytes()
                .rev()
                .take_while(|b| *b == b'\\')
                .count()
                % 2
                == 1
    })
}

/// Whether `stmt` is a bare string-literal expression, the only statement
/// kind that becomes a docstring when it is first in a module, class or
/// function body.
pub fn is_string_expr(stmt: &Stmt) -> bool {
    matches!(stmt, Stmt::Expr(e) if matches!(&*e.value, Expr::Str(_)))
}

/// The statement list that directly contains `target`, searching the
/// whole module.
pub fn containing_body<'a>(body: &'a [Stmt], target: &Stmt) -> Option<&'a [Stmt]> {
    if body.iter().any(|s| std::ptr::eq(s, target)) {
        return Some(body);
    }
    for stmt in body {
        let found = match stmt {
            Stmt::FunctionDef(s) => containing_body(&s.body, target),
            Stmt::ClassDef(s) => containing_body(&s.body, target),
            Stmt::For(s) => {
                containing_body(&s.body, target).or_else(|| containing_body(&s.orelse, target))
            }
            Stmt::While(s) => {
                containing_body(&s.body, target).or_else(|| containing_body(&s.orelse, target))
            }
            Stmt::If(s) => {
                containing_body(&s.body, target).or_else(|| containing_body(&s.orelse, target))
            }
            Stmt::With(s) => containing_body(&s.body, target),
            Stmt::TryExcept(s) => containing_body(&s.body, target)
                .or_else(|| {
                    s.handlers
                        .iter()
                        .find_map(|h| containing_body(&h.body, target))
                })
                .or_else(|| containing_body(&s.orelse, target)),
            Stmt::TryFinally(s) => {
                containing_body(&s.body, target).or_else(|| containing_body(&s.finalbody, target))
            }
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// Calls `f` for every expression of the module, including those in nested
/// functions, classes and lambdas.
pub fn walk_exprs<'a>(body: &'a [Stmt], f: &mut dyn FnMut(&'a Expr)) {
    use iglint_python::visitor::{Visitor, walk_expr};
    struct V<'f, 'a>(&'f mut dyn FnMut(&'a Expr));
    impl<'a> Visitor<'a> for V<'_, 'a> {
        fn visit_expr(&mut self, expr: &'a Expr) {
            (self.0)(expr);
            walk_expr(self, expr);
        }
    }
    let mut v = V(f);
    iglint_python::visitor::walk_body(&mut v, body);
}

/// Calls `f` for every statement of the module, including nested ones.
pub fn walk_stmts<'a>(body: &'a [Stmt], f: &mut dyn FnMut(&'a Stmt)) {
    use iglint_python::visitor::{Visitor, walk_stmt};
    struct V<'f, 'a>(&'f mut dyn FnMut(&'a Stmt));
    impl<'a> Visitor<'a> for V<'_, 'a> {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            (self.0)(stmt);
            walk_stmt(self, stmt);
        }
    }
    let mut v = V(f);
    iglint_python::visitor::walk_body(&mut v, body);
}

/// Whether the name `n` reads Jython's builtin of its name: the reference
/// resolves to the builtins and no scope it is looked up in can bind
/// names the code does not show (`exec`, `import *`, writes through
/// `globals()` or `__builtin__`). An unknown reference is not.
pub fn reads_builtin(model: &SemanticModel<'_>, n: &iglint_python::ast::ExprName) -> bool {
    model.reference_at(n.range).is_some_and(|r| {
        r.resolution == iglint_python_semantic::Resolution::Builtin
            && !model.is_dynamic_chain(r.scope)
    })
}

/// Whether `expr` is `True`, `False` or `None` reading the builtin
/// constant ([`reads_builtin`]). In Jython 2.7 all three are names:
/// `True = lambda: 7` and `False = 1` rebind them, and so do
/// `import x as None`, `except E, None` and `globals()['None'] = 1`
/// (verified on Ignition 8.3.3; only `None = 1` and `def f(None)` are
/// syntax errors).
pub fn is_builtin_constant(model: &SemanticModel<'_>, expr: &Expr) -> bool {
    matches!(expr, Expr::Name(n) if matches!(&*n.id, "True" | "False" | "None") && reads_builtin(model, n))
}

/// Whether the name `name` in the module refers to the builtin of that
/// name (it is not rebound anywhere in a scope that the reference sees).
pub fn is_builtin_reference(model: &SemanticModel<'_>, expr: &Expr, name: &str) -> bool {
    let Expr::Name(n) = expr else { return false };
    if &*n.id != name {
        return false;
    }
    model
        .reference_at(n.range)
        .is_some_and(|r| r.resolution == iglint_python_semantic::Resolution::Builtin)
}

/// The 1-based line of a script-source offset as users see it: the line in
/// the file for `.py` files, the line of the script body for scripts stored
/// in JSON strings (where the whole script is one line of the file).
pub fn file_line(
    cx: &Checker<'_, '_>,
    unit: &ScriptUnit<'_, '_>,
    offset: TextSize,
) -> Option<usize> {
    let script = unit.script;
    match script.body {
        ScriptBody::Json(_) => {
            let start = script.source_map.body_start().to_usize();
            let at = offset.to_usize().max(start);
            Some(
                TextRange::from(start..at)
                    .slice(&script.source)
                    .matches('\n')
                    .count()
                    + 1,
            )
        }
        ScriptBody::File { .. } => {
            let file = cx.file(unit.file)?;
            let at = script.source_map.file_offset(offset).to_usize();
            Some(
                TextRange::from(0..at)
                    .slice(file.text())
                    .matches('\n')
                    .count()
                    + 1,
            )
        }
    }
}
