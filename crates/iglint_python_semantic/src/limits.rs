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
    // Line ends are ASCII line breaks or the end, so `get` always succeeds.
    let units = |from: usize, to: usize| {
        source
            .get(from..to)
            .map_or(0, |line| line.encode_utf16().count())
    };
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
    let (init, crossing) = init_lower(module, source);
    if init > METHOD_LIMIT {
        out.push(CompileError {
            range: crossing.map_or_else(|| TextRange::empty(TextSize::ZERO), |s| match s {
                Stmt::FunctionDef(f) => f.name.range(),
                Stmt::ClassDef(c) => c.name.range(),
                other => other.range(),
            }),
            message: format!(
                "Jython cannot compile this module: it has too many functions, classes, lambdas and generator expressions (code objects) for Jython's class initializer, which needs at least {} bytes of Java bytecode for them, more than one Java method can hold ({METHOD_LIMIT} bytes, Module or method too large)",
                group(init)
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

/// Why Jython's verdict on the size of the module is not known, if it is
/// not: a Java method (a code unit, or the class's `<init>`, which builds
/// a code object per unit, and `call_function`, which dispatches to them)
/// whose lower bound fits a Java method and whose upper bound does not, or
/// a constant pool whose bounds lie on both sides of its limit. The lower
/// bounds are those `SCR002` reports from ([`size_errors`]; `<init>` and
/// `call_function` have none), the upper bounds are [`upper`]'s. A fix that
/// changes such a module may change whether it compiles without the
/// bounds seeing it, so the safety net does not trust it.
pub fn size_unknown(module: &Module, source: &str) -> Option<String> {
    for unit in code_units(module, source) {
        if unit.bytes <= METHOD_LIMIT && unit.upper > METHOD_LIMIT {
            let what = match unit.owner {
                None if unit.name == "f" && unit.bytes == 0 => "lambda or generator expression",
                None => "module",
                Some(Stmt::ClassDef(_)) => "class body",
                Some(_) => "function",
            };
            return Some(format!(
                "the {what} `{}` needs between {} and {} bytes of Java bytecode, so whether Jython can compile it ({} bytes per method) is not known",
                unit.name,
                group(unit.bytes),
                group(unit.upper),
                group(METHOD_LIMIT)
            ));
        }
    }
    let class = upper::class(module, source);
    let (init_lower, _) = init_lower(module, source);
    if init_lower > METHOD_LIMIT {
        // Certainly too large: decided, `size_errors` reports it.
        return None;
    }
    for (method, bytes) in [
        ("<init>", class.init),
        ("call_function", class.call_function),
        ("set$$", class.displays),
    ] {
        if bytes > METHOD_LIMIT {
            return Some(format!(
                "the module's Java method `{method}` (one entry per code unit) may need up to {} bytes of Java bytecode, so whether Jython can compile the module ({} bytes per method) is not known",
                group(bytes),
                group(METHOD_LIMIT)
            ));
        }
    }
    let mut strings = Strings::default();
    strings.suite(&module.body, true);
    let lower = strings.distinct.len() * 2;
    if lower <= CONSTANT_POOL_LIMIT && class.pool > CONSTANT_POOL_LIMIT {
        return Some(format!(
            "the module's constant pool needs between {} and {} entries, so whether Jython can compile it ({} entries per class) is not known",
            group(lower),
            group(class.pool),
            group(CONSTANT_POOL_LIMIT)
        ));
    }
    None
}

/// Least bytes of the class initializer `<init>` per code unit (it builds
/// the unit's code object) and per parameter of a function or method (a
/// variable name of the code object). Measured with
/// `oracle/run-oracle.sh --method-size`: about 31 bytes per unit and 6
/// per variable name; `tests/method_size_oracle.rs` checks the bound
/// against every measured `<init>`.
const INIT_PER_UNIT: usize = 30;
const INIT_PER_PARAM: usize = 6;

/// A lower bound of the bytecode of the module's `<init>` and the `def`
/// or `class` statement (of the module's own code) whose code object makes
/// it exceed the method limit: every unit Jython generates
/// ([`code_units`]) counts once, a function with its parameters.
pub fn init_lower<'a>(module: &'a Module, source: &str) -> (usize, Option<&'a Stmt>) {
    let mut bytes = 0;
    let mut crossing = None;
    for unit in code_units(module, source) {
        let params = match unit.owner {
            Some(Stmt::FunctionDef(f)) => {
                fn names(e: &Expr) -> usize {
                    match e {
                        Expr::Tuple(t) => t.elts.iter().map(names).sum(),
                        Expr::List(t) => t.elts.iter().map(names).sum(),
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
                f.args.args.iter().map(names).sum::<usize>()
                    + usize::from(f.args.vararg.is_some())
                    + usize::from(f.args.kwarg.is_some())
                    + assigned
            }
            _ => 0,
        };
        bytes += INIT_PER_UNIT + INIT_PER_PARAM * params;
        if bytes > METHOD_LIMIT && crossing.is_none() {
            crossing = unit.owner;
        }
    }
    (bytes, crossing)
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
    /// Upper bound of the bytes of bytecode ([`upper`]): Jython never
    /// generates more for the unit.
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
    let cost = Cost {
        // Jython's line numbers: a lone CR does not end a line.
        index: LineIndex::python(source),
    };
    let mut out = Vec::new();
    cost.unit(UnitBody::Body(&module.body, true), None, "f", &mut out);
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
    Exprs(Vec<&'a Expr>),
}

struct Cost {
    index: LineIndex,
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
            upper: upper::unit(&body, owner),
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

/// Sound upper bounds of what Jython's code generator emits.
///
/// Every figure is at least what Jython 2.7's code generator (the Ignition
/// 8.3.3 image) emits for the construct in any context; the tests in
/// `tests/method_size_oracle.rs` check the bounds against the measured
/// size of every Java method and the constant pool count of thousands of
/// generated programs, including programs close to each limit, and report
/// the margins. Unlike the lower bound, the upper bound counts the code
/// after a statement that ends a suite, a `finally` body once for the
/// normal exit, once for the exception handler and once for every
/// `return`, `break` and `continue` inside the `try` (Jython inlines it
/// at each), the exit code of a `with` at each of those too, the stack and
/// temporaries a `yield` saves and restores, and the closure and
/// generator set-up of a unit.
pub mod upper {
    use iglint_python::ast::*;
    use iglint_text::Ranged;
    use rustc_hash::FxHashSet;

    use super::UnitBody;

    /// A frame line number update.
    const SETLINE: usize = 7;
    /// The exit code of a `with` statement, inlined at every way out.
    const WITH_EXIT: usize = 24;

    #[derive(Default)]
    struct Cx<'a> {
        /// Compound statements around the current one, in the unit.
        depth: usize,
        yields: usize,
        /// Names of the unit (an upper bound of its local, cell and free
        /// variables).
        names: FxHashSet<&'a str>,
        /// The unit creates nested units (closures may need cells).
        nests: bool,
        /// Hidden variables of list comprehensions (`_[1]`, a local each).
        hidden: usize,
        /// Names the nested units read (free variables the unit may pass
        /// in cells).
        free: FxHashSet<&'a str>,
    }

    /// Upper bound of the bytecode of a code unit.
    pub(crate) fn unit(body: &UnitBody<'_>, owner: Option<&Stmt>) -> usize {
        let mut cx = Cx::default();
        let (bytes, params) = match body {
            UnitBody::Body(stmts, _) => {
                let params = match owner {
                    Some(Stmt::FunctionDef(f)) => params(&f.args, &mut cx),
                    _ => 0,
                };
                (suite(stmts, &mut cx).0, params)
            }
            // A lambda's body, or a generator expression (a generator with
            // a loop per `for` and a `yield`): every expression counts
            // as a loop with its set-up.
            UnitBody::Exprs(exprs) => (
                exprs
                    .iter()
                    .map(|e| 60 + expr(e, 0, &mut cx))
                    .sum::<usize>()
                    + 120,
                0,
            ),
        };
        let base = match owner {
            None => 60,
            Some(Stmt::ClassDef(_)) => 60,
            Some(_) => 60,
        };
        // Generators save and restore their temporaries around the code
        // (measured: displays cost a quarter more in a generator).
        let (bytes, generator) = if cx.yields > 0 {
            (bytes * 13 / 10, 80 + 16 * cx.yields)
        } else {
            (bytes, 0)
        };
        let closure = if cx.nests {
            12 * (cx.names.len() + cx.hidden + cx.free.len())
        } else {
            0
        };
        // A quarter on top of the largest costs measured per construct: the
        // margin for shapes the measured programs do not combine.
        (base + 20 * params + generator + closure + bytes) * 5 / 4
    }

    fn params<'a>(args: &'a Arguments, cx: &mut Cx<'a>) -> usize {
        fn count<'a>(e: &'a Expr, cx: &mut Cx<'a>) -> usize {
            match e {
                Expr::Name(n) => {
                    cx.names.insert(&n.id);
                    1
                }
                Expr::Tuple(t) => 2 + t.elts.iter().map(|e| count(e, cx)).sum::<usize>(),
                Expr::List(t) => 2 + t.elts.iter().map(|e| count(e, cx)).sum::<usize>(),
                _ => 2,
            }
        }
        let mut n = args.args.iter().map(|a| count(a, cx)).sum::<usize>();
        for name in args.vararg.iter().chain(&args.kwarg) {
            cx.names.insert(name.as_str());
            n += 1;
        }
        n
    }

    /// Bytes and the number of ways out (`return`, `break`, `continue`)
    /// of the code generated for `body`.
    fn suite<'a>(body: &'a [Stmt], cx: &mut Cx<'a>) -> (usize, usize) {
        let mut out = (0, 0);
        for s in body {
            let (b, e) = stmt(s, cx);
            out.0 += b;
            out.1 += e;
        }
        out
    }

    fn nested<'a, R>(cx: &mut Cx<'a>, f: impl FnOnce(&mut Cx<'a>) -> R) -> R {
        cx.depth += 1;
        let r = f(cx);
        cx.depth -= 1;
        r
    }

    fn stmt<'a>(s: &'a Stmt, cx: &mut Cx<'a>) -> (usize, usize) {
        let e = |x: &'a Expr, cx: &mut Cx<'a>| expr(x, 0, cx);
        let opt =
            |x: &'a Option<Box<Expr>>, cx: &mut Cx<'a>| x.as_deref().map_or(0, |x| expr(x, 0, cx));
        let all =
            |xs: &'a [Expr], cx: &mut Cx<'a>| xs.iter().map(|x| expr(x, 0, cx)).sum::<usize>();
        let (own, exits) = match s {
            Stmt::Global(_) => return (0, 0),
            Stmt::Pass(_) => (0, 0),
            Stmt::Break(_) | Stmt::Continue(_) => (8, 1),
            Stmt::Expr(x) => (2 + e(&x.value, cx), 0),
            Stmt::Assign(x) => (
                4 + 8 * x.targets.len() + all(&x.targets, cx) + e(&x.value, cx),
                0,
            ),
            Stmt::AugAssign(x) => (30 + 2 * e(&x.target, cx) + e(&x.value, cx), 0),
            Stmt::Delete(x) => (4 + all(&x.targets, cx), 0),
            Stmt::Print(x) => (
                8 + 8 * x.values.len() + opt(&x.dest, cx) + all(&x.values, cx),
                0,
            ),
            Stmt::Return(x) => (14 + opt(&x.value, cx), 1),
            Stmt::Raise(x) => (
                16 + opt(&x.type_, cx) + opt(&x.inst, cx) + opt(&x.tback, cx),
                0,
            ),
            Stmt::Assert(x) => (45 + e(&x.test, cx) + opt(&x.msg, cx), 0),
            Stmt::Exec(x) => (
                12 + e(&x.body, cx) + opt(&x.globals, cx) + opt(&x.locals, cx),
                0,
            ),
            Stmt::Import(x) => {
                for a in &x.names {
                    alias(a, cx);
                }
                (28 * x.names.len(), 0)
            }
            Stmt::ImportFrom(x) => {
                for a in &x.names {
                    alias(a, cx);
                }
                (24 + 28 * x.names.len(), 0)
            }
            Stmt::If(x) => nested(cx, |cx| {
                let test = expr(&x.test, 0, cx);
                let (b, be) = suite(&x.body, cx);
                let (o, oe) = suite(&x.orelse, cx);
                (16 + test + b + o, be + oe)
            }),
            Stmt::While(x) => nested(cx, |cx| {
                let test = expr(&x.test, 0, cx);
                let (b, be) = suite(&x.body, cx);
                let (o, oe) = suite(&x.orelse, cx);
                (20 + test + b + o, be + oe)
            }),
            Stmt::For(x) => nested(cx, |cx| {
                let head = expr(&x.target, 0, cx) + expr(&x.iter, 0, cx);
                let (b, be) = suite(&x.body, cx);
                let (o, oe) = suite(&x.orelse, cx);
                (30 + head + b + o, be + oe)
            }),
            Stmt::With(x) => nested(cx, |cx| {
                let head = expr(&x.context_expr, 0, cx)
                    + x.optional_vars.as_deref().map_or(0, |v| expr(v, 0, cx));
                let (b, be) = suite(&x.body, cx);
                (60 + head + b + WITH_EXIT * be, be)
            }),
            Stmt::TryExcept(x) => nested(cx, |cx| {
                let (mut n, mut exits) = suite(&x.body, cx);
                n += 24;
                for h in &x.handlers {
                    n += 24;
                    n += h.type_.as_deref().map_or(0, |t| expr(t, 0, cx));
                    n += h.name.as_deref().map_or(0, |t| expr(t, 0, cx));
                    let (b, be) = suite(&h.body, cx);
                    n += b;
                    exits += be;
                }
                let (o, oe) = suite(&x.orelse, cx);
                (n + o, exits + oe)
            }),
            Stmt::TryFinally(x) => nested(cx, |cx| {
                let (b, be) = suite(&x.body, cx);
                let (f, fe) = suite(&x.finalbody, cx);
                let copies = 2 + be;
                (30 + b + f * copies, be + fe * copies)
            }),
            Stmt::FunctionDef(x) => {
                cx.names.insert(x.name.as_str());
                cx.nests = true;
                free_candidates(cx, &x.body, &[]);
                (
                    50 + 10 * (x.decorator_list.len() + x.args.defaults.len())
                        + all(&x.decorator_list, cx)
                        + all(&x.args.defaults, cx),
                    0,
                )
            }
            Stmt::ClassDef(x) => {
                cx.names.insert(x.name.as_str());
                cx.nests = true;
                free_candidates(cx, &x.body, &[]);
                (
                    50 + 10 * (x.decorator_list.len() + x.bases.len())
                        + all(&x.decorator_list, cx)
                        + all(&x.bases, cx),
                    0,
                )
            }
        };
        (SETLINE + own, exits)
    }

    /// The names a nested unit reads (`stmt` a `def` or `class`, or the
    /// expressions of a lambda or generator expression): each may be a
    /// free variable that the unit creating it passes in a cell.
    fn free_candidates<'a>(cx: &mut Cx<'a>, stmts: &'a [Stmt], exprs: &[&'a Expr]) {
        struct Names<'c, 'a>(&'c mut FxHashSet<&'a str>);
        impl<'a> iglint_python::visitor::Visitor<'a> for Names<'_, 'a> {
            fn visit_expr(&mut self, e: &'a Expr) {
                if let Expr::Name(n) = e {
                    self.0.insert(&n.id);
                }
                iglint_python::visitor::walk_expr(self, e);
            }
        }
        let mut v = Names(&mut cx.free);
        for s in stmts {
            iglint_python::visitor::Visitor::visit_stmt(&mut v, s);
        }
        for e in exprs {
            iglint_python::visitor::Visitor::visit_expr(&mut v, e);
        }
    }

    fn alias<'a>(a: &'a Alias, cx: &mut Cx<'a>) {
        let bound = a.asname.as_ref().map_or_else(
            || a.name.as_str().split('.').next().unwrap_or(""),
            |n| n.as_str(),
        );
        cx.names.insert(bound);
    }

    /// Upper bound of an expression at nesting depth `depth`, nested units
    /// excluded (only the code that creates them).
    fn expr<'a>(x: &'a Expr, depth: usize, cx: &mut Cx<'a>) -> usize {
        let d = depth + 1;
        let all =
            |xs: &'a [Expr], cx: &mut Cx<'a>| xs.iter().map(|x| expr(x, d, cx)).sum::<usize>();
        match x {
            Expr::Name(n) => {
                cx.names.insert(&n.id);
                match n.ctx {
                    ExprContext::Param => 0,
                    ExprContext::Store => 12,
                    ExprContext::Del => 8,
                    _ => 8,
                }
            }
            Expr::Num(n) => match n.n {
                Number::Int(_) => 7,
                Number::Long(_) => 8,
                Number::Float(_) | Number::Complex(_) => 8,
            },
            Expr::Str(s) => {
                let len = match &s.s {
                    StringValue::Bytes(b) => b.len(),
                    StringValue::Unicode(u) => u.len(),
                };
                7 + 8 * (len / 16_000)
            }
            Expr::Attribute(a) => {
                let own = match a.ctx {
                    ExprContext::Store => 14,
                    ExprContext::Del => 8,
                    _ => 7,
                };
                own + expr(&a.value, d, cx)
            }
            Expr::Subscript(a) => {
                let own = match a.ctx {
                    ExprContext::Store => 14,
                    ExprContext::Del => 8,
                    _ => 5,
                };
                own + expr(&a.value, d, cx) + slice(&a.slice, d, cx)
            }
            Expr::Call(c) => {
                let simple = c.args.len() <= 4
                    && c.keywords.is_empty()
                    && c.starargs.is_none()
                    && c.kwargs.is_none();
                let own = if simple {
                    6
                } else {
                    16 + 8 * c.args.len()
                        + 22 * c.keywords.len()
                        + 12 * (usize::from(c.starargs.is_some()) + usize::from(c.kwargs.is_some()))
                };
                own + expr(&c.func, d, cx)
                    + all(&c.args, cx)
                    + c.keywords
                        .iter()
                        .map(|k| expr(&k.value, d, cx))
                        .sum::<usize>()
                    + c.starargs.as_deref().map_or(0, |a| expr(a, d, cx))
                    + c.kwargs.as_deref().map_or(0, |a| expr(a, d, cx))
            }
            Expr::UnaryOp(u) => 5 + expr(&u.operand, d, cx),
            Expr::BinOp(b) => 5 + expr(&b.left, d, cx) + expr(&b.right, d, cx),
            Expr::Repr(r) => 5 + expr(&r.value, d, cx),
            Expr::BoolOp(b) => 12 * b.values.len().saturating_sub(1) + all(&b.values, cx),
            Expr::Compare(c) => 22 * c.ops.len() + expr(&c.left, d, cx) + all(&c.comparators, cx),
            Expr::List(ExprList { elts, ctx, .. }) | Expr::Tuple(ExprTuple { elts, ctx, .. }) => {
                let each = if *ctx == ExprContext::Store { 12 } else { 9 };
                20 + each * elts.len() + all(elts, cx)
            }
            Expr::Set(e) => 14 + 9 * e.elts.len() + all(&e.elts, cx),
            Expr::Dict(e) => 14 + 16 * e.keys.len() + all(&e.keys, cx) + all(&e.values, cx),
            Expr::IfExp(e) => {
                14 + expr(&e.test, d, cx) + expr(&e.body, d, cx) + expr(&e.orelse, d, cx)
            }
            Expr::Yield(y) => {
                cx.yields += 1;
                120 + 50 * (cx.depth + depth) + y.value.as_deref().map_or(0, |v| expr(v, d, cx))
            }
            Expr::ListComp(e) => {
                cx.hidden += 1;
                110 + 60 * e.generators.len()
                    + 20 * e.generators.iter().map(|g| g.ifs.len()).sum::<usize>()
                    + expr(&e.elt, d, cx)
                    + e.generators
                        .iter()
                        .map(|g| expr(&g.target, d, cx) + expr(&g.iter, d, cx) + all(&g.ifs, cx))
                        .sum::<usize>()
            }
            Expr::Lambda(l) => {
                cx.nests = true;
                free_candidates(cx, &[], &[&l.body]);
                30 + 8 * l.args.defaults.len() + all(&l.args.defaults, cx)
            }
            Expr::GeneratorExp(ExprGeneratorExp { generators, .. })
            | Expr::SetComp(ExprSetComp { generators, .. })
            | Expr::DictComp(ExprDictComp { generators, .. }) => {
                cx.nests = true;
                free_candidates(cx, &[], &[x]);
                60 + generators.first().map_or(0, |g| expr(&g.iter, d, cx))
            }
        }
    }

    fn slice<'a>(s: &'a Slice, d: usize, cx: &mut Cx<'a>) -> usize {
        match s {
            Slice::Index(i) => expr(&i.value, d, cx),
            Slice::Ellipsis(_) => 6,
            Slice::Slice(s) => {
                8 + [&s.lower, &s.upper, &s.step]
                    .into_iter()
                    .map(|e| e.as_deref().map_or(0, |e| expr(e, d, cx)))
                    .sum::<usize>()
            }
            Slice::ExtSlice(s) => {
                30 + 12 * s.dims.len() + s.dims.iter().map(|x| slice(x, d, cx)).sum::<usize>()
            }
        }
    }

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

    /// What the walk of [`class`] counts.
    #[derive(Default)]
    struct Pool<'a> {
        /// Compiled units, a unit in a `finally` body once per copy.
        units: usize,
        /// `<init>` bytes of the units.
        init: usize,
        /// Distinct strings and identifiers (each a `String` and a `Utf8`
        /// entry), and extra chunks of long strings.
        texts: FxHashSet<std::borrow::Cow<'a, str>>,
        chunks: usize,
        /// Hidden list comprehension variables (`_[1]`, ...).
        hidden: usize,
        /// Elements of the largest display of constants.
        display: usize,
        /// Distinct numeric literals.
        numbers: FxHashSet<String>,
        /// Distinct line numbers beyond `sipush` (an `Integer` entry each).
        lines: FxHashSet<usize>,
    }

    /// Upper bounds of the module's `<init>`, `call_function` and constant
    /// pool.
    pub fn class(module: &Module, source: &str) -> Class {
        let index = iglint_text::LineIndex::python(source);
        let mut pool = Pool::default();
        unit_names(
            UnitBody::Body(&module.body, true),
            None,
            1,
            &index,
            &mut pool,
        );
        Class {
            init: 100 + pool.init,
            call_function: 60 + 12 * pool.units,
            // 600 entries for the runtime's classes, methods and fields
            // and a twentieth on top as the margin (the measured class of
            // 32,600 distinct strings needs 2 entries each and 300 more).
            pool: 600
                + (2 * pool.texts.len()
                    + 2 * pool.chunks
                    + 2 * pool.hidden
                    + 4 * pool.numbers.len()
                    + 24 * pool.units
                    + pool.lines.len())
                    / 20
                + 2 * pool.texts.len()
                + 2 * pool.chunks
                + 2 * pool.hidden
                + 4 * pool.numbers.len()
                + 24 * pool.units
                + pool.lines.len(),
            displays: 200 + 14 * pool.display,
        }
    }

    /// Counts the unit `body` (`copies` times) and the units nested in it.
    fn unit_names<'a>(
        body: UnitBody<'a>,
        owner: Option<&'a Stmt>,
        copies: usize,
        index: &iglint_text::LineIndex,
        pool: &mut Pool<'a>,
    ) {
        let mut cx = Cx::default();
        if let Some(Stmt::FunctionDef(f)) = owner {
            params(&f.args, &mut cx);
        }
        let mut walk = Walk {
            pool,
            index,
            nested: Vec::new(),
            copies: 1,
        };
        match &body {
            UnitBody::Body(stmts, _) => {
                walk.stmts(stmts);
                suite(stmts, &mut cx);
            }
            UnitBody::Exprs(exprs) => {
                for e in exprs {
                    walk.expr(e);
                    expr(e, 0, &mut cx);
                }
            }
        }
        let nested = std::mem::take(&mut walk.nested);
        pool.units += copies;
        pool.init += copies * (60 + 12 * (cx.names.len() + cx.hidden));
        pool.hidden += cx.hidden;
        for name in cx.names {
            pool.texts.insert(std::borrow::Cow::Borrowed(name));
        }
        for (n, times) in nested {
            match n {
                NestedUnit::Def(stmt @ Stmt::FunctionDef(f)) => unit_names(
                    UnitBody::Body(&f.body, false),
                    Some(stmt),
                    copies * times,
                    index,
                    pool,
                ),
                NestedUnit::Def(stmt @ Stmt::ClassDef(c)) => unit_names(
                    UnitBody::Body(&c.body, false),
                    Some(stmt),
                    copies * times,
                    index,
                    pool,
                ),
                NestedUnit::Def(_) => {}
                NestedUnit::Exprs(exprs) => {
                    unit_names(UnitBody::Exprs(exprs), None, copies * times, index, pool)
                }
            }
        }
    }

    enum NestedUnit<'a> {
        Def(&'a Stmt),
        Exprs(Vec<&'a Expr>),
    }

    /// The constants and nested units of one unit.
    struct Walk<'p, 'a> {
        pool: &'p mut Pool<'a>,
        index: &'p iglint_text::LineIndex,
        /// Nested units with the number of copies Jython compiles.
        nested: Vec<(NestedUnit<'a>, usize)>,
        /// Copies of the code being walked (inside `finally` bodies).
        copies: usize,
    }

    impl<'a> Walk<'_, 'a> {
        fn text(&mut self, t: &'a str) {
            self.pool.texts.insert(std::borrow::Cow::Borrowed(t));
        }

        fn stmts(&mut self, body: &'a [Stmt]) {
            for s in body {
                self.stmt(s);
            }
        }

        fn stmt(&mut self, s: &'a Stmt) {
            let line = self.index.line_of(s.range().start()) + 1;
            if line > 32_000 {
                self.pool.lines.insert(line);
            }
            match s {
                Stmt::FunctionDef(f) => {
                    self.text(f.name.as_str());
                    f.decorator_list.iter().for_each(|e| self.expr(e));
                    f.args.defaults.iter().for_each(|e| self.expr(e));
                    self.nested.push((NestedUnit::Def(s), self.copies));
                }
                Stmt::ClassDef(c) => {
                    self.text(c.name.as_str());
                    c.decorator_list.iter().for_each(|e| self.expr(e));
                    c.bases.iter().for_each(|e| self.expr(e));
                    self.nested.push((NestedUnit::Def(s), self.copies));
                }
                Stmt::TryFinally(t) => {
                    self.stmts(&t.body);
                    let exits = exits(&t.body);
                    let before = self.copies;
                    self.copies = before * (2 + exits);
                    self.stmts(&t.finalbody);
                    self.copies = before;
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
                    let mut suites = Vec::new();
                    crate::compile::for_each_child_body(other, &mut |b| suites.push(b));
                    for b in suites {
                        self.stmts(b);
                    }
                    if let Stmt::TryExcept(t) = other {
                        for h in &t.handlers {
                            h.type_.iter().for_each(|e| self.expr(e));
                            h.name.iter().for_each(|e| self.expr(e));
                        }
                    }
                }
            }
        }

        fn expr(&mut self, e: &'a Expr) {
            if super::moved_display(e) {
                let n = match e {
                    Expr::List(ExprList { elts, .. })
                    | Expr::Tuple(ExprTuple { elts, .. })
                    | Expr::Set(ExprSet { elts, .. }) => elts.len(),
                    Expr::Dict(d) => 2 * d.keys.len(),
                    _ => 0,
                };
                self.pool.display = self.pool.display.max(n);
            }
            match e {
                Expr::Name(n) => self.text(&n.id),
                Expr::Attribute(a) => {
                    self.text(a.attr.as_str());
                    self.expr(&a.value);
                }
                Expr::Str(s) => {
                    let (text, len): (std::borrow::Cow<'a, str>, usize) = match &s.s {
                        StringValue::Bytes(b) => (
                            b.iter().map(|&c| char::from(c)).collect::<String>().into(),
                            b.len(),
                        ),
                        StringValue::Unicode(u) => (std::borrow::Cow::Borrowed(&**u), u.len()),
                    };
                    self.pool.chunks += len / 16_000;
                    self.pool.texts.insert(text);
                }
                Expr::Num(n) => {
                    self.pool.numbers.insert(format!("{:?}", n.n));
                }
                Expr::Call(c) => {
                    self.expr(&c.func);
                    c.args.iter().for_each(|a| self.expr(a));
                    for k in &c.keywords {
                        self.text(k.arg.as_str());
                        self.expr(&k.value);
                    }
                    c.starargs.iter().for_each(|a| self.expr(a));
                    c.kwargs.iter().for_each(|a| self.expr(a));
                }
                Expr::Lambda(l) => {
                    l.args.defaults.iter().for_each(|a| self.expr(a));
                    self.nested
                        .push((NestedUnit::Exprs(vec![&*l.body]), self.copies));
                }
                Expr::GeneratorExp(ExprGeneratorExp {
                    elt, generators, ..
                })
                | Expr::SetComp(ExprSetComp {
                    elt, generators, ..
                }) => {
                    self.generator(vec![&**elt], generators);
                }
                Expr::DictComp(d) => self.generator(vec![&*d.key, &*d.value], &d.generators),
                other => iglint_python::visitor::walk_expr(self, other),
            }
        }

        fn generator(&mut self, mut inner: Vec<&'a Expr>, generators: &'a [Comprehension]) {
            if let Some(g) = generators.first() {
                self.expr(&g.iter);
            }
            for (i, g) in generators.iter().enumerate() {
                inner.push(&g.target);
                if i > 0 {
                    inner.push(&g.iter);
                }
                inner.extend(g.ifs.iter());
            }
            self.nested.push((NestedUnit::Exprs(inner), self.copies));
        }
    }

    impl<'a> iglint_python::visitor::Visitor<'a> for Walk<'_, 'a> {
        fn visit_expr(&mut self, e: &'a Expr) {
            self.expr(e);
        }
    }

    /// The ways out of `body` (`return`, `break`, `continue`, nested
    /// statements included, nested units not), as [`suite`] counts them.
    fn exits(body: &[Stmt]) -> usize {
        suite(body, &mut Cx::default()).1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(source: &str) -> Option<(String, usize)> {
        mark_limit_error(source).map(|e| {
            let text = e.range.get(source).unwrap();
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
            assert!(sizes(&dead).is_empty(), "{}", dead.get(..60).unwrap());
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
                live.get(..60).unwrap()
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

    /// The review's repro (round 18, B2): 2,176 lines of `a = abs(i % 7)`
    /// after an import, which Jython rejects, and accepts without the
    /// import line. Neither is certainly too large for the lower bound, so
    /// the verdict is open; many code units leave `<init>` open; small
    /// scripts are decided.
    pub(crate) fn repro(import: bool) -> String {
        let mut s = String::from("def g():\n");
        s.push_str(if import { "    import math\n" } else { "\n" });
        for i in 0..2_176 {
            s.push_str(&format!("    a = abs({})\n", i % 7));
        }
        s.push_str("    c = 1\n    c = 1\n    return a + c\n");
        s
    }

    #[test]
    fn open_verdicts() {
        let open = |source: &str| {
            let parsed = iglint_python::parse_module(source);
            assert!(parsed.errors.is_empty());
            (
                size_unknown(&parsed.module, source),
                size_errors(&parsed.module, source).is_empty(),
            )
        };
        for import in [true, false] {
            let (verdict, fits) = open(&repro(import));
            assert!(fits);
            let verdict = verdict.expect("open");
            assert!(
                verdict.starts_with("the function `g` needs between"),
                "{verdict}"
            );
        }
        let units: String = (0..760)
            .map(|i| format!("def g{i}(a, b, c, d, e, f1, f2, f3):\n    return a\n"))
            .collect();
        assert!(open(&units).0.is_some());
        let parsed = iglint_python::parse_module(&units);
        assert!(upper::class(&parsed.module, &units).init > METHOD_LIMIT);
        assert_eq!(open("import math\nx = 1\n").0, None);
        // Certainly too large: decided (SCR002 reports it).
        let (verdict, fits) = open(&"pass\n".repeat(40_000));
        assert_eq!((verdict, fits), (None, false));
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
