//! Loads a private Ignition repository. Run with
//! `IGLINT_PRIVATE_CORPUS=/path cargo test -p iglint_project --release --test corpus -- --ignored --nocapture`.
//! Prints aggregate numbers only (counts and timings, never names).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use iglint_expression::Reference;
use iglint_project::view::RefResolution;
use iglint_project::{ModelData, ProjectIndex, ResourceModel, Workspace, build_model, summarize};
use rayon::prelude::*;

fn roots() -> Option<Vec<PathBuf>> {
    let root = PathBuf::from(std::env::var_os("IGLINT_PRIVATE_CORPUS")?);
    Some(vec![root.join("projects"), root.join("services")])
}

#[derive(Default)]
struct Stats {
    scripts: BTreeMap<&'static str, usize>,
    expressions: usize,
    invalid_expressions: usize,
    script_bytes: usize,
    problems: BTreeMap<String, usize>,
    file_errors: usize,
    property_refs: usize,
    unresolved_refs: usize,
    scope_refs: usize,
    components: usize,
    bindings: usize,
    tags: usize,
}

impl Stats {
    fn merge(mut self, o: Stats) -> Stats {
        for (k, v) in o.scripts {
            *self.scripts.entry(k).or_default() += v;
        }
        for (k, v) in o.problems {
            *self.problems.entry(k).or_default() += v;
        }
        self.expressions += o.expressions;
        self.invalid_expressions += o.invalid_expressions;
        self.script_bytes += o.script_bytes;
        self.file_errors += o.file_errors;
        self.property_refs += o.property_refs;
        self.unresolved_refs += o.unresolved_refs;
        self.scope_refs += o.scope_refs;
        self.components += o.components;
        self.bindings += o.bindings;
        self.tags += o.tags;
        self
    }
}

fn stats(model: &ResourceModel<'_>) -> Stats {
    let mut s = Stats::default();
    for (_, script) in model.scripts() {
        *s.scripts.entry(script.kind.name()).or_default() += 1;
        s.script_bytes += script.source.len();
    }
    for (_, e) in model.expressions() {
        s.expressions += 1;
        if !e.parsed.is_valid() {
            s.invalid_expressions += 1;
        }
    }
    for p in model.problems() {
        *s.problems.entry(p.message.clone()).or_default() += 1;
    }
    s.file_errors += model.errors.len();
    if let ModelData::View { view, .. } = &model.data {
        s.components += view.components.len();
        s.bindings += view.bindings.len();
        for e in &view.expressions {
            for (_, r) in e.references() {
                if let Reference::Property(p) = r {
                    s.property_refs += 1;
                    match view.resolve_ref(e.owner, &p) {
                        RefResolution::Unresolved(_) => s.unresolved_refs += 1,
                        RefResolution::Scope(_) => s.scope_refs += 1,
                        _ => {}
                    }
                }
            }
        }
        for b in &view.bindings {
            if let iglint_project::view::BindingConfig::Property(p) = &b.typed
                && let Some(r) = &p.reference
            {
                s.property_refs += 1;
                match view.resolve_ref(b.owner, r) {
                    RefResolution::Unresolved(_) => s.unresolved_refs += 1,
                    RefResolution::Scope(_) => s.scope_refs += 1,
                    _ => {}
                }
            }
        }
    }
    if let ModelData::Tags(t) = &model.data {
        s.tags += t.tags.len();
    }
    s
}

