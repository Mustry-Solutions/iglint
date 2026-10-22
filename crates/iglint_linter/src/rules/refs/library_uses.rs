//! Uses of the project script library in a script, for `REF002` and
//! `REF003`.
//!
//! A use is an attribute chain read by the script (`core.util.fmt`,
//! `u.fmt` after `u = core.util`, `fmt` after `from core.util import fmt`)
//! whose dotted name starts with a top-level library name, an `import`
//! statement of a library module, or (in a library module) a call of one
//! of the module's own top-level functions by its bare name.

use iglint_python::ast::{Expr, ExprCall, ExprContext, Stmt};
use iglint_python::visitor::{Visitor, walk_expr, walk_stmt};
use iglint_python_semantic::{BindingKind, Resolution, ScopeId, SemanticModel, UnboundNames};
use iglint_text::{Ranged, TextRange};

use crate::checker::Checker;
use crate::rule::ScriptUnit;
use crate::rules::script::semantic;
use crate::script_facts::calls::qualified;

/// One use of a library name.
pub(crate) struct LibraryUse<'s> {
    /// The dotted name, e.g. `core.util.fmt`.
    pub dotted: String,
    /// For every segment of `dotted`, where it is written in the script.
    /// Segments that come from an alias or an import share the range of
    /// the written name.
    pub segments: Vec<TextRange>,
    /// How many leading segments come from the written root name through
    /// an alias or an import (`fmt` after `from core.util import fmt`
    /// stands for 3 segments: 2 are aliased). A missing aliased segment is
    /// reported where the alias is made.
    pub aliased: usize,
    /// The call, when the chain is called.
    pub call: Option<&'s ExprCall>,
    /// Inside a `try` whose handlers catch the error a missing name raises
    /// (`AttributeError`, `ImportError`, `NameError`, `Exception`, bare).
    pub guarded: bool,
}

/// The library uses of a script of a project. `None` when the script does
/// not parse or has no project.
pub(crate) fn library_uses<'s>(
    cx: &Checker<'_, '_>,
    unit: &ScriptUnit<'s, '_>,
) -> Option<Vec<LibraryUse<'s>>> {
    let (index, project) = cx.project_index()?;
    let model = semantic(cx, unit)?;
    let library = index.library(project);
    let module = match cx.data() {
        iglint_project::ModelData::Library(l) => Some(l.module_path.clone()),
        _ => None,
    };
    let mut collector = Collector {
        model,
        is_global: &|name: &str| library.is_global(name),
        module: module.as_deref(),
        guard: 0,
        out: Vec::new(),
    };
    collector.visit_module(model.module);
    Some(collector.out)
}

struct Collector<'m, 's, 'f> {
    model: &'m SemanticModel<'s>,
    is_global: &'f dyn Fn(&str) -> bool,
    module: Option<&'f str>,
    guard: u32,
    out: Vec<LibraryUse<'s>>,
}

/// Exception names whose handlers make a missing library name a handled
/// condition.
const GUARDS: &[&str] = &[
    "AttributeError",
    "ImportError",
    "NameError",
    "Exception",
    "BaseException",
    "StandardError",
    "Throwable",
];

fn catches_missing_names(handlers: &[iglint_python::ast::ExceptHandler]) -> bool {
    handlers.iter().any(|h| match h.type_.as_deref() {
        None => true,
        Some(t) => {
            let names: Vec<&Expr> = match t {
                Expr::Tuple(tuple) => tuple.elts.iter().collect(),
                other => vec![other],
            };
            names.iter().any(|n| {
                let last = match n {
                    Expr::Name(n) => n.id.as_ref(),
                    Expr::Attribute(a) => a.attr.as_str(),
                    _ => return true,
                };
                GUARDS.contains(&last)
            })
        }
    })
}

fn chain_segments(e: &Expr, out: &mut Vec<TextRange>) -> bool {
    match e {
        Expr::Name(n) => {
            out.push(n.range);
            true
        }
        Expr::Attribute(a) => {
            if !chain_segments(&a.value, out) {
                return false;
            }
            out.push(a.attr.range);
            true
        }
        _ => false,
    }
}

