//! Size limits of Jython's compiler: scripts that parse but cannot be
//! loaded or compiled because they are too large.
//!
//! Measured with IA's Jython 2.7 (`oracle/run-oracle.sh --compile`, see
//! `docs/internal/ignition-format.md`, "Size limits"):
//!
//! * Jython reads the first two lines of a script through a reader marked
//!   with a 100,000 character limit while it looks for an encoding
//!   declaration. When the end of line 2 lies at or beyond that limit,
//!   reading fails with `java.io.IOException: Mark invalid`. This is
//!   exact. No other line, literal or identifier has a length limit.
//! * Every code unit (the module body, each function and class body)
//!   becomes one Java method, limited to 65,535 bytes of bytecode
//!   (`Module or method too large`). Each statement costs at least 6
//!   bytes (the frame's line number update), so more than 10,922 lines
//!   with statements (from line 6 on) cannot compile. Jython fails earlier
//!   (9,379 lines of `pass` fit, 3,282 of `x = 1`); between the two
//!   nothing is reported.
//! * Jython generates no code for the rest of a suite after a statement
//!   that ends it (`ends_suite`): those statements, the
//!   `def`s among them and their string constants count for neither
//!   limit. The module's own statement list is compiled whole.
//! * A module becomes one Java class, limited to 65,535 constant pool
//!   entries (`Class too large`). Each distinct string constant takes at
//!   least two (the string and its UTF-8 text), so more than 32,767
//!   distinct string literals cannot compile (Jython fails at 32,594).

use iglint_python::ast::*;
use iglint_python::visitor::{Visitor, walk_expr};
use iglint_text::{LineIndex, Ranged, TextRange, TextSize};
use rustc_hash::FxHashSet;

use crate::compile::{
    CompileError, CompileErrorKind, ends_suite, for_each_child_body, for_each_stmt_expr,
};

/// Jython's read-ahead limit while it looks for an encoding declaration,
/// in UTF-16 code units.
pub const MARK_LIMIT: usize = 100_000;

/// The largest Java method, in bytes of bytecode.
const METHOD_LIMIT: usize = 65_535;
/// The least bytecode of a source line that starts a statement, from line
/// 6 on (`aload` the frame, `bipush`/`sipush`/`ldc` the line,
/// `invokevirtual PyFrame.setline`).
const LINE_COST: usize = 6;
/// Lines whose number is pushed with a 1-byte `iconst` (lines 1 to 5).
const CHEAP_LINES: usize = 5;
/// The largest constant pool of a Java class.
const CONSTANT_POOL_LIMIT: usize = 65_535;

/// The `Mark invalid` failure: the first two lines of `source` (as Jython
/// compiles it, with the generated header of an embedded script) reach
/// [`MARK_LIMIT`] UTF-16 code units. Reports the line that crosses the
/// limit.
pub fn mark_limit_error(source: &str) -> Option<CompileError> {
    // Jython appends "\n\n" before reading. The reader fails when the
    // terminator of line 2 starts at UTF-16 index `MARK_LIMIT` or later;
    // a line ends at `\n`, `\r` or `\r\n`.
    let bytes = source.as_bytes();
    let line_end = |from: usize| {
        bytes[from..]
            .iter()
            .position(|&b| matches!(b, b'\n' | b'\r'))
            .map_or(source.len(), |i| from + i)
    };
    let units = |from: usize, to: usize| source[from..to].encode_utf16().count();
    let end1 = line_end(0);
    let (start2, terminator) = match bytes.get(end1) {
        None => (end1, 1),
        Some(b'\r') if bytes.get(end1 + 1) == Some(&b'\n') => (end1 + 2, 2),
        Some(_) => (end1 + 1, 1),
    };
    let end2 = line_end(start2);
    let (units1, units2) = (units(0, end1), units(start2, end2));
    let at = units1 + terminator + units2;
    if at < MARK_LIMIT {
        return None;
    }
    // Point at the longer line.
    let range = if units1 >= units2 {
        TextRange::new(TextSize::ZERO, TextSize::from(end1))
    } else {
        TextRange::new(TextSize::from(start2), TextSize::from(end2))
    };
    Some(mark_error(range, at))
}

