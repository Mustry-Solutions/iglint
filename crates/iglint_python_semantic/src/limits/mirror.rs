//! An upper bound of the bytecode Jython 2.7's code generator emits for
//! every Java method of a module's class, derived from the code generator
//! itself (`org.python.compiler.CodeCompiler`, `Module`, `Code` and
//! `ScopeInfo` of Jython 2.7.3; the `jython-ia-2.7.3.5.jar` of the
//! Ignition 8.3.3 image generates the same instructions, see
//! `docs/internal/fix-safety.md`, "Size limits").
//!
//! The mirror walks the tree the way `CodeCompiler` does: the same visitor
//! per node kind, the same order of temporaries (`Code.getLocal`, which
//! takes the lowest free JVM local, so the size of every `aload` and
//! `astore` is known), the same operand stack record (`stackProduce`,
//! which a `yield` saves and restores), the same stack of exception
//! handlers (a `finally` body is compiled again at every way out of its
//! `try`: the normal end, the handler, and each `return`, `break` and
//! `continue` that leaves it, with the handlers outside it active), and
//! nested units compiled where their creating code is, once per copy of
//! that code. Where the generator's choice depends on something the
//! mirror does not model (whether a name is local, global, a cell or
//! free; the constant pool index of an `ldc`), it charges the larger
//! alternative. A jump costs 3 bytes while the method stays below 32,000
//! bytes; above, a jump whose span may exceed a signed 16-bit offset
//! costs what ASM widens it to (`goto_w`, 5 bytes; a conditional jump
//! becomes the inverse jump around a `goto_w`, 8).
//!
//! Each figure in the comments names the instruction and its largest
//! encoding: `aload`/`astore` 1 byte up to local 3, 2 up to 255, 4 with
//! `wide`; `iconst` 1 (-1 to 5), 2 (`bipush`), 3 (`sipush` or `ldc_w`);
//! `ldc` 3 (`ldc_w`); field and method references, `new`, `anewarray`,
//! `checkcast` and `instanceof` 3; `invokeinterface` 5.
//!
//! Every string the generator loads goes through `Code.ldc`, which builds
//! a string of more than 16,000 UTF-16 units with a `StringBuilder`
//! ([`ldc_string`]): names, attribute names, keywords, imported names and
//! modules, class names, the variable, cell and free variable names and
//! the name of every code object in `<init>`, as well as string literals.
//! The mirror charges each by its length ([`ldc_spelled`]), private names
//! as the class mangles them (review round 20, Codex, blocker: a flat
//! `ldc` per name let 1,683 attribute reads of 16,001 characters fit).

use iglint_python::ast::*;
use rustc_hash::FxHashMap;

use super::scopes::{Facts, UnitRef, facts};
use super::{Bytes, b, count};

/// `ldc`/`ldc_w` of a constant.
pub(super) const LDC: u64 = 3;
/// A field or method reference, `new`, `anewarray`, `checkcast`.
const REF: u64 = 3;
/// `invokeinterface`.
const IFACE: u64 = 5;
/// The frame's line number update (`aload_1`, `iconst` of the line,
/// `invokevirtual PyFrame.setline`).
const SETLINE: u64 = 7;
/// A name: `aload_1`, `ldc` of the name or `iconst` of its index, and
/// `invokevirtual` of `getname`, `getglobal`, `getlocal`, `getderef`,
/// `dellocal` or `delglobal` (the store adds the `aload` of the value).
const NAME: u64 = 7;
/// Below this size no jump of a method can need a wide offset.
const SHORT_SPAN: u64 = 32_000;
/// Nodes the mirror visits per module before it gives up (nested
/// `finally` bodies are copied at every way out, exponentially).
const WORK_LIMIT: u64 = 4_000_000;
/// The JVM locals of a code unit's method: `this`, the frame, the thread
/// state.
const ARGCOUNT: usize = 3;
/// `Module.USE_SETTERS_LIMIT` and `MAX_SETTINGS_PER_SETTER`.
const SETTERS_FROM: usize = 100;
const PER_SETTER: usize = 4096;
/// `Code.ldc` splits longer strings into chunks of this many UTF-16 code
/// units.
const LDC_CHUNK: usize = 16_000;

/// A Java method holding a code unit, as Jython compiles it.
#[derive(Debug)]
pub(crate) struct Compiled {
    /// The unit ([`UnitRef::key`]).
    pub key: usize,
    /// The Java method's name (`name$id` or `f$id`).
    pub name: String,
    /// Upper bound of its bytecode.
    pub bytes: Bytes,
}

/// The upper bounds of every Java method of the module's class.
#[derive(Debug, Default)]
pub(crate) struct Report {
    /// Every compiled code unit in Jython's order (a unit in a `finally`
    /// body once per copy).
    pub methods: Vec<Compiled>,
    /// `<init>`, which builds a code object per compiled unit.
    pub init: Bytes,
    /// `call_function`, a branch per compiled unit.
    pub call_function: Bytes,
    /// The largest `set$$N` method (constant displays filled outside the
    /// unit).
    pub setters: Bytes,
    /// The mirror gave up (too much work): every bound is unknown.
    pub saturated: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    Free,
    Used,
    /// `Code.getFinallyLocal`: not saved around a `yield`.
    Finally,
}

#[derive(Clone, Copy)]
enum Handler<'a> {
    Finally(&'a [Stmt]),
    /// The exit call of a `with` statement, with its manager's local.
    WithExit(usize),
    Except,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Jump {
    Goto,
    Cond,
}

/// The state of the method being generated (`Code` and the per-method
/// fields of `CodeCompiler`).
struct Code<'a> {
    /// Size with short jumps.
    narrow: Bytes,
    /// Size with jumps widened where their span may need it.
    wide: Bytes,
    slots: Vec<Slot>,
    return_local: Option<usize>,
    /// `CodeCompiler.stack`: `true` for a thread state.
    stack: Vec<bool>,
    handlers: Vec<Handler<'a>>,
    bcf: usize,
    yields: usize,
    temporary: usize,
    aug: [usize; 4],
    generator: bool,
    class_name: Option<&'a str>,
}

impl Code<'_> {
    fn new(generator: bool) -> Self {
        Code {
            narrow: b(0),
            wide: b(0),
            slots: vec![Slot::Used; ARGCOUNT],
            return_local: None,
            stack: Vec::new(),
            handlers: Vec::new(),
            bcf: 0,
            yields: 0,
            temporary: 0,
            aug: [0; 4],
            generator,
            class_name: None,
        }
    }
}

/// The sum of instruction sizes, saturating.
fn sum(parts: &[u64]) -> Bytes {
    parts.iter().fold(b(0), |s, p| s + b(*p))
}

/// `aload`, `astore` or `iload` of local `i`.
fn var(i: usize) -> u64 {
    match i {
        0..=3 => 1,
        4..=255 => 2,
        _ => 4,
    }
}

/// `Code.iconst(v)`.
fn iconst(v: usize) -> u64 {
    match v {
        0..=5 => 1,
        6..=127 => 2,
        _ => 3,
    }
}

/// `Code.ldc` of a string of `len` UTF-16 code units: longer strings are
/// built with a `StringBuilder` (`new`, `dup`, `iconst`, `invokespecial`,
/// an `ldc` and `invokevirtual append` per chunk, `invokevirtual
/// toString`).
pub(super) fn ldc_string(len: usize) -> Bytes {
    if len > LDC_CHUNK {
        let chunks = len.div_ceil(LDC_CHUNK);
        sum(&[REF, 1, iconst(len), REF, REF]) + count(chunks) * sum(&[LDC, REF])
    } else {
        b(LDC)
    }
}

/// The length of `s` in UTF-16 code units (Java's `String.length`).
pub(super) fn utf16_len(s: &str) -> usize {
    if s.is_ascii() {
        s.len()
    } else {
        s.encode_utf16().count()
    }
}

/// At least the length of `name` as the code generator spells it in a
/// unit inside class `class` (`CodeCompiler.getName`: a private name is
/// mangled to `_<class>__x`; counted mangled wherever Jython may mangle
/// it).
pub(super) fn spelled_len(class: Option<&str>, name: &str) -> usize {
    let len = utf16_len(name);
    match class {
        Some(c) if crate::mangle::is_private(name) => len
            .saturating_add(utf16_len(c.trim_start_matches('_')))
            .saturating_add(1),
        _ => len,
    }
}

