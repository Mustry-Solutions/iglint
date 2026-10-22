//! Content-hash cache of per-file results.
//!
//! One JSON file per project root and iglint version holds, per file, a hash
//! of the file contents, its kind and its resolved settings, plus the
//! diagnostics found. A file whose hash matches is not analysed again.
//!
//! The cache file also records the build that wrote it ([`build_id`]); a
//! different build discards all entries, so a rebuilt binary with changed
//! rules never serves stale findings even when the version is unchanged.
//! The cache directory is marked with a `.gitignore` and a `CACHEDIR.TAG`
//! (<https://bford.info/cachedir/>) so that version control and backup tools
//! skip it.

use std::collections::BTreeMap;
use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use iglint_text::{TextEdit, TextRange, TextSize};
use serde::{Deserialize, Serialize};

use crate::diagnostic::{
    Applicability, Diagnostic, FileEdit, Fix, Fnv64, Label, Severity, SourceFile,
};
use crate::discovery::FileKind;
use crate::settings::{LinterSettings, ProjectSettings};

const FORMAT: u32 = 2;

const CACHEDIR_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55
# This file is a cache directory tag created by iglint.
# For information about cache directory tags, see:
#	https://bford.info/cachedir/
";

/// Identifies the running binary: the crate version, the commit and compile
/// time stamped in by `build.rs` (`IGLINT_BUILD_ID`) and the size and
/// modification time of the executable.
///
/// The build script only reruns when a file of this crate changes, so its id
/// alone misses rebuilds caused by edits to other crates (the parser, the
/// semantic model, the stubs) that change findings just as well. The linker
/// rewrites the executable on every such rebuild, so its metadata covers
/// them for the price of one `stat` per run. Hashing the executable's
/// contents would be exact but costs milliseconds on every run; a spurious
/// miss after copying the binary is harmless.
pub(crate) fn build_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        let mut hasher = Fnv64::default();
        if let Some(metadata) = std::env::current_exe()
            .ok()
            .and_then(|exe| std::fs::metadata(exe).ok())
        {
            hasher.write(&metadata.len().to_le_bytes());
            if let Some(since) = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            {
                hasher.write(&since.as_nanos().to_le_bytes());
            }
        }
        format!(
            "{}+{}+{:016x}",
            env!("CARGO_PKG_VERSION"),
            env!("IGLINT_BUILD_ID"),
            hasher.finish()
        )
    })
}

#[derive(Serialize, Deserialize, Default)]
struct CacheFile {
    format: u32,
    /// The [`build_id`] of the binary that wrote the file.
    build: String,
    files: BTreeMap<String, Entry>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Entry {
    #[serde(skip)]
    path: String,
    hash: String,
    diagnostics: Vec<CachedDiagnostic>,
}

#[derive(Clone, Serialize, Deserialize)]
struct CachedDiagnostic {
    rule: String,
    severity: String,
    message: String,
    range: (u32, u32),
    #[serde(default, skip_serializing_if = "Option::is_none")]
    node_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fingerprint_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ign_lint_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ign_lint_message: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    labels: Vec<(u32, u32, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fix: Option<CachedFix>,
}

#[derive(Clone, Serialize, Deserialize)]
struct CachedFix {
    message: String,
    applicability: String,
    edits: Vec<(u32, u32, String)>,
}

pub(crate) struct Cache {
    /// The cache directory, marked with `.gitignore` and `CACHEDIR.TAG`.
    dir: PathBuf,
    path: PathBuf,
    build: String,
    entries: BTreeMap<String, Entry>,
    lock: Mutex<()>,
}

impl Cache {
    pub(crate) fn load(dir: &Path, settings: &ProjectSettings) -> Cache {
        Self::load_with_build_id(dir, settings, build_id())
    }

    /// Like [`Cache::load`] for the binary identified by `build_id`; entries
    /// written by another build are discarded.
    pub(crate) fn load_with_build_id(
        dir: &Path,
        settings: &ProjectSettings,
        build_id: &str,
    ) -> Cache {
        let mut hasher = Fnv64::default();
        hasher.write(settings.root.to_string_lossy().as_bytes());
        let path = dir
            .join(env!("CARGO_PKG_VERSION"))
            .join(format!("{:016x}.json", hasher.finish()));
        let entries = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<CacheFile>(&bytes).ok())
            .filter(|file| file.format == FORMAT && file.build == build_id)
            .map(|file| file.files)
            .unwrap_or_default();
        Cache {
            dir: dir.to_path_buf(),
            path,
            build: build_id.to_owned(),
            entries,
            lock: Mutex::new(()),
        }
    }