fn mark_error(range: TextRange, units: usize) -> CompileError {
    let shown = if units > MARK_LIMIT {
        "more than 100,000".to_owned()
    } else {
        "100,000".to_owned()
    };
    CompileError {
        range,
        message: format!(
            "Jython cannot read this script: its first two lines are {shown} characters long, and Jython fails from 100,000 on (java.io.IOException: Mark invalid)"
        ),
        anchor: None,
        kind: CompileErrorKind::MarkInvalid,
    }
}

/// Code units that are certainly larger than a Java method allows, and a
/// module with certainly too many constants for one Java class. `source`
/// is the text `module` was parsed from (with the generated header of an
/// embedded script).
pub fn size_errors<'a>(module: &'a Module, source: &str) -> Vec<CompileError> {
    let index = LineIndex::new(source);
    let mut out = Vec::new();
    // Code units: (statements of the unit, the node to report at).
    let mut units: Vec<(&'a [Stmt], Option<&'a Stmt>)> = vec![(&module.body, None)];
    while let Some((body, owner)) = units.pop() {
        let mut lines = FxHashSet::default();
        let mut crossing = None;
        let mut pending: Vec<&'a Stmt> = emitted(body, owner.is_none()).iter().rev().collect();
        while let Some(stmt) = pending.pop() {
            let line = index.line_of(stmt.range().start()) + 1;
            // `global` may generate no code.
            if !matches!(stmt, Stmt::Global(_))
                && line > CHEAP_LINES
                && lines.insert(line)
                && lines.len() * LINE_COST > METHOD_LIMIT
                && crossing.is_none()
            {
                crossing = Some(stmt);
            }
            match stmt {
                Stmt::FunctionDef(s) => units.push((&s.body, Some(stmt))),
                Stmt::ClassDef(s) => units.push((&s.body, Some(stmt))),
                other => {
                    let mut suites: Vec<&'a [Stmt]> = Vec::new();
                    for_each_child_body(other, &mut |b| suites.push(b));
                    for suite in suites.into_iter().rev() {
                        pending.extend(emitted(suite, false).iter().rev());
                    }
                }
            }
        }
        if let Some(stmt) = crossing {
            let what = match owner {
                None => "module",
                Some(Stmt::ClassDef(_)) => "class body",
                Some(_) => "function",
            };
            let range = match owner {
                Some(Stmt::FunctionDef(s)) => s.name.range(),
                Some(Stmt::ClassDef(s)) => s.name.range(),
                _ => stmt.range(),
            };
            out.push(CompileError {
                range,
                message: format!(
                    "Jython cannot compile this {what}: it has {} lines of statements, more than one Java method can hold (Module or method too large)",
                    lines.len()
                ),
                anchor: None,
                kind: CompileErrorKind::MethodTooLarge,
            });
        }
    }
    let mut strings = Strings::default();
    strings.suite(&module.body, true);
    if strings.distinct.len() * 2 > CONSTANT_POOL_LIMIT
        && let Some(range) = strings.crossing
    {
        out.push(CompileError {
            range,
            message: format!(
                "Jython cannot compile this module: it has {} distinct string constants, more than one Java class can hold (Class too large)",
                strings.distinct.len()
            ),
            anchor: None,
            kind: CompileErrorKind::ClassTooLarge,
        });
    }
    out.sort_by_key(|e| (e.range.start(), e.range.end()));
    out
}

/// Distinct string constants as Java sees them (`str` bytes as Latin-1).
#[derive(Default)]
struct Strings {
    distinct: FxHashSet<String>,
    /// The literal that makes the count exceed the limit.
    crossing: Option<TextRange>,
}

impl Strings {
    /// The constants of the statements of a suite that Jython generates
    /// code for.
    fn suite(&mut self, body: &[Stmt], whole: bool) {
        for stmt in emitted(body, whole) {
            match stmt {
                Stmt::FunctionDef(f) => {
                    f.decorator_list.iter().for_each(|e| self.visit_expr(e));
                    f.args.defaults.iter().for_each(|e| self.visit_expr(e));
                }
                Stmt::ClassDef(c) => {
                    c.decorator_list.iter().for_each(|e| self.visit_expr(e));
                    c.bases.iter().for_each(|e| self.visit_expr(e));
                }
                other => for_each_stmt_expr(other, &mut |e| self.visit_expr(e)),
            }
            let mut suites = Vec::new();
            for_each_child_body(stmt, &mut |b| suites.push(b));
            for suite in suites {
                self.suite(suite, false);
            }
        }
    }
}

