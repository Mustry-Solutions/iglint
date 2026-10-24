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
//!   a unit is reported only when the bound exceeds the limit. The upper
//!   bound ([`upper`]) mirrors the code generator itself (`mirror`).
//! * Jython generates no code for the rest of a suite after a statement
//!   that ends it (`ends_suite`): those statements, the
//!   `def`s among them and their string constants count for neither
//!   limit. The module's own statement list is compiled whole.
//! * A module becomes one Java class, limited to 65,535 constant pool
//!   entries (`Class too large`). Each distinct string constant takes at
//!   least two (the string and its UTF-8 text), so more than 32,767
//!   distinct string literals cannot compile (Jython fails at 32,594).
//!
//! Every size is a [`Bytes`], which saturates instead of wrapping: a bound
//! too large to represent is above every limit (nested `finally` bodies
//! multiply). The module denies unchecked arithmetic to keep it that way.

#![deny(clippy::arithmetic_side_effects, clippy::cast_possible_truncation)]

mod mirror;
mod scopes;
#[cfg(test)]
#[allow(clippy::arithmetic_side_effects, clippy::cast_possible_truncation)]
mod tests;

use std::num::Saturating;

use iglint_python::ast::*;
use iglint_python::visitor::{Visitor, walk_expr};
use iglint_text::{LineIndex, Ranged, TextRange, TextSize};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::compile::{
    CompileError, CompileErrorKind, ends_suite, for_each_child_body, for_each_stmt_expr,
};
use scopes::{expr_key, stmt_key};

/// A size in bytes (or a count of entries) that saturates at `u64::MAX`
/// instead of wrapping.
pub(crate) type Bytes = Saturating<u64>;

pub(crate) const fn b(n: u64) -> Bytes {
    Saturating(n)
}

pub(crate) fn count(n: usize) -> Bytes {
    Saturating(u64::try_from(n).unwrap_or(u64::MAX))
}

/// `n` as a `usize`, `usize::MAX` when it does not fit.
fn to_usize(n: Bytes) -> usize {
    usize::try_from(n.0).unwrap_or(usize::MAX)
}

/// Jython's read-ahead limit while it looks for an encoding declaration,
/// in UTF-16 code units.
pub const MARK_LIMIT: usize = 100_000;

/// The largest Java method, in bytes of bytecode.
const METHOD_LIMIT: u64 = 65_535;
/// The largest constant pool of a Java class.
const CONSTANT_POOL_LIMIT: u64 = 65_535;

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
        bytes
            .get(from..)
            .unwrap_or_default()
            .iter()
            .position(|&b| matches!(b, b'\n' | b'\r'))
            .map_or(source.len(), |i| from.saturating_add(i))
    };
    // Line ends are ASCII line breaks or the end, so `get` always succeeds.
    let units = |from: usize, to: usize| {
        source
            .get(from..to)
            .map_or(0, |line| line.encode_utf16().count())
    };
    let end1 = line_end(0);
    let (start2, terminator) = match bytes.get(end1) {
        None => (end1, 1),
        Some(b'\r') if bytes.get(end1.saturating_add(1)) == Some(&b'\n') => {
            (end1.saturating_add(2), 2)
        }
        Some(_) => (end1.saturating_add(1), 1),
    };
    let end2 = line_end(start2);
    let (units1, units2) = (units(0, end1), units(start2, end2));
    let at = units1.saturating_add(terminator).saturating_add(units2);
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
    let units = units(module, source, None);
    for unit in &units {
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
                "Jython cannot compile this {what}: its statements need at least {} bytes of Java bytecode, more than one Java method can hold ({} bytes, Module or method too large)",
                group(count(unit.bytes)),
                group(b(METHOD_LIMIT))
            ),
            anchor: None,
            kind: CompileErrorKind::MethodTooLarge,
        });
    }
    let (init, crossing) = init_lower_of(&units);
    if init.0 > METHOD_LIMIT {
        out.push(CompileError {
            range: crossing.map_or_else(|| TextRange::empty(TextSize::ZERO), |s| match s {
                Stmt::FunctionDef(f) => f.name.range(),
                Stmt::ClassDef(c) => c.name.range(),
                other => other.range(),
            }),
            message: format!(
                "Jython cannot compile this module: it has too many functions, classes, lambdas and generator expressions (code objects) for Jython's class initializer, which needs at least {} bytes of Java bytecode for them, more than one Java method can hold ({} bytes, Module or method too large)",
                group(init),
                group(b(METHOD_LIMIT))
            ),
            anchor: None,
            kind: CompileErrorKind::MethodTooLarge,
        });
    }
    let mut strings = Strings::default();
    strings.suite(&module.body, true);
    if count(strings.distinct.len()) * b(2) > b(CONSTANT_POOL_LIMIT)
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

