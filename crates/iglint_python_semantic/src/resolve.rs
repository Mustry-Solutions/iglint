//! Resolving expressions to qualified names through imports and aliases.

use iglint_python::ast::{Expr, ExprCall};

use crate::model::{BindingKind, Resolution, SemanticModel};

/// How [`SemanticModel::qualified_name_with`] treats a name that is bound
/// nowhere the code can see.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnboundNames {
    /// No qualified name (what [`SemanticModel::qualified_name`] does).
    Reject,
    /// The name stands for itself when it is [`Resolution::Unresolved`]:
    /// for callers that inject global names later (a project's library
    /// packages are only known once every resource is indexed).
    AsGlobal,
    /// Like [`UnboundNames::AsGlobal`], also for [`Resolution::Dynamic`]
    /// names (`from x import *` in scope, unknown globals): for callers
    /// that over-approximate, such as usage counts.
    AsGlobalEvenIfDynamic,
}

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
        self.qualified(expr, 0, UnboundNames::Reject)
    }

    /// Like [`SemanticModel::qualified_name`], with a choice of what names
    /// bound nowhere stand for.
    pub fn qualified_name_with(&self, expr: &Expr, unbound: UnboundNames) -> Option<String> {
        self.qualified(expr, 0, unbound)
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

    fn qualified(&self, expr: &Expr, depth: u32, unbound: UnboundNames) -> Option<String> {
        match expr {
            Expr::Attribute(a) => {
                let mut q = self.qualified(&a.value, depth, unbound)?;
                q.push('.');
                q.push_str(&a.attr.id);
                Some(q)
            }
            Expr::Name(n) => {
                let reference = self.reference_at(n.range)?;
                let mut result: Option<String> = None;
                if reference.bindings.is_empty() {
                    let global = match reference.resolution {
                        Resolution::Unresolved => unbound != UnboundNames::Reject,
                        Resolution::Dynamic => unbound == UnboundNames::AsGlobalEvenIfDynamic,
                        _ => false,
                    };
                    return global.then(|| n.id.to_string());
                }
                if reference.maybe_unbound {
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
                            self.qualified(value, depth + 1, unbound)?
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