/// The statements of a suite that Jython generates code for: all of the
/// module's own (`whole`), otherwise those up to the first that ends the
/// suite ([`ends_suite`]).
fn emitted(body: &[Stmt], whole: bool) -> &[Stmt] {
    match body.iter().position(ends_suite) {
        Some(i) if !whole => &body[..=i],
        _ => body,
    }
}

impl<'a> Visitor<'a> for Strings {
    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Str(s) = expr {
            let text = match &s.s {
                StringValue::Bytes(bytes) => bytes.iter().map(|&b| char::from(b)).collect(),
                StringValue::Unicode(text) => text.to_string(),
            };
            if self.distinct.insert(text)
                && self.distinct.len() * 2 > CONSTANT_POOL_LIMIT
                && self.crossing.is_none()
            {
                self.crossing = Some(s.range);
            }
        }
        walk_expr(self, expr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(source: &str) -> Option<(String, usize)> {
        mark_limit_error(source).map(|e| {
            let text = &source[e.range.start().to_usize()..e.range.end().to_usize()];
            (text.chars().take(3).collect(), text.len())
        })
    }

    #[test]
    fn first_two_lines() {
        let n = MARK_LIMIT;
        // Line 2 ends at index 100,000 or later: fails.
        let comment = |len: usize| format!("#{}\nx=1\n", "a".repeat(len));
        // "#" + len + "\n" + "x=1" ends line 2 at len + 5.
        assert!(mark(&comment(n - 6)).is_none());
        assert_eq!(mark(&comment(n - 5)), Some(("#aa".to_owned(), n - 4)));
        // A single line: the appended newlines make an empty line 2.
        let single = |len: usize| "#".repeat(len);
        assert!(mark(&single(n - 2)).is_none());
        assert!(mark(&single(n - 1)).is_some());
        assert!(mark(&(single(n - 2) + "\n")).is_none());
        assert!(mark(&(single(n - 1) + "\n")).is_some());
        // Line 2 is the long one.
        let header = "def runAction(self, event):\n";
        let body = |len: usize| format!("{header}\t#{}\n\tpass\n", "b".repeat(len));
        let fits = n - header.len() - 2;
        assert!(mark(&body(fits - 1)).is_none());
        assert_eq!(mark(&body(fits)).unwrap().0, "\t#b");
        // UTF-16 units: an astral character counts twice, é once.
        assert!(mark(&format!("#{}\n", "é".repeat(n - 3))).is_none());
        assert!(mark(&format!("#{}\n", "😀".repeat(n / 2))).is_some());
        // `\r\n` after line 1 counts twice; after line 2 only the `\r`.
        let crlf = |len: usize| format!("#{}\r\nx=1\r\n", "a".repeat(len));
        assert!(mark(&crlf(n - 7)).is_none());
        assert!(mark(&crlf(n - 6)).is_some());
        // Later lines are not limited.
        assert!(mark(&format!("x=1\ny=2\n#{}\n", "a".repeat(10 * n))).is_none());
    }

    fn sizes(source: &str) -> Vec<CompileErrorKind> {
        let parsed = iglint_python::parse_module(source);
        assert!(parsed.errors.is_empty());
        size_errors(&parsed.module, source)
            .into_iter()
            .map(|e| e.kind)
            .collect()
    }

    #[test]
    fn method_size() {
        let lines = |n: usize| "pass\n".repeat(n);
        let limit = METHOD_LIMIT / LINE_COST + CHEAP_LINES;
        assert!(sizes(&lines(limit)).is_empty());
        assert_eq!(sizes(&lines(limit + 1)), [CompileErrorKind::MethodTooLarge]);
        // Statements on one line count once (a lower bound: Jython pays for
        // each); `global` not at all.
        assert!(sizes(&"pass; ".repeat(3 * limit)).is_empty());
        assert!(sizes(&format!("def f():\n{}", "    global x\n".repeat(2 * limit))).is_empty());
        // Function bodies are their own methods; nested blocks are not.
        let function = format!("def f():\n{}", "    pass\n".repeat(limit));
        assert_eq!(
            sizes(&format!("{function}{function}")),
            [
                CompileErrorKind::MethodTooLarge,
                CompileErrorKind::MethodTooLarge
            ]
        );
        let half = "    pass\n".repeat(limit / 2 + 1);
        assert_eq!(
            sizes(&format!("if x:\n{half}else:\n{half}")),
            [CompileErrorKind::MethodTooLarge]
        );
    }

    #[test]
    fn dead_code_is_not_generated() {
        let limit = METHOD_LIMIT / LINE_COST + CHEAP_LINES;
        let passes = |indent: &str| format!("{indent}pass\n").repeat(limit + 1);
        for dead in [
            format!("def f():\n    return\n{}", passes("    ")),
            format!("def f():\n    raise X\n{}", passes("    ")),
            format!(
                "def f():\n    for i in x:\n        continue\n{}",
                passes("        ")
            ),
            format!(
                "def f():\n    if x:\n        return\n    elif y:\n        raise X\n    else:\n        return\n{}",
                passes("    ")
            ),
            format!("class C:\n    raise X\n{}", passes("    ")),
            format!("if x:\n    raise X\n{}", passes("    ")),
            format!("def f():\n    return\n    def g():\n{}", passes("        ")),
            // Dead code inside a `try` body: the `try` itself goes on.
            format!(
                "def f():\n    try:\n        return\n{}    finally:\n        pass\n",
                passes("        ")
            ),
        ] {
            assert!(sizes(&dead).is_empty(), "{}", &dead[..60]);
        }
        for live in [
            format!("raise X\n{}", passes("")),
            format!("def f():\n    if x:\n        return\n{}", passes("    ")),
            format!(
                "def f():\n    while x:\n        break\n{}",
                passes("        ")
            ),
            format!(
                "def f():\n    try:\n        return\n    except:\n        return\n{}",
                passes("    ")
            ),
            format!(
                "def f():\n    if x:\n        return\n    else:\n        pass\n{}",
                passes("    ")
            ),
            format!(
                "def f():\n    for i in x:\n        break\n{}",
                passes("        ")
            ),
            format!(
                "def f():\n    try:\n        return\n    finally:\n        pass\n{}",
                passes("    ")
            ),
            format!("def f():\n    with x:\n        return\n{}", passes("    ")),
            format!(
                "def f():\n    for i in x:\n        pass\n    else:\n        return\n{}",
                passes("    ")
            ),
        ] {
            assert_eq!(
                sizes(&live),
                [CompileErrorKind::MethodTooLarge],
                "{}",
                &live[..60]
            );
        }
        let strings: String = (0..=CONSTANT_POOL_LIMIT / 2)
            .map(|i| format!("'{i}', "))
            .collect();
        assert!(sizes(&format!("def f():\n    return\n    x = [{strings}]\n")).is_empty());
        // String statements after `return`, one per line.
        let lines: String = (0..=CONSTANT_POOL_LIMIT / 2)
            .map(|i| format!("    's{i}'\n"))
            .collect();
        assert!(sizes(&format!("def f():\n    return\n{lines}")).is_empty());
        assert_eq!(
            sizes(&format!("def f():\n    x = [{strings}]\n    return\n")),
            [CompileErrorKind::ClassTooLarge]
        );
    }

    #[test]
    fn constant_pool() {
        // Few lines, so that the module is not too large a method first.
        let packed = |n: usize| -> String {
            (0..n)
                .map(|i| format!("'{i}'"))
                .collect::<Vec<_>>()
                .chunks(1000)
                .map(|c| format!("x = [{}]\n", c.join(", ")))
                .collect()
        };
        assert!(sizes(&packed(CONSTANT_POOL_LIMIT / 2)).is_empty());
        assert_eq!(
            sizes(&packed(CONSTANT_POOL_LIMIT / 2 + 1)),
            [CompileErrorKind::ClassTooLarge]
        );
        // Equal values count once; `str` and `unicode` of the same text too.
        let same: String = (0..CONSTANT_POOL_LIMIT).map(|_| "'a', u'a', ").collect();
        assert!(sizes(&format!("x = [{same}]\n")).is_empty());
    }
}
