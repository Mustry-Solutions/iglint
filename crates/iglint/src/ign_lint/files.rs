//! File selection and whitelist files, as ign-lint does them (sections 8.2
//! and 8.3).

use std::collections::BTreeSet;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use iglint_linter::discovery::absolute;

/// Python `glob.glob(pattern, recursive=True)` relative to `cwd`: `**` spans
/// directories, wildcards do not match a leading `.`. Results are sorted
/// (Python returns them in directory order) and returned as written: relative
/// to `cwd` for relative patterns.
pub fn python_glob(pattern: &str, cwd: &Path) -> Vec<String> {
    let options = glob::MatchOptions {
        case_sensitive: true,
        require_literal_separator: true,
        require_literal_leading_dot: true,
    };
    let relative = !Path::new(pattern).is_absolute();
    let full = if relative {
        let base = glob::Pattern::escape(&cwd.to_string_lossy());
        format!("{}/{}", base.trim_end_matches('/'), pattern)
    } else {
        pattern.to_owned()
    };
    let Ok(paths) = glob::glob_with(&full, options) else {
        return Vec::new();
    };
    let mut out: Vec<String> = paths
        .filter_map(Result::ok)
        .map(|path| {
            if relative {
                path.strip_prefix(cwd)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| path.to_string_lossy().into_owned())
            } else {
                path.to_string_lossy().into_owned()
            }
        })
        .collect();
    out.sort();
    out
}

/// Files selected by the command line, in processing order, as displayed.
pub struct Selection {
    pub files: Vec<String>,
    /// Lines printed while selecting (warnings, deprecation notice).
    pub messages: Vec<String>,
}

/// Merges `--files` and positional files (section 8.2).
pub fn select(files_option: Option<&str>, positional: &[String], cwd: &Path) -> Selection {
    let mut messages = Vec::new();
    let mut candidates: Vec<String> = Vec::new();
    if !positional.is_empty() {
        if let Some(first) = files_option {
            messages.push(format!(
                "⚠️  Deprecation: '--files {first} FILE ...' is deprecated; pass files as positional arguments or as one comma-separated --files value (ignition-lint)"
            ));
            candidates.push(first.to_owned());
        }
        for file in positional {
            candidates.push(file.clone());
        }
        let mut explicit = Vec::new();
        for file in candidates {
            if cwd.join(&file).is_file() {
                explicit.push(file);
            } else {
                messages.push(format!("Warning: File {file} does not exist"));
            }
        }
        return Selection {
            files: dedupe(explicit, cwd),
            messages,
        };
    }
    let patterns = files_option.unwrap_or("**/view.json");
    let mut matched = Vec::new();
    for pattern in patterns.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        for path in python_glob(pattern, cwd) {
            if Path::new(&path)
                .file_name()
                .is_some_and(|n| n == "view.json")
                && cwd.join(&path).is_file()
            {
                matched.push(path);
            }
        }
    }
    Selection {
        files: dedupe(matched, cwd),
        messages,
    }
}

fn dedupe(files: Vec<String>, cwd: &Path) -> Vec<String> {
    let mut seen = BTreeSet::new();
    files
        .into_iter()
        .filter(|f| seen.insert(absolute(&cwd.join(f))))
        .collect()
}

/// `--generate-whitelist`: writes (or previews) the whitelist and returns the
/// exit status code.
pub fn generate_whitelist(
    patterns: &[String],
    output: &str,
    append: bool,
    dry_run: bool,
    cwd: &Path,
    out: &mut dyn Write,
) -> io::Result<u8> {
    let mut matched = BTreeSet::new();
    for pattern in patterns {
        for path in python_glob(pattern, cwd) {
            if cwd.join(&path).is_file() {
                matched.insert(path);
            }
        }
    }
    if dry_run {
        writeln!(out, "🔍 Would add {} files to whitelist:", matched.len())?;
        for path in matched.iter().take(20) {
            writeln!(out, "  {path}")?;
        }
        if matched.len() > 20 {
            writeln!(out, "  ... and {} more files", matched.len() - 20)?;
        }
        return Ok(0);
    }
    let target = cwd.join(output);
    let existing: Option<BTreeSet<String>> = if append {
        std::fs::read_to_string(&target).ok().map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(str::to_owned)
                .collect()
        })
    } else {
        None
    };
    let new: BTreeSet<String> = match &existing {
        Some(existing) => matched.difference(existing).cloned().collect(),
        None => matched.clone(),
    };
    let mut all = matched.clone();
    if let Some(existing) = &existing {
        all.extend(existing.iter().cloned());
    }
    let mut text = String::from(
        "# Ignition-lint whitelist - files to ignore during linting\n\
         # Lines starting with # are comments\n\
         # One file path per line (relative to repository root)\n",
    );
    text.push_str(&format!("# Generated: {} files added\n", new.len()));
    if let Some(existing) = &existing {
        text.push_str(&format!("# Existing: {} files\n", existing.len()));
    }
    text.push('\n');
    for path in &all {
        text.push_str(path);
        text.push('\n');
    }
    let written = target
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&target, text));
    if let Err(error) = written {
        writeln!(out, "❌ Error writing whitelist {output}: {error}")?;
        return Ok(1);
    }
    match &existing {
        Some(existing) => {
            writeln!(out, "✓ Whitelist appended to: {output}")?;
            writeln!(out, "  Total files: {}", all.len())?;
            writeln!(out, "  New files: {}", new.len())?;
            writeln!(out, "  Existing files: {}", existing.len())?;
        }
        None => {
            writeln!(out, "✓ Whitelist generated: {output}")?;
            writeln!(out, "  Total files: {}", all.len())?;
        }
    }
    Ok(0)
}

/// Resolved whitelist entries (absolute paths).
pub fn whitelist(path: &str, cwd: &Path) -> io::Result<BTreeSet<PathBuf>> {
    Ok(iglint_config::compat::read_whitelist(&cwd.join(path), cwd)?
        .into_iter()
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_skips_hidden_and_recurses() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for path in [
            "a/view.json",
            "a/b/view.json",
            ".hidden/view.json",
            "view.json",
            "a/x.json",
        ] {
            let full = root.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, "{}").unwrap();
        }
        assert_eq!(
            python_glob("**/view.json", root),
            vec!["a/b/view.json", "a/view.json", "view.json"]
        );
        assert_eq!(
            python_glob("a/*.json", root),
            vec!["a/view.json", "a/x.json"]
        );
        let selection = select(Some("a/*.json, view.json"), &[], root);
        assert_eq!(selection.files, vec!["a/view.json", "view.json"]);
    }
}
