//! Review round 28: every resolver of dotted library paths agrees with
//! Jython's name resolution through the imports of library modules.
//!
//! Generated libraries (modules that define functions, import a module
//! from outside the library, import library modules, packages and names
//! under aliases, star-import other modules, or bind a name to one of two
//! imports) and generated paths through them are evaluated by a small
//! model of how Ignition resolves them (a library module's import binds
//! the target's object, measured in `chains.cases` of the linter's
//! library-routes oracle), and compared with the project index: every
//! name the path reads is counted as used (or its module may have any
//! name read), and where the path ends is what the model says, or unknown
//! where the model's answer depends on something the index does not
//! follow with certainty (a star import, a name with several bindings, a
//! cycle).

use std::collections::BTreeSet;
use std::path::Path;

use iglint_project::library::{ModuleSymbols, Signature, Symbol, SymbolKind, VariableImport};
use iglint_project::{LibraryLookup, ProjectIndex, ResourceId, Workspace, summarize_workspace};
use iglint_text::TextRange;
use proptest::prelude::*;

/// The modules of the generated library, and `user`, which holds the
/// paths.
const MODULES: &[&str] = &["m0", "m1", "m2", "lib.a", "lib.b", "pkg.c.d"];
const PACKAGES: &[&str] = &["lib", "pkg", "pkg.c"];
/// What imports may name: modules, packages, names of modules (some
/// missing).
const TARGETS: &[&str] = &[
    "m0",
    "m1",
    "m2",
    "lib.a",
    "lib.b",
    "pkg.c.d",
    "lib",
    "pkg",
    "pkg.c",
    "lib.a.f",
    "m1.json",
    "lib.b.u",
    "m2.a",
    "pkg.c.d.v",
];
/// Names an import may bind.
const ALIASES: &[&str] = &["a", "b", "u", "w"];
/// Segments a generated path may continue with.
const SEGMENTS: &[&str] = &[
    "a", "b", "u", "w", "v", "f", "json", "nope", "lib", "c", "d", "m1", "_p",
];

#[derive(Clone, Debug)]
enum Binding {
    Function,
    /// `import json`: outside the library.
    Leaf,
    /// An import of a library object.
    Import(String),
    /// A name bound by each of these imports (and possibly otherwise).
    Ambiguous(Vec<String>),
}

#[derive(Clone, Debug)]
struct Spec {
    names: Vec<(String, Binding)>,
    /// `from <path> import *`, in order, before the module's own bindings
    /// (review round 30, B1: also of a module named through another
    /// module's import, a package or a name).
    stars: Vec<String>,
}

/// What a star import may name: a module, a package, a name, or a path
/// through another module's import.
const STAR_TARGETS: &[&str] = &[
    "m0", "m1", "m2", "lib.a", "lib.b", "pkg.c.d", "lib", "pkg.c", "m0.a", "m1.u", "m2.w",
    "lib.a.b", "lib.b.u", "m0.a.u", "m1.v", "lib.a.f",
];

fn spec() -> impl Strategy<Value = Spec> {
    (
        any::<bool>(),
        any::<bool>(),
        prop::collection::vec((0..ALIASES.len(), 0..TARGETS.len()), 0..4),
        prop::option::of((0..TARGETS.len(), 0..TARGETS.len())),
        prop::collection::vec(0..STAR_TARGETS.len(), 0..3),
        0..4usize,
    )
        .prop_map(|(f, json, imports, ambiguous, stars, some)| {
            let mut names: Vec<(String, Binding)> = Vec::new();
            let mut add = |name: &str, b: Binding| {
                if !names.iter().any(|(n, _)| n == name) {
                    names.push((name.to_owned(), b));
                }
            };
            if f {
                add("f", Binding::Function);
            }
            if json {
                add("json", Binding::Leaf);
            }
            for (alias, target) in imports {
                add(ALIASES[alias], Binding::Import(TARGETS[target].to_owned()));
            }
            if let Some((t1, t2)) = ambiguous {
                add(
                    "v",
                    Binding::Ambiguous(vec![TARGETS[t1].to_owned(), TARGETS[t2].to_owned()]),
                );
            }
            Spec {
                names,
                // About half of the modules star-import.
                stars: match some {
                    0 | 1 => stars
                        .into_iter()
                        .map(|s| STAR_TARGETS[s].to_owned())
                        .collect(),
                    _ => Vec::new(),
                },
            }
        })
}

