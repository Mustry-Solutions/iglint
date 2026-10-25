//! The top-level names of a library module, for the project index.
//!
//! A module's namespace after import holds every name its module-level code
//! binds, and every name a function binds through `global`. A name bound
//! exactly once by a plain `def` gets the function's signature; a name bound
//! several times (conditional definitions, redefinitions, `global`
//! assignments) or by a decorated `def` is a [`SymbolKind::Variable`], whose
//! calls are never checked. A module whose namespace cannot be known
//! (`from x import *`, `exec` without a namespace, writes through
//! `globals()`, `eval`, `execfile`) is marked dynamic: lookups of names it
//! does not define are then unknown rather than missing.
//!
//! A name the module may rebind at run time in a way the bindings do not
//! show is a [`SymbolKind::Variable`] too, so that no rule trusts the body
//! or signature of a `def` that may no longer be the module's function:
//! every name when the module runs code from strings (`exec`, also inside
//! a function, where the code may declare a name `global`; `eval`;
//! `execfile`) or writes its own namespace (`globals()[...]`,
//! `setattr(sys.modules[__name__], ...)`), a name a `from x import *` may
//! overwrite (the import runs after the binding or in a loop), and a
//! function whose code or defaults the module replaces
//! (`f.func_code = ...`, `setattr(f, name, ...)`).
//! Rebinding from other resources (`core.util.f = ...`, the module used as
//! a value, a function of the module used as a value, `func_globals`) is
//! known only to the project index (`ProjectIndex::library_name_rebound`).
//! A module with a frame escape route of either kind
//! ([`iglint_python_semantic::dynamic::frame_escape`]) may write its
//! globals, so every name is a variable and the namespace is dynamic. That
//! is the view every consumer trusts for fixes. When only routes that do
//! not observe names by themselves make it so (a reflective Java member
//! such as `.type`, a computed `getattr`), the view without them is kept
//! for findings (`ModuleSymbols::findings`). Variables bound only to
//! literals are listed as constants: they never hold a function.

use iglint_project::library::{
    ModuleSymbols, Parameter, ParameterKind, Signature, StarExports, Symbol, SymbolKind,
    VariableImport,
};
use iglint_python::ast::{Arguments, Expr, ExprContext, Module, Stmt};
use iglint_python::visitor::{Visitor, walk_expr, walk_stmt};
use iglint_python_semantic::{BindingKind, ScopeId, SemanticModel};
use iglint_text::TextRange;
use rustc_hash::FxHashSet;

/// The symbols of a module, from its semantic model and syntax tree.
pub fn module_symbols(model: &SemanticModel<'_>, ast: &Module) -> ModuleSymbols {
    let mut symbols = symbols_of(model, ast, true);
    if model.frame_escape().is_some() {
        let findings = symbols_of(model, ast, false);
        if findings != symbols {
            symbols.findings = Some(Box::new(findings));
        }
    }
    symbols
}

