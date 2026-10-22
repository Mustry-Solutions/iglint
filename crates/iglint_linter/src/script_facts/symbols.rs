//! The top-level names of a library module, for the project index.
//!
//! A module's namespace after import holds every name its module-level code
//! binds, and every name a function binds through `global`. A name bound
//! exactly once by a plain `def` gets the function's signature; a name bound
//! several times (conditional definitions, redefinitions, `global`
//! assignments) or by a decorated `def` is a [`SymbolKind::Variable`], whose
//! calls are never checked. A module whose namespace cannot be known
//! (`from x import *`, unqualified `exec`, writes through `globals()`) is
//! marked dynamic: lookups of names it does not define are then unknown
//! rather than missing.

use iglint_project::library::{
    ModuleSymbols, Parameter, ParameterKind, Signature, Symbol, SymbolKind,
};
use iglint_python::ast::{Arguments, Expr, Stmt};
use iglint_python_semantic::{BindingKind, ScopeId, SemanticModel};

/// The symbols of a module, from its semantic model. `None` when the module
/// does not parse (its content is unknown).
pub fn module_symbols(model: &SemanticModel<'_>) -> ModuleSymbols {
    let module = model.scope(ScopeId::MODULE);
    let dynamic = model.scopes().any(|(_, s)| s.writes_globals)
        || module.star_import.is_some()
        || module.unqualified_exec.is_some();
    let mut symbols = Vec::new();
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
        let kind = if real.len() > 1 {
            SymbolKind::Variable
        } else {
            match first.kind {
                BindingKind::FunctionDef => match first.stmt {
                    Some(Stmt::FunctionDef(f)) if f.decorator_list.is_empty() => {
                        let mut signature = signature(&f.args);
                        signature.uses = super::access::param_uses(f);
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
    ModuleSymbols { symbols, dynamic }
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
        module_symbols(&model)
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
    }
}