fn paths() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec(
        (
            0..MODULES.len() + PACKAGES.len(),
            prop::collection::vec(0..SEGMENTS.len(), 0..5),
        )
            .prop_map(|(root, rest)| {
                let mut p = MODULES
                    .iter()
                    .chain(PACKAGES)
                    .nth(root)
                    .copied()
                    .unwrap_or("m0")
                    .to_owned();
                for s in rest {
                    p.push('.');
                    p.push_str(SEGMENTS[s]);
                }
                p
            }),
        1..12,
    )
}

/// Where the model says a path ends.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Out {
    Package(String),
    Module(usize),
    /// A name of a module that the model does not look into (a function,
    /// an import from outside the library), with the segments after it.
    Name(usize, String, usize),
    /// Segment `i` (of the path as written) does not exist.
    Missing(usize),
    /// Raises while a module loads (a missing import target, a cycle).
    Fails,
}

struct Model<'s> {
    specs: &'s [Spec],
    reads: BTreeSet<(usize, String)>,
    /// The answer depends on what the index does not follow with
    /// certainty (a star import, an ambiguous name, a cycle).
    uncertain: bool,
    /// The path passes through an ambiguous name: the index may end there.
    through_variable: Option<(usize, String)>,
    loading: Vec<(usize, String)>,
}

fn module_index(dotted: &str) -> Option<usize> {
    MODULES.iter().position(|m| *m == dotted)
}

impl Model<'_> {
    /// Where the name `name` of `module` comes from: the module that binds
    /// it (none for a package's child a star import of the package binds)
    /// and the binding. Star imports bind every name not starting with `_`
    /// of what they name when the module loads (also the names that module
    /// got from its own star imports), the last one winning, before the
    /// module's own bindings (measured in `chains.cases`).
    fn lookup(&mut self, module: usize, name: &str) -> Option<(Option<usize>, Binding)> {
        let spec = &self.specs[module];
        if let Some((_, b)) = spec.names.iter().find(|(n, _)| n == name) {
            return Some((Some(module), b.clone()));
        }
        if name.starts_with('_') {
            return None;
        }
        let stars = spec.stars.clone();
        for star in stars.iter().rev() {
            let key = (module, format!("*{star}"));
            if self.loading.contains(&key) {
                // A cycle of star imports: Ignition raises ImportError.
                self.uncertain = true;
                continue;
            }
            self.loading.push(key);
            let segs: Vec<&str> = star.split('.').collect();
            let target = self.eval(&segs);
            let found = match target {
                Out::Module(m) => self.lookup(m, name),
                Out::Package(p) => {
                    let child = format!("{p}.{name}");
                    (module_index(&child).is_some() || PACKAGES.contains(&child.as_str()))
                        .then(|| (None, Binding::Import(child)))
                }
                _ => {
                    // The star import fails (or what it binds is not
                    // modelled).
                    self.uncertain = true;
                    None
                }
            };
            self.loading.pop();
            if found.is_some() {
                return found;
            }
        }
        None
    }

    fn eval(&mut self, segs: &[&str]) -> Out {
        let mut at = if module_index(segs[0]).is_some() || PACKAGES.contains(&segs[0]) {
            match module_index(segs[0]) {
                Some(m) => Out::Module(m),
                None => Out::Package(segs[0].to_owned()),
            }
        } else {
            return Out::Missing(0);
        };
        for i in 1..segs.len() {
            let seg = segs[i];
            match at {
                Out::Package(p) => {
                    let child = format!("{p}.{seg}");
                    at = match module_index(&child) {
                        Some(m) => Out::Module(m),
                        None if PACKAGES.contains(&child.as_str()) => Out::Package(child),
                        None => return Out::Missing(i),
                    };
                }
                Out::Module(m) => {
                    if self
                        .loading
                        .iter()
                        .any(|(l, k)| *l == m && k.starts_with('*'))
                    {
                        // A star import of the module itself, while it
                        // loads: its names are not bound yet.
                        self.uncertain = true;
                        return Out::Fails;
                    }
                    if !self.specs[m].stars.is_empty()
                        && !self.specs[m].names.iter().any(|(n, _)| n == seg)
                    {
                        // The index does not know which names a star
                        // import binds.
                        self.uncertain = true;
                    }
                    let Some((owner, binding)) = self.lookup(m, seg) else {
                        return Out::Missing(i);
                    };
                    if let Some(owner) = owner {
                        self.reads.insert((owner, seg.to_owned()));
                    }
                    let owner = owner.unwrap_or(m);
                    let rest = segs.len() - i - 1;
                    match binding {
                        Binding::Function | Binding::Leaf => {
                            return Out::Name(owner, seg.to_owned(), rest);
                        }
                        Binding::Import(target) => match self.load(owner, seg, &target) {
                            Out::Name(n, name, 0) => return Out::Name(n, name, rest),
                            Out::Package(p) => at = Out::Package(p),
                            Out::Module(n) => at = Out::Module(n),
                            _ => return Out::Fails,
                        },
                        Binding::Ambiguous(targets) => {
                            self.uncertain = true;
                            self.through_variable.get_or_insert((owner, seg.to_owned()));
                            let key = (owner, seg.to_owned());
                            if self.loading.contains(&key) {
                                return Out::Fails;
                            }
                            self.loading.push(key);
                            for t in targets {
                                let mut p: Vec<&str> = t.split('.').collect();
                                p.extend(&segs[i + 1..]);
                                let _ = self.eval(&p);
                            }
                            self.loading.pop();
                            return Out::Fails;
                        }
                    }
                }
                _ => unreachable!(),
            }
        }
        at
    }

    /// What `owner`'s import of `target` as `name` binds.
    fn load(&mut self, owner: usize, name: &str, target: &str) -> Out {
        let key = (owner, name.to_owned());
        if self.loading.contains(&key) {
            self.uncertain = true;
            return Out::Fails;
        }
        self.loading.push(key);
        let segs: Vec<&str> = target.split('.').collect();
        let out = self.eval(&segs);
        self.loading.pop();
        match out {
            Out::Missing(_) => Out::Fails,
            other => other,
        }
    }
}

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn workspace(root: &Path) -> Workspace {
    write(
        root,
        "projects/p/project.json",
        r#"{"title": "p", "enabled": true, "inheritable": false}"#,
    );
    for m in MODULES.iter().chain(&["user"]) {
        let dir = format!("projects/p/ignition/script-python/{}", m.replace('.', "/"));
        write(root, &format!("{dir}/code.py"), "\n");
        write(
            root,
            &format!("{dir}/resource.json"),
            r#"{"scope": "G", "version": 1, "restricted": false, "overridable": true, "files": ["code.py"], "attributes": {}}"#,
        );
    }
    Workspace::discover(&[root.join("projects")])
}