/// Why Jython's verdict on the size of the module is not known, if it is
/// not: a Java method (a code unit, or the class's `<init>`, which builds
/// a code object per unit, `call_function`, which dispatches to them, or
/// a method that fills a large constant display) whose lower bound fits a
/// Java method and whose upper bound does not, or a constant pool whose
/// bounds lie on both sides of its limit. The lower bounds are those
/// `SCR002` reports from ([`size_errors`]; `call_function` and the display
/// methods have none), the upper bounds are [`upper`]'s. A fix that
/// changes such a module may change whether it compiles without the
/// bounds seeing it, so the safety net does not trust it.
pub fn size_unknown(module: &Module, source: &str) -> Option<String> {
    let report = mirror::report(module);
    if report.saturated {
        // Too much to mirror (nested `finally` bodies are copied at every
        // way out, exponentially): nothing is known, not even that the
        // failure Jython reports is the one `size_errors` names.
        return Some(
            "the code Jython would generate for it is too large to bound (nested `finally` bodies are copied at every way out of their `try`), so whether and how Jython fails to compile it is not known"
                .to_owned(),
        );
    }
    let units = units(module, source, Some(&report));
    for unit in &units {
        if count(unit.bytes).0 <= METHOD_LIMIT && count(unit.upper).0 > METHOD_LIMIT {
            let what = match unit.owner {
                None if unit.name == "f" && unit.bytes == 0 => "lambda or generator expression",
                None => "module",
                Some(Stmt::ClassDef(_)) => "class body",
                Some(_) => "function",
            };
            return Some(format!(
                "the {what} `{}` needs between {} and {} bytes of Java bytecode, so whether Jython can compile it ({} bytes per method) is not known",
                unit.name,
                group(count(unit.bytes)),
                group(count(unit.upper)),
                group(b(METHOD_LIMIT))
            ));
        }
    }
    let class = upper::class_of(module, source, &report);
    let (init_lower, _) = init_lower_of(&units);
    if init_lower.0 > METHOD_LIMIT {
        // Certainly too large: decided, `size_errors` reports it.
        return None;
    }
    for (method, bytes) in [
        ("<init>", class.init),
        ("call_function", class.call_function),
        ("set$$", class.displays),
    ] {
        if count(bytes).0 > METHOD_LIMIT {
            return Some(format!(
                "the module's Java method `{method}` (one entry per code unit) may need up to {} bytes of Java bytecode, so whether Jython can compile the module ({} bytes per method) is not known",
                group(count(bytes)),
                group(b(METHOD_LIMIT))
            ));
        }
    }
    let mut strings = Strings::default();
    strings.suite(&module.body, true);
    let lower = count(strings.distinct.len()) * b(2);
    if lower.0 <= CONSTANT_POOL_LIMIT && count(class.pool).0 > CONSTANT_POOL_LIMIT {
        return Some(format!(
            "the module's constant pool needs between {} and {} entries, so whether Jython can compile it ({} entries per class) is not known",
            group(lower),
            group(count(class.pool)),
            group(b(CONSTANT_POOL_LIMIT))
        ));
    }
    None
}

/// Least bytes of `iconst(v)`: 1 up to 5, 2 up to 127, 3 beyond (`sipush`,
/// or `ldc` of an `Integer`).
fn iconst_least(v: usize) -> u64 {
    match v {
        0..=5 => 1,
        6..=127 => 2,
        _ => 3,
    }
}

/// A lower bound of the bytecode of the module's `<init>` and the `def`
/// or `class` statement (of the module's own code) whose code object makes
/// it exceed the method limit: every unit Jython generates
/// ([`code_units`]) counts once, a function with its parameters.
pub fn init_lower<'a>(module: &'a Module, source: &str) -> (usize, Option<&'a Stmt>) {
    let (bytes, crossing) = init_lower_of(&units(module, source, None));
    (to_usize(bytes), crossing)
}

