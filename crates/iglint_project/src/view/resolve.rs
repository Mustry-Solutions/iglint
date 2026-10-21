//! Resolution of property references (`{this.custom.x}`, `../Label.props.text`,
//! `/root/Panel.custom.y`, `view.params.z`, ...) against a view.

use iglint_expression::{PropertyRef, PropertyTarget, SegmentKind, parse_property_ref};
use iglint_text::{TextRange, TextSize};

use super::{PropCategory, View};
use crate::ids::ComponentId;
use crate::script::Owner;

/// What a reference points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RefTarget {
    View,
    Component(ComponentId),
}

/// A reference that resolved inside the view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedRef {
    pub target: RefTarget,
    /// `props`, `custom`, `params`, `meta`, `position`; `None` for a bare
    /// component reference.
    pub category: Option<String>,
    /// The first property name after the category.
    pub name: Option<String>,
    /// Index into [`View::properties`] when the reference names a custom
    /// property or view parameter.
    pub definition: Option<usize>,
    /// True when a component name on the way matched more than one
    /// sibling (the first one was used, as Perspective does).
    pub ambiguous: bool,
    /// True when only the property's existence could not be checked
    /// (`props`, `meta`, `position` are free-form or schema-defined).
    pub unchecked: bool,
}

/// A reference to session or page properties; check it against the
/// project's session properties ([`crate::ProjectIndex::session_property`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeRef {
    /// `session` or `page`.
    pub scope: &'static str,
    pub category: Option<String>,
    pub name: Option<String>,
}

/// Why a reference does not resolve. Ranges are relative to the source the
/// [`PropertyRef`] was parsed from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unresolved {
    /// A component-relative reference (`this`, `parent`, `./`, `../`) where
    /// there is no component (view-level or session properties).
    NoComponent,
    /// `parent` or `../` steps above the root container.
    AboveRoot { up: u32 },
    /// A component name that does not exist at that point of the path.
    MissingComponent { name: String, range: TextRange },
    /// `custom.<name>` or `params.<name>` that is not defined.
    MissingProperty {
        category: String,
        name: String,
        range: TextRange,
    },
}

/// Result of resolving a property reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefResolution {
    Resolved(ResolvedRef),
    Unresolved(Unresolved),
    /// Session or page property.
    Scope(ScopeRef),
    /// Cannot be decided from the view alone (references from outside a
    /// view).
    Dynamic,
}

impl RefResolution {
    /// True for [`RefResolution::Unresolved`].
    pub fn is_unresolved(&self) -> bool {
        matches!(self, RefResolution::Unresolved(_))
    }
}

impl<'a> View<'a> {
    /// Parses `path` (a property binding path) and resolves it. `None` when
    /// the path is not a property reference.
    pub fn resolve_path(&self, from: Owner, path: &str) -> Option<RefResolution> {
        parse_property_ref(path, TextSize::ZERO).map(|r| self.resolve_ref(from, &r))
    }

