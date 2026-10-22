//! Resolving expressions to qualified names through imports and aliases.

use iglint_python::ast::{Expr, ExprCall};

use crate::model::{BindingKind, SemanticModel};

/// How many aliases (`db = system.db`) a lookup follows.
const MAX_ALIAS_DEPTH: u32 = 8;

impl<'a> SemanticModel<'a> {
    /// The dotted name `expr` refers to, following imports and simple
    /// aliases: `system.db.runQuery` for `system.db.runQuery`, for `db.runQuery`
    /// after `db = system.db`, and `time.sleep` after `from time import
    /// sleep` for `sleep`. `None` when the name is unknown, rebound to
    /// something else on some path, or not a name or attribute chain.
    ///
    /// Builtins and injected names resolve to themselves (`len`, `system`);
    /// relative imports keep their leading dots.
    pub fn qualified_name(&self, expr: &Expr) -> Option<String> {
        self.qualified(expr, 0)
    }

    /// The qualified name of a call's callee (see
    /// [`SemanticModel::qualified_name`]).
    pub fn callee(&self, call: &ExprCall) -> Option<String> {
        self.qualified_name(&call.func)
    }

    /// Whether `call` calls the function `qualified` (e.g. `time.sleep`).
    pub fn is_call_to(&self, call: &ExprCall, qualified: &str) -> bool {
        self.callee(call).is_some_and(|q| q == qualified)
    }

    fn qualified(&self, expr: &Expr, depth: u32) -> Option<String> {
        match expr {
            Expr::Attribute(a) => {
                let mut q = self.qualified(&a.value, depth)?;
                q.push('.');
                q.push_str(&a.attr.id);
                Some(q)
            }
            Expr::Name(n) => {
                let reference = self.reference_at(n.range)?;
                let mut result: Option<String> = None;
                if reference.bindings.is_empty() || reference.maybe_unbound {
                    return None;
                }
                for id in &reference.bindings {
                    let binding = self.binding(*id);
                    let q = match binding.kind {
                        BindingKind::Import
                        | BindingKind::FromImport
                        | BindingKind::Builtin
                        | BindingKind::Injected => binding.qualified.clone()?,
                        BindingKind::Assignment if depth < MAX_ALIAS_DEPTH && !binding.unpacked => {
                            let value = binding.value?;
                            if !matches!(value, Expr::Name(_) | Expr::Attribute(_)) {
                                return None;
                            }
                            self.qualified(value, depth + 1)?
                        }
                        _ => return None,
                    };
                    match &result {
                        None => result = Some(q),
                        Some(r) if *r == q => {}
                        Some(_) => return None,
                    }
                }
                result
            }
            _ => None,
        }
    }
}