fn init_lower_of<'a>(units: &[CodeUnit<'a>]) -> (Bytes, Option<&'a Stmt>) {
    let mut bytes = b(0);
    let mut crossing = None;
    for (id, unit) in units.iter().enumerate() {
        let params = match unit.owner {
            Some(Stmt::FunctionDef(f)) => {
                fn names(e: &Expr) -> usize {
                    match e {
                        Expr::Tuple(t) => t.elts.iter().map(names).fold(0, usize::saturating_add),
                        Expr::List(t) => t.elts.iter().map(names).fold(0, usize::saturating_add),
                        _ => 1,
                    }
                }
                // Parameters, and the names the body binds by assignment
                // (local variables unless declared global; a name both
                // ways counts once).
                let mut locals: FxHashSet<&str> = FxHashSet::default();
                let mut globals: FxHashSet<&str> = FxHashSet::default();
                for stmt in emitted(&f.body, false) {
                    match stmt {
                        Stmt::Global(g) => globals.extend(g.names.iter().map(|n| n.as_str())),
                        Stmt::FunctionDef(d) => {
                            locals.insert(d.name.as_str());
                        }
                        Stmt::ClassDef(c) => {
                            locals.insert(c.name.as_str());
                        }
                        Stmt::Assign(a) => {
                            for t in &a.targets {
                                if let Expr::Name(n) = t {
                                    locals.insert(&n.id);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                let params: FxHashSet<&str> = f
                    .args
                    .args
                    .iter()
                    .filter_map(|a| match a {
                        Expr::Name(n) => Some(&*n.id),
                        _ => None,
                    })
                    .collect();
                let assigned = locals
                    .iter()
                    .filter(|n| !globals.contains(*n) && !params.contains(*n))
                    .count();
                f.args
                    .args
                    .iter()
                    .map(names)
                    .fold(0, usize::saturating_add)
                    .saturating_add(usize::from(f.args.vararg.is_some()))
                    .saturating_add(usize::from(f.args.kwarg.is_some()))
                    .saturating_add(assigned)
            }
            _ => 0,
        };
        bytes += init_least(id, params);
        if bytes.0 > METHOD_LIMIT && crossing.is_none() {
            crossing = unit.owner;
        }
    }
    (bytes, crossing)
}

/// Least bytes of `PyCodeConstant.put` in `<init>` for the unit numbered
/// `id` with `names` variable names: `iconst` of the argument count, the
/// names array (`iconst`, `anewarray`, `astore_1`, per name `aload_1`,
/// `iconst`, `ldc`, `aastore`), `aload_1`, `aload_1`, `ldc` of the name,
/// `iconst` of the line, two `iconst`, `getstatic`, `iconst` of `id`,
/// the cell and free variable arrays (`aconst_null` at least), two
/// `iconst`, `invokestatic Py.newCode`, `putstatic`. `ldc` takes at least
/// 2 bytes; Jython numbers every unit at least as late as [`code_units`]
/// lists it (it compiles some twice).
fn init_least(id: usize, names: usize) -> Bytes {
    let mut n = b(1 + 1 + 3 + 1);
    for i in 0..names {
        n += b(4) + b(iconst_least(i));
    }
    n + b(1 + 1 + 2 + 1 + 1 + 1 + 3 + 1 + 1 + 1 + 1 + 3 + 3) + b(iconst_least(id))
}

/// `n` with a comma between groups of three digits; sizes beyond any
/// real script are shown as "more than 1,000,000,000,000".
fn group(n: Bytes) -> String {
    if n.0 > 1_000_000_000_000 {
        return "more than 1,000,000,000,000".to_owned();
    }
    let digits = n.0.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && digits.len().saturating_sub(i).is_multiple_of(3) {
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
    /// Upper bound of the bytes of bytecode ([`upper`]): Jython never
    /// generates more for the unit (for a unit Jython compiles several
    /// times, more for none of the copies); `usize::MAX` when the bound
    /// is not representable.
    pub upper: usize,
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
    let report = mirror::report(module);
    units(module, source, Some(&report))
}

/// The units with their lower bounds, and their upper bounds from
/// `report` (0 without one).
fn units<'a>(
    module: &'a Module,
    source: &str,
    report: Option<&mirror::Report>,
) -> Vec<CodeUnit<'a>> {
    let mut uppers: FxHashMap<usize, Bytes> = FxHashMap::default();
    if let Some(report) = report {
        for m in &report.methods {
            let entry = uppers.entry(m.key).or_insert(b(0));
            *entry = (*entry).max(if report.saturated {
                b(u64::MAX)
            } else {
                m.bytes
            });
        }
    }
    let cost = Cost {
        // Jython's line numbers: a lone CR does not end a line.
        index: LineIndex::python(source),
        uppers,
        saturated: report.is_some_and(|r| r.saturated),
    };
    let mut out = Vec::new();
    cost.unit(
        UnitBody::Body(&module.body, true),
        std::ptr::from_ref(module).addr(),
        None,
        "f",
        &mut out,
    );
    out
}

pub(crate) enum UnitBody<'a> {
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
    /// A lambda or generator expression (its key) and the expressions it
    /// compiles.
    Exprs(usize, Vec<&'a Expr>),
}

struct Cost {
    index: LineIndex,
    /// Upper bounds by unit key.
    uppers: FxHashMap<usize, Bytes>,
    saturated: bool,
}

/// Least bytes of the frame's line number update: `aload` the frame,
/// push the line (`iconst` up to 5, `bipush` up to 127, `sipush` up to
/// 32,767, `ldc` beyond) and `invokevirtual PyFrame.setline`. The
/// thresholds are taken one line later, in case Jython numbers a
/// statement one line further than its first token.
fn setline(line: usize) -> u64 {
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
        Expr::Dict(d) => {
            d.keys.len().saturating_mul(2) >= FROM && d.keys.iter().chain(&d.values).all(constant)
        }
        _ => false,
    }
}

impl Cost {
    fn upper(&self, key: usize) -> usize {
        if self.saturated {
            return usize::MAX;
        }
        self.uppers.get(&key).copied().map_or(0, to_usize)
    }

    fn unit<'a>(
        &self,
        body: UnitBody<'a>,
        key: usize,
        owner: Option<&'a Stmt>,
        name: &'a str,
        out: &mut Vec<CodeUnit<'a>>,
    ) {
        let at = out.len();
        out.push(CodeUnit {
            owner,
            name,
            bytes: 0,
            upper: self.upper(key),
            crossing: None,
        });
        let mut nested = Vec::new();
        match body {
            UnitBody::Body(stmts, whole) => {
                let mut bytes = b(0);
                let mut crossing = None;
                for stmt in emitted(stmts, whole) {
                    bytes += self.stmt(stmt, &mut nested);
                    if bytes.0 > METHOD_LIMIT && crossing.is_none() {
                        crossing = Some(stmt);
                    }
                }
                if let Some(unit) = out.get_mut(at) {
                    unit.bytes = to_usize(bytes);
                    unit.crossing = crossing;
                }
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
                    stmt_key(stmt),
                    Some(stmt),
                    f.name.as_str(),
                    out,
                ),
                Nested::Def(stmt @ Stmt::ClassDef(c)) => self.unit(
                    UnitBody::Body(&c.body, false),
                    stmt_key(stmt),
                    Some(stmt),
                    c.name.as_str(),
                    out,
                ),
                Nested::Def(_) => {}
                Nested::Exprs(key, exprs) => self.unit(UnitBody::Exprs(exprs), key, None, "f", out),
            }
        }
    }

    fn line(&self, stmt: &Stmt) -> usize {
        self.index.line_of(stmt.range().start()).saturating_add(1)
    }

    /// Least bytecode of `stmt` and the statements nested in it (not those
    /// of nested units, which are their own methods).
    fn stmt<'a>(&self, stmt: &'a Stmt, nested: &mut Vec<Nested<'a>>) -> Bytes {
        let line = || b(setline(self.line(stmt)));
        let exprs = |list: &'a [Expr], nested: &mut Vec<Nested<'a>>| -> Bytes {
            list.iter()
                .map(|e| self.expr(e, nested))
                .fold(b(0), |a, x| a + x)
        };
        let opt = |e: &'a Option<Box<Expr>>, nested: &mut Vec<Nested<'a>>| -> Bytes {
            e.as_deref().map_or(b(0), |e| self.expr(e, nested))
        };
        let suite = |body: &'a [Stmt], nested: &mut Vec<Nested<'a>>| -> Bytes {
            emitted(body, false)
                .iter()
                .map(|s| self.stmt(s, nested))
                .fold(b(0), |a, x| a + x)
        };
        match stmt {
            // `global` generates nothing; `break` and `continue` a jump
            // without a line number update.
            Stmt::Global(_) | Stmt::Continue(_) => b(0),
            Stmt::Break(_) => b(3),
            Stmt::Pass(_) => line(),
            Stmt::Expr(s) => line() + b(1) + self.expr(&s.value, nested),
            Stmt::Assign(s) => {
                // A single target is stored through a new temporary
                // (`set`: `astore`, the store, `aconst_null`, `astore`),
                // several through one (`astore`).
                let temporary = if s.targets.len() == 1 { 3 } else { 1 };
                line() + b(temporary) + exprs(&s.targets, nested) + self.expr(&s.value, nested)
            }
            // The target is loaded and stored again (counted once).
            Stmt::AugAssign(s) => {
                line() + b(12) + self.expr(&s.target, nested) + self.expr(&s.value, nested)
            }
            Stmt::Delete(s) => line() + exprs(&s.targets, nested),
            Stmt::Print(s) => {
                line()
                    + b(3) * count(s.values.len().max(1))
                    + opt(&s.dest, nested)
                    + exprs(&s.values, nested)
            }
            // The value through a temporary, `f_lasti = -1`, `areturn`.
            Stmt::Return(s) => line() + b(6) + opt(&s.value, nested),
            Stmt::Raise(s) => {
                line() + b(4) + opt(&s.type_, nested) + opt(&s.inst, nested) + opt(&s.tback, nested)
            }
            Stmt::Assert(s) => line() + b(25) + self.expr(&s.test, nested) + opt(&s.msg, nested),
            // `invokestatic Py.exec`, `aconst_null` for a missing part.
            Stmt::Exec(s) => {
                line()
                    + b(3)
                    + count(usize::from(s.globals.is_none()))
                    + count(usize::from(s.locals.is_none()))
                    + self.expr(&s.body, nested)
                    + opt(&s.globals, nested)
                    + opt(&s.locals, nested)
            }
            Stmt::Import(s) => line() + b(15) * count(s.names.len()),
            Stmt::ImportFrom(s) => {
                if s.names.first().is_some_and(|a| a.name.as_str() == "*") {
                    line() + b(5)
                } else {
                    line() + b(12) + b(17) * count(s.names.len())
                }
            }
            // The line of the test, `invokevirtual __nonzero__`, `ifeq`,
            // and a `goto` past the `else` after a body that does not end.
            Stmt::If(s) => {
                let goto = if emitted(&s.body, false).iter().any(ends_suite) {
                    0
                } else {
                    3
                };
                line()
                    + b(6)
                    + b(goto)
                    + self.expr(&s.test, nested)
                    + suite(&s.body, nested)
                    + suite(&s.orelse, nested)
            }
            Stmt::While(s) => {
                line()
                    + b(9)
                    + self.expr(&s.test, nested)
                    + suite(&s.body, nested)
                    + suite(&s.orelse, nested)
            }
            Stmt::For(s) => {
                line()
                    + b(20)
                    + self.expr(&s.target, nested)
                    + self.expr(&s.iter, nested)
                    + suite(&s.body, nested)
                    + suite(&s.orelse, nested)
            }
            // No line number update: the manager, `__enter__` (12 bytes),
            // the exception handler (23), and after a body that does not
            // end the inlined exit and a `goto` (12).
            Stmt::With(s) => {
                let exit = if emitted(&s.body, false).iter().any(ends_suite) {
                    0
                } else {
                    12
                };
                b(35)
                    + b(exit)
                    + self.expr(&s.context_expr, nested)
                    + opt(&s.optional_vars, nested)
                    + suite(&s.body, nested)
            }
            // No line number update: `aload_1`, `invokestatic
            // setException`, `astore`, the final `aload`, `athrow`, a
            // `goto` after a body that does not end and after every
            // handler.
            Stmt::TryExcept(s) => {
                let goto = if emitted(&s.body, false).iter().any(ends_suite) {
                    0
                } else {
                    3
                };
                let mut n = b(7) + b(goto) + suite(&s.body, nested);
                for h in &s.handlers {
                    n += b(3);
                    if let Some(t) = &h.type_ {
                        n += b(7) + self.expr(t, nested);
                    }
                    if let Some(name) = &h.name {
                        n += b(6) + self.expr(name, nested);
                    }
                    n += suite(&h.body, nested);
                }
                n + suite(&s.orelse, nested)
            }
            // The finally body is generated in the exception handler, and
            // again on the way out of a body that does not end its suite
            // (and before each `return`, `break` or `continue` that leaves
            // the `try`, not counted). Its code is costed once (its nested
            // units listed once) and counted per copy.
            Stmt::TryFinally(s) => {
                let finalbody = suite(&s.finalbody, nested);
                let copies = if emitted(&s.body, false).iter().any(ends_suite) {
                    1
                } else {
                    2
                };
                b(6) + suite(&s.body, nested) + finalbody * b(copies)
            }
            Stmt::FunctionDef(f) => {
                let n = line()
                    + b(25)
                    + b(3) * count(f.decorator_list.len().saturating_add(f.args.defaults.len()))
                    + exprs(&f.decorator_list, nested)
                    + exprs(&f.args.defaults, nested);
                nested.push(Nested::Def(stmt));
                n
            }
            Stmt::ClassDef(c) => {
                let n = line()
                    + b(25)
                    + b(3) * count(c.decorator_list.len().saturating_add(c.bases.len()))
                    + exprs(&c.decorator_list, nested)
                    + exprs(&c.bases, nested);
                nested.push(Nested::Def(stmt));
                n
            }
        }
    }

    /// Least bytecode of an expression and its sub-expressions (not those
    /// of a nested lambda or generator expression).
    fn expr<'a>(&self, expr: &'a Expr, nested: &mut Vec<Nested<'a>>) -> Bytes {
        let all = |list: &'a [Expr], nested: &mut Vec<Nested<'a>>| -> Bytes {
            list.iter()
                .map(|e| self.expr(e, nested))
                .fold(b(0), |a, x| a + x)
        };
        let comprehension = |g: &'a Comprehension, nested: &mut Vec<Nested<'a>>| -> Bytes {
            self.expr(&g.target, nested) + self.expr(&g.iter, nested) + all(&g.ifs, nested)
        };
        match expr {
            // `aload_1`, `iconst` or `ldc`, `invokevirtual`; a store also
            // loads the value (`aload`).
            Expr::Name(e) => match e.ctx {
                ExprContext::Param => b(0),
                ExprContext::Store => b(6),
                _ => b(5),
            },
            // `iconst`/`bipush`/`sipush`/`ldc` and `invokestatic
            // Py.newInteger`; `ldc` and `PyString.fromInterned`;
            // `ldc2_w` and `Py.newFloat`.
            Expr::Num(e) => match e.n {
                Number::Int(_) => b(4),
                Number::Long(_) | Number::Float(_) | Number::Complex(_) => b(5),
            },
            Expr::Str(_) => b(5),
            // A store: `ldc`, `aload` of the value, `invokevirtual`.
            Expr::Attribute(e) => {
                let own = if e.ctx == ExprContext::Store { 6 } else { 5 };
                b(own) + self.expr(&e.value, nested)
            }
            Expr::Subscript(e) => {
                let own = if e.ctx == ExprContext::Store { 4 } else { 3 };
                // A simple slice is passed to `__getslice__` as its parts,
                // `aconst_null` for a missing one.
                let slice = match &*e.slice {
                    Slice::Slice(s) => [&s.lower, &s.upper, &s.step]
                        .into_iter()
                        .map(|p| p.as_deref().map_or(b(1), |p| self.expr(p, nested)))
                        .fold(b(0), |a, x| a + x),
                    other => self.slice(other, nested),
                };
                b(own) + self.expr(&e.value, nested) + slice
            }
            Expr::Call(e) => {
                let simple = e.args.len() <= 4
                    && e.keywords.is_empty()
                    && e.starargs.is_none()
                    && e.kwargs.is_none();
                let own = if simple {
                    b(4)
                } else {
                    b(14) + b(3) * count(e.args.len().saturating_add(e.keywords.len()))
                };
                own + self.expr(&e.func, nested)
                    + all(&e.args, nested)
                    + e.keywords
                        .iter()
                        .map(|k| self.expr(&k.value, nested))
                        .fold(b(0), |a, x| a + x)
                    + e.starargs.as_deref().map_or(b(0), |a| self.expr(a, nested))
                    + e.kwargs.as_deref().map_or(b(0), |a| self.expr(a, nested))
            }
            Expr::UnaryOp(e) => b(3) + self.expr(&e.operand, nested),
            Expr::BinOp(e) => b(3) + self.expr(&e.left, nested) + self.expr(&e.right, nested),
            Expr::Repr(e) => b(3) + self.expr(&e.value, nested),
            Expr::BoolOp(e) => {
                b(7) * count(e.values.len().saturating_sub(1)) + all(&e.values, nested)
            }
            Expr::Compare(e) => {
                b(7) * count(e.ops.len()) + self.expr(&e.left, nested) + all(&e.comparators, nested)
            }
            // Jython fills a display of 100 or more constants in methods
            // of its own (`set$$0`, ...): here only the object, the array
            // and the call (`new`, `dup`, the length, `anewarray`, `dup`,
            // `invokestatic`, `invokespecial`, 16 bytes).
            _ if moved_display(expr) => b(14),
            Expr::List(ExprList { elts, ctx, .. }) | Expr::Tuple(ExprTuple { elts, ctx, .. }) => {
                match ctx {
                    // `seqSet`: `aload`, `iconst`, `invokestatic
                    // unpackSequence`, `astore`; per element `aload`,
                    // `iconst`, `aaload` and a `set` of it (`astore`, the
                    // store, `aconst_null`, `astore`).
                    ExprContext::Store => b(6) + b(6) * count(elts.len()) + all(elts, nested),
                    // `seqDel`: only the elements.
                    ExprContext::Del => all(elts, nested),
                    _ => b(10) + b(3) * count(elts.len()) + all(elts, nested),
                }
            }
            Expr::Set(e) => b(10) + b(3) * count(e.elts.len()) + all(&e.elts, nested),
            Expr::Dict(e) => {
                b(10) + b(6) * count(e.keys.len()) + all(&e.keys, nested) + all(&e.values, nested)
            }
            Expr::IfExp(e) => {
                b(14)
                    + self.expr(&e.test, nested)
                    + self.expr(&e.body, nested)
                    + self.expr(&e.orelse, nested)
            }
            Expr::Yield(e) => b(30) + e.value.as_deref().map_or(b(0), |v| self.expr(v, nested)),
            // List comprehensions are generated inline.
            Expr::ListComp(e) => {
                b(45)
                    + b(26) * count(e.generators.len())
                    + b(12) * count(e.generators.iter().map(|g| g.ifs.len()).sum::<usize>())
                    + self.expr(&e.elt, nested)
                    + e.generators
                        .iter()
                        .map(|g| comprehension(g, nested))
                        .fold(b(0), |a, x| a + x)
            }
            // Their own units; the first iterable is evaluated here.
            Expr::Lambda(e) => {
                let n = b(24) + b(3) * count(e.args.defaults.len()) + all(&e.args.defaults, nested);
                nested.push(Nested::Exprs(expr_key(expr), vec![&e.body]));
                n
            }
            Expr::GeneratorExp(ExprGeneratorExp {
                elt, generators, ..
            })
            | Expr::SetComp(ExprSetComp {
                elt, generators, ..
            }) => self.generator(expr, b(40), vec![&**elt], generators, nested),
            Expr::DictComp(e) => {
                self.generator(expr, b(40), vec![&*e.key, &*e.value], &e.generators, nested)
            }
        }
    }

    /// A generator expression or a set or dict comprehension: `own` bytes
    /// and the first iterable here, the rest in a nested unit.
    fn generator<'a>(
        &self,
        expr: &'a Expr,
        own: Bytes,
        mut inner: Vec<&'a Expr>,
        generators: &'a [Comprehension],
        nested: &mut Vec<Nested<'a>>,
    ) -> Bytes {
        let first = generators
            .first()
            .map_or(b(0), |g| self.expr(&g.iter, nested));
        for (i, g) in generators.iter().enumerate() {
            inner.push(&g.target);
            if i > 0 {
                inner.push(&g.iter);
            }
            inner.extend(g.ifs.iter());
        }
        nested.push(Nested::Exprs(expr_key(expr), inner));
        own + first
    }

    fn slice<'a>(&self, slice: &'a Slice, nested: &mut Vec<Nested<'a>>) -> Bytes {
        match slice {
            Slice::Index(s) => self.expr(&s.value, nested),
            Slice::Ellipsis(_) => b(3),
            Slice::Slice(s) => {
                b(3) + [&s.lower, &s.upper, &s.step]
                    .into_iter()
                    .map(|e| e.as_deref().map_or(b(0), |e| self.expr(e, nested)))
                    .fold(b(0), |a, x| a + x)
            }
            Slice::ExtSlice(s) => {
                b(3) + s
                    .dims
                    .iter()
                    .map(|d| self.slice(d, nested))
                    .fold(b(0), |a, x| a + x)
            }
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
        Some(i) if !whole => body.get(..=i).unwrap_or(body),
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
                && count(self.distinct.len()) * b(2) > b(CONSTANT_POOL_LIMIT)
                && self.crossing.is_none()
            {
                self.crossing = Some(s.range);
            }
        }
        walk_expr(self, expr);
    }
}

