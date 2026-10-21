//! Classification of `{...}` references.
//!
//! What a reference means depends on where the expression runs. Perspective
//! (module 3.3.3 in Ignition 8.3.3) resolves them in
//! `ExpressionBindingParseContext.createBoundExpression`:
//!
//! 1. A path starting with `[` is a tag path.
//! 2. Otherwise, a path accepted by `PropertyRef.isParseable` or
//!    `LegacyPropertyRef.isParseable` is a property reference.
//! 3. Anything else is a tag path in the default provider (so a typo such as
//!    `{this.prop.text}` silently becomes a tag subscription).
//!
//! In expression transforms (`ExpressionTransformParseContext`) the exact path
//! `value` is the incoming value; everything else falls through to the rules
//! above.
//!
//! The property grammar is the set of regular expressions in `PropertyRef`
//! (names `[A-Za-z_][A-Za-z0-9_-]*`) and `LegacyPropertyRef` (names
//! `[A-Za-z_][A-Za-z0-9_ ]*`), matched against the whole path:
//!
//! ```text
//! SUB      = "[" [0-9]* "]" | "[" ("\"" | "'") NAME ("\"" | "'") "]"
//! TAIL     = ("." NAME SUB*)*
//! TYPE     = props | position | custom | meta | params
//! PROP     = NAME "." TYPE "."? SUB? TAIL
//! SHORTCUT = (view | parent | this | session | page) SUB? TAIL
//! RELATIVE = ("../")+ (NAME "/")* PROP  |  "." "."* "/" (NAME "/")* PROP
//! ABSOLUTE = "/" (NAME "/")* PROP
//! ```

use iglint_text::{TextRange, TextSize};

use crate::tag_path::{TagPath, parse_tag_path};

/// Where an expression is evaluated, which decides how references resolve.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReferenceContext {
    /// Perspective expression bindings, expression-structure bindings and
    /// other Perspective expressions.
    PerspectiveBinding,
    /// Perspective expression transforms: `{value}` is the transform input.
    PerspectiveTransform,
    /// Tag expressions, alarm expressions and other gateway contexts: paths
    /// starting with `[` are tag paths; everything else is context-defined
    /// (UDT parameters, alarm properties) and returned as
    /// [`Reference::Other`].
    Gateway,
}

/// A classified reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reference {
    Property(PropertyRef),
    Tag(TagPath),
    /// `{value}` inside an expression transform.
    TransformValue,
    /// A context-defined name, verbatim.
    Other(String),
}

/// A Perspective property reference such as `{this.props.text}`,
/// `{../Label.props.text}` or `{/root/Panel.custom.items[0].name}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropertyRef {
    pub target: PropertyTarget,
    /// Property path after the target: for component targets this starts
    /// with the property type (`props`, `custom`, ...).
    pub segments: Vec<PropertySegment>,
    /// True when only the legacy grammar (names with spaces, no hyphens)
    /// accepts the path.
    pub legacy: bool,
    /// Range of the whole path in the expression source.
    pub range: TextRange,
}

/// What a property reference starts from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PropertyTarget {
    /// `this`: the component (or view) that owns the binding.
    This,
    /// `view`: the view.
    View,
    /// `parent`: the parent component.
    Parent,
    /// `session`: the session.
    Session,
    /// `page`: the page.
    Page,
    /// `./Child`, `../Sibling`, `.../Uncle`, `../../Uncle`: `up` parent
    /// steps from the owning component (`./` is 0, `../` is 1, `.../` and
    /// `../../` are 2), then down through `components`; the last one is the
    /// target.
    Relative {
        up: u32,
        prefix_range: TextRange,
        components: Vec<PropertySegment>,
    },
    /// `/root/Panel/Label`: components from the view's root container.
    Absolute { components: Vec<PropertySegment> },
}