    /// Resolves a parsed reference seen from `from` (the owner of the
    /// binding, expression or script).
    pub fn resolve_ref(&self, from: Owner, reference: &PropertyRef) -> RefResolution {
        let from_component = match from {
            Owner::Component(id) => Some(id),
            Owner::View => None,
            Owner::Session => {
                return match &reference.target {
                    PropertyTarget::Session | PropertyTarget::This => {
                        scope_ref("session", reference)
                    }
                    PropertyTarget::Page => scope_ref("page", reference),
                    _ => RefResolution::Unresolved(Unresolved::NoComponent),
                };
            }
            Owner::Tag(_) | Owner::Resource => return RefResolution::Dynamic,
        };
        let mut ambiguous = false;
        let target = match &reference.target {
            PropertyTarget::Session => return scope_ref("session", reference),
            PropertyTarget::Page => return scope_ref("page", reference),
            PropertyTarget::View => RefTarget::View,
            PropertyTarget::This => match from_component {
                Some(c) => RefTarget::Component(c),
                None => RefTarget::View,
            },
            PropertyTarget::Parent => {
                let Some(c) = from_component else {
                    return RefResolution::Unresolved(Unresolved::NoComponent);
                };
                match self.components[c.index()].parent {
                    Some(p) => RefTarget::Component(p),
                    None => return RefResolution::Unresolved(Unresolved::AboveRoot { up: 1 }),
                }
            }
            PropertyTarget::Relative { up, components, .. } => {
                let Some(mut at) = from_component else {
                    return RefResolution::Unresolved(Unresolved::NoComponent);
                };
                for _ in 0..*up {
                    match self.components[at.index()].parent {
                        Some(p) => at = p,
                        None => {
                            return RefResolution::Unresolved(Unresolved::AboveRoot { up: *up });
                        }
                    }
                }
                for seg in components {
                    let name = seg.name().unwrap_or("");
                    match self.find_child(at, name) {
                        (Some(c), amb) => {
                            ambiguous |= amb;
                            at = c;
                        }
                        (None, _) => {
                            return RefResolution::Unresolved(Unresolved::MissingComponent {
                                name: name.to_owned(),
                                range: seg.range,
                            });
                        }
                    }
                }
                RefTarget::Component(at)
            }
            PropertyTarget::Absolute { components } => {
                let Some(root) = self.components.first() else {
                    let (name, range) = components
                        .first()
                        .map(|s| (s.name().unwrap_or("").to_owned(), s.range))
                        .unwrap_or_default();
                    return RefResolution::Unresolved(Unresolved::MissingComponent { name, range });
                };
                let mut iter = components.iter();
                let mut at = root.id;
                if let Some(first) = iter.next() {
                    let name = first.name().unwrap_or("");
                    if name != root.name {
                        return RefResolution::Unresolved(Unresolved::MissingComponent {
                            name: name.to_owned(),
                            range: first.range,
                        });
                    }
                }
                for seg in iter {
                    let name = seg.name().unwrap_or("");
                    match self.find_child(at, name) {
                        (Some(c), amb) => {
                            ambiguous |= amb;
                            at = c;
                        }
                        (None, _) => {
                            return RefResolution::Unresolved(Unresolved::MissingComponent {
                                name: name.to_owned(),
                                range: seg.range,
                            });
                        }
                    }
                }
                RefTarget::Component(at)
            }
        };
        self.check_property(target, reference, ambiguous)
    }

    /// The first child of `parent` named `name`, and whether more than one
    /// child has that name.
    fn find_child(&self, parent: ComponentId, name: &str) -> (Option<ComponentId>, bool) {
        let mut found = None;
        let mut count = 0;
        for c in self.children(parent) {
            if c.name == name {
                count += 1;
                found.get_or_insert(c.id);
            }
        }
        (found, count > 1)
    }

    fn check_property(
        &self,
        target: RefTarget,
        reference: &PropertyRef,
        ambiguous: bool,
    ) -> RefResolution {
        let category = reference.category().map(str::to_owned);
        let name_segment = reference.segments.get(1);
        let name = name_segment.and_then(|s| match &s.kind {
            SegmentKind::Name(n) | SegmentKind::Key(n) => Some(n.clone()),
            SegmentKind::Index(_) => None,
        });
        let owner = match target {
            RefTarget::View => Owner::View,
            RefTarget::Component(c) => Owner::Component(c),
        };
        let checked = match (target, category.as_deref()) {
            (_, Some("custom")) => Some(PropCategory::Custom),
            (RefTarget::View, Some("params")) => Some(PropCategory::Params),
            _ => None,
        };
        let mut definition = None;
        if let (Some(cat), Some(n)) = (checked, name.as_deref()) {
            definition = self
                .properties
                .iter()
                .position(|p| p.owner == owner && p.category == cat && p.name == n);
            if definition.is_none() {
                return RefResolution::Unresolved(Unresolved::MissingProperty {
                    category: cat.name().to_owned(),
                    name: n.to_owned(),
                    range: name_segment.map(|s| s.range).unwrap_or(reference.range),
                });
            }
        }
        RefResolution::Resolved(ResolvedRef {
            target,
            category,
            name,
            definition,
            ambiguous,
            unchecked: checked.is_none(),
        })
    }
}

fn scope_ref(scope: &'static str, reference: &PropertyRef) -> RefResolution {
    RefResolution::Scope(ScopeRef {
        scope,
        category: reference.category().map(str::to_owned),
        name: reference
            .segments
            .get(1)
            .and_then(|s| s.name())
            .map(str::to_owned),
    })
}
