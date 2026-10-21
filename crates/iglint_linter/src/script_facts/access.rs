//! Property accesses of view scripts, found on the Jython syntax tree.
//!
//! The analysis follows values that are view objects through attribute
//! chains starting at the script's `self`:
//!
//! | expression | value |
//! |---|---|
//! | `self` | the owning component (the view in view-level scripts) |
//! | `<component>.view` | the view |
//! | `<component>.parent`, `.getParent()` | the parent component |
//! | `<component>.getSibling('A')`, `.getChild('A/B')` | that component (any component when the name is not a literal) |
//! | `<component>.children`, `.getChildren()` | any component |
//! | `<view>.rootContainer`, `<view>.getChild('root/A')` | that component |
//! | `<object>.custom`, `<view>.params` | a property container |
//!
//! An attribute of a container (`self.view.custom.total`) or a subscript
//! with a string literal (`self.custom['total']`) accesses that property;
//! the attribute's context says whether it is read (`Load`), written
//! (`Store`, `Del`) or both (augmented assignment).
//!
//! Everything else that touches these values is treated conservatively:
//!
//! * a container used as a value (`d = self.view.custom`, `len(self.custom)`,
//!   `self.custom[key]`, `self.custom.keys()`) accesses every property of
//!   that container (`name: None`);
//! * a view or component used as a value (passed to a function, returned,
//!   stored in a variable that is assigned more than once, `exec`,
//!   `locals()`) makes everything reachable, so every property of every
//!   component and of the view counts as accessed.
//!
//! A local name assigned exactly once from one of these values
//! (`view = self.view`, `table = self.getSibling('Table')`) is followed as
//! an alias.

use iglint_project::view::{PropCategory, View};
use iglint_project::{ComponentId, Owner};
use iglint_python::ast::{Expr, ExprContext, Slice, Stmt};
use iglint_python::visitor::{Visitor, walk_stmt};
use iglint_text::TextRange;
use rustc_hash::FxHashMap;

use super::{AccessTarget, PropertyAccess, ScriptFacts};
use crate::rule::ScriptUnit;
use iglint_project::summary::References;

/// Script facts from the syntax tree: property accesses only.
pub struct AstScriptFacts;

impl ScriptFacts for AstScriptFacts {
    fn complete(&self) -> bool {
        true
    }

    fn provides_references(&self) -> bool {
        false
    }

    fn references(&self, _unit: &ScriptUnit<'_, '_>, _refs: &mut References) {}

    fn property_accesses(
        &self,
        unit: &ScriptUnit<'_, '_>,
        view: &View<'_>,
        out: &mut Vec<PropertyAccess>,
    ) {
        let this = match unit.script.owner {
            Owner::View => Val::Object(AccessTarget::View),
            Owner::Component(c) => Val::Object(AccessTarget::Component(c)),
            _ => return,
        };
        let mut found = Vec::new();
        if !unit.parsed.is_valid() {
            // A script that does not parse may access anything.
            let anchor = unit.script.anchor();
            let mut a = Analysis {
                view,
                this,
                aliases: FxHashMap::default(),
                out,
            };
            a.everything(anchor);
            return;
        }
        let mut a = Analysis {
            view,
            this,
            aliases: FxHashMap::default(),
            out: &mut found,
        };
        a.collect_aliases(&unit.parsed.module.body);
        a.visit_module(&unit.parsed.module);
        for mut access in found {
            access.range = unit.script.file_range(access.range);
            out.push(access);
        }
    }
}

/// A value the analysis follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Val {
    Object(AccessTarget),
    Container(AccessTarget, PropCategory),
}

/// How an expression's value is used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    /// The value goes somewhere unknown.
    Value,
    /// The value is the base of an attribute, subscript or call that the
    /// caller interprets.
    Base,
}

struct Analysis<'v, 'a, 'o> {
    view: &'v View<'a>,
    this: Val,
    aliases: FxHashMap<String, Option<Val>>,
    out: &'o mut Vec<PropertyAccess>,
}

const OPAQUE_CALLS: &[&str] = &["locals", "globals", "vars", "eval", "execfile", "dir"];