impl PropertyTarget {
    /// The scope keyword for shortcut targets.
    pub fn scope_name(&self) -> Option<&'static str> {
        Some(match self {
            PropertyTarget::This => "this",
            PropertyTarget::View => "view",
            PropertyTarget::Parent => "parent",
            PropertyTarget::Session => "session",
            PropertyTarget::Page => "page",
            _ => return None,
        })
    }
}

/// One step of a property path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropertySegment {
    pub kind: SegmentKind,
    /// Range in the expression source.
    pub range: TextRange,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SegmentKind {
    /// `.name` (or a component name in a component path).
    Name(String),
    /// `[0]`. `None` for the empty brackets `[]` the grammar allows.
    /// Indexes beyond `usize` saturate.
    Index(Option<usize>),
    /// `['key']` or `["key"]`.
    Key(String),
}

impl PropertySegment {
    /// The name or key text, if any.
    pub fn name(&self) -> Option<&str> {
        match &self.kind {
            SegmentKind::Name(n) | SegmentKind::Key(n) => Some(n),
            SegmentKind::Index(_) => None,
        }
    }
}

impl PropertyRef {
    /// The property type for component targets and the first segment for
    /// shortcut targets: `props`, `custom`, `params`, `meta`, `position`.
    pub fn category(&self) -> Option<&str> {
        match &self.segments.first()?.kind {
            SegmentKind::Name(n) => Some(n),
            _ => None,
        }
    }

    /// Segment names and keys joined with `.`, indexes as `[i]`, for
    /// example `props.items[0].name`.
    pub fn property_path(&self) -> String {
        let mut out = String::new();
        for s in &self.segments {
            match &s.kind {
                SegmentKind::Name(n) | SegmentKind::Key(n) => {
                    if !out.is_empty() {
                        out.push('.');
                    }
                    out.push_str(n);
                }
                SegmentKind::Index(Some(i)) => {
                    out.push('[');
                    out.push_str(&i.to_string());
                    out.push(']');
                }
                SegmentKind::Index(None) => out.push_str("[]"),
            }
        }
        out
    }
}

/// Classifies a reference path. `offset` is the position of the path in the
/// expression source; ranges in the result are relative to that source.
pub fn classify(path: &str, offset: TextSize, context: ReferenceContext) -> Reference {
    match context {
        ReferenceContext::PerspectiveTransform if path == "value" => Reference::TransformValue,
        ReferenceContext::PerspectiveBinding | ReferenceContext::PerspectiveTransform => {
            if path.starts_with('[') {
                Reference::Tag(parse_tag_path(path, offset))
            } else if let Some(p) = parse_property_ref(path, offset) {
                Reference::Property(p)
            } else {
                Reference::Tag(parse_tag_path(path, offset))
            }
        }
        ReferenceContext::Gateway => {
            if path.starts_with('[') {
                Reference::Tag(parse_tag_path(path, offset))
            } else {
                Reference::Other(path.to_owned())
            }
        }
    }
}

/// Whether Perspective's `PropertyRef` or `LegacyPropertyRef` accepts `path`.
pub fn is_property_ref(path: &str) -> bool {
    matches_grammar(path, Names::Modern) || matches_grammar(path, Names::Legacy)
}

/// Parses `path` as a Perspective property reference.
pub fn parse_property_ref(path: &str, offset: TextSize) -> Option<PropertyRef> {
    let legacy = if matches_grammar(path, Names::Modern) {
        false
    } else if matches_grammar(path, Names::Legacy) {
        true
    } else {
        return None;
    };
    let names = if legacy { Names::Legacy } else { Names::Modern };
    Some(decompose(path, offset, names, legacy))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Names {
    Modern,
    Legacy,
}

const SCOPES: [&str; 5] = ["view", "parent", "this", "session", "page"];
const TYPES: [&str; 5] = ["props", "position", "custom", "meta", "params"];

struct M<'a> {
    b: &'a [u8],
    names: Names,
}

