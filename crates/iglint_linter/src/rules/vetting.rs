//! Whether code uses only vetted facilities, for safe fixes that depend on
//! what code can read (see `iglint_python_semantic::vetted`).

use iglint_project::library::LibraryLookup;
use iglint_project::script::EmbeddedScript;
use iglint_project::{FileId, ProjectId, ProjectIndex, ResourceId};
use iglint_python::ast::Stmt;
use iglint_python_semantic::SemanticModel;
use iglint_text::TextRange;
use rustc_hash::FxHashSet;

use crate::checker::{Checker, FileTable};
use crate::diagnostic::{Applicability, Diagnostic, ReasonSite, UnsafeReason};

/// Where code is not vetted ([`unknown_code_at`]).
#[derive(Clone, Copy, Debug)]
pub(crate) enum UnknownAt {
    /// In the script itself, at this range of its source.
    Here(TextRange),
    /// In a project library module the script imports or calls, or a
    /// vetted module name that a library package shadows.
    Library,
}

/// Where `body` (a script, `model` its semantic model) or a project
/// library module it imports or uses (transitively) uses something that
/// is not vetted: a module, Java class, builtin or module attribute off
/// the allowlists, an import that is not a vetted module or a project
/// library module, or a vetted module name that a project library package
/// shadows. Without the project index every import and library name is
/// unknown. The first use in the script itself, else the library code.
pub(crate) fn unknown_code_at(
    cx: &Checker<'_, '_>,
    model: &SemanticModel<'_>,
    body: &[Stmt],
) -> Option<UnknownAt> {
    let unknown = iglint_python_semantic::vetted::unknown_code(model);
    // `system.*` members the 8.3 stubs do not list count unless the
    // project vets them; without the index none is.
    let vetted_system = cx
        .index()
        .map_or(&[][..], |index| &index.vetted().system_functions[..]);
    if let Some(range) = unknown.first(vetted_system) {
        return Some(UnknownAt::Here(range));
    }
    let names = crate::script_facts::access::library_names_used(model, body);
    if unknown.imports.is_empty() && names.is_empty() && unknown.vetted_imports.is_empty() {
        return None;
    }
    let Some((index, project)) = cx.project_index() else {
        return match unknown.imports.first() {
            Some((_, range)) => Some(UnknownAt::Here(*range)),
            None => (!names.is_empty()).then_some(UnknownAt::Library),
        };
    };
    let (globals, complete) = index.script_library_globals(project);
    if !complete
        || unknown
            .vetted_imports
            .iter()
            .any(|r| globals.contains(&r.as_str()))
    {
        return Some(UnknownAt::Library);
    }
    let mut seen = FxHashSet::default();
    for (name, range) in &unknown.imports {
        if !library_vetted(index, project, name, true, &mut seen) {
            // A module that is not a library module is unknown here; a
            // library module whose code is not vetted, there.
            return Some(match index.resolve_library(project, name) {
                LibraryLookup::Module { .. } | LibraryLookup::Symbol { .. } => UnknownAt::Library,
                _ => UnknownAt::Here(*range),
            });
        }
    }
    names
        .iter()
        .any(|name| !library_vetted(index, project, name, false, &mut seen))
        .then_some(UnknownAt::Library)
}

/// Why a fix is unsafe when the script `script` (in `file`) uses code that
/// is not vetted ([`unknown_code_at`]).
pub(crate) fn script_reason(
    cx: &Checker<'_, '_>,
    file: FileId,
    script: &EmbeddedScript<'_>,
    at: UnknownAt,
) -> UnsafeReason {
    match at {
        UnknownAt::Here(range) => {
            let site = cx.file(file).map(|source| {
                let start = script.file_range(range).start();
                let at = source.line_index().line_column(start, source.text());
                ReasonSite {
                    path: source.path().to_path_buf(),
                    line: at.line,
                    column: at.column,
                    what: crate::script_facts::access::snippet(&script.source, range),
                }
            });
            UnsafeReason {
                message: "the script uses code that is not vetted".to_owned(),
                site,
                workspace: false,
            }
        }
        UnknownAt::Library => UnsafeReason {
            message: "the script uses a library module that is not vetted; first such code in \
                      the workspace"
                .to_owned(),
            site: None,
            workspace: true,
        },
    }
}

