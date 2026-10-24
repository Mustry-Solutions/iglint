//! Where a view refers to its components by name, for the component rename
//! fix of `NAME001`.
//!
//! References are found by *resolution*, not by name: `{../Label.props.text}`
//! is a reference to the `Label` that is a sibling of the expression's owner,
//! not to every component called `Label`. Sources:
//!
//! * property references in every expression of the view (every binding
//!   and transform field Perspective parses, measured in
//!   `iglint_project::perspective_fields`);
//! * property binding paths and indirect tag binding references;
//! * `getChild('...')` / `getSibling('...')` calls with literal arguments in
//!   scripts, on receivers that resolve statically (`self`, `self.parent`,
//!   `self.view.rootContainer`, earlier `getChild`/`getSibling` calls).
//!
//! A literal lookup whose receiver cannot be resolved, or whose literal
//! cannot be rewritten in place, blocks renaming components of that name.
//!
//! # Component names are observable data
//!
//! A component's name is readable through `meta.name`, so a rename is
//! unsafe wherever the view could read it. The only understood form is a
//! `meta.name` read on a value that resolves to a component (a script's
//! `<component>.meta.name`, a `{this.meta.name}` or `{Label.meta.name}`
//! reference): that blocks renaming only that component. Every other use
//! of `.meta` fails closed and blocks every rename ([`References::dynamic_meta_name`]):
//! `meta.name` on a value that does not resolve, an alias of `.meta`
//! (`m = self.meta`), a subscript (`self.meta['name']`), `getattr` on it,
//! another member (`self.meta.tooltip`), or `.meta` passed to code.
//! Handing a component itself to code outside the view is covered
//! separately (`components_may_escape`), since that code could read
//! `meta.name` too.

use iglint_expression::{PropertyRef, PropertyTarget, Reference, misspaced_property_ref};
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
    /// Every string literal of the view's scripts, lookup arguments
    /// included, with the file range of the literal: where a name occurs
    /// that a rename *to* it could make a lookup find.
    pub script_strings: Vec<(String, TextRange)>,
    /// `getChild(x)` / `getSibling(x)` with a computed argument.
    pub dynamic_lookup: bool,
    /// References Perspective reads as tag paths only because of
    /// whitespace inside the braces (`{ ../Label.props.text }`): the
    /// components they would name (or whose `meta` they would read) and
    /// where. Renaming cannot be proven not to change what they mean.
    pub fragile: Vec<(ComponentId, TextRange)>,
}

impl References {
    /// The references of a view. `parsed` are the parses of
    /// `view.scripts`, in order.
    pub fn collect(view: &View<'_>, parsed: &[iglint_python::Parsed]) -> References {
        let mut refs = References::default();
        for e in &view.expressions {
            for (bound, r) in e.references() {
                match r {
                    Reference::Property(p) => {
                        refs.property_ref(view, e.owner, &p, &|r| e.file_range(r));
                    }
                    Reference::Tag(_) => {
                        let Some(p) = misspaced_property_ref(&bound.path, bound.path_range.start())
                        else {
                            continue;
                        };
                        let mut named = References::default();
                        named.property_ref(view, e.owner, &p, &|r| e.file_range(r));
                        let at = e.file_range(bound.path_range);
                        refs.fragile
                            .extend(named.sites.iter().map(|s| (s.component, at)));
                        refs.fragile
                            .extend(named.meta_name_read.iter().map(|c| (*c, at)));
                    }
                    _ => {}
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
        // A reference may be modelled twice (an indirect tag binding's
        // reference is also an expression): one edit per place.
        refs.sites
            .sort_by_key(|s| (s.range.start(), s.range.end(), s.component));
        refs.sites
            .dedup_by_key(|s| (s.range.start(), s.range.end(), s.component));
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
        // Latin-1, not UTF-8: a byte-string argument that is not valid
        // UTF-8 must still be read so the component it names is seen.
        let text = arg.s.to_latin1();
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
                && body
                    .strip_prefix(q)
                    .and_then(|b| b.strip_suffix(q))
                    .is_some_and(|inner| inner == value)
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
                    let text = arg.s.to_latin1();
                    self.refs
                        .script_strings
                        .push((text.to_string(), self.script.file_range(arg.range)));
                    let content = self.literal_content(arg.range, &text);
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
                    // The one understood form: `<value>.meta.name`.
                    match self.eval(&meta.value) {
                        Value::Component(c) => {
                            self.refs.meta_name_read.insert(c);
                        }
                        _ => self.refs.dynamic_meta_name = true,
                    }
                    // Visit the base before `.meta` (lookups may be there),
                    // but not the `.meta` node, which would otherwise be
                    // taken as an un-understood meta access below.
                    self.visit_expr(&meta.value);
                    return;
                }
            }
            Expr::Attribute(a) if a.attr.as_str() == "meta" => {
                // Any other use of `.meta`: the component name is
                // observable data reachable through it, so renaming is
                // unsafe. Fail closed. Covers aliasing (`m = self.meta`),
                // subscripts (`self.meta['name']`), `getattr(self.meta,
                // ...)`, other members (`self.meta.tooltip`) and passing
                // `.meta` to code. A `.meta` node reaches here only when no
                // understood `.meta.name` consumer handled it above.
                self.refs.dynamic_meta_name = true;
                self.visit_expr(&a.value);
                return;
            }
            Expr::Str(s) => {
                // Latin-1 so a non-UTF-8 byte string still registers as a
                // literal that may equal the component name.
                let text = s.s.to_latin1().into_owned();
                self.refs
                    .script_strings
                    .push((text.clone(), self.script.file_range(s.range)));
                self.refs.literals.insert(text);
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