/// [`module_symbols`], with every escape route counting as a write of
/// any global (`escapes`, for fixes) or only the routes that observe
/// names (for findings).
fn symbols_of(model: &SemanticModel<'_>, ast: &Module, escapes: bool) -> ModuleSymbols {
    let module = model.scope(ScopeId::MODULE);
    let mut rebinds = Rebinds::collect(model, ast);
    rebinds.anything |= escapes && model.frame_escape().is_some();
    let dynamic = rebinds.anything || module.star_import.is_some();
    let mut symbols = Vec::new();
    let mut constants = Vec::new();
    let mut variable_imports = Vec::new();
    for name in module.local_names() {
        let ids = module.bindings_of(name);
        let real: Vec<_> = ids
            .iter()
            .map(|id| model.binding(*id))
            .filter(|b| !matches!(b.kind, BindingKind::Global))
            .collect();
        let Some(first) = real.first() else {
            continue;
        };
        let rebound = rebinds.anything
            || rebinds.attributes_set.contains(name)
            || rebinds.star_imports.iter().any(|(star, in_loop)| {
                *in_loop || real.iter().any(|b| b.range.start() < star.start())
            });
        if !rebound
            && real.iter().all(|b| {
                b.kind == BindingKind::Assignment
                    && !b.unpacked
                    && b.value.is_some_and(|v| is_literal(model, v))
            })
        {
            constants.push((*name).to_owned());
        }
        let kind = if real.len() > 1 || rebound {
            SymbolKind::Variable
        } else {
            match first.kind {
                BindingKind::FunctionDef => match first.stmt {
                    Some(Stmt::FunctionDef(f)) if f.decorator_list.is_empty() => {
                        let mut signature = signature(&f.args);
                        signature.uses = super::access::param_uses(model, f);
                        SymbolKind::Function(signature)
                    }
                    _ => SymbolKind::Variable,
                },
                BindingKind::ClassDef => SymbolKind::Class,
                BindingKind::Import | BindingKind::FromImport => SymbolKind::Import {
                    target: first.qualified.clone(),
                },
                _ => SymbolKind::Variable,
            }
        };
        if !matches!(kind, SymbolKind::Import { .. }) {
            // The name may hold what any of its imports binds; when it has
            // other bindings, or a rebinding the module shows, nothing says
            // which.
            let several = real.len() > 1
                || rebinds.attributes_set.contains(name)
                || rebinds.star_imports.iter().any(|(star, in_loop)| {
                    *in_loop || real.iter().any(|b| b.range.start() < star.start())
                });
            for b in &real {
                if matches!(b.kind, BindingKind::Import | BindingKind::FromImport)
                    && let Some(target) = &b.qualified
                {
                    variable_imports.push(VariableImport {
                        name: (*name).to_owned(),
                        target: target.clone(),
                        several,
                    });
                }
            }
        }
        symbols.push(Symbol {
            name: (*name).to_owned(),
            kind,
            range: first.range,
        });
    }
    let unknown = iglint_python_semantic::vetted::unknown_code(model);
    ModuleSymbols {
        symbols,
        dynamic,
        constants,
        // A frame/interpreter route counts as unknown code too (see
        // `script_facts::refs`): it can observe any module's names.
        unknown_code: unknown.other.is_some()
            || iglint_python_semantic::dynamic::reads_frames(&ast.body).is_some(),
        imports: unknown.imports.into_iter().map(|(name, _)| name).collect(),
        system: unknown.system.into_iter().map(|(path, _)| path).collect(),
        library_names: super::access::library_names_used(model, &ast.body),
        // Dynamic only because of absolute star imports: a name the module
        // does not bind can only come from the modules they name.
        names_from_stars_only: dynamic
            && !rebinds.anything
            && rebinds.star_imports.len() == rebinds.star_modules.len(),
        star_imports: rebinds.star_modules,
        star_exports: star_exports(model, ast),
        variable_imports,
        findings: None,
        names: Default::default(),
    }
}

/// What `from module import *` binds: the names of `__all__` when the
/// module assigns it once in its own statement list, from literals (and
/// appends or extends it with literals at module level); every top-level
/// name not starting with `_` when the module has no `__all__`; any name
/// when `__all__` may hold names the code does not show (review round 29,
/// N1: `__all__[:] = [...]`, `__all__[1] = 'json'`, `del __all__[0]`,
/// `__all__.insert(...)`, `__all__ += [...]`, `x = __all__`, a use in a
/// function, several or conditional bindings, code that writes the
/// module's namespace dynamically).
pub(crate) fn star_exports(model: &SemanticModel<'_>, module: &Module) -> StarExports {
    if model
        .scopes()
        .any(|(_, s)| s.writes_globals || s.unqualified_exec.is_some())
    {
        return StarExports::Any;
    }
    let all = model.scope(ScopeId::MODULE).bindings_of("__all__");
    // The receivers of the module-level calls the model reads
    // (`__all__.append('x')`, `__all__.extend([...])`).
    let mut modelled = Vec::new();
    module_level_all_calls(&module.body, &mut modelled);
    let other_use = model
        .references()
        .any(|(_, r)| r.name == "__all__" && !r.is_implicit() && !modelled.contains(&r.range));
    if all.is_empty() && !other_use {
        return StarExports::Public;
    }
    let unconditional_all = match all {
        [only] => model
            .binding(*only)
            .stmt
            .is_some_and(|stmt| module.body.as_ptr_range().contains(&(stmt as *const Stmt))),
        _ => false,
    };
    if unconditional_all && !other_use && !model.dunder_all_is_dynamic() {
        StarExports::Listed(model.dunder_all().to_vec())
    } else {
        StarExports::Any
    }
}

