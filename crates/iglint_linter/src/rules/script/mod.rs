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
use iglint_project::script::ScriptBody;
use iglint_python::ast::{Expr, Stmt};
use iglint_python_semantic::{SemanticModel, SemanticOptions};
use iglint_text::{TextEdit, TextRange, TextSize};

use crate::checker::{Checker, Report};
use crate::diagnostic::{Applicability, Fix};
use crate::rule::ScriptUnit;

pub mod assert_on_tuple;
pub mod bare_except;
pub mod compile_error;
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
pub mod module_level_side_effect;
pub mod mutable_default;
pub mod pointless_statement;
pub mod print_statement;
pub mod raise_string;
pub mod redefined_builtin;
pub mod redefined_unused;
pub mod return_in_finally;
pub mod self_assignment;
pub mod shadowed_library_package;
pub mod sleep_in_event_script;
pub mod syntax_error;
pub mod system_api;
pub mod system_function_arity;
pub mod undefined_name;
pub mod unknown_system_function;
pub mod unknown_system_keyword;
pub mod unreachable_code;
pub mod unused_import;
pub mod unused_suppression;
pub mod unused_variable;
pub mod used_before_assignment;
pub mod wrong_scope_system_function;

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
        let mut injected: Vec<&str> = unit.script.implicit_globals().collect();
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
            // A file outside any project is a plain module run from disk.
            injected.push("__file__");
        }
        if unit.script.kind == iglint_project::ScriptKind::LibraryModule {
            // `__package__` appears once the module runs an import.
            injected.push("__package__");
        }
        let options = SemanticOptions {
            injected,
            unknown_globals,
        };
        SemanticModel::build(&unit.parsed.module, &options)
    }))
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
    let start = stmt.start().to_usize();
    let end = stmt.end().to_usize();
    let line_start = source[..start].rfind('\n').map_or(0, |i| i + 1);
    if !source[line_start..start].trim().is_empty() {
        return None;
    }
    let rest = &source[end..];
    let line_end = rest.find('\n').map_or(source.len(), |i| end + i + 1);
    let tail = source[end..line_end].trim();
    if !(tail.is_empty() || tail.starts_with('#')) {
        return None;
    }
    Some(TextRange::from(line_start..line_end))
}

/// The indentation of the line `offset` is on.
pub fn indentation_at(source: &str, offset: TextSize) -> &str {
    let at = offset.to_usize();
    let line_start = source[..at].rfind('\n').map_or(0, |i| i + 1);
    let line = &source[line_start..];
    let len = line.len() - line.trim_start_matches([' ', '\t']).len();
    &line[..len]
}

/// Edits that delete `stmt`, which is an element of `body`: the whole line
/// when the statement is alone on it, replaced by `pass` when it is the
/// only statement of its block.
pub fn delete_statement(source: &str, body: &[Stmt], stmt: &Stmt) -> Option<(TextRange, String)> {
    let range = iglint_text::Ranged::range(stmt);
    let lines = statement_line_range(source, range)?;
    if body.len() == 1 {
        Some((range, "pass".to_owned()))
    } else {
        Some((lines, String::new()))
    }
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
            Some(script.source[start..at].matches('\n').count() + 1)
        }
        ScriptBody::File { .. } => {
            let file = cx.file(unit.file)?;
            let at = script.source_map.file_offset(offset).to_usize();
            Some(
                file.text()[..at.min(file.text().len())]
                    .matches('\n')
                    .count()
                    + 1,
            )
        }
    }
}
