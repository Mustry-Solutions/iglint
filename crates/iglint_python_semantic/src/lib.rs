//! Semantic analysis of Jython 2.7 code: scopes, bindings, references,
//! control flow and compile-time errors, with exact Python 2.7 semantics.
//!
//! ```
//! use iglint_python_semantic::{SemanticModel, SemanticOptions, Resolution};
//!
//! let parsed = iglint_python::parse_module("import time\ndef f(x):\n    time.sleep(x)\n    return y\n");
//! let model = SemanticModel::build(&parsed.module, &SemanticOptions::default());
//! let undefined: Vec<&str> = model
//!     .references()
//!     .filter(|(_, r)| r.resolution == Resolution::Unresolved)
//!     .map(|(_, r)| r.name)
//!     .collect();
//! assert_eq!(undefined, ["y"]);
//! ```
//!
//! # Python 2.7 scoping, as modelled
//!
//! * Scopes: the module, class bodies, functions, lambdas, generator
//!   expressions and set/dict comprehensions. **List comprehensions have no
//!   scope**: their variables are bound in the enclosing scope.
//! * Class bodies are not visible to the functions nested in them; class
//!   bodies themselves see enclosing functions' variables.
//! * Decorators, default values, base classes and the first iterable of a
//!   generator expression are evaluated in the enclosing scope.
//! * A name is local to a function if the function binds it anywhere and
//!   does not declare it `global`. There is no `nonlocal`.
//! * `from x import *` and `exec` without `in` make a scope dynamic: names
//!   the code does not bind may exist, so unresolved names there are
//!   [`Resolution::Dynamic`], not [`Resolution::Unresolved`].
//! * Names fall back to the module, then to the caller's injected names
//!   ([`SemanticOptions::injected`], Ignition's implicit globals), then to
//!   Jython's `__builtin__` module ([`iglint_stubs::jython_builtins`]).
//!
//! # Flow
//!
//! References to local names carry the bindings that may reach them
//! (reaching definitions over a conservative control-flow model with
//! loops, `try`/`except`/`else`/`finally`, `with`, `break`, `continue`,
//! `return`, `raise` and constant `while`/`if` tests). A reference is
//! [`Reference::unbound`] only if no binding reaches it on any path, so
//! "used before assignment" findings do not depend on guesses. The model
//! also lists unreachable statements and answers whether a function can
//! finish without `return` ([`SemanticModel::function_can_fall_through`]).
//!
//! # Cost
//!
//! Two linear passes over the tree; loops are re-walked until their state
//! is stable (in practice twice), `finally` clauses twice. Bindings and
//! references live in flat vectors indexed by ids.

mod compile;
mod flow;
mod model;
mod resolve;
mod symbols;

pub use compile::{CompileError, CompileErrorKind, compile_errors};
pub use model::*;

use iglint_python::ast::Module;
use rustc_hash::FxHashSet;

/// What the caller knows about the environment a module runs in.
#[derive(Clone, Debug, Default)]
pub struct SemanticOptions<'o> {
    /// Names that exist as globals without being bound by the code, in
    /// addition to Jython's builtins: for Ignition scripts `system`,
    /// `fpmi`, `sys`, the generated function's own name and the project's
    /// top-level library packages. `__builtins__` is always defined.
    pub injected: Vec<&'o str>,
}

impl<'a> SemanticModel<'a> {
    /// Analyses `module`.
    pub fn build(module: &'a Module, options: &SemanticOptions<'_>) -> SemanticModel<'a> {
        let injected: FxHashSet<&str> = options.injected.iter().copied().collect();
        let table = symbols::build(module);
        let out = flow::Flow::new(module, table, &injected).run();
        SemanticModel {
            module,
            scopes: out.scopes,
            bindings: out.bindings,
            references: out.references,
            scope_of_node: out.scope_of_node,
            implicit: out.implicit,
            unreachable: out.unreachable,
            reference_at: out.reference_at,
            dunder_all: out.dunder_all,
            dunder_all_dynamic: out.dunder_all_dynamic,
        }
    }
}

#[cfg(test)]
mod tests;