/// The ranges of `__all__` in module-level `__all__.append(...)` and
/// `__all__.extend(...)` statements (also inside `if`, loops, `try` and
/// `with` at module level), which the semantic model reads.
fn module_level_all_calls(body: &[Stmt], out: &mut Vec<TextRange>) {
    for stmt in body {
        match stmt {
            Stmt::Expr(s) => {
                if let Expr::Call(c) = &*s.value
                    && let Expr::Attribute(a) = &*c.func
                    && matches!(a.attr.as_str(), "append" | "extend")
                    && let Expr::Name(n) = &*a.value
                    && &*n.id == "__all__"
                {
                    out.push(n.range);
                }
            }
            Stmt::If(s) => {
                module_level_all_calls(&s.body, out);
                module_level_all_calls(&s.orelse, out);
            }
            Stmt::For(s) => {
                module_level_all_calls(&s.body, out);
                module_level_all_calls(&s.orelse, out);
            }
            Stmt::While(s) => {
                module_level_all_calls(&s.body, out);
                module_level_all_calls(&s.orelse, out);
            }
            Stmt::With(s) => module_level_all_calls(&s.body, out),
            Stmt::TryExcept(s) => {
                module_level_all_calls(&s.body, out);
                for h in &s.handlers {
                    module_level_all_calls(&h.body, out);
                }
                module_level_all_calls(&s.orelse, out);
            }
            Stmt::TryFinally(s) => {
                module_level_all_calls(&s.body, out);
                module_level_all_calls(&s.finalbody, out);
            }
            _ => {}
        }
    }
}

/// Whether `from module import *` binds `name` (Python 2.7), given the
/// module's [`star_exports`].
pub(crate) fn exported_to_star_imports(exports: &StarExports, name: &str) -> bool {
    exports.exports(name)
}

/// An expression made of literals only (numbers, strings, `None`, `True`,
/// `False`, containers and operators of them): never a function. The
/// three names count only where they read the builtin constants
/// (`True = lambda: 7` rebinds one).
fn is_literal(model: &SemanticModel<'_>, e: &Expr) -> bool {
    let literal = |e: &Expr| is_literal(model, e);
    match e {
        Expr::Num(_) | Expr::Str(_) => true,
        Expr::Name(_) => crate::rules::script::is_builtin_constant(model, e),
        Expr::List(l) => l.elts.iter().all(literal),
        Expr::Tuple(t) => t.elts.iter().all(literal),
        Expr::Set(s) => s.elts.iter().all(literal),
        Expr::Dict(d) => d.keys.iter().chain(&d.values).all(literal),
        Expr::UnaryOp(u) => literal(&u.operand),
        Expr::BinOp(b) => literal(&b.left) && literal(&b.right),
        _ => false,
    }
}

/// Ways the module may rebind its own top-level names at run time that
/// its bindings do not show.
#[derive(Default)]
struct Rebinds<'a> {
    /// Any name: code from strings, writes to the module namespace.
    anything: bool,
    /// Module-level `from x import *`, and whether it is inside a loop.
    star_imports: Vec<(TextRange, bool)>,
    /// The modules those statements name (absolute imports only).
    star_modules: Vec<String>,
    /// Module-level names whose code or defaults may be replaced
    /// (`f.func_code = g.func_code`, `setattr(f, name, ...)`).
    attributes_set: FxHashSet<&'a str>,
}

impl<'a> Rebinds<'a> {
    fn collect(model: &SemanticModel<'a>, ast: &'a Module) -> Rebinds<'a> {
        let mut v = RebindVisitor {
            model,
            out: Rebinds {
                // `exec code` binds in the namespace it runs in; in a
                // function the code may declare a name `global` (verified
                // on Jython), so it is not limited to the module scope.
                anything: model
                    .scopes()
                    .any(|(_, s)| s.writes_globals || s.unqualified_exec.is_some()),
                ..Rebinds::default()
            },
            function_depth: 0,
            loop_depth: 0,
        };
        iglint_python::visitor::walk_body(&mut v, &ast.body);
        v.out
    }
}

/// Function attributes that change what a call does or accepts.
pub(crate) const CODE_ATTRIBUTES: &[&str] =
    &["func_code", "__code__", "func_defaults", "__defaults__"];

struct RebindVisitor<'m, 'a> {
    model: &'m SemanticModel<'a>,
    out: Rebinds<'a>,
    /// Inside a `def`, `class` or `lambda` (not module-level code).
    function_depth: usize,
    /// Inside a module-level `for` or `while`.
    loop_depth: usize,
}

impl<'a> RebindVisitor<'_, 'a> {
    /// The module-level name `e` refers to, when it is a name that only
    /// module-level bindings reach.
    fn module_name(&self, e: &'a Expr) -> Option<&'a str> {
        let Expr::Name(n) = e else { return None };
        let r = self.model.reference_at(n.range)?;
        (!r.bindings.is_empty()
            && r.bindings
                .iter()
                .all(|b| self.model.binding(*b).scope == ScopeId::MODULE))
        .then_some(r.name)
    }

    fn attribute_target(&mut self, e: &'a Expr) {
        if let Expr::Attribute(a) = e
            && matches!(a.ctx, ExprContext::Store | ExprContext::Del)
            && CODE_ATTRIBUTES.contains(&a.attr.as_str())
            && let Some(name) = self.module_name(&a.value)
        {
            self.out.attributes_set.insert(name);
        }
    }
}