impl M<'_> {
    fn name(&self, i: usize) -> Option<usize> {
        let first = *self.b.get(i)?;
        if !(first.is_ascii_alphabetic() || first == b'_') {
            return None;
        }
        let mut j = i + 1;
        while let Some(&c) = self.b.get(j) {
            let ok = c.is_ascii_alphanumeric()
                || c == b'_'
                || match self.names {
                    Names::Modern => c == b'-',
                    Names::Legacy => c == b' ',
                };
            if !ok {
                break;
            }
            j += 1;
        }
        Some(j)
    }

    fn sub(&self, i: usize) -> Option<usize> {
        if self.b.get(i) != Some(&b'[') {
            return None;
        }
        let mut j = i + 1;
        while self.b.get(j).is_some_and(u8::is_ascii_digit) {
            j += 1;
        }
        if self.b.get(j) == Some(&b']') {
            return Some(j + 1);
        }
        if !matches!(self.b.get(i + 1), Some(b'"' | b'\'')) {
            return None;
        }
        let j = self.name(i + 2)?;
        if matches!(self.b.get(j), Some(b'"' | b'\'')) && self.b.get(j + 1) == Some(&b']') {
            Some(j + 2)
        } else {
            None
        }
    }

    fn subs(&self, mut i: usize) -> usize {
        while let Some(j) = self.sub(i) {
            i = j;
        }
        i
    }

    /// `("." NAME SUB*)*`, greedy; returns where it stops.
    fn tail(&self, mut i: usize) -> usize {
        while self.b.get(i) == Some(&b'.') {
            match self.name(i + 1) {
                Some(j) => i = self.subs(j),
                None => break,
            }
        }
        i
    }

    /// `NAME "." TYPE "."? SUB? TAIL` matching through the end.
    fn prop_to_end(&self, i: usize) -> bool {
        let Some(j) = self.name(i) else { return false };
        if self.b.get(j) != Some(&b'.') {
            return false;
        }
        let rest = &self.b[j + 1..];
        TYPES.iter().any(|t| {
            if !rest.starts_with(t.as_bytes()) {
                return false;
            }
            let k = j + 1 + t.len();
            let after_dot = if self.b.get(k) == Some(&b'.') {
                vec![k + 1, k]
            } else {
                vec![k]
            };
            after_dot.into_iter().any(|k| {
                let mut starts = vec![k];
                if let Some(s) = self.sub(k) {
                    starts.push(s);
                }
                starts.into_iter().any(|s| self.tail(s) == self.b.len())
            })
        })
    }

    /// `(NAME "/")* PROP` through the end.
    fn components_prop(&self, mut i: usize) -> bool {
        loop {
            if self.prop_to_end(i) {
                return true;
            }
            match self.name(i) {
                Some(j) if self.b.get(j) == Some(&b'/') => i = j + 1,
                _ => return false,
            }
        }
    }
}

fn matches_grammar(path: &str, names: Names) -> bool {
    let m = M {
        b: path.as_bytes(),
        names,
    };
    let b = m.b;
    // SHORTCUT
    for scope in SCOPES {
        if b.starts_with(scope.as_bytes()) {
            let i = scope.len();
            let mut starts = vec![i];
            if let Some(s) = m.sub(i) {
                starts.push(s);
            }
            if starts.into_iter().any(|s| m.tail(s) == b.len()) {
                return true;
            }
        }
    }
    // ABSOLUTE
    if b.first() == Some(&b'/') && m.components_prop(1) {
        return true;
    }
    // RELATIVE
    if b.first() == Some(&b'.') {
        if let Some(after) = relative_prefix_ups(b).map(|(_, end)| end)
            && m.components_prop(after)
        {
            return true;
        }
        if let Some(after) = dots_prefix(b).map(|(_, end)| end)
            && m.components_prop(after)
        {
            return true;
        }
    }
    false
}

/// `("../")+`: number of repetitions and end offset.
fn relative_prefix_ups(b: &[u8]) -> Option<(u32, usize)> {
    let mut i = 0;
    let mut n = 0;
    while b[i..].starts_with(b"../") {
        i += 3;
        n += 1;
    }
    (n > 0).then_some((n, i))
}

