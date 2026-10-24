//! The gateway context a WebDev handler receives.
//!
//! A WebDev Python resource is `def doGet(request, session):` (and the other
//! methods), with `request` a dictionary whose `context` item is the
//! gateway's `GatewayContext`. The context hands out every gateway manager:
//! tag managers that import tags with their scripts from a JSON string,
//! the project, resource collection and configuration managers, script
//! managers. Those are store types (`JAVA_STORE_TYPES`, measured by
//! `stubs-gen/gen_vetted.py`), so the context is not followed: any use of
//! it is unknown code. So is any use of `request` that may hand out the
//! context: everything but an item with a literal key other than
//! `context` (`request['params']`, `request.get('remainingPath')`).

use iglint_python::ast::{Expr, Slice, Stmt};
use iglint_python::visitor::{Visitor, walk_expr};
use iglint_text::TextRange;

/// The first use of the `request` parameter of the WebDev handler in
/// `body` that may reach the gateway context.
pub fn context_use(body: &[Stmt]) -> Option<TextRange> {
    let mut first: Option<TextRange> = None;
    for stmt in body {
        let Stmt::FunctionDef(f) = stmt else { continue };
        let Some(Expr::Name(param)) = f.args.args.first() else {
            continue;
        };
        let mut v = Uses {
            name: &param.id,
            found: None,
        };
        for s in &f.body {
            v.visit_stmt(s);
        }
        if let Some(r) = v.found
            && first.is_none_or(|f| r.start() < f.start())
        {
            first = Some(r);
        }
    }
    first
}

struct Uses<'n> {
    name: &'n str,
    found: Option<TextRange>,
}

impl Uses<'_> {
    fn is_request(&self, e: &Expr) -> bool {
        matches!(e, Expr::Name(n) if &*n.id == self.name)
    }
}

/// Whether `e` is a string literal other than `context`.
fn plain_key(e: &Expr) -> bool {
    matches!(e, Expr::Str(s) if s.s.to_latin1() != "context")
}

impl<'a> Visitor<'a> for Uses<'_> {
    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Subscript(s) if self.is_request(&s.value) => {
                if let Slice::Index(i) = &*s.slice
                    && plain_key(&i.value)
                {
                    return;
                }
            }
            Expr::Call(c) => {
                if let Expr::Attribute(a) = &*c.func
                    && self.is_request(&a.value)
                    && a.attr.as_str() == "get"
                    && c.args.first().is_some_and(plain_key)
                    && c.starargs.is_none()
                    && c.kwargs.is_none()
                {
                    for arg in c.args.iter().skip(1) {
                        self.visit_expr(arg);
                    }
                    return;
                }
            }
            Expr::Name(_) if self.is_request(expr) => {
                if self.found.is_none() {
                    self.found = Some(iglint_text::Ranged::range(expr));
                }
                return;
            }
            _ => {}
        }
        walk_expr(self, expr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(src: &str) -> bool {
        let parsed = iglint_python::parse_module(src);
        assert!(parsed.errors.is_empty(), "{src}");
        context_use(&parsed.module.body).is_some()
    }

    #[test]
    fn context_uses() {
        for src in [
            "def doGet(request, session):\n    c = request['context']\n",
            "def doGet(request, session):\n    request['context'].getTagManager()\n",
            "def doPost(request, session):\n    c = request.get('context')\n",
            "def doGet(request, session):\n    k = 'context'\n    c = request[k]\n",
            "def doGet(request, session):\n    lib.handle(request)\n",
            "def doGet(request, session):\n    for k, v in request.items():\n        pass\n",
            "def doGet(req, session):\n    c = req['context']\n",
        ] {
            assert!(found(src), "{src}");
        }
        for src in [
            "def doGet(request, session):\n    p = request['params']\n    return {'json': p}\n",
            "def doGet(request, session):\n    r = request.get('remainingPath', '')\n",
            "def doGet(request, session):\n    d = request['data']['x']\n",
        ] {
            assert!(!found(src), "{src}");
        }
    }
}