fn symbols(spec: &Spec) -> ModuleSymbols {
    let mut out = ModuleSymbols {
        dynamic: !spec.stars.is_empty(),
        // The script layer's flag for a module dynamic only by its star
        // import (review round 29, M2: such a module passes on what the
        // star-imported module binds, nothing else).
        names_from_stars_only: !spec.stars.is_empty(),
        star_imports: spec.stars.clone(),
        ..ModuleSymbols::default()
    };
    for (name, b) in &spec.names {
        let kind = match b {
            Binding::Function => SymbolKind::Function(Signature::default()),
            Binding::Leaf => SymbolKind::Import {
                target: Some("json".into()),
            },
            Binding::Import(t) => SymbolKind::Import {
                target: Some(t.clone()),
            },
            Binding::Ambiguous(targets) => {
                for t in targets {
                    out.variable_imports.push(VariableImport {
                        name: name.clone(),
                        target: t.clone(),
                        several: true,
                    });
                }
                SymbolKind::Variable
            }
        };
        out.symbols.push(Symbol {
            name: name.clone(),
            kind,
            range: TextRange::default(),
        });
    }
    out
}

fn check(ws: &Workspace, specs: &[Spec], paths: &[String]) -> Result<(), TestCaseError> {
    let project = ws.project_by_name("p").unwrap().id;
    let resource = |dotted: &str| -> ResourceId {
        ws.resources
            .iter()
            .find(|r| r.path.replace('/', ".") == dotted)
            .unwrap()
            .id
    };
    let mut summaries = summarize_workspace(ws);
    for s in &mut summaries {
        let dotted = ws.resource(s.resource).path.replace('/', ".");
        if let Some(i) = module_index(&dotted) {
            s.set_module_symbols(symbols(&specs[i]));
            // The script layer records each import's target.
            for (_, b) in &specs[i].names {
                let targets = match b {
                    Binding::Import(t) => vec![t.clone()],
                    Binding::Ambiguous(ts) => ts.clone(),
                    _ => Vec::new(),
                };
                s.references
                    .library
                    .extend(targets.into_iter().filter(|t| t.contains('.')));
            }
            s.references
                .library_star_imports
                .extend(specs[i].stars.iter().cloned());
            s.references
                .dynamic
                .library
                .extend(specs[i].stars.iter().cloned());
        } else if dotted == "user" {
            s.set_module_symbols(ModuleSymbols::default());
            s.references.library.extend(paths.iter().cloned());
        }
    }
    let index = ProjectIndex::build(ws, summaries);
    let resources: Vec<ResourceId> = MODULES.iter().map(|m| resource(m)).collect();
    for path in paths {
        let segs: Vec<&str> = path.split('.').collect();
        let mut model = Model {
            specs,
            reads: BTreeSet::new(),
            uncertain: false,
            through_variable: None,
            loading: Vec::new(),
        };
        let out = model.eval(&segs);
        // Every name the path reads is used, or its module may have any
        // name read.
        for (m, name) in &model.reads {
            let r = resources[*m];
            prop_assert!(
                index.external_symbol_usage_count(r, name) > 0
                    || index.module_used_as_value(project, r)
                    || index.module_escapes(project, r),
                "{path}: `{name}` of {} not counted; {specs:#?}",
                MODULES[*m]
            );
        }
        // The resolvers agree with each other.
        let full = index.resolve_library_path(project, path);
        let end = index.resolve_library(project, path);
        prop_assert_eq!(&full.end, &end, "{}", path);
        prop_assert_eq!(
            &index
                .library(project)
                .resolve(path, |r| index.module_symbols(r)),
            &end,
            "{}",
            path
        );
        // And with the model, where it is certain.
        let ends_at_variable = |e: &LibraryLookup<'_>| match (e, &model.through_variable) {
            (
                LibraryLookup::Symbol {
                    resource, symbol, ..
                },
                Some((m, name)),
            ) => *resource == resources[*m] && symbol.name == *name,
            _ => false,
        };
        if model.uncertain {
            prop_assert!(
                end == LibraryLookup::Unknown || ends_at_variable(&end),
                "{path}: {end:?} for an uncertain path ({out:?}); {specs:#?}"
            );
            continue;
        }
        let agrees = match (&out, &end) {
            (Out::Package(p), LibraryLookup::Package { dotted }) => p == dotted,
            (Out::Module(m), LibraryLookup::Module { resource, .. }) => *resource == resources[*m],
            (
                Out::Name(m, name, rest),
                LibraryLookup::Symbol {
                    resource,
                    symbol,
                    rest: r,
                },
            ) => *resource == resources[*m] && symbol.name == *name && rest == r,
            (
                Out::Missing(i),
                LibraryLookup::Missing { index, .. }
                | LibraryLookup::MissingAttribute { index, .. },
            ) => i == index,
            (Out::Missing(0), LibraryLookup::NotLibrary) => true,
            // A module fails to load: what its import names is reported
            // there.
            (Out::Fails, LibraryLookup::Unknown) => true,
            _ => false,
        };
        prop_assert!(agrees, "{path}: model {out:?}, index {end:?}; {specs:#?}");
    }
    Ok(())
}

