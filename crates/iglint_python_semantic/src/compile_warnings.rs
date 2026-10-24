//! Warnings Jython's compiler issues (`SyntaxWarning`) for code it
//! compiles.
//!
//! Verified against IA's Jython 2.7 with full `compile()` and
//! `warnings.catch_warnings(record=True)` (`tests/oracle/warnings.cases`
//! and the `compile_warning_oracle` test): Jython warns about exactly
//! these, with these messages.
//!
//! * `name 'x' declared global after assignment`: a `global` statement
//!   names a name that the same scope binds earlier in the source (an
//!   assignment, augmented assignment, `del`, `import`, `def`, `class`,
//!   `for`, `with` or `except` target, a list comprehension variable),
//!   whatever the control flow (a binding in the other branch of an `if`
//!   counts);
//! * `name 'x' declared global after use`: otherwise, the same scope reads
//!   the name earlier (also as the object of an attribute or subscript
//!   target, and in the decorators, default values, base classes and first
//!   iterable of a generator that the scope evaluates). Uses in nested
//!   functions, classes and lambdas do not count, nor uses after an
//!   earlier `global` of the name;
//! * `import * only allowed at module level`: `from m import *` in a
//!   function, or in a class inside a function (not in a class at module
//!   level), unless Jython generates no code for it (after a `return` in
//!   the same suite, say).
//!
//! The scope is a function, a class body or the module (where a `global`
//! after an assignment warns too).

use iglint_python::ast::*;
use iglint_text::TextRange;

use crate::compile::ends_suite;
use rustc_hash::FxHashMap;

use crate::model::{Binding, BindingKind, Reference, ReferenceKind, ScopeId, SemanticModel};

/// One compile-time warning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompileWarning {
    /// The code to underline: the name in the `global` statement, or the
    /// `import *` statement.
    pub range: TextRange,
    /// Jython's message (without the `SyntaxWarning: ` prefix).
    pub message: String,
}

/// The compile-time warnings of a module, in source order.
pub fn compile_warnings(model: &SemanticModel<'_>) -> Vec<CompileWarning> {
    let mut out = Vec::new();
    // The bindings by the scope whose code makes them and the references
    // by scope, each by name, built once when some scope declares a name
    // `global` (review round 25, M3: scanning every binding and reference
    // per declaration made `n` functions declaring `global` quadratic).
    let declares = model
        .scopes()
        .any(|(_, s)| !s.global_declarations.is_empty());
    let mut bindings_at: FxHashMap<(ScopeId, &str), Vec<&Binding<'_>>> = FxHashMap::default();
    let mut references_at: FxHashMap<(ScopeId, &str), Vec<&Reference<'_>>> = FxHashMap::default();
    if declares {
        for (_, b) in model.bindings() {
            bindings_at
                .entry((b.site, b.source_name))
                .or_default()
                .push(b);
        }
        for (_, r) in model.references() {
            references_at
                .entry((r.scope, r.source_name))
                .or_default()
                .push(r);
        }
    }
    for (scope_id, scope) in model.scopes() {
        let mut declared: Vec<&str> = Vec::new();
        let mut globals: Vec<(&str, TextRange)> = scope
            .global_declarations
            .iter()
            .map(|id| {
                let b = model.binding(*id);
                (b.source_name, b.range)
            })
            .collect();
        globals.sort_by_key(|(_, r)| r.start());
        for (name, range) in globals {
            if declared.contains(&name) {
                continue;
            }
            declared.push(name);
            let before = |r: TextRange| r.start() < range.start();
            let bindings = bindings_at
                .get(&(scope_id, name))
                .map_or(&[][..], Vec::as_slice);
            let references = references_at
                .get(&(scope_id, name))
                .map_or(&[][..], Vec::as_slice);
            let assigned = bindings.iter().any(|b| {
                before(b.range)
                    && !matches!(
                        b.kind,
                        BindingKind::Global
                            | BindingKind::Builtin
                            | BindingKind::Injected
                            | BindingKind::ClassModule
                            | BindingKind::ImportPackage
                            | BindingKind::Parameter
                    )
            }) || references
                .iter()
                .any(|r| r.kind == ReferenceKind::Del && before(r.range));
            let used = || {
                references.iter().any(|r| {
                    !matches!(r.kind, ReferenceKind::Del | ReferenceKind::Implicit(_))
                        && before(r.range)
                })
            };
            let what = if assigned {
                "assignment"
            } else if used() {
                "use"
            } else {
                continue;
            };
            out.push(CompileWarning {
                range,
                message: format!("name '{name}' declared global after {what}"),
            });
        }
    }
    stars(&model.module.body, false, false, true, &mut out);
    out.sort_by_key(|w| (w.range.start(), w.range.end()));
    out
}

/// `import *` below a function, in code Jython generates: its code
/// generator issues the warning, and generates nothing for the rest of a
/// suite after a statement that ends it ([`ends_suite`]; not the module's
/// own statements).
fn stars(
    body: &[Stmt],
    in_function: bool,
    mut dead: bool,
    top: bool,
    out: &mut Vec<CompileWarning>,
) {
    for stmt in body {
        let suite =
            |b: &[Stmt], out: &mut Vec<CompileWarning>| stars(b, in_function, dead, false, out);
        match stmt {
            Stmt::FunctionDef(f) => stars(&f.body, true, dead, false, out),
            Stmt::ClassDef(c) => suite(&c.body, out),
            Stmt::If(s) => {
                suite(&s.body, out);
                suite(&s.orelse, out);
            }
            Stmt::For(s) => {
                suite(&s.body, out);
                suite(&s.orelse, out);
            }
            Stmt::While(s) => {
                suite(&s.body, out);
                suite(&s.orelse, out);
            }
            Stmt::With(s) => suite(&s.body, out),
            Stmt::TryExcept(s) => {
                suite(&s.body, out);
                for h in &s.handlers {
                    suite(&h.body, out);
                }
                suite(&s.orelse, out);
            }
            Stmt::TryFinally(s) => {
                suite(&s.body, out);
                suite(&s.finalbody, out);
            }
            Stmt::ImportFrom(i)
                if in_function && !dead && i.names.iter().any(|a| a.name.as_str() == "*") =>
            {
                out.push(CompileWarning {
                    range: i.range,
                    message: "import * only allowed at module level".to_owned(),
                });
            }
            _ => {}
        }
        if !top {
            dead |= ends_suite(stmt);
        }
    }
}