/// Why a fix is unsafe when the script `script` (in `file`) has an escape
/// route at `range` ([`SemanticModel::frame_escape`]): a first hop into
/// reflection or the interpreter (`o.type`, a computed `getattr`) that
/// does not silence findings, but through which code may read names.
pub(crate) fn escape_reason(
    cx: &Checker<'_, '_>,
    file: FileId,
    script: &EmbeddedScript<'_>,
    range: TextRange,
) -> UnsafeReason {
    let mut reason = script_reason(cx, file, script, UnknownAt::Here(range));
    reason.message = "the script uses reflection or introspection (a Java member or attribute \
                      that leads to reflection or the interpreter, or an attribute looked up by \
                      a computed name), through which code may read names iglint does not \
                      follow"
        .to_owned();
    reason
}

/// Why a fix is unsafe under the closed-world rule
/// ([`workspace_has_unknown_code`]): the site, the first use in the
/// workspace, is filled in after the cache from the project index.
pub(crate) fn workspace_reason(cx: &Checker<'_, '_>) -> UnsafeReason {
    if cx.index().is_none_or(|index| index.closed_world_unknown()) {
        return UnsafeReason {
            message: CLOSED_WORLD_UNKNOWN.to_owned(),
            site: None,
            workspace: false,
        };
    }
    if let Some(unread) = cx.index().and_then(|index| index.unread_directory()) {
        return UnsafeReason {
            message: unread_reason("closed world unknown", unread),
            site: None,
            workspace: false,
        };
    }
    UnsafeReason {
        message: "closed world: code in the workspace is not vetted, reads frames, reaches \
                  other views or runs in the browser, first"
            .to_owned(),
        site: None,
        workspace: true,
    }
}

/// Why a fix that needs the closed world is unsafe when no workspace root
/// was found around a linted path.
pub(crate) const CLOSED_WORLD_UNKNOWN: &str = "closed world unknown: no workspace root (a \
     configuration file, a `projects/` or `config/resources/` layout, a Git work tree or the \
     working directory) contains the file, so code elsewhere on its gateway is not known";

/// Why a fix is unsafe because a directory of the workspace was not read
/// (`iglint_project::workspace::Workspace::unread`), after `what`.
pub(crate) fn unread_reason(
    what: &str,
    unread: &iglint_project::workspace::WorkspaceProblem,
) -> String {
    format!(
        "{what}: `{}` could hold Ignition resources but was not read ({}), so code there is not \
         known",
        crate::discovery::slash_path(&unread.path),
        unread.message
    )
}

/// Why a fix of the unit is unsafe because of code that is not vetted:
/// the unit's own first script that uses some, else the closed-world
/// rule. `None` when neither applies.
pub(crate) fn unit_reason(cx: &Checker<'_, '_>) -> Option<UnsafeReason> {
    use rayon::prelude::*;
    // In parallel, the first script in order (review round 30: a view with
    // two thousand scripts took 20 ms one after the other, in the language
    // server's time to publish an edit).
    let scripts = cx.unit().data.scripts();
    let found = scripts
        .par_iter()
        .zip(cx.parsed_scripts().par_iter())
        .find_map_first(|((file, script), parsed)| {
            if !parsed.errors.is_empty() {
                return None;
            }
            let model = SemanticModel::build(
                &parsed.module,
                &iglint_python_semantic::SemanticOptions {
                    injected: script.implicit_globals().collect(),
                    unknown_globals: false,
                    module_has_name: script.has_module_name(),
                },
            );
            unknown_code_at(cx, &model, &parsed.module.body).map(|at| (*file, *script, at))
        });
    if let Some((file, script, at)) = found {
        return Some(script_reason(cx, file, script, at));
    }
    workspace_has_unknown_code(cx).then(|| workspace_reason(cx))
}

/// Whether the library name `dotted` is vetted code: a package, or a
/// module or a name of a module whose code is vetted. An import of
/// anything else is not; a used name that does not exist is (using it
/// raises).
fn library_vetted(
    index: &ProjectIndex<'_>,
    project: ProjectId,
    dotted: &str,
    import: bool,
    seen: &mut FxHashSet<ResourceId>,
) -> bool {
    if dotted.is_empty() {
        // A relative import.
        return false;
    }
    let path = index.resolve_library_path(project, dotted);
    match path.end {
        LibraryLookup::Package { .. } => true,
        // Every module on the way runs (`m0.util.f` imports `m0`, whose
        // import of `lib.util` runs that).
        LibraryLookup::Module { .. } | LibraryLookup::Symbol { .. } => path
            .modules
            .iter()
            .all(|&resource| module_vetted(index, project, resource, seen)),
        LibraryLookup::NotLibrary => !import,
        LibraryLookup::Missing { .. }
        | LibraryLookup::HiddenByModule { .. }
        | LibraryLookup::MissingAttribute { .. } => !import,
        LibraryLookup::Unknown => false,
    }
}