    fn key(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }

    fn hash(kind: FileKind, settings: &LinterSettings, file: &SourceFile, context: u64) -> String {
        let mut hasher = Fnv64::default();
        hasher.write(env!("CARGO_PKG_VERSION").as_bytes());
        hasher.write(&context.to_le_bytes());
        hasher.write(kind.as_str().as_bytes());
        hasher.write(format!("{settings:?}").as_bytes());
        hasher.write(&(file.text().len() as u64).to_le_bytes());
        hasher.write(file.text().as_bytes());
        // The text has a substitute where the file is not UTF-8.
        for bad in file.invalid_utf8() {
            hasher.write(&bad.start().to_u32().to_le_bytes());
            hasher.write(&bad.end().to_u32().to_le_bytes());
        }
        format!("{:016x}", hasher.finish())
    }

    pub(crate) fn get(
        &self,
        path: &Path,
        kind: FileKind,
        settings: &LinterSettings,
        file: &SourceFile,
        context: u64,
    ) -> Option<Vec<Diagnostic>> {
        let entry = self.entries.get(&Self::key(path))?;
        if entry.hash != Self::hash(kind, settings, file, context) {
            return None;
        }
        entry
            .diagnostics
            .iter()
            .map(|d| restore(d, settings, file))
            .collect()
    }

    pub(crate) fn entry(
        &self,
        path: &Path,
        kind: FileKind,
        settings: &LinterSettings,
        file: &SourceFile,
        context: u64,
        diagnostics: &[Diagnostic],
    ) -> Option<Entry> {
        let diagnostics = diagnostics
            .iter()
            .map(|d| store(d, file))
            .collect::<Option<Vec<_>>>()?;
        Some(Entry {
            path: Self::key(path),
            hash: Self::hash(kind, settings, file, context),
            diagnostics,
        })
    }