impl<'s> Collector<'_, 's, '_> {
    fn dotted(&self, e: &Expr) -> Option<String> {
        if let Some(q) = qualified(self.model, e, UnboundNames::Reject) {
            let root = q.split('.').next().unwrap_or("");
            return (self.is_global)(root).then_some(q);
        }
        // A module's own top-level function or class, by its bare name.
        let module = self.module?;
        let Expr::Name(n) = e else { return None };
        let r = self.model.reference_at(n.range)?;
        let module_level = match r.resolution {
            Resolution::Global => true,
            Resolution::Local => r.scope == ScopeId::MODULE,
            _ => false,
        };
        let defs = !r.bindings.is_empty()
            && r.bindings.iter().all(|b| {
                let b = self.model.binding(*b);
                b.scope == ScopeId::MODULE
                    && matches!(b.kind, BindingKind::FunctionDef | BindingKind::ClassDef)
            });
        (module_level && defs).then(|| format!("{module}.{}", n.id))
    }

    fn record(&mut self, e: &'s Expr, call: Option<&'s ExprCall>) {
        let Some(dotted) = self.dotted(e) else {
            return;
        };
        let mut written = Vec::new();
        if !chain_segments(e, &mut written) {
            return;
        }
        let total = dotted.split('.').count();
        let Some(extra) = total.checked_sub(written.len()) else {
            return;
        };
        let mut segments = vec![written[0]; extra + 1];
        segments.extend_from_slice(&written[1..]);
        self.out.push(LibraryUse {
            dotted,
            segments,
            aliased: extra,
            call,
            guarded: self.guard > 0,
        });
    }

    fn import(&mut self, dotted: String, range: TextRange) {
        let root = dotted.split('.').next().unwrap_or("");
        if !(self.is_global)(root) {
            return;
        }
        let n = dotted.split('.').count();
        self.out.push(LibraryUse {
            dotted,
            segments: vec![range; n],
            aliased: 0,
            call: None,
            guarded: self.guard > 0,
        });
    }

    fn expr(&mut self, e: &'s Expr, call: Option<&'s ExprCall>) {
        match e {
            Expr::Name(n) if matches!(n.ctx, ExprContext::Load | ExprContext::AugLoad) => {
                self.record(e, call);
            }
            Expr::Attribute(a) if is_chain(e) => {
                if matches!(a.ctx, ExprContext::Load | ExprContext::AugLoad) {
                    self.record(e, call);
                } else {
                    self.expr(&a.value, None);
                }
            }
            Expr::Call(c) => {
                self.expr(&c.func, Some(c));
                for a in &c.args {
                    self.visit_expr(a);
                }
                for k in &c.keywords {
                    self.visit_expr(&k.value);
                }
                if let Some(s) = &c.starargs {
                    self.visit_expr(s);
                }
                if let Some(k) = &c.kwargs {
                    self.visit_expr(k);
                }
            }
            other => walk_expr(self, other),
        }
    }
}

fn is_chain(e: &Expr) -> bool {
    match e {
        Expr::Name(_) => true,
        Expr::Attribute(a) => is_chain(&a.value),
        _ => false,
    }
}

impl<'s> Visitor<'s> for Collector<'_, 's, '_> {
    fn visit_expr(&mut self, expr: &'s Expr) {
        self.expr(expr, None);
    }

    fn visit_stmt(&mut self, stmt: &'s Stmt) {
        match stmt {
            Stmt::TryExcept(t) => {
                let guarded = catches_missing_names(&t.handlers);
                if guarded {
                    self.guard += 1;
                }
                for s in &t.body {
                    self.visit_stmt(s);
                }
                if guarded {
                    self.guard -= 1;
                }
                for h in &t.handlers {
                    self.visit_except_handler(h);
                }
                for s in &t.orelse {
                    self.visit_stmt(s);
                }
            }
            Stmt::Import(i) => {
                for alias in &i.names {
                    self.import(alias.name.as_str().to_owned(), alias.name.range());
                }
            }
            Stmt::ImportFrom(i) => {
                if i.level == 0
                    && let Some(m) = &i.module
                {
                    for alias in &i.names {
                        if alias.name.as_str() != "*" {
                            self.import(
                                format!("{}.{}", m.as_str(), alias.name.as_str()),
                                alias.name.range(),
                            );
                        }
                    }
                }
            }
            _ => walk_stmt(self, stmt),
        }
    }
}