/// `"." "."* "/"`: number of dots and end offset.
fn dots_prefix(b: &[u8]) -> Option<(u32, usize)> {
    let mut i = 0;
    while b.get(i) == Some(&b'.') {
        i += 1;
    }
    (i > 0 && b.get(i) == Some(&b'/')).then_some((i as u32, i + 1))
}

/// Splits a path already known to match into target and segments.
fn decompose(path: &str, offset: TextSize, names: Names, legacy: bool) -> PropertyRef {
    let m = M {
        b: path.as_bytes(),
        names,
    };
    let b = m.b;
    let at = |i: usize| offset + TextSize::from(i);
    let rng = |i: usize, j: usize| TextRange::new(at(i), at(j));
    let full = rng(0, b.len());

    let mut segments = Vec::new();
    // Parses `SUB* ` at i into segments.
    let push_subs = |mut i: usize, segments: &mut Vec<PropertySegment>| -> usize {
        while let Some(j) = m.sub(i) {
            let inner = &path[i + 1..j - 1];
            let kind = if inner.starts_with('"') || inner.starts_with('\'') {
                SegmentKind::Key(inner[1..inner.len() - 1].to_owned())
            } else if inner.is_empty() {
                SegmentKind::Index(None)
            } else {
                SegmentKind::Index(Some(inner.parse().unwrap_or(usize::MAX)))
            };
            segments.push(PropertySegment {
                kind,
                range: rng(i, j),
            });
            i = j;
        }
        i
    };
    // Parses `("."? NAME SUB*)*` loosely (the grammar is already checked).
    let push_tail = |mut i: usize, segments: &mut Vec<PropertySegment>| {
        while i < b.len() {
            if b[i] == b'.' {
                i += 1;
                continue;
            }
            if b[i] == b'[' {
                i = push_subs(i, segments);
                continue;
            }
            let j = m.name(i).unwrap_or(b.len()).max(i + 1);
            segments.push(PropertySegment {
                kind: SegmentKind::Name(path[i..j].to_owned()),
                range: rng(i, j),
            });
            i = push_subs(j, segments);
        }
    };

    for scope in SCOPES {
        if path.starts_with(scope) && matches!(b.get(scope.len()), None | Some(b'.' | b'[')) {
            let target = match scope {
                "view" => PropertyTarget::View,
                "parent" => PropertyTarget::Parent,
                "this" => PropertyTarget::This,
                "session" => PropertyTarget::Session,
                _ => PropertyTarget::Page,
            };
            push_tail(scope.len(), &mut segments);
            return PropertyRef {
                target,
                segments,
                legacy,
                range: full,
            };
        }
    }

    // Component path: absolute or relative prefix, then NAME ("/" NAME)* "."
    let (mut i, relative) = if b[0] == b'/' {
        (1, None)
    } else if let Some((n, end)) = relative_prefix_ups(b).filter(|&(_, end)| m.components_prop(end))
    {
        (end, Some((n, end)))
    } else {
        let (dots, end) = dots_prefix(b).expect("grammar checked");
        (end, Some((dots - 1, end)))
    };
    let mut components = Vec::new();
    loop {
        let j = m.name(i).expect("grammar checked");
        components.push(PropertySegment {
            kind: SegmentKind::Name(path[i..j].to_owned()),
            range: rng(i, j),
        });
        if b.get(j) == Some(&b'/') {
            i = j + 1;
        } else {
            i = j;
            break;
        }
    }
    push_tail(i, &mut segments);
    let target = match relative {
        None => PropertyTarget::Absolute { components },
        Some((up, end)) => PropertyTarget::Relative {
            up,
            prefix_range: rng(0, end),
            components,
        },
    };
    PropertyRef {
        target,
        segments,
        legacy,
        range: full,
    }
}
