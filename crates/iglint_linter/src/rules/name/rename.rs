//! Where a view refers to its components by name, for the component rename
//! fix of `NAME001`.
//!
//! References are found by *resolution*, not by name: `{../Label.props.text}`
//! is a reference to the `Label` that is a sibling of the expression's owner,
//! not to every component called `Label`. Sources:
//!
//! * property references in every expression of the view (bindings,
//!   transforms, struct leaves, query parameters, ...);
//! * property binding paths and indirect tag binding references;
//! * `getChild('...')` / `getSibling('...')` calls with literal arguments in
//!   scripts, on receivers that resolve statically (`self`, `self.parent`,
//!   `self.view.rootContainer`, earlier `getChild`/`getSibling` calls).
//!
//! A literal lookup whose receiver cannot be resolved, or whose literal
//! cannot be rewritten in place, blocks renaming components of that name.

use iglint_expression::{PropertyRef, PropertyTarget, Reference};
use iglint_project::view::{BindingConfig, View};
use iglint_project::{ComponentId, EmbeddedScript, Owner};
use iglint_python::ast::{Expr, ExprCall};
use iglint_python::visitor::{Visitor, walk_expr};
use iglint_text::{TextRange, TextSize};
use rustc_hash::FxHashSet;

/// A place that names a component.
#[derive(Clone, Debug)]
pub struct Site {
    pub component: ComponentId,
    /// File range of the name text (in the view's file).
    pub range: TextRange,
}

/// Every component reference of a view.
#[derive(Default, Debug)]
pub struct References {
    pub sites: Vec<Site>,
    /// Components whose `meta.name` is read (`{this.meta.name}`,
    /// `self.meta.name`): renaming changes a value the view uses.
    pub meta_name_read: FxHashSet<ComponentId>,
    /// `meta.name` read on something we cannot resolve.
    pub dynamic_meta_name: bool,
    /// Names used in lookups we cannot resolve or rewrite: components with
    /// these names are not renamed.
    pub blocked: FxHashSet<String>,
    /// Other string literals of the view's scripts: a component whose name
    /// appears among them may be looked up dynamically.
    pub literals: FxHashSet<String>,
    /// `getChild(x)` / `getSibling(x)` with a computed argument.
    pub dynamic_lookup: bool,
}

impl References {
    /// The references of a view. `parsed` are the parses of
    /// `view.scripts`, in order.
    pub fn collect(view: &View<'_>, parsed: &[iglint_python::Parsed]) -> References {
        let mut refs = References::default();
        for e in &view.expressions {
            for (_, r) in e.references() {
                if let Reference::Property(p) = r {
                    refs.property_ref(view, e.owner, &p, &|r| e.file_range(r));
                }
            }
        }
        for b in &view.bindings {
            match &b.typed {
                BindingConfig::Property(p) => {
                    if let (Some(path), Some(r)) = (p.path, &p.reference) {
                        refs.property_ref(view, b.owner, r, &|range| path.raw_range(range));
                    }
                }
                BindingConfig::Tag(t) => {
                    for ir in &t.references {
                        if let (Some(r), Some(s)) = (&ir.reference, ir.value.as_json_string()) {
                            refs.property_ref(view, b.owner, r, &|range| s.raw_range(range));
                        }
                    }
                }
                _ => {}
            }
        }
        for (script, parsed) in view.scripts.iter().zip(parsed) {
            let mut walker = ScriptWalker {
                view,
                script,
                refs: &mut refs,
            };
            walker.visit_module(&parsed.module);
        }
        refs
    }

    fn property_ref(
        &mut self,
        view: &View<'_>,
        owner: Owner,
        r: &PropertyRef,
        map: &dyn Fn(TextRange) -> TextRange,
    ) {
        let (start, segments) = match &r.target {
            PropertyTarget::Relative { up, components, .. } => {
                let Owner::Component(mut at) = owner else {
                    return;
                };
                for _ in 0..*up {
                    match view.component(at).parent {
                        Some(p) => at = p,
                        None => return,
                    }
                }
                (Some(at), components.as_slice())
            }
            PropertyTarget::Absolute { components } => (None, components.as_slice()),
            PropertyTarget::This => {
                if let Owner::Component(c) = owner {
                    self.meta_read(r, c);
                }
                return;
            }
            PropertyTarget::Parent => {
                if let Owner::Component(c) = owner
                    && let Some(p) = view.component(c).parent
                {
                    self.meta_read(r, p);
                }
                return;
            }
            _ => return,
        };
        let mut at = start;
        for seg in segments {
            let Some(name) = seg.name() else {
                return;
            };
            let next = match at {
                None => view
                    .root_component()
                    .filter(|root| root.name == name)
                    .map(|root| root.id),
                Some(c) => child(view, c, name),
            };
            let Some(next) = next else {
                return;
            };
            self.sites.push(Site {
                component: next,
                range: map(seg.range),
            });
            at = Some(next);
        }
        if let Some(c) = at {
            self.meta_read(r, c);
        }
    }

    fn meta_read(&mut self, r: &PropertyRef, c: ComponentId) {
        let category = r.category();
        let name = r.segments.get(1).and_then(|s| s.name());
        if category == Some("meta") && (name.is_none() || name == Some("name")) {
            self.meta_name_read.insert(c);
        }
    }
}

fn child(view: &View<'_>, parent: ComponentId, name: &str) -> Option<ComponentId> {
    view.children(parent).find(|c| c.name == name).map(|c| c.id)
}