    /// Adds `entries` and writes the cache file. Failures are ignored: the
    /// cache is an optimisation only.
    pub(crate) fn store(mut self, entries: Vec<Entry>) {
        if entries.is_empty() {
            return;
        }
        let _guard = self.lock.lock();
        for entry in entries {
            self.entries.insert(entry.path.clone(), entry);
        }
        let file = CacheFile {
            format: FORMAT,
            build: std::mem::take(&mut self.build),
            files: std::mem::take(&mut self.entries),
        };
        let Some(dir) = self.path.parent() else {
            return;
        };
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
        for (name, text) in [
            (".gitignore", "# Automatically created by iglint.\n*\n"),
            ("CACHEDIR.TAG", CACHEDIR_TAG),
        ] {
            let marker = self.dir.join(name);
            if !marker.exists() {
                let _ = std::fs::write(marker, text);
            }
        }
        if let Ok(json) = serde_json::to_vec(&file) {
            let _ = crate::engine::write_atomically(&self.path, &String::from_utf8_lossy(&json));
        }
    }
}

fn range(range: TextRange) -> (u32, u32) {
    (range.start().to_u32(), range.end().to_u32())
}

fn store(d: &Diagnostic, file: &SourceFile) -> Option<CachedDiagnostic> {
    if d.file.path() != file.path() || d.labels.iter().any(|l| l.file.path() != file.path()) {
        return None;
    }
    let fix = match &d.fix {
        None => None,
        Some(fix) => {
            if fix.edits.iter().any(|e| e.path != file.path()) {
                return None;
            }
            Some(CachedFix {
                message: fix.message.clone(),
                applicability: match fix.applicability {
                    Applicability::Safe => "safe",
                    Applicability::Unsafe => "unsafe",
                    Applicability::DisplayOnly => "display-only",
                }
                .to_owned(),
                edits: fix
                    .edits
                    .iter()
                    .map(|e| {
                        let (start, end) = range(e.edit.range);
                        (start, end, e.edit.content.clone())
                    })
                    .collect(),
            })
        }
    };
    Some(CachedDiagnostic {
        rule: d.rule.code().to_owned(),
        severity: d.severity.as_str().to_owned(),
        message: d.message.clone(),
        range: range(d.range),
        node_path: d.node_path.clone(),
        fingerprint_key: d.fingerprint_key.clone(),
        ign_lint_path: d.ign_lint_path.clone(),
        ign_lint_message: d.ign_lint_message.clone(),
        labels: d
            .labels
            .iter()
            .map(|l| {
                let (start, end) = range(l.range);
                (start, end, l.message.clone())
            })
            .collect(),
        fix,
    })
}

fn text_range(file: &SourceFile, (start, end): (u32, u32)) -> Option<TextRange> {
    (start <= end && end as usize <= file.text().len())
        .then(|| TextRange::new(TextSize::new(start), TextSize::new(end)))
}

fn restore(
    d: &CachedDiagnostic,
    settings: &LinterSettings,
    file: &SourceFile,
) -> Option<Diagnostic> {
    let rule = *settings.rules.keys().find(|r| r.code() == d.rule)?;
    let mut diagnostic = Diagnostic::new(
        rule,
        Severity::parse(&d.severity)?,
        d.message.clone(),
        file.clone(),
        text_range(file, d.range)?,
    );
    diagnostic.node_path = d.node_path.clone();
    diagnostic.fingerprint_key = d.fingerprint_key.clone();
    diagnostic.ign_lint_path = d.ign_lint_path.clone();
    diagnostic.ign_lint_message = d.ign_lint_message.clone();
    for (start, end, message) in &d.labels {
        diagnostic.labels.push(Label {
            file: file.clone(),
            range: text_range(file, (*start, *end))?,
            message: message.clone(),
        });
    }
    if let Some(fix) = &d.fix {
        let edits = fix
            .edits
            .iter()
            .map(|(start, end, content)| {
                Some(FileEdit {
                    path: file.path().to_path_buf(),
                    edit: TextEdit::replace(text_range(file, (*start, *end))?, content.clone()),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        diagnostic.fix = Some(Fix {
            message: fix.message.clone(),
            applicability: match fix.applicability.as_str() {
                "safe" => Applicability::Safe,
                "unsafe" => Applicability::Unsafe,
                _ => Applicability::DisplayOnly,
            },
            edits,
        });
    }
    Some(diagnostic)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(dir: &Path, settings: &ProjectSettings, build_id: &str, file: &SourceFile) {
        let cache = Cache::load_with_build_id(dir, settings, build_id);
        let entry = cache
            .entry(file.path(), FileKind::Python, &settings.base, file, 0, &[])
            .unwrap();
        cache.store(vec![entry]);
    }

    fn hits(
        dir: &Path,
        settings: &ProjectSettings,
        linter: &LinterSettings,
        build_id: &str,
        file: &SourceFile,
    ) -> bool {
        Cache::load_with_build_id(dir, settings, build_id)
            .get(file.path(), FileKind::Python, linter, file, 0)
            .is_some()
    }

    #[test]
    fn entries_are_keyed_by_build_and_settings() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join(".iglint_cache");
        let settings = ProjectSettings {
            root: temp.path().to_path_buf(),
            ..ProjectSettings::default()
        };
        let file = SourceFile::new(temp.path().join("a.py"), "x = 1\n");
        stored(&dir, &settings, "build-a", &file);

        assert!(hits(&dir, &settings, &settings.base, "build-a", &file));
        assert!(!hits(&dir, &settings, &settings.base, "build-b", &file));
        let strict = LinterSettings {
            strict: true,
            ..(*settings.base).clone()
        };
        assert!(!hits(&dir, &settings, &strict, "build-a", &file));

        // A store by another build replaces the old entries.
        stored(
            &dir,
            &settings,
            "build-b",
            &SourceFile::new(temp.path().join("b.py"), ""),
        );
        assert!(!hits(&dir, &settings, &settings.base, "build-b", &file));
        assert!(!hits(&dir, &settings, &settings.base, "build-a", &file));
    }

    #[test]
    fn cache_directory_is_marked() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join(".iglint_cache");
        let settings = ProjectSettings {
            root: temp.path().to_path_buf(),
            ..ProjectSettings::default()
        };
        let file = SourceFile::new(temp.path().join("a.py"), "x = 1\n");
        stored(&dir, &settings, "build-a", &file);

        let ignore = std::fs::read_to_string(dir.join(".gitignore")).unwrap();
        assert!(ignore.lines().any(|line| line == "*"));
        let tag = std::fs::read_to_string(dir.join("CACHEDIR.TAG")).unwrap();
        assert!(tag.starts_with("Signature: 8a477f597d28d172789f06886806bc55"));

        // Existing markers are left alone.
        std::fs::write(dir.join(".gitignore"), "*\n# mine\n").unwrap();
        stored(&dir, &settings, "build-a", &file);
        let ignore = std::fs::read_to_string(dir.join(".gitignore")).unwrap();
        assert_eq!(ignore, "*\n# mine\n");
    }

    #[test]
    fn build_id_names_the_version() {
        assert!(build_id().starts_with(concat!(env!("CARGO_PKG_VERSION"), "+")));
        assert_eq!(build_id(), build_id());
    }
}