/// `Code.ldc` of `name` as spelled in a unit inside class `class`.
pub(super) fn ldc_spelled(class: Option<&str>, name: &str) -> Bytes {
    ldc_string(spelled_len(class, name))
}

/// A number or string constant (`Module.*Constant.get`): the value and
/// `invokestatic` of `Py.newInteger`, `newFloat`, `newImaginary`,
/// `newLong` or `PyString.fromInterned`.
fn constant(e: &Expr) -> Bytes {
    match e {
        Expr::Num(n) => match &n.n {
            Number::Int(v) => {
                let small = usize::try_from(*v).map_or(3, iconst);
                let small = if *v == -1 { 1 } else { small };
                sum(&[small, REF])
            }
            Number::Long(digits) => ldc_string(digits.len()) + b(REF),
            Number::Float(_) | Number::Complex(_) => sum(&[LDC, REF]),
        },
        Expr::Str(s) => {
            let len = match &s.s {
                StringValue::Bytes(bytes) => bytes.len(),
                StringValue::Unicode(text) => text.encode_utf16().count(),
            };
            ldc_string(len) + b(REF)
        }
        _ => b(0),
    }
}

/// The docstring of a body (`CodeCompiler.getDocStr`).
fn docstring(body: &[Stmt]) -> Option<&Expr> {
    match body.first() {
        Some(Stmt::Expr(s)) if matches!(*s.value, Expr::Str(_)) => Some(&s.value),
        _ => None,
    }
}

/// The value a `yield` produces.
#[derive(Clone, Copy)]
enum Value<'a> {
    Expr(&'a Expr),
    /// The `(key, value)` tuple of a dict comprehension.
    Pair(&'a Expr, &'a Expr),
}

/// The iterable of a `for` loop.
#[derive(Clone, Copy)]
enum Iter<'a> {
    Expr(&'a Expr),
    /// `_(x)`, the parameter of a generator expression.
    Param,
}

struct Mirror<'a> {
    facts: FxHashMap<usize, Facts>,
    /// `setup_closure` calls per unit: each appends the free variables
    /// again (`ScopeInfo.freevars`), so the n-th compiled copy of a unit
    /// in a `finally` body has n times its free variables.
    closures: FxHashMap<usize, usize>,
    /// Compiles per `def`: `ArgListCompiler.appendInitCode` prepends the
    /// unpacking of tuple parameters to the function's own statement
    /// list each time.
    defs: FxHashMap<usize, usize>,
    code: Code<'a>,
    report: Report,
    work: u64,
}

/// Upper bounds of every Java method Jython generates for `module`.
pub(crate) fn report(module: &Module) -> Report {
    let facts = facts(module);
    let mut m = Mirror {
        facts,
        closures: FxHashMap::default(),
        defs: FxHashMap::default(),
        code: Code::new(false),
        report: Report::default(),
        work: 0,
    };
    // `<init>`: `aload_0`, `invokespecial`, `aload_0`, `putstatic self`,
    // the code objects, `return`.
    m.report.init = sum(&[1, REF, 1, REF, 1]);
    m.compile(UnitRef::Module(module), None, 1);
    let units = count(m.report.methods.len());
    // `call_function`: `aload_0`, `aload_2`, `aload_3`, `iload_1`, a
    // `tableswitch` (opcode, up to 3 bytes of padding, default, low,
    // high, an offset per unit), per unit `invokevirtual` and `areturn`,
    // then `aconst_null`, `areturn`.
    m.report.call_function = sum(&[4, 1, 3, 12, 2]) + units * sum(&[4, REF, 1]);
    m.report
}

impl<'a> Mirror<'a> {
    fn emit(&mut self, n: u64) {
        self.code.narrow += b(n);
        self.code.wide += b(n);
    }

    fn emit_all(&mut self, parts: &[u64]) {
        self.emit_bytes(sum(parts));
    }

    fn emit_bytes(&mut self, n: Bytes) {
        self.code.narrow += n;
        self.code.wide += n;
    }

    /// The position for spans.
    fn mark(&self) -> Bytes {
        self.code.wide
    }

    /// A jump over `span` bytes (`None`: not known). Callers measure the
    /// span on the code emitted so far and add what lies between it and
    /// the jump's target but is not emitted yet: the jumps themselves at
    /// their widest (8 for a conditional jump, 16 for two) and fixed code
    /// after them.
    fn jump(&mut self, kind: Jump, span: Option<Bytes>) {
        self.code.narrow += b(3);
        let short = span.is_some_and(|s| s.0 <= SHORT_SPAN);
        self.code.wide += b(match (short, kind) {
            (true, _) => 3,
            (false, Jump::Goto) => 5,
            (false, Jump::Cond) => 8,
        });
    }

    fn since(&self, from: Bytes) -> Bytes {
        self.mark() - from
    }

    /// Counts work; `true` when the mirror has given up.
    fn tick(&mut self) -> bool {
        self.work = self.work.saturating_add(1);
        if self.work > WORK_LIMIT {
            self.report.saturated = true;
        }
        self.report.saturated
    }

    /// `Code.ldc` of a name the unit spells (mangled in a class).
    fn ldc_name(&self, name: &str) -> Bytes {
        ldc_spelled(self.code.class_name, name)
    }

    /// `visitName` of `name`: [`NAME`] with the `ldc` of its spelling.
    fn name_bytes(&self, name: &str) -> Bytes {
        b(NAME) - b(LDC) + self.ldc_name(name)
    }

    // ---- Locals (`Code.getLocal`, `freeLocal`) ----

    fn get_local(&mut self) -> usize {
        let slots = &mut self.code.slots;
        if let Some(i) = slots
            .iter()
            .skip(ARGCOUNT)
            .position(|s| *s == Slot::Free)
            .map(|i| i.saturating_add(ARGCOUNT))
            && let Some(slot) = slots.get_mut(i)
        {
            *slot = Slot::Used;
            return i;
        }
        slots.push(Slot::Used);
        slots.len().saturating_sub(1)
    }

    fn free_local(&mut self, i: usize) {
        if let Some(slot) = self.code.slots.get_mut(i) {
            *slot = Slot::Free;
        }
    }

    fn get_finally_local(&mut self) -> usize {
        let i = self.get_local();
        if let Some(slot) = self.code.slots.get_mut(i) {
            *slot = Slot::Finally;
        }
        i
    }

    fn return_local(&mut self) -> usize {
        match self.code.return_local {
            Some(i) => i,
            None => {
                let i = self.get_local();
                self.code.return_local = Some(i);
                i
            }
        }
    }

    /// `getActiveLocals`: the locals a `yield` saves.
    fn active(&self) -> Vec<usize> {
        self.code
            .slots
            .iter()
            .enumerate()
            .skip(ARGCOUNT)
            .filter(|(i, s)| **s == Slot::Used && Some(*i) != self.code.return_local)
            .map(|(i, _)| i)
            .collect()
    }

    /// `storeTop`: a new local and `astore`.
    fn store_top(&mut self) -> usize {
        let t = self.get_local();
        self.emit(var(t));
        t
    }

    fn push(&mut self, thread_state: bool) {
        self.code.stack.push(thread_state);
    }

    fn pop(&mut self, n: usize) {
        let len = self.code.stack.len().saturating_sub(n);
        self.code.stack.truncate(len);
    }

    // ---- Units ----