/// What a script expression evaluates to, as far as component lookups go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Value {
    Component(ComponentId),
    View,
    Unknown,
}

/// A component named in a lookup literal: the component, and the offset
/// and length of its name in the literal.
type Named = (ComponentId, usize, usize);

struct ScriptWalker<'r, 'v, 's> {
    view: &'v View<'v>,
    script: &'s EmbeddedScript<'s>,
    refs: &'r mut References,
}

impl ScriptWalker<'_, '_, '_> {
    fn eval(&self, e: &Expr) -> Value {
        match e {
            Expr::Name(n) if &*n.id == "self" => match self.script.owner {
                Owner::Component(c) => Value::Component(c),
                Owner::View => Value::View,
                _ => Value::Unknown,
            },
            Expr::Attribute(a) => match (self.eval(&a.value), a.attr.as_str()) {
                (Value::Component(c), "parent") => self
                    .view
                    .component(c)
                    .parent
                    .map_or(Value::Unknown, Value::Component),
                (Value::Component(_), "view") => Value::View,
                (Value::View, "rootContainer") => self
                    .view
                    .root_component()
                    .map_or(Value::Unknown, |r| Value::Component(r.id)),
                _ => Value::Unknown,
            },
            Expr::Call(call) => self.lookup(call).map_or(Value::Unknown, |(v, _)| v),
            _ => Value::Unknown,
        }
    }

    /// `receiver.getChild('a/b')` or `receiver.getSibling('a')` with a
    /// literal argument: the result and the components named on the way.
    fn lookup(&self, call: &ExprCall) -> Option<(Value, Vec<Named>)> {
        let Expr::Attribute(f) = &*call.func else {
            return None;
        };
        let method = f.attr.as_str();
        if method != "getChild" && method != "getSibling" {
            return None;
        }
        let [Expr::Str(arg)] = call.args.as_slice() else {
            return Some((Value::Unknown, Vec::new()));
        };
        let text = arg.s.as_str()?;
        let mut at = match (self.eval(&f.value), method) {
            (Value::Component(c), "getChild") => c,
            (Value::Component(c), _) => self.view.component(c).parent?,
            _ => return Some((Value::Unknown, Vec::new())),
        };
        let mut named = Vec::new();
        let mut offset = 0;
        for segment in text.split('/') {
            let next = child(self.view, at, segment);
            match next {
                Some(c) => {
                    named.push((c, offset, segment.len()));
                    at = c;
                }
                None => return Some((Value::Unknown, named)),
            }
            offset += segment.len() + 1;
        }
        Some((Value::Component(at), named))
    }

    /// The source range of a literal's content when it can be rewritten in
    /// place (a single literal without escapes).
    fn literal_content(&self, range: TextRange, value: &str) -> Option<TextRange> {
        let source: &str = &self.script.source;
        let text = source.get(range.to_std())?;
        let body = text.trim_start_matches(['u', 'U', 'r', 'R', 'b', 'B']);
        let prefix = text.len() - body.len();
        for q in ["\"\"\"", "'''", "\"", "'"] {
            if body.len() >= 2 * q.len()
                && body.starts_with(q)
                && body.ends_with(q)
                && &body[q.len()..body.len() - q.len()] == value
                && !value.contains('\\')
            {
                let start = range.start() + TextSize::from(prefix + q.len());
                return Some(TextRange::at(start, TextSize::of(value)));
            }
        }
        None
    }
}

impl<'a> Visitor<'a> for ScriptWalker<'_, '_, '_> {
    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Call(call) => {
                if let Some((value, named)) = self.lookup(call) {
                    let Some(Expr::Str(arg)) = call.args.first().filter(|_| call.args.len() == 1)
                    else {
                        self.refs.dynamic_lookup = true;
                        walk_expr(self, expr);
                        return;
                    };
                    let text = arg.s.as_str().unwrap_or_default();
                    let content = self.literal_content(arg.range, text);
                    let resolved = value != Value::Unknown;
                    match content {
                        Some(content) if resolved => {
                            for (c, offset, len) in named {
                                let range = TextRange::at(
                                    content.start() + TextSize::from(offset),
                                    TextSize::from(len),
                                );
                                self.refs.sites.push(Site {
                                    component: c,
                                    range: self.script.file_range(range),
                                });
                            }
                        }
                        _ => {
                            for segment in text.split('/') {
                                self.refs.blocked.insert(segment.to_owned());
                            }
                        }
                    }
                    // Visit the receiver, not the literal argument again.
                    if let Expr::Attribute(f) = &*call.func {
                        self.visit_expr(&f.value);
                    }
                    return;
                }
            }
            Expr::Attribute(a) if a.attr.as_str() == "name" => {
                if let Expr::Attribute(meta) = &*a.value
                    && meta.attr.as_str() == "meta"
                {
                    match self.eval(&meta.value) {
                        Value::Component(c) => {
                            self.refs.meta_name_read.insert(c);
                        }
                        _ => self.refs.dynamic_meta_name = true,
                    }
                }
            }
            Expr::Str(s) => {
                if let Some(text) = s.s.as_str() {
                    self.refs.literals.insert(text.to_owned());
                }
            }
            _ => {}
        }
        walk_expr(self, expr);
    }
}

/// Text replacing a name inside a JSON string, escaped like the file.
pub fn escaped(name: &str, style: iglint_json::EscapeStyle) -> String {
    let mut out = String::new();
    iglint_json::escape_into(&mut out, name, style);
    out
}