impl<'a> Visitor<'a> for RebindVisitor<'_, 'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {
                self.function_depth += 1;
                walk_stmt(self, stmt);
                self.function_depth -= 1;
            }
            Stmt::For(_) | Stmt::While(_) if self.function_depth == 0 => {
                self.loop_depth += 1;
                walk_stmt(self, stmt);
                self.loop_depth -= 1;
            }
            Stmt::ImportFrom(i)
                if self.function_depth == 0 && i.names.iter().any(|a| a.name.as_str() == "*") =>
            {
                self.out.star_imports.push((i.range, self.loop_depth > 0));
                if let Some(m) = &i.module
                    && i.level == 0
                {
                    self.out.star_modules.push(m.as_str().to_owned());
                }
            }
            _ => walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Lambda(_) => {
                self.function_depth += 1;
                walk_expr(self, expr);
                self.function_depth -= 1;
                return;
            }
            Expr::Attribute(_) => self.attribute_target(expr),
            Expr::Call(c) => {
                let builtin =
                    |name| crate::rules::script::is_builtin_reference(self.model, &c.func, name);
                // Code run from a string can rebind any global
                // (`eval("globals().update(...)")`, `execfile(path)` runs
                // in the caller's globals).
                if builtin("eval") || builtin("execfile") {
                    self.out.anything = true;
                }
                if (builtin("setattr") || builtin("delattr"))
                    && let Some(name) = c.args.first().and_then(|a| self.module_name(a))
                    && c.args.get(1).is_none_or(|attr| match attr {
                        Expr::Str(s) => s.s.as_str().is_none_or(|a| CODE_ATTRIBUTES.contains(&a)),
                        _ => true,
                    })
                {
                    self.out.attributes_set.insert(name);
                }
            }
            _ => {}
        }
        walk_expr(self, expr);
    }
}