impl<'v, 'a> Analysis<'v, 'a, '_> {
    // ---- aliases ----

    /// Names assigned exactly once, from a followed value.
    fn collect_aliases(&mut self, body: &[Stmt]) {
        #[derive(Default)]
        struct Stores<'e> {
            count: FxHashMap<String, (usize, Option<&'e Expr>)>,
        }
        impl<'e> Visitor<'e> for Stores<'e> {
            fn visit_stmt(&mut self, stmt: &'e Stmt) {
                if let Stmt::Assign(s) = stmt
                    && let [Expr::Name(n)] = s.targets.as_slice()
                {
                    let e = self.count.entry(n.id.to_string()).or_default();
                    e.0 += 1;
                    e.1 = Some(&s.value);
                    self.visit_expr(&s.value);
                    return;
                }
                walk_stmt(self, stmt);
            }
            fn visit_expr(&mut self, expr: &'e Expr) {
                if let Expr::Name(n) = expr
                    && matches!(
                        n.ctx,
                        ExprContext::Store | ExprContext::Param | ExprContext::AugStore
                    )
                {
                    // Other kinds of stores: never an alias.
                    self.count.entry(n.id.to_string()).or_default().0 += 2;
                }
                iglint_python::visitor::walk_expr(self, expr);
            }
        }
        let mut stores = Stores::default();
        for stmt in body {
            stores.visit_stmt(stmt);
        }
        // Resolve in a few rounds so aliases of aliases work.
        for _ in 0..3 {
            for (name, (count, value)) in &stores.count {
                if name == "self" {
                    continue;
                }
                if *count == 1
                    && let Some(value) = value
                {
                    let v = self.peek(value);
                    self.aliases.insert(name.clone(), v);
                } else {
                    self.aliases.insert(name.clone(), None);
                }
            }
        }
    }

    /// The value of `e` without recording anything.
    fn peek(&self, e: &Expr) -> Option<Val> {
        match e {
            Expr::Name(n) if n.ctx == ExprContext::Load => self.name_value(&n.id),
            Expr::Attribute(a) => {
                let base = self.peek(&a.value)?;
                self.attribute(base, a.attr.as_str())
            }
            Expr::Call(c) => {
                let Expr::Attribute(a) = &*c.func else {
                    return None;
                };
                let base = self.peek(&a.value)?;
                self.method(base, a.attr.as_str(), &c.args)
            }
            Expr::Subscript(s) => match self.peek(&s.value)? {
                Val::Object(AccessTarget::AnyComponent) => {
                    Some(Val::Object(AccessTarget::AnyComponent))
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn name_value(&self, name: &str) -> Option<Val> {
        if name == "self" {
            return Some(self.this);
        }
        self.aliases.get(name).copied().flatten()
    }

    // ---- navigation ----

    fn parent(&self, t: AccessTarget) -> AccessTarget {
        match t {
            AccessTarget::Component(c) => match self.view.component(c).parent {
                Some(p) => AccessTarget::Component(p),
                None => AccessTarget::AnyComponent,
            },
            _ => AccessTarget::AnyComponent,
        }
    }

    fn child_path(&self, from: Option<ComponentId>, path: &str) -> Option<AccessTarget> {
        let mut segments = path.split('/').filter(|s| !s.is_empty());
        let mut at = match from {
            Some(c) => c,
            None => {
                // From the view: the first segment names the root.
                let root = self.view.root_component()?;
                if segments.next()? != root.name {
                    return None;
                }
                root.id
            }
        };
        for segment in segments {
            at = self.view.children(at).find(|c| c.name == segment)?.id;
        }
        Some(AccessTarget::Component(at))
    }

    fn attribute(&self, base: Val, attr: &str) -> Option<Val> {
        match base {
            Val::Object(t) => match (t, attr) {
                (_, "custom") => Some(Val::Container(t, PropCategory::Custom)),
                (AccessTarget::View, "params") => Some(Val::Container(t, PropCategory::Params)),
                (AccessTarget::View, "rootContainer") => self
                    .view
                    .root_component()
                    .map(|r| Val::Object(AccessTarget::Component(r.id))),
                (AccessTarget::Component(_) | AccessTarget::AnyComponent, "view") => {
                    Some(Val::Object(AccessTarget::View))
                }
                (AccessTarget::Component(_) | AccessTarget::AnyComponent, "parent") => {
                    Some(Val::Object(self.parent(t)))
                }
                (AccessTarget::Component(_) | AccessTarget::AnyComponent, "children") => {
                    Some(Val::Object(AccessTarget::AnyComponent))
                }
                _ => None,
            },
            Val::Container(..) => None,
        }
    }

    fn method(&self, base: Val, name: &str, args: &[Expr]) -> Option<Val> {
        let Val::Object(t) = base else {
            return None;
        };
        let literal = match args.first() {
            Some(Expr::Str(s)) => s.s.as_str().map(str::to_owned),
            _ => None,
        };
        match (t, name) {
            (AccessTarget::Component(c), "getSibling") => {
                let Some(name) = literal else {
                    return Some(Val::Object(AccessTarget::AnyComponent));
                };
                let parent = self.view.component(c).parent?;
                let sibling = self
                    .view
                    .children(parent)
                    .find(|s| s.name == name && s.id != c)?;
                Some(Val::Object(AccessTarget::Component(sibling.id)))
            }
            (AccessTarget::Component(c), "getChild") => match literal {
                Some(path) => self.child_path(Some(c), &path).map(Val::Object),
                None => Some(Val::Object(AccessTarget::AnyComponent)),
            },
            (AccessTarget::View, "getChild") => match literal {
                Some(path) => self.child_path(None, &path).map(Val::Object),
                None => Some(Val::Object(AccessTarget::AnyComponent)),
            },
            (AccessTarget::AnyComponent, "getSibling" | "getChild") => {
                Some(Val::Object(AccessTarget::AnyComponent))
            }
            (AccessTarget::Component(_) | AccessTarget::AnyComponent, "getParent") => {
                Some(Val::Object(self.parent(t)))
            }
            (AccessTarget::Component(_) | AccessTarget::AnyComponent, "getChildren") => {
                Some(Val::Object(AccessTarget::AnyComponent))
            }
            _ => None,
        }
    }

    // ---- recording ----

    fn record(&mut self, t: AccessTarget, category: PropCategory, name: Option<String>, ctx: ExprContext, range: TextRange) {
        let (read, write) = match ctx {
            ExprContext::Load => (true, false),
            ExprContext::AugLoad | ExprContext::AugStore => (true, true),
            ExprContext::Store | ExprContext::Del | ExprContext::Param => (false, true),
        };
        self.out.push(PropertyAccess {
            target: t,
            category,
            name,
            read,
            write,
            range,
        });
    }

    /// A followed value escapes: everything reachable may be accessed.
    fn escape(&mut self, val: Val, range: TextRange) {
        match val {
            Val::Container(t, c) => self.record(t, c, None, ExprContext::Load, range),
            Val::Object(_) => self.everything(range),
        }
    }

    fn everything(&mut self, range: TextRange) {
        for (t, c) in [
            (AccessTarget::View, PropCategory::Custom),
            (AccessTarget::View, PropCategory::Params),
            (AccessTarget::AnyComponent, PropCategory::Custom),
        ] {
            self.out.push(PropertyAccess {
                target: t,
                category: c,
                name: None,
                read: true,
                write: true,
                range,
            });
        }
    }

    // ---- walking ----

    /// Walks an expression; returns its followed value when `role` is
    /// [`Role::Base`]. In [`Role::Value`] a followed value escapes.
    fn walk(&mut self, e: &Expr, role: Role, aug: bool) -> Option<Val> {
        let result = match e {
            Expr::Name(n) => match n.ctx {
                ExprContext::Load | ExprContext::AugLoad => self.name_value(&n.id),
                _ => None,
            },
            Expr::Attribute(a) => {
                let base = self.walk(&a.value, Role::Base, false);
                match base {
                    Some(Val::Container(t, c)) => {
                        let ctx = if aug { ExprContext::AugStore } else { a.ctx };
                        self.record(t, c, Some(a.attr.as_str().to_owned()), ctx, a.attr.range);
                        None
                    }
                    Some(b @ Val::Object(_)) => {
                        if !matches!(a.ctx, ExprContext::Load) && a.attr.as_str() == "custom" {
                            // `self.custom = {...}` replaces every property.
                            if let Some(Val::Container(t, c)) = self.attribute(b, "custom") {
                                self.record(t, c, None, ExprContext::Store, a.attr.range);
                            }
                            None
                        } else {
                            self.attribute(b, a.attr.as_str())
                        }
                    }
                    None => None,
                }
            }
            Expr::Subscript(s) => {
                let base = self.walk(&s.value, Role::Base, false);
                let key = match &*s.slice {
                    Slice::Index(i) => match &*i.value {
                        Expr::Str(st) => st.s.as_str().map(str::to_owned),
                        _ => None,
                    },
                    _ => None,
                };
                self.walk_slice(&s.slice);
                match base {
                    Some(Val::Container(t, c)) => {
                        let ctx = if aug { ExprContext::AugStore } else { s.ctx };
                        self.record(t, c, key, ctx, s.range);
                        None
                    }
                    Some(Val::Object(AccessTarget::AnyComponent)) => {
                        Some(Val::Object(AccessTarget::AnyComponent))
                    }
                    Some(other) => {
                        self.escape(other, s.range);
                        None
                    }
                    None => None,
                }
            }
            Expr::Call(c) => {
                let mut value = None;
                match &*c.func {
                    Expr::Attribute(a) => {
                        let base = self.walk(&a.value, Role::Base, false);
                        match base {
                            Some(b @ Val::Object(_)) => {
                                value = self.method(b, a.attr.as_str(), &c.args);
                            }
                            Some(Val::Container(t, cat)) => {
                                // `self.custom.keys()`, `.get(...)`, ...
                                self.record(t, cat, None, ExprContext::Load, a.range);
                            }
                            None => {}
                        }
                    }
                    Expr::Name(n) if OPAQUE_CALLS.contains(&&*n.id) => {
                        self.everything(c.range);
                    }
                    other => {
                        self.walk(other, Role::Value, false);
                    }
                }
                for arg in &c.args {
                    self.walk(arg, Role::Value, false);
                }
                for k in &c.keywords {
                    self.walk(&k.value, Role::Value, false);
                }
                if let Some(s) = &c.starargs {
                    self.walk(s, Role::Value, false);
                }
                if let Some(k) = &c.kwargs {
                    self.walk(k, Role::Value, false);
                }
                value
            }
            other => {
                // Any other expression: walk its children as values.
                let mut children = Children::default();
                iglint_python::visitor::walk_expr(&mut children, other);
                for child in children.0 {
                    self.walk(child, Role::Value, false);
                }
                None
            }
        };
        match (role, result) {
            (Role::Value, Some(v)) => {
                self.escape(v, e_range(e));
                None
            }
            _ => result,
        }
    }

    fn walk_slice(&mut self, slice: &Slice) {
        let mut children = Children::default();
        children.visit_slice(slice);
        for child in children.0 {
            self.walk(child, Role::Value, false);
        }
    }
}

fn e_range(e: &Expr) -> TextRange {
    use iglint_text::Ranged;
    e.range()
}

/// Collects the direct child expressions of a node.
#[derive(Default)]
struct Children<'e>(Vec<&'e Expr>);

impl<'e> Visitor<'e> for Children<'e> {
    fn visit_expr(&mut self, expr: &'e Expr) {
        self.0.push(expr);
    }
}

impl<'e> Visitor<'e> for Analysis<'_, '_, '_> {
    fn visit_stmt(&mut self, stmt: &'e Stmt) {
        match stmt {
            Stmt::Assign(s) => {
                if let [Expr::Name(n)] = s.targets.as_slice()
                    && self.aliases.get(&*n.id).copied().flatten().is_some()
                {
                    // An alias definition: the value is followed, not lost.
                    self.walk(&s.value, Role::Base, false);
                    return;
                }
                for t in &s.targets {
                    self.walk(t, Role::Value, false);
                }
                self.walk(&s.value, Role::Value, false);
            }
            Stmt::AugAssign(s) => {
                self.walk(&s.target, Role::Value, true);
                self.walk(&s.value, Role::Value, false);
            }
            Stmt::Exec(s) => {
                self.everything(s.range);
                walk_stmt(self, stmt);
            }
            _ => walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &'e Expr) {
        self.walk(expr, Role::Value, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iglint_project::view::build_view;

    const VIEW: &str = r#"{"custom": {"a": 1}, "params": {"p": 1},
      "root": {"meta": {"name": "root"}, "type": "ia.container.flex", "children": [
        {"meta": {"name": "A"}, "type": "ia.display.label", "custom": {"x": 1}},
        {"meta": {"name": "B"}, "type": "ia.input.button", "custom": {"y": 1},
         "events": {"component": {"onActionPerformed": {"type": "script", "scope": "G", "config": {"script": ""}}}}}
      ]}}"#;

    /// Accesses of `body` run as the button's action script.
    fn accesses(body: &str) -> Vec<String> {
        let mut json: serde_json::Value = serde_json::from_str(VIEW).unwrap();
        json["root"]["children"][1]["events"]["component"]["onActionPerformed"]["config"]
            ["script"] = serde_json::Value::String(body.to_owned());
        let text = serde_json::to_string(&json).unwrap();
        let root = iglint_json::parse(&text).unwrap();
        let view = build_view(&root);
        let script = &view.scripts[0];
        let parsed = crate::script_facts::parse_script(script);
        let unit = ScriptUnit {
            file: iglint_project::FileId(0),
            script,
            parsed: &parsed,
            index: 0,
        };
        let mut out = Vec::new();
        AstScriptFacts.property_accesses(&unit, &view, &mut out);
        out.iter()
            .map(|a| {
                let t = match a.target {
                    AccessTarget::View => "view".to_owned(),
                    AccessTarget::Component(c) => view.component(c).name.to_owned(),
                    AccessTarget::AnyComponent => "*".to_owned(),
                };
                format!(
                    "{t}.{}.{}{}{}",
                    a.category.name(),
                    a.name.as_deref().unwrap_or("*"),
                    if a.read { " r" } else { "" },
                    if a.write { " w" } else { "" },
                )
            })
            .collect()
    }

    #[test]
    fn direct_accesses() {
        assert_eq!(
            accesses("\tx = self.view.custom.a\n\tself.view.params.p = self.custom.y\n\tself.getSibling('A').custom['x'] += 1"),
            vec!["view.custom.a r", "view.params.p w", "B.custom.y r", "A.custom.x r w"]
        );
    }

    #[test]
    fn navigation() {
        assert_eq!(
            accesses("\tself.parent.getChild('A').custom.x\n\tself.view.getChild('root/A').custom.q\n\tself.view.rootContainer.custom.r\n\tself.getSibling('Missing').custom.z"),
            vec!["A.custom.x r", "A.custom.q r", "root.custom.r r"]
        );
        // Iterating over children loses track of the components.
        assert_eq!(
            accesses("\tfor c in self.parent.children:\n\t\tc.custom.x = 1\n\tself.getSibling(name).custom.w"),
            vec!["view.custom.* r w", "view.params.* r w", "*.custom.* r w", "*.custom.w r"]
        );
    }

    #[test]
    fn aliases() {
        assert_eq!(
            accesses("\tv = self.view\n\tc = v.custom\n\tc.a = v.params.p"),
            vec!["view.custom.a w", "view.params.p r"]
        );
    }

    #[test]
    fn dynamic_uses_are_conservative() {
        assert_eq!(accesses("\tk = 'a'\n\tself.view.custom[k]"), vec!["view.custom.* r"]);
        assert_eq!(accesses("\tdata = dict(self.custom)"), vec!["B.custom.* r"]);
        assert_eq!(accesses("\tfor k in self.view.custom.keys(): pass"), vec!["view.custom.* r"]);
        let escaped = accesses("\tproject.util.handle(self)");
        assert!(escaped.contains(&"*.custom.* r w".to_owned()), "{escaped:?}");
        assert!(escaped.contains(&"view.params.* r w".to_owned()));
        let reassigned = accesses("\tv = self.view\n\tv = other\n\tv.custom.a");
        assert!(reassigned.contains(&"view.custom.* r w".to_owned()), "{reassigned:?}");
        assert!(accesses("\tx = locals()").len() == 3);
    }

    #[test]
    fn unrelated_code_has_no_accesses() {
        assert!(accesses("\tx = self.props.text\n\ty = 'self.custom.a'\n\t# self.view.custom.a\n\tsystem.perspective.print(self.meta.name)").is_empty());
    }
}
