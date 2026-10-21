//! Discovery and model of Ignition 8.3 projects and gateway resources.
//!
//! The pipeline, each step parallel and each file read and parsed once:
//!
//! 1. [`Workspace::discover`] walks the roots, finds projects
//!    (`project.json`) and gateway config collections (`config-mode.json`),
//!    their resources and files, classifies every file ([`FileKind`]),
//!    parses manifests and computes inheritance.
//! 2. [`Workspace::load_resource`] reads a resource's data files;
//!    [`build_model`] turns them into a typed [`ResourceModel`] (views,
//!    style classes, page config, session props, named queries, event
//!    scripts, WebDev, library modules, tags, gateway settings) that borrows
//!    from the parsed JSON.
//! 3. [`summarize`] reduces a model to an owned [`ResourceSummary`]; the
//!    script layer may add library symbols and message sends.
//! 4. [`ProjectIndex::build`] answers cross-resource questions (does a view
//!    exist, its params, named queries, pages, message handlers, library
//!    symbols, tags, usage counts).
//!
//! Embedded code is exposed as [`EmbeddedScript`] (Python wrapped in the
//! exact header Ignition generates, with a [`SourceMap`] back to the file)
//! and [`EmbeddedExpression`] (parsed with `iglint_expression`). Node path
//! formats are documented in [`node_path`].
//!
//! ```no_run
//! use iglint_project::{ProjectIndex, Workspace, build_model, summarize};
//!
//! let ws = Workspace::discover(&["projects", "services"]);
//! let summaries = iglint_project::summarize_workspace(&ws);
//! let index = ProjectIndex::build(&ws, summaries);
//! for r in &ws.resources {
//!     let loaded = ws.load_resource(r.id);
//!     let model = build_model(&ws, &loaded);
//!     for (_file, script) in model.scripts() {
//!         let _python = &script.source; // parse with the Python layer
//!     }
//! }
//! # let _ = index;
//! ```

mod document;
pub mod dump;
pub mod event_resource;
pub mod expression;
pub mod gateway;
mod ids;
pub mod index;
pub mod kinds;
pub mod library;
pub mod manifest;
pub mod model;
pub mod named_query;
pub mod node_path;
pub mod perspective;
pub mod script;
pub mod source_map;
pub mod summary;
pub mod tags;
pub mod view;
pub mod workspace;

use rayon::prelude::*;

pub use document::JsonDocument;
pub use expression::{EmbeddedExpression, ExpressionKind};
pub use ids::*;
pub use index::{HandlerSite, Lookup, PageMatch, ProjectIndex};
pub use kinds::{EventScriptKind, FileKind, HttpMethod, ResourceKind, RunScope};
pub use library::{LibraryLookup, LibraryTree, ModuleSymbols};
pub use model::{LoadedFile, LoadedResource, ModelData, ResourceModel, build_model};
pub use script::{
    EmbeddedScript, HeaderStatus, IMPLICIT_GLOBALS, Owner, ScriptFile, ScriptKind, TagEventKind,
};
pub use source_map::SourceMap;
pub use summary::{ResourceSummary, summarize};
pub use tags::{Tag, TagFolder};
pub use view::{Component, SessionProps, View, build_view};
pub use workspace::{Container, Existence, Workspace};

/// Loads, models and summarizes every resource of the workspace in
/// parallel. Convenience for callers that do not lint while summarizing.
pub fn summarize_workspace(ws: &Workspace) -> Vec<ResourceSummary> {
    ws.resources
        .par_iter()
        .map(|r| {
            let loaded = ws.load_resource(r.id);
            let model = build_model(ws, &loaded);
            summarize(&model)
        })
        .collect()
}