/// What `path` names, statically (for generating paths only): a module or
/// a package, through imports and star imports, bounded.
fn object(specs: &[Spec], path: &str, depth: usize) -> Option<Out> {
    let segs: Vec<&str> = path.split('.').collect();
    let mut at = match module_index(segs[0]) {
        Some(m) => Out::Module(m),
        None if PACKAGES.contains(&segs[0]) => Out::Package(segs[0].to_owned()),
        None => return None,
    };
    for seg in &segs[1..] {
        at = member(specs, &at, seg, depth)?;
    }
    Some(at)
}

/// What the name `seg` of `at` names, statically.
fn member(specs: &[Spec], at: &Out, seg: &str, depth: usize) -> Option<Out> {
    if depth == 0 {
        return None;
    }
    match at {
        Out::Package(p) => {
            let child = format!("{p}.{seg}");
            match module_index(&child) {
                Some(m) => Some(Out::Module(m)),
                None if PACKAGES.contains(&child.as_str()) => Some(Out::Package(child)),
                None => None,
            }
        }
        Out::Module(m) => {
            let spec = &specs[*m];
            if let Some((_, b)) = spec.names.iter().find(|(n, _)| n == seg) {
                return match b {
                    Binding::Import(t) => object(specs, t, depth - 1),
                    _ => None,
                };
            }
            spec.stars.iter().rev().find_map(|star| {
                let target = object(specs, star, depth - 1)?;
                member(specs, &target, seg, depth - 1)
            })
        }
        _ => None,
    }
}