/// Sound upper bounds of what Jython's code generator emits.
///
/// The bytecode of every Java method comes from a mirror of Jython 2.7's
/// code generator (`mirror`, derived from `CodeCompiler` and the classes
/// it uses; the derivation is in `docs/internal/fix-safety.md`, "Size
/// limits"): it counts the largest encoding of every instruction each
/// visitor method emits, every loop over a collection (closure cells per
/// free variable and creation, arguments, keywords, defaults, decorators,
/// bases, display elements, unpacking targets, comparators, handlers,
/// import names, the locals and stack a `yield` saves) and every copy
/// Jython makes (a `finally` body at each way out of its `try`, a nested
/// unit once per copy of the code that creates it). The constant pool is
/// counted from every distinct string and name, numbers, compiled units,
/// long lines and the runtime's own entries, with a twentieth on top.
/// `tests/method_size_oracle.rs` checks the bounds against the measured
/// size of every Java method and the constant pool count of thousands of
/// generated programs, including adversarial shapes for every term of
/// the derivation, and reports the margins.
pub mod upper {
    use std::borrow::Cow;

    use iglint_python::ast::*;
    use iglint_text::Ranged;
    use rustc_hash::FxHashSet;

    use super::{Bytes, b, count, mirror, to_usize};

    /// Upper bounds of what the module's Java class holds besides its code
    /// units.
    #[derive(Debug, Default)]
    pub struct Class {
        /// `<init>`: a code object per compiled unit, with its variable
        /// names.
        pub init: usize,
        /// `call_function`: a branch per compiled unit.
        pub call_function: usize,
        /// The constant pool entries.
        pub pool: usize,
        /// The largest method that fills a display of constants Jython
        /// builds outside the unit (`set$$0`, ...).
        pub displays: usize,
    }

