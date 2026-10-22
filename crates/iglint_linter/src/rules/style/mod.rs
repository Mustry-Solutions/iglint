//! STYLE: style classes.

pub mod empty_style_class;
pub mod unknown_style_class;
pub mod unused_style_class;

use iglint_project::kinds::ResourceKind;
use iglint_project::summary::Facts;
use iglint_project::{Container, ProjectId, ProjectIndex};

/// Whether the project stylesheet or a gateway theme in the workspace
/// mentions the CSS class `class` in a selector.
pub(crate) fn css_mentions(index: &ProjectIndex<'_>, project: ProjectId, class: &str) -> bool {
    let ws = index.workspace();
    let own = ws.effective_resources(Container::Project(project));
    let themes = ws
        .resources
        .iter()
        .filter(|r| {
            r.kind == ResourceKind::PerspectiveAsset
                && &*r.type_id == "themes"
                && !r.manifest.missing
        })
        .map(|r| r.id);
    own.into_iter()
        .filter(|r| ws.resource(*r).kind == ResourceKind::Stylesheet)
        .chain(themes)
        .any(|r| match index.summary(r).map(|s| &s.facts) {
            Some(Facts::Css(css)) => css.classes.iter().any(|c| c == class),
            _ => false,
        })
}
