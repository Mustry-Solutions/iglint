//! Content-hash cache of per-file results.
//!
//! One JSON file per project root and iglint version holds, per file, a hash
//! of the file contents, its kind and its resolved settings, plus the
//! diagnostics found. A file whose hash matches is not analysed again.

use std::collections::BTreeMap;
use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use iglint_text::{TextEdit, TextRange, TextSize};
use serde::{Deserialize, Serialize};

use crate::diagnostic::{
    Applicability, Diagnostic, FileEdit, Fix, Fnv64, Label, Severity, SourceFile,
};
use crate::discovery::FileKind;
use crate::settings::{LinterSettings, ProjectSettings};

const FORMAT: u32 = 1;

#[derive(Serialize, Deserialize, Default)]
struct CacheFile {
    format: u32,
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
    path: PathBuf,
    entries: BTreeMap<String, Entry>,
    lock: Mutex<()>,
}

impl Cache {
    pub(crate) fn load(dir: &Path, settings: &ProjectSettings) -> Cache {
        let mut hasher = Fnv64::default();
        hasher.write(settings.root.to_string_lossy().as_bytes());
        let path = dir
            .join(env!("CARGO_PKG_VERSION"))
            .join(format!("{:016x}.json", hasher.finish()));
        let entries = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<CacheFile>(&bytes).ok())
            .filter(|file| file.format == FORMAT)
            .map(|file| file.files)
            .unwrap_or_default();
        Cache {
            path,
            entries,
            lock: Mutex::new(()),
        }
    }

    fn key(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }

    fn hash(kind: FileKind, settings: &LinterSettings, file: &SourceFile) -> String {
        let mut hasher = Fnv64::default();
        hasher.write(env!("CARGO_PKG_VERSION").as_bytes());
        hasher.write(kind.as_str().as_bytes());
        hasher.write(format!("{settings:?}").as_bytes());
        hasher.write(&(file.text().len() as u64).to_le_bytes());
        hasher.write(file.text().as_bytes());
        format!("{:016x}", hasher.finish())
    }

    pub(crate) fn get(
        &self,
        path: &Path,
        kind: FileKind,
        settings: &LinterSettings,
        file: &SourceFile,
    ) -> Option<Vec<Diagnostic>> {
        let entry = self.entries.get(&Self::key(path))?;
        if entry.hash != Self::hash(kind, settings, file) {
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
        diagnostics: &[Diagnostic],
    ) -> Option<Entry> {
        let diagnostics = diagnostics
            .iter()
            .map(|d| store(d, file))
            .collect::<Option<Vec<_>>>()?;
        Some(Entry {
            path: Self::key(path),
            hash: Self::hash(kind, settings, file),
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
            files: std::mem::take(&mut self.entries),
        };
        let Some(dir) = self.path.parent() else {
            return;
        };
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
        if let Some(root) = dir.parent() {
            let ignore = root.join(".gitignore");
            if !ignore.exists() {
                let _ = std::fs::write(ignore, "# Automatically created by iglint.\n*\n");
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