    /// Compiles a code unit into a Java method of its own
    /// (`Module.codeConstant`, `CodeCompiler.parse`) and adds its code
    /// object to `<init>`. `closures` is the number of `setup_closure`
    /// calls of the unit so far.
    fn compile(&mut self, unit: UnitRef<'a>, class_name: Option<&'a str>, closures: usize) {
        if self.tick() {
            return;
        }
        let key = unit.key();
        let facts = self.facts.get(&key).copied().unwrap_or_default();
        let id = self.report.methods.len();
        let mangle = |name: &'a str| -> String {
            match class_name {
                Some(class) if name.starts_with("__") && !name.ends_with("__") => {
                    format!("_{}{name}", class.trim_start_matches('_'))
                }
                _ => name.to_owned(),
            }
        };
        let (name, own_class) = match unit {
            UnitRef::Function(_, f) => (format!("{}${id}", mangle(f.name.as_str())), class_name),
            UnitRef::Class(_, c) => {
                let mangled = mangle(c.name.as_str());
                (format!("{mangled}${id}"), Some(c.name.as_str()))
            }
            UnitRef::Module(_) => (format!("f${id}"), None),
            UnitRef::Lambda(..) | UnitRef::Gen(_) => (format!("f${id}"), class_name),
        };
        // The code object's name (`co_name`).
        let co_name = match unit {
            UnitRef::Function(_, f) => ldc_spelled(class_name, f.name.as_str()),
            UnitRef::Class(_, c) => ldc_spelled(class_name, c.name.as_str()),
            _ => b(LDC),
        };
        self.report.methods.push(Compiled {
            key,
            name,
            bytes: b(0),
        });
        self.report.init += init_bytes(&facts, unit, closures, id, co_name);
        let mut code = Code::new(facts.generator);
        code.class_name = own_class;
        let parent = std::mem::replace(&mut self.code, code);
        self.parse(unit, &facts);
        let bytes = if self.code.narrow.0 <= SHORT_SPAN {
            self.code.narrow
        } else {
            self.code.wide
        };
        self.code = parent;
        if let Some(m) = self.report.methods.get_mut(id) {
            m.bytes = bytes;
        }
    }

    /// `CodeCompiler.parse`.
    fn parse(&mut self, unit: UnitRef<'a>, facts: &Facts) {
        if let UnitRef::Class(_, c) = unit {
            // `__module__ = __name__`: `aload_1`, `ldc`, `aload_1`, `ldc`,
            // two `invokevirtual`.
            self.emit_all(&[1, LDC, 1, LDC, REF, REF]);
            if let Some(doc) = docstring(&c.body) {
                self.emit_all(&[1, LDC, REF]);
                self.emit_bytes(constant(doc));
            }
        }
        let start = self.mark();
        // Parameters that are cells: `aload_1`, two `iconst`,
        // `invokevirtual to_cell`.
        let index = iconst(facts.names.max(facts.cells));
        self.emit_bytes(count(facts.paramcells) * sum(&[1, index, index, REF]));
        if facts.with {
            // `f_exits = new PyObject[n]`.
            self.emit_all(&[1, 3, REF, REF]);
        }
        let exit = match unit {
            UnitRef::Module(m) => {
                if let Some(doc) = docstring(&m.body) {
                    self.emit_all(&[1, LDC, REF]);
                    self.emit_bytes(constant(doc));
                }
                // `visitModule` traverses every statement.
                for s in &m.body {
                    self.stmt(s);
                }
                false
            }
            UnitRef::Function(s, f) => {
                let key = super::scopes::stmt_key(s);
                let copies = self.defs.entry(key).or_insert(0);
                *copies = copies.saturating_add(1);
                let copies = *copies;
                for _ in 0..copies {
                    self.init_code(&f.args);
                }
                self.suite(&f.body)
            }
            UnitRef::Class(_, c) => {
                self.suite(&c.body);
                false
            }
            UnitRef::Lambda(_, l) => {
                self.init_code(&l.args);
                self.ret(Some(&l.body));
                true
            }
            UnitRef::Gen(e) => {
                self.gen_body(e);
                false
            }
        };
        if matches!(unit, UnitRef::Class(..)) {
            // `aload_1`, `invokevirtual getf_locals`, `areturn`.
            self.emit_all(&[1, REF, 1]);
        } else if !exit {
            // `f_lasti = -1`, `getstatic None`, `areturn`.
            self.emit_all(&[1, 1, REF, REF, 1]);
        }
        if facts.generator {
            // `aload_1`, `getfield f_lasti`, `tableswitch` over the
            // restart points, and the `goto` to it at the start.
            let labels = count(self.code.yields.saturating_add(1));
            self.emit_all(&[1, REF]);
            self.emit_bytes(sum(&[1, 3, 12]) + labels * b(4));
            let span = self.since(start);
            self.jump(Jump::Goto, Some(span));
        }
    }

    /// The unpacking of tuple parameters (`ArgListCompiler.init_code`):
    /// `(a, b) = <the tuple's name>`.
    fn init_code(&mut self, args: &'a Arguments) {
        for a in &args.args {
            if matches!(a, Expr::Tuple(_) | Expr::List(_)) {
                // The tuple's name, `(a, b)`.
                self.emit(SETLINE);
                self.emit_bytes(b(NAME) - b(LDC) + ldc_string(tuple_name_len(a)));
                self.set(a);
            }
        }
    }

    // ---- Statements ----

    /// `CodeCompiler.suite`: stops after a statement that exits.
    fn suite(&mut self, body: &'a [Stmt]) -> bool {
        for s in body {
            if self.stmt(s) {
                return true;
            }
        }
        false
    }

    /// Compiles a statement; `true` when Jython considers that it exits.
    fn stmt(&mut self, s: &'a Stmt) -> bool {
        if self.tick() {
            return false;
        }
        match s {
            Stmt::Global(_) => false,
            Stmt::Pass(_) => {
                self.emit(SETLINE);
                false
            }
            Stmt::Expr(x) => {
                self.emit(SETLINE);
                self.expr(&x.value);
                // `pop`.
                self.emit(1);
                false
            }
            Stmt::Assign(x) => {
                self.emit(SETLINE);
                self.expr(&x.value);
                if let [target] = x.targets.as_slice() {
                    self.set(target);
                } else {
                    let tmp = self.store_top();
                    for t in &x.targets {
                        self.set_with(t, tmp);
                    }
                    self.free_local(tmp);
                }
                false
            }
            Stmt::AugAssign(x) => {
                self.aug_assign(x);
                false
            }
            Stmt::Delete(x) => {
                self.emit(SETLINE);
                for t in &x.targets {
                    self.expr(t);
                }
                false
            }
            Stmt::Print(x) => {
                self.emit(SETLINE);
                let tmp = x.dest.as_deref().map(|d| {
                    self.expr(d);
                    self.store_top()
                });
                if x.values.is_empty() {
                    self.emit_all(&[tmp.map_or(0, var), REF]);
                } else {
                    for v in &x.values {
                        self.emit(tmp.map_or(0, var));
                        self.expr(v);
                        self.emit(REF);
                    }
                }
                if let Some(t) = tmp {
                    self.free_local(t);
                }
                false
            }
            Stmt::Return(x) => {
                self.ret(x.value.as_deref());
                true
            }
            Stmt::Raise(x) => {
                self.emit(SETLINE);
                let mut n = 0usize;
                for e in [&x.type_, &x.inst, &x.tback].into_iter().flatten() {
                    self.expr(e);
                    self.push(false);
                    n = n.saturating_add(1);
                }
                self.pop(n);
                // `invokestatic makeException`, `athrow`.
                self.emit_all(&[REF, 1]);
                true
            }
            Stmt::Assert(x) => {
                // `aload_1`, `ldc __debug__`, `invokevirtual getglobal`,
                // `invokevirtual __nonzero__`, `ifeq`.
                self.emit_all(&[SETLINE, 1, LDC, REF, REF]);
                let from = self.mark();
                self.expr(&x.test);
                self.emit(REF);
                let jump_at = self.mark();
                match &x.msg {
                    Some(m) => self.expr(m),
                    None => self.emit(REF),
                }
                // `aload_1`, `ldc`, `invokevirtual getglobal`, `swap`,
                // `invokestatic makeException`, `athrow`.
                self.emit_all(&[1, LDC, REF, 1, REF, 1]);
                let (all, rest) = (self.since(from), self.since(jump_at));
                // The `ifeq` past the whole check and the `ifne` past the
                // raise: each also spans both jumps (at most 16 bytes).
                self.jump(Jump::Cond, Some(all + b(16)));
                self.jump(Jump::Cond, Some(rest + b(8)));
                false
            }
            Stmt::Exec(x) => {
                self.emit(SETLINE);
                self.expr(&x.body);
                self.push(false);
                for part in [&x.globals, &x.locals] {
                    match part.as_deref() {
                        Some(e) => self.expr(e),
                        None => self.emit(1),
                    }
                    self.push(false);
                }
                self.pop(3);
                self.emit(REF);
                false
            }
            Stmt::Import(x) => {
                self.emit(SETLINE);
                for a in &x.names {
                    // `ldc` of the (dotted) name, `aload_1`, `iconst` of the
                    // level, `invokestatic importOne`; the `set` of the alias
                    // or the first part.
                    self.emit_bytes(ldc_string(utf16_len(a.name.as_str())));
                    self.emit_all(&[1, 1, REF]);
                    let bound = a.asname.as_ref().map_or_else(
                        || a.name.as_str().split('.').next().unwrap_or(""),
                        |n| n.as_str(),
                    );
                    self.set_name(self.ldc_name(bound));
                }
                false
            }
            Stmt::ImportFrom(x) => {
                self.emit(SETLINE);
                // `ldc` of the module (empty for `from . import`).
                let module = x.module.as_ref().map_or("", |m| m.as_str());
                self.emit_bytes(ldc_string(utf16_len(module)));
                let level = iconst(usize::try_from(x.level).unwrap_or(usize::MAX));
                if x.names.first().is_some_and(|a| a.name.as_str() == "*") {
                    self.emit_all(&[1, level, REF]);
                } else {
                    let names: Vec<&str> = x.names.iter().map(|a| a.name.as_str()).collect();
                    let strings = self.make_strings(&names);
                    self.emit(var(strings));
                    self.free_local(strings);
                    self.emit_all(&[1, level, REF]);
                    let tmp = self.store_top();
                    for (i, a) in x.names.iter().enumerate() {
                        // `aload`, `iconst`, `aaload`.
                        self.emit_all(&[var(tmp), iconst(i), 1]);
                        self.set_name(self.ldc_name(a.asname.as_ref().unwrap_or(&a.name).as_str()));
                    }
                    self.free_local(tmp);
                }
                false
            }
            Stmt::If(x) => {
                let body = &x.body;
                self.if_(&x.test, &mut |m| m.suite(body), &x.orelse)
            }
            Stmt::While(x) => {
                let saved = self.begin_loop();
                let from = self.mark();
                self.suite(&x.body);
                self.emit(SETLINE);
                self.expr(&x.test);
                self.emit(REF);
                let span = self.since(from);
                // `goto` the test, `ifne` back to the body.
                self.jump(Jump::Goto, Some(span));
                self.jump(Jump::Cond, Some(span + b(8)));
                self.code.bcf = saved;
                self.suite(&x.orelse);
                false
            }
            Stmt::For(x) => {
                let body = &x.body;
                self.for_(
                    &x.target,
                    Iter::Expr(&x.iter),
                    &mut |m| {
                        m.suite(body);
                    },
                    &x.orelse,
                );
                false
            }
            Stmt::TryExcept(x) => {
                self.try_except(x);
                false
            }
            Stmt::TryFinally(x) => {
                self.try_finally(x);
                false
            }
            Stmt::With(x) => {
                self.with(x);
                false
            }
            Stmt::Break(_) => {
                self.finallys_down_to(self.code.bcf);
                self.jump(Jump::Goto, None);
                false
            }
            Stmt::Continue(_) => {
                self.finallys_down_to(self.code.bcf);
                self.jump(Jump::Goto, None);
                true
            }
            Stmt::FunctionDef(f) => {
                self.emit(SETLINE);
                let defaults = self.make_array(&f.args.defaults.iter().collect::<Vec<_>>());
                // `new PyFunction`, `dup`, `aload_1`, `getfield f_globals`,
                // `aload` of the defaults.
                self.emit_all(&[REF, 1, 1, REF, var(defaults)]);
                self.free_local(defaults);
                let unit = UnitRef::Function(s, f);
                let (closures, facts) = self.setup_closure(unit);
                self.compile(unit, self.code.class_name, closures);
                // `getstatic` of the code object, then the docstring
                // (`aconst_null` without one or with tuple parameters,
                // whose unpacking comes first).
                self.emit(REF);
                match docstring(&f.body) {
                    Some(doc) => self.emit_bytes(constant(doc)),
                    None => self.emit(1),
                }
                self.make_closure(closures, &facts);
                self.emit(REF);
                self.decorators(&f.decorator_list);
                self.set_name(self.ldc_name(f.name.as_str()));
                false
            }
            Stmt::ClassDef(c) => {
                self.emit(SETLINE);
                let bases = self.make_array(&c.bases.iter().collect::<Vec<_>>());
                // `ldc` of the class's name.
                self.emit_bytes(self.ldc_name(c.name.as_str()));
                self.emit(var(bases));
                let unit = UnitRef::Class(s, c);
                let (closures, facts) = self.setup_closure(unit);
                self.compile(unit, self.code.class_name, closures);
                self.emit(REF);
                self.make_closure(closures, &facts);
                // `invokestatic makeClass`.
                self.emit(REF);
                self.decorators(&c.decorator_list);
                self.set_name(self.ldc_name(c.name.as_str()));
                self.free_array(bases);
                false
            }
        }
    }

    /// `visitReturn`: the value through the return local, the `finally`
    /// bodies of every enclosing `try`, `f_lasti = -1`, `areturn`.
    fn ret(&mut self, value: Option<&'a Expr>) {
        self.emit(SETLINE);
        let tmp = value.map(|v| {
            self.expr(v);
            let t = self.return_local();
            self.emit(var(t));
            t
        });
        self.finallys_down_to(0);
        self.emit_all(&[1, 1, REF]);
        self.emit_all(&[tmp.map_or(REF, var), 1]);
    }

    /// `doTest` of `visitIf`; `body` compiles the body and says whether it
    /// exits.
    fn if_(
        &mut self,
        test: &'a Expr,
        body: &mut dyn FnMut(&mut Self) -> bool,
        orelse: &'a [Stmt],
    ) -> bool {
        self.emit(SETLINE);
        self.expr(test);
        self.emit(REF);
        let from = self.mark();
        let exit = body(self);
        let body_span = self.since(from);
        let at = self.mark();
        let orelse_exit = self.suite(orelse);
        let orelse_span = self.since(at);
        if !exit {
            // `goto` the end (the `else` list always exists, perhaps empty).
            self.jump(Jump::Goto, Some(orelse_span));
        }
        self.jump(Jump::Cond, Some(body_span + b(8)));
        exit && orelse_exit && !orelse.is_empty()
    }

    fn begin_loop(&mut self) -> usize {
        let saved = self.code.bcf;
        self.code.bcf = self.code.handlers.len();
        saved
    }

    /// `visitFor`, also for the loops Jython builds for comprehensions.
    fn for_(
        &mut self,
        target: &'a Expr,
        iter: Iter<'a>,
        body: &mut dyn FnMut(&mut Self),
        orelse: &'a [Stmt],
    ) {
        let saved = self.begin_loop();
        self.emit(SETLINE);
        match iter {
            Iter::Expr(e) => self.expr(e),
            Iter::Param => self.emit(NAME),
        }
        let it = self.get_local();
        let next = self.get_local();
        // `invokevirtual __iter__`, `astore`, `goto` the test.
        self.emit_all(&[REF, var(it)]);
        let from = self.mark();
        self.set_with(target, next);
        body(self);
        // `aload`, `invokevirtual __iternext__`, `astore`, `aload`,
        // `ifnonnull` back.
        self.emit_all(&[SETLINE, var(it), REF, var(next), var(next)]);
        let span = self.since(from);
        self.jump(Jump::Goto, Some(span));
        self.jump(Jump::Cond, Some(span + b(8)));
        self.code.bcf = saved;
        self.suite(orelse);
        self.free_local(it);
        self.free_local(next);
    }

    fn try_except(&mut self, x: &'a StmtTryExcept) {
        self.code.handlers.push(Handler::Except);
        let exit = self.suite(&x.body);
        self.code.handlers.pop();
        let from = self.mark();
        // `aload_1`, `invokestatic setException`, `astore`.
        self.emit_all(&[1, REF]);
        let exc = self.get_finally_local();
        self.emit(var(exc));
        let mut gotos = Vec::new();
        for h in &x.handlers {
            let tested = h.type_.as_deref().map(|t| {
                self.emit(var(exc));
                self.expr(t);
                self.emit(REF);
                self.mark()
            });
            if let Some(name) = &h.name {
                // `aload`, `getfield value`.
                self.emit_all(&[var(exc), REF]);
                self.set(name);
            }
            self.suite(&h.body);
            gotos.push(self.mark());
            if let Some(t) = tested {
                let span = self.since(t);
                self.jump(Jump::Cond, Some(span + b(8)));
            }
        }
        // `aload`, `athrow`.
        self.emit_all(&[var(exc), 1]);
        let handlers_end = self.mark();
        self.suite(&x.orelse);
        let end = self.mark();
        for g in gotos {
            self.jump(Jump::Goto, Some(end - g));
        }
        if !exit {
            self.jump(Jump::Goto, Some(handlers_end - from));
        }
        if let Some(slot) = self.code.slots.get_mut(exc) {
            *slot = Slot::Free;
        }
    }

    fn try_finally(&mut self, x: &'a StmtTryFinally) {
        self.code.handlers.push(Handler::Finally(&x.finalbody));
        let exc = self.get_local();
        // `aconst_null`, `astore`.
        self.emit_all(&[1, var(exc)]);
        let exit = self.suite(&x.body);
        self.code.handlers.pop();
        if !exit {
            self.suite(&x.finalbody);
        }
        let from = self.mark();
        // `astore`, `aload`, `aload_1`, `invokestatic addTraceback`.
        self.emit_all(&[var(exc), var(exc), 1, REF]);
        self.suite(&x.finalbody);
        // `aload`, `checkcast`, `athrow`.
        self.emit_all(&[var(exc), REF, 1]);
        if !exit {
            let span = self.since(from);
            self.jump(Jump::Goto, Some(span));
        }
        self.free_local(exc);
    }

    fn with(&mut self, x: &'a StmtWith) {
        self.expr(&x.context_expr);
        // `invokestatic ContextGuard.getManager`, `dup`.
        self.emit_all(&[REF, 1]);
        let mgr = self.get_local();
        // `astore`, `aload_2`, `invokeinterface __enter__`.
        self.emit_all(&[var(mgr), 1, IFACE]);
        let value = self.get_local();
        self.emit(var(value));
        self.code.handlers.push(Handler::WithExit(mgr));
        self.code.handlers.push(Handler::Except);
        if let Some(v) = &x.optional_vars {
            self.set_with(v, value);
        }
        self.free_local(value);
        let exit = self.suite(&x.body);
        self.code.handlers.pop();
        self.code.handlers.pop();
        if !exit {
            self.emit_bytes(with_exit(mgr));
            // Past the handler below (at most 23 bytes and its jump, 8)
            // and itself (5): 40 bounds it.
            self.jump(Jump::Goto, Some(b(40)));
        }
        // `aload_1`, `invokestatic setException`, `aload`, `swap`,
        // `aload_2`, `swap`, `invokeinterface __exit__`, `ifne`,
        // `invokestatic makeException`, `checkcast`, `athrow`.
        self.emit_all(&[1, REF, var(mgr), 1, 1, 1, IFACE]);
        // Past `makeException`, `checkcast`, `athrow` (7 bytes) and
        // itself.
        self.jump(Jump::Cond, Some(b(16)));
        self.emit_all(&[REF, REF, 1]);
        self.free_local(mgr);
    }

    /// `doFinallysDownTo`: the `finally` bodies and `with` exits of the
    /// handlers above `level`, each with only the handlers below it.
    fn finallys_down_to(&mut self, level: usize) {
        let mut popped = Vec::new();
        while self.code.handlers.len() > level {
            let Some(h) = self.code.handlers.pop() else {
                break;
            };
            match h {
                Handler::Finally(body) => {
                    self.suite(body);
                }
                Handler::WithExit(mgr) => self.emit_bytes(with_exit(mgr)),
                Handler::Except => {}
            }
            popped.push(h);
            if self.report.saturated {
                break;
            }
        }
        while let Some(h) = popped.pop() {
            self.code.handlers.push(h);
        }
    }

    fn aug_assign(&mut self, x: &'a StmtAugAssign) {
        self.emit(SETLINE);
        // Load the target (keeping its parts in temporaries).
        let parts = match &*x.target {
            Expr::Name(n) => {
                self.emit_bytes(self.name_bytes(&n.id));
                0
            }
            Expr::Attribute(a) => {
                self.expr(&a.value);
                self.emit_bytes(self.ldc_name(a.attr.as_str()));
                self.save_aug(2);
                self.emit(REF);
                2
            }
            Expr::Subscript(s) => {
                let n = match &*s.slice {
                    Slice::Slice(sl) => {
                        self.slice_operands(&s.value, sl);
                        self.save_aug(4);
                        self.pop(4);
                        4
                    }
                    other => {
                        self.expr(&s.value);
                        self.push(false);
                        self.slice(other);
                        self.pop(1);
                        self.save_aug(2);
                        2
                    }
                };
                self.emit(REF);
                n
            }
            other => {
                self.expr(other);
                0
            }
        };
        let target = self.store_top();
        self.expr(&x.value);
        // `aload`, `swap`, `invokevirtual __iadd__`...
        self.emit_all(&[var(target), 1, REF]);
        self.free_local(target);
        let tmp = self.store_top();
        self.code.temporary = tmp;
        match (parts, &*x.target) {
            (0, Expr::Name(n)) => {
                self.emit_bytes(self.name_bytes(&n.id));
                self.emit(var(tmp));
            }
            (0, _) => self.emit_all(&[NAME, var(tmp)]),
            (n, _) => {
                self.restore_aug(n);
                self.emit_all(&[var(tmp), REF]);
            }
        }
        self.free_local(tmp);
    }

    fn save_aug(&mut self, n: usize) {
        for k in (1..n).rev() {
            let t = self.get_local();
            self.emit(var(t));
            if let Some(slot) = self.code.aug.get_mut(k) {
                *slot = t;
            }
        }
        let t = self.get_local();
        self.emit(var(t));
        self.code.aug[0] = t;
        for k in 0..n {
            let t = self.code.aug.get(k).copied().unwrap_or(0);
            self.emit(var(t));
        }
    }

    fn restore_aug(&mut self, n: usize) {
        for k in 0..n {
            let t = self.code.aug.get(k).copied().unwrap_or(0);
            self.emit(var(t));
            self.free_local(t);
        }
    }

    // ---- Assignment ----

    /// `set(node)`: the value through a new temporary.
    fn set(&mut self, target: &'a Expr) {
        let tmp = self.store_top();
        self.set_with(target, tmp);
        // `aconst_null`, `astore`.
        self.emit_all(&[1, var(tmp)]);
        self.free_local(tmp);
    }

    fn set_with(&mut self, target: &'a Expr, tmp: usize) {
        self.code.temporary = tmp;
        self.store(target);
    }

    /// Stores `temporary` into `target` (a parameter tuple counts as a
    /// store too).
    fn store(&mut self, target: &'a Expr) {
        match target {
            Expr::Tuple(ExprTuple { elts, .. }) | Expr::List(ExprList { elts, .. }) => {
                self.seq_set(elts)
            }
            Expr::Name(n) => {
                let t = self.code.temporary;
                self.emit_bytes(self.name_bytes(&n.id));
                self.emit(var(t));
            }
            other => self.expr(other),
        }
    }

    /// `set` of a name the generator makes (`def`, `class`, `import`),
    /// whose `ldc` costs `ldc`.
    fn set_name(&mut self, ldc: Bytes) {
        let tmp = self.store_top();
        self.emit_bytes(b(NAME) - b(LDC) + ldc);
        self.emit(var(tmp));
        self.emit_all(&[1, var(tmp)]);
        self.free_local(tmp);
    }

    /// `seqSet`: `Py.unpackSequence` and a `set` per element.
    fn seq_set(&mut self, elts: &'a [Expr]) {
        let t = self.code.temporary;
        self.emit_all(&[var(t), iconst(elts.len()), REF]);
        let tmp = self.get_local();
        self.emit(var(tmp));
        for (i, e) in elts.iter().enumerate() {
            self.emit_all(&[var(tmp), iconst(i), 1]);
            self.set(e);
        }
        self.free_local(tmp);
    }

    // ---- Arrays ----

    /// `makeArray`: the elements into a new local array.
    fn make_array(&mut self, nodes: &[&'a Expr]) -> usize {
        self.make_array_with(nodes.len(), &mut |m, i| {
            if let Some(e) = nodes.get(i) {
                m.expr(e);
            }
        })
    }

    fn make_array_with(&mut self, n: usize, each: &mut dyn FnMut(&mut Self, usize)) -> usize {
        let array = self.get_local();
        if n == 0 {
            self.emit_all(&[REF, var(array)]);
        } else {
            self.emit_all(&[iconst(n), REF, var(array)]);
            for i in 0..n {
                each(self, i);
                // `aload`, `swap`, `iconst`, `swap`, `aastore`.
                self.emit_all(&[var(array), 1, iconst(i), 1, 1]);
            }
        }
        array
    }

    /// `freeArray`: `aload`, `aconst_null`, `invokestatic Arrays.fill`.
    fn free_array(&mut self, array: usize) {
        self.emit_all(&[var(array), 1, REF]);
        self.free_local(array);
    }

    /// `makeStrings`: an array of `names` in a new local.
    fn make_strings(&mut self, names: &[&str]) -> usize {
        self.emit_all(&[iconst(names.len()), REF]);
        let s = self.get_local();
        self.emit(var(s));
        for (i, name) in names.iter().enumerate() {
            self.emit_all(&[var(s), iconst(i)]);
            self.emit_bytes(self.ldc_name(name));
            self.emit(1);
        }
        s
    }

    /// `loadArray`: the elements on the stack, or for 100 and more number
    /// and string literals, filled by `set$$N` methods of their own.
    fn load_array(&mut self, nodes: &[&'a Expr]) {
        let n = nodes.len();
        if n == 0 {
            self.emit(REF);
            return;
        }
        if n >= SETTERS_FROM
            && nodes
                .iter()
                .all(|e| matches!(e, Expr::Num(_) | Expr::Str(_)))
        {
            self.emit_all(&[iconst(n), REF]);
            for chunk in nodes.chunks(PER_SETTER) {
                // `dup`, `invokestatic set$$N`.
                self.emit_all(&[1, REF]);
                // Per element `aload_0`, `iconst` of the index, the
                // constant, `aastore`; `return`.
                let size = chunk
                    .iter()
                    .fold(b(1), |s, e| s + sum(&[1, 3, 1]) + constant(e));
                self.report.setters = self.report.setters.max(size);
            }
            return;
        }
        self.emit_all(&[iconst(n), REF]);
        for (i, e) in nodes.iter().enumerate() {
            // `dup`, `iconst`, the element, `aastore`.
            self.emit_all(&[1, iconst(i)]);
            self.expr(e);
            self.emit(1);
        }
    }

    /// A list, tuple, set or dict display (`new`, `dup`, the array,
    /// `invokespecial`; in a generator through a local array).
    fn display(&mut self, nodes: &[&'a Expr]) {
        if self.code.generator {
            let content = self.make_array(nodes);
            self.emit_all(&[REF, 1, var(content), REF]);
            self.free_array(content);
        } else {
            self.emit_all(&[REF, 1]);
            self.load_array(nodes);
            self.emit(REF);
        }
    }

    // ---- Nested units ----

    /// `setup_closure` of a unit about to be compiled: the number of
    /// calls so far (this one included) and the unit's facts.
    fn setup_closure(&mut self, unit: UnitRef<'a>) -> (usize, Facts) {
        let key = unit.key();
        let n = self.closures.entry(key).or_insert(0);
        *n = n.saturating_add(1);
        let n = *n;
        let facts = self.facts.get(&key).copied().unwrap_or_default();
        (n, facts)
    }

    /// `makeClosure`: an array of the cells of the free variables (per
    /// variable `aload`, `iconst`, `aload_1`, a `getfield f_back` per
    /// class body in between, `iconst`, `invokevirtual getclosure`,
    /// `aastore`).
    fn make_closure(&mut self, closures: usize, facts: &Facts) {
        let n = facts.free.saturating_mul(closures);
        if n == 0 {
            return;
        }
        let tmp = self.get_local();
        self.emit_all(&[iconst(n), REF, var(tmp)]);
        let back = count(facts.distance.saturating_sub(1)) * b(REF);
        for i in 0..n {
            self.emit_all(&[var(tmp), iconst(i), 1, 3, REF, 1]);
            self.emit_bytes(back);
            // Count work per 4,096 cells (iglint's own granularity).
            if i % 4096 == 0 && self.tick() {
                return;
            }
        }
        self.emit(var(tmp));
        self.free_local(tmp);
    }

    /// `applyDecorators`.
    fn decorators(&mut self, decorators: &'a [Expr]) {
        if decorators.is_empty() {
            return;
        }
        let res = self.store_top();
        for d in decorators {
            self.expr(d);
            self.push(false);
        }
        for _ in decorators {
            self.pop(1);
            // `aload_2`, `aload`, `invokevirtual __call__`, `astore`.
            self.emit_all(&[1, var(res), REF, var(res)]);
        }
        self.emit(var(res));
        self.free_local(res);
    }

    /// The body of a generator expression or set or dict comprehension:
    /// a loop per `for` (the first over the parameter), an `if` per
    /// condition, and `yield` of the element.
    fn gen_body(&mut self, e: &'a Expr) {
        let (value, generators) = match e {
            Expr::GeneratorExp(g) => (Value::Expr(&g.elt), &g.generators[..]),
            Expr::SetComp(g) => (Value::Expr(&g.elt), &g.generators[..]),
            Expr::DictComp(g) => (Value::Pair(&g.key, &g.value), &g.generators[..]),
            _ => return,
        };
        self.comprehension(generators, 0, true, &mut |m| {
            // `Expr(Yield(elt))`: the line, the `yield`, `pop`.
            m.emit(SETLINE);
            m.yield_(Some(value));
            m.emit(1);
        });
    }

    /// The loops of a comprehension from generator `i` on, with `inner`
    /// in the innermost.
    fn comprehension(
        &mut self,
        generators: &'a [Comprehension],
        i: usize,
        param: bool,
        inner: &mut dyn FnMut(&mut Self),
    ) {
        let Some(g) = generators.get(i) else {
            inner(self);
            return;
        };
        let iter = if i == 0 && param {
            Iter::Param
        } else {
            Iter::Expr(&g.iter)
        };
        self.for_(
            &g.target,
            iter,
            &mut |m| m.conditions(generators, i, 0, inner),
            &[],
        );
    }

    fn conditions(
        &mut self,
        generators: &'a [Comprehension],
        i: usize,
        j: usize,
        inner: &mut dyn FnMut(&mut Self),
    ) {
        let Some(g) = generators.get(i) else {
            return;
        };
        match g.ifs.get(j) {
            Some(test) => {
                self.if_(
                    test,
                    &mut |m| {
                        m.conditions(generators, i, j.saturating_add(1), inner);
                        false
                    },
                    &[],
                );
            }
            None => self.comprehension(generators, i.saturating_add(1), false, inner),
        }
    }

    // ---- Expressions ----

    fn name(&mut self, id: &str, ctx: ExprContext) {
        self.emit_bytes(self.name_bytes(id));
        if matches!(
            ctx,
            ExprContext::Store | ExprContext::Param | ExprContext::AugStore
        ) {
            let t = self.code.temporary;
            self.emit(var(t));
        }
    }

    fn expr(&mut self, e: &'a Expr) {
        if self.tick() {
            return;
        }
        match e {
            Expr::Name(n) => self.name(&n.id, n.ctx),
            Expr::Num(_) | Expr::Str(_) => self.emit_bytes(constant(e)),
            Expr::Attribute(a) => {
                self.expr(&a.value);
                self.emit_bytes(self.ldc_name(a.attr.as_str()));
                match a.ctx {
                    ExprContext::Store | ExprContext::Param | ExprContext::AugStore => {
                        let t = self.code.temporary;
                        self.emit_all(&[var(t), REF]);
                    }
                    _ => self.emit(REF),
                }
            }
            Expr::Subscript(s) => {
                let store = matches!(
                    s.ctx,
                    ExprContext::Store | ExprContext::Param | ExprContext::AugStore
                );
                match &*s.slice {
                    Slice::Slice(sl) => {
                        self.slice_operands(&s.value, sl);
                        self.pop(4);
                        let t = self.code.temporary;
                        self.emit_all(&[if store { var(t) } else { 0 }, REF]);
                    }
                    other => {
                        let value = self.code.temporary;
                        self.expr(&s.value);
                        self.push(false);
                        self.slice(other);
                        self.pop(1);
                        self.emit_all(&[if store { var(value) } else { 0 }, REF]);
                    }
                }
            }
            Expr::Call(c) => self.call(c),
            Expr::UnaryOp(u) => {
                self.expr(&u.operand);
                self.emit(REF);
            }
            Expr::BinOp(x) => {
                self.expr(&x.left);
                self.push(false);
                self.expr(&x.right);
                self.pop(1);
                self.emit(REF);
            }
            Expr::Repr(r) => {
                self.expr(&r.value);
                self.emit(REF);
            }
            Expr::BoolOp(x) => {
                let mut marks = Vec::new();
                for (i, v) in x.values.iter().enumerate() {
                    if i > 0 {
                        // `dup`, `invokevirtual __nonzero__`, the jump, `pop`.
                        self.emit_all(&[1, REF]);
                        marks.push(self.mark());
                        self.emit(1);
                    }
                    self.expr(v);
                }
                let end = self.mark();
                for m in marks {
                    self.jump(Jump::Cond, Some(end - m + b(8)));
                }
            }
            Expr::Compare(x) => {
                let last = self.get_local();
                let result = self.get_local();
                self.expr(&x.left);
                self.emit(var(last));
                let n = x.comparators.len();
                let mut marks = Vec::new();
                for (i, c) in x.comparators.iter().enumerate() {
                    self.expr(c);
                    // `aload`, `swap`, (`dup`, `astore`,) the comparison.
                    if i.saturating_add(1) < n {
                        self.emit_all(&[var(last), 1, 1, var(last), REF, 1, var(result), REF]);
                        marks.push(self.mark());
                    } else {
                        self.emit_all(&[var(last), 1, REF]);
                    }
                }
                if n > 1 {
                    self.emit_all(&[var(result), var(result)]);
                }
                // `aconst_null`, `astore`.
                self.emit_all(&[1, var(last)]);
                let end = self.mark();
                for m in marks {
                    self.jump(Jump::Cond, Some(end - m + b(8)));
                }
                self.free_local(last);
                self.free_local(result);
            }
            Expr::List(ExprList { elts, ctx, .. }) | Expr::Tuple(ExprTuple { elts, ctx, .. }) => {
                match ctx {
                    ExprContext::Store | ExprContext::Param => self.seq_set(elts),
                    ExprContext::Del => elts.iter().for_each(|e| self.expr(e)),
                    _ => self.display(&elts.iter().collect::<Vec<_>>()),
                }
            }
            Expr::Set(s) => self.display(&s.elts.iter().collect::<Vec<_>>()),
            Expr::Dict(d) => {
                let mut nodes = Vec::new();
                for (k, v) in d.keys.iter().zip(&d.values) {
                    nodes.push(k);
                    nodes.push(v);
                }
                self.display(&nodes);
            }
            Expr::IfExp(x) => {
                self.emit(SETLINE);
                self.expr(&x.test);
                self.emit(REF);
                let from = self.mark();
                self.expr(&x.body);
                let body_span = self.since(from);
                let at = self.mark();
                self.expr(&x.orelse);
                let orelse_span = self.since(at);
                self.jump(Jump::Goto, Some(orelse_span));
                self.jump(Jump::Cond, Some(body_span + b(8)));
            }
            Expr::Yield(y) => self.yield_(y.value.as_deref().map(Value::Expr)),
            Expr::ListComp(x) => {
                // `new PyList`, `dup`, `invokespecial`, `dup`, `ldc
                // append`, `invokevirtual __getattr__`, `set` of `_[n]`.
                self.emit_all(&[REF, 1, REF, 1, LDC, REF]);
                // `_[n]` is short.
                self.set_name(b(LDC));
                let elt = &x.elt;
                self.comprehension(&x.generators, 0, false, &mut |m| {
                    // `_[n](elt)` as a statement: the line, the name,
                    // `aload_2`, the element, `invokevirtual __call__`,
                    // `pop`.
                    m.emit(SETLINE + NAME);
                    m.push(false);
                    m.emit(1);
                    m.push(true);
                    m.expr(elt);
                    m.pop(2);
                    m.emit(REF + 1);
                });
                // `del _[n]`.
                self.emit_all(&[SETLINE, NAME]);
            }
            Expr::Lambda(l) => {
                self.emit(SETLINE);
                let defaults = self.make_array(&l.args.defaults.iter().collect::<Vec<_>>());
                // `new PyFunction`, `dup`, `aload`, `aload_1`, `getfield
                // f_globals`, `swap`.
                self.emit_all(&[REF, 1, var(defaults), 1, REF, 1]);
                self.free_local(defaults);
                let unit = UnitRef::Lambda(e, l);
                let (closures, facts) = self.setup_closure(unit);
                self.compile(unit, self.code.class_name, closures);
                self.emit(REF);
                self.make_closure(closures, &facts);
                self.emit(REF);
            }
            Expr::GeneratorExp(ExprGeneratorExp { generators, .. })
            | Expr::SetComp(ExprSetComp { generators, .. })
            | Expr::DictComp(ExprDictComp { generators, .. }) => {
                self.emit(SETLINE);
                match e {
                    // `new PySet`, `dup`.
                    Expr::SetComp(_) => self.emit_all(&[REF, 1]),
                    // `new PyDictionary`, `dup`, `invokespecial`, `dup`.
                    Expr::DictComp(_) => self.emit_all(&[REF, 1, REF, 1]),
                    _ => {}
                }
                // `new PyFunction`, `dup`, `aload_1`, `getfield f_globals`.
                self.emit_all(&[REF, 1, 1, REF]);
                let empty = self.make_array(&[]);
                self.emit(var(empty));
                let unit = UnitRef::Gen(e);
                let (closures, facts) = self.setup_closure(unit);
                self.compile(unit, self.code.class_name, closures);
                // `getstatic`, `aconst_null`.
                self.emit_all(&[REF, 1]);
                self.make_closure(closures, &facts);
                self.emit(REF);
                let created = self.store_top();
                if let Some(g) = generators.first() {
                    self.expr(&g.iter);
                }
                // `aload`, `swap`, `invokevirtual __iter__`, `aload_2`,
                // `swap`, `invokevirtual __call__`.
                self.emit_all(&[var(created), 1, REF, 1, 1, REF]);
                self.free_local(created);
                self.free_array(empty);
                if !matches!(e, Expr::GeneratorExp(_)) {
                    self.emit(REF);
                }
            }
        }
    }

    /// The operands of a simple slice (`x[a:b]`): the object, then the
    /// bounds or `aconst_null`, each recorded on the stack.
    fn slice_operands(&mut self, value: &'a Expr, sl: &'a SliceSlice) {
        self.expr(value);
        self.push(false);
        for part in [&sl.lower, &sl.upper, &sl.step] {
            match part.as_deref() {
                Some(e) => self.expr(e),
                None => self.emit(1),
            }
            self.push(false);
        }
    }

    fn slice(&mut self, s: &'a Slice) {
        match s {
            Slice::Index(i) => self.expr(&i.value),
            // `getstatic Py.Ellipsis`.
            Slice::Ellipsis(_) => self.emit(REF),
            Slice::ExtSlice(x) => {
                let dims = &x.dims;
                let array = self.make_array_with(dims.len(), &mut |m, i| {
                    if let Some(d) = dims.get(i) {
                        m.slice(d);
                    }
                });
                // `new PyTuple`, `dup`, `aload`, `invokespecial`.
                self.emit_all(&[REF, 1, var(array), REF]);
                self.free_array(array);
            }
            Slice::Slice(sl) => {
                // `visitSlice`: a `PySlice` of the bounds (`None` for a
                // missing one), the step through a temporary.
                for (i, part) in [&sl.lower, &sl.upper, &sl.step].into_iter().enumerate() {
                    match part.as_deref() {
                        Some(e) => self.expr(e),
                        None => self.emit(REF),
                    }
                    if i < 2 {
                        self.push(false);
                    }
                }
                let step = self.store_top();
                self.pop(2);
                // `new`, `dup`, `dup2_x2`, `pop2`, `aload`, `invokespecial`.
                self.emit_all(&[REF, 1, 1, 1, var(step), REF]);
                self.free_local(step);
            }
        }
    }

    fn call(&mut self, c: &'a ExprCall) {
        let mut values: Vec<&'a Expr> = c.args.iter().collect();
        values.extend(c.keywords.iter().map(|k| &k.value));
        let plain = c.keywords.is_empty() && c.starargs.is_none() && c.kwargs.is_none();
        if plain && let Expr::Attribute(a) = &*c.func {
            // `invokeNoKeywords`: the object, `ldc`, `invokevirtual
            // __getattr__`, `aload_2`.
            self.expr(&a.value);
            self.push(false);
            self.emit_bytes(self.ldc_name(a.attr.as_str()));
            self.emit_all(&[REF, 1]);
            self.push(true);
            self.call_args(&values);
            return;
        }
        self.expr(&c.func);
        self.push(false);
        if c.starargs.is_some() || c.kwargs.is_some() {
            let array = self.make_array(&values);
            let keywords: Vec<&str> = c.keywords.iter().map(|k| k.arg.as_str()).collect();
            let strings = self.make_strings(&keywords);
            for part in [&c.starargs, &c.kwargs] {
                match part.as_deref() {
                    Some(e) => self.expr(e),
                    None => self.emit(1),
                }
                self.push(false);
            }
            // `aload`, `aload`, `dup2_x2`, `pop2`, `invokevirtual
            // _callextra`, `aconst_null`, `astore`.
            self.emit_all(&[var(array), var(strings), 1, 1]);
            self.free_local(strings);
            self.pop(3);
            self.emit_all(&[REF, 1, var(array)]);
            self.free_local(array);
        } else if !c.keywords.is_empty() {
            self.emit(1);
            self.push(true);
            let array = self.make_array(&values);
            let keywords: Vec<&str> = c.keywords.iter().map(|k| k.arg.as_str()).collect();
            let strings = self.make_strings(&keywords);
            self.emit_all(&[var(array), var(strings)]);
            self.free_local(strings);
            self.pop(2);
            self.emit_all(&[REF, 1, var(array)]);
            self.free_local(array);
        } else {
            self.emit(1);
            self.push(true);
            self.call_args(&values);
        }
    }

    /// The positional arguments of a call without keywords: up to four on
    /// the stack, more in an array; `invokevirtual __call__`.
    fn call_args(&mut self, values: &[&'a Expr]) {
        let n = values.len();
        if n <= 4 {
            for (i, v) in values.iter().enumerate() {
                self.expr(v);
                if i.saturating_add(1) < n {
                    self.push(false);
                }
            }
            self.pop(n.saturating_sub(1).saturating_add(2));
            self.emit(REF);
        } else {
            let array = self.make_array(values);
            self.emit(var(array));
            self.free_local(array);
            self.pop(2);
            self.emit(REF);
        }
    }

    /// `visitYield`: the stack and the locals saved into the frame,
    /// `areturn`, and restored at the restart point.
    fn yield_(&mut self, value: Option<Value<'a>>) {
        self.emit(SETLINE);
        // `saveStack`.
        let array = if self.code.stack.is_empty() {
            None
        } else {
            let array = self.get_local();
            self.emit_all(&[iconst(self.code.stack.len()), REF, var(array)]);
            let mut i = 0usize;
            let items: Vec<bool> = self.code.stack.iter().rev().copied().collect();
            for thread_state in items {
                if thread_state {
                    self.emit(1);
                    i = i.saturating_add(1);
                } else {
                    self.emit_all(&[var(array), 1, iconst(i), 1, 1]);
                    i = i.saturating_add(2);
                }
            }
            Some(array)
        };
        match value {
            Some(Value::Expr(v)) => self.expr(v),
            Some(Value::Pair(k, v)) => {
                // The `(key, value)` tuple, built in the generator.
                let pair = [k, v];
                let content = self.make_array(&pair);
                self.emit_all(&[REF, 1, var(content), REF]);
                self.free_array(content);
            }
            None => self.emit(REF),
        }
        self.code.yields = self.code.yields.saturating_add(1);
        // `f_lasti = n`.
        self.emit_all(&[1, iconst(self.code.yields), REF]);
        // `saveLocals`.
        let active = self.active();
        let n = self.code.slots.len();
        self.emit_all(&[iconst(n), REF]);
        let locals = self.get_local();
        self.emit(var(locals));
        for &i in &active {
            // `CodeCompiler.saveLocals` stores `aconst_null` instead of
            // `aload` for local 2222 (a debugging leftover in Jython).
            let load = if i == 2222 { 1 } else { var(i) };
            self.emit_all(&[var(locals), iconst(i), load, 1]);
        }
        // `aload_1`, `aload`, `putfield f_savedlocals`, `areturn`.
        self.emit_all(&[1, var(locals), REF, 1]);
        self.free_local(locals);
        // `restoreLocals`.
        let active = self.active();
        self.emit_all(&[1, REF]);
        let locals = self.get_local();
        self.emit(var(locals));
        for &i in &active {
            self.emit_all(&[var(locals), iconst(i), 1, REF, var(i)]);
        }
        self.free_local(locals);
        // `restoreStack`.
        if let Some(array) = array {
            let mut i = self.code.stack.len().saturating_sub(1);
            let items: Vec<bool> = self.code.stack.clone();
            for thread_state in items {
                if thread_state {
                    self.emit(1);
                } else {
                    self.emit_all(&[var(array), iconst(i), 1, REF]);
                    i = i.saturating_sub(1);
                }
            }
            self.free_local(array);
        }
        // `aload_1`, `invokevirtual getGeneratorInput`, `dup`,
        // `instanceof`, `ifeq`, `checkcast`, `athrow`, `checkcast`.
        self.emit_all(&[1, REF, 1, REF, 3, REF, 1, REF]);
    }
}

/// The exit call of a `with` statement inlined at a way out: `aload`,
/// `aload_2`, `aconst_null`, `invokeinterface __exit__`, `pop`.
fn with_exit(mgr: usize) -> Bytes {
    sum(&[var(mgr), 1, 1, IFACE, 1])
}

/// The length of the name Jython gives a tuple parameter
/// (`ArgListCompiler.visitTuple`: `(a, (b, c))`), at least.
pub(super) fn tuple_name_len(e: &Expr) -> usize {
    match e {
        Expr::Tuple(ExprTuple { elts, .. }) | Expr::List(ExprList { elts, .. }) => {
            elts.iter().map(tuple_name_len).fold(
                2usize.saturating_mul(elts.len()).saturating_add(2),
                usize::saturating_add,
            )
        }
        Expr::Name(n) => utf16_len(&n.id),
        _ => 0,
    }
}

/// The code of `PyCodeConstant.put` in `<init>` for one compiled unit:
/// the argument count, an array of the variable names (none for a class
/// body), the name (`co_name`, whose `ldc` costs `co_name`), line and
/// flags, the cell and free variable arrays, `invokestatic Py.newCode`,
/// `putstatic`.
fn init_bytes(
    facts: &Facts,
    unit: UnitRef<'_>,
    closures: usize,
    id: usize,
    co_name: Bytes,
) -> Bytes {
    // `iconst`, `anewarray`, `astore_1`, per name `aload_1`, `iconst`,
    // the `ldc` (`ldc` bytes in all), `aastore`; then `aload_1`.
    let strings = |n: usize, ldc: Bytes| -> Bytes {
        let mut s = sum(&[iconst(n), REF, 1, 1]);
        for i in 0..n {
            s += sum(&[1, iconst(i), 1]);
        }
        s + ldc
    };
    let optional = |n: usize, ldc: Bytes| if n == 0 { b(1) } else { strings(n, ldc) };
    let (names, names_ldc) = match unit {
        UnitRef::Class(..) => (0, b(0)),
        _ => (facts.names, facts.names_ldc),
    };
    b(iconst(facts.argcount))
        + strings(names, names_ldc)
        // `aload_1`, the name, `iconst` line, two `iconst`, `getstatic
        // self`, `iconst` id.
        + b(1)
        + co_name
        + sum(&[3, 1, 1, REF, iconst(id)])
        + optional(facts.cells, facts.cells_ldc)
        + optional(
            facts.free.saturating_mul(closures),
            facts.free_ldc * count(closures),
        )
        // `iconst` pure cells, `iconst` flags, `invokestatic`, `putstatic`.
        + sum(&[iconst(facts.cells), 3, REF, REF])
}