/// The signature of a `def`.
pub fn signature(args: &Arguments) -> Signature {
    let first_default = args.args.len().saturating_sub(args.defaults.len());
    let mut params: Vec<Parameter> = args
        .args
        .iter()
        .enumerate()
        .map(|(i, a)| Parameter {
            name: match a {
                Expr::Name(n) => n.id.to_string(),
                _ => String::new(),
            },
            kind: ParameterKind::Positional,
            has_default: i >= first_default,
        })
        .collect();
    if let Some(v) = &args.vararg {
        params.push(Parameter {
            name: v.as_str().to_owned(),
            kind: ParameterKind::VarArgs,
            has_default: true,
        });
    }
    if let Some(k) = &args.kwarg {
        params.push(Parameter {
            name: k.as_str().to_owned(),
            kind: ParameterKind::KwArgs,
            has_default: true,
        });
    }
    Signature {
        params,
        uses: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iglint_python_semantic::SemanticOptions;

    fn symbols(src: &str) -> ModuleSymbols {
        let parsed = iglint_python::parse_module(src);
        assert!(parsed.errors.is_empty());
        let model = SemanticModel::build(&parsed.module, &SemanticOptions::default());
        module_symbols(&model, &parsed.module)
    }

    fn kinds(s: &ModuleSymbols) -> Vec<(String, String)> {
        s.symbols
            .iter()
            .map(|s| {
                let k = match &s.kind {
                    SymbolKind::Function(sig) => format!(
                        "def({})",
                        sig.params
                            .iter()
                            .map(|p| match p.kind {
                                ParameterKind::Positional if p.has_default =>
                                    format!("{}=", p.name),
                                ParameterKind::Positional => p.name.clone(),
                                ParameterKind::VarArgs => format!("*{}", p.name),
                                ParameterKind::KwArgs => format!("**{}", p.name),
                            })
                            .collect::<Vec<_>>()
                            .join(",")
                    ),
                    SymbolKind::Class => "class".into(),
                    SymbolKind::Variable => "var".into(),
                    SymbolKind::Import { target } => {
                        format!("import {}", target.as_deref().unwrap_or("?"))
                    }
                };
                (s.name.clone(), k)
            })
            .collect()
    }

    #[test]
    fn functions_classes_variables_imports() {
        let s = symbols(
            "import json\nfrom core.util import fmt as f2\nLOG = 1\ndef go(a, b=1, *rest, **kw):\n    global cache\n    cache = {}\nclass K(object):\n    pass\n@deco\ndef wrapped(x):\n    pass\nif c:\n    def two(a): pass\nelse:\n    def two(a, b): pass\nfor i in range(3): pass\n",
        );
        assert!(!s.dynamic);
        let mut k = kinds(&s);
        k.sort();
        assert_eq!(
            k,
            vec![
                ("K".into(), "class".into()),
                ("LOG".into(), "var".into()),
                ("cache".into(), "var".into()),
                ("f2".into(), "import core.util.fmt".into()),
                ("go".into(), "def(a,b=,*rest,**kw)".into()),
                ("i".into(), "var".into()),
                ("json".into(), "import json".into()),
                ("two".into(), "var".into()),
                ("wrapped".into(), "var".into()),
            ]
        );
    }

    /// Reviewer round 15: `True`, `False` and `None` are names a module
    /// can rebind, so a name bound to one is a constant only when it is
    /// the builtin.
    #[test]
    fn constants_read_the_builtin_names() {
        let s = symbols("A = (1, None, True)\nB = -False\n");
        assert_eq!(s.constants, ["A", "B"]);
        let s = symbols("True = lambda: 7\nA = True\nB = 1\n");
        assert_eq!(s.constants, ["B"]);
        let s = symbols("import helpers as None\nA = [None]\n");
        assert!(s.constants.is_empty(), "{:?}", s.constants);
    }

    #[test]
    fn dynamic_namespaces() {
        assert!(symbols("from x import *\n").dynamic);
        assert!(symbols("def f():\n    globals()['a'] = 1\n").dynamic);
        assert!(symbols("exec 'a = 1'\n").dynamic);
        assert!(!symbols("exec 'a = 1' in ns\n").dynamic);
        assert!(symbols("def f():\n    exec 'global a\\na = 1'\n").dynamic);
        assert!(symbols("execfile('x.py')\n").dynamic);
        assert!(symbols("def f(s):\n    return eval(s)\n").dynamic);
    }

    fn kind_of(src: &str, name: &str) -> String {
        kinds(&symbols(src))
            .into_iter()
            .find(|(n, _)| n == name)
            .map(|(_, k)| k)
            .unwrap()
    }

    /// Verified on Jython: each of these makes `handle` another function
    /// after the module ran, so its `def` says nothing about calls.
    #[test]
    fn functions_the_module_may_rebind_are_variables() {
        let def = "def handle(p):\n    pass\n";
        for rebind in [
            "exec 'def handle(p): print p.custom.used'\n",
            "exec code\n",
            "def setup():\n    exec 'global handle\\ndef handle(p): pass'\n",
            "globals()['handle'] = other\n",
            "globals().update(handle=other)\n",
            "def setup():\n    globals()[name] = other\n",
            "import sys\nsetattr(sys.modules[__name__], 'handle', other)\n",
            "import sys\nsys.modules[__name__].handle = other\n",
            "execfile(path)\n",
            "eval(code)\n",
            "from other import *\n",
            "handle.func_code = other.func_code\n",
            "setattr(handle, '__defaults__', (1,))\n",
            "setattr(handle, name, None)\n",
            "handle = wrap(handle)\n",
            "def handle(p, q):\n    pass\n",
            "from other import handle\n",
            "def setup():\n    global handle\n    handle = other\n",
        ] {
            let src = format!("{def}{rebind}");
            assert_eq!(kind_of(&src, "handle"), "var", "{src}");
        }
        // The same function rebinding nothing.
        for innocent in [
            "",
            "exec 'x = 1' in ns\n",
            "handle.cache = {}\n",
            "setattr(handle, 'cache', {})\n",
            "def eval(s):\n    return s\neval(code)\n",
        ] {
            let src = format!("{def}{innocent}");
            assert_eq!(kind_of(&src, "handle"), "def(p)", "{src}");
        }
        // A star import before the definition cannot overwrite it, one in
        // a loop or after it can.
        assert_eq!(
            kind_of(&format!("from other import *\n{def}"), "handle"),
            "def(p)"
        );
        assert_eq!(
            kind_of(
                &format!("for m in mods:\n    from other import *\n{def}"),
                "handle"
            ),
            "var"
        );
        assert_eq!(
            kind_of(
                &format!(
                    "if c:\n    {}\nfrom other import *\n",
                    def.trim_end().replace('\n', "\n    ")
                ),
                "handle"
            ),
            "var"
        );
    }
}