#[test]
#[ignore = "needs IGLINT_PRIVATE_CORPUS"]
fn corpus_models_cleanly() {
    let Some(roots) = roots() else {
        eprintln!("IGLINT_PRIVATE_CORPUS not set");
        return;
    };
    // Warm the file system cache.
    let _ = Workspace::discover(&roots);

    let t0 = Instant::now();
    let ws = Workspace::discover(&roots);
    let t_discover = t0.elapsed();

    let t1 = Instant::now();
    let results: Vec<(iglint_project::ResourceSummary, Stats)> = ws
        .resources
        .par_iter()
        .map(|r| {
            let loaded = ws.load_resource(r.id);
            let model = build_model(&ws, &loaded);
            (summarize(&model), stats(&model))
        })
        .collect();
    let t_model = t1.elapsed();
    let (summaries, stats): (Vec<_>, Vec<_>) = results.into_iter().unzip();
    let total = stats.into_iter().fold(Stats::default(), Stats::merge);

    let t2 = Instant::now();
    let index = ProjectIndex::build(&ws, summaries);
    let t_index = t2.elapsed();

    // Reference checks through the index, aggregated.
    let mut view_refs = 0;
    let mut missing_views = 0;
    let mut style_refs = 0;
    let mut missing_styles = 0;
    let mut query_refs = 0;
    let mut missing_queries = 0;
    let mut tag_refs = 0;
    let mut missing_tags = 0;
    let mut unknown_tags = 0;
    for p in &ws.projects {
        for rid in ws.effective_resources(iglint_project::Container::Project(p.id)) {
            let Some(s) = index.summary(rid) else {
                continue;
            };
            for v in &s.references.views {
                view_refs += 1;
                if index.view(p.id, v).existence().is_missing() {
                    missing_views += 1;
                }
            }
            for c in &s.references.style_classes {
                style_refs += 1;
                if index.style_class_reference(p.id, c).is_missing() {
                    missing_styles += 1;
                }
            }
            for q in &s.references.named_queries {
                query_refs += 1;
                if index.named_query(p.id, q).existence().is_missing() {
                    missing_queries += 1;
                }
            }
        }
    }
    for s in ws.resources.iter().filter_map(|r| index.summary(r.id)) {
        for t in &s.references.tag_paths {
            tag_refs += 1;
            match index.tag(t) {
                iglint_project::Existence::Missing => missing_tags += 1,
                iglint_project::Existence::Unknown => unknown_tags += 1,
                _ => {}
            }
        }
    }

    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    for r in &ws.resources {
        *kinds.entry(r.kind.name()).or_default() += 1;
    }
    let mut file_kinds: BTreeMap<&str, usize> = BTreeMap::new();
    for f in &ws.files {
        *file_kinds.entry(f.kind.name()).or_default() += 1;
    }
    let unlisted = ws.files.iter().filter(|f| f.listed == Some(false)).count();
    let missing_listed: usize = ws.resources.iter().map(|r| r.missing_files.len()).sum();

    println!(
        "projects: {} ({} context only), collections: {}",
        ws.projects.len(),
        ws.projects.iter().filter(|p| p.context_only).count(),
        ws.collections.len()
    );
    println!(
        "files: {}, resources: {}",
        ws.files.len(),
        ws.resources.len()
    );
    println!("resources per kind: {kinds:?}");
    println!("files per kind: {file_kinds:?}");
    println!("unlisted data files: {unlisted}, listed but missing: {missing_listed}");
    println!("workspace problems: {}", ws.problems.len());
    println!(
        "components: {}, bindings: {}, tags: {}",
        total.components, total.bindings, total.tags
    );
    println!(
        "scripts: {:?} ({} bytes wrapped)",
        total.scripts, total.script_bytes
    );
    println!(
        "expressions: {} ({} with syntax errors)",
        total.expressions, total.invalid_expressions
    );
    println!(
        "view property references: {} ({} unresolved, {} session/page)",
        total.property_refs, total.unresolved_refs, total.scope_refs
    );
    println!(
        "view refs: {view_refs} ({missing_views} missing), style class refs: {style_refs} ({missing_styles} missing), named query refs: {query_refs} ({missing_queries} missing)"
    );
    println!("tag path refs: {tag_refs} ({missing_tags} missing, {unknown_tags} unknown)");
    println!("file errors: {}", total.file_errors);
    println!("model problems: {}", total.problems.values().sum::<usize>());
    for (k, v) in &total.problems {
        println!("  {v} x {k}");
    }
    println!(
        "timing: discover {:.1} ms, load+model+summarize {:.1} ms, index {:.1} ms, total {:.1} ms",
        t_discover.as_secs_f64() * 1e3,
        t_model.as_secs_f64() * 1e3,
        t_index.as_secs_f64() * 1e3,
        (t_discover + t_model + t_index).as_secs_f64() * 1e3
    );
    assert_eq!(total.file_errors, 0, "files that failed to load or parse");
}