/// The names `at` may have, statically: its own, those its star imports
/// may bring, children of a package.
fn visible(specs: &[Spec], at: &Out, depth: usize, out: &mut Vec<String>) {
    if depth == 0 {
        return;
    }
    match at {
        Out::Package(p) => {
            for child in MODULES.iter().chain(PACKAGES) {
                if let Some(rest) = child.strip_prefix(&format!("{p}."))
                    && !rest.contains('.')
                    && !out.iter().any(|o| o == rest)
                {
                    out.push(rest.to_owned());
                }
            }
        }
        Out::Module(m) => {
            for (n, _) in &specs[*m].names {
                if !out.contains(n) {
                    out.push(n.clone());
                }
            }
            for star in &specs[*m].stars {
                if let Some(target) = object(specs, star, depth - 1) {
                    visible(specs, &target, depth - 1, out);
                }
            }
        }
        _ => {}
    }
}

/// Paths that follow what the generated library binds (review round 30,
/// B1: a path through star imports of modules named through imports
/// rarely comes up by chance): from a random module, each segment one of
/// the names the object may have, sometimes a missing one.
fn guided(specs: &[Spec], mut seed: u64) -> Vec<String> {
    let mut next = |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n as u64) as usize
    };
    let mut out = Vec::new();
    for _ in 0..8 {
        let root = MODULES[next(MODULES.len())];
        let mut path = root.to_owned();
        let mut at = Out::Module(module_index(root).expect("module"));
        for _ in 0..5 {
            let mut names = Vec::new();
            visible(specs, &at, 6, &mut names);
            names.extend(["json".to_owned(), "nope".to_owned()]);
            let seg = names[next(names.len())].clone();
            path.push('.');
            path.push_str(&seg);
            match member(specs, &at, &seg, 6) {
                Some(o) => at = o,
                None => break,
            }
        }
        out.push(path);
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 400,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn library_paths_resolve_like_jython(
        specs in prop::collection::vec(spec(), MODULES.len()),
        paths in paths(),
        seed in 1..u64::MAX,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let ws = workspace(dir.path());
        check(&ws, &specs, &paths)?;
        // Each guided path alone: what one path reads must not be credited
        // only because another path of the case reads it too.
        for path in guided(&specs, seed) {
            check(&ws, &specs, std::slice::from_ref(&path))?;
        }
    }
}

/// Fixed cases of the property: one import, aliases, a chain longer than
/// the bound of earlier rounds, a cycle through module objects.
#[test]
fn representative_chains() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(dir.path());
    let plain = |names: &[(&str, Binding)]| Spec {
        names: names
            .iter()
            .map(|(n, b)| ((*n).to_owned(), b.clone()))
            .collect(),
        stars: Vec::new(),
    };
    let import = |t: &str| Binding::Import(t.to_owned());
    let specs = vec![
        plain(&[("u", import("lib.a")), ("a", import("m1"))]),
        plain(&[("a", import("m0")), ("w", import("lib.a"))]),
        plain(&[("f", Binding::Function)]),
        plain(&[("json", Binding::Leaf), ("f", Binding::Function)]),
        plain(&[]),
        plain(&[]),
    ];
    let paths: Vec<String> = [
        "m0.u.json",
        "m0.u.f",
        "m0.u.nope",
        "m0.a.w.json",
        "m0.a.a.a.a.a.a.a.a.a.a.u.json",
        "m0.a.a.a.a.a.a.a.a.a.a.a.w.f",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    check(&ws, &specs, &paths).unwrap();
}

/// Review round 30 (B1): a star import of a module named through another
/// module's import (`from m0.a import *`, `m0` importing `lib.a`), passed
/// on by a further star import, reaches what that module re-exports.
#[test]
fn star_imports_through_imports() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(dir.path());
    let spec = |names: &[(&str, Binding)], stars: &[&str]| Spec {
        names: names
            .iter()
            .map(|(n, b)| ((*n).to_owned(), b.clone()))
            .collect(),
        stars: stars.iter().map(|s| (*s).to_owned()).collect(),
    };
    let import = |t: &str| Binding::Import(t.to_owned());
    // m0, m1, m2, lib.a, lib.b, pkg.c.d
    let specs = vec![
        spec(&[("a", import("lib.a"))], &[]),
        spec(&[], &["m0.a"]),
        spec(&[], &["m1"]),
        spec(&[("b", import("lib.b"))], &[]),
        spec(&[("json", Binding::Leaf)], &[]),
        spec(&[], &[]),
    ];
    let paths: Vec<String> = ["m2.b.json"].iter().map(|s| (*s).to_owned()).collect();
    check(&ws, &specs, &paths).unwrap();
}