/// Whether the code of library module `resource`, and of every library
/// module it imports or uses, is vetted.
fn module_vetted(
    index: &ProjectIndex<'_>,
    project: ProjectId,
    resource: ResourceId,
    seen: &mut FxHashSet<ResourceId>,
) -> bool {
    if !seen.insert(resource) {
        return true;
    }
    let Some(symbols) = index.module_symbols(resource) else {
        return false;
    };
    let (globals, _) = index.script_library_globals(project);
    !symbols.unknown_code
        && symbols
            .system
            .iter()
            .all(|path| iglint_stubs::system_path_listed(path, &index.vetted().system_functions))
        && symbols
            .imports
            .iter()
            .all(|name| library_vetted(index, project, name, true, seen))
        && symbols
            .library_names
            .iter()
            .filter(|n| globals.contains(&n.split('.').next().unwrap_or(n)))
            .all(|name| library_vetted(index, project, name, false, seen))
}

/// Whether any script anywhere in the workspace uses code that is not
/// vetted or reads a frame or the interpreter
/// ([`iglint_project::ProjectIndex::has_any_unknown_code`]). The Ignition
/// runtime is shared, so such code in any project or gateway-scoped script
/// can observe any module's names and any frame's locals (through
/// `sys._current_frames`, reflection or a `cPickle` opcode). Safe fixes
/// that assume a name or value is not observed (`SCR102`, `VIEW015`,
/// `NAME001`) are only safe when the whole closed world is free of it.
/// It holds for the scripts of gateway collections and for files outside
/// any project too (the runtime is the same), and without a project index
/// (which only rules of the local phase lack) it fails closed.
pub(crate) fn workspace_has_unknown_code(cx: &Checker<'_, '_>) -> bool {
    cx.index().is_none_or(|index| index.has_any_unknown_code())
}

/// The first use of code that is not vetted, or that reads a frame, in the
/// workspace ([`ProjectIndex::first_unknown_code`]), placed in its file.
pub(crate) fn first_unknown_site(
    index: &ProjectIndex<'_>,
    files: &FileTable,
) -> Option<ReasonSite> {
    let site = index.first_unknown_code()?;
    let source = files.get(site.file)?;
    let at = source
        .line_index()
        .line_column(site.range.start(), source.text());
    Some(ReasonSite {
        path: source.path().to_path_buf(),
        line: at.line,
        column: at.column,
        what: site.what.clone(),
    })
}

/// Fills in the site of closed-world reasons ([`UnsafeReason::workspace`])
/// that have none: findings computed now, and findings restored from the
/// cache, which never keeps a site of another file.
pub(crate) fn complete_reasons(diagnostics: &mut [Diagnostic], site: Option<&ReasonSite>) {
    let Some(site) = site else { return };
    for d in diagnostics {
        if let Some(reason) = d.fix.as_mut().and_then(|f| f.unsafe_reason.as_mut())
            && reason.workspace
            && reason.site.is_none()
        {
            reason.site = Some(site.clone());
        }
    }
}

/// Withholds fixes of files that other paths share, before the file-read
/// gate ([`gate_file_reads`] runs it first, so it runs on every path that
/// reports or applies fixes, and `assume-scripts-do-not-read-workspace-files`
/// does not lift it):
///
/// * a fix that edits a file behind a symbolic link to a directory
///   ([`iglint_project::Workspace::directory_links`]) is dropped: the
///   gateway does not load the file at that path, and the edit would land
///   in the directory the link points to, which may be outside the
///   repository and loaded elsewhere. `REF009` reports the file;
/// * so is a fix that edits a file inside a project or collection that
///   the workspace walk did not reach
///   ([`iglint_project::Workspace::unreached_container`]: an editor
///   folder whose walk skips the hidden directory that holds the file):
///   the fix was analysed without the file's own project and gateway.
///   `REF009` reports the file;
/// * a safe fix that edits a file that is a symbolic link, the target of a
///   symbolic link of the workspace, or a file with hard links
///   ([`iglint_project::Workspace::shared_file`]) is unsafe, with the
///   other paths named. The gateway reads files through links (verified
///   on 8.3.3), so every resource that reaches the file runs the edited
///   text, and the analysis of the fix only knows the resource of the
///   linted path (an import that is unused in one project's module may be
///   used through another project's link to the same file).
pub(crate) fn gate_shared_files(diagnostics: &mut [Diagnostic], ws: &iglint_project::Workspace) {
    let mut seen: rustc_hash::FxHashMap<std::path::PathBuf, Option<String>> =
        rustc_hash::FxHashMap::default();
    for d in diagnostics {
        let Some(fix) = d.fix.as_ref() else { continue };
        if fix.edits.iter().any(|e| {
            ws.directory_link_above(&e.path).is_some() || ws.unreached_container(&e.path).is_some()
        }) {
            d.fix = None;
            continue;
        }
        if fix.applicability != Applicability::Safe {
            continue;
        }
        let reason = fix.edits.iter().find_map(|e| {
            seen.entry(e.path.clone())
                .or_insert_with(|| {
                    let shared = ws.shared_file(&e.path);
                    shared
                        .is_shared()
                        .then(|| shared_file_reason(&e.path, &shared))
                })
                .clone()
        });
        if let (Some(reason), Some(fix)) = (reason, d.fix.as_mut()) {
            fix.applicability = Applicability::Unsafe;
            fix.unsafe_reason = Some(UnsafeReason {
                message: reason,
                site: None,
                workspace: false,
            });
        }
    }
}