    /// Upper bounds of the module's `<init>`, `call_function`, display
    /// methods and constant pool.
    pub fn class(module: &Module, source: &str) -> Class {
        class_of(module, source, &mirror::report(module))
    }

    /// Every Java method Jython generates for a code unit of `module`, in
    /// Jython's order (`name$id`, or `f$id` for the module, lambdas and
    /// generator expressions), with an upper bound of its bytecode
    /// (`usize::MAX` when the bound is not representable). A unit inside
    /// a `finally` body is listed once per copy Jython compiles.
    pub fn methods(module: &Module) -> Vec<(String, usize)> {
        let report = mirror::report(module);
        report
            .methods
            .into_iter()
            .map(|m| {
                let bytes = if report.saturated {
                    usize::MAX
                } else {
                    to_usize(m.bytes)
                };
                (m.name, bytes)
            })
            .collect()
    }

    pub(super) fn class_of(module: &Module, source: &str, report: &mirror::Report) -> Class {
        let mut texts = Texts {
            index: iglint_text::LineIndex::python(source),
            texts: FxHashSet::default(),
            chunks: b(0),
            synthetic: b(0),
            numbers: FxHashSet::default(),
            lines: FxHashSet::default(),
            class: None,
        };
        texts.stmts(&module.body);
        let entries = b(2) * count(texts.texts.len())
            + b(2) * texts.chunks
            + b(2) * texts.synthetic
            + b(4) * count(texts.numbers.len())
            + b(24) * count(report.methods.len())
            + count(texts.lines.len());
        let saturate = |n: Bytes| {
            if report.saturated {
                usize::MAX
            } else {
                to_usize(n)
            }
        };
        Class {
            init: saturate(report.init),
            call_function: saturate(report.call_function),
            // 600 entries for the runtime's classes, methods and fields
            // and a twentieth on top as the margin (the measured class of
            // 32,600 distinct strings needs 2 entries each and 300 more).
            pool: saturate(b(600) + entries + b(entries.0 / 20)),
            displays: saturate(report.setters),
        }
    }

