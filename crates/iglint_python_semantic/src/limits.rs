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
//!   (`Module or method too large`). [`code_units`] adds a lower bound of
//!   the bytecode of every statement and expression of a unit (measured
//!   on Jython's code generator, `oracle/jython_method_size_oracle.py`);
//!   a unit is reported only when the bound exceeds the limit.
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
pub fn size_errors(module: &Module, source: &str) -> Vec<CompileError> {
    let mut out = Vec::new();
    for unit in code_units(module, source) {
        let Some(crossing) = unit.crossing else {
            continue;
        };
        let what = match unit.owner {
            None => "module",
            Some(Stmt::ClassDef(_)) => "class body",
            Some(_) => "function",
        };
        let range = match unit.owner {
            Some(Stmt::FunctionDef(s)) => s.name.range(),
            Some(Stmt::ClassDef(s)) => s.name.range(),
            _ => crossing.range(),
        };
        out.push(CompileError {
            range,
            message: format!(
                "Jython cannot compile this {what}: its statements need at least {} bytes of Java bytecode, more than one Java method can hold ({METHOD_LIMIT} bytes, Module or method too large)",
                group(unit.bytes)
            ),
            anchor: None,
            kind: CompileErrorKind::MethodTooLarge,
        });
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

/// `n` with a comma between groups of three digits.
fn group(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// One code unit (a Java method): the module body, a function or class
/// body, a lambda or a generator expression (dict and set comprehensions
/// too), with a lower bound of the bytecode Jython generates for it.
#[derive(Debug)]
pub struct CodeUnit<'a> {
    /// The `def` or `class` statement; `None` for the module, lambdas and
    /// generator expressions.
    pub owner: Option<&'a Stmt>,
    /// The name Jython gives the unit's Java method (`f` for the module,
    /// lambdas and generator expressions).
    pub name: &'a str,
    /// Lower bound of the bytes of bytecode. Lambdas and generator
    /// expressions are not modelled (0).
    pub bytes: usize,
    /// The statement whose code makes the bound exceed the method limit.
    pub crossing: Option<&'a Stmt>,
}

/// Every code unit of `module` in the order Jython numbers their Java
/// methods (the module first, then each nested unit before the units
/// that follow it), with a lower bound of the bytecode of each. `source`
/// is the text `module` was parsed from.
///
/// The bound adds, for every statement Jython generates code for (see
/// [`ends_suite`]), the frame's line number update and the least code of
/// the statement and of each of its expression nodes. Each figure is the
/// least that Jython 2.7's code generator emits for that construct in any
/// context (module, function or class body, any position), measured on
/// the code generator of the Ignition 8.3.3 image with
/// `oracle/method_size.py`; `tests/method_size_oracle.rs` checks the
/// bound against the recorded size of every code unit of thousands of
/// generated programs. Code shared by a whole unit (its prologue and
/// epilogue) is not counted.
pub fn code_units<'a>(module: &'a Module, source: &str) -> Vec<CodeUnit<'a>> {
    let cost = Cost {
        index: LineIndex::new(source),
        // A lone CR ends a line for the parser but not for Jython's line
        // numbers: count every line as the cheapest then.
        cheap_lines: source
            .as_bytes()
            .iter()
            .enumerate()
            .any(|(i, &b)| b == b'\r' && source.as_bytes().get(i + 1) != Some(&b'\n')),
    };
    let mut out = Vec::new();
    cost.unit(UnitBody::Body(&module.body, true), None, "f", &mut out);
    out
}

