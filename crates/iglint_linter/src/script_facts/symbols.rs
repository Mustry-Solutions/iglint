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
//! A module with a frame escape route
//! ([`iglint_python_semantic::dynamic::frame_escape`]) writes its
//! globals, so every name is a variable. Variables bound only to literals
//! are listed as constants: they never hold a function.

use iglint_project::library::{
    ModuleSymbols, Parameter, ParameterKind, Signature, Symbol, SymbolKind,
};
use iglint_python::ast::{Arguments, Expr, ExprContext, Module, Stmt};
use iglint_python::visitor::{Visitor, walk_expr, walk_stmt};
use iglint_python_semantic::{BindingKind, ScopeId, SemanticModel};
use iglint_text::TextRange;
use rustc_hash::FxHashSet;

/// The symbols of a module, from its semantic model and syntax tree.
pub fn module_symbols(model: &SemanticModel<'_>, ast: &Module) -> ModuleSymbols {
    let module = model.scope(ScopeId::MODULE);
    let rebinds = Rebinds::collect(model, ast);
    let dynamic = rebinds.anything || module.star_import.is_some();
    let mut symbols = Vec::new();
    let mut constants = Vec::new();
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
                b.kind == BindingKind::Assignment && !b.unpacked && b.value.is_some_and(is_literal)
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
        symbols.push(Symbol {
            name: (*name).to_owned(),
            kind,
            range: first.range,
        });
    }
    let unknown = iglint_python_semantic::vetted::unknown_code(&ast.body);
    ModuleSymbols {
        symbols,
        dynamic,
        constants,
        unknown_code: unknown.other.is_some(),
        imports: unknown.imports.into_iter().map(|(name, _)| name).collect(),
        library_names: super::access::library_names_used(model, &ast.body),
    }
}

/// An expression made of literals only (numbers, strings, `None`, `True`,
/// `False`, containers and operators of them): never a function.
fn is_literal(e: &Expr) -> bool {
    match e {
        Expr::Num(_) | Expr::Str(_) => true,
        Expr::Name(n) => matches!(&*n.id, "None" | "True" | "False"),
        Expr::List(l) => l.elts.iter().all(is_literal),
        Expr::Tuple(t) => t.elts.iter().all(is_literal),
        Expr::Set(s) => s.elts.iter().all(is_literal),
        Expr::Dict(d) => d.keys.iter().chain(&d.values).all(is_literal),
        Expr::UnaryOp(u) => is_literal(&u.operand),
        Expr::BinOp(b) => is_literal(&b.left) && is_literal(&b.right),
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
