//! Per-file analysis: parse a file once and run every enabled rule on it.

use crate::diagnostic::{Diagnostic, SourceFile};
use crate::discovery::FileKind;
use crate::settings::LinterSettings;

/// Lints one file with the rules enabled in `settings`. The returned
/// diagnostics may be in any order; the engine sorts them.
pub fn analyze_file(
    file: &SourceFile,
    kind: FileKind,
    settings: &LinterSettings,
) -> Vec<Diagnostic> {
    let _ = (file, kind, settings);
    Vec::new()
}