    /// The texts of the constant pool: every distinct name (also mangled
    /// in classes), attribute, keyword, imported name and string, chunks
    /// of long strings, the names Jython makes (`_[n]`, `_(n)`, `_(x)`,
    /// `(a, b)`), numbers and line numbers beyond `sipush`.
    struct Texts<'a> {
        index: iglint_text::LineIndex,
        texts: FxHashSet<Cow<'a, str>>,
        chunks: Bytes,
        synthetic: Bytes,
        numbers: FxHashSet<String>,
        lines: FxHashSet<usize>,
        class: Option<&'a str>,
    }

    impl<'a> Texts<'a> {
        fn text(&mut self, t: &'a str) {
            if let Some(class) = self.class
                && t.starts_with("__")
                && !t.ends_with("__")
            {
                self.texts
                    .insert(Cow::Owned(format!("_{}{t}", class.trim_start_matches('_'))));
            }
            self.texts.insert(Cow::Borrowed(t));
        }

        fn stmts(&mut self, body: &'a [Stmt]) {
            for s in body {
                self.stmt(s);
            }
        }

        fn args(&mut self, args: &'a Arguments) {
            for a in &args.args {
                if !matches!(a, Expr::Name(_)) {
                    self.synthetic += b(1);
                }
                self.expr(a);
            }
            for n in args.vararg.iter().chain(&args.kwarg) {
                self.text(n.as_str());
            }
            args.defaults.iter().for_each(|e| self.expr(e));
        }

        fn stmt(&mut self, s: &'a Stmt) {
            let line = self.index.line_of(s.range().start()).saturating_add(1);
            if line > 32_000 {
                self.lines.insert(line);
            }
            match s {
                Stmt::FunctionDef(f) => {
                    self.text(f.name.as_str());
                    f.decorator_list.iter().for_each(|e| self.expr(e));
                    self.args(&f.args);
                    self.stmts(&f.body);
                }
                Stmt::ClassDef(c) => {
                    self.text(c.name.as_str());
                    c.decorator_list.iter().for_each(|e| self.expr(e));
                    c.bases.iter().for_each(|e| self.expr(e));
                    let outer = self.class.replace(c.name.as_str());
                    self.stmts(&c.body);
                    self.class = outer;
                }
                Stmt::Import(i) => {
                    for a in &i.names {
                        for part in a.name.as_str().split('.') {
                            self.text(part);
                        }
                        self.text(a.name.as_str());
                        if let Some(n) = &a.asname {
                            self.text(n.as_str());
                        }
                    }
                }
                Stmt::ImportFrom(i) => {
                    if let Some(m) = &i.module {
                        self.text(m.as_str());
                    }
                    for a in &i.names {
                        self.text(a.name.as_str());
                        if let Some(n) = &a.asname {
                            self.text(n.as_str());
                        }
                    }
                }
                Stmt::Global(g) => {
                    for n in &g.names {
                        self.text(n.as_str());
                    }
                }
                other => {
                    crate::compile::for_each_stmt_expr(other, &mut |e| self.expr(e));
                    if let Stmt::TryExcept(t) = other {
                        for h in &t.handlers {
                            h.type_.iter().for_each(|e| self.expr(e));
                            h.name.iter().for_each(|e| self.expr(e));
                        }
                    }
                    let mut suites = Vec::new();
                    crate::compile::for_each_child_body(other, &mut |b| suites.push(b));
                    for b in suites {
                        self.stmts(b);
                    }
                }
            }
        }

        fn expr(&mut self, e: &'a Expr) {
            match e {
                Expr::Name(n) => self.text(&n.id),
                Expr::Attribute(a) => {
                    self.text(a.attr.as_str());
                    self.expr(&a.value);
                }
                Expr::Str(s) => {
                    let (text, len): (Cow<'a, str>, usize) = match &s.s {
                        StringValue::Bytes(bytes) => (
                            bytes
                                .iter()
                                .map(|&c| char::from(c))
                                .collect::<String>()
                                .into(),
                            bytes.len(),
                        ),
                        StringValue::Unicode(u) => (Cow::Borrowed(&**u), u.encode_utf16().count()),
                    };
                    self.chunks += count(len / 16_000);
                    self.texts.insert(text);
                }
                Expr::Num(n) => {
                    self.numbers.insert(format!("{:?}", n.n));
                }
                Expr::Call(c) => {
                    for k in &c.keywords {
                        self.text(k.arg.as_str());
                    }
                    iglint_python::visitor::walk_expr(self, e);
                }
                Expr::Lambda(l) => {
                    self.args(&l.args);
                    self.expr(&l.body);
                }
                Expr::ListComp(_) => {
                    self.synthetic += b(1);
                    iglint_python::visitor::walk_expr(self, e);
                }
                Expr::GeneratorExp(_) | Expr::SetComp(_) | Expr::DictComp(_) => {
                    // `_(n)` and the parameter `_(x)`.
                    self.synthetic += b(2);
                    iglint_python::visitor::walk_expr(self, e);
                }
                _ => iglint_python::visitor::walk_expr(self, e),
            }
        }
    }

    impl<'a> iglint_python::visitor::Visitor<'a> for Texts<'a> {
        fn visit_expr(&mut self, e: &'a Expr) {
            self.expr(e);
        }
    }
}