/// Why a fix of `path`, a file shared with other paths, is unsafe.
pub(crate) fn shared_file_reason(
    path: &std::path::Path,
    shared: &iglint_project::workspace::SharedFile,
) -> String {
    use crate::discovery::slash_path;
    let mut others: Vec<String> = Vec::new();
    let is_target = |p: &std::path::Path| {
        shared
            .link_target
            .as_deref()
            .is_some_and(|t| std::fs::canonicalize(p).is_ok_and(|c| c == t))
    };
    if let Some(target) = &shared.link_target
        && !shared.same_file.iter().any(|p| is_target(p))
    {
        others.push(format!(
            "`{}` (the file this symbolic link points to)",
            slash_path(target)
        ));
    }
    others.extend(shared.same_file.iter().map(|p| {
        let what = if std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()) {
            "a symbolic link to it"
        } else if is_target(p) {
            "the file this symbolic link points to"
        } else {
            "a hard link"
        };
        format!("`{}` ({what})", slash_path(p))
    }));
    if shared.other_hard_links > 0 {
        others.push(format!(
            "{} other hard link{} outside the workspace",
            shared.other_hard_links,
            if shared.other_hard_links == 1 {
                ""
            } else {
                "s"
            }
        ));
    }
    format!(
        "the fix edits `{}`, which is the same file as {}: every resource that reaches the file \
         runs the edited text, and the fix is analysed only for this path",
        slash_path(path),
        others.join(", ")
    )
}

/// Why every fix is unsafe while code in the workspace may read the files
/// a fix edits ([`ProjectIndex::may_read_workspace_files`]).
pub(crate) const FILE_READ_REASON: &str = "code in the workspace may read the files a fix edits \
     (set `assume-scripts-do-not-read-workspace-files = true` under `[fix]` if no script \
     does)";

/// Makes every safe fix of `diagnostics` unsafe when code in the workspace
/// may read the files a fix edits, unless `assume_no_reads` (the project
/// setting `[fix] assume-scripts-do-not-read-workspace-files`). Every fix
/// edits a workspace file, and a script that reads it (`open(path).read()`,
/// its size or modification time) observes the edit, however neutral the
/// edit is for the code that runs: removing an unused import, trailing
/// whitespace or a suppression comment, or a custom property. Run after
/// [`complete_reasons`] on every path that reports or applies fixes
/// (`--fix`, `--diff`, the language server), after the cache.
pub(crate) fn gate_file_reads(
    diagnostics: &mut [Diagnostic],
    index: &ProjectIndex<'_>,
    files: &FileTable,
    assume_no_reads: bool,
) {
    gate_shared_files(diagnostics, index.workspace());
    if assume_no_reads || !index.may_read_workspace_files() {
        return;
    }
    let site = index.first_file_read().and_then(|site| {
        let source = files.get(site.file)?;
        let at = source
            .line_index()
            .line_column(site.range.start(), source.text());
        Some(ReasonSite {
            path: source.path().to_path_buf(),
            line: at.line,
            column: at.column,
            what: site.what.clone(),
        })
    });
    let unread = index.unread_directory().filter(|_| site.is_none());
    for d in diagnostics {
        if let Some(fix) = d.fix.as_mut()
            && fix.applicability == Applicability::Safe
        {
            fix.applicability = Applicability::Unsafe;
            fix.unsafe_reason = Some(UnsafeReason {
                message: match unread {
                    Some(unread) => unread_reason(FILE_READ_REASON, unread),
                    None if site.is_some() => format!("{FILE_READ_REASON}, first"),
                    None => FILE_READ_REASON.to_owned(),
                },
                site: site.clone(),
                workspace: true,
            });
        }
    }
}