enum UnitBody<'a> {
    /// Statements; `true` for the module's own statement list, which is
    /// compiled whole.
    Body(&'a [Stmt], bool),
    /// A lambda or generator expression: the expressions it compiles
    /// (only to find the units nested in them).
    Exprs(Vec<&'a Expr>),
}

/// A nested unit found while costing a statement.
enum Nested<'a> {
    Def(&'a Stmt),
    Exprs(Vec<&'a Expr>),
}

struct Cost {
    index: LineIndex,
    cheap_lines: bool,
}

/// Least bytes of the frame's line number update: `aload` the frame,
/// push the line (`iconst` up to 5, `bipush` up to 127, `sipush` up to
/// 32,767, `ldc` beyond) and `invokevirtual PyFrame.setline`. The
/// thresholds are taken one line later, in case Jython numbers a
/// statement one line further than its first token.
fn setline(line: usize) -> usize {
    match line {
        0..=6 => 5,
        7..=128 => 6,
        129..=32_768 => 7,
        _ => 6,
    }
}

/// Whether Jython may build the display `expr` outside the code unit: it
/// moves a list, tuple, set or dict display of 100 or more constants (a
/// dict counts its keys and values; `-1` counts as a constant) into
/// methods of its own. Taken from 90 elements, and with any unary
/// operator, names of constants and nested constant displays counted as
/// constants, to stay below the real rule.
fn moved_display(expr: &Expr) -> bool {
    fn constant(e: &Expr) -> bool {
        match e {
            Expr::Num(_) | Expr::Str(_) => true,
            Expr::UnaryOp(u) => constant(&u.operand),
            Expr::Name(n) => matches!(&*n.id, "None" | "True" | "False"),
            Expr::List(ExprList { elts, .. })
            | Expr::Tuple(ExprTuple { elts, .. })
            | Expr::Set(ExprSet { elts, .. }) => elts.iter().all(constant),
            Expr::Dict(d) => d.keys.iter().chain(&d.values).all(constant),
            _ => false,
        }
    }
    const FROM: usize = 90;
    match expr {
        Expr::List(ExprList { elts, ctx, .. }) | Expr::Tuple(ExprTuple { elts, ctx, .. }) => {
            *ctx == ExprContext::Load && elts.len() >= FROM && elts.iter().all(constant)
        }
        Expr::Set(e) => e.elts.len() >= FROM && e.elts.iter().all(constant),
        Expr::Dict(d) => 2 * d.keys.len() >= FROM && d.keys.iter().chain(&d.values).all(constant),
        _ => false,
    }
}

impl Cost {
    fn unit<'a>(
        &self,
        body: UnitBody<'a>,
        owner: Option<&'a Stmt>,
        name: &'a str,
        out: &mut Vec<CodeUnit<'a>>,
    ) {
        let at = out.len();
        out.push(CodeUnit {
            owner,
            name,
            bytes: 0,
            crossing: None,
        });
        let mut nested = Vec::new();
        match body {
            UnitBody::Body(stmts, whole) => {
                let mut bytes = 0;
                let mut crossing = None;
                for stmt in emitted(stmts, whole) {
                    bytes += self.stmt(stmt, &mut nested);
                    if bytes > METHOD_LIMIT && crossing.is_none() {
                        crossing = Some(stmt);
                    }
                }
                out[at].bytes = bytes;
                out[at].crossing = crossing;
            }
            UnitBody::Exprs(exprs) => {
                for e in exprs {
                    self.expr(e, &mut nested);
                }
            }
        }
        for n in nested {
            match n {
                Nested::Def(stmt @ Stmt::FunctionDef(f)) => self.unit(
                    UnitBody::Body(&f.body, false),
                    Some(stmt),
                    f.name.as_str(),
                    out,
                ),
                Nested::Def(stmt @ Stmt::ClassDef(c)) => self.unit(
                    UnitBody::Body(&c.body, false),
                    Some(stmt),
                    c.name.as_str(),
                    out,
                ),
                Nested::Def(_) => {}
                Nested::Exprs(exprs) => self.unit(UnitBody::Exprs(exprs), None, "f", out),
            }
        }
    }

    fn line(&self, stmt: &Stmt) -> usize {
        if self.cheap_lines {
            return 1;
        }
        self.index.line_of(stmt.range().start()) + 1
    }

    /// Least bytecode of `stmt` and the statements nested in it (not those
    /// of nested units, which are their own methods).
    fn stmt<'a>(&self, stmt: &'a Stmt, nested: &mut Vec<Nested<'a>>) -> usize {
        let line = || setline(self.line(stmt));
        let exprs = |list: &'a [Expr], nested: &mut Vec<Nested<'a>>| -> usize {
            list.iter().map(|e| self.expr(e, nested)).sum()
        };
        let opt = |e: &'a Option<Box<Expr>>, nested: &mut Vec<Nested<'a>>| -> usize {
            e.as_deref().map_or(0, |e| self.expr(e, nested))
        };
        let suite = |body: &'a [Stmt], nested: &mut Vec<Nested<'a>>| -> usize {
            emitted(body, false)
                .iter()
                .map(|s| self.stmt(s, nested))
                .sum()
        };
        match stmt {
            // `global` generates nothing; `break` and `continue` a jump
            // without a line number update.
            Stmt::Global(_) | Stmt::Continue(_) => 0,
            Stmt::Break(_) => 3,
            Stmt::Pass(_) => line(),
            Stmt::Expr(s) => line() + 1 + self.expr(&s.value, nested),
            Stmt::Assign(s) => {
                // A single name target is stored through a temporary.
                let temporary = match s.targets.as_slice() {
                    [Expr::Name(_)] => 4,
                    _ => 0,
                };
                line()
                    + temporary
                    + (s.targets.len() - 1)
                    + exprs(&s.targets, nested)
                    + self.expr(&s.value, nested)
            }
            // The target is loaded and stored again (counted once).
            Stmt::AugAssign(s) => {
                line() + 12 + self.expr(&s.target, nested) + self.expr(&s.value, nested)
            }
            Stmt::Delete(s) => line() + exprs(&s.targets, nested),
            Stmt::Print(s) => {
                line() + 3 * s.values.len().max(1) + opt(&s.dest, nested) + exprs(&s.values, nested)
            }
            // The value through a temporary, `f_lasti = -1`, `areturn`.
            Stmt::Return(s) => line() + 6 + opt(&s.value, nested),
            Stmt::Raise(s) => {
                line() + 4 + opt(&s.type_, nested) + opt(&s.inst, nested) + opt(&s.tback, nested)
            }
            Stmt::Assert(s) => line() + 25 + self.expr(&s.test, nested) + opt(&s.msg, nested),
            Stmt::Exec(s) => {
                line()
                    + 5
                    + self.expr(&s.body, nested)
                    + opt(&s.globals, nested)
                    + opt(&s.locals, nested)
            }
            Stmt::Import(s) => line() + 15 * s.names.len(),
            Stmt::ImportFrom(s) => {
                if s.names.first().is_some_and(|a| a.name.as_str() == "*") {
                    line() + 5
                } else {
                    line() + 12 + 18 * s.names.len()
                }
            }
            Stmt::If(s) => {
                line()
                    + 9
                    + self.expr(&s.test, nested)
                    + suite(&s.body, nested)
                    + suite(&s.orelse, nested)
            }
            Stmt::While(s) => {
                line()
                    + 9
                    + self.expr(&s.test, nested)
                    + suite(&s.body, nested)
                    + suite(&s.orelse, nested)
            }
            Stmt::For(s) => {
                line()
                    + 20
                    + self.expr(&s.target, nested)
                    + self.expr(&s.iter, nested)
                    + suite(&s.body, nested)
                    + suite(&s.orelse, nested)
            }
            Stmt::With(s) => {
                line()
                    + 40
                    + self.expr(&s.context_expr, nested)
                    + opt(&s.optional_vars, nested)
                    + suite(&s.body, nested)
            }
            Stmt::TryExcept(s) => {
                let mut n = line() + 6 + suite(&s.body, nested);
                for h in &s.handlers {
                    if let Some(t) = &h.type_ {
                        n += 7 + self.expr(t, nested);
                    }
                    if let Some(name) = &h.name {
                        n += 6 + self.expr(name, nested);
                    }
                    n += suite(&h.body, nested);
                }
                n + suite(&s.orelse, nested)
            }
            // The finally body is generated in the exception handler, and
            // again on the way out of a body that does not end its suite
            // (and before each `return`, `break` or `continue` that leaves
            // the `try`, not counted), nested units included.
            Stmt::TryFinally(s) => {
                let mut n = line() + 1 + suite(&s.body, nested) + suite(&s.finalbody, nested);
                if !emitted(&s.body, false).iter().any(ends_suite) {
                    n += suite(&s.finalbody, nested);
                }
                n
            }
            Stmt::FunctionDef(f) => {
                let n = line()
                    + 25
                    + 3 * (f.decorator_list.len() + f.args.defaults.len())
                    + exprs(&f.decorator_list, nested)
                    + exprs(&f.args.defaults, nested);
                nested.push(Nested::Def(stmt));
                n
            }
            Stmt::ClassDef(c) => {
                let n = line()
                    + 25
                    + 3 * (c.decorator_list.len() + c.bases.len())
                    + exprs(&c.decorator_list, nested)
                    + exprs(&c.bases, nested);
                nested.push(Nested::Def(stmt));
                n
            }
        }
    }

    /// Least bytecode of an expression and its sub-expressions (not those
    /// of a nested lambda or generator expression).
    fn expr<'a>(&self, expr: &'a Expr, nested: &mut Vec<Nested<'a>>) -> usize {
        let all = |list: &'a [Expr], nested: &mut Vec<Nested<'a>>| -> usize {
            list.iter().map(|e| self.expr(e, nested)).sum()
        };
        let comprehension = |g: &'a Comprehension, nested: &mut Vec<Nested<'a>>| -> usize {
            self.expr(&g.target, nested) + self.expr(&g.iter, nested) + all(&g.ifs, nested)
        };
        match expr {
            Expr::Name(e) => {
                if e.ctx == ExprContext::Param {
                    0
                } else {
                    5
                }
            }
            // `iconst`/`bipush`/`sipush`/`ldc` and `invokestatic
            // Py.newInteger`; `ldc` and `PyString.fromInterned`;
            // `ldc2_w` and `Py.newFloat`.
            Expr::Num(e) => match e.n {
                Number::Int(_) => 4,
                Number::Long(_) | Number::Float(_) | Number::Complex(_) => 5,
            },
            Expr::Str(_) => 5,
            Expr::Attribute(e) => {
                let own = if e.ctx == ExprContext::Store { 9 } else { 5 };
                own + self.expr(&e.value, nested)
            }
            Expr::Subscript(e) => {
                let own = if e.ctx == ExprContext::Store { 7 } else { 3 };
                own + self.expr(&e.value, nested) + self.slice(&e.slice, nested)
            }
            Expr::Call(e) => {
                let simple = e.args.len() <= 4
                    && e.keywords.is_empty()
                    && e.starargs.is_none()
                    && e.kwargs.is_none();
                let own = if simple {
                    4
                } else {
                    14 + 3 * (e.args.len() + e.keywords.len())
                };
                own + self.expr(&e.func, nested)
                    + all(&e.args, nested)
                    + e.keywords
                        .iter()
                        .map(|k| self.expr(&k.value, nested))
                        .sum::<usize>()
                    + e.starargs.as_deref().map_or(0, |a| self.expr(a, nested))
                    + e.kwargs.as_deref().map_or(0, |a| self.expr(a, nested))
            }
            Expr::UnaryOp(e) => 3 + self.expr(&e.operand, nested),
            Expr::BinOp(e) => 3 + self.expr(&e.left, nested) + self.expr(&e.right, nested),
            Expr::Repr(e) => 3 + self.expr(&e.value, nested),
            Expr::BoolOp(e) => 7 * (e.values.len().saturating_sub(1)) + all(&e.values, nested),
            Expr::Compare(e) => {
                7 * e.ops.len() + self.expr(&e.left, nested) + all(&e.comparators, nested)
            }
            // Jython fills a display of 100 or more constants in methods
            // of its own (`set$$0`, ...): here only the object, the array
            // and the call (`new`, `dup`, the length, `anewarray`, `dup`,
            // `invokestatic`, `invokespecial`, 16 bytes).
            _ if moved_display(expr) => 14,
            Expr::List(ExprList { elts, ctx, .. }) | Expr::Tuple(ExprTuple { elts, ctx, .. }) => {
                // Unpacking takes more than building.
                let each = if *ctx == ExprContext::Store { 8 } else { 3 };
                10 + each * elts.len() + all(elts, nested)
            }
            Expr::Set(e) => 10 + 3 * e.elts.len() + all(&e.elts, nested),
            Expr::Dict(e) => 10 + 6 * e.keys.len() + all(&e.keys, nested) + all(&e.values, nested),
            Expr::IfExp(e) => {
                14 + self.expr(&e.test, nested)
                    + self.expr(&e.body, nested)
                    + self.expr(&e.orelse, nested)
            }
            Expr::Yield(e) => 30 + e.value.as_deref().map_or(0, |v| self.expr(v, nested)),
            // List comprehensions are generated inline.
            Expr::ListComp(e) => {
                45 + 30 * e.generators.len()
                    + 12 * e.generators.iter().map(|g| g.ifs.len()).sum::<usize>()
                    + self.expr(&e.elt, nested)
                    + e.generators
                        .iter()
                        .map(|g| comprehension(g, nested))
                        .sum::<usize>()
            }
            // Their own units; the first iterable is evaluated here.
            Expr::Lambda(e) => {
                let n = 24 + 3 * e.args.defaults.len() + all(&e.args.defaults, nested);
                nested.push(Nested::Exprs(vec![&e.body]));
                n
            }
            Expr::GeneratorExp(ExprGeneratorExp {
                elt, generators, ..
            })
            | Expr::SetComp(ExprSetComp {
                elt, generators, ..
            }) => self.generator(40, vec![&**elt], generators, nested),
            Expr::DictComp(e) => {
                self.generator(40, vec![&*e.key, &*e.value], &e.generators, nested)
            }
        }
    }

    /// A generator expression or a set or dict comprehension: `own` bytes
    /// and the first iterable here, the rest in a nested unit.
    fn generator<'a>(
        &self,
        own: usize,
        mut inner: Vec<&'a Expr>,
        generators: &'a [Comprehension],
        nested: &mut Vec<Nested<'a>>,
    ) -> usize {
        let first = generators.first().map_or(0, |g| self.expr(&g.iter, nested));
        for (i, g) in generators.iter().enumerate() {
            inner.push(&g.target);
            if i > 0 {
                inner.push(&g.iter);
            }
            inner.extend(g.ifs.iter());
        }
        nested.push(Nested::Exprs(inner));
        own + first
    }

    fn slice<'a>(&self, slice: &'a Slice, nested: &mut Vec<Nested<'a>>) -> usize {
        match slice {
            Slice::Index(s) => self.expr(&s.value, nested),
            Slice::Ellipsis(_) => 3,
            Slice::Slice(s) => {
                3 + [&s.lower, &s.upper, &s.step]
                    .into_iter()
                    .map(|e| e.as_deref().map_or(0, |e| self.expr(e, nested)))
                    .sum::<usize>()
            }
            Slice::ExtSlice(s) => 3 + s.dims.iter().map(|d| self.slice(d, nested)).sum::<usize>(),
        }
    }
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
        // 5 bytes for lines 1 to 6, 6 to line 128, 7 beyond: 9,381 lines
        // stay within 65,535 bytes (Jython fails from 9,380).
        assert!(sizes(&lines(9_381)).is_empty());
        assert_eq!(sizes(&lines(9_382)), [CompileErrorKind::MethodTooLarge]);
        // Every statement updates the line number, also on one line.
        assert!(sizes(&"pass; ".repeat(13_107)).is_empty());
        assert_eq!(
            sizes(&"pass; ".repeat(13_108)),
            [CompileErrorKind::MethodTooLarge]
        );
        // `global` generates nothing.
        assert!(sizes(&format!("def f():\n{}", "    global x\n".repeat(30_000))).is_empty());
        // Function bodies are their own methods; nested blocks are not.
        let function = format!("def f():\n{}", "    pass\n".repeat(9_400));
        assert_eq!(
            sizes(&format!("{function}{function}")),
            [
                CompileErrorKind::MethodTooLarge,
                CompileErrorKind::MethodTooLarge
            ]
        );
        let half = "    pass\n".repeat(4_700);
        assert_eq!(
            sizes(&format!("if x:\n{half}else:\n{half}")),
            [CompileErrorKind::MethodTooLarge]
        );
        // Expressions count: 681 statements of this shape fail on Jython
        // (97 bytes each); 742 are certainly too large.
        let items = |n: usize| -> String {
            (0..n)
                .map(|i| format!("    d[\"k{i}\"] = {{\"a\": {i}, \"b\": [1, 2, 3]}}\n"))
                .collect()
        };
        assert!(sizes(&format!("def f():\n{}", items(679))).is_empty());
        assert_eq!(
            sizes(&format!("def f():\n{}", items(900))),
            [CompileErrorKind::MethodTooLarge]
        );
    }

    #[test]
    fn dead_code_is_not_generated() {
        let passes = |indent: &str| format!("{indent}pass\n").repeat(9_500);
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
